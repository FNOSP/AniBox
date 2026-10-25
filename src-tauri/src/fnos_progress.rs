//! [progress] 播放进度回传飞牛 —— 「继续观看」的真正数据源。
//!
//! 背景（与旧项目 Fntv-Plus 对齐）：
//!   · 首页「继续观看」= `get_continue_watching` → 飞牛 `GET /v/api/v1/play/list`，
//!     这是**服务端**的播放进度台账；只写本地 `history.json` 永远不会让它变化。
//!   · 旧项目在播放器进度事件里调 `recordPlayStatus` → `POST /v/api/v1/play/record`
//!     （`src/main/handlers/plugins/media.ts` 的 `eventHandler`，字段见
//!     `src/modules/fn_api/types.ts` 的 `PlayStatusData`）。本模块即该链路的 Rust 版。
//!
//! 为什么需要 play/info：
//!   `play/record` 的 body 要求 `media_guid/video_guid/audio_guid/subtitle_guid`，这些只有
//!   `play/info` 响应里有。起播时 `resolve_play_guid` 已经拿到过一份，故先 [`remember`] 缓存；
//!   整季播放列表自动切到下一集时缓存里没有，再按需补一次 `play/info`（成功后也缓存）。
//!
//! 失败策略：只记 trace、绝不 panic、绝不阻塞播放；调用方据此决定是否广播刷新事件。

use crate::config::Config;
use crate::fnapi;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

/// 一条可回传的播放元数据（来自 `play/info` 顶层）。
#[derive(Clone, Debug, Default)]
pub struct PlayMeta {
    /// 条目 guid（= play/info 的 `guid`，也是代理直链路径里的那个 guid）
    pub item_guid: String,
    pub media_guid: String,
    pub video_guid: String,
    pub audio_guid: String,
    pub subtitle_guid: String,
}

fn cache() -> &'static Mutex<HashMap<String, PlayMeta>> {
    static CACHE: OnceLock<Mutex<HashMap<String, PlayMeta>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 起播时调用：把 `play/info` 已拿到的流 guid 记下来，首帧进度回传零额外请求。
pub fn remember(meta: PlayMeta) {
    if meta.item_guid.is_empty() {
        return;
    }
    if let Ok(mut g) = cache().lock() {
        g.insert(meta.item_guid.clone(), meta);
    }
}

