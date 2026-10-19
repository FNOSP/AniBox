//! 统一播放启动封装（mpv / 本机 PotPlayer）。
//!
//! 设计目标：
//!   · **mpv**（随包自带，默认）：完全复用 `mpv::MpvSession` 的既有逻辑
//!     （库内 m3u8 / 在线源头参数 / 会话复用），行为与接入前一致。
//!   · **PotPlayer**（可选本机安装）：PotPlayer 没有 IPC，只能靠命令行参数
//!     （`<url>` + `/seek=HH:MM:SS` + 重复 `-sub=`）启动；进度回传依赖随包
//!     `potctl.exe info` 轮询，转换成现有 `mpv-event`（id=1 位置 / id=2 时长）。
//!
//! 在线源的 Referer/UA 无法通过 PotPlayer 命令行传递 → 由本模块内置的
//! **本地 HTTP relay**（127.0.0.1:22349）代转，透传 Range 以支持拖动/续播。
//!
//! 约定：`launch` 是唯一入口；调用方只负责把「最终要播的 URL + 标题 + 续播点」
//! 准备好，不再关心底层播放器差异。

use crate::config::Config;
use crate::AppState;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex as StdMutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};

/// relay 监听端口。刻意避开 22346（Go 代理）与 22347（弹幕 shim）。
pub const RELAY_PORT: u16 = 22349;

/// PotPlayer 常见安装位置（按优先级）。
const POTPLAYER_CANDIDATES: [&str; 2] = [
    r"C:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe",
    r"C:\Program Files (x86)\DAUM\PotPlayer\PotPlayerMini64.exe",
];

/// 在线源 HTTP 头（只保留当前调用方实际会提供的内容）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpHeaders {
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    /// 预留：规则源自定义头（PotPlayer 模式经 relay 透传）。
    pub extra: Vec<(String, String)>,
}

impl HttpHeaders {
    pub fn new(referer: Option<String>, user_agent: Option<String>) -> Self {
        let norm = |s: Option<String>| {
            s.map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
        };
        HttpHeaders {
            referer: norm(referer),
            user_agent: norm(user_agent),
            extra: Vec::new(),
        }
    }

    /// 是否含有任何需要 relay 代转的头。
    pub fn needs_relay(&self) -> bool {
        self.referer.is_some() || self.user_agent.is_some() || !self.extra.is_empty()
    }
}

// ───────────────────────── 路径探测 / 校验 ─────────────────────────

/// 校验并规范化一个 PotPlayer 路径：必须存在、是文件、扩展名为 .exe。
/// 返回去掉 Windows `\\?\` 前缀的绝对路径。
pub fn validate_potplayer_path(path: &str) -> Result<String, String> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err("PotPlayer 路径为空".into());
    }
    let p = Path::new(raw);
    if !p.exists() {
        return Err(format!("PotPlayer 不存在：{raw}"));
    }
    if !p.is_file() {
        return Err(format!("不是可执行文件：{raw}"));
    }
    let is_exe = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("exe"))
        .unwrap_or(false);
    if !is_exe {
        return Err(format!("不是 .exe 文件：{raw}"));
    }
    let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    Ok(strip_verbatim_prefix(&abs.to_string_lossy()))
}

fn strip_verbatim_prefix(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    s.strip_prefix(r"\\?\").unwrap_or(s).to_string()
}

/// 常见安装位置探测（不访问注册表，避免额外依赖）。
pub fn detect_potplayer_path() -> Option<String> {
    let mut candidates: Vec<PathBuf> = POTPLAYER_CANDIDATES.iter().map(PathBuf::from).collect();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join(r"PotPlayer\PotPlayerMini64.exe"));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| strip_verbatim_prefix(&p.to_string_lossy()))
}

/// 解析实际使用的 PotPlayer 路径：配置优先，其次自动探测。
pub fn resolve_potplayer_path(configured: &str) -> Result<String, String> {
    if !configured.trim().is_empty() {
        return validate_potplayer_path(configured)
            .map_err(|e| format!("{e}（可在设置里重新选择 PotPlayer）"));
    }
    detect_potplayer_path().ok_or_else(|| {
        "未找到 PotPlayer，请安装 PotPlayer 或在设置中手动指定 PotPlayerMini64.exe".into()
    })
}

/// 解析播放器：返回 (kind, path)。kind 为 "mpv" | "potplayer"。
pub fn resolve_player(cfg: &Config) -> Result<(String, String), String> {
    match cfg.player_kind.trim() {
        "" | "mpv" => {
            if cfg.mpv_path.trim().is_empty() {
                Err("mpv 路径未配置".into())
            } else {
                Ok(("mpv".into(), cfg.mpv_path.clone()))
            }
        }
        "potplayer" => Ok(("potplayer".into(), resolve_potplayer_path(&cfg.potplayer_path)?)),
        other => Err(format!(
            "未知播放器类型：{other}（只允许 mpv/potplayer）"
        )),
    }
}

// ───────────────────────── PotPlayer 参数构建（纯函数，可测） ─────────────────────────

/// 是否是 HLS 播放列表（PotPlayer 模式下若还要求 Referer/UA 则无法完整代理分段）。
pub fn looks_like_hls(url: &str) -> bool {
    let u = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    u.ends_with(".m3u8") || u.ends_with(".m3u")
}

