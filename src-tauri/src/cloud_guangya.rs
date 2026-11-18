//! [lc-2230] 光鸭网盘（guangyapan）—— 设备码登录 + 列目录 + 取链 + token 续期。
//!
//! 端点证据来源：`nianzhibai/91`（MIT）`backend/internal/drives/guangyapan/*` +
//! 本机实测（2026-11-18）。历史结论「光鸭 device/code 404，上游不可用」
//! （docs/cloud-first-design.md:118）是**误判**：当时用的是 LumiPlayer 字符串池里
//! 那个多一位的 `client_id`（`…UR8`）。正确的是 `…UR`，实测 200 正常下发 device_code。
//!
//! 与其它网盘的关键差别：
//! - 登录是 **OAuth 设备码**（`/v1/auth/device/code` → `/v1/auth/token` device_code 授权），
//!   不是二维码图；二维码只是把 `verification_uri_complete` 本地渲染出来。
//! - 请求必须带一整套客户端指纹头（`X-Device-Sign: wdi10.<32位hex>xxx…` 等），
//!   缺任一项上游判为非法客户端。
//! - 取链返回**签名直链**（signedURL），不带账号头，浏览器/播放器可直取。
//! - 文件列表条目自带 `gcid`（40 位十六进制），是匿名字幕 API 的精确匹配键。
//!
//! 凭据只落本机 `app_data_dir/direct-cloud-auth.json`（provider = `guangyapan`），
//! 与其它网盘同仓同待遇，绝不上传。

use serde_json::{json, Value};
use std::time::Duration;

use crate::cloud::{self, qr_svg_data_uri};

/// 官方 web 客户端 client_id（91 `defaultClientID`，实测有效）。
const CLIENT_ID: &str = "aMe-8VSlkrbQXpUR";
const ACCOUNT_BASE: &str = "https://account.guangyapan.com";
const API_BASE: &str = "https://api.guangyapan.com";
const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36";

// ───────────────────────── 设备指纹 ─────────────────────────

/// 32 位小写 hex 设备 id（首次登录生成，随凭据一起持久化）。
fn new_device_id() -> String {
    let mut out = String::with_capacity(32);
    for _ in 0..16 {
        out.push_str(&format!("{:02x}", rand::random::<u8>()));
    }
    out
}

/// 从凭据里取设备 id；不存在则生成（登录时必须先有，才能拼出签名头）。
fn device_id_from(cred: &Value) -> String {
    let existing = cred.get("device_id").and_then(|x| x.as_str()).unwrap_or("");
    let v = existing.trim().to_ascii_lowercase().replace('-', "");
    if v.len() == 32 && v.chars().all(|c| c.is_ascii_hexdigit()) {
        return v;
    }
    new_device_id()
}

