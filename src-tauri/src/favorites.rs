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
    // [lc-1806b] 方向判定必须用「真实收藏态」：服务端列表标记（item.is_favorite=1）或本地表命中
    // 任意一个即视为已收藏。上一版只查本地表 —— 服务端已收藏、本地无记录的条目第一次点击
    // 会被判成「未收藏」走去加收藏（PUT），服务端无变化、返回 favorited=true 心形保持亮，
    // 表现为「要点两次才取消」；且服务端从未取消，返回首页刷新又回来。
    let was_favorited = map.contains_key(&guid)
        || item.get("is_favorite").and_then(|v| v.as_i64()) == Some(1);
    // [lc-1806] 服务端优先（飞鲸口径，真 NAS 实测）：
    //   加 = PUT  /v/api/v1/item/favorite  body {"item_guid": guid}（snake 才认，camel 回 Not Found）
    //   取消 = DELETE 同路由 body {"itemGuid": guid}（camel 实测 true）
    // 当年「NAS 未实现」的结论是 POST 打错方法；服务端状态从没被取消过，导致
    // 「取消收藏 → 刷新又回来」（列表 is_favorite 来自服务端）。失败回落纯本地（离线兜底）。
    let server_ok = {
        let state = app.state::<crate::AppState>();
        let cfg = state.config.read().await.clone();
        if cfg.token.is_empty() {
            false
        } else {
            let client = crate::fnapi::new_client();
            let body = if was_favorited {
                json!({ "itemGuid": guid })
            } else {
                json!({ "item_guid": guid })
            };
            let method = if was_favorited { "DELETE" } else { "PUT" };
            match client
                .call(&cfg.server, "/v/api/v1/item/favorite", method, &cfg.token, Some(body))
                .await
            {
                Ok(_) => true,
                Err(e) => {
                    crate::libmpv::trace(&format!("[favorites] 服务端收藏切换失败，回落本地: {e}"));
                    false
                }
            }
        }
    };
    let _ = server_ok;
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
