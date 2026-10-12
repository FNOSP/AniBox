//! mpv 会话：**独立 mpv.exe 进程** + `--wid` 内嵌我们的子窗 + 命名管道 IPC（lc-087 定案）。
//!
//! 为什么放弃进程内 libmpv（lc-080~086 的教训）：libmpv 内嵌(wid)模式下 mpv 不跑自己的消息循环 →
//! 既不处理鼠标/键盘、也永远不置 `mouse-pos.hover` → uosc/OSC 底栏、悬停交互、右键菜单全废，
//! 只能靠「页面代做 + 另开窗口」绕，问题越积越多。
//! 独立 mpv.exe 自带消息循环 → uosc 底栏/原生鼠标/键盘/缩略图/弹幕等 Lua 脚本全部原生可用
//! （便携配置 portable_config 在 mpv.exe 同目录，自动生效，与主仓库完全同款）。
//!
//! 发起方式：pwsh Start-Process（lc-079 实测：Rust/tokio 直接 spawn 的 mpv 在应用内必死，
//! 经 powershell 中转存活；用 -PassThru 拿 PID 以便退出时清理）。
//! 控制与事件：`--input-ipc-server` 命名管道（JSON 行协议），前端协议保持
//! `mpv-event {event:"property-change", id, data}` 不变。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

/// 观察 id ↔ 属性名（前端 switch 依赖这些 id；6=fullscreen 只用于镜像到 Tauri 窗口）
const OBSERVED: [(u64, &str); 9] = [
    (1, "time-pos"),
    (2, "duration"),
    (3, "pause"),
    (4, "speed"),
    (5, "volume"),
    (6, "fullscreen"),
    // [lc-101] 加载/缓冲可见性：让页面能显示"缓冲中 / 空闲 / 播完"，而不是干等一个转圈
    (7, "paused-for-cache"),
    (8, "cache-buffering-state"),
    (9, "idle-active"),
];

pub struct MpvSession {
    pub embed_hwnd: isize,
    /// IPC 写端（JSON 行）
    tx: Arc<Mutex<Option<std::fs::File>>>,
    /// request_id → 回包等待者
    pending: Arc<Mutex<HashMap<u64, std_mpsc::Sender<Value>>>>,
    next_id: Arc<AtomicU64>,
    pub pid: Arc<Mutex<Option<u32>>>,
    /// [lc-093] 「App 全屏状态 → mpv fullscreen 属性」镜像的期望值。
    /// 观测到的值与它相同 ⇒ 是我们自己写进去的 → 不再抛给前端（防回声死循环）。
    /// 用「比对值」而非「布尔标记」：镜像值与当前值相同时 mpv 不发事件，残留的
    /// 布尔标记会把之后一次真实的用户操作误吞。
    fs_expect: Arc<Mutex<Option<bool>>>,
}

unsafe impl Send for MpvSession {}
unsafe impl Sync for MpvSession {}

fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\{}", name)
}