/// 取服务器 host（旧项目 `play_link` = `new URL(getVideoUrl(media_guid)).hostname`，
/// 注意 `hostname` **不含端口**，这里保持一致）。
fn host_of(server: &str) -> String {
    let s = server.trim();
    let s = s.split("://").nth(1).unwrap_or(s);
    let host_port = s.split('/').next().unwrap_or(s);
    host_port.split(':').next().unwrap_or(host_port).to_string()
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 命中缓存直接返回；否则补一次 `play/info`（整季自动切集时会走到这里）。
async fn resolve_meta(cfg: &Config, item_guid: &str) -> Result<PlayMeta, String> {
    if let Ok(g) = cache().lock() {
        if let Some(m) = g.get(item_guid) {
            if !m.media_guid.is_empty() {
                return Ok(m.clone());
            }
        }
    }
    let client = fnapi::new_client();
    let v = client
        .call(
            &cfg.server,
            "/v/api/v1/play/info",
            "POST",
            &cfg.token,
            Some(json!({ "item_guid": item_guid })),
        )
        .await?;
    let guid = str_of(&v, "guid");
    let meta = PlayMeta {
        item_guid: if guid.is_empty() {
            item_guid.to_string()
        } else {
            guid
        },
        media_guid: str_of(&v, "media_guid"),
        video_guid: str_of(&v, "video_guid"),
        audio_guid: str_of(&v, "audio_guid"),
        subtitle_guid: str_of(&v, "subtitle_guid"),
    };
    if meta.media_guid.is_empty() {
        return Err("play/info 缺少 media_guid".into());
    }
    remember(meta.clone());
    Ok(meta)
}

/// 把「当前播放位置 / 总时长（秒）」回传飞牛。成功 = 「继续观看」已更新。
pub async fn report(cfg: &Config, item_guid: &str, ts: f64, duration: f64) -> Result<(), String> {
    if item_guid.is_empty() || cfg.token.is_empty() {
        return Ok(());
    }
    let meta = resolve_meta(cfg, item_guid).await?;
    let body = json!({
        "item_guid": meta.item_guid,
        "media_guid": meta.media_guid,
        "video_guid": meta.video_guid,
        "audio_guid": meta.audio_guid,
        "subtitle_guid": meta.subtitle_guid,
        "play_link": host_of(&cfg.server),
        "ts": ts.max(0.0).round() as i64,
        "duration": duration.max(0.0).round() as i64,
    });
    let client = fnapi::new_client();
    client
        .call(
            &cfg.server,
            "/v/api/v1/play/record",
            "POST",
            &cfg.token,
            Some(body),
        )
        .await
        .map(|_| ())
}

// ───────────────────────── 退出兜底：后端直接落盘 ─────────────────────────
//
// 前端在 `mpv-load ended` / `mpv-end` 会立即 flush；但「用户直接关掉 App」时 WebView
// 可能来不及跑完 invoke。这里在 mpv 读线程里维护最后位置，**IPC 断开（= mpv 退出）时
// 由 Rust 直接落本地历史 + 回传飞牛**，不依赖前端。
//
// [fix] 进程被**强杀**（任务管理器 / taskkill / kill -9）时任何退出钩子都不会跑，所以再加一层
// 「本地落盘兜底」：播放中每 [`PERSIST_INTERVAL`] 把跟踪器快照写进
// `app_data_dir/progress-pending.json`，下次启动由 [`replay_pending`] 补报一次；
// 正常退出 / 切集结算后删除该文件，避免重复补报。

#[derive(Clone, Debug, Default)]
struct Tracker {
    /// 本地历史记录键（`history-recorded` 给的那条；整季切集不变）
    series_guid: String,
    /// 当前正在播的那一集 guid（mpv `path` 变化时更新）
    item_guid: String,
    /// [speed] **剧级**倍速键（`history::series_key_of` 同一套规则；空 = 不持久化倍速）。
    /// 与 `series_guid` 不同：后者是本地历史记录键（可能等于单集 guid），
    /// 这里只用于 [`crate::playback_speed`]，同剧各集共享、切集不变。
    speed_key: String,
    pos_ms: u64,
    dur_ms: u64,
    /// [speed] 当前播放速度（mpv `speed` 事件，见 [`set_speed`]）。
    /// 只写进本地进度快照，**不进**飞牛 `play/record`（该接口无速率字段，见模块头注释）。
    speed: f64,
    /// [lc-1845] **我们主动套用**的倍速（id=13 起播/切集时 set 的值）。用于识别
    /// speed 事件回声：套用动作本身会让 mpv 回发一条 speed 事件，若不过滤，
    /// 「key 查漏→套用 1.0」的回声会把 1.0 写进记录，**永久覆盖**用户设过的值。
    applied_speed: Option<f64>,
    /// 上次写 pending 文件的时间（仅内存，不参与上报语义）
    last_persist: Option<Instant>,
}

fn tracker() -> &'static Mutex<Tracker> {
    static TRACKER: OnceLock<Mutex<Tracker>> = OnceLock::new();
    TRACKER.get_or_init(|| Mutex::new(Tracker::default()))
}

/// 跟踪器里的速度：0/非法值一律按 1.0（旧逻辑/未收到事件时的默认）。
fn tracker_speed(t: &Tracker) -> f64 {
    if t.speed.is_finite() && t.speed > 0.0 {
        t.speed
    } else {
        1.0
    }
}

/// [lc-1845] 记录「我们主动套用」的倍速（套用前调用；识别回声用）。
pub fn set_applied_speed(v: f64) {
    if let Ok(mut t) = tracker().lock() {
        t.applied_speed = Some(v);
    }
}

