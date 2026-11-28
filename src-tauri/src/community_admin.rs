//! [lc-2446] 社区 v2 P4：举报 + 管理命令（仅 role=admin 的账号能通过服务端 requireAdmin）。
//! 协议：docs/community-v2-design.md §4.5/§4.7/§6.1。
//!
//! 权限模型：前端 role=admin 只是渲染管理台入口，**每个管理请求服务端都再过一次
//! requireAdmin**（secret 现算，库里无 role 字段可改）。撤回可恢复、删除不可恢复。

use serde_json::{json, Value};
use std::time::Duration;

use crate::community::{base_url, http};
use crate::community_auth::{authed, net_err, parse_v2};

const READ_TIMEOUT_MS: u64 = 10_000;
const WRITE_TIMEOUT_MS: u64 = 15_000;

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

const REASONS: [&str; 5] = ["spam", "abuse", "porn", "illegal", "other"];

// ───────────────────────── 举报（所有登录用户）─────────────────────────

/// 举报内容。reason ∈ spam|abuse|porn|illegal|other；detail 可选 ≤500 字。
#[tauri::command]
pub async fn community_v2_report(
    app: tauri::AppHandle,
    kind: String,
    ref_id: String,
    reason: String,
    detail: Option<String>,
) -> Result<Value, String> {
    if !REASONS.contains(&reason.trim()) {
        return Err("举报原因不合法".into());
    }
    let ref_id = ref_id.trim().to_string();
    if ref_id.is_empty() || ref_id.len() > 64 {
        return Err("目标 id 不合法".into());
    }
    let payload = json!({
        "kind": kind.trim(),
        "refId": ref_id,
        "reason": reason.trim(),
        "detail": detail.map(|d| d.trim().to_string()).unwrap_or_default(),
    });
    post_authed(&app, "/v2/reports", &payload).await
}

// ───────────────────────── 管理命令（role=admin）─────────────────────────

/// 举报队列（status: open|resolved|rejected|all）。
#[tauri::command]
pub async fn community_admin_reports(
    app: tauri::AppHandle,
    status: String,
    cursor: Option<i64>,
) -> Result<Value, String> {
    let cursor = cursor.unwrap_or(0).max(0);
    get_authed(&app, &format!("/v2/admin/reports?status={}&cursor={cursor}&limit=20", status.trim())).await
}

/// 处理举报：action = resolve | reject。
#[tauri::command]
pub async fn community_admin_report_handle(
    app: tauri::AppHandle,
    id: String,
    action: String,
    note: Option<String>,
) -> Result<Value, String> {
    let action = action.trim().to_string();
    if action != "resolve" && action != "reject" {
        return Err("action 需 resolve|reject".into());
    }
    let payload = json!({ "action": action, "note": note.map(|n| n.trim().to_string()).unwrap_or_default() });
    post_authed(&app, &format!("/v2/admin/reports/{}/handle", id.trim()), &payload).await
}

/// 撤回帖子（可恢复）。
#[tauri::command]
pub async fn community_admin_retract_post(
    app: tauri::AppHandle,
    id: String,
    reason: Option<String>,
) -> Result<Value, String> {
    post_authed(
        &app,
        &format!("/v2/admin/posts/{}/retract", id.trim()),
        &json!({ "reason": reason.map(|r| r.trim().to_string()).unwrap_or_default() }),
    )
    .await
}

/// 恢复撤回。
#[tauri::command]
pub async fn community_admin_unretract_post(app: tauri::AppHandle, id: String) -> Result<Value, String> {
    post_authed(&app, &format!("/v2/admin/posts/{}/unretract", id.trim()), &json!({})).await
}

/// 删除帖子（不可恢复；正文留审计）。
#[tauri::command]
pub async fn community_admin_delete_post(
    app: tauri::AppHandle,
    id: String,
    reason: Option<String>,
) -> Result<Value, String> {
    post_authed(
        &app,
        &format!("/v2/admin/posts/{}/delete", id.trim()),
        &json!({ "reason": reason.map(|r| r.trim().to_string()).unwrap_or_default() }),
    )
    .await
}

/// 撤回评论（软删 + 占位）。
#[tauri::command]
pub async fn community_admin_retract_comment(
    app: tauri::AppHandle,
    id: String,
    reason: Option<String>,
) -> Result<Value, String> {
    post_authed(
        &app,
        &format!("/v2/admin/comments/{}/retract", id.trim()),
        &json!({ "reason": reason.map(|r| r.trim().to_string()).unwrap_or_default() }),
    )
    .await
}