impl MpvSession {
    /// 拉起独立 mpv 进程并连上 IPC（幂等：实例常驻，播放/停止只切播放状态）
    pub async fn launch(
        app: AppHandle,
        mpv_path: &str,
        embed_hwnd: isize,
    ) -> Result<Arc<MpvSession>, String> {
        let name = format!("fntv-tauri2-mpv-{}", std::process::id());
        let path = pipe_path(&name);
        crate::libmpv::trace_reset(&format!(
            "launch(mpv.exe) pid={} embed_hwnd={}",
            std::process::id(),
            embed_hwnd
        ));

        // ── 1) 启动 mpv.exe（pwsh 中转，-PassThru 拿 PID）
        let log_path = std::env::temp_dir().join("fntv-tauri2-mpv.log");
        let args = [
            format!("--wid={}", embed_hwnd),
            format!("--input-ipc-server={}", path),
            "--idle=yes".to_string(),
            "--keep-open=yes".to_string(),
            "--force-window=immediate".to_string(),
            "--terminal=no".to_string(),
            // [lc-101] 关掉 ytdl_hook：直链是我们的 Go 代理，永远不需要 yt-dlp；
            // 但加载失败时 mpv 会自动跑 `ytdl_hook/on_load_fail`（实测会去叫 yt-dlp，白折腾 0.3s+
            // 并污染日志）。关掉让失败路径干净。
            "--ytdl=no".to_string(),
            format!("--log-file={}", log_path.to_string_lossy()),
        ];
        let q = '\'';
        let arg_list = args
            .iter()
            .map(|a| format!("{q}{}{q}", a.replace(q, "")))
            .collect::<Vec<_>>()
            .join(",");
        let cmd = format!(
            "$p = Start-Process -FilePath {q}{}{q} -ArgumentList @({arg_list}) -WindowStyle Hidden -PassThru; $p.Id",
            mpv_path.replace(q, "")
        );
        let out = tokio::process::Command::new("pwsh")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &cmd])
            .output()
            .await
            .map_err(|e| format!("拉起 mpv 失败: {e}"))?;
        let pid = String::from_utf8_lossy(&out.stdout).trim().parse::<u32>().ok();
        crate::libmpv::trace(&format!("mpv.exe spawned pid={pid:?} args={args:?}"));

        // ── 2) 连 IPC 管道（mpv 起得要一会儿，重试 ~20s）
        let mut file = None;
        for _ in 0..100 {
            match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
                Ok(f) => {
                    file = Some(f);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            }
        }
        let file = file.ok_or_else(|| {
            format!(
                "连不上 mpv IPC 管道（mpv 是否已退出？见 {}）",
                log_path.display()
            )
        })?;

        let tx = Arc::new(Mutex::new(Some(file)));
        let pending: Arc<Mutex<HashMap<u64, std_mpsc::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(AtomicU64::new(1));
        let pid_slot = Arc::new(Mutex::new(pid));
        let fs_expect: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
        let session = Arc::new(MpvSession {
            embed_hwnd,
            tx: tx.clone(),
            pending: pending.clone(),
            next_id: next_id.clone(),
            pid: pid_slot.clone(),
            fs_expect: fs_expect.clone(),
        });

        // ── 3) 读线程：事件 → tauri emit；回包 → 唤醒等待者
        {
            let read_file = tx
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|f| f.try_clone().ok())
                .ok_or_else(|| "IPC 管道复制失败".to_string())?;
            let app2 = app.clone();
            let pending2 = pending.clone();
            let fs_expect2 = fs_expect.clone();
            // [lc-101] start-file → file-loaded 的耗时（"拉取半天"到底花在哪一步，有数可查）
            let load_started2: Arc<Mutex<Option<std::time::Instant>>> = Arc::new(Mutex::new(None));
            std::thread::spawn(move || {
                let first_event = std::sync::atomic::AtomicBool::new(true);
                let reader = BufReader::new(read_file);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Ok(v) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if let Some(rid) = v.get("request_id").and_then(|r| r.as_u64()) {
                        if let Some(tx) = pending2.lock().unwrap().remove(&rid) {
                            let _ = tx.send(v);
                        }
                        continue;
                    }
                    let Some(ev) = v.get("event").and_then(|e| e.as_str()) else {
                        continue;
                    };
                    // [lc-101] 不能只转发 property-change：mpv 的**加载失败/结束全在事件里**
                    // （end-file 带 reason=error + 错误文本；eof/quit/stop 也走它）。
                    // 之前这一行把它们一律 continue 丢掉 —— 这就是"错误直接不显示"的根因：
                    // 页面永远停在"正在拉起 mpv…"，用户只看到无限转圈。
                    if ev != "property-change" {
                        match ev {
                            "start-file" => {
                                *load_started2.lock().unwrap() = Some(std::time::Instant::now());
                                let _ = app2.emit("mpv-load", json!({ "phase": "start" }));
                            }
                            "file-loaded" => {
                                let took = load_started2
                                    .lock()
                                    .unwrap()
                                    .take()
                                    .map(|t| t.elapsed().as_millis())
                                    .unwrap_or(0);
                                crate::libmpv::trace(&format!("mpv file-loaded（媒体就绪，加载耗时 {took}ms）"));
                                let _ = app2.emit("mpv-load", json!({ "phase": "loaded" }));
                            }
                            "end-file" => {
                                let reason = v.get("reason").and_then(|r| r.as_str()).unwrap_or("");
                                let err = v.get("error").and_then(|e| e.as_str()).unwrap_or("");
                                // 用户主动切片/停止/重定向不算失败，别弹错；
                                // 另外 "Opening failed or was aborted" / "Failed writing received
                                // data…" 是 `loadfile replace` 打断上一次加载留下的产物（实测日志里
                                // 与真实 500 混在一起），也不能拿去吓用户。
                                let benign = matches!(reason, "eof" | "quit" | "stop" | "redirect")
                                    || err.contains("aborted")
                                    || err.contains("Failed writing received data");
                                let is_fail = !benign && (reason == "error" || !err.is_empty());
                                crate::libmpv::trace(&format!("mpv end-file reason={reason} error={err}"));
                                if is_fail {
                                    let msg = if err.is_empty() {
                                        format!("播放失败（{reason}）")
                                    } else {
                                        format!("播放失败：{err}")
                                    };
                                    // ① 抛给页面（内嵌形态能看到 HTML 提示）
                                    let _ = app2.emit("mpv-end", json!({ "reason": reason, "error": err }));
                                    // ② 同时用 mpv 自己的 OSD 显示 —— 全页/全屏时「挖洞 = 整窗」，
                                    //    WebView 被整块裁掉，页面 HTML 一律不可见，只有 mpv 画的才看得见。
                                    let mut g = tx.lock().unwrap();
                                    if let Some(f) = g.as_mut() {
                                        let line = json!({
                                            "command": ["show-text", msg, 8000]
                                        })
                                        .to_string()
                                            + "\n";
                                        let _ = f.write_all(line.as_bytes());
                                        let _ = f.flush();
                                    }
                                } else {
                                    let _ = app2.emit("mpv-load", json!({ "phase": "ended", "reason": reason }));
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }
                    let Some(id) = v.get("id").and_then(|i| i.as_u64()) else {
                        continue;
                    };
                    if id == 6 {
                        // [lc-093] fullscreen 现在承载两件事：① App 形态（内嵌/全页）的镜像；
                        // ② uosc 底栏双形态的判据（uosc 用 mp.get_property_bool('fullscreen')
                        //    决定显示「极简组」还是「完整组」，见 portable_config/script-opts/uosc.conf）。
                        // 嵌入窗（--wid）下 mpv 的 update_fullscreen_state 会 early-return，
                        // 所以改这个属性只是状态标记，不会动窗口几何（已实测：几何不变）。
                        let on = v.get("data").and_then(|d| d.as_bool()).unwrap_or(false);
                        let mirrored = {
                            let mut g = fs_expect2.lock().unwrap();
                            let hit = *g == Some(on);
                            *g = None; // 命中与否都清掉，避免过期期望吞掉后续的真实用户操作
                            hit
                        };
                        if !mirrored {
                            // 用户从 mpv 侧（按键 / uosc 全屏按钮）改的 → 抛给前端切「内嵌 ↔ 全页 + 窗口全屏」
                            let _ = app2.emit("mpv-fullscreen", json!({ "on": on }));
                        }
                        continue;
                    }
                    if !OBSERVED.iter().any(|(i, _)| *i == id) {
                        continue;
                    }
                    if first_event.swap(false, Ordering::SeqCst) {
                        crate::libmpv::trace(&format!("IPC 事件流已通（首个属性变化 id={id}）"));
                    }
                    let _ = app2.emit(
                        "mpv-event",
                        json!({ "event": "property-change", "id": id, "data": v.get("data").cloned().unwrap_or(Value::Null) }),
                    );
                }
                crate::libmpv::trace("IPC 读线程结束（mpv 退出或管道断开）");
            });
        }

        // ── 4) 观察属性（id 与前端 switch 对齐）
        for (id, prop) in OBSERVED {
            let _ = session
                .command(json!(["observe_property", id, prop]))
                .await;
        }

        // ── 5) [lc-089] 启用 mpv 自建子窗 —— mpv 在 --wid 嵌入时会主动 EnableWindow(0)，
        // 而 Windows 命中测试跳过 disabled 窗口 → 鼠标永远进不了 mpv（点击全落到我们宿主窗）。
        // mpv 建窗是异步的（VO 初始化后才出现），故轮询等待。详见 win_embed::enable_mpv_child。
        crate::win_embed::spawn_enable_mpv_child(embed_hwnd, 75);

        Ok(session)
    }

    /// 发一条命令（JSON 数组）并等回包（5s 超时）
    pub async fn command(&self, cmd: Value) -> Result<Value, String> {
        let rid = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = std_mpsc::channel();
        self.pending.lock().unwrap().insert(rid, tx);
        {
            let mut guard = self.tx.lock().unwrap();
            let Some(f) = guard.as_mut() else {
                return Err("mpv 进程已退出".into());
            };
            let line = json!({ "command": cmd, "request_id": rid }).to_string() + "\n";
            if let Err(e) = f.write_all(line.as_bytes()) {
                self.pending.lock().unwrap().remove(&rid);
                return Err(format!("写 IPC 失败: {e}"));
            }
            let _ = f.flush();
        }
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(v) => {
                let err = v.get("error").and_then(|e| e.as_str()).unwrap_or("success");
                if err == "success" {
                    Ok(v)
                } else {
                    Err(err.to_string())
                }
            }
            Err(_) => {
                self.pending.lock().unwrap().remove(&rid);
                Err("mpv 响应超时".into())
            }
        }
    }

    /// 进程是否已退出
    pub async fn is_dead(&self) -> bool {
        let pid = *self.pid.lock().unwrap();
        match pid {
            Some(pid) => {
                // tasklist 查一下即可（轮询频率低，代价可接受）
                let out = tokio::process::Command::new("tasklist")
                    .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                    .output()
                    .await;
                match out {
                    Ok(o) => !String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
                    Err(_) => false,
                }
            }
            None => true,
        }
    }

    /// 停止播放（回 idle），实例保留
    pub async fn stop(&self) -> Result<(), String> {
        self.command(json!(["stop"])).await.map(|_| ())
    }

    /// [lc-093] 把 App 形态（内嵌=false / 全页全屏=true）镜像到 mpv 的 `fullscreen` 属性。
    /// 该属性是 uosc 底栏「极简 / 完整」双形态的判据；嵌入窗下它只是状态标记，不动窗口几何。
    /// 写入前先记下期望值，观测线程据此识别「这是自己写的」而不回声给前端。
    pub async fn set_ui_fullscreen(&self, on: bool) -> Result<(), String> {
        *self.fs_expect.lock().unwrap() = Some(on);
        let r = self
            .command(json!(["set", "fullscreen", if on { "yes" } else { "no" }]))
            .await;
        if r.is_err() {
            *self.fs_expect.lock().unwrap() = None; // 写失败 → 清掉期望，别吞后续真实事件
        }
        r.map(|_| ())
    }

    /// 退出 mpv 并销毁内嵌子窗
    pub async fn shutdown(&self) {
        let _ = self.command(json!(["quit"])).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let pid_now = *self.pid.lock().unwrap();
        if let Some(pid) = pid_now {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output()
                .await;
        }
        *self.tx.lock().unwrap() = None;
        crate::win_embed::destroy(self.embed_hwnd);
    }
}
