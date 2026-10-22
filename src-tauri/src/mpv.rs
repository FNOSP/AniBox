//! mpv 会话：**独立 mpv.exe 进程 + 独立窗口**（lc-106：不再 --wid 内嵌，与主仓库一致）+ 命名管道 IPC。
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
const OBSERVED: [(u64, &str); 13] = [
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
    // [playlist] 当前播放列表位置/总数：整季播放列表下，前端与 uosc 都能据此显示「第 N/M 集」。
    (10, "playlist-pos"),
    (11, "playlist-count"),
    // [progress] 百分比进度（0..100，mpv 在时长已知后才会给出）：前端可直接用于展示/上报，
    // 但进度回传仍以 time-pos/duration 为准（percent-pos 在时长为 0 时是 null）。
    (12, "percent-pos"),
    // [progress] 当前播放条目的**完整 URL**：整季播放列表自动切集后，前端据此解析出
    // 「当前正在播的是哪一集（guid）」，把进度回传给飞牛对应的 item（旧项目同做法：
    // 从 `/api/v1/playvideo/<guid>` 的 URL 里取 guid）。
    // [fix] 必须观察 `path` 而不是 `filename`：`filename` 是 URL 的最后一段（basename），
    // `/api/v1/playvideo/` 前缀会被剥掉 → 解析永远为空（切集进度记到第一集上的根因）。
    // 参照随包 Lua 脚本 `fntv_nas_sub.lua` 也是用 `path` 匹配 `/playvideo/`。
    (13, "path"),
];

