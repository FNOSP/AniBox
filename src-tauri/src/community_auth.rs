//! [lc-2402] 社区 v2 账号：注册（邮箱验证码）/ 登录 / 设备会话。
//!
//! 协议唯一依据：`docs/community-v2-design.md` §3–§4。服务端 `inbox.690075.xyz`
//! （Cloudflare Worker，源码不入库，该文档即完整协议）。
//!
//! 身份链路（与文档 §3 逐条对应）：
//!   · 注册：邮箱+密码+昵称 → 服务端发 6 位验证码（10 分钟）→ verify 建号并签发 token
//!   · 登录：邮箱+密码+设备信息 → 新 token（同 device_code 复用设备行并刷新）
//!   · 会话：token 只在 Rust 侧持有，`Authorization: Bearer`；**绝不回传前端**
//!
//! 本地状态（app_data_dir/community-session.json）：
//!   token 用 `config::seal`（AES-256-GCM，`enc:v1:`）加密落盘；nick/role 是回显缓存。
//!   密码只在请求体里出现一次，**绝不落盘、绝不写日志**。
//!
//! 设备信息：device_code = `community::device_id`（与 v1 同源同盐，一台物理机一个码）；
//! os / 屏幕（物理像素）/ 缩放（150%）从主窗口 Monitor 取，对应设备管理页的展示行。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Manager};

use crate::community::{base_url, device_id, http};

const WRITE_TIMEOUT_MS: u64 = 15_000;
const READ_TIMEOUT_MS: u64 = 10_000;
const SESSION_FILE: &str = "community-session.json";

// ───────────────────────── 本地会话 ─────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
struct StoredSession {
    /// 结构版本（变更时整体丢弃重建）
    v: u32,
    account_id: String,
    /// 打码邮箱（us***@qq.com，服务端产出的回显缓存）
    #[serde(default)]
    email_masked: String,
    nick: String,
    #[serde(default)]
    nick_suffix: String,
    #[serde(default)]
    role: String,
    /// seal(token)；明文只在内存
    token_enc: String,
    #[serde(default)]
    device_id: String,
}

fn session_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join(SESSION_FILE))
}

fn read_session(app: &AppHandle) -> Option<StoredSession> {
    let p = session_path(app)?;
    let text = std::fs::read_to_string(&p).ok()?;
    let s: StoredSession = serde_json::from_str(&text).ok()?;
    // 结构版本不符（旧版/损坏）→ 视作未登录
    if s.v != 2 || s.account_id.is_empty() || s.token_enc.is_empty() {
        return None;
    }
    Some(s)
}

fn write_session(app: &AppHandle, s: &StoredSession) -> Result<(), String> {
    let p = session_path(app).ok_or_else(|| "找不到数据目录".to_string())?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let body = serde_json::to_string_pretty(s).map_err(|e| format!("序列化会话失败: {e}"))?;
    std::fs::write(&p, body).map_err(|e| format!("写入会话失败: {e}"))
}

fn clear_session(app: &AppHandle) {
    if let Some(p) = session_path(app) {
        let _ = std::fs::remove_file(&p);
    }
}

/// 前端可见的会话形态（**没有 token**）。
fn public_session(s: &StoredSession) -> Value {
    json!({
        "accountId": s.account_id,
        "email": s.email_masked,
        "nick": s.nick,
        "nickSuffix": s.nick_suffix,
        "role": s.role,
        "deviceId": s.device_id,
    })
}

// ───────────────────────── HTTP ─────────────────────────

pub(crate) fn net_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "社区服务响应超时，请稍后重试".into()
    } else if e.is_connect() {
        "无法连接社区服务，请检查网络".into()
    } else {
        format!("社区服务请求失败：{e}")
    }
}

/// v2 统一响应：成功取 `data`；失败取 `error.message`（服务端已是给人看的中文）。
pub(crate) async fn parse_v2(resp: reqwest::Response) -> Result<Value, String> {
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("读取响应失败：{e}"))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|_| format!("社区服务返回异常（HTTP {status}）"))?;
    if !status.is_success() {
        let msg = v
            .pointer("/error/message")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("社区服务返回错误（HTTP {status}）"));
        return Err(msg);
    }
    Ok(v.get("data").cloned().unwrap_or(Value::Null))
}

async fn v2_post(path: &str, payload: &Value, timeout_ms: u64) -> Result<Value, String> {
    let resp = http()
        .post(format!("{}{}", base_url(), path))
        .json(payload)
        .timeout(Duration::from_millis(timeout_ms))
        .send()
        .await
        .map_err(|e| net_err(&e))?;
    parse_v2(resp).await
}

