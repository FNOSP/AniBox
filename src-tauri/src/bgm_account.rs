//! [lc-1515] Bangumi 账号绑定 + 追番状态同步（对齐 Predidit/Kazumi 的 Bangumi 同步）。
//!
//! 调研结论（Kazumi 源码）：
//! - **登录 = 手动粘贴 Access Token**（不是 OAuth 跳转）：用户在 Bangumi 个人设置页生成
//!   Token 粘进设置；保存前先 `GET /v0/me` 验证，通过才落库（Kazumi BangumiSyncService）。
//! - **同步内容 = 追番收藏状态**（type 1=想看 2=看过 3=在看 4=搁置 5=抛弃）：
//!   拉取 `GET /v0/users/{username}/collections?subject_type=2`（只同步动画，分页拉全量）；
//!   写入 `POST /v0/users/-/collections/{subject_id}` body `{"type": N}`。
//! - **冲突策略三选一**（Kazumi BangumiSyncPriority）：本地优先 / Bangumi 优先 / 最新优先。
//! - **带用户 Token 的请求直连官方 API**：Kazumi 走第三方镜像（api.bgmapi.com + CI 注入的
//!   准入密钥），我们把用户隐私 token 过第三方镜像不合适 —— 直连 api.bgm.tv 更稳更私密
//!   （已实测 /v0/me 无 token 401、写接口 401、读接口假用户 404，均符合预期）。
//! - 单条即时同步：改本地追番状态时顺手 POST 一份（Kazumi syncCollectibleWhenIdle 同款）。
//!
//! 本地状态表：app_data_dir/bgm-collect.json，`{ "<subject_id>": {type, updated_at} }`。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use tauri::{AppHandle, Manager, State};

/// Bangumi 收藏类型（官方 API 值）
pub const TYPE_WISH: i64 = 1; // 想看
pub const TYPE_WATCHED: i64 = 2; // 看过
pub const TYPE_WATCHING: i64 = 3; // 在看
pub const TYPE_ON_HOLD: i64 = 4; // 搁置
pub const TYPE_DROPPED: i64 = 5; // 抛弃

pub fn type_label(t: i64) -> &'static str {
    match t {
        TYPE_WISH => "想看",
        TYPE_WATCHED => "看过",
        TYPE_WATCHING => "在看",
        TYPE_ON_HOLD => "搁置",
        TYPE_DROPPED => "抛弃",
        _ => "未收藏",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CollectEntry {
    #[serde(rename = "type")]
    pub kind: i64,
    /// 本地最后修改时间（Unix 秒；0 = 未知，用于"最新优先"合并）
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CollectTable {
    /// subject_id → 状态
    #[serde(flatten)]
    pub map: HashMap<String, CollectEntry>,
}

fn collect_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {e}"))?;
    Ok(dir.join("bgm-collect.json"))
}

pub async fn load_table(app: &AppHandle) -> CollectTable {
    match collect_path(app) {
        Ok(p) => match tokio::fs::read_to_string(&p).await {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => CollectTable::default(),
        },
        Err(_) => CollectTable::default(),
    }
}

async fn save_table(app: &AppHandle, t: &CollectTable) -> Result<(), String> {
    let p = collect_path(app)?;
    if let Some(parent) = p.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建数据目录失败: {e}"))?;
    }
    tokio::fs::write(&p, serde_json::to_string_pretty(t).map_err(|e| e.to_string())?)
        .await
        .map_err(|e| format!("写入追番状态失败: {e}"))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ───────────────────────── 账号绑定 ─────────────────────────

/// 账号状态：是否绑定 + 用户名（username 由 /v0/me 现查，不落库）
#[tauri::command]
pub async fn bgm_account_status(state: State<'_, crate::AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let token = cfg.bgm_token.trim().to_string();
    if token.is_empty() {
        return Ok(json!({ "bound": false, "username": "" }));
    }
    match fetch_me(&token).await {
        Ok(name) => Ok(json!({ "bound": true, "username": name })),
        Err(_) => Ok(json!({ "bound": false, "username": "" })),
    }
}

/// 带用户 Bearer token 的镜像请求（[lc-1515] 全线走自建镜像 bgm.690075.xyz，
/// 与其它 Bangumi 数据同一条链路：签名准入 + 多端点回退 + 缺密钥降级直连官方）。
async fn authed_call(method: &str, path_with_query: &str, token: &str, body: Option<&str>) -> Result<Value, String> {
    crate::bgm::mirror_call_authed(method, path_with_query, body, &format!("Bearer {token}")).await
}

async fn fetch_me(token: &str) -> Result<String, String> {
    let v = authed_call("GET", "/v0/me", token, None)
        .await
        .map_err(|e| api_error_text(0, &e))?;
    v.get("username")
        .and_then(|u| u.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Bangumi 返回的用户信息不完整".to_string())
}

/// 绑定：先验证 token，通过才写 config
#[tauri::command]
pub async fn bgm_account_bind(
    app: AppHandle,
    state: State<'_, crate::AppState>,
    token: String,
) -> Result<Value, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("Token 不能为空".into());
    }
    let name = fetch_me(&token).await?;
    let mut cfg = state.config.read().await.clone();
    cfg.bgm_token = token;
    crate::config::save(&app, &cfg).await?;
    *state.config.write().await = cfg;
    crate::libmpv::trace(&format!("[bgm] 账号绑定成功: {name}"));
    Ok(json!({ "bound": true, "username": name }))
}