pub struct MpvSession {
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

/// [lc-1621] 归一化在线源 HTTP 头：去空白、滤空值，并额外拼出 `http-header-fields` 串。
///
/// 返回 `(referrer, user_agent, header_fields)`，供两处共用：
///   ① 新建 mpv 会话时 → `http_header_args()` 拼成命令行参数；
///   ② 复用 mpv 会话时 → IPC `set referrer/user-agent/http-header-fields`。
/// 两处必须用**同一套**头，否则「首次播」（新建）与「连续播下一集」（复用）表现不一致。
pub fn normalize_http_headers(
    referrer: Option<&str>,
    ua: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    let r = referrer
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let u = ua
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // [lc-1506] 再补一份 http-header-fields：ffmpeg(hls) 的分段请求走它
    // （mpv 的 `referrer` 属性只覆盖部分加载路径，缺了它分段会被源站 403）。
    let mut hf: Vec<String> = Vec::new();
    if let Some(r) = &r {
        hf.push(format!("Referer: {r}"));
    }
    if let Some(u) = &u {
        hf.push(format!("User-Agent: {u}"));
    }
    let hf = if hf.is_empty() { None } else { Some(hf.join(", ")) };
    (r, u, hf)
}

/// [lc-1621] 在线源 HTTP 头 → mpv **命令行参数**（`--referrer` / `--user-agent` /
/// `--http-header-fields`，参数名与 mpv 属性名一致）。
///
/// 为什么走命令行而不是 IPC `set`（对齐库内 `play_item` 的 lc-1559 形态）：
/// `referrer` / `http-header-fields` 只在**打开 URL 那一刻**被读取；而 mpv 启动头几秒
/// 事件循环被脚本/滤镜/D3D 初始化占住，任何 IPC 命令都会卡到秒级（lc-1559 实测）。
/// 「先 idle 启动 + IPC 设头 + IPC loadfile」的老形态 ⇒ 头常常没设上就 loadfile 了 ⇒
/// 源站拒流 ⇒ **窗口起来但播不动、且没有任何报错**。改走启动参数后，
/// mpv 一启动就带着头去加载直链，全程零启动期 IPC。
pub fn http_header_args(referrer: Option<&str>, ua: Option<&str>) -> Vec<String> {
    let (r, u, hf) = normalize_http_headers(referrer, ua);
    let mut a: Vec<String> = Vec::new();
    if let Some(r) = r {
        a.push(format!("--referrer={r}"));
    }
    if let Some(u) = u {
        a.push(format!("--user-agent={u}"));
    }
    if let Some(hf) = hf {
        a.push(format!("--http-header-fields={hf}"));
    }
    a
}

impl MpvSession {
    /// 拉起独立 mpv 进程并连上 IPC（幂等：实例常驻，播放/停止只切播放状态）
    ///
    /// `initial_url`：启动即加载的播放列表/直链（None = idle 启动，等 loadfile）。
    /// `force_title`：**随命令行下发的 `--force-media-title`**。必须在命令行而不是靠
    /// IPC `set` —— 实测（探针）：IPC `set force-media-title` 只在设置那一刻生效，
    /// 而容器自带的 `Title` tag（我们这来源的 MP4 常带繁体标题）会在 **loadfile 后**覆盖
    /// media-title，于是 uosc_danmaku 读到的是无季集标记的容器标题 → 判「非剧集」跳过
    /// B站搜索。命令行下发的值优先级最高，容器 tag 不再能顶掉它。
    /// `extra_args`：追加的**启动参数**（如在线源的 `--referrer/--user-agent/--http-header-fields`，
    /// 见 `http_header_args`）。同样属于「打开那一刻才读」的东西，必须随命令行下发、不靠启动期 IPC。
    pub async fn launch(
        app: AppHandle,
        mpv_path: &str,
        initial_url: Option<String>,
        force_title: Option<String>,
        extra_args: Vec<String>,
    ) -> Result<Arc<MpvSession>, String> {
        let name = format!("anibox-mpv-{}", std::process::id());
        let path = pipe_path(&name);
        crate::libmpv::trace_reset(&format!("launch(mpv.exe) pid={}", std::process::id()));

        // ── 1) 启动 mpv.exe
        // [lc-1552] 改为 **Rust 直接 spawn**（原为 pwsh Start-Process 中转，白花 ~450ms）：
        //   · 实测：直接 Popen → 6ms 就返回；经 pwsh → 454ms（pwsh 冷启本身约 440ms）。
        //   · lc-079 当年记的「Rust/tokio 直接 spawn 必死」是 **--wid 内嵌时代**的结论；
        //     现在已是独立窗口（lc-106），实测直接 spawn 的 mpv 连播 8s 稳定存活、
        //     IPC 正常、窗口正常（本机验证）。
        //   · tokio 的 child.id() 本来就能拿 PID，不需要 pwsh -PassThru。
        //   · 用 kill_on_drop(false)：mpv 要活过本次调用（会话常驻，播放/停止只切状态）。
        let log_path = std::env::temp_dir().join("anibox-mpv.log");
        let args = [
            format!("--input-ipc-server={}", path),
            "--idle=yes".to_string(),
            "--keep-open=yes".to_string(),
            "--force-window=immediate".to_string(),
            "--terminal=no".to_string(),
            // [lc-101] 关掉 ytdl_hook：直链是我们的 Go 代理，永远不需要 yt-dlp；
            // 但加载失败时 mpv 会自动跑 `ytdl_hook/on_load_fail`（实测会去叫 yt-dlp，白折腾 0.3s+
            // 并污染日志）。关掉让失败路径干净。
            "--ytdl=no".to_string(),
            // 允许拖动画面/边缘移动窗口（border=no 时这是唯一的拖动方式）。
            // 之前关掉是为了避免误拖，但用户反馈窗口无法移动，这里恢复默认行为。
            "--window-dragging=yes".to_string(),
            format!("--log-file={}", log_path.to_string_lossy()),
        ];
        // [lc-1559] 启动即加载（可选）：命令行直接带 M3U8/URL ⇒ 无 idle 空窗，
        // 不依赖 IPC（启动头几秒 IPC 命令会卡，见 lib.rs lc-1559 注释）。
        let mut args = args.to_vec();
        if let Some(t) = force_title {
            if !t.is_empty() {
                args.push(format!("--force-media-title={t}"));
            }
        }
        // [lc-1621] 在线源的头参数（--referrer/--user-agent/--http-header-fields）随命令行下发：
        // 与 initial_url 同属「打开那一刻才读」，必须启动前给到，不能靠启动期 IPC（会卡）。
        args.extend(extra_args);
        if let Some(u) = initial_url {
            args.push(u);
        }
        let mut command = tokio::process::Command::new(mpv_path);
        command
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // CREATE_NO_WINDOW：别给 mpv 弹一个控制台窗口（pwsh 时代由 -WindowStyle Hidden 承担）。
        // 仅 Windows 有该方法；其它平台无需处理。
        #[cfg(target_os = "windows")]
        {
            command.creation_flags(0x0800_0000);
        }
        let child = command
            .spawn()
            .map_err(|e| format!("拉起 mpv 失败: {e}"))?;
        let pid = child.id();
        crate::libmpv::trace(&format!(
            "mpv.exe spawned pid={pid:?} args={args:?}（直接 spawn，无 pwsh 中转）"
        ));
        // 不持有 child：mpv 要活过本次 launch（会话常驻，播放/停止只切状态），
        // 退出统一走 shutdown() 的 taskkill /PID（它只认 pid，不需要句柄）。
        drop(child);

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
            // [title] 逐集标题回填状态机（整季 M3U 专用，见下方 start-file/file-loaded 分支）：
            //   playlist-pos → playlist/<pos>/title → set force-media-title
            // 两个 request_id 记在这里，回包在同一个读线程里消化，不占用 pending 表。
            let next_id2 = next_id.clone();
            std::thread::spawn(move || {
                let first_event = std::sync::atomic::AtomicBool::new(true);
                let mut pos_req: Option<u64> = None;
                let mut title_req: Option<u64> = None;
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
                        // [title] 逐集标题回填：pos 回包 → 取该条 #EXTINF 标题；标题回包 → 写回。
                        if pos_req == Some(rid) {
                            pos_req = None;
                            if let Some(pos) = v.get("data").and_then(|d| d.as_i64()) {
                                if pos >= 0 {
                                    let rid2 = next_id2.fetch_add(1, Ordering::SeqCst);
                                    title_req = Some(rid2);
                                    let line = json!({
                                        "command": ["get_property", format!("playlist/{pos}/title")],
                                        "request_id": rid2,
                                    })
                                    .to_string()
                                        + "\n";
                                    if let Some(f) = tx.lock().unwrap().as_mut() {
                                        let _ = f.write_all(line.as_bytes());
                                        let _ = f.flush();
                                    }
                                }
                            }
                            continue;
                        }
                        if title_req == Some(rid) {
                            title_req = None;
                            let title = v
                                .get("data")
                                .and_then(|d| d.as_str())
                                .map(str::trim)
                                .filter(|s| !s.is_empty());
                            if let Some(t) = title {
                                // 容器自带的 Title tag 会顶掉 #EXTINF，只有 force-media-title 优先级更高。
                                let line = json!({
                                    "command": ["set", "force-media-title", t],
                                })
                                .to_string()
                                    + "\n";
                                if let Some(f) = tx.lock().unwrap().as_mut() {
                                    let _ = f.write_all(line.as_bytes());
                                    let _ = f.flush();
                                }
                            }
                            continue;
                        }
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
                                // [fix] 切集/停止/播完时**先结算当前集**（跟踪器里还是这一集的终值），
                                // 再把计数清零；否则下一集的 time-pos 会顶掉上一集的最后位置。
                                // `redirect` 是同一内容被内部重开（位置连续），不能当结束结算。
                                if reason != "redirect" {
                                    crate::fnos_progress::settle_current(&app2);
                                }
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
                        // [title] 每个文件开始加载 / 加载完成时，把**该条 #EXTINF 标题**写回
                        // `force-media-title`。原因（实测 mpv v0.41）：media-title 的优先级是
                        // `force-media-title` → 容器 Title tag → 播放列表条目标题；fnOS 出的 MP4
                        // 自带繁体 `Title` tag，会把整季 M3U 的 #EXTINF（`剧名 - S1E3: 集名`）
                        // 整个顶掉，窗口标题只剩剧名。逐集写 force-media-title 既能把季/集/集名
                        // 带回来，又不会像命令行那样把**所有集**盖成同一个标题。
                        if matches!(ev, "start-file" | "file-loaded")
                            && pos_req.is_none()
                            && title_req.is_none()
                        {
                            let rid = next_id2.fetch_add(1, Ordering::SeqCst);
                            pos_req = Some(rid);
                            let line = json!({
                                "command": ["get_property", "playlist-pos"],
                                "request_id": rid,
                            })
                            .to_string()
                                + "\n";
                            if let Some(f) = tx.lock().unwrap().as_mut() {
                                let _ = f.write_all(line.as_bytes());
                                let _ = f.flush();
                            }
                        }
                        continue;
                    }
                    let Some(id) = v.get("id").and_then(|i| i.as_u64()) else {
                        continue;
                    };
                    // [progress] 退出兜底：在 Rust 侧维护最后位置/当前集（见 fnos_progress）。
                    match id {
                        1 => {
                            if let Some(d) = v.get("data").and_then(|x| x.as_f64()) {
                                crate::fnos_progress::tick(&app2, d, 0.0);
                            }
                        }
                        2 => {
                            if let Some(d) = v.get("data").and_then(|x| x.as_f64()) {
                                crate::fnos_progress::tick(&app2, 0.0, d);
                            }
                        }
                        13 => {
                            if let Some(s) = v.get("data").and_then(|x| x.as_str()) {
                                // 切集：结算上一集 + 重置本集计时（见 fnos_progress）。
                                crate::fnos_progress::set_item_from_url(&app2, s);
                            }
                        }
                        _ => {}
                    }
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
                // [lc-104] mpv 退出（关窗 / 崩溃 / 被杀）→ **必须**隐藏宿主窗并撤洞。
                // 否则页面里会留下一个"坑"：洞还在挖，但宿主窗后面没人画（子窗消失后不会自动擦底），
                // 于是透出主窗的亚克力浅色底 —— 就是用户报的「把 mpv 窗口关闭后那里直接是个坑」。
                // [lc-106] 独立窗口模式：mpv 自己的窗没了就是没了，无需处理页面上的"坑"。
                //
                // [progress] 读线程结束 = mpv 已退出（关窗/崩溃/被杀）。此时 mpv 不可能再发
                // `end-file`，若不在这里补一个「结束」事件，前端最后一小段进度就永远不会 flush。
                // 统一走 `mpv-load {phase:"ended"}`（前端在 ended 里结算进度；重复结算幂等）。
                // [progress] 同时由 Rust 直接落盘一次（App 正在退出时前端 invoke 可能来不及）。
                crate::fnos_progress::flush_on_exit(app2.clone());
                let _ = app2.emit(
                    "mpv-load",
                    json!({ "phase": "ended", "reason": "ipc-closed" }),
                );
                crate::libmpv::trace("IPC 读线程结束（mpv 退出或管道断开）");
            });
        }