/// 账号域请求头（登录/续期；与业务 API 头不同：带 X-Device-Sign 全签名）。
fn account_headers(device_id: &str, captcha: Option<&str>) -> Vec<(String, String)> {
    let mut h = vec![
        ("Accept".to_string(), "application/json, text/plain, */*".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("X-Device-Model".to_string(), "chrome%2F147.0.0.0".to_string()),
        ("X-Device-Name".to_string(), "PC-Chrome".to_string()),
        (
            "X-Device-Sign".to_string(),
            format!("wdi10.{device_id}xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
        ),
        ("X-Net-Work-Type".to_string(), "NONE".to_string()),
        ("X-OS-Version".to_string(), "MacIntel".to_string()),
        ("X-Platform-Version".to_string(), "1".to_string()),
        ("X-Protocol-Version".to_string(), "301".to_string()),
        ("X-Provider-Name".to_string(), "NONE".to_string()),
        ("X-SDK-Version".to_string(), "9.0.2".to_string()),
        ("X-Client-Id".to_string(), CLIENT_ID.to_string()),
        ("X-Client-Version".to_string(), "0.0.1".to_string()),
        ("X-Device-Id".to_string(), device_id.to_string()),
    ];
    if let Some(c) = captcha.filter(|s| !s.is_empty()) {
        h.push(("X-Captcha-Token".to_string(), c.to_string()));
    }
    h
}

/// 业务 API 请求头（列表/取链）：只需 Did + Dt，账号身份走 Bearer。
fn api_headers(device_id: &str) -> Vec<(String, String)> {
    vec![
        ("Accept".to_string(), "application/json, text/plain, */*".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Did".to_string(), device_id.to_string()),
        ("Dt".to_string(), "4".to_string()),
    ]
}

fn post_json_with(
    url: &str,
    body: Value,
    hdrs: &[(String, String)],
    timeout: Duration,
) -> impl std::future::Future<Output = Result<Value, String>> {
    let url = url.to_string();
    let body = body.clone();
    let hdrs = hdrs.to_vec();
    async move {
        let mut req = cloud::client()
            .post(&url)
            .timeout(timeout)
            .header("Content-Type", "application/json")
            .json(&body);
        for (k, v) in &hdrs {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("响应读取失败: {e}"))?;
        // 设备码轮询用 400 表示 authorization_pending，不能当传输错误丢掉 body
        if !status.is_success() && status.as_u16() != 400 {
            return Err(format!("上游 HTTP {}", status.as_u16()));
        }
        serde_json::from_str(&text).map_err(|_| {
            format!("返回非 JSON：{}", text.chars().take(120).collect::<String>())
        })
    }
}

// ───────────────────────── 登录（OAuth 设备码） ─────────────────────────

/// 开始登录：拿 device_code + verification_uri，本地渲二维码。
/// 返回 `{state, qrcode, hint, expiresAtMs, ttlMs, intervalMs}`（与其它厂商同形）。
pub(crate) async fn qr_start() -> Result<Value, String> {
    let device_id = new_device_id();
    let v = post_json_with(
        &format!("{ACCOUNT_BASE}/v1/auth/device/code"),
        json!({ "client_id": CLIENT_ID, "scope": "user" }),
        &account_headers(&device_id, None),
        Duration::from_secs(20),
    )
    .await?;
    let device_code = v.get("device_code").and_then(|x| x.as_str()).unwrap_or("");
    if device_code.is_empty() {
        return Err(format!("光鸭登录码获取失败: {v}"));
    }
    let uri = v
        .get("verification_uri_complete")
        .and_then(|x| x.as_str())
        .filter(|s| s.starts_with("http"))
        .or_else(|| v.get("verification_url").and_then(|x| x.as_str()))
        .unwrap_or("https://account.guangyapan.com/");
    // 上游 expires_in 实测 120s，比其它家短，必须如实透传（前端倒计时按它走）
    let expires_in = v.get("expires_in").and_then(|x| x.as_u64()).unwrap_or(120);
    let interval = v.get("interval").and_then(|x| x.as_u64()).unwrap_or(2);
    let ttl_ms = expires_in.saturating_mul(1000);
    Ok(json!({
        "qrcode": qr_svg_data_uri(uri)?,
        "hint": "用手机浏览器打开光鸭登录页确认，或直接扫二维码".to_string(),
        // device_code 交给前端原样回传（与 QrState 的 state 等价）；设备 id 必须跟着走，
        // 否则确认时 token 会绑到另一个指纹上。
        "state": cloud::pack_guangya_state(device_code, &device_id, ttl_ms),
        "expiresAtMs": cloud::now_ms_pub() + ttl_ms,
        "ttlMs": ttl_ms,
        "intervalMs": interval.saturating_mul(1000).max(1000),
    }))
}

/// 轮询登录状态：返回 `{status, ok, userName?}`（0 待扫 / 1 已扫待确认 / 2 成功落盘 / 3 过期）。
pub(crate) async fn qr_poll(state: &str) -> Result<Value, String> {
    let (device_code, device_id, expired) = cloud::unpack_guangya_state(state)?;
    if expired {
        return Ok(json!({ "status": 3, "expired": true, "ok": false }));
    }
    let v = post_json_with(
        &format!("{ACCOUNT_BASE}/v1/auth/token"),
        json!({
            "client_id": CLIENT_ID,
            "grant_type": DEVICE_CODE_GRANT,
            "device_code": device_code,
        }),
        &account_headers(&device_id, None),
        Duration::from_secs(20),
    )
    .await?;

    let access = v.get("access_token").and_then(|x| x.as_str()).unwrap_or("");
    let refresh = v.get("refresh_token").and_then(|x| x.as_str()).unwrap_or("");
    if !access.is_empty() && !refresh.is_empty() {
        let user_name = user_me(access).await.unwrap_or_else(|| "光鸭网盘".to_string());
        let cred = json!({
            "access_token": access,
            "refresh_token": refresh,
            "device_id": device_id,
            "user_name": user_name,
            "saved_at_ms": cloud::now_ms_pub(),
        });
        cloud::save_provider_auth("guangyapan", cred)?;
        crate::libmpv::trace("[guangyapan] 设备码登录成功，凭据已落盘");
        return Ok(json!({ "status": 2, "ok": true, "userName": user_name }));
    }

    // 未完成时的错误码（OAuth 设备码标准语义）
    match v.get("error").and_then(|x| x.as_str()).unwrap_or("") {
        "authorization_pending" => Ok(json!({ "status": 0, "ok": false })),
        // 用户已在手机上点了，但授权服务还没换出 token
        "slow_down" | "authorization_declined" => Ok(json!({ "status": 1, "ok": false })),
        "expired_token" | "access_denied" => Ok(json!({ "status": 3, "expired": true, "ok": false })),
        other if !other.is_empty() => {
            let desc = v
                .get("error_description")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            Err(format!("光鸭登录失败: {other} {desc}"))
        }
        _ => Ok(json!({ "status": 0, "ok": false })),
    }
}

async fn user_me(access_token: &str) -> Option<String> {
    let h = account_headers("", None);
    let mut req = cloud::client()
        .get(format!("{ACCOUNT_BASE}/v1/user/me"))
        .timeout(Duration::from_secs(15))
        .header("Authorization", format!("Bearer {access_token}"));
    for (k, v) in &h {
        if k == "X-Device-Id" || k == "X-Device-Sign" {
            continue;
        }
        req = req.header(k.as_str(), v.as_str());
    }
    let v: Value = req.send().await.ok()?.json().await.ok()?;
    // sub 是账号 id；没有昵称字段时用它做展示
    let sub = v.get("sub").and_then(|x| x.as_str()).unwrap_or("");
    if sub.is_empty() {
        None
    } else {
        Some(format!("光鸭 {sub}"))
    }
}

// ───────────────────────── 业务 API（带 token 续期） ─────────────────────────

/// 取最新凭据：access_token 缺失时才 refresh；refresh 后落盘。
async fn access_token() -> Result<(String, String), String> {
    let cred = cloud::require_cred("guangyapan", &["refresh_token"])?;
    let device_id = device_id_from(&cred);
    let access = cred.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if !access.is_empty() {
        return Ok((access, device_id));
    }
    let refresh = cred.get("refresh_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let (new_access, new_refresh) = refresh_token(&refresh, &device_id).await?;
    let user_name = cred.get("user_name").and_then(|x| x.as_str()).unwrap_or("光鸭网盘").to_string();
    cloud::save_provider_auth(
        "guangyapan",
        json!({
            "access_token": new_access,
            "refresh_token": new_refresh,
            "device_id": device_id,
            "user_name": user_name,
            "saved_at_ms": cloud::now_ms_pub(),
        }),
    )?;
    Ok((new_access, device_id))
}

/// refresh_token → 新 token 对。光鸭在 access 有效期内不返回新 refresh_token，须保留旧值。
async fn refresh_token(refresh: &str, device_id: &str) -> Result<(String, String), String> {
    let v = post_json_with(
        &format!("{ACCOUNT_BASE}/v1/auth/token"),
        json!({
            "client_id": CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": refresh,
        }),
        &account_headers(device_id, None),
        Duration::from_secs(20),
    )
    .await
    .map_err(|e| format!("光鸭 token 续期失败（可能需要重新登录）: {e}"))?;
    let access = v.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let next_refresh = v
        .get("refresh_token")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(refresh)
        .to_string();
    if access.is_empty() {
        let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("未知错误");
        return Err(format!("光鸭 token 续期被拒: {err}"));
    }
    Ok((access, next_refresh))
}

/// 业务 API POST：401/403 时 refresh 一次重试（与 91 同口径）。
async fn api_post(path: &str, body: Value) -> Result<Value, String> {
    let (token, device_id) = access_token().await?;
    let url = format!("{API_BASE}{path}");
    let mut hdrs = api_headers(&device_id);
    hdrs.push(("Authorization".to_string(), format!("Bearer {token}")));
    match post_json_with(&url, body.clone(), &hdrs, Duration::from_secs(30)).await {
        Ok(v) => {
            if v.get("code").and_then(|x| x.as_i64()).unwrap_or(0) == 0 {
                return Ok(v);
            }
            Err(format!("光鸭接口 {}: {}", path, api_error_text(&v)))
        }
        Err(e) => {
            if !e.contains("401") && !e.contains("403") {
                return Err(e);
            }
            // access token 被拒 → 强制 refresh 一次
            access_token_force_refresh().await?;
            let (token, device_id) = access_token().await?;
            let mut hdrs = api_headers(&device_id);
            hdrs.push(("Authorization".to_string(), format!("Bearer {token}")));
            let v = post_json_with(&url, body, &hdrs, Duration::from_secs(30)).await?;
            if v.get("code").and_then(|x| x.as_i64()).unwrap_or(0) != 0 {
                return Err(format!("光鸭接口 {}: {}", path, api_error_text(&v)));
            }
            Ok(v)
        }
    }
}

/// 强制用 refresh_token 换新 access（上游 401 时调用）。
async fn access_token_force_refresh() -> Result<(), String> {
    let cred = cloud::require_cred("guangyapan", &["refresh_token", "device_id"])?;
    let device_id = device_id_from(&cred);
    let refresh = cred.get("refresh_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let (access, new_refresh) = refresh_token(&refresh, &device_id).await?;
    let user_name = cred.get("user_name").and_then(|x| x.as_str()).unwrap_or("光鸭网盘").to_string();
    cloud::save_provider_auth(
        "guangyapan",
        json!({
            "access_token": access,
            "refresh_token": new_refresh,
            "device_id": device_id,
            "user_name": user_name,
            "saved_at_ms": cloud::now_ms_pub(),
        }),
    )
}

fn api_error_text(v: &Value) -> String {
    let msg = v.get("msg").and_then(|x| x.as_str()).unwrap_or("");
    let code = v.get("code").and_then(|x| x.as_i64()).unwrap_or(0);
    if msg.is_empty() {
        format!("code={code}")
    } else {
        format!("code={code} {msg}")
    }
}

// ───────────────────────── 列目录 ─────────────────────────

const PAGE_SIZE: u32 = 100;

/// 统一列目录：`cursor` = 页号（从 0 起，上游分页语义）。返回 `{items, nextCursor}`。
pub(crate) async fn list(fid: &str, cursor: &str) -> Result<Value, String> {
    let page: u32 = if cursor.is_empty() { 0 } else { cursor.parse().unwrap_or(0) };
    let v = api_post(
        "/userres/v1/file/get_file_list",
        json!({
            "parentId": fid,
            "page": page,
            "pageSize": PAGE_SIZE,
            "orderBy": 3,
            "sortType": 1,
            "fileTypes": [],
        }),
    )
    .await?;
    let list = v
        .pointer("/data/list")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let total = v.pointer("/data/total").and_then(|x| x.as_u64()).unwrap_or(0);
    let items: Vec<Value> = list
        .iter()
        .map(|it| {
            let name = it.get("fileName").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let res_type = it.get("resType").and_then(|x| x.as_i64()).unwrap_or(0);
            let is_dir = res_type == 2;
            let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let kind = if is_dir {
                "dir"
            } else if VideoCat::is_video(&ext) {
                "video"
            } else if VideoCat::is_audio(&ext) {
                "audio"
            } else {
                "other"
            };
            json!({
                "fileId": it.get("fileId").and_then(|x| x.as_str()).unwrap_or(""),
                "navId": it.get("fileId").and_then(|x| x.as_str()).unwrap_or(""),
                "name": name,
                "isDir": is_dir,
                "size": it.get("fileSize").and_then(|x| x.as_u64()).unwrap_or(0),
                "kind": kind,
                // 光鸭列表自带 gcid —— 匿名字幕精确匹配键，透出给前端/媒体库用
                "hash": normalize_gcid(it.get("gcid").and_then(|x| x.as_str()).unwrap_or("")),
            })
        })
        .collect();
    let fetched = ((page as u64) + 1) * PAGE_SIZE as u64;
    let next_cursor = if (list.len() as u32) < PAGE_SIZE || (total > 0 && fetched >= total) {
        String::new()
    } else {
        (page + 1).to_string()
    };
    Ok(json!({ "items": items, "nextCursor": next_cursor, "total": total }))
}

/// 40 位十六进制 gcid 归一化（其余一律空串，避免把乱码当哈希用）。
pub(crate) fn normalize_gcid(value: &str) -> String {
    let v = value.trim();
    if v.len() != 40 || !v.chars().all(|c| c.is_ascii_hexdigit()) {
        return String::new();
    }
    v.to_ascii_uppercase()
}

/// 扩展名分类（与 cloud.rs 其它厂商同一套表，避免各写一份）。
struct VideoCat;
impl VideoCat {
    fn is_video(ext: &str) -> bool {
        ["mkv", "mp4", "avi", "wmv", "flv", "mov", "iso", "m2ts", "webm", "ts", "rmvb", "mpg"]
            .contains(&ext)
    }
    fn is_audio(ext: &str) -> bool {
        ["mp3", "m4a", "aac", "wav", "ogg", "flac", "ape"].contains(&ext)
    }
}

// ───────────────────────── 取链 ─────────────────────────

/// 取签名直链：优先 `signedURL`，回落 `downloadUrl`。链接不带账号头，可直接播。
pub(crate) async fn resolve_play(file_id: &str) -> Result<String, String> {
    let v = api_post(
        "/nd.bizuserres.s/v1/get_res_download_url",
        json!({ "fileId": file_id }),
    )
    .await?;
    let signed = v.pointer("/data/signedURL").and_then(|x| x.as_str()).unwrap_or("");
    let plain = v.pointer("/data/downloadUrl").and_then(|x| x.as_str()).unwrap_or("");
    let url = if !signed.is_empty() { signed } else { plain };
    if url.is_empty() {
        return Err(format!("光鸭未返回下载地址: {}", api_error_text(&v)));
    }
    Ok(url.to_string())
}

/// 重命名（`/nd.bizuserres.s/v1/file/rename`，91 同口径）。
pub(crate) async fn rename(file_id: &str, new_name: &str) -> Result<(), String> {
    let v = api_post(
        "/nd.bizuserres.s/v1/file/rename",
        json!({ "fileId": file_id, "fileName": new_name }),
    )
    .await?;
    if v.get("code").and_then(|x| x.as_i64()).unwrap_or(0) != 0 {
        return Err(format!("光鸭改名被拒: {}", api_error_text(&v)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcid_normalization_rejects_garbage() {
        assert_eq!(
            normalize_gcid(" ab4a38aa8bc710c7c0c7829cae9a3fd6678ec4ea "),
            "AB4A38AA8BC710C7C0C7829CAE9A3FD6678EC4EA"
        );
        assert_eq!(normalize_gcid(""), "");
        assert_eq!(normalize_gcid("not-a-hash"), "");
        assert_eq!(normalize_gcid(&"z".repeat(40)), "");
    }

    #[test]
    fn device_id_is_32_hex_and_stable() {
        let id = new_device_id();
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        // 已有合法值必须原样沿用（换 id 会让已发出的 device_code 绑错指纹）
        let cred = json!({ "device_id": "0123456789abcdef0123456789abcdef" });
        assert_eq!(device_id_from(&cred), "0123456789abcdef0123456789abcdef");
        // 带连字符/大写/超长的值归一化后再用
        let cred = json!({ "device_id": "01234567-89AB-CDEF-0123-456789ABCDEF" });
        assert_eq!(device_id_from(&cred), "0123456789abcdef0123456789abcdef");
        // 非法值必须重新生成（32 位，不能把坏值直接发给上游）
        let cred = json!({ "device_id": "short" });
        assert_eq!(device_id_from(&cred).len(), 32);
    }

    #[test]
    fn headers_carry_device_signature() {
        let h = account_headers("0123456789abcdef0123456789abcdef", None);
        let get = |k: &str| h.iter().find(|(name, _)| name == k).map(|(_, v)| v.clone());
        assert_eq!(get("X-Client-Id").as_deref(), Some(CLIENT_ID));
        assert_eq!(get("X-Device-Id").as_deref(), Some("0123456789abcdef0123456789abcdef"));
        let sign = get("X-Device-Sign").unwrap();
        assert!(sign.starts_with("wdi10."));
        assert!(sign.contains("0123456789abcdef0123456789abcdef"));
    }
}
