//! mpv 会话：spawn mpv.exe（--wid 指向嵌入子窗口）+ JSON IPC 命名管道控制。
//! 控制协议与 Fntv-Plus 的 node-mpv-2 相同（newline-delimited JSON，request_id 应答配对）。
//! 属性变化以 `mpv-event` Tauri 事件推给前端自绘控制栏。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot, Mutex};

pub struct MpvSession {
    pub embed_hwnd: isize,
    child: Mutex<Child>,
    tx: mpsc::UnboundedSender<String>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
}

impl MpvSession {
    pub async fn launch(
        app: AppHandle,
        exe: &str,
        embed_hwnd: isize,
    ) -> Result<Arc<MpvSession>, String> {
        let pipe = format!(
            r"\\.\pipe\fntv-tauri-mpv-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let args = [
            "--no-config".to_string(),
            format!("--wid={}", embed_hwnd),
            "--vo=gpu-next".to_string(),
            "--hwdec=auto-safe".to_string(),
            "--input-ipc-server".to_string(),
            pipe.clone(),
            "--osc=no".to_string(),
            "--input-default-bindings=no".to_string(),
            "--input-vo-keyboard=no".to_string(),
            "--keep-open=yes".to_string(),
            "--idle=yes".to_string(),
            "--force-window=immediate".to_string(),
            "--network-timeout=180".to_string(),
            "--cache=yes".to_string(),
            "--cache-secs=30".to_string(),
            "--cache-pause=yes".to_string(),
            "--demuxer-cache-wait=no".to_string(),
            "--fullscreen=no".to_string(),
            "--volume=100".to_string(),
        ];
        let child = tokio::process::Command::new(exe)
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("启动 mpv 失败 ({}): {}", exe, e))?;

        // 等待 IPC 管道就绪
        let mut client = None;
        for _ in 0..60 {
            match ClientOptions::new().open(&pipe) {
                Ok(c) => {
                    client = Some(c);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(150)).await,
            }
        }
        let client = client.ok_or_else(|| "连接 mpv IPC 管道超时".to_string())?;

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let mut writer = client.try_clone().map_err(|e| format!("克隆管道失败: {}", e))?;
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if writer.write_all(b"\n").await.is_err() {
                    break;
                }
                let _ = writer.flush().await;
            }
        });

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let p2 = pending.clone();
        let app2 = app;
        tokio::spawn(async move {
            let mut lines = BufReader::new(client).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&line) {
                            if v.get("event").is_some() {
                                let _ = app2.emit("mpv-event", v);
                            } else if let Some(id) = v.get("request_id").and_then(|i| i.as_u64()) {
                                let mut map = p2.lock().await;
                                if let Some(s) = map.remove(&id) {
                                    let _ = s.send(v);
                                }
                            }
                        }
                    }
                    _ => break,
                }
            }
        });

        let session = Arc::new(MpvSession {
            embed_hwnd,
            child: Mutex::new(child),
            tx,
            next_id: AtomicU64::new(1),
            pending,
        });

        // 观察控制栏需要的属性：time-pos / duration / pause / speed / volume
        let _ = session.command(json!(["observe_property", 1, "time-pos"])).await;
        let _ = session.command(json!(["observe_property", 2, "duration"])).await;
        let _ = session.command(json!(["observe_property", 3, "pause"])).await;
        let _ = session.command(json!(["observe_property", 4, "speed"])).await;
        let _ = session.command(json!(["observe_property", 5, "volume"])).await;
        Ok(session)
    }

    pub async fn command(&self, cmd: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (otx, orx) = oneshot::channel();
        self.pending.lock().await.insert(id, otx);
        let msg = json!({ "command": cmd, "request_id": id }).to_string();
        self.tx
            .send(msg)
            .map_err(|_| "mpv 进程已退出".to_string())?;
        match tokio::time::timeout(std::time::Duration::from_secs(5), orx).await {
            Ok(Ok(v)) => {
                let err = v
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("success");
                if err == "success" {
                    Ok(v)
                } else {
                    Err(err.to_string())
                }
            }
            _ => Err("mpv 响应超时".into()),
        }
    }

    /// 进程是否已退出
    pub async fn is_dead(&self) -> bool {
        let mut c = self.child.lock().await;
        matches!(c.try_wait(), Ok(Some(_)))
    }

    pub async fn shutdown(&self) {
        if let Ok(mut c) = self.child.try_lock() {
            let _ = c.kill().await;
        }
        crate::win_embed::destroy(self.embed_hwnd);
    }
}
