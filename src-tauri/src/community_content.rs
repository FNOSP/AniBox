//! [lc-2445] 社区 v2 内容：发帖 / 信息流 / 作品聚合 / 评论 / 通知。
//! 协议：docs/community-v2-design.md §4.5；服务端 inbox.690075.xyz（源码不入库）。
//!
//! 语义对齐（全部有文档依据）：
//!   · 文字永久：feed 不过滤时间；「我的发言」含已删除标记（deleted=true）
//!   · 图片 30 天：images 元数据行永久、r2 对象由 R2 生命周期删 → images[].fullKey
//!     为 null 时前端显示「配图已过期」
//!   · 读接口不拦封禁（readOk）：被封账号能浏览、能进「我的」，写操作服务端 403
//!   · 幂等：clientRequestId（UUID）作 posts.id / comments.id，重发不产生两条
//!   · 通知只在回复别人评论时产生（自回复/回复 v1 老行不通知）

use serde_json::{json, Value};
use std::time::Duration;

use crate::community::{base_url, http};
use crate::community_auth::{authed, net_err, parse_v2};

const READ_TIMEOUT_MS: u64 = 10_000;
const WRITE_TIMEOUT_MS: u64 = 15_000;

/// 统一 GET（带鉴权）。
async fn get_authed(app: &tauri::AppHandle, path: &str) -> Result<Value, String> {
    let rb = authed(
        app,
        http()
            .get(format!("{}{}", base_url(), path))
            .timeout(Duration::from_millis(READ_TIMEOUT_MS)),
    )?;
    let resp = rb.send().await.map_err(|e| net_err(&e))?;
    parse_v2(resp).await
}

/// 统一 POST（带鉴权）。
async fn post_authed(app: &tauri::AppHandle, path: &str, payload: &Value) -> Result<Value, String> {
    let rb = authed(
        app,
        http()
            .post(format!("{}{}", base_url(), path))
            .timeout(Duration::from_millis(WRITE_TIMEOUT_MS)),
    )?;
    let resp = rb.json(payload).send().await.map_err(|e| net_err(&e))?;
    parse_v2(resp).await
}

fn uuid_or_err(s: &str) -> Result<String, String> {
    let s = s.trim();
    const UUID_RE_LEN: usize = 36;
    if s.len() != UUID_RE_LEN || s.chars().any(|c| !(c.is_ascii_hexdigit() || c == '-')) {
        return Err("clientRequestId 须为 UUID".into());
    }
    Ok(s.to_string())
}

// ───────────────────────── 命令 ─────────────────────────

/// 发帖（影评分享）。mediaType: movie|tv；score 1–5 或不传。
#[tauri::command]
pub async fn community_v2_post(
    app: tauri::AppHandle,
    media_type: String,
    tmdb_id: i64,
    work_title: String,
    score: Option<i64>,
    title: Option<String>,
    body: String,
    image_ids: Option<Vec<String>>,
    client_request_id: String,
) -> Result<Value, String> {
    let client_request_id = uuid_or_err(&client_request_id)?;
    let text = body.trim().to_string();
    if text.chars().count() < 3 {
        return Err("正文至少 3 个字".into());
    }
    if text.chars().count() > 5000 {
        return Err("正文最多 5000 字".into());
    }
    let payload = json!({
        "mediaType": media_type.trim(),
        "tmdbId": tmdb_id,
        "workTitle": work_title.trim(),
        "score": score,
        "title": title.map(|t| t.trim().to_string()).unwrap_or_default(),
        "body": text,
        "imageIds": image_ids.unwrap_or_default(),
        "clientRequestId": client_request_id,
    });
    post_authed(&app, "/v2/posts", &payload).await
}

/// 信息流（sort: new|hot）。
#[tauri::command]
pub async fn community_v2_feed(app: tauri::AppHandle, sort: String, cursor: Option<i64>) -> Result<Value, String> {
    let s = if sort.trim() == "hot" { "hot" } else { "new" };
    let cursor = cursor.unwrap_or(0).max(0);
    get_authed(&app, &format!("/v2/feed?sort={s}&cursor={cursor}&limit=10")).await
}

