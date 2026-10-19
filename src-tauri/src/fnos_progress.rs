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
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
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

#[derive(Clone, Debug, Default)]
struct Tracker {
    /// 本地历史记录键（`history-recorded` 给的那条；整季切集不变）
    series_guid: String,
    /// 当前正在播的那一集 guid（mpv `filename` 变化时更新）
    item_guid: String,
    pos_ms: u64,
    dur_ms: u64,
}

fn tracker() -> &'static Mutex<Tracker> {
    static TRACKER: OnceLock<Mutex<Tracker>> = OnceLock::new();
    TRACKER.get_or_init(|| Mutex::new(Tracker::default()))
}

/// 起播时调用：重置跟踪器（新一次播放）。
pub fn begin(series_guid: &str, item_guid: &str) {
    if let Ok(mut t) = tracker().lock() {
        *t = Tracker {
            series_guid: series_guid.to_string(),
            item_guid: item_guid.to_string(),
            pos_ms: 0,
            dur_ms: 0,
        };
    }
}

/// mpv `time-pos` / `duration`（秒）→ 毫秒，只增不减。
pub fn tick(pos_secs: f64, dur_secs: f64) {
    if let Ok(mut t) = tracker().lock() {
        if pos_secs.is_finite() && pos_secs > 0.0 {
            t.pos_ms = t.pos_ms.max((pos_secs * 1000.0).round() as u64);
        }
        if dur_secs.is_finite() && dur_secs > 0.0 {
            t.dur_ms = t.dur_ms.max((dur_secs * 1000.0).round() as u64);
        }
    }
}

/// 从 mpv `filename`（代理直链 `/api/v1/playvideo/<guid>?...`）解析当前集 guid。
pub fn set_item_from_url(url: &str) {
    const MARK: &str = "/api/v1/playvideo/";
    let Some(idx) = url.find(MARK) else { return };
    let rest = &url[idx + MARK.len()..];
    let guid = rest.split(['?', '#', '/']).next().unwrap_or("").trim();
    if guid.is_empty() {
        return;
    }
    if let Ok(mut t) = tracker().lock() {
        t.item_guid = guid.to_string();
    }
}

/// mpv 读线程结束时调用：后台线程里把最后位置落本地历史 + 回传飞牛。
pub fn flush_on_exit(app: AppHandle) {
    let snap = match tracker().lock() {
        Ok(t) => t.clone(),
        Err(_) => return,
    };
    if snap.series_guid.is_empty() || snap.pos_ms == 0 {
        return;
    }
    tauri::async_runtime::spawn(async move {
        crate::history::update_progress(&app, &snap.series_guid, snap.pos_ms, snap.dur_ms);
        let cfg = app.state::<crate::AppState>().config.read().await.clone();
        if !snap.item_guid.is_empty()
            && !snap.item_guid.starts_with("webrs:")
            && !cfg.token.is_empty()
            && snap.dur_ms > 0
        {
            if let Err(e) = report(
                &cfg,
                &snap.item_guid,
                snap.pos_ms as f64 / 1000.0,
                snap.dur_ms as f64 / 1000.0,
            )
            .await
            {
                crate::libmpv::trace(&format!("[progress] 退出兜底回传失败: {e}"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::host_of;

    #[test]
    fn host_of_strips_scheme_path_and_port() {
        assert_eq!(host_of("http://192.168.1.2:5666"), "192.168.1.2");
        assert_eq!(host_of("https://nas.example.com/"), "nas.example.com");
        assert_eq!(host_of("nas.local"), "nas.local");
    }
}