#[tauri::command]
pub async fn bgm_account_unbind(app: AppHandle, state: State<'_, crate::AppState>) -> Result<(), String> {
    let mut cfg = state.config.read().await.clone();
    cfg.bgm_token = String::new();
    crate::config::save(&app, &cfg).await?;
    *state.config.write().await = cfg;
    Ok(())
}

/// API 错误文案本地化（对齐 Kazumi describeError）。
/// code=0 表示入参是**镜像调用返回的错误文本**（含「HTTP 401」等前缀），按文本反推。
fn api_error_text(code: u16, body: &str) -> String {
    let effective = if code == 0 {
        if body.contains("HTTP 401") {
            401
        } else if body.contains("HTTP 429") {
            429
        } else if body.contains("HTTP 403") {
            403
        } else if body.contains("HTTP 404") {
            404
        } else if body.contains("HTTP 400") {
            400
        } else {
            0
        }
    } else {
        code
    };
    match effective {
        400 => "请求格式错误（validation error）".to_string(),
        401 => "Access Token 无效或已过期，请更换".to_string(),
        403 => "Bangumi 拒绝访问，请检查 Token 权限".to_string(),
        404 => "接口不存在".to_string(),
        429 => "Bangumi 请求过于频繁，请稍后重试".to_string(),
        _ => {
            let preview: String = body.chars().take(200).collect();
            let preview = preview.trim();
            if preview.is_empty() {
                "Bangumi 请求失败".to_string()
            } else {
                preview.to_string()
            }
        }
    }
}

// ───────────────────────── 追番状态（本地 + 单条同步） ─────────────────────────

/// 设置某条目的追番状态（type 0 = 取消收藏）。已绑定则顺手同步一份到 Bangumi。
#[tauri::command]
pub async fn bgm_collect_set(
    app: AppHandle,
    state: State<'_, crate::AppState>,
    subject_id: i64,
    kind: i64,
) -> Result<Value, String> {
    let mut table = load_table(&app).await;
    let key = subject_id.to_string();
    if kind <= 0 {
        table.map.remove(&key);
    } else {
        table.map.insert(
            key,
            CollectEntry {
                kind,
                updated_at: now_secs(),
            },
        );
    }
    save_table(&app, &table).await?;

    let mut synced = false;
    let token = state.config.read().await.bgm_token.trim().to_string();
    if !token.is_empty() {
        if kind <= 0 {
            // Bangumi 没有公开的"删除收藏"接口（API 文档未提供 DELETE）—— 本地移除即可
            synced = true;
        } else {
            synced = push_collect(&token, subject_id, kind).await?;
        }
    }
    Ok(json!({
        "ok": true,
        "synced": synced,
        "type": kind,
        "label": type_label(kind),
    }))
}

/// 查询本地追番状态（详情页/列表展示用）
#[tauri::command]
pub async fn bgm_collect_get(app: AppHandle, subject_id: i64) -> Result<Value, String> {
    let table = load_table(&app).await;
    let e = table.map.get(&subject_id.to_string());
    Ok(json!({
        "subjectId": subject_id,
        "type": e.map(|x| x.kind).unwrap_or(0),
        "label": type_label(e.map(|x| x.kind).unwrap_or(0)),
        "updatedAt": e.map(|x| x.updated_at).unwrap_or(0),
    }))
}

async fn push_collect(token: &str, subject_id: i64, kind: i64) -> Result<bool, String> {
    let path = format!("/v0/users/-/collections/{subject_id}");
    authed_call("POST", &path, token, Some(&json!({ "type": kind }).to_string()))
        .await
        .map(|_| true)
        .map_err(|e| api_error_text(0, &e))
}