/// 秒 → PotPlayer `/seek=` 需要的 `HH:MM:SS`。
/// PotPlayer 官方语法是 `hh:mm:ss.ms`，部分版本不认纯秒数，故统一转换。
pub fn format_seek(secs: f64) -> String {
    let s = if secs.is_finite() { secs.max(0.0).floor() as u64 } else { 0 };
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// 是否应该下发 `/seek=`：续播点 > 0，且离片尾不太近（≤98%）。
/// 时长未知（<=0）时仍允许 seek（续播优先）。
pub fn should_seek(resume_secs: f64, duration_secs: f64) -> bool {
    resume_secs > 0.0 && (duration_secs <= 0.0 || resume_secs <= 0.98 * duration_secs)
}

/// 构建 PotPlayer 命令行参数。
///
/// 顺序：`<url>` → `/seek=HH:MM:SS`（可选）→ 重复 `-sub=<path>`。
/// **不要手动加引号**：Windows 上 `Command` 会为含空格参数自动加引号，
/// 手动加反而会让引号变成路径里的字面字符（旧项目实测坑）。
pub fn build_potplayer_args(
    url: &str,
    resume_secs: f64,
    duration_secs: f64,
    subs: &[String],
) -> Vec<String> {
    let mut args = vec![url.to_string()];
    if should_seek(resume_secs, duration_secs) {
        args.push(format!("/seek={}", format_seek(resume_secs)));
    }
    for s in subs {
        let s = s.trim();
        if !s.is_empty() {
            // 官方前缀是横杠 `-sub=`（斜杠 /sub= 常被静默忽略）；多字幕 = 重复该参数。
            args.push(format!("-sub={s}"));
        }
    }
    args
}

// ───────────────────────── 进程管理 ─────────────────────────

/// 全局代际号：每次启动/停止 PotPlayer 都自增，旧 watcher 据此退出。
static POTPLAYER_GEN: AtomicU64 = AtomicU64::new(0);

/// 停止并清理 PotPlayer。`emit_end=true` 时额外补一条 ended 事件
/// （`player_stop` 用；切换新集时传 false，避免旧会话的 ended 干扰新会话）。
pub async fn stop_potplayer(app: &AppHandle, state: &AppState, emit_end: bool) {
    POTPLAYER_GEN.fetch_add(1, Ordering::SeqCst);
    let pid = state.potplayer_pid.lock().await.take();
    if let Some(pid) = pid {
        let ok = tokio::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            // PID 已失效/被重启：按镜像名兜底（只针对我们刚记录的 PotPlayer 进程）
            let _ = tokio::process::Command::new("taskkill")
                .args(["/F", "/IM", "PotPlayerMini64.exe"])
                .output()
                .await;
        }
    }
    if emit_end {
        let _ = app.emit("mpv-load", json!({ "phase": "ended", "reason": "stop" }));
    }
}

/// 清理全部播放器（退出 / player_stop 共用）。
pub async fn stop_all(app: &AppHandle, state: &AppState, emit_end: bool) {
    if let Some(m) = state.mpv.lock().await.as_ref() {
        let _ = m.stop().await;
    }
    stop_potplayer(app, state, emit_end).await;
}

// ───────────────────────── 统一启动入口 ─────────────────────────

/// 统一播放启动。
///
/// - `url`：最终播放地址（库内为本地 m3u8；在线源为直链；PotPlayer+头时内部再包 relay）。
/// - `title`：媒体标题（仅 mpv 的 force-media-title 使用；PotPlayer 无对应参数）。
/// - `resume_secs` / `duration_secs`：续播点与总时长（仅 PotPlayer 的 `/seek` 使用）。
/// - `headers`：在线源 Referer/UA；mpv 走启动参数，PotPlayer 走 relay。
/// - `subs`：外挂字幕路径（仅 PotPlayer 使用，重复 `-sub=`）。
/// - `playlist_start`：`url` 是**多条目播放列表文件**时，从哪一条开始播（0 基）。
///   `None` = 单条媒体（沿用 force-media-title / loadfile 逻辑）。
#[allow(clippy::too_many_arguments)]
pub async fn launch(
    app: &AppHandle,
    state: &AppState,
    cfg: &Config,
    url: &str,
    title: &str,
    resume_secs: f64,
    duration_secs: f64,
    headers: Option<&HttpHeaders>,
    subs: &[String],
    playlist_start: Option<usize>,
) -> Result<(), String> {
    let (kind, path) = resolve_player(cfg)?;
    crate::libmpv::trace(&format!(
        "[player] launch kind={kind} path={path} url={url} resume={resume_secs} dur={duration_secs} playlist_start={playlist_start:?}"
    ));
    match kind.as_str() {
        "mpv" => launch_mpv(app, state, cfg, url, title, headers, playlist_start).await,
        "potplayer" => {
            launch_potplayer(
                app,
                state,
                &path,
                url,
                title,
                resume_secs,
                duration_secs,
                headers,
                subs,
            )
            .await
        }
        _ => unreachable!(),
    }
}