/// [lc-1845] 该值是否为我们主动套用的值（回声判定，±0.001 容差）。
pub fn is_applied_speed(v: f64) -> bool {
    if let Ok(t) = tracker().lock() {
        if let Some(a) = t.applied_speed {
            return (a - v).abs() < 0.001;
        }
    }
    false
}

/// [speed] mpv `speed` 变化时调用（mpv.rs 观察 id=4）。
pub fn set_speed(speed: f64) {
    if !speed.is_finite() || speed <= 0.0 {
        return;
    }
    if let Ok(mut t) = tracker().lock() {
        t.speed = speed;
    }
}

/// 当前播放速度（给上报/快照用；未知时 1.0）。
pub fn current_speed() -> f64 {
    tracker()
        .lock()
        .map(|t| tracker_speed(&t))
        .unwrap_or(1.0)
}

/// [speed] 当前这次播放的**剧级**倍速键（同剧各集相同；空 = 不持久化倍速）。
pub fn current_speed_key() -> String {
    tracker()
        .lock()
        .map(|t| t.speed_key.clone())
        .unwrap_or_default()
}

/// 落盘间隔：即使进程被强杀，最多也只丢这几秒。
const PERSIST_INTERVAL: Duration = Duration::from_secs(3);
/// 启动补报的「新鲜度」门槛：超过这个时间（例如隔天才开软件）只补本地历史，
/// 不再回传飞牛 —— 避免用一份很旧的快照把服务端已有的更新进度冲回去。
const REPLAY_FRESH_MS: u64 = 30 * 60 * 1000;

/// 落盘快照（`progress-pending.json`）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct PendingProgress {
    series_guid: String,
    item_guid: String,
    pos_ms: u64,
    dur_ms: u64,
    /// [speed] 快照时的播放速度。旧快照没有该字段 → `serde(default)` 回落 1.0。
    #[serde(default = "default_speed")]
    speed: f64,
    updated_at_ms: u64,
}

fn default_speed() -> f64 {
    1.0
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn pending_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("progress-pending.json"))
}

