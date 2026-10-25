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
use tauri::{AppHandle, Emitter, Manager};

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

/// mpv IPC 连接类型：Windows 是命名管道（用 File 打开），
/// Linux/macOS 是 Unix domain socket（必须用 UnixStream 连接，open() 打不开 socket 文件）。
#[cfg(target_os = "windows")]
type IpcStream = std::fs::File;
#[cfg(not(target_os = "windows"))]
type IpcStream = std::os::unix::net::UnixStream;

pub struct MpvSession {
    /// IPC 写端（JSON 行）
    tx: Arc<Mutex<Option<IpcStream>>>,
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

/// mpv `--input-ipc-server` 的地址：
///   · Windows：命名管道 `\\.\pipe\<name>`；
///   · Linux/macOS：Unix domain socket 文件（mpv 只接受路径，不认 Windows 管道名）。
#[cfg(target_os = "windows")]
fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\{}", name)
}

#[cfg(not(target_os = "windows"))]
fn pipe_path(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("{name}.sock"))
        .to_string_lossy()
        .into_owned()
}

/// [lc-1621] 归一化在线源 HTTP 头：去空白、滤空值，并额外拼出 `http-header-fields` 串。
///
/// 返回 `(referrer, user_agent, header_fields)`，供两处共用：
///   ① 新建 mpv 会话时 → `http_header_args()` 拼成命令行参数；
///   ② 复用 mpv 会话时 → IPC `set referrer/user-agent/http-header-fields`。
/// 两处必须用**同一套**头，否则「首次播」（新建）与「连续播下一集」（复用）表现不一致。
///
/// [lc-1748] `http-header-fields` 的两条硬约束（本机 mpv 实测，探针见 tests/artifacts）：
///   ① **字段间分隔符必须是 `,`（不带空格）**——`, ` 会让第二个字段带前导空格，
///     ffmpeg 拼请求头时把它按 obs-fold 折进上一个头的值里：实测 `Referer` 变成
///     `"https://x/\r\n User-Agent: …"`，防盗链 CDN 看到**畸形 Referer** 直接
///     403/404（这是「同源 Kazumi 能播、AniBox 显示 404 / 播放失败」的主根因）；
///   ② **User-Agent 绝不能进 http-header-fields**——UA 值自带逗号
///     （`KHTML, like Gecko`），mpv 的 string-list 按逗号切分会把 UA 撕成碎字段。
///     UA 由 `--user-agent` / IPC `user-agent` 单独下发即可（实测 ffmpeg(hls) 的
///     **分片**请求同样带上，不需要在 header-fields 里重复）。
/// 值里含逗号的头一律不进 http-header-fields（进去了必然被撕碎，宁缺毋滥）。
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
    // [lc-1506][lc-1748] http-header-fields：ffmpeg(hls) 的分段请求走它
    // （mpv 的 `referrer` 属性只覆盖部分加载路径，缺了它分段会被源站 403）。
    // 只装「值里没有逗号」的 Referer（见上方两条硬约束），分隔符用 `,`。
    let mut hf: Vec<String> = Vec::new();
    if let Some(r) = &r {
        if !r.contains(',') {
            hf.push(format!("Referer: {r}"));
        }
    }
    let hf = if hf.is_empty() { None } else { Some(hf.join(",")) };
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
        // 非 Windows：Unix socket 文件若残留（上次异常退出），mpv 会因 bind 失败而启动不了，
        // 先清掉旧文件。
        #[cfg(not(target_os = "windows"))]
        {
            let _ = std::fs::remove_file(&path);
        }
        let mut command = tokio::process::Command::new(mpv_path);
        // [lc-1781] 把应用自身路径带给 mpv：sub-assrt.lua 用它作哈希助手
        // （`<exe> --subtitle-hash <file>` → 迅雷 gcid / OpenSubtitles moviehash）。
        if let Ok(exe) = std::env::current_exe() {
            command.env("ANIBOX_EXE", exe);
        }
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

        // ── 2) 连 IPC（mpv 起得要一会儿，重试 ~20s）
        // Windows：命名管道 → File::open；Linux/macOS：Unix socket → UnixStream::connect。
        let mut file: Option<IpcStream> = None;
        for _ in 0..100 {
            let conn = {
                #[cfg(target_os = "windows")]
                {
                    std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .ok()
                }
                #[cfg(not(target_os = "windows"))]
                {
                    std::os::unix::net::UnixStream::connect(&path).ok()
                }
            };
            if let Some(f) = conn {
                file = Some(f);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
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
        // [nas-subtitle] 当前集标题槽：标题状态机写回 force-media-title 的同一个值。
        // 字幕自动挂载任务按重试节奏重读（path 事件常先于标题回填到达）。
        let title_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let title_slot2 = title_slot.clone();
        // [nas-subtitle] 挂载任务的 IPC 写端（往命名管道写一行 JSON，fire-and-forget）。
        let tx_subtitle = Arc::clone(&tx);
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
                                // [nas-subtitle] 同步标题槽：字幕自动挂载按此做剧集防错配匹配。
                                *title_slot2.lock().unwrap() = Some(t.to_string());
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
                                    // [speed] 单集结束/停止：把该剧最后一次调速落盘。
                                    crate::playback_speed::flush(&app2);
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
                                // [speed] 切集前先把该剧最后一次调速落盘（防抖线程可能还没到点）。
                                crate::playback_speed::flush(&app2);
                                // 切集：结算上一集 + 重置本集计时（见 fnos_progress）。
                                crate::fnos_progress::set_item_from_url(&app2, s);
                                // [speed] 应用**本剧的**速度：有记录用它，没有就回 1.0。
                                // key 是剧级键（`fnos_progress::current_speed_key`，起播时按
                                // play/info 的剧级标识算好），同剧切集**沿用**、换剧才换记录 ——
                                // 这是本版相对上一版（按单集 item guid）的关键行为差异。
                                // 必须在 set_item_from_url 之后取（切集后 tracker 的 item_guid
                                // 已更新，但 speed_key 本就同剧不变）。
                                // 首帧（observe_property 的初始值）也会走这里，所以「打开 App 直接
                                // 播某集」同样能恢复速度，不依赖用户先切一次集。
                                let key = crate::fnos_progress::current_speed_key();
                                let speed = crate::playback_speed::speed_for(&app2, &key)
                                    .unwrap_or(crate::playback_speed::DEFAULT_SPEED);
                                // 同步更新跟踪器：若 mpv 因「速度未变」不发事件，快照里也不会
                                // 残留上一集的旧速度（set_item_from_url 已把 speed 重置为 1.0）。
                                crate::fnos_progress::set_speed(speed);
                                let line = json!({
                                    "command": ["set_property", "speed", speed],
                                })
                                .to_string()
                                    + "\n";
                                if let Some(f) = tx.lock().unwrap().as_mut() {
                                    let _ = f.write_all(line.as_bytes());
                                    let _ = f.flush();
                                }
                            }

                            // [nas-subtitle] NAS 外挂字幕自动挂载：path 变化（起播 / 整季自动切集）
                            // → 后台「stream/list → 选最佳外挂 → 下载 → sub-add select」。
                            // 库内媒体对 mpv 是代理 URL，sub-auto 永远扫不到 NAS 同目录字幕，
                            // 这条应用侧链路是对齐主仓库 mountSubtitlesWithRetry 的唯一自动挂载。
                            // 弹幕走副字幕轨（secondary-sid），sub-add select 只动主字幕轨，互不冲突；
                            // 非代理 URL（在线源/直播）在 parse_proxy_url 里就地返回，不触发。
                            if let Some(url_now) = v.get("data").and_then(|x| x.as_str()) {
                                let tx_sub = Arc::clone(&tx_subtitle);
                                let writer: crate::nas_subtitle::IpcWriter = Arc::new(move |line: &str| {
                                    let mut g = tx_sub.lock().unwrap();
                                    if let Some(f) = g.as_mut() {
                                        let _ = f.write_all(line.as_bytes());
                                        let _ = f.flush();
                                    }
                                });
                                let slot = Arc::clone(&title_slot);
                                let title_get: crate::nas_subtitle::TitleGetter =
                                    Arc::new(move || slot.lock().unwrap().clone().unwrap_or_default());
                                // [lc-1803] 字幕记忆挂载（用户手选下载的，晚于 NAS 自动挂载后挂为主轨）
                                {
                                    let writer2 = Arc::clone(&writer) as crate::nas_subtitle::IpcWriter;
                                    let app3: tauri::AppHandle = app2.clone();
                                    crate::subtitle_memory::spawn_mount(
                                        app3,
                                        url_now.to_string(),
                                        writer2,
                                    );
                                }
                                crate::nas_subtitle::spawn_mount(
                                    url_now,
                                    title_get,
                                    writer,
                                    // [lc-1763] 当前会话票兜底：URL 里的 token 是拼 URL 那刻的，
                                    // 重登录/长会话后可能滞后于配置里的新票
                                    app2.state::<crate::AppState>().config.blocking_read().token.clone(),
                                );
                            }
                        }
                        4 => {
                            // [speed] mpv 的 speed 变化（用户 uosc/快捷键/手柄调速，或我们自己
                            // 切集时 set_property 的回抛）。按**剧级键**记入本地持久化 →
                            // 同剧各集共享这一个倍速。
                            if let Some(d) = crate::playback_speed::speed_from_value(
                                v.get("data").unwrap_or(&Value::Null),
                            ) {
                                crate::fnos_progress::set_speed(d);
                                let key = crate::fnos_progress::current_speed_key();
                                if !key.is_empty() {
                                    crate::playback_speed::record_speed(&app2, &key, d);
                                }
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
                // [speed] mpv 退出：把最后一次调速立即落盘（防抖线程可能被进程一起带走）。
                crate::playback_speed::flush(&app2);
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
                #[cfg(target_os = "windows")]
                {
                    // tasklist 查一下即可（轮询频率低，代价可接受）
                    let mut probe = tokio::process::Command::new("tasklist");
                    #[cfg(target_os = "windows")]
                    probe.creation_flags(0x0800_0000); // CREATE_NO_WINDOW：轮询高频，不隐藏会狂闪 cmd 窗
                    let out = probe
                        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                        .output()
                        .await;
                    match out {
                        Ok(o) => !String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
                        Err(_) => false,
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    // `kill -0`：进程存在 → 退出码 0；不存在 → 非 0。
                    match tokio::process::Command::new("kill")
                        .args(["-0", &pid.to_string()])
                        .output()
                        .await
                    {
                        Ok(o) => !o.status.success(),
                        Err(_) => false,
                    }
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
        let pid_now = *self.pid.lock().unwrap();
        if let Some(pid) = pid_now {
            #[cfg(target_os = "windows")]
            {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                #[allow(unused_mut)]
                let mut kill = tokio::process::Command::new("taskkill");
                #[cfg(target_os = "windows")]
                kill.creation_flags(0x0800_0000);
                let _ = kill
                    .args(["/F", "/PID", &pid.to_string()])
                    .output()
                    .await;
            }
            // [macos/linux] 不要直接 `kill -9`：那会跳过 mpv 的正常退出流程（写 watch_later /
            // 关窗 / 落盘），下次起播可能丢续播点。顺序改为：
            //   ① 等 IPC `quit` 让 mpv 自己退（最多 ~1.2s）
            //   ② 仍在 → SIGTERM（再等最多 ~1s）
            //   ③ 还在 → 才 SIGKILL 兜底
            // 参考项目 node-mpv 也是「发 quit 命令 + 等进程退出」这一路。
            #[cfg(not(target_os = "windows"))]
            {
                for _ in 0..8 {
                    if self.is_dead().await {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
                if !self.is_dead().await {
                    let _ = tokio::process::Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .output()
                        .await;
                    for _ in 0..10 {
                        if self.is_dead().await {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
                if !self.is_dead().await {
                    let _ = tokio::process::Command::new("kill")
                        .args(["-9", &pid.to_string()])
                        .output()
                        .await;
                }
            }
        }
        *self.tx.lock().unwrap() = None;
    }
}

// ───────────────────────── 单测（纯逻辑，不触网） ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// [lc-1748] http-header-fields 的 obs-fold 硬约束：字段分隔符必须是 `,` 而非 `, `。
    /// 实测（本机 mpv + 本地 HTTP 探针）：`, ` 分隔时 ffmpeg 把带前导空格的第二个字段
    /// 折进 Referer 的值（`"https://x/\r\n User-Agent: …"`）→ 防盗链 CDN 403/404。
    #[test]
    fn header_fields_join_without_space() {
        let (_, _, hf) = normalize_http_headers(Some("https://ref.test/"), Some("UA"));
        assert_eq!(hf.as_deref(), Some("Referer: https://ref.test/"));
        // 多字段也不允许出现 ", "（当前只有 Referer 一种，此处防回归写死）
        assert!(!hf.unwrap().contains(", "), "分隔符不得带空格（obs-fold 会折坏上一个头）");
    }

    /// [lc-1748] UA 不进 http-header-fields：UA 值自带逗号（KHTML, like Gecko），
    /// mpv 的 string-list 按逗号切分会把 UA 撕碎 —— 只允许 --user-agent / IPC user-agent 下发。
    #[test]
    fn user_agent_never_rides_header_fields() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
        let args = http_header_args(Some("https://ref.test/"), Some(ua));
        // 命令行形态：referrer/user-agent 各自一个参数；header-fields 只含 Referer
        assert!(args.contains(&format!("--referrer=https://ref.test/")));
        assert!(args.contains(&format!("--user-agent={ua}")));
        let hf = args.iter().find(|a| a.starts_with("--http-header-fields=")).unwrap();
        assert_eq!(hf, "--http-header-fields=Referer: https://ref.test/");
        assert!(!hf.contains("Mozilla"), "UA 值（含逗号）绝不能进 http-header-fields");
    }

    /// [lc-1748] 值里含逗号的 Referer 不进 http-header-fields（进去必被撕碎，宁缺毋滥；
    /// mpv 侧仍带 --referrer，主流路径不受影响）。
    #[test]
    fn comma_in_referer_drops_header_fields_entry() {
        let (_, _, hf) = normalize_http_headers(Some("https://a.test/?x=1,y=2"), Some("UA"));
        assert!(hf.is_none(), "含逗号的值进 list 会被切碎，应整条丢弃");
    }

    /// 空头：什么参数都不出。
    #[test]
    fn empty_headers_produce_no_args() {
        assert!(http_header_args(None, None).is_empty());
        assert!(http_header_args(Some("  "), Some("")).is_empty());
    }
}