async fn launch_mpv(
    app: &AppHandle,
    state: &AppState,
    cfg: &Config,
    url: &str,
    title: &str,
    headers: Option<&HttpHeaders>,
    playlist_start: Option<usize>,
) -> Result<(), String> {
    let mut extra = match headers {
        Some(h) if h.needs_relay() => {
            crate::mpv::http_header_args(h.referer.as_deref(), h.user_agent.as_deref())
        }
        _ => Vec::new(),
    };
    // [playlist] 多条目播放列表：命令行 `--playlist-start=<idx>` 指定起播集，
    // 与「先启动再 IPC jump」不同，起播零 IPC 依赖（启动期 IPC 会卡数秒，见 lib.rs lc-1559）。
    if let Some(idx) = playlist_start {
        extra.push(format!("--playlist-start={idx}"));
    }
    let mut slot = state.mpv.lock().await;
    let existing = match slot.as_ref() {
        Some(m) if !m.is_dead().await => Some(m.clone()),
        Some(m) => {
            m.shutdown().await;
            None
        }
        None => None,
    };
    let s = match existing {
        Some(m) => {
            // 复用会话：mpv 已过启动期，IPC 不卡。标题先于 loadfile。
            // 多条目播放列表靠 #EXTINF 标题驱动 media-title（弹幕链按集名匹配），
            // 此时不能写 force-media-title —— 它会把所有集都盖成同一个标题。
            let _ = m
                .command(json!([
                    "set",
                    "force-media-title",
                    if playlist_start.is_some() { "" } else { title }
                ]))
                .await;
            if let Some(h) = headers {
                let (r, u, hf) =
                    crate::mpv::normalize_http_headers(h.referer.as_deref(), h.user_agent.as_deref());
                // referrer 无论有没有都写（没有就清空），避免上一部在线源的头残留。
                let _ = m
                    .command(json!(["set", "referrer", r.unwrap_or_default()]))
                    .await;
                if let Some(u) = u {
                    let _ = m.command(json!(["set", "user-agent", u])).await;
                }
                if let Some(hf) = hf {
                    let _ = m
                        .command(json!(["set", "http-header-fields", hf]))
                        .await;
                }
            } else {
                // 库内播放：清掉上一部在线源可能残留的头（fire-and-forget，不拖起播）。
                m.command_no_reply(json!(["set", "referrer", ""])).await;
                m.command_no_reply(json!(["set", "http-header-fields", ""]))
                    .await;
            }
            match playlist_start {
                Some(idx) => {
                    // loadlist 会整体替换播放列表；随后把播放位置定到点击的那一集。
                    m.command(json!(["loadlist", url.to_string(), "replace"]))
                        .await?;
                    let _ = m
                        .command(json!(["set", "playlist-pos", idx]))
                        .await;
                }
                None => {
                    m.command(json!(["loadfile", url.to_string(), "replace"]))
                        .await?;
                }
            }
            let _ = m.command(json!(["set", "pause", "no"])).await;
            m
        }
        None => {
            // 多条目播放列表：标题由 #EXTINF 提供，不传 force-media-title。
            let force_title = if playlist_start.is_some() {
                String::new()
            } else {
                title.to_string()
            };
            crate::mpv::MpvSession::launch(
                app.clone(),
                &cfg.mpv_path,
                Some(url.to_string()),
                Some(force_title),
                extra,
            )
            .await?
        }
    };
    *slot = Some(s);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn launch_potplayer(
    app: &AppHandle,
    state: &AppState,
    path: &str,
    url: &str,
    title: &str,
    resume_secs: f64,
    duration_secs: f64,
    headers: Option<&HttpHeaders>,
    subs: &[String],
) -> Result<(), String> {
    // 1) 新播放前清理上一个 PotPlayer（PotPlayer 默认单实例，残留会抢新参数）。
    stop_potplayer(app, state, false).await;

    // 1.5) HLS + 必须带头：relay 只能代转播放列表本身，分段请求由 PotPlayer 直接发起
    // （拿不到 Referer/UA）→ 必然失败。这里明确报错，避免"窗口起来但播不动"的静默失败。
    if let Some(h) = headers {
        if h.needs_relay() && looks_like_hls(url) {
            return Err(
                "该在线源是 HLS 且需要 Referer/UA，PotPlayer 无法为分段请求携带请求头。\
                 请到「设置 → 播放器」切回自带 mpv 播放。"
                    .into(),
            );
        }
    }

    // 2) 一律经本地 relay 转发，并在 URL 路径里附带可读文件名：
    //    PotPlayer 把 URL 末段当作标题/播放列表项名，直链（GUID/查询串）会显示成一串字符。
    //    与旧项目 playbackShim 同思路：`/relay/<id>/<剧名 - 集名>.mp4`，relay 仍只按 id 转发。
    //    无 Referer/UA 时 relay 仅做本机逐字节转发（Range 透传），不影响 /seek、-sub。
    ensure_relay().await?;
    let relay_headers = headers.cloned().unwrap_or_default();
    let id = relay_register(RelayTarget {
        url: url.to_string(),
        headers: relay_headers,
    });
    let play_url = relay_url_named(&id, title);
    crate::libmpv::trace(&format!(
        "[player] potplayer relay 注册: {url} -> {play_url}（referer={:?} ua={:?} title={title:?}）",
        headers.and_then(|h| h.referer.as_deref()),
        headers.and_then(|h| h.user_agent.as_deref()),
    ));

    // 3) 构建参数并启动（不等待，记录 PID）。
    let args = build_potplayer_args(&play_url, resume_secs, duration_secs, subs);
    let _ = app.emit("mpv-load", json!({ "phase": "start" }));
    let mut command = std::process::Command::new(path);
    command
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        // CREATE_NO_WINDOW：不额外弹控制台（PotPlayer 自己的 GUI 窗口不受影响）
        command.creation_flags(0x0800_0000);
    }
    let child = command
        .spawn()
        .map_err(|e| format!("拉起 PotPlayer 失败（{path}）：{e}"))?;
    let pid = child.id();
    drop(child); // 不等待：PotPlayer 要活过本次调用
    *state.potplayer_pid.lock().await = Some(pid);
    crate::libmpv::trace(&format!(
        "[player] PotPlayer spawned pid={pid} args={args:?}"
    ));

    // 4) 启动进度/结束 watcher（potctl 可用时回传进度；否则仅监测进程退出）。
    let generation = POTPLAYER_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let potctl = potctl_path(app);
    spawn_potplayer_watcher(app.clone(), pid, generation, potctl);
    Ok(())
}

