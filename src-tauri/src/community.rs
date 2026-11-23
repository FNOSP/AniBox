//! [lc-2332] 公共评论客户端（作品级）。
//!
//! 服务端是 `inbox.690075.xyz`（Cloudflare Worker + D1），协议见
//! `docs/community-comments-api.md`。**服务端源码不入库**，文档是唯一依据。
//!
//! 设计取舍（与协议文档一一对应）：
//!
//! 1. **身份是设备码，不是账号**。`sha256("anibox-community-v1:" + 机器标识)`，
//!    机器标识与 `usage_stats` 同源（Windows `MachineGuid` / Linux `/etc/machine-id`），
//!    但**加不同的盐** —— 否则评论设备码与统计匿名 ID 相同，两个服务的运营方一对照
//!    就能把「谁在评论」和「谁在用」关联起来。加盐后两者不可互推。
//! 2. **正文原样存储、原样返回**。服务端不转义 HTML（存了就得考虑清洗，不存最安全），
//!    所以**渲染时必须由前端转义** —— React 的 `{text}` 默认就是转义渲染，不要再
//!    用 `dangerouslySetInnerHTML`。
//! 3. **失败了要说出来**，不能静默。评论是用户的真实创作，发出去失败却不告诉他，
//!    他会以为发成功了然后丢掉内容。所以写操作失败一律返回 `Err` 给前端。
//! 4. 读失败就返回空列表 + 错误信息（不影响播放，评论只是页面上的一块）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

use crate::AppState;

/// 服务根地址。留空则整个评论功能隐藏（自建用户可完全离线）。
pub const DEFAULT_BASE: &str = "https://inbox.690075.xyz";

/// 设备码的盐。**改动会让所有用户变成「新用户」**（历史评论找不回来），不要动。
const DEVICE_SALT: &str = "anibox-community-v1:";

const LIST_TIMEOUT_MS: u64 = 8000;
const WRITE_TIMEOUT_MS: u64 = 12000;

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("AniBox/community")
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("community http client")
    })
}

// ───────────────────────── 身份 ─────────────────────────

/// 设备码 = sha256(salt + 机器标识)，取前 32 位 hex（协议要求 32–64 位）。
///
/// 加盐的理由见文件头第 1 条：防止与 `usage_stats` 的匿名 ID 互相关联。
/// 资产标识拿不到时（极罕见的无权限/无该注册表项）回退到一个随机值 —— 随机值
/// 每次启动都变，会导致「评论历史消失」；所以回退值要**落盘**，见 `device_id()`。
pub fn derive_device_id(raw_machine: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(DEVICE_SALT.as_bytes());
    h.update(raw_machine.trim().as_bytes());
    let digest = h.finalize();
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

fn fallback_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("community-device-id.txt"))
}

/// 取设备码。机器标识可用时随时可再算；不可用时用落盘的随机值。
///
/// 为什么不能每次都现算随机值：那样重装/重启就换身份，用户的历史评论全部找不到。
fn device_id(app: &tauri::AppHandle) -> String {
    let raw = crate::usage_stats::machine_raw_id_pub();
    if !raw.trim().is_empty() {
        return derive_device_id(&raw);
    }
    // 拿不到机器标识：读落盘值，没有就生成一个并写盘
    if let Some(p) = fallback_path(app) {
        if let Ok(s) = std::fs::read_to_string(&p) {
            let s = s.trim().to_string();
            if s.len() >= 32 && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
                return s;
            }
        }
        let id = random_device_id();
        let _ = std::fs::write(&p, &id);
        return id;
    }
    // 最后兜底：从一个稳定但弱的值派生（至少不会每次都变）
    derive_device_id("anibox-fallback")
}

/// 生成随机设备码（仅当拿不到机器标识时用）。复用 `rand` 而不是引 windows-sys
/// 的 BCryptGenRandom —— 后者要额外开 `Win32_Security_Cryptography` feature，
/// 而 rand 已经是既有依赖（`usage_stats::random_anon_id` 就在用）。
fn random_device_id() -> String {
    let (a, b) = (rand::random::<u64>(), rand::random::<u64>());
    format!("{a:016x}{b:016x}")
}