/// 作品聚合（AniBox 评分 + 最近帖子 + 作品短评）。
#[tauri::command]
pub async fn community_v2_work(app: tauri::AppHandle, media_type: String, tmdb_id: i64) -> Result<Value, String> {
    get_authed(&app, &format!("/v2/works/{}/{}", media_type.trim(), tmdb_id)).await
}

/// 帖子详情（单帖 + 作品评分摘要 + 作品级短评）。
#[tauri::command]
pub async fn community_v2_post_detail(app: tauri::AppHandle, id: String) -> Result<Value, String> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("缺少帖子 id".into());
    }
    get_authed(&app, &format!("/v2/posts/{}", id)).await
}

/// 删自己的帖子（软删；别人的返回 404）。
#[tauri::command]
pub async fn community_v2_delete_post(app: tauri::AppHandle, id: String) -> Result<Value, String> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("缺少帖子 id".into());
    }
    let rb = authed(
        &app,
        http()
            .delete(format!("{}{}", base_url(), &format!("/v2/posts/{id}")))
            .timeout(Duration::from_millis(WRITE_TIMEOUT_MS)),
    )?;
    let resp = rb.send().await.map_err(|e| net_err(&e))?;
    parse_v2(resp).await
}

/// 作品短评（v2，带账号）。回复时传 parent_id。
#[tauri::command]
pub async fn community_v2_comment(
    app: tauri::AppHandle,
    media_type: String,
    tmdb_id: i64,
    work_title: String,
    body: String,
    client_request_id: String,
    parent_id: Option<String>,
) -> Result<Value, String> {
    let client_request_id = uuid_or_err(&client_request_id)?;
    let text = body.trim().to_string();
    if text.chars().count() < 3 {
        return Err("评论至少 3 个字".into());
    }
    if text.chars().count() > 2000 {
        return Err("评论最多 2000 字".into());
    }
    let mut payload = json!({
        "mediaType": media_type.trim(),
        "tmdbId": tmdb_id,
        "title": work_title.trim(),
        "body": text,
        "clientRequestId": client_request_id,
    });
    if let Some(p) = parent_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        payload["parentId"] = json!(p);
    }
    post_authed(&app, "/v2/comments", &payload).await
}

/// 我的发言（永久可翻；含已删除标记）。
#[tauri::command]
pub async fn community_v2_mine_posts(app: tauri::AppHandle, cursor: Option<i64>) -> Result<Value, String> {
    let cursor = cursor.unwrap_or(0).max(0);
    get_authed(&app, &format!("/v2/mine/posts?cursor={cursor}&limit=10")).await
}

/// 通知列表（含 unread 计数）。
#[tauri::command]
pub async fn community_v2_notifications(app: tauri::AppHandle, cursor: Option<i64>) -> Result<Value, String> {
    let cursor = cursor.unwrap_or(0).max(0);
    get_authed(&app, &format!("/v2/notifications?cursor={cursor}&limit=10")).await
}

/// 通知已读：ids 或 all。
#[tauri::command]
pub async fn community_v2_read_notifications(
    app: tauri::AppHandle,
    ids: Option<Vec<i64>>,
    all: Option<bool>,
) -> Result<Value, String> {
    let mut payload = json!({});
    if all.unwrap_or(false) {
        payload["all"] = json!(true);
    } else {
        payload["ids"] = json!(ids.unwrap_or_default());
    }
    post_authed(&app, "/v2/notifications/read", &payload).await
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_check() {
        assert!(uuid_or_err("00000000-0000-4000-8000-000000000001").is_ok());
        assert!(uuid_or_err("not-a-uuid").is_err());
        assert!(uuid_or_err("").is_err());
        // 长度对但含非法字符
        assert!(uuid_or_err("zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz").is_err());
    }

    #[test]
    fn client_id_length_is_36() {
        // UUID 形状常量与 worker.js 的 UUID_RE 口径一致（36 位）
        const UUID_RE_LEN: usize = 36;
        let u = uuid_or_err("00000000-0000-4000-8000-000000000001").unwrap();
        assert_eq!(u.len(), UUID_RE_LEN);
    }
}