/// 原子写快照（先写 .tmp 再 rename，避免强杀瞬间留下半截 JSON）。
fn persist(app: &AppHandle, t: &Tracker) {
    if t.series_guid.is_empty() || t.pos_ms == 0 {
        return;
    }
    let Some(path) = pending_path(app) else { return };
    let snap = PendingProgress {
        series_guid: t.series_guid.clone(),
        item_guid: t.item_guid.clone(),
        pos_ms: t.pos_ms,
        dur_ms: t.dur_ms,
        speed: tracker_speed(t),
        updated_at_ms: now_ms(),
    };
    let Ok(bytes) = serde_json::to_vec(&snap) else { return };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn clear_pending(app: &AppHandle) {
    if let Some(p) = pending_path(app) {
        let _ = std::fs::remove_file(p);
    }
}

/// 起播时调用：重置跟踪器（新一次播放）。
///
/// 无剧级倍速键（在线源 / 直播 / 本地文件）→ 用 [`begin`]，倍速不持久化。
pub fn begin(series_guid: &str, item_guid: &str) {
    begin_with_speed_key(series_guid, item_guid, "");
}

/// [speed] 起播时调用：除 [`begin`] 的字段外，额外带上**剧级倍速键**
/// （由 `lib.rs::play_item` 用 `history::series_key_of` 同一套规则算出）。
pub fn begin_with_speed_key(series_guid: &str, item_guid: &str, speed_key: &str) {
    if let Ok(mut t) = tracker().lock() {
        *t = Tracker {
            series_guid: series_guid.to_string(),
            item_guid: item_guid.to_string(),
            speed_key: speed_key.to_string(),
            pos_ms: 0,
            dur_ms: 0,
            speed: 1.0,
            applied_speed: None,
            last_persist: None,
        };
    }
}

/// mpv `time-pos` / `duration`（秒）→ 毫秒，只增不减；顺带按 [`PERSIST_INTERVAL`] 落盘。
pub fn tick(app: &AppHandle, pos_secs: f64, dur_secs: f64) {
    let snap = {
        let Ok(mut t) = tracker().lock() else { return };
        if pos_secs.is_finite() && pos_secs > 0.0 {
            t.pos_ms = t.pos_ms.max((pos_secs * 1000.0).round() as u64);
        }
        if dur_secs.is_finite() && dur_secs > 0.0 {
            t.dur_ms = t.dur_ms.max((dur_secs * 1000.0).round() as u64);
        }
        let due = t
            .last_persist
            .map(|i| i.elapsed() >= PERSIST_INTERVAL)
            .unwrap_or(true);
        if due {
            t.last_persist = Some(Instant::now());
        }
        if due {
            Some(t.clone())
        } else {
            None
        }
    };
    if let Some(s) = snap {
        persist(app, &s);
    }
}

/// 从代理直链 `/api/v1/playvideo/<guid>?...` 解析条目 guid。
fn parse_item_guid(url: &str) -> String {
    const MARK: &str = "/api/v1/playvideo/";
    let Some(idx) = url.find(MARK) else {
        return String::new();
    };
    let rest = &url[idx + MARK.len()..];
    rest.split(['?', '#', '/'])
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 异步上报某一集的最后位置（失败只记 trace，不阻塞播放）。
fn report_episode(app: &AppHandle, item_guid: &str, pos_ms: u64, dur_ms: u64) {
    if item_guid.is_empty()
        || item_guid.starts_with("webrs:")
        || pos_ms == 0
        || dur_ms == 0
    {
        return;
    }
    let app = app.clone();
    let item = item_guid.to_string();
    tauri::async_runtime::spawn(async move {
        let cfg = app.state::<crate::AppState>().config.read().await.clone();
        if cfg.token.is_empty() {
            return;
        }
        if let Err(e) = report(
            &cfg,
            &item,
            pos_ms as f64 / 1000.0,
            dur_ms as f64 / 1000.0,
        )
        .await
        {
            crate::libmpv::trace(&format!("[progress] 回传 {item} 失败: {e}"));
        }
    });
}

/// [fix] `end-file`（mpv 单集结束 / 切集 / 停止）时调用：把**当前集**最后位置结算一次，
/// 然后把位置/时长清零，等下一集开始重新计时。
///
/// 为什么放在 `end-file` 而不是等 `path` 变化：mpv 在切换前一定会先发 `end-file`，
/// 此刻跟踪器里还是**上一集**的终值；等 `path` 变成新一集时位置已经被新集的
/// `time-pos` 覆盖，结算就会串集。两者都做（幂等），`end-file` 是主路径。
pub fn settle_current(app: &AppHandle) {
    let prev = {
        let Ok(mut t) = tracker().lock() else { return };
        let prev = (t.item_guid.clone(), t.pos_ms, t.dur_ms);
        t.pos_ms = 0;
        t.dur_ms = 0;
        prev
    };
    let (guid, pos, dur) = prev;
    report_episode(app, &guid, pos, dur);
}

/// mpv `path`（代理直链 `/api/v1/playvideo/<guid>?...`）→ 当前集 guid。
///
/// 整季播放列表自动连播 / 手动上下集时，`path` 会切到下一集的代理 URL：
///   · 先把**上一集**的最后位置结算并回传（`end-file` 已结算时这里是空操作，幂等）；
///   · 再重置位置/时长计数 —— 否则 [`tick`] 的 `max()` 会把上一集的进度带进下一集。
///
/// [fix] 必须用 mpv 的 `path`（完整 URL），**不能用 `filename`**：`filename` 是 URL 的
/// 最后一段（basename），`/api/v1/playvideo/` 前缀会被剥掉 → 解析永远为空，
/// 切集后 `itemGuid` 不更新，进度就都记到第一集上（缺陷 1 的根因）。
pub fn set_item_from_url(app: &AppHandle, url: &str) {
    let guid = parse_item_guid(url);
    if guid.is_empty() {
        return;
    }
    let prev = {
        let Ok(mut t) = tracker().lock() else { return };
        if t.item_guid == guid {
            return; // 同一集（首次 file-loaded 也会走到这里，无需结算）
        }
        let prev = (t.item_guid.clone(), t.pos_ms, t.dur_ms);
        t.item_guid = guid;
        t.pos_ms = 0;
        t.dur_ms = 0;
        // [speed] 切集只重置本集计时，**不动剧级 speed_key**（同剧各集共享倍速）。
        // 速度值先归 1.0；若该剧有记录，mpv.rs 会紧接着按剧级键 set speed 并回抛事件覆盖。
        t.speed = 1.0;
        prev
    };
    let (old_guid, old_pos, old_dur) = prev;
    report_episode(app, &old_guid, old_pos, old_dur);
}

/// mpv 读线程结束时调用：后台线程里把最后位置落本地历史 + 回传飞牛。
pub fn flush_on_exit(app: AppHandle) {
    let snap = match tracker().lock() {
        Ok(t) => t.clone(),
        Err(_) => return,
    };
    if snap.series_guid.is_empty() || snap.pos_ms == 0 {
        clear_pending(&app);
        return;
    }
    let item = snap.item_guid.clone();
    let pos = snap.pos_ms;
    let dur = snap.dur_ms;
    let series = snap.series_guid.clone();
    tauri::async_runtime::spawn(async move {
        crate::history::update_progress(&app, &series, pos, dur);
        if !item.is_empty() && !item.starts_with("webrs:") && dur > 0 {
            let cfg = app.state::<crate::AppState>().config.read().await.clone();
            if !cfg.token.is_empty() {
                if let Err(e) = report(&cfg, &item, pos as f64 / 1000.0, dur as f64 / 1000.0).await
                {
                    crate::libmpv::trace(&format!("[progress] 退出兜底回传失败: {e}"));
                }
            }
        }
        clear_pending(&app);
    });
}

/// [fix] **应用退出路径专用**：同步（有超时上限）完成最后一次落本地 + 回传。
///
/// 为什么需要它：`RunEvent::Exit` 之后 Tauri 的异步运行时会很快被丢弃，`flush_on_exit`
/// 里 `spawn` 出去的任务可能根本跑不到 —— 这就是「直接关掉软件，最后一次进度丢失」的根因。
/// 这里用 `block_on` 在退出前把它做完，并给网络请求 3s 上限，绝不无限阻塞退出。
pub async fn flush_on_exit_sync(app: &AppHandle) {
    let snap = match tracker().lock() {
        Ok(t) => t.clone(),
        Err(_) => return,
    };
    if snap.series_guid.is_empty() || snap.pos_ms == 0 {
        clear_pending(app);
        return;
    }
    crate::history::update_progress(app, &snap.series_guid, snap.pos_ms, snap.dur_ms);
    let can_remote = !snap.item_guid.is_empty()
        && !snap.item_guid.starts_with("webrs:")
        && snap.dur_ms > 0;
    if !can_remote {
        clear_pending(app);
        return;
    }
    let cfg = app.state::<crate::AppState>().config.read().await.clone();
    if cfg.token.is_empty() {
        clear_pending(app);
        return;
    }
    let fut = report(
        &cfg,
        &snap.item_guid,
        snap.pos_ms as f64 / 1000.0,
        snap.dur_ms as f64 / 1000.0,
    );
    match tokio::time::timeout(Duration::from_secs(3), fut).await {
        Ok(Ok(())) => clear_pending(app),
        Ok(Err(e)) => {
            // 保留 pending 文件：下次启动还能补报
            crate::libmpv::trace(&format!("[progress] 退出同步回传失败（已落盘待补）: {e}"));
        }
        Err(_) => {
            crate::libmpv::trace("[progress] 退出同步回传超时（已落盘待补）");
        }
    }
}

/// [fix] 启动时补报上一次进程被强杀前落盘的最后进度。
///
/// · 本地历史：无条件按 max 合并（`update_progress` 只增不减，安全）；
/// · 飞牛：仅当快照「足够新」（[`REPLAY_FRESH_MS`] 内）才回传，避免把服务端较新的进度冲回去；
/// · 处理完删除文件；回传失败（网络问题）时保留，下次启动再试。
pub fn replay_pending(app: &AppHandle) {
    let Some(path) = pending_path(app) else { return };
    let Ok(bytes) = std::fs::read(&path) else { return };
    let Ok(snap) = serde_json::from_slice::<PendingProgress>(&bytes) else {
        let _ = std::fs::remove_file(&path);
        return;
    };
    if snap.pos_ms == 0 {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let fresh = now_ms().saturating_sub(snap.updated_at_ms) < REPLAY_FRESH_MS;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if !snap.series_guid.is_empty() {
            crate::history::update_progress(&app, &snap.series_guid, snap.pos_ms, snap.dur_ms);
        }
        let can_remote = fresh
            && !snap.item_guid.is_empty()
            && !snap.item_guid.starts_with("webrs:")
            && snap.dur_ms > 0;
        if can_remote {
            let cfg = app.state::<crate::AppState>().config.read().await.clone();
            if !cfg.token.is_empty() {
                if let Err(e) = report(
                    &cfg,
                    &snap.item_guid,
                    snap.pos_ms as f64 / 1000.0,
                    snap.dur_ms as f64 / 1000.0,
                )
                .await
                {
                    crate::libmpv::trace(&format!("[progress] 启动补报失败（保留待下次）: {e}"));
                    return; // 保留文件
                }
            }
        }
        clear_pending(&app);
    });
}


#[cfg(test)]
mod tests {
    use super::{host_of, parse_item_guid, PendingProgress};

    #[test]
    fn host_of_strips_scheme_path_and_port() {
        assert_eq!(host_of("http://192.168.1.2:5666"), "192.168.1.2");
        assert_eq!(host_of("https://nas.example.com/"), "nas.example.com");
        assert_eq!(host_of("nas.local"), "nas.local");
    }

    #[test]
    fn parse_item_guid_from_proxy_url() {
        assert_eq!(
            parse_item_guid("http://127.0.0.1:22346/api/v1/playvideo/abc123?token=t&skipVerify=1"),
            "abc123"
        );
        // 非代理直链（m3u 本地文件等）不产生 guid
        assert_eq!(parse_item_guid(r"C:\Temp\anibox-playlist.m3u"), "");
        // 带 fragment / 末尾斜杠
        assert_eq!(
            parse_item_guid("http://h/api/v1/playvideo/xyz#frag"),
            "xyz"
        );
    }

    /// [fix] 哨兵：mpv 的 `filename` 是 URL 最后一段（basename），解析不出 guid；
    /// 必须用完整 URL（`path`）。这条用例锁定「只喂 basename 时返回空」的语义，
    /// 防止有人把观察属性改回 `filename`。
    #[test]
    fn basename_only_does_not_yield_guid() {
        // mpv filename 的实际形态：最后一段 + 查询串
        assert_eq!(parse_item_guid("abc123?token=t&skipVerify=1"), "");
        assert_eq!(parse_item_guid("abc123"), "");
    }

    #[test]
    fn pending_progress_roundtrip() {
        let p = PendingProgress {
            series_guid: "series".into(),
            item_guid: "ep1".into(),
            pos_ms: 12345,
            dur_ms: 1440000,
            speed: 1.25,
            updated_at_ms: 1_700_000_000_000,
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: PendingProgress = serde_json::from_str(&s).unwrap();
        assert_eq!(back.item_guid, "ep1");
        assert_eq!(back.pos_ms, 12345);
        assert_eq!(back.dur_ms, 1440000);
        assert_eq!(back.speed, 1.25);
    }

    /// [speed] 旧版本快照没有 `speed` 字段：反序列化必须成功并回落 1.0（不能因缺字段丢快照）。
    #[test]
    fn pending_progress_legacy_without_speed() {
        let legacy = r#"{"series_guid":"s","item_guid":"ep1","pos_ms":1000,"dur_ms":2000,"updated_at_ms":1}"#;
        let back: PendingProgress = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.item_guid, "ep1");
        assert_eq!(back.speed, 1.0);
    }
}