/// potctl.exe 路径解析：resources / exe 同级 / dev 目录 / 旧项目目录兜底。
pub fn potctl_path(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join("potctl.exe"));
        candidates.push(dir.join("resources").join("potctl.exe"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("potctl.exe"));
            candidates.push(dir.join("resources").join("potctl.exe"));
            candidates.push(dir.join("third_party").join("proxy").join("potctl.exe"));
        }
    }
    // dev 形态：src-tauri/resources/potctl.exe
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("potctl.exe"),
    );
    candidates.push(PathBuf::from(r"D:\GitHub\AniBox\third_party\proxy\potctl.exe"));
    candidates.into_iter().find(|p| p.is_file())
}

fn spawn_potplayer_watcher(app: AppHandle, pid: u32, generation: u64, potctl: Option<PathBuf>) {
    tauri::async_runtime::spawn(async move {
        let mut miss = 0u32;
        let mut ended_sent = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            if POTPLAYER_GEN.load(Ordering::SeqCst) != generation {
                return; // 已被新一次启动/停止取代
            }

            let alive = pid_alive(pid).await;
            // potctl 一次查询同时用于「窗口是否仍在」与进度。
            let info = match potctl.as_ref() {
                Some(p) => potctl_info(p).await,
                None => None,
            };
            let found = info
                .as_ref()
                .and_then(|v| v.get("found"))
                .and_then(|b| b.as_bool())
                .unwrap_or(false);

            if !alive && !found {
                miss += 1;
                if miss >= 2 {
                    crate::libmpv::trace(&format!(
                        "[player] PotPlayer 窗口已关闭 pid={pid} → emit ended"
                    ));
                    if !ended_sent {
                        let _ = app.emit(
                            "mpv-load",
                            json!({ "phase": "ended", "reason": "eof" }),
                        );
                    }
                    // 只有仍是本次会话时才清 PID，避免误清新实例。
                    let st = app.state::<AppState>();
                    let mut slot = st.potplayer_pid.lock().await;
                    if *slot == Some(pid) {
                        *slot = None;
                    }
                    return;
                }
                continue;
            }
            miss = 0;

            // 单实例转发：spawn 出来的进程可能立刻退出，把参数转交给已在运行的 PotPlayer。
            // 此时 pid 已死但窗口还在 → 清掉 PID（避免误杀用户手动开的实例），继续轮询进度。
            if !alive && found {
                let st = app.state::<AppState>();
                let mut slot = st.potplayer_pid.lock().await;
                if *slot == Some(pid) {
                    *slot = None;
                }
            }

            // potctl info → 进度事件（PotPlayer 无 IPC，进度只能轮询）
            if found {
                if let Some(info) = info.as_ref() {
                    let pos_ms = info.get("position").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let dur_ms = info.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let state = info.get("state").and_then(|v| v.as_i64()).unwrap_or(0);
                    let pos = (pos_ms / 1000.0).floor();
                    let dur = (dur_ms / 1000.0).floor();
                    if dur > 0.0 {
                        // [progress] 退出兜底跟踪（PotPlayer 无 filename，当前集沿用起播时的那一集）。
                        crate::fnos_progress::tick(pos, dur);
                        let _ = app.emit(
                            "mpv-event",
                            json!({ "event": "property-change", "id": 1, "data": pos }),
                        );
                        let _ = app.emit(
                            "mpv-event",
                            json!({ "event": "property-change", "id": 2, "data": dur }),
                        );
                        let _ = app.emit(
                            "mpv-event",
                            json!({ "event": "property-change", "id": 3, "data": state == 1 }),
                        );
                        // 播到片尾（PotPlayer 停在最后一帧时）也补一条 ended，
                        // 让「继续观看 / 观影记录」能结算（窗口仍开着不重复发）。
                        if !ended_sent && pos >= dur - 3.0 {
                            ended_sent = true;
                            let _ = app.emit(
                                "mpv-load",
                                json!({ "phase": "ended", "reason": "eof" }),
                            );
                        }
                    }
                }
            }
        }
    });
}

async fn pid_alive(pid: u32) -> bool {
    let out = tokio::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .await;
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
        // 查询失败时保守认为还活着，避免误发 ended。
        Err(_) => true,
    }
}

async fn potctl_info(potctl: &Path) -> Option<Value> {
    let mut cmd = tokio::process::Command::new(potctl);
    cmd.arg("info")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x0800_0000);
    let out = cmd.output().await.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str::<Value>(text.trim()).ok()
}

// ───────────────────────── 本地 HTTP relay ─────────────────────────

#[derive(Clone, Debug)]
pub struct RelayTarget {
    pub url: String,
    pub headers: HttpHeaders,
}

static RELAYS: OnceLock<StdMutex<HashMap<String, RelayTarget>>> = OnceLock::new();
static RELAY_SERVER: OnceLock<()> = OnceLock::new();
static RELAY_READY: AtomicBool = AtomicBool::new(false);

fn relay_map() -> &'static StdMutex<HashMap<String, RelayTarget>> {
    RELAYS.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// 注册一个转发目标，返回 id。