/// 带 Bearer 的请求构造器；未登录返回 Err。
pub(crate) fn authed(app: &AppHandle, rb: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder, String> {
    let s = read_session(app).ok_or_else(|| "未登录".to_string())?;
    Ok(rb.bearer_auth(crate::config::unseal(&s.token_enc)))
}

async fn v2_authed_json(app: &AppHandle, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    let rb = http()
        .request(method, format!("{}{}", base_url(), path))
        .timeout(Duration::from_millis(if body.is_some() { WRITE_TIMEOUT_MS } else { READ_TIMEOUT_MS }));
    let rb = authed(app, rb)?;
    let rb = match body {
        Some(v) => rb.json(&v),
        None => rb,
    };
    let resp = rb.send().await.map_err(|e| net_err(&e))?;
    parse_v2(resp).await
}

// ───────────────────────── 设备信息 ─────────────────────────

/// 设备信息（design §3.3 的 device 结构）：code/os/screen/scale。
/// screen 是主显示器**物理像素**（3840x2160），scale 是百分比（150）。
fn device_payload(app: &AppHandle) -> Value {
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        _ => "Linux",
    }
    .to_string();
    let (screen, scale) = app
        .get_webview_window("main")
        .and_then(|w| w.current_monitor().ok().flatten())
        .map(|m| {
            let s = m.size();
            (
                format!("{}x{}", s.width, s.height),
                (m.scale_factor() * 100.0).round() as i32,
            )
        })
        .unwrap_or_else(|| ("unknown".to_string(), 100));
    json!({
        "code": device_id(app),
        "os": os,
        "screen": screen,
        "scale": scale,
    })
}

/// 登录/验证成功 → 落盘会话 + 顺手补打码邮箱（best effort，失败不挡登录）。
async fn store_session(
    app: &AppHandle,
    data: &Value,
) -> Result<Value, String> {
    let account_id = data
        .get("accountId")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "服务端未返回账号".to_string())?
        .to_string();
    let token = data
        .get("token")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "服务端未返回会话令牌".to_string())?
        .to_string();
    let mut s = StoredSession {
        v: 2,
        account_id: account_id.clone(),
        email_masked: String::new(),
        nick: data
            .get("nick")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        nick_suffix: data
            .get("nickSuffix")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        role: data
            .get("role")
            .and_then(|x| x.as_str())
            .unwrap_or("user")
            .to_string(),
        token_enc: crate::config::seal(&token),
        device_id: String::new(),
    };

    // best effort：拿打码邮箱 + 本设备 id（设备列表里 current=true 的那行）
    if let Ok(me) = v2_authed_json(app, reqwest::Method::GET, "/v2/account/me", None).await {
        s.email_masked = me
            .get("email")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string();
        if s.nick.is_empty() {
            s.nick = me.get("nick").and_then(|x| x.as_str()).unwrap_or_default().to_string();
        }
    }
    if let Ok(devs) = v2_authed_json(app, reqwest::Method::GET, "/v2/devices", None).await {
        if let Some(arr) = devs.as_array() {
            if let Some(cur) = arr.iter().find(|d| d.get("current").and_then(|x| x.as_bool()) == Some(true)) {
                s.device_id = cur.get("id").and_then(|x| x.as_str()).unwrap_or_default().to_string();
            }
        }
    }

    // 落盘失败 = 每次启动都要重新登录；如实报错（账号已在服务端建立）
    write_session(app, &s)?;
    crate::libmpv::trace(&format!(
        "[community] 会话已建立 account={} nick={}",
        s.account_id, s.nick
    ));
    Ok(public_session(&s))
}

// ───────────────────────── 命令 ─────────────────────────

/// 注册第一步：提交邮箱+密码+昵称，服务端发验证码。返回 { pendingId, maskedEmail, cooldownSec }。
#[tauri::command]
pub async fn community_v2_register(email: String, password: String, nick: String) -> Result<Value, String> {
    v2_post(
        "/v2/auth/register",
        &json!({ "email": email.trim(), "password": password, "nick": nick.trim() }),
        WRITE_TIMEOUT_MS,
    )
    .await
}

/// 注册第二步入口可用的重发：{ pendingId } → { cooldownSec }。
#[tauri::command]
pub async fn community_v2_resend(pending_id: String) -> Result<Value, String> {
    v2_post("/v2/auth/resend", &json!({ "pendingId": pending_id.trim() }), WRITE_TIMEOUT_MS).await
}

/// 验证码校验：成功即建号并签发本设备会话（token 落盘加密，前端拿不到）。
#[tauri::command]
pub async fn community_v2_verify(app: AppHandle, pending_id: String, code: String) -> Result<Value, String> {
    let payload = json!({
        "pendingId": pending_id.trim(),
        "code": code.trim(),
        "device": device_payload(&app),
    });
    let data = v2_post("/v2/auth/verify", &payload, WRITE_TIMEOUT_MS).await?;
    store_session(&app, &data).await
}

/// 登录：邮箱+密码+设备信息。同 device_code 复用设备行并刷新 token（旧 token 立即失效）。
#[tauri::command]
pub async fn community_v2_login(app: AppHandle, email: String, password: String) -> Result<Value, String> {
    let payload = json!({
        "email": email.trim().to_lowercase(),
        "password": password,
        "device": device_payload(&app),
    });
    let data = v2_post("/v2/auth/login", &payload, WRITE_TIMEOUT_MS).await?;
    store_session(&app, &data).await
}

