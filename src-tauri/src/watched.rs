//! [lc-1654] 海报卡「已看」切换（对齐飞牛网页端实测请求）。
//!
//! 飞牛网页端（`/v/assets/14a77…js` 路由表 + 实测点击）：
//!   · 标记已看   POST   /v/api/v1/item/watched   body `{ item_guid }`
//!   · 取消已看   DELETE /v/api/v1/item/watched   body `{ item_guid }`
//! 字段名照实测，不套自造枚举。
//!
//! 降级：这台 NAS 上部分写接口会回 `{"message":"Not Implemented, please update."}`
//! （收藏接口已确认如此）。`item/watched` 若同样未实现/请求失败，就退化为**本地已看标记**：
//! app_data_dir/watched.json，`{ "<guid>": true }`。远程成功时会把本地同名标记清掉，
//! 让「以飞牛为准」；只有降级时本地才留痕。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

use crate::{fnapi, AppState};

fn store_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("取应用数据目录失败: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("watched.json"))
}

fn load(app: &AppHandle) -> HashMap<String, bool> {
    let Ok(p) = store_path(app) else {
        return HashMap::new();
    };
    let Ok(text) = std::fs::read_to_string(&p) else {
        return HashMap::new();
    };
    serde_json::from_str::<HashMap<String, bool>>(&text).unwrap_or_default()
}

fn save(app: &AppHandle, map: &HashMap<String, bool>) -> Result<(), String> {
    let p = store_path(app)?;
    let text = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| format!("写已看标记失败: {e}"))
}

/// 写/删本地降级标记（watched=true 写入，false 删除）
fn set_local(app: &AppHandle, guid: &str, watched: bool) -> Result<(), String> {
    let mut map = load(app);
    if watched {
        map.insert(guid.to_string(), true);
    } else {
        map.remove(guid);
    }
    save(app, &map)
}

/// 本地降级标记集合（前端启动时一次性拉取，用于覆盖卡片初始态）
#[tauri::command]
pub async fn watched_get_all(app: AppHandle) -> Result<Vec<String>, String> {
    Ok(load(&app)
        .into_iter()
        .filter_map(|(guid, on)| if on { Some(guid) } else { None })
        .collect())
}

/// 切换条目已看状态。返回 `{ watched, local, error? }`：
/// `local=true` 表示飞牛接口不可用、已降级为本地标记（`error` 带上原始原因）。
#[tauri::command]
pub async fn set_item_watched(
    state: State<'_, AppState>,
    app: AppHandle,
    guid: String,
    watched: bool,
) -> Result<Value, String> {
    let guid = guid.trim().to_string();
    if guid.is_empty() {
        return Err("缺少 guid".into());
    }
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let method = if watched { "POST" } else { "DELETE" };
    let res = client
        .call(
            &cfg.server,
            "/v/api/v1/item/watched",
            method,
            &cfg.token,
            Some(json!({ "item_guid": guid })),
        )
        .await;

    match res {
        Ok(_) => {
            // 远程已生效：清掉可能遗留的本地降级标记，状态以飞牛为准
            let _ = set_local(&app, &guid, false);
            Ok(json!({ "watched": watched, "local": false }))
        }
        Err(e) => {
            set_local(&app, &guid, watched)?;
            Ok(json!({ "watched": watched, "local": true, "error": e }))
        }
    }
}
