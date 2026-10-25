//! [lc-1803] 字幕选择记忆：应用内「下载字幕」挂载过的文件，同一条目下次起播自动重挂。
//!
//! 用户反馈「字幕选完没有记忆，退出再点开没字幕」：应用内下载的字幕挂载是一次性
//! `sub-add`，文件其实还留在 `%TEMP%/anibox-subtitles`，但没人再挂它。本模块把
//! 「当前播放地址 → 已挂字幕文件」落进 `app_data_dir/subtitle-memory.json`（LRU，
//! 上限 200 条），mpv `path` 事件时延迟挂载（晚于 NAS 外挂自动挂载，用户手选的最后
//! 生效成为主字幕轨）。字幕文件被清（TEMP 清理）时条目自动失效删除。

use serde_json::Value;
use tauri::AppHandle;

const MAX_ENTRIES: usize = 200;
/// 挂载延迟：NAS 外挂自动挂载（nas_subtitle 有 1.5s 起播宽限）之后，
/// 用户手选记忆最后挂、`select` 顶为主字幕轨。
const MOUNT_DELAY_SECS: u64 = 5;

pub type IpcWriter = crate::nas_subtitle::IpcWriter;

fn file_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("subtitle-memory.json"))
}

#[derive(Clone)]
struct Entry {
    sub: String,
    ts: u64,
}

fn load(app: &AppHandle) -> Vec<(String, Entry)> {
    let Some(p) = file_path(app) else { return Vec::new() };
    let Ok(s) = std::fs::read_to_string(&p) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&s) else { return Vec::new() };
    let mut out = Vec::new();
    if let Some(arr) = v.get("entries").and_then(|x| x.as_array()) {
        for e in arr {
            let (Some(k), Some(sub)) = (
                e.get("path").and_then(|x| x.as_str()),
                e.get("sub").and_then(|x| x.as_str()),
            ) else {
                continue;
            };
            if k.is_empty() || sub.is_empty() {
                continue;
            }
            out.push((
                k.to_string(),
                Entry {
                    sub: sub.to_string(),
                    ts: e.get("ts").and_then(|x| x.as_u64()).unwrap_or(0),
                },
            ));
        }
    }
    out
}

fn save(app: &AppHandle, mut list: Vec<(String, Entry)>) {
    let Some(p) = file_path(app) else { return };
    // LRU：按 ts 新→旧，超上限截断
    list.sort_by(|a, b| b.1.ts.cmp(&a.1.ts));
    list.truncate(MAX_ENTRIES);
    let entries: Vec<Value> = list
        .into_iter()
        .map(|(k, e)| serde_json::json!({ "path": k, "sub": e.sub, "ts": e.ts }))
        .collect();
    let body = serde_json::to_string_pretty(&serde_json::json!({ "entries": entries }))
        .unwrap_or_default();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, body);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 记录一条：`media_path`（mpv path 属性原值）→ 字幕文件绝对路径。
pub fn remember(app: &AppHandle, media_path: &str, sub_path: &str) {
    if media_path.trim().is_empty() || sub_path.trim().is_empty() {
        return;
    }
    let mut list = load(app);
    list.retain(|(k, _)| k != media_path);
    list.push((
        media_path.to_string(),
        Entry {
            sub: sub_path.to_string(),
            ts: now_ms(),
        },
    ));
    save(app, list);
}

/// path 事件挂载钩子：命中记忆且文件仍在 → 延迟 `sub-add select`（晚于 NAS 自动挂载）。
pub fn spawn_mount(app: AppHandle, media_path: String, writer: IpcWriter) {
    let hit = {
        let list = load(&app);
        list.into_iter().find(|(k, e)| {
            *k == media_path
                && std::path::Path::new(&e.sub).is_file()
        })
    };
    let Some((_, entry)) = hit else { return };
    let sub = entry.sub.clone();
    let title = std::path::Path::new(&sub)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "字幕".into());
    let app2 = app.clone();
    let media_key = media_path.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(MOUNT_DELAY_SECS)).await;
        // 文件又被清了 → 顺手失效条目
        if !std::path::Path::new(&sub).is_file() {
            let mut list = load(&app2);
            let before = list.len();
            list.retain(|(k, _)| *k != media_key);
            if list.len() != before {
                save(&app2, list);
            }
            return;
        }
        let line = serde_json::json!({
            "command": ["sub-add", sub, "select", title],
        })
        .to_string();
        writer(&format!("{line}\n"));
        crate::libmpv::trace(&format!("[subtitle-memory] 已按记忆挂载字幕: {title}"));
    });
}
