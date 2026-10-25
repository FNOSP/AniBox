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
use std::sync::atomic::{AtomicU64, Ordering};
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
    // [macos/linux] PotPlayer 是 Windows 专属软件，非 Windows 平台没有可执行文件。
    // 若用户的 config.json 是从 Windows 迁移来的（player_kind=potplayer），在 mac 上
    // 会直接报「未找到 PotPlayer」→ 无法播放。这里统一降级为随包/系统 mpv，
    // 与参考项目 Fntv-Plus 在 mac 上只用 mpv 的行为一致。
    #[cfg(not(target_os = "windows"))]
    if cfg.player_kind.trim().eq_ignore_ascii_case("potplayer") {
        return if cfg.mpv_path.trim().is_empty() {
            Err(crate::config::mpv_missing_message())
        } else {
            Ok(("mpv".into(), cfg.mpv_path.clone()))
        };
    }
    match cfg.player_kind.trim() {
        // [lc-1794] mpv-local：本机 mpv（路径已在 resolve_bundled_paths 解析进 mpv_path）
        "" | "mpv" | "mpv-local" => {
            if cfg.mpv_path.trim().is_empty() {
                Err(crate::config::mpv_missing_message())
            } else {
                Ok(("mpv".into(), cfg.mpv_path.clone()))
            }
        }
        "potplayer" => Ok(("potplayer".into(), resolve_potplayer_path(&cfg.potplayer_path)?)),
        other => Err(format!(
            "未知播放器类型：{other}（只允许 mpv/mpv-local/potplayer）"
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
        #[allow(unused_mut)]
        let mut kill = tokio::process::Command::new("taskkill");
        #[cfg(target_os = "windows")]
        kill.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let ok = kill
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            // PID 已失效/被重启：按镜像名兜底（只针对我们刚记录的 PotPlayer 进程）
            #[allow(unused_mut)]
            let mut kill2 = tokio::process::Command::new("taskkill");
            #[cfg(target_os = "windows")]
            kill2.creation_flags(0x0800_0000);
            let _ = kill2
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
/// - `direct`：`url` 是**源站直链**（电视直播 `live_channels[].path`）。为 `true` 时
///   PotPlayer 直接播该地址，不注册本地 relay（旧项目 lc-632：直播直链绕过 shim）。
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
    direct: bool,
) -> Result<(), String> {
    let (kind, path) = resolve_player(cfg)?;
    crate::libmpv::trace(&format!(
        "[player] launch kind={kind} path={path} url={url} resume={resume_secs} dur={duration_secs} playlist_start={playlist_start:?} direct={direct}"
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
                direct,
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
            // 多条目播放列表也先写「起播集」标题：容器 Title tag 会顶掉 #EXTINF，
            // 留空会让首集退化成容器标题（只剩剧名）。后续每一集由 mpv.rs 的
            // start-file/file-loaded 把该条 #EXTINF 标题逐集写回 force-media-title。
            let _ = m
                .command(json!(["set", "force-media-title", title]))
                .await;
            // [lc-1772] 在线源换片：先清弹幕脚本的会话内匹配缓存（弹弹play 干净标题等
            // 残留会把上一部片的弹幕搜给本片），必须在 loadfile 之前广播才赶在
            // file-loaded 自动流程之前生效。库内播放（headers=None）不广播：
            // NAS 连播场景弹幕自动跟随是既定行为。
            if headers.is_some() {
                m.command_no_reply(json!(["script-message", "reset-state"]))
                    .await;
            }
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
            // 多条目播放列表：命令行仍带起播集标题（容器 Title tag 会顶掉 #EXTINF），
            // 后续每集由 mpv.rs 在 start-file/file-loaded 时按 #EXTINF 逐集写回。
            crate::mpv::MpvSession::launch(
                app.clone(),
                &cfg.mpv_path,
                Some(url.to_string()),
                Some(title.to_string()),
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
    direct: bool,
) -> Result<(), String> {
    // 1) 新播放前清理上一个 PotPlayer（PotPlayer 默认单实例，残留会抢新参数）。
    stop_potplayer(app, state, false).await;

    // [lc-1749] 旧版在这里「HLS + 需要头 → 直接报错拒播」：webrs 的 UA 恒非空
    // （effective_ua 有随机兜底），needs_relay() 恒真 ⇒ **所有 m3u8 在线源**在
    // PotPlayer 模式下一律失败（用户报的「调用 potplayer 播放失败」主根因）。
    // 现在 relay 会把 HLS 分段/密钥也一并代转（带同一套头，见 handle_relay_conn），
    // 不再需要前置拦截；真正的失败由 potplayer-relay-error 事件回传面板。

    // 2) 常规媒体：经本地 relay 转发，并在 URL 路径里附带可读文件名：
    //    PotPlayer 把 URL 末段当作标题/播放列表项名，直链（GUID/查询串）会显示成一串字符。
    //    与旧项目 playbackShim 同思路：`/relay/<id>/<剧名 - 集名>.mp4`，relay 仍只按 id 转发。
    //    无 Referer/UA 时 relay 仅做本机逐字节转发（Range 透传），不影响 /seek、-sub。
    //
    //    [live] `direct=true`（电视直播）：旧项目 lc-632 实测直播直链必须**原样**交给
    //    PotPlayer —— 直播源是 1905 网关 302 → 外部 CDN HLS，经 relay 包一层既无必要，
    //    还会把播放列表地址变成 relay 地址，源站 302/分片鉴权容易失败。
    let play_url = if direct {
        crate::libmpv::trace(&format!(
            "[player] potplayer 直播直链（跳过 relay）: {url}（title={title:?}）"
        ));
        url.to_string()
    } else {
        ensure_relay_with(app).await?;
        let relay_headers = headers.cloned().unwrap_or_default();
        let id = relay_register(RelayTarget {
            url: url.to_string(),
            headers: relay_headers,
        });
        // [lc-1749] 上游是 HLS 就用 .m3u8 扩展名（PotPlayer 按 URL 扩展挑分离器）。
        let ext = if looks_like_hls(url) { "m3u8" } else { "mp4" };
        let u = relay_url_named_ext(&id, title, ext);
        crate::libmpv::trace(&format!(
            "[player] potplayer relay 注册: {url} -> {u}（referer={:?} ua={:?} title={title:?}）",
            headers.and_then(|h| h.referer.as_deref()),
            headers.and_then(|h| h.user_agent.as_deref()),
        ));
        u
    };

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
    // potctl 是 Windows 专用助手（PotPlayer 只有 Windows 版），非 Windows 直接跳过。
    if !cfg!(target_os = "windows") {
        return None;
    }
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
                        // [lc-1751] ended 已结算（含面板连播到下一集）后冻结回写：`/Current /add`
                        // 追加的下一集起播后位置归零，继续 tick 会把「下一集的位置」写成本集
                        // 进度，污染 fnOS「继续观看」记录（本集在 ended 时已按近片尾结算）。
                        if !ended_sent {
                            crate::fnos_progress::tick(&app, pos, dur);
                        }
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

// ───────────────────────── [lc-1751] PotPlayer 整季面板 ─────────────────────────

/// PotPlayer 整季面板追加顺序：点击集（索引 `clicked`）已在播，其余集按**自然集序**
/// 尾部环绕排列 —— `[clicked+1..len, 0..clicked]`。对标 Fntv-Plus potplayer.ts 的
/// rotate（`slice(pos) + slice(0, pos)`）后 `/add` 除首集外全部：本集播完 PotPlayer
/// 顺着面板连播到下一集，而不是从头一集。
pub fn panel_append_order(len: usize, clicked: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let c = clicked.min(len - 1);
    ((c + 1)..len).chain(0..c).collect()
}

/// 当前 PotPlayer 代次号（`launch_potplayer`/`stop_potplayer` 每次自增）。
/// 面板追加任务延时结束后据此判断「本集会话是否已被下一次播放/停止取代」。
pub fn potplayer_generation() -> u64 {
    POTPLAYER_GEN.load(Ordering::SeqCst)
}

/// [lc-1751] 把整季其余集经 `/Current /add` 追加进**正在运行**的 PotPlayer 播放列表面板
/// （对标 Fntv-Plus populatePlaylistPanel，display + PotPlayer 原生顺序连播）。
///
/// · 延时 ~1.2s：起播实例刚 spawn 时单实例握手未就绪，立刻 `/Current` 会丢参数
///   （原项目实测用 900ms，这里稍放宽）。
/// · 代次号校验：延时期间用户点了别的集 / 停止播放 → 本次追加作废，避免把旧剧的
///   集追加进新会话的面板。
/// · 转发进程（PotPlayerMini64.exe /Current …）把参数交给运行中的实例后自行退出，
///   不跟踪、不等待（与原项目 spawn + unref 同款）。
pub fn schedule_potplayer_panel_append(path: String, urls: Vec<String>, generation: u64) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        if POTPLAYER_GEN.load(Ordering::SeqCst) != generation {
            crate::libmpv::trace("[player] PotPlayer 面板追加取消：播放会话已切换");
            return;
        }
        let mut args: Vec<String> = Vec::with_capacity(urls.len() * 2 + 1);
        args.push("/Current".into());
        for u in &urls {
            args.push("/add".into());
            args.push(u.clone());
        }
        let mut command = std::process::Command::new(&path);
        command
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0800_0000);
        }
        match command.spawn() {
            Ok(child) => {
                let pid = child.id();
                drop(child); // 转发进程自己退出，不等待
                crate::libmpv::trace(&format!(
                    "[player] PotPlayer 面板追加已转发 pid={pid}（{} 集）",
                    urls.len()
                ));
            }
            Err(e) => {
                crate::libmpv::trace(&format!(
                    "[player] PotPlayer 面板追加拉起失败（不影响当前播放）: {e}"
                ));
            }
        }
    });
}

async fn pid_alive(pid: u32) -> bool {
    let mut probe = tokio::process::Command::new("tasklist");
    #[cfg(target_os = "windows")]
    probe.creation_flags(0x0800_0000); // CREATE_NO_WINDOW：轮询高频，不隐藏会狂闪 cmd 窗
    let out = probe
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

// [lc-1749] 不再提供 relay_clear()：单测并行跑时，一个测试 clear 全局表会与另一个
// 测试的 register→lookup 窗口交错（实测表现为 relay e2e 随机 404「target expired」，
// 15 次里中 1~2 次，干净分支同样复现）。各测试用随机 id 注册、只查自己的 id 即可，
// 表在本进程内多留几条测试目标没有影响。

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
/// （播放路径已改用 [`relay_url_named_ext`] 按内容挑扩展名；本函数保留给测试/调试。）
#[allow(dead_code)]
pub fn relay_url_named(id: &str, display_name: &str) -> String {
    relay_url_named_ext(id, display_name, "mp4")
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

/// 把 HLS 播放列表里的相对 URI 重写为绝对 URL；有 `wrap_prefix` 时再包一层 relay。
///
/// 经 relay 转发后，PotPlayer 看到的播放列表地址是 `http://127.0.0.1:22349/relay/<id>/<名>.mp4`，
/// 若列表里是相对分片 URI，PotPlayer 会向 `/relay/<id>/xxx.ts` 请求，被 relay 误当成同 id 的
/// 播放列表请求。这里按最终上游地址（`base`，含 302 后的地址）解析成绝对 URL，再按需包装：
/// · `wrap_prefix = None`（无头场景）：让 PotPlayer 直接去源站取分片，省一层本机转发；
/// · `wrap_prefix = Some("http://127.0.0.1:22349/relay/<id>")`（[lc-1749] 有头场景）：
///   分片/密钥/子播放列表改写成 `<prefix>/~u/<percent-encoded 绝对URL>` —— PotPlayer
///   仍请求 relay，relay 解码出绝对地址后**带同一套 Referer/UA** 代取（对齐 mpv 链路：
///   ffmpeg(hls) 的分段请求同样带播放头）。没有这层包装，防盗链 CDN 会对分段 403，
///   表现为「列表能打开、一播就失败」。
pub fn rewrite_hls_playlist(body: &str, base: &url::Url, wrap_prefix: Option<&str>) -> String {
    let abs = |u: &str| -> String {
        match base.join(u) {
            Ok(a) => match wrap_prefix {
                Some(p) => format!("{p}/~u/{}", urlencoding::encode(a.as_str())),
                None => a.to_string(),
            },
            Err(_) => u.to_string(),
        }
    };
    let mut out = String::with_capacity(body.len() + 64);
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            out.push_str(line);
        } else if t.starts_with('#') {
            // #EXT-X-KEY / #EXT-X-MAP 的 URI="..." 也可能是相对地址（加密密钥、fMP4 初始化段）。
            out.push_str(&rewrite_uri_attributes(line, &abs));
        } else {
            out.push_str(&abs(t));
        }
        out.push('\n');
    }
    out
}

fn rewrite_uri_attributes(line: &str, abs: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(line.len() + 32);
    let mut rest = line;
    while let Some(pos) = rest.find("URI=\"") {
        let (before, after) = rest.split_at(pos + 5);
        out.push_str(before);
        if let Some(endq) = after.find('"') {
            let uri = &after[..endq];
            out.push_str(&abs(uri));
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

/// [lc-1749] 从请求目标中解析 relay 子资源（HLS 分段/密钥/子播放列表）：
/// `/relay/<id>/~u/<percent-encoded 绝对URL>` → 还原出绝对地址。
/// 不带 `~u/` 段（纯 id 或可读文件名）→ None = 播放列表本体，按 id 转发注册地址。
/// 可读文件名经 [`urlencoding::encode`] 后是单个路径段、不以 `~u/` 开头，无歧义。
///
/// [lc-1751] 入参改为**未截断的原始请求目标**（`target`，含查询串）：部分播放器
/// （含 PotPlayer 的部分形态）会把百分号编码先解码成原文再发请求 —— 包装后的绝对
/// URL 变成 `~u/https://x/a.ts?tok=1`，查询串在 `?` 之后。若调用方先按 `?` 截断，
/// token 会被整段丢掉（上游 403）。这里取 `~u/` 之后的**全部剩余内容**再解码：
/// · 编码形态：`%3Ftok%3D1` 在路径里，解码后还原成 `?tok=1`（与旧行为一致）；
/// · 解码形态：字面 `?tok=1` 原样保留。
pub fn relay_sub_resource(target: &str, id: &str) -> Option<String> {
    let rest = target.strip_prefix(&format!("/relay/{id}/"))?;
    let enc = rest.strip_prefix("~u/")?;
    let decoded = urlencoding::decode(enc).ok()?.to_string();
    if decoded.starts_with("http://") || decoded.starts_with("https://") {
        Some(decoded)
    } else {
        None
    }
}

/// [lc-1749] 指定扩展名版本：上游是 HLS 时用 `.m3u8`（PotPlayer 按 URL 扩展名挑
/// 分离器，`.mp4` 名字配 m3u8 内容依赖内容嗅探；给对扩展名不赌运气）。
pub fn relay_url_named_ext(id: &str, display_name: &str, ext: &str) -> String {
    let stem = sanitize_display_name(display_name);
    let encoded = urlencoding::encode(&stem);
    format!("http://127.0.0.1:{RELAY_PORT}/relay/{id}/{encoded}.{ext}")
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

/// [lc-1749] relay 服务持有的 AppHandle：转发失败/上游拒流时 emit
/// `potplayer-relay-error` 给前端（PotPlayer 自己不会把错误报到面板）。
static RELAY_APP: OnceLock<StdMutex<Option<AppHandle>>> = OnceLock::new();

fn relay_app_handle() -> Option<AppHandle> {
    RELAY_APP
        .get_or_init(|| StdMutex::new(None))
        .lock()
        .ok()
        .and_then(|g| g.clone())
}

/// 带 AppHandle 的 [`ensure_relay`]：注册转发失败事件所需的句柄（播放路径统一走这个）。
pub async fn ensure_relay_with(app: &AppHandle) -> Result<(), String> {
    if let Ok(mut g) = RELAY_APP
        .get_or_init(|| StdMutex::new(None))
        .lock()
    {
        if g.is_none() {
            *g = Some(app.clone());
        }
    }
    ensure_relay().await
}

/// 惰性启动 relay 服务（幂等、自愈）。端口已被占用时视为已有实例在跑。
///
/// [lc-1749] 不再用「OnceLock/AtomicBool 记住已启动」的短路标志：accept 循环可能
/// 随所属 runtime 一起消亡（单测里每个 `#[tokio::test]` 一个 runtime，上一次测试
/// 结束后监听就没了），旧标志却仍声称服务在跑 → 后续请求一律连接被拒且永不自愈。
/// 现在每次调用都**尝试重新 bind**：端口仍被占（服务还活着）→ 视为已有实例；
/// 端口空闲（旧循环已死）→ 重新起一个。bind 一次的开销可忽略（每次播放才调一次）。
pub async fn ensure_relay() -> Result<(), String> {
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", RELAY_PORT)).await {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // 已有监听则复用。真伪校验：连一下，能通 = 有活实例（本进程旧循环或另一
            // 个 AniBox）；连不上 = 端口被 TIME_WAIT 之类的残连接占着（刚退出/刚跑过
            // 单测的 60s 内常见），此时绝不能谎报成功，否则后续请求全部落空。
            match tokio::net::TcpStream::connect(("127.0.0.1", RELAY_PORT)).await {
                Ok(_) => return Ok(()),
                Err(_) => {
                    return Err(format!(
                        "播放转发端口 127.0.0.1:{RELAY_PORT} 被占用且无服务应答（可能是刚退出的残留连接，稍等约 1 分钟再试）"
                    ))
                }
            }
        }
        Err(e) => return Err(format!("启动播放转发服务失败（127.0.0.1:{RELAY_PORT}）：{e}")),
    };
    // [lc-1749] 用 tokio::spawn（当前 runtime）而不是 tauri::async_runtime::spawn：
    // 生产里两者等价（ensure_relay 只会在 tauri 命令上下文被调）；但 listener 是在
    // **当前 runtime** 上 bind 的，跨 runtime 轮询 IO 会随机失效（单测里每个
    // #[tokio::test] 一个 runtime，全局 runtime 上的 accept 任务摸着别的 runtime 的
    // listener，服务一两个连接后就死 → 连接被拒）。同 runtime 内 spawn 才稳定。
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(async move {
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

    // [lc-1749] 子资源（`~u`）：HLS 分段/密钥/子播放列表的代取地址；无 `~u` 段则
    // 转发注册的原始地址（播放列表本体）。两类请求带**同一套** Referer/UA —— 与
    // mpv 链路一致（ffmpeg(hls) 分段请求也带播放头）。
    // [lc-1751] 子资源解析改用**未截断的原始 target**（含查询串）：PotPlayer 若把
    // 百分号编码解码成原文再发请求，`~u/https://x/a.ts?tok=1` 的 `?tok=1` 是查询串，
    // 先按 `?` 截断再解析会丢 token → 分段 403。见 [`relay_sub_resource`]。
    let sub = relay_sub_resource(&target_path, id);
    let upstream_url = sub.clone().unwrap_or_else(|| t.url.clone());
    // [lc-1751] 防御性校验：转发目标必须是 http/https。真机事故根因（lc-1751 修复前）：
    // 多集库播放把本地 m3u **文件路径**注册成 relay 目标，reqwest 构建 URL 直接失败，
    // 每个请求都 502，PotPlayer 只会报「服务已关闭或地址错误」。这里把同类错误前置成
    // 可诊断的 400 + potplayer-relay-error 事件，不再让播放器只看到含糊的 502。
    if !upstream_url.starts_with("http://") && !upstream_url.starts_with("https://") {
        crate::libmpv::trace(&format!(
            "[relay] 转发失败拦截：目标不是 http/https 地址: {}",
            t.url
        ));
        if sub.is_none() {
            emit_relay_error(&t.url, "转发目标不是 http/https 地址（本地文件路径不可经 relay 转发）".into());
        }
        let _ = write_plain(&mut stream, 400, "Bad Request", b"relay target must be http(s)").await;
        return;
    }

    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        // [lc-1749] 口径对齐：webrs 的 http() 与 mpv/ffmpeg 默认都不校验证书，
        // 在线源 CDN 常见自签/过期证书 —— relay 若校验，就会出现「mpv 能播、
        // PotPlayer 502(证书错误)」的分叉。只留连接超时，**不设总超时**（正片
        // 可能要连续转发数小时）。
        .danger_accept_invalid_certs(true)
        .connect_timeout(std::time::Duration::from_secs(10))
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
    let mut req = client.request(upstream_method, &upstream_url);
    for (k, v) in relay_upstream_headers(&t, headers.get("range").map(|s| s.as_str())) {
        req = req.header(k, v);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            crate::libmpv::trace(&format!("[relay] 转发失败 {upstream_url}: {e}"));
            if sub.is_none() {
                emit_relay_error(&t.url, format!("转发失败: {e}"));
            }
            let _ = write_plain(&mut stream, 502, "Bad Gateway", format!("转发失败: {e}").as_bytes())
                .await;
            return;
        }
    };
    // 播放列表本体被上游拒绝（4xx/5xx）→ PotPlayer 只会在自己窗口里报错，面板一无所知。
    // 回传一条失败事件（分段级失败不回传，避免几百个分片刷屏）。
    if sub.is_none() && !resp.status().is_success() {
        let status = resp.status();
        crate::libmpv::trace(&format!("[relay] 上游拒绝 {} → {status}", t.url));
        emit_relay_error(&t.url, format!("上游返回 {status}"));
    }

    // HLS：播放列表里的 URI 要重写，否则 PotPlayer 会把 `/relay/<id>/xxx.ts`
    // 当成同一 id 再请求一次播放列表。有头场景把 URI 包回 relay（`~u`，分段也带头）；
    // 无头场景绝对化直连源站（省一层本机转发）。
    let upstream_ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let upstream_is_hls = is_hls_content_type(&upstream_ct)
        || looks_like_hls(upstream_url.as_str())
        || looks_like_hls(resp.url().as_str());
    if upstream_is_hls && resp.status().is_success() && method != "HEAD" {
        let base = resp.url().clone();
        match resp.text().await {
            Ok(text) => {
                let wrap_prefix = t
                    .headers
                    .needs_relay()
                    .then(|| format!("http://127.0.0.1:{RELAY_PORT}/relay/{id}"));
                let rewritten = rewrite_hls_playlist(&text, &base, wrap_prefix.as_deref());
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

/// [lc-1749] relay 播放列表级失败 → 前端（PotPlayer 链路没有 mpv-end，失败原本
/// 只能靠用户肉眼看 PotPlayer 窗口里的报错）。WebrsPanel 按 60s 起播窗口归因展示。
fn emit_relay_error(url: &str, error: String) {
    if let Some(app) = relay_app_handle() {
        let _ = app.emit("potplayer-relay-error", json!({ "url": url, "error": error }));
    }
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
        // [lc-1749] 不 clear 全局表（并行测试会互相清掉对方刚注册的目标 → e2e 随机 404）。
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
        // 无头场景：绝对化直连源站（旧行为不变）
        let out = rewrite_hls_playlist(body, &base, None);
        assert!(out.contains("https://cdn.test/hls/seg-1.ts"));
        assert!(out.contains("https://cdn.test/abs/seg-2.ts"));
        assert!(out.contains("https://other/x.ts"));
        assert!(out.contains("URI=\"https://cdn.test/hls/key.bin\""));
        assert!(out.contains("#EXTINF:10,"));
        assert!(is_hls_content_type("application/vnd.apple.mpegurl"));
        assert!(!is_hls_content_type("video/mp4"));
    }

    #[test]
    fn relay_sub_resource_parses_wrapped_and_ignores_names() {
        let enc = urlencoding::encode("https://cdn.test/hls/seg-1.ts?token=a b");
        assert_eq!(
            relay_sub_resource(&format!("/relay/abc/~u/{enc}"), "abc").as_deref(),
            Some("https://cdn.test/hls/seg-1.ts?token=a b")
        );
        assert_eq!(relay_sub_resource("/relay/abc/%E5%89%A7%E5%90%8D.mp4", "abc"), None);
        assert_eq!(relay_sub_resource("/relay/abc", "abc"), None);
        assert_eq!(relay_sub_resource("/relay/abc/~u/ftp://x", "abc"), None);
    }

    /// [lc-1751] 解码形态：播放器把百分号编码解码成原文再发请求时，`~u` 后是字面
    /// URL（含 `?query`）。解析必须保留查询串（丢 token = 分段 403）。
    #[test]
    fn relay_sub_resource_keeps_query_in_decoded_form() {
        // 解码形态（字面 URL，查询串在 ? 之后）
        assert_eq!(
            relay_sub_resource("/relay/abc/~u/https://cdn.test/hls/seg-1.ts?token=ab&sig=1", "abc")
                .as_deref(),
            Some("https://cdn.test/hls/seg-1.ts?token=ab&sig=1")
        );
        // 编码形态（%3F 在路径里）——与旧行为一致
        let enc = urlencoding::encode("https://cdn.test/hls/seg-1.ts?token=ab");
        assert_eq!(
            relay_sub_resource(&format!("/relay/abc/~u/{enc}"), "abc").as_deref(),
            Some("https://cdn.test/hls/seg-1.ts?token=ab")
        );
        // 可读名段带了无关查询串也不影响（无 ~u → 播放列表本体）
        assert_eq!(
            relay_sub_resource("/relay/abc/%E5%89%A7%E5%90%8D.mp4?range=0-", "abc"),
            None
        );
    }

    /// [lc-1751] PotPlayer 面板追加顺序：自然集序尾部环绕（对标 Fntv-Plus rotate+/add）。
    #[test]
    fn panel_append_order_wraps_naturally() {
        assert_eq!(panel_append_order(0, 0), Vec::<usize>::new());
        assert_eq!(panel_append_order(1, 0), Vec::<usize>::new());
        assert_eq!(panel_append_order(3, 0), vec![1, 2]);
        assert_eq!(panel_append_order(3, 1), vec![2, 0]);
        assert_eq!(panel_append_order(3, 2), vec![0, 1]);
        assert_eq!(panel_append_order(5, 2), vec![3, 4, 0, 1]);
        // clicked 越界钳制到最后一个
        assert_eq!(panel_append_order(3, 9), vec![0, 1]);
    }

    #[test]
    fn relay_url_named_ext_picks_hls_extension() {
        assert!(relay_url_named_ext("abc123", "ju", "m3u8").ends_with(".m3u8"));
    }

    /// [lc-1749] 有头场景：分片/密钥 URI 一律包回 relay（`~u`），分段请求才能带头。
    #[test]
    fn hls_playlist_wraps_uris_through_relay_when_headers() {
        let base = url::Url::parse("https://cdn.test/hls/index.m3u8").unwrap();
        let body = "#EXTM3U\n#EXTINF:10,\nseg-1.ts\nhttps://other/x.ts\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n";
        let prefix = "http://127.0.0.1:22349/relay/abcd0123abcd0123";
        let out = rewrite_hls_playlist(body, &base, Some(prefix));
        let wrap = |u: &str| format!("{prefix}/~u/{}", urlencoding::encode(u));
        assert!(out.contains(&wrap("https://cdn.test/hls/seg-1.ts")));
        assert!(out.contains(&wrap("https://other/x.ts")));
        assert!(out.contains(&format!("URI=\"{}\"", wrap("https://cdn.test/hls/key.bin"))));
        // 包装出的分片路径要能被 relay_sub_resource 完整还原（含查询串）
        let seg = wrap("https://cdn.test/hls/seg-1.ts");
        let path = seg.trim_start_matches("http://127.0.0.1:22349");
        assert_eq!(
            relay_sub_resource(path, "abcd0123abcd0123").as_deref(),
            Some("https://cdn.test/hls/seg-1.ts")
        );
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
    /// [lc-1749] 追加 HLS 分段代转段：有头 HLS 的播放列表分片被包回 relay（`~u`），
    /// 客户端（模拟 PotPlayer）请求包装后的分片地址时，relay 解码绝对地址并带**同一套
    /// Referer/UA** 代取 —— 这是「HLS 在线源在 PotPlayer 模式可播」的关键（旧版在此
    /// 直接拒播）。全部走 127.0.0.1，不触外网。
    /// [lc-1751] 追加三段真实时序场景：
    ///   ⓪ **秒连竞态**：`ensure_relay` 返回后**立即**并发连接（模拟 PotPlayer 秒开）——
    ///      监听 bind 在 ensure_relay 内同步完成、accept 循环异步起步，连接先进内核
    ///      backlog，8 连发必须全部 200（此前真机 502 的怀疑方向之一，就此钉死）；
    ///   ③ **非 http(s) 目标回归**：本地文件路径被误注册成转发目标（lc-1751 修复前的
    ///      真机根因）必须立刻 400 + 明确文案，不再是含糊的 502 builder error；
    ///   ④ **无头 HLS 直通**（库内 NAS-HLS 经 Go 代理的形态）：无 Referer/UA 时分片
    ///      绝对化直连源站、不包 `~u`。
    ///
    /// 注意：所有 e2e 流程合在**同一个** #[tokio::test] 里 —— relay 监听 22349 的
    /// 生命周期跟所属 runtime 走，拆成多个测试在 `--test-threads=1` 下必挂（上一个
    /// 测试的 runtime 连同监听一起销毁，端口又卡 TIME_WAIT，本测试重绑失败）。
    #[tokio::test]
    async fn relay_forwards_range_and_headers() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // 假源站 ①：mp4 直链（Range 场景），收 1 个连接，请求头经 oneshot 回传。
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

        // 假源站 ②：/hls/index.m3u8 返回相对分片列表；其余路径返回分片字节。
        // 收 2 个连接（播放列表 + 分片），各记一份请求头。
        let hls_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let hls_addr = hls_listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen_c = seen.clone();
        tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((mut s, _)) = hls_listener.accept().await else { break };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                seen_c.lock().unwrap().push(req.to_ascii_lowercase());
                let is_playlist = req.contains("index.m3u8");
                let (ct, body) = if is_playlist {
                    ("application/vnd.apple.mpegurl", "#EXTM3U\n#EXTINF:10,\nseg-1.ts\n")
                } else {
                    ("video/mp2t", "SEGDATA")
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                s.write_all(head.as_bytes()).await.unwrap();
                s.write_all(body.as_bytes()).await.unwrap();
                s.flush().await.unwrap();
                drop(s);
            }
        });

        // 假源站 ③（秒连竞态用）：无限接受连接，每连接回 1 字节 mp4。
        let burst_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let burst_addr = burst_listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = burst_listener.accept().await else { break };
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let head = "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Type: video/mp4\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(b"B").await;
                let _ = s.flush().await;
            }
        });

        // 假源站 ④（无头 HLS 直通用）：单连接，返回相对分片播放列表。
        let plain_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let plain_addr = plain_listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = plain_listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let body = "#EXTM3U\n#EXTINF:10,\nseg-x.ts\n";
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(body.as_bytes()).await;
                let _ = s.flush().await;
            }
        });

        // （[lc-1749] 不 clear 全局表：并行测试会互相清掉对方的目标 → 随机 404）

        // ── ⓪ [lc-1751] 秒连竞态：bind 完成后**零等待**并发连接，必须全部 200 ──
        let burst_id = relay_register(RelayTarget {
            url: format!("http://{burst_addr}/burst.mp4"),
            headers: HttpHeaders::default(),
        });
        ensure_relay().await.unwrap();
        let mut burst = Vec::new();
        for _ in 0..8 {
            let u = relay_url(&burst_id);
            burst.push(tokio::spawn(async move {
                reqwest::Client::new().get(&u).send().await.unwrap().status()
            }));
        }
        for h in burst {
            assert_eq!(
                h.await.unwrap(),
                reqwest::StatusCode::OK,
                "ensure_relay 返回后立即连接（PotPlayer 秒开时序）必须成功"
            );
        }

        // ── ① Range/头透传（原有断言，保持不变）──
        let id = relay_register(RelayTarget {
            url: format!("http://{addr}/video.mp4"),
            headers: HttpHeaders::new(Some("https://ref.test/".into()), Some("UA/9".into())),
        });
        // ⓪ 已 ensure 过：这里走 AddrInUse → 连接探测 → 复用路径（生产里每次播放都会经过）
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

        // ── ② [lc-1749] 有头 HLS：分片包回 relay 代转 ──
        let hls_id = relay_register(RelayTarget {
            url: format!("http://{hls_addr}/hls/index.m3u8"),
            headers: HttpHeaders::new(Some("https://ref.test/".into()), Some("UA/9".into())),
        });
        // 播放列表：分片被包回 relay
        let resp = reqwest::Client::new().get(relay_url(&hls_id)).send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        assert!(resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("mpegurl"));
        let body = resp.text().await.unwrap();
        let seg_line = body
            .lines()
            .find(|l| l.starts_with(&format!("http://127.0.0.1:{RELAY_PORT}/")))
            .expect("分片 URI 应被包回 relay");
        let expected = format!(
            "http://127.0.0.1:{RELAY_PORT}/relay/{hls_id}/~u/{}",
            urlencoding::encode(&format!("http://{hls_addr}/hls/seg-1.ts"))
        );
        assert_eq!(seg_line, &expected, "播放列表分片应包装为 ~u 形态: {body}");

        // 请求包装后的分片地址 → relay 解码绝对地址代取，字节原样回传
        let resp2 = reqwest::Client::new().get(seg_line).send().await.unwrap();
        assert_eq!(resp2.status(), reqwest::StatusCode::OK);
        assert_eq!(&resp2.bytes().await.unwrap()[..], b"SEGDATA");

        // HLS 的两次上游请求（列表 + 分片）都必须带 Referer/UA
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 2, "应恰好发生两次上游请求: {reqs:?}");
        for r in reqs.iter() {
            assert!(r.contains("referer: https://ref.test/"), "Referer 必须代转: {r}");
            assert!(r.contains("user-agent: ua/9"), "UA 必须代转: {r}");
        }
        drop(reqs);

        // ── ③ [lc-1751] 回归：本地文件路径被误注册成转发目标（真机根因形态）→ 400 ──
        let bad_id = relay_register(RelayTarget {
            url: r"C:\Users\x\AppData\Local\Temp\anibox-playlist.m3u".into(),
            headers: HttpHeaders::default(),
        });
        let resp = reqwest::Client::new().get(relay_url(&bad_id)).send().await.unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "非 http(s) 转发目标必须 400（修复前是 502 builder error → PotPlayer「服务已关闭或地址错误」）"
        );
        let body = resp.bytes().await.unwrap();
        assert_eq!(&body[..], b"relay target must be http(s)");

        // ── ④ [lc-1751] 无头 HLS 直通（库内 NAS-HLS 形态）：分片绝对化、不包 ~u ──
        let plain_id = relay_register(RelayTarget {
            url: format!("http://{plain_addr}/hls/index.m3u8"),
            headers: HttpHeaders::default(),
        });
        let resp = reqwest::Client::new().get(relay_url(&plain_id)).send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let body = resp.text().await.unwrap();
        assert!(
            body.contains(&format!("http://{plain_addr}/hls/seg-x.ts")),
            "相对分片应绝对化为源站地址: {body}"
        );
        assert!(!body.contains("~u/"), "无头场景不应包 ~u: {body}");
    }
}