pub fn relay_register(target: RelayTarget) -> String {
    let id = format!("{:016x}", rand::random::<u64>());
    relay_map().lock().unwrap().insert(id.clone(), target);
    id
}

pub fn relay_lookup(id: &str) -> Option<RelayTarget> {
    relay_map().lock().unwrap().get(id).cloned()
}

/// 测试/清理用（仅测试构建）。
#[cfg(test)]
pub fn relay_clear() {
    relay_map().lock().unwrap().clear();
}

/// 不带可读名的 relay URL（保留给测试/调试；播放路径统一用 [`relay_url_named`]）。
#[allow(dead_code)]
pub fn relay_url(id: &str) -> String {
    format!("http://127.0.0.1:{RELAY_PORT}/relay/{id}")
}

/// 把可读标题整理成可安全放进 URL 路径的文件名基名（不含扩展名）。
///
/// 对齐旧项目 `playbackShim.makeUrl`：先替换 Windows 文件名非法字符
/// `\ / : * ? " < > |` 为 `_`，再由 [`relay_url_named`] 整段 URL 编码。
/// 编码结果是纯 ASCII 百分号序列，PotPlayer 解码后仍显示正确中文。
pub fn sanitize_display_name(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for ch in title.chars() {
        // 控制字符（含 \r\n\t）与 Windows 非法文件名统一替换，避免破坏 URL/文件名。
        if ch.is_control() || matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let out = out.trim().trim_matches('.').trim().to_string();
    if out.is_empty() {
        "video".to_string()
    } else {
        out
    }
}

/// 生成带可读文件名的 relay URL：`http://127.0.0.1:22349/relay/<id>/<可读名>.mp4`。
///
/// PotPlayer 会把 URL 末段当作显示标题，因此把剧名/集名编码进路径即可让它显示中文名，
/// 而不是一串 GUID/查询串。relay 路由只取 `/relay/` 后的**第一段**作为 id，
/// 后续文件名段被忽略，仍按 id 转发（见 [`relay_id_from_path`]）。
pub fn relay_url_named(id: &str, display_name: &str) -> String {
    let stem = sanitize_display_name(display_name);
    let encoded = urlencoding::encode(&stem);
    format!("http://127.0.0.1:{RELAY_PORT}/relay/{id}/{encoded}.mp4")
}

/// 从请求路径中取出 relay id：`/relay/<id>` 与 `/relay/<id>/<可读名>.mp4` 都返回 `<id>`。
pub fn relay_id_from_path(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/relay/")?;
    let id = rest.split('/').next().unwrap_or("");
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

/// 上游响应的 Content-Type 是否表示 HLS 播放列表。
pub fn is_hls_content_type(ct: &str) -> bool {
    let c = ct.to_ascii_lowercase();
    c.contains("mpegurl") || c.contains("vnd.apple.mpegurl")
}

/// 把 HLS 播放列表里的相对 URI 重写为绝对 URL。
///
/// 经 relay 转发后，PotPlayer 看到的播放列表地址是 `http://127.0.0.1:22349/relay/<id>/<名>.mp4`，
/// 若列表里是相对分片 URI，PotPlayer 会向 `/relay/<id>/xxx.ts` 请求，被 relay 误当成同 id 的
/// 播放列表请求。这里按最终上游地址（`base`，含 302 后的地址）解析成绝对 URL，
/// 让 PotPlayer 直接去源站取分片。仅用于「无 Referer/UA」的 HLS（有头时 launch 已提前报错）。
pub fn rewrite_hls_playlist(body: &str, base: &url::Url) -> String {
    let mut out = String::with_capacity(body.len() + 64);
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            out.push_str(line);
        } else if t.starts_with('#') {
            // #EXT-X-KEY / #EXT-X-MAP 的 URI="..." 也可能是相对地址（加密密钥、fMP4 初始化段）。
            out.push_str(&rewrite_uri_attributes(line, base));
        } else {
            match base.join(t) {
                Ok(abs) => out.push_str(abs.as_str()),
                Err(_) => out.push_str(line),
            }
        }
        out.push('\n');
    }
    out
}

/// 把一行标签里的 `URI="..."` 属性解析成绝对地址。
fn rewrite_uri_attributes(line: &str, base: &url::Url) -> String {
    let mut out = String::with_capacity(line.len() + 32);
    let mut rest = line;
    while let Some(pos) = rest.find("URI=\"") {
        let (before, after) = rest.split_at(pos + 5);
        out.push_str(before);
        if let Some(endq) = after.find('"') {
            let uri = &after[..endq];
            match base.join(uri) {
                Ok(abs) => out.push_str(abs.as_str()),
                Err(_) => out.push_str(uri),
            }
            out.push('"');
            rest = &after[endq + 1..];
        } else {
            out.push_str(after);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// 构建转发给源站的请求头（纯函数，可测）。
pub fn relay_upstream_headers(target: &RelayTarget, incoming_range: Option<&str>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(r) = target.headers.referer.as_deref().filter(|s| !s.is_empty()) {
        out.push(("referer".into(), r.to_string()));
    }
    if let Some(u) = target.headers.user_agent.as_deref().filter(|s| !s.is_empty()) {
        out.push(("user-agent".into(), u.to_string()));
    }
    for (k, v) in &target.headers.extra {
        out.push((k.clone(), v.clone()));
    }
    if let Some(r) = incoming_range.filter(|s| !s.is_empty()) {
        out.push(("range".into(), r.to_string()));
    }
    out
}

/// 惰性启动 relay 服务（幂等）。端口已被占用时视为已有实例在跑。
pub async fn ensure_relay() -> Result<(), String> {
    if RELAY_READY.load(Ordering::SeqCst) {
        return Ok(());
    }
    if RELAY_SERVER.get().is_some() {
        RELAY_READY.store(true, Ordering::SeqCst);
        return Ok(());
    }
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", RELAY_PORT)).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            RELAY_SERVER.set(()).ok();
            RELAY_READY.store(true, Ordering::SeqCst);
            return Ok(());
        }
        Err(e) => return Err(format!("启动播放转发服务失败（127.0.0.1:{RELAY_PORT}）：{e}")),
    };
    RELAY_SERVER.set(()).ok();
    RELAY_READY.store(true, Ordering::SeqCst);
    tauri::async_runtime::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tauri::async_runtime::spawn(async move {
                        let _ = handle_relay_conn(stream).await;
                    });
                }
                Err(_) => break,
            }
        }
    });
    crate::libmpv::trace(&format!("[relay] 本地转发服务已启动 127.0.0.1:{RELAY_PORT}"));
    Ok(())
}