        // ── 4) 观察属性（id 与前端 switch 对齐）
        for (id, prop) in OBSERVED {
            let _ = session
                .command(json!(["observe_property", id, prop]))
                .await;
        }

        Ok(session)
    }

    /// [lc-1558] 发一条命令但**不等回包**（fire-and-forget）。
    ///
    /// 实测：mpv 启动/加载期事件循环被脚本、滤镜、D3D 初始化占住，等回包的命令
    /// 会卡数秒（单条 set 实测 2.7~3.5s，连 loadfile 也一样）。播放链路里那些
    /// "发了就完事"的命令（loadfile 本体、清理 referrer/header、set pause、
    /// set force-media-title）用这个发送 —— 成败由事件侧兜底（end-file error → mpv-end）。
    pub async fn command_no_reply(&self, cmd: Value) {
        let rid = self.next_id.fetch_add(1, Ordering::SeqCst);
        let line = json!({ "command": cmd, "request_id": rid }).to_string() + "
";
        let tx = Arc::clone(&self.tx);
        // [lc-1558] 写管道放**阻塞线程**（spawn_blocking）：mpv 启动/加载期事件循环被占、
        // 可能暂时不读管道 ⇒ 同步写在 async 上下文里会把 executor 线程一起拖住。
        // 交给 blocking 池后：最多占一个池线程，executor 与其它命令完全不受影响；
        // mpv 一腾出手就会读走数据，write 自然完成。
        let _ = tokio::task::spawn_blocking(move || {
            let mut guard = tx.lock().unwrap();
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
            }
        })
        .await;
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
    }
}
