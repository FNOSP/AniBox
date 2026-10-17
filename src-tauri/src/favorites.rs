//! [lc-1651] 本地收藏（侧边栏「收藏」的写入端）。
//!
//! 为什么不走飞牛 `POST /v/api/v1/item/favorite`：实测这台 NAS 直接回
//! `{"message":"Not Implemented, please update."}`（服务端版本没实现该路由），
//! `favorite/list` 也恒为空。所以收藏状态由客户端自己落盘，`get_favorites` 再把
//! 「本地收藏 ∪ 飞牛收藏」合并返回 —— 飞牛哪天支持了也自动带上。
//!
//! 存储：app_data_dir/favorites.json，`{ "<guid>": { guid,title,tvTitle,poster,type,
//! seasonNumber,episodeNumber,addedAt } }`。存的是卡片渲染需要的字段，列表页零额外请求。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

fn store_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("取应用数据目录失败: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("favorites.json"))
}

fn load(app: &AppHandle) -> HashMap<String, Value> {
    let Ok(p) = store_path(app) else {
        return HashMap::new();
    };
    let Ok(text) = std::fs::read_to_string(&p) else {
        return HashMap::new();
    };
    serde_json::from_str::<HashMap<String, Value>>(&text).unwrap_or_default()
}

fn save(app: &AppHandle, map: &HashMap<String, Value>) -> Result<(), String> {
    let p = store_path(app)?;
    let text = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| format!("写收藏失败: {e}"))
}

/// 收藏列表（按加入时间倒序），供 `get_favorites` 合并
pub fn list(app: &AppHandle) -> Vec<Value> {
    let mut items: Vec<Value> = load(app).into_values().collect();
    items.sort_by_key(|v| {
        std::cmp::Reverse(v.get("addedAt").and_then(|x| x.as_i64()).unwrap_or(0))
    });
    items
}

/// 是否已收藏
#[tauri::command]
pub async fn favorite_get(app: AppHandle, guid: String) -> Result<bool, String> {
    Ok(load(&app).contains_key(&guid))
}

/// 切换收藏：已收藏则取消，否则加入。返回 `{ favorited: bool }`。
#[tauri::command]
pub async fn favorite_toggle(app: AppHandle, item: Value) -> Result<Value, String> {
    let guid = item
        .get("guid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if guid.is_empty() {
        return Err("缺少 guid".into());
    }
    let mut map = load(&app);
    let favorited = if map.remove(&guid).is_some() {
        false
    } else {
        let it = item.clone();
        // 只留卡片需要的字段，避免把整份详情塞进收藏文件
        let keep = |k: &str| it.get(k).cloned();
        let mut slim = json!({
            "guid": guid,
            "title": keep("title").unwrap_or(json!("")),
            "tvTitle": keep("tvTitle").unwrap_or(json!("")),
            "poster": keep("poster").unwrap_or(json!("")),
            "type": keep("type").unwrap_or(json!("")),
            "seasonNumber": keep("seasonNumber").unwrap_or(json!(0)),
            "episodeNumber": keep("episodeNumber").unwrap_or(json!(0)),
        });
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        slim["addedAt"] = json!(now);
        map.insert(guid, slim);
        true
    };
    save(&app, &map)?;
    Ok(json!({ "favorited": favorited, "total": map.len() }))
}