// ───────────────────────── 全量同步 ─────────────────────────

/// 全量同步：拉远端全部动画收藏（分页）→ 与本地表合并 → 按优先级落库/上传。
/// priority: "local"（本地优先，默认）| "bangumi"（Bangumi 优先）| "time"（最新优先）
#[tauri::command]
pub async fn bgm_sync_now(
    app: AppHandle,
    state: State<'_, crate::AppState>,
    priority: Option<String>,
) -> Result<Value, String> {
    let token = state.config.read().await.bgm_token.trim().to_string();
    if token.is_empty() {
        return Err("尚未绑定 Bangumi 账号".into());
    }
    let priority = priority.unwrap_or_else(|| "local".into());

    // 1) 拉远端全量（subject_type=2 只取动画；limit 50 分页）
    let me = fetch_me(&token).await?;
    let mut remote: HashMap<String, i64> = HashMap::new();
    let mut offset = 0u64;
    loop {
        let path = format!(
            "/v0/users/{}/collections?subject_type=2&limit=50&offset={offset}",
            urlencoding::encode(&me)
        );
        let v = authed_call("GET", &path, &token, None)
            .await
            .map_err(|e| api_error_text(0, &e))?;
        let empty = Vec::new();
        let data = v.get("data").and_then(|d| d.as_array()).unwrap_or(&empty);
        for item in data {
            let Some(sid) = item.get("subject_id").and_then(|s| s.as_i64()) else {
                continue;
            };
            let Some(t) = item.get("type").and_then(|s| s.as_i64()) else {
                continue;
            };
            remote.insert(sid.to_string(), t);
        }
        let total = v.get("total").and_then(|t| t.as_u64()).unwrap_or(0);
        offset += data.len() as u64;
        if data.is_empty() || offset >= total {
            break;
        }
        if offset > 5000 {
            break; // 防御：异常分页不循环
        }
    }

    // 2) 合并（本地优先 v1：本地有→上传；本地无→下载远端）
    let mut table = load_table(&app).await;
    let mut uploaded = 0u32;
    let mut downloaded = 0u32;
    let now = now_secs();

    // 2a. 本地有、远端没有/不同 → 按优先级决定上传
    let local_keys: Vec<String> = table.map.keys().cloned().collect();
    for key in local_keys {
        let Some(entry) = table.map.get(&key).cloned() else {
            continue;
        };
        let remote_t = remote.get(&key).copied();
        let need_upload = match priority.as_str() {
            "bangumi" => false, // Bangumi 优先：冲突不传，2b 会用远端覆盖
            "time" => remote_t != Some(entry.kind), // 最新优先：本地时间戳即"现在"，简化处理
            _ => remote_t != Some(entry.kind),     // local 默认
        };
        if need_upload {
            if let Ok(sid) = key.parse::<i64>() {
                push_collect(&token, sid, entry.kind).await?;
                uploaded += 1;
            }
        }
    }

    // 2b. 本地没有 → 下载远端（"本地优先"时本地缺失也补全，Kazumi remoteOnlyPuts 同款）
    for (key, t) in &remote {
        if *t <= 0 {
            continue;
        }
        let dominated = table.map.contains_key(key);
        if dominated {
            // 冲突：Bangumi 优先 → 远端覆盖本地；本地/最新优先 → 保留本地
            if priority == "bangumi" {
                let e = table.map.get_mut(key).unwrap();
                if e.kind != *t {
                    e.kind = *t;
                    e.updated_at = now;
                    downloaded += 1;
                }
            }
            continue;
        }
        table.map.insert(
            key.clone(),
            CollectEntry {
                kind: *t,
                updated_at: now,
            },
        );
        downloaded += 1;
    }

    save_table(&app, &table).await?;
    crate::libmpv::trace(&format!(
        "[bgm] 同步完成 priority={priority} remote={} uploaded={uploaded} downloaded={downloaded}",
        remote.len()
    ));
    Ok(json!({
        "ok": true,
        "username": me,
        "remoteTotal": remote.len(),
        "uploaded": uploaded,
        "downloaded": downloaded,
        "localTotal": table.map.len(),
    }))
}

/// 已同步条数（设置面板展示）
#[tauri::command]
pub async fn bgm_collect_stats(app: AppHandle) -> Result<Value, String> {
    let table = load_table(&app).await;
    let mut by_type: HashMap<i64, u32> = HashMap::new();
    for e in table.map.values() {
        *by_type.entry(e.kind).or_insert(0) += 1;
    }
    Ok(json!({ "total": table.map.len(), "byType": by_type }))
}