/// 删除评论。
#[tauri::command]
pub async fn community_admin_delete_comment(
    app: tauri::AppHandle,
    id: String,
    reason: Option<String>,
) -> Result<Value, String> {
    post_authed(
        &app,
        &format!("/v2/admin/comments/{}/delete", id.trim()),
        &json!({ "reason": reason.map(|r| r.trim().to_string()).unwrap_or_default() }),
    )
    .await
}

/// 删除图片（R2 对象 + 行标记）。
#[tauri::command]
pub async fn community_admin_delete_image(
    app: tauri::AppHandle,
    id: String,
    reason: Option<String>,
) -> Result<Value, String> {
    post_authed(
        &app,
        &format!("/v2/admin/images/{}/delete", id.trim()),
        &json!({ "reason": reason.map(|r| r.trim().to_string()).unwrap_or_default() }),
    )
    .await
}

/// 封禁账号。
#[tauri::command]
pub async fn community_admin_ban_account(
    app: tauri::AppHandle,
    id: String,
    reason: Option<String>,
) -> Result<Value, String> {
    post_authed(
        &app,
        &format!("/v2/admin/accounts/{}/ban", id.trim()),
        &json!({ "reason": reason.map(|r| r.trim().to_string()).unwrap_or_default() }),
    )
    .await
}

/// 解封账号。
#[tauri::command]
pub async fn community_admin_unban_account(app: tauri::AppHandle, id: String) -> Result<Value, String> {
    post_authed(&app, &format!("/v2/admin/accounts/{}/unban", id.trim()), &json!({})).await
}

/// 禁言账号（minutes：1–43200）。
#[tauri::command]
pub async fn community_admin_mute_account(
    app: tauri::AppHandle,
    id: String,
    minutes: i64,
    reason: Option<String>,
) -> Result<Value, String> {
    post_authed(
        &app,
        &format!("/v2/admin/accounts/{}/mute", id.trim()),
        &json!({ "minutes": minutes, "reason": reason.map(|r| r.trim().to_string()).unwrap_or_default() }),
    )
    .await
}

/// 封禁设备码（物理机，跨账号；重登无效）。
#[tauri::command]
pub async fn community_admin_ban_device(
    app: tauri::AppHandle,
    device_code: String,
    reason: Option<String>,
) -> Result<Value, String> {
    post_authed(
        &app,
        "/v2/admin/devices/ban-code",
        &json!({
            "deviceCode": device_code.trim(),
            "reason": reason.map(|r| r.trim().to_string()).unwrap_or_default(),
        }),
    )
    .await
}

/// 审计查询（action 支持前缀：admin_ / post_ / comment_ …）。
#[tauri::command]
pub async fn community_admin_audit(
    app: tauri::AppHandle,
    actor_id: Option<String>,
    device_code: Option<String>,
    action: Option<String>,
    cursor: Option<i64>,
) -> Result<Value, String> {
    let cursor = cursor.unwrap_or(0).max(0);
    let mut qs = format!("cursor={cursor}&limit=20");
    if let Some(a) = actor_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        qs.push_str(&format!("&actorId={}", urlencoding::encode(&a)));
    }
    if let Some(d) = device_code.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        qs.push_str(&format!("&deviceCode={}", urlencoding::encode(&d)));
    }
    if let Some(a) = action.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        qs.push_str(&format!("&action={}", urlencoding::encode(&a)));
    }
    get_authed(&app, &format!("/v2/admin/audit?{qs}")).await
}

/// 用量总览（用户/帖子/评论/图片/待处理举报/存储）。
#[tauri::command]
pub async fn community_admin_stats(app: tauri::AppHandle) -> Result<Value, String> {
    get_authed(&app, "/v2/admin/stats").await
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn reasons_match_worker_whitelist() {
        // 与 worker.js 的 REPORT_REASONS 白名单一致（两端口径不同步会出现 400 断层）
        const WORKER_REASONS: [&str; 5] = ["spam", "abuse", "porn", "illegal", "other"];
        const CLIENT_REASONS: [&str; 5] = ["spam", "abuse", "porn", "illegal", "other"];
        for r in CLIENT_REASONS {
            assert!(WORKER_REASONS.contains(&r), "客户端原因 {r} 必须在服务端白名单内");
        }
    }
}
