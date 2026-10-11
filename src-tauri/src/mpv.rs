//! mpv 会话：spawn mpv.exe（--wid 指向嵌入子窗口）+ JSON IPC 命名管道控制。
//! 控制协议与 Fntv-Plus 的 node-mpv-2 相同（newline-delimited JSON，request_id 应答配对）。
//!
//! 连接模型：tokio 的 NamedPipeClient 不支持 try_clone，因此开两条连接——
//! 连接 A 只读事件（mpv 向所有客户端广播事件）；连接 B 经 tokio::io::split 拆成
//! 读写两半，写半发命令、读半收本连接的应答。

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
    tx: mpsc::UnboundedSender<(Value, oneshot::Sender<Value>)>,
}

async fn try_open_pipe(pipe: &str) -> Option<tokio::net::windows::named_pipe::NamedPipeClient> {
    // 单次尝试：由 launch 的重试环统一控节奏（便于同时观察子进程是否已退出）
    ClientOptions::new().open(pipe).ok()
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
            // ⚠ 必须用等号形式：空格分隔形式会让 mpv 静默 abort（exit 1 零输出，连 --log-file 都不建）
            format!("--input-ipc-server={}", pipe.clone()),
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
            // mpv 内部日志恒定 verbose 写文件（主仓库同款），不依赖 stderr——诊断兜底
            "--log-file".to_string(),
            std::env::temp_dir().join("fntv-tauri2-mpv.log").to_string_lossy().to_string(),
        ];
        // 调试：记录完整命令行（定位 spawn 环境差异）
        {
            let argv_log = std::env::temp_dir().join("fntv-tauri2-mpv-spawn.log");
            let _ = std::fs::write(&argv_log, format!("exe={}
args={:?}
", exe, &args[..]));
        }
        let mut child = tokio::process::Command::new(exe)
            .args(&args)
            .stdout(std::process::Stdio::null())
            // stderr 直写文件：进程退出后的错误文本时序无关，诊断最可靠
            .stderr({
                use std::fs::OpenOptions;
                std::process::Stdio::from(
                    OpenOptions::new().create(true).write(true).truncate(true)
                        .open(std::env::temp_dir().join("fntv-tauri2-mpv-stderr.log"))
                        .map_err(|e| format!("打开 mpv stderr 日志失败: {}", e))?,
                )
            })
            .spawn()
            .map_err(|e| format!("启动 mpv 失败 ({}): {}", exe, e))?;

        // 等待 IPC 管道出现（最长 20s）；期间若 mpv 进程退出则立刻带 stderr 报错，不留孤儿
        let stderr_log = std::env::temp_dir().join("fntv-tauri2-mpv-stderr.log");
        let mut ev_client = None;
        let stderr_tail;
        let read_tail = |p: &std::path::Path| -> String {
            std::fs::read_to_string(p)
                .unwrap_or_default()
                .lines()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ")
        };
        for _ in 0..200 {
            if let Ok(Some(status)) = child.try_wait() {
                stderr_tail = read_tail(&stderr_log);
                return Err(format!("mpv 提前退出 ({}): {}", status, stderr_tail));
            }
            if let Some(c) = try_open_pipe(&pipe).await {
                ev_client = Some(c);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let ev_client = ev_client.ok_or_else(|| {
            let _ = child.start_kill();
            format!("连接 mpv IPC 管道超时(20s) {}", read_tail(&stderr_log))
        })?;
        // 连接 B：命令/应答通道（管道已存在，直连）
        let cmd_client = try_open_pipe(&pipe)
            .await
            .ok_or_else(|| "连接 mpv IPC 管道超时(2)".to_string())?;

        let (tx, mut rx) = mpsc::unbounded_channel::<(Value, oneshot::Sender<Value>)>();
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(AtomicU64::new(1));

        // 任务 A：转发属性/事件到前端
        let app2 = app;
        tokio::spawn(async move {
            let mut lines = BufReader::new(ev_client).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&line) {
                            if v.get("event").is_some() {
                                let _ = app2.emit("mpv-event", v);
                            }
                        }
                    }
                    _ => break,
                }
            }
        });

        // 任务 B1：命令写出端（写半）
        let pending_w = pending.clone();
        let nid_w = next_id.clone();
        let (r_half, mut w_half) = tokio::io::split(cmd_client);
        tokio::spawn(async move {
            while let Some((cmd, cb)) = rx.recv().await {
                let id = nid_w.fetch_add(1, Ordering::SeqCst);
                pending_w.lock().await.insert(id, cb);
                let msg = json!({ "command": cmd, "request_id": id }).to_string();
                if w_half.write_all(msg.as_bytes()).await.is_err()
                    || w_half.write_all(b"\n").await.is_err()
                {
                    break;
                }
                let _ = w_half.flush().await;
            }
        });

        // 任务 B2：应答读端（读半）
        let pending_r = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(r_half).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&line) {
                            if let Some(id) = v.get("request_id").and_then(|i| i.as_u64()) {
                                if let Some(s) = pending_r.lock().await.remove(&id) {
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
        let (otx, orx) = oneshot::channel();
        self.tx
            .send((cmd, otx))
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