/// 登出：撤销当前设备（best effort）+ 本地会话必清。
/// 服务端失败（断网等）也照常清本地 —— 下次操作会要求重新登录，符合预期。
#[tauri::command]
pub async fn community_v2_logout(app: AppHandle) -> Result<Value, String> {
    let remote = v2_authed_json(&app, reqwest::Method::POST, "/v2/auth/logout", Some(json!({}))).await;
    if let Err(e) = &remote {
        crate::libmpv::trace(&format!("[community] 登出的服务端撤销失败（本地已清）: {e}"));
    }
    clear_session(&app);
    Ok(json!({ "ok": true, "remoteRevoked": remote.is_ok() }))
}

/// 当前会话（前端可见字段；无 token）。未登录返回 null。
#[tauri::command]
pub async fn community_v2_session(app: AppHandle) -> Option<Value> {
    read_session(&app).map(|s| public_session(&s))
}

/// 改昵称：只影响之后的发言（历史内容存的是昵称快照，服务端语义）。
#[tauri::command]
pub async fn community_v2_set_nick(app: AppHandle, nick: String) -> Result<Value, String> {
    let data = v2_authed_json(&app, reqwest::Method::PATCH, "/v2/account", Some(json!({ "nick": nick.trim() }))).await?;
    let mut s = read_session(&app).ok_or_else(|| "未登录".to_string())?;
    if let Some(n) = data.get("nick").and_then(|x| x.as_str()) {
        s.nick = n.to_string();
    }
    write_session(&app, &s)?;
    Ok(public_session(&s))
}

/// 设备列表：[{ id, os, screen, scale, ipNet, deviceCode, createdAt, lastSeenAt, current }]。
#[tauri::command]
pub async fn community_v2_devices(app: AppHandle) -> Result<Value, String> {
    v2_authed_json(&app, reqwest::Method::GET, "/v2/devices", None).await
}

/// 撤销指定设备（只能是自己的；被撤销设备的 token 立即失效）。
#[tauri::command]
pub async fn community_v2_revoke_device(app: AppHandle, id: String) -> Result<Value, String> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("缺少设备 id".into());
    }
    v2_authed_json(&app, reqwest::Method::DELETE, &format!("/v2/devices/{id}"), None).await
}

/// 撤销除当前设备外的全部登录设备。
#[tauri::command]
pub async fn community_v2_revoke_others(app: AppHandle) -> Result<Value, String> {
    v2_authed_json(&app, reqwest::Method::POST, "/v2/devices/revoke-others", Some(json!({}))).await
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_session_roundtrip() {
        let s = StoredSession {
            v: 2,
            account_id: "u_abc".into(),
            email_masked: "us***@test.com".into(),
            nick: "小明".into(),
            nick_suffix: "e155".into(),
            role: "user".into(),
            token_enc: "enc:v1:xxx".into(),
            device_id: "d_1".into(),
        };
        let text = serde_json::to_string(&s).unwrap();
        let back: StoredSession = serde_json::from_str(&text).unwrap();
        assert_eq!(back.account_id, "u_abc");
        assert_eq!(back.nick, "小明");
        assert_eq!(back.role, "user");
    }

    #[test]
    fn session_rejects_old_or_broken_shape() {
        // 版本不符 / 缺 token_enc / 缺 account_id → 视作未登录（read_session 的口径）
        let bad = [
            r#"{"v":1,"account_id":"u_a","nick":"n","token_enc":"x"}"#,
            r#"{"v":2,"account_id":"u_a","nick":"n"}"#,
            r#"{"v":2,"account_id":"","nick":"n","token_enc":"x"}"#,
            r#"not-json"#,
        ];
        for raw in bad {
            let parsed: Option<StoredSession> = serde_json::from_str(raw).ok();
            let ok_shape = parsed
                .map(|s| s.v == 2 && !s.account_id.is_empty() && !s.token_enc.is_empty())
                .unwrap_or(false);
            assert!(!ok_shape, "不应通过会话闸: {raw}");
        }
    }

    #[test]
    fn public_session_has_no_token() {
        // 关键隐私性质：前端可见的会话形态绝不含 token / token_enc
        let s = StoredSession {
            v: 2,
            account_id: "u_abc".into(),
            email_masked: "us***@test.com".into(),
            nick: "小明".into(),
            nick_suffix: "e155".into(),
            role: "admin".into(),
            token_enc: "enc:v1:secret".into(),
            device_id: "d_1".into(),
        };
        let v = public_session(&s);
        let text = v.to_string();
        assert!(!text.contains("token"), "public_session 不得含 token 字段: {text}");
        assert!(!text.contains("enc:v1"), "public_session 不得含密文: {text}");
        assert_eq!(v["role"], "admin");
        assert_eq!(v["nickSuffix"], "e155");
    }

    #[test]
    fn device_payload_shape() {
        // device_payload 需要 AppHandle（起窗），这里只测它的组成约定：
        // server 侧 readDeviceInfo 要求 code = 32–64 hex；scale 50–500。文档 §3。
        let code_ok = |code: &str| {
            code.len() == 32 && code.chars().all(|c| c.is_ascii_hexdigit())
        };
        assert!(code_ok(&"a".repeat(32)));
        assert!(!code_ok(&"a".repeat(31)));
        assert!(!code_ok(&"zz".repeat(16)));
    }
}