// ───────────────────────── 数据类型 ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunityComment {
    pub id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub root_id: Option<String>,
    pub nick: String,
    /// 软删除后为 null
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub deleted: bool,
    pub created_at: i64,
    /// 服务端按 device_id 判定，不是按昵称
    #[serde(default)]
    pub mine: bool,
    /// 列表接口会在主评论上内嵌最多 3 条回复
    #[serde(default)]
    pub replies: Vec<CommunityComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentPage {
    pub items: Vec<CommunityComment>,
    #[serde(default)]
    pub next_cursor: Option<i64>,
    #[serde(default)]
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkRef {
    pub media_type: String,
    pub tmdb_id: i64,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MyComment {
    #[serde(flatten)]
    pub comment: CommunityComment,
    pub work: WorkRef,
}

// ───────────────────────── 昵称设置 ─────────────────────────

const NICK_FILE: &str = "community-nick.json";

fn nick_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    use tauri::Manager;
    app.path().app_data_dir().ok().map(|d| d.join(NICK_FILE))
}

/// 读取本地设置的昵称（空 = 未设置，服务端会生成「观众-xxxx」）。
pub fn read_nick(app: &tauri::AppHandle) -> String {
    let Some(p) = nick_path(app) else {
        return String::new();
    };
    let Ok(text) = std::fs::read_to_string(&p) else {
        return String::new();
    };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("nick").and_then(|n| n.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// 设置昵称。空串 = 清除（回到服务端自动生成）。
///
/// 注意：**改昵称只影响之后的评论** —— 服务端在每条评论上存了昵称快照，
/// 已发出的评论保持当时的名字。这是刻意的：别人回复的是「当时的那个小明」。
pub fn write_nick(app: &tauri::AppHandle, nick: &str) -> Result<(), String> {
    let clean = nick.trim();
    let chars = clean.chars().count();
    if chars > 24 {
        return Err("昵称最多 24 个字".into());
    }
    if clean.chars().any(|c| c.is_control()) {
        return Err("昵称不能包含控制字符".into());
    }
    let Some(p) = nick_path(app) else {
        return Err("找不到数据目录".into());
    };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let body = json!({ "nick": clean }).to_string();
    std::fs::write(&p, body).map_err(|e| format!("写入昵称失败: {e}"))
}

/// 昵称放进 HTTP header 前必须百分号编码（header 只能放 ASCII）。
fn encode_header_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 拼一个做了共同处理的请求（设备码 + 可选昵称）。返回 `None` = 功能未配置。
fn build(
    req: reqwest::RequestBuilder,
    device: &str,
    nick: &str,
) -> reqwest::RequestBuilder {
    let req = req.header("X-Anibox-Device", device);
    if nick.trim().is_empty() {
        req
    } else {
        req.header("X-Anibox-Nick", encode_header_value(nick.trim()))
    }
}

fn base_url() -> &'static str {
    DEFAULT_BASE
}

// ───────────────────────── 命令 ─────────────────────────

/// 评论功能是否可用（含设备码就绪）。前端据此决定显不显示入口。
#[tauri::command]
pub async fn community_status(app: tauri::AppHandle) -> Result<Value, String> {
    Ok(json!({
        "enabled": !base_url().is_empty(),
        "base": base_url(),
        // 昵称为空表示用服务端自动生成的「观众-xxxx」
        "nick": read_nick(&app),
    }))
}

/// 读一页评论。
#[tauri::command]
pub async fn community_list(
    app: tauri::AppHandle,
    media_type: String,
    tmdb_id: i64,
    cursor: Option<i64>,
    limit: Option<i64>,
) -> Result<CommentPage, String> {
    if base_url().is_empty() {
        return Err("评论服务未配置".into());
    }
    if !matches!(media_type.as_str(), "movie" | "tv") {
        return Err(format!("mediaType 非法：{media_type}"));
    }
    if tmdb_id <= 0 {
        return Err("tmdbId 不合法".into());
    }
    let device = device_id(&app);
    let nick = read_nick(&app);
    let url = format!(
        "{}/v1/comments?mediaType={}&tmdbId={}&cursor={}&limit={}",
        base_url(),
        media_type,
        tmdb_id,
        cursor.unwrap_or(0).max(0),
        limit.unwrap_or(20).clamp(1, 50)
    );
    let req = build(http().get(&url), &device, &nick).timeout(Duration::from_millis(LIST_TIMEOUT_MS));
    let resp = req
        .send()
        .await
        .map_err(|e| format!("连接评论服务失败：{e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("评论加载失败 HTTP {status}：{}", truncate(&text, 200)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("返回格式错误：{e}"))?;
    serde_json::from_value(v.get("data").cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("评论数据解析失败：{e}"))
}

/// 发表评论或回复。
///
/// `client_request_id` 由**前端**生成并在重试时复用 —— 服务端据此幂等，
/// 网络抖动重发不会产生两条相同评论。
#[tauri::command]
pub async fn community_post(
    app: tauri::AppHandle,
    media_type: String,
    tmdb_id: i64,
    title: String,
    body: String,
    client_request_id: String,
    parent_id: Option<String>,
) -> Result<Value, String> {
    if base_url().is_empty() {
        return Err("评论服务未配置".into());
    }
    if !matches!(media_type.as_str(), "movie" | "tv") {
        return Err(format!("mediaType 非法：{media_type}"));
    }
    if tmdb_id <= 0 {
        return Err("tmdbId 不合法".into());
    }
    // 本地先校验一遍：省一次往返，也让错误提示更即时
    let chars = body.trim().chars().count();
    if chars < 3 {
        return Err("评论至少 3 个字".into());
    }
    if chars > 2000 {
        return Err("评论最多 2000 字".into());
    }
    if client_request_id.trim().is_empty() {
        return Err("缺少 clientRequestId".into());
    }

    let device = device_id(&app);
    let nick = read_nick(&app);
    let mut payload = json!({
        "mediaType": media_type,
        "tmdbId": tmdb_id,
        "title": title,
        "body": body.trim(),
        "clientRequestId": client_request_id,
    });
    if let Some(p) = parent_id.filter(|p| !p.trim().is_empty()) {
        payload["parentId"] = json!(p);
    }
    let req = build(http().post(format!("{}/v1/comments", base_url())), &device, &nick)
        .json(&payload)
        .timeout(Duration::from_millis(WRITE_TIMEOUT_MS));
    let resp = req
        .send()
        .await
        .map_err(|e| format!("发送失败（网络）：{e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        // 服务端给的是给人看的中文 message；取出来直接展示比「HTTP 429」有用得多
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| truncate(&text, 200));
        return Err(msg);
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("返回格式错误：{e}"))?;
    Ok(v.get("data").cloned().unwrap_or(Value::Null))
}

/// 删除自己的评论（软删除：服务端清空正文并保留占位）。
#[tauri::command]
pub async fn community_delete(app: tauri::AppHandle, id: String) -> Result<Value, String> {
    if base_url().is_empty() {
        return Err("评论服务未配置".into());
    }
    if id.trim().is_empty() {
        return Err("缺少评论 id".into());
    }
    let device = device_id(&app);
    let nick = read_nick(&app);
    let req = build(
        http().delete(format!("{}/v1/comments/{}", base_url(), id.trim())),
        &device,
        &nick,
    )
    .timeout(Duration::from_millis(WRITE_TIMEOUT_MS));
    let resp = req
        .send()
        .await
        .map_err(|e| format!("删除失败（网络）：{e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        if status.as_u16() == 404 {
            return Err("这条评论不存在或不属于你".into());
        }
        return Err(format!("删除失败 HTTP {status}：{}", truncate(&text, 200)));
    }
    Ok(json!({ "id": id, "deleted": true }))
}

/// 我的发言。
#[tauri::command]
pub async fn community_mine(
    app: tauri::AppHandle,
    cursor: Option<i64>,
    limit: Option<i64>,
) -> Result<Value, String> {
    if base_url().is_empty() {
        return Err("评论服务未配置".into());
    }
    let device = device_id(&app);
    let nick = read_nick(&app);
    let url = format!(
        "{}/v1/comments/mine?cursor={}&limit={}",
        base_url(),
        cursor.unwrap_or(0).max(0),
        limit.unwrap_or(20).clamp(1, 50)
    );
    let req = build(http().get(&url), &device, &nick).timeout(Duration::from_millis(LIST_TIMEOUT_MS));
    let resp = req
        .send()
        .await
        .map_err(|e| format!("连接评论服务失败：{e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("加载失败 HTTP {status}：{}", truncate(&text, 200)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("返回格式错误：{e}"))?;
    Ok(v.get("data").cloned().unwrap_or(Value::Null))
}

/// 设置 / 清除昵称（空串 = 清除，回到「观众-xxxx」）。
#[tauri::command]
pub async fn community_set_nick(app: tauri::AppHandle, nick: String) -> Result<Value, String> {
    write_nick(&app, &nick)?;
    Ok(json!({ "nick": read_nick(&app) }))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_shape() {
        let a = derive_device_id("machine-guid-aaaa");
        let b = derive_device_id("machine-guid-bbbb");
        assert_eq!(a.len(), 32, "32 位 hex（协议要求 32–64）");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "不同机器不同码");
        assert_eq!(a, derive_device_id("machine-guid-aaaa"), "同机器稳定");
        // 首尾空白不该改变结果
        assert_eq!(a, derive_device_id("  machine-guid-aaaa  "));
    }

    #[test]
    fn device_id_differs_from_usage_stats_anon() {
        // 关键隐私性质：评论设备码不能与统计匿名 ID 相同，否则两个服务一对照
        // 就能把「谁在评论」和「谁在用」关联起来。
        let raw = "machine-guid-aaaa";
        let dev = derive_device_id(raw);
        let anon = crate::usage_stats::derive_anon_id(raw);
        assert_ne!(dev, anon, "设备码与统计匿名 ID 必须不同（加盐隔离）");
    }

    #[test]
    fn header_encoding_is_ascii_safe() {
        assert_eq!(encode_header_value("abc"), "abc");
        assert_eq!(encode_header_value("小明"), "%E5%B0%8F%E6%98%8E");
        let enc = encode_header_value("小明 123");
        assert!(enc.is_ascii(), "编码后必须全是 ASCII（HTTP header 约束）");
        assert!(!enc.contains(' '), "空格也要编码");
    }

    #[test]
    fn truncate_counts_chars() {
        assert_eq!(truncate("一二三四五", 3), "一二三…");
        assert_eq!(truncate("abc", 10), "abc");
    }
}