fn parse_request(head: &str) -> Option<(String, String, HashMap<String, String>)> {
    let mut lines = head.split("\r\n");
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_ascii_uppercase();
    let target = parts.next()?.to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Some((method, target, headers))
}

async fn handle_relay_conn(mut stream: tokio::net::TcpStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // 读请求头（上限 64KB，10s 超时）
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_double_crlf(&buf) {
            break Some(pos);
        }
        if buf.len() > 64 * 1024 {
            break None;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut tmp)).await
        {
            Ok(Ok(0)) => break None,
            Ok(Ok(n)) => buf.extend_from_slice(&tmp[..n]),
            _ => break None,
        }
    };
    let Some(end) = header_end else {
        let _ = write_plain(&mut stream, 400, "Bad Request", b"").await;
        return;
    };
    let head = String::from_utf8_lossy(&buf[..end]);
    let Some((method, target_path, headers)) = parse_request(&head) else {
        let _ = write_plain(&mut stream, 400, "Bad Request", b"").await;
        return;
    };
    if method != "GET" && method != "HEAD" {
        let _ = write_plain(&mut stream, 405, "Method Not Allowed", b"").await;
        return;
    }
    let path = target_path.split('?').next().unwrap_or("");
    // 只取 `/relay/` 后第一段作为 id：可读文件名段（`/relay/<id>/<名>.mp4`）仅用于显示。
    let Some(id) = relay_id_from_path(path) else {
        let _ = write_plain(&mut stream, 404, "Not Found", b"relay target missing").await;
        return;
    };
    let Some(t) = relay_lookup(id) else {
        let _ = write_plain(&mut stream, 404, "Not Found", b"relay target expired").await;
        return;
    };

    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = write_plain(&mut stream, 502, "Bad Gateway", e.to_string().as_bytes()).await;
            return;
        }
    };
    let upstream_method = if method == "HEAD" {
        reqwest::Method::HEAD
    } else {
        reqwest::Method::GET
    };
    let mut req = client.request(upstream_method, &t.url);
    for (k, v) in relay_upstream_headers(&t, headers.get("range").map(|s| s.as_str())) {
        req = req.header(k, v);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            crate::libmpv::trace(&format!("[relay] 转发失败 {}: {e}", t.url));
            let _ = write_plain(&mut stream, 502, "Bad Gateway", format!("转发失败: {e}").as_bytes())
                .await;
            return;
        }
    };

    // HLS：播放列表里的相对分片 URI 要重写成绝对地址，否则 PotPlayer 会把
    // `/relay/<id>/xxx.ts` 当成同一 id 再请求一次播放列表。仅无头场景（有头 HLS 已提前报错）。
    let upstream_ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let upstream_is_hls = is_hls_content_type(&upstream_ct)
        || looks_like_hls(t.url.as_str())
        || looks_like_hls(resp.url().as_str());
    if upstream_is_hls && resp.status().is_success() && method != "HEAD" {
        let base = resp.url().clone();
        match resp.text().await {
            Ok(text) => {
                let rewritten = rewrite_hls_playlist(&text, &base);
                let body = rewritten.as_bytes();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if stream.write_all(head.as_bytes()).await.is_ok() {
                    let _ = stream.write_all(body).await;
                    let _ = stream.flush().await;
                }
            }
            Err(e) => {
                crate::libmpv::trace(&format!("[relay] 读取 HLS 播放列表失败 {}: {e}", t.url));
                let _ = write_plain(&mut stream, 502, "Bad Gateway", format!("读取播放列表失败: {e}").as_bytes())
                    .await;
            }
        }
        return;
    }

    let status = resp.status();
    let content_length = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut out_headers: Vec<(String, String)> = Vec::new();
    for name in [
        reqwest::header::CONTENT_TYPE,
        reqwest::header::CONTENT_RANGE,
        reqwest::header::ACCEPT_RANGES,
        reqwest::header::LAST_MODIFIED,
        reqwest::header::ETAG,
        reqwest::header::CACHE_CONTROL,
    ] {
        if let Some(v) = resp.headers().get(&name).and_then(|v| v.to_str().ok()) {
            out_headers.push((name.as_str().to_string(), v.to_string()));
        }
    }
    if let Some(cl) = &content_length {
        out_headers.push(("content-length".into(), cl.clone()));
    } else {
        out_headers.push(("transfer-encoding".into(), "chunked".into()));
    }
    out_headers.push(("connection".into(), "close".into()));

    let reason = status.canonical_reason().unwrap_or("");
    let mut head_out = format!("HTTP/1.1 {} {}\r\n", status.as_u16(), reason);
    for (k, v) in &out_headers {
        head_out.push_str(&format!("{k}: {v}\r\n"));
    }
    head_out.push_str("\r\n");
    if stream.write_all(head_out.as_bytes()).await.is_err() {
        return;
    }
    if method == "HEAD" {
        return;
    }

    // body：有 Content-Length 直接透传；没有则用 chunked 编码，避免整段缓冲。
    let chunked = content_length.is_none();
    let mut resp = resp;
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if chunked {
                    let prefix = format!("{:x}\r\n", chunk.len());
                    if stream.write_all(prefix.as_bytes()).await.is_err()
                        || stream.write_all(&chunk).await.is_err()
                        || stream.write_all(b"\r\n").await.is_err()
                    {
                        return;
                    }
                } else if stream.write_all(&chunk).await.is_err() {
                    return;
                }
            }
            Ok(None) => break,
            Err(e) => {
                crate::libmpv::trace(&format!("[relay] 读取上游中断: {e}"));
                break;
            }
        }
    }
    if chunked {
        let _ = stream.write_all(b"0\r\n\r\n").await;
    }
    let _ = stream.flush().await;
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_plain(
    stream: &mut tokio::net::TcpStream,
    code: u16,
    reason: &str,
    body: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

// ───────────────────────── 单测（纯逻辑，不触网） ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_format_is_hh_mm_ss() {
        assert_eq!(format_seek(0.0), "00:00:00");
        assert_eq!(format_seek(59.9), "00:00:59");
        assert_eq!(format_seek(3661.0), "01:01:01");
        assert_eq!(format_seek(36000.0), "10:00:00");
        assert_eq!(format_seek(-5.0), "00:00:00");
        assert_eq!(format_seek(f64::NAN), "00:00:00");
    }

    #[test]
    fn seek_skipped_near_end() {
        assert!(should_seek(10.0, 100.0));
        assert!(should_seek(98.0, 100.0)); // 恰好 98%
        assert!(!should_seek(99.0, 100.0)); // 太靠近片尾
        assert!(!should_seek(0.0, 100.0));
        // 时长未知时允许续播
        assert!(should_seek(10.0, 0.0));
    }

    #[test]
    fn potplayer_args_order_and_subs() {
        let subs = vec![
            r"C:\a b\sub.vtt".to_string(),
            "  ".to_string(),
            r"D:\danmaku.ass".to_string(),
        ];
        let args = build_potplayer_args("http://x/y.mp4", 61.0, 600.0, &subs);
        assert_eq!(
            args,
            vec![
                "http://x/y.mp4".to_string(),
                "/seek=00:01:01".to_string(),
                r"-sub=C:\a b\sub.vtt".to_string(),
                r"-sub=D:\danmaku.ass".to_string(),
            ]
        );
    }

    #[test]
    fn potplayer_args_without_seek_or_subs() {
        let args = build_potplayer_args("http://x/y.mp4", 0.0, 0.0, &[]);
        assert_eq!(args, vec!["http://x/y.mp4".to_string()]);
    }

    #[test]
    fn player_kind_defaults_to_mpv_and_resolves() {
        // [bundle-mpv] 默认 mpv_path 为空，真实路径由 config::load 的
        // resolve_bundled_paths 注入；这里模拟已解析后的配置。
        let mut cfg = Config::default();
        cfg.mpv_path = "mpv.exe".into();
        let (kind, path) = resolve_player(&cfg).unwrap();
        assert_eq!(kind, "mpv");
        assert_eq!(path, cfg.mpv_path);

        let mut c2 = Config::default();
        c2.player_kind = "potplayer".into();
        c2.potplayer_path = String::new();
        // 没装 PotPlayer 的环境返回 Err；装了则 Ok。两者都接受，但不能 panic。
        let _ = resolve_player(&c2);

        let mut c3 = Config::default();
        c3.player_kind = "weird".into();
        assert!(resolve_player(&c3).is_err());
    }

    #[test]
    fn validate_potplayer_rejects_non_exe_and_missing() {
        assert!(validate_potplayer_path("").is_err());
        assert!(validate_potplayer_path(r"C:\definitely\missing\PotPlayerMini64.exe").is_err());

        let dir = std::env::temp_dir().join(format!("anibox-potplayer-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let txt = dir.join("not-an-exe.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(validate_potplayer_path(txt.to_string_lossy().as_ref()).is_err());

        let exe = dir.join("PotPlayerMini64.EXE");
        std::fs::write(&exe, b"MZ").unwrap();
        let resolved = validate_potplayer_path(exe.to_string_lossy().as_ref()).unwrap();
        assert!(resolved.to_lowercase().ends_with("potplayermini64.exe"));
        assert!(!resolved.starts_with(r"\\?\"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relay_register_lookup_and_headers() {
        relay_clear();
        let target = RelayTarget {
            url: "https://example.test/a.m3u8".into(),
            headers: HttpHeaders::new(Some(" https://ref.test/ ".into()), Some(" UA/1 ".into())),
        };
        let id = relay_register(target.clone());
        assert_eq!(id.len(), 16);
        assert!(relay_url(&id).ends_with(&format!("/relay/{id}")));

        let got = relay_lookup(&id).expect("应能查回目标");
        assert_eq!(got.url, "https://example.test/a.m3u8");
        assert_eq!(got.headers.referer.as_deref(), Some("https://ref.test/"));
        assert_eq!(got.headers.user_agent.as_deref(), Some("UA/1"));

        let hs = relay_upstream_headers(&got, Some("bytes=0-1023"));
        assert_eq!(
            hs,
            vec![
                ("referer".to_string(), "https://ref.test/".to_string()),
                ("user-agent".to_string(), "UA/1".to_string()),
                ("range".to_string(), "bytes=0-1023".to_string()),
            ]
        );
        assert!(relay_lookup("nope").is_none());
        relay_clear();
    }

    #[test]
    fn headers_normalize_and_needs_relay() {
        let h = HttpHeaders::new(Some("  ".into()), None);
        assert!(!h.needs_relay());
        let h2 = HttpHeaders::new(None, Some("UA".into()));
        assert!(h2.needs_relay());
        assert_eq!(h2.user_agent.as_deref(), Some("UA"));
    }

    #[test]
    fn hls_detection() {
        assert!(looks_like_hls("https://x/a.m3u8"));
        assert!(looks_like_hls("https://x/a.M3U8?token=1"));
        assert!(looks_like_hls("https://x/a.m3u"));
        assert!(!looks_like_hls("https://x/a.mp4"));
        assert!(!looks_like_hls("https://x/a.m3u8x"));
    }

    #[test]
    fn display_name_sanitizes_and_falls_back() {
        assert_eq!(
            sanitize_display_name(" 葬送的芙莉莲 - S01E01: 冒险的终点 "),
            "葬送的芙莉莲 - S01E01_ 冒险的终点"
        );
        assert_eq!(
            sanitize_display_name(r#"a/b\c:d*e?f"g<h>i|j"#),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_display_name("   "), "video");
        assert_eq!(sanitize_display_name("..."), "video");
    }

    #[test]
    fn relay_url_named_encodes_readable_name() {
        let u = relay_url_named("abc123", "剧名 - S01E04: 集标题");
        assert!(u.starts_with("http://127.0.0.1:22349/relay/abc123/"));
        assert!(u.ends_with(".mp4"));
        assert!(!u.contains(' '));
        let last = u.rsplit('/').next().unwrap();
        let stem = last.trim_end_matches(".mp4");
        let decoded = urlencoding::decode(stem).unwrap();
        assert_eq!(decoded.as_ref(), "剧名 - S01E04_ 集标题");
    }

    #[test]
    fn relay_id_from_path_ignores_readable_tail() {
        assert_eq!(relay_id_from_path("/relay/abc"), Some("abc"));
        assert_eq!(relay_id_from_path("/relay/abc/%E5%89%A7%E5%90%8D.mp4"), Some("abc"));
        assert_eq!(relay_id_from_path("/relay/"), None);
        assert_eq!(relay_id_from_path("/other/abc"), None);
    }

    #[test]
    fn hls_playlist_rewrites_relative_uris() {
        let base = url::Url::parse("https://cdn.test/hls/index.m3u8").unwrap();
        let body = "#EXTM3U\n#EXTINF:10,\nseg-1.ts\n/abs/seg-2.ts\nhttps://other/x.ts\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n";
        let out = rewrite_hls_playlist(body, &base);
        assert!(out.contains("https://cdn.test/hls/seg-1.ts"));
        assert!(out.contains("https://cdn.test/abs/seg-2.ts"));
        assert!(out.contains("https://other/x.ts"));
        assert!(out.contains("URI=\"https://cdn.test/hls/key.bin\""));
        assert!(out.contains("#EXTINF:10,"));
        assert!(is_hls_content_type("application/vnd.apple.mpegurl"));
        assert!(!is_hls_content_type("video/mp4"));
    }

    #[test]
    fn parse_http_request_head() {
        let head = "GET /relay/abc?x=1 HTTP/1.1\r\nRange: bytes=10-\r\nUser-Agent: test\r\n";
        let (method, target, headers) = parse_request(head).unwrap();
        assert_eq!(method, "GET");
        assert_eq!(target, "/relay/abc?x=1");
        assert_eq!(headers.get("range").map(String::as_str), Some("bytes=10-"));
        assert_eq!(headers.get("user-agent").map(String::as_str), Some("test"));
    }

    /// 端到端：本地 relay 把 Range 与 Referer/UA 转发给上游，并把 206 原样回给客户端。
    /// 全部走 127.0.0.1，不触外网。
    #[tokio::test]
    async fn relay_forwards_range_and_headers() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = tx.send(req);
            let body = b"hello-range";
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-10/100\r\nContent-Type: video/mp4\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).await.unwrap();
            s.write_all(body).await.unwrap();
            s.flush().await.unwrap();
        });

        relay_clear();
        let id = relay_register(RelayTarget {
            url: format!("http://{addr}/video.mp4"),
            headers: HttpHeaders::new(Some("https://ref.test/".into()), Some("UA/9".into())),
        });
        ensure_relay().await.unwrap();

        let resp = reqwest::Client::new()
            .get(relay_url(&id))
            .header("range", "bytes=0-10")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp.headers().get("content-range").unwrap(),
            "bytes 0-10/100"
        );
        let body = resp.bytes().await.unwrap();
        assert_eq!(&body[..], b"hello-range");

        let upstream = rx.await.unwrap().to_ascii_lowercase();
        assert!(upstream.contains("range: bytes=0-10"), "Range 必须透传: {upstream}");
        assert!(upstream.contains("referer: https://ref.test/"), "Referer 必须转发: {upstream}");
        assert!(upstream.contains("user-agent: ua/9"), "UA 必须转发: {upstream}");
        relay_clear();
    }
}
