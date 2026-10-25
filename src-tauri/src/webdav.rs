//! [lc-1811] WebDAV 媒体源（LumiPlayer streamhub jar 的 WebDAV 能力对齐，
//! Rust 单进程实现，不起 Java 侧车）：
//!
//! - 多账号：`app_data_dir/webdav-accounts.json`，密码复用 config 的 AES-256-GCM
//!   `enc:v1:` 加密（LumiPlayer 用 DPAPI，同为「本机加密、明文只在内存」的口径）
//! - 列目录：PROPFIND Depth:1（XML 响应用宽容正则提取 href/displayname/size/
//!   resourcetype —— WebDAV 命名空间前缀各家不一（D:/d:/无前缀），解析器不认前缀）
//! - 扩展名口径与 LumiPlayer 一致：视频 mkv/mp4/avi/wmv/flv/mov/iso/m2ts/webm/ts，
//!   字幕 ass/srt/ssa/vtt 随片扫描（播放时同目录找同名可挂字幕，下载到临时目录后
//!   `sub-add` 挂给 mpv）
//! - 播放：直链 = URL + Basic Auth，走 cloud_proxy 统一链路（Range 透传 + 流式转发；
//!   WebDAV 直链永不过期，续签路径天然不触发）
//!
//! 不做（与 LumiPlayer 的差异，如实说明）：TMDB 刮削建库 / scanIntervalMinutes 定时
//! 同步 —— 那是 streamhub jar 的刮削服务器职责，AniBox 的元数据体系在飞牛侧，
//! WebDAV 在 AniBox 是「直连浏览型」媒体源。

use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct WebdavAccount {
    id: String,
    name: String,
    /// 服务器根地址（https://dav.example.com/dav 或 Alist 端点）
    url: String,
    username: String,
    /// enc:v1: 密文（config::encrypt_access_code）
    password_enc: String,
    saved_at_ms: u64,
}

const VIDEO_EXTS: &[&str] = &["mkv", "mp4", "avi", "wmv", "flv", "mov", "iso", "m2ts", "webm", "ts", "rmvb", "mpg"];
const SUB_EXTS: &[&str] = &["ass", "srt", "ssa", "vtt"];

fn accounts_path() -> Option<std::path::PathBuf> {
    crate::cloud::app_data_dir().map(|d| d.join("webdav-accounts.json"))
}

fn load_accounts() -> Vec<WebdavAccount> {
    let Some(p) = accounts_path() else { return Vec::new() };
    let Ok(s) = std::fs::read_to_string(p) else { return Vec::new() };
    serde_json::from_str::<Vec<WebdavAccount>>(&s).unwrap_or_default()
}

fn save_accounts(list: &[WebdavAccount]) -> Result<(), String> {
    let p = accounts_path().ok_or("app_data_dir 未初始化")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let s = serde_json::to_string_pretty(list).map_err(|e| format!("序列化账号失败: {e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写账号文件失败: {e}"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 AniBox-WebDAV/1.0")
            .danger_accept_invalid_certs(true) // Alist/NAS 自签证书常见（LumiPlayer --insecure 同口径）
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("webdav http client")
    })
}

fn basic_auth(user: &str, pass: &str) -> String {
    use base64::Engine;
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}")))
}

/// 拼完整 URL：目录型服务器（根地址以 / 结尾或 path 以 / 开头）容错拼接。
fn join_url(base: &str, path: &str) -> String {
    let b = base.trim_end_matches('/');
    let p = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    // path 可能已 URL 编码（PROPFIND 返回的 href 是编码过的）——保持原样拼
    format!("{b}{p}")
}

// ───────────────────────── 账号管理 ─────────────────────────

#[tauri::command]
pub async fn webdav_accounts() -> Result<Value, String> {
    Ok(Value::Array(
        load_accounts()
            .into_iter()
            .map(|a| json!({ "id": a.id, "name": a.name, "url": a.url, "username": a.username }))
            .collect(),
    ))
}

#[tauri::command]
pub async fn webdav_account_add(name: String, url: String, username: String, password: String) -> Result<Value, String> {
    let name = name.trim().to_string();
    let url = url.trim().trim_end_matches('/').to_string();
    if name.is_empty() || url.is_empty() || !url.starts_with("http") {
        return Err("名称与地址必填（地址须 http(s):// 开头）".into());
    }
    // 连通性验证：PROPFIND 根目录（Depth:0），失败直接报给用户
    let auth = basic_auth(&username, &password);
    let resp = client()
        .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
        .header("Authorization", &auth)
        .header("Depth", "0")
        .header("Content-Type", "application/xml")
        .body(r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/></D:prop></D:propfind>"#)
        .send()
        .await
        .map_err(|e| format!("连接失败: {e}"))?;
    if !resp.status().is_success() && resp.status().as_u16() != 207 {
        return Err(format!("WebDAV 服务器返回 {}（检查地址/账号/密码）", resp.status().as_u16()));
    }
    let mut list = load_accounts();
    let id = format!("wd-{:x}", now_ms());
    list.push(WebdavAccount {
        id: id.clone(),
        name,
        url,
        username,
        password_enc: crate::config::encrypt_access_code(&password),
        saved_at_ms: now_ms(),
    });
    save_accounts(&list)?;
    Ok(json!({ "id": id }))
}

#[tauri::command]
pub async fn webdav_account_del(id: String) -> Result<(), String> {
    let mut list = load_accounts();
    list.retain(|a| a.id != id);
    save_accounts(&list)
}

fn find_account(id: &str) -> Result<WebdavAccount, String> {
    load_accounts()
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| "WebDAV 账号不存在".to_string())
}

// ───────────────────────── 列目录（PROPFIND Depth:1） ─────────────────────────

/// 宽容 XML 值提取：任意前缀的 `<x:tag>…</x:tag>`（D:/d:/无前缀都认）。
/// 自闭合形态（`<x:tag/>`，如 `<D:collection/>`）追加一个空串 —— 调用方只做存在性判断。
fn xml_tags(xml: &str, tag: &str) -> Vec<String> {
    let pair = regex::Regex::new(&format!(
        r"(?is)<(?:[A-Za-z0-9_]+:)?{tag}(\s[^>]*)?>(.*?)</(?:[A-Za-z0-9_]+:)?{tag}\s*>"
    ))
    .expect("webdav xml regex");
    let mut v: Vec<String> = pair.captures_iter(xml).map(|c| c[2].to_string()).collect();
    let selfclose = regex::Regex::new(&format!(
        r"(?is)<(?:[A-Za-z0-9_]+:)?{tag}(\s[^>]*)?/>"
    ))
    .expect("webdav xml selfclose regex");
    if selfclose.is_match(xml) {
        v.push(String::new());
    }
    v
}

/// 从一段 <response> 块提取一个条目。
fn parse_propfind_entry(block: &str, base: &str) -> Option<Value> {
    let href = xml_tags(block, "href").first()?.trim().to_string();
    let display = xml_tags(block, "displayname").first().map(|s| s.trim().to_string()).unwrap_or_default();
    let is_dir = !xml_tags(block, "collection").is_empty();
    let size = xml_tags(block, "getcontentlength")
        .first()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    // 名称：displayname 优先，缺省用 href 尾段
    let name = if !display.is_empty() {
        display
    } else {
        let dec = percent_decode(&href);
        let p = dec.trim_end_matches('/');
        p.rsplit('/').next().unwrap_or("").to_string()
    };
    if name.is_empty() {
        return None;
    }
    let full_url = join_url(base, &href);
    let path = percent_decode(&href);
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let kind = if is_dir {
        "dir"
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        "video"
    } else if SUB_EXTS.contains(&ext.as_str()) {
        "sub"
    } else {
        "other"
    };
    Some(json!({
        "fileId": full_url,      // WebDAV 的「id」就是完整 URL
        "navId": full_url.clone(),
        "path": path,
        "name": name,
        "isDir": is_dir,
        "size": size,
        "kind": kind,
    }))
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[tauri::command]
pub async fn webdav_list(account_id: String, path: Option<String>) -> Result<Value, String> {
    let acc = find_account(&account_id)?;
    let pass = crate::config::decrypt_access_code(&acc.password_enc);
    let auth = basic_auth(&acc.username, &pass);
    let root_path = path.as_deref().unwrap_or("").trim_end_matches('/').to_string();
    let target = if root_path.is_empty() {
        acc.url.clone()
    } else {
        join_url(&acc.url, &root_path)
    };
    let resp = client()
        .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &target)
        .header("Authorization", &auth)
        .header("Depth", "1")
        .header("Content-Type", "application/xml")
        .body(r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:displayname/><D:getcontentlength/></D:prop></D:propfind>"#)
        .send()
        .await
        .map_err(|e| format!("PROPFIND 失败: {e}"))?;
    let status = resp.status().as_u16();
    if status == 401 {
        return Err("WebDAV 账号或密码错误（401）".into());
    }
    if status != 207 && !(200..299).contains(&status) {
        return Err(format!("WebDAV 服务器返回 {status}"));
    }
    let xml = resp.text().await.map_err(|e| format!("PROPFIND 响应读取失败: {e}"))?;
    let mut items = Vec::new();
    for block in split_responses(&xml) {
        if let Some(it) = parse_propfind_entry(&block, &acc.url) {
            // 过滤目录自身：href 去尾斜杠后 == 请求路径（或根请求时与根一致）
            let p = it.get("path").and_then(|x| x.as_str()).unwrap_or("");
            let norm = |s: &str| s.trim_end_matches('/').to_string();
            let is_root_dup = norm(p) == norm(&root_path)
                || (root_path.is_empty() && norm(p) == norm(&percent_decode(acc.url.trim_start_matches("https://").trim_start_matches("http://"))));
            if is_root_dup {
                continue;
            }
            items.push(it);
        }
    }
    // 目录优先，再按名称排（与网盘面板一致的浏览体验）
    items.sort_by(|a, b| {
        let ad = a.get("isDir").and_then(|x| x.as_bool()).unwrap_or(false);
        let bd = b.get("isDir").and_then(|x| x.as_bool()).unwrap_or(false);
        bd.cmp(&ad).then_with(|| {
            a.get("name").and_then(|x| x.as_str()).unwrap_or("").cmp(b.get("name").and_then(|x| x.as_str()).unwrap_or(""))
        })
    });
    Ok(json!({ "items": items, "nextCursor": "" }))
}

/// 按 <D:response> 块切分（任意前缀）。正向扫描配对：找 open `<x:response…>` 后，
/// 从该位置起**正向**找第一个 `</…response>`（用 rfind 会一路吞到文档尾，只剩 1 块）。
fn split_responses(xml: &str) -> Vec<String> {
    let lower = xml.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < xml.len() {
        let Some(lt) = lower[i..].find('<') else { break };
        let at = i + lt;
        let Some(gt) = lower[at + 1..].find('>') else { break };
        let tag_raw = &lower[at + 1..at + 1 + gt];
        let tag_name = tag_raw.split_whitespace().next().unwrap_or("").trim_end_matches('/');
        let local = tag_name.rsplit(':').next().unwrap_or("");
        if local == "response" {
            let mut j = at + gt;
            let mut close_end = None;
            while let Some(cl) = lower[j..].find("</") {
                let ca = j + cl;
                let Some(cg) = lower[ca + 2..].find('>') else { break };
                let cname_raw = &lower[ca + 2..ca + 2 + cg];
                let cname = cname_raw.rsplit(':').next().unwrap_or("");
                if cname == "response" {
                    close_end = Some(ca + 2 + cg + 1);
                    break;
                }
                j = ca + 2;
            }
            if let Some(ce) = close_end {
                out.push(xml[at..ce].to_string());
                i = ce;
                continue;
            }
        }
        i = at + gt + 1;
    }
    out
}

// ───────────────────────── 播放（含随片字幕） ─────────────────────────

#[tauri::command]
pub async fn webdav_play(
    app: AppHandle,
    state: tauri::State<'_, crate::AppState>,
    account_id: String,
    url: String,
    name: String,
) -> Result<Value, String> {
    let acc = find_account(&account_id)?;
    let pass = crate::config::decrypt_access_code(&acc.password_enc);
    let auth = basic_auth(&acc.username, &pass);
    if !url.starts_with("http") {
        return Err("WebDAV 地址非法".into());
    }
    let _ = app.emit("mpv-load", json!({ "phase": "start" }));

    // 随片字幕：PROPFIND 同目录，找同 basename 的 .ass/.srt/.ssa/.vtt，全部下载到临时目录
    let mut sub_files: Vec<String> = Vec::new();
    if let Some(parent) = parent_url(&url) {
        if let Ok(list) = webdav_list_raw(&acc, &pass, &parent).await {
            let stem = name.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| name.clone());
            for it in list {
                let (iname, iurl, ikind) = (
                    it.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    it.get("fileId").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    it.get("kind").and_then(|x| x.as_str()).unwrap_or(""),
                );
                if ikind != "sub" {
                    continue;
                }
                let istem = iname.rsplit_once('.').map(|(s, _)| s).unwrap_or(iname.as_str());
                if istem == stem || istem == name {
                    if let Ok(local) = download_sub(&acc, &pass, &iurl, &iname).await {
                        sub_files.push(local);
                    }
                }
            }
        }
    }

    // 直链走续签代理（Basic Auth 头由代理携带；WebDAV 永不过期 → TTL=∞）
    let proxied = crate::cloud_proxy::register_webdav(&name, url, ("Authorization".to_string(), auth))
        .await?;
    let cfg = state.config.read().await.clone();
    crate::fnos_progress::begin("", "");
    crate::player::launch(
        &app,
        &state,
        &cfg,
        &proxied,
        &name,
        0.0,
        0.0,
        None,
        &[],
        None,
        true,
    )
    .await?;
    // 字幕挂载：mpv 起播后 sub-add（select 首轨）
    if !sub_files.is_empty() {
        let slot = state.mpv.lock().await;
        if let Some(m) = slot.as_ref() {
            for (i, sf) in sub_files.iter().enumerate() {
                let title = sf.rsplit(['/', '\\']).next().unwrap_or("字幕");
                let _ = m
                    .command(json!(["sub-add", sf, if i == 0 { "select" } else { "auto" }, title]))
                    .await;
            }
        }
    }
    Ok(json!({ "url": proxied, "subs": sub_files.len() }))
}

async fn webdav_list_raw(acc: &WebdavAccount, pass: &str, target: &str) -> Result<Vec<Value>, String> {
    let auth = basic_auth(&acc.username, pass);
    let resp = client()
        .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), target)
        .header("Authorization", &auth)
        .header("Depth", "1")
        .header("Content-Type", "application/xml")
        .body(r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:displayname/><D:getcontentlength/></D:prop></D:propfind>"#)
        .send()
        .await
        .map_err(|e| format!("PROPFIND 失败: {e}"))?;
    if resp.status().as_u16() == 401 {
        return Err("401".into());
    }
    let xml = resp.text().await.map_err(|e| e.to_string())?;
    Ok(split_responses(&xml)
        .iter()
        .filter_map(|b| parse_propfind_entry(b, &acc.url))
        .collect())
}

async fn download_sub(acc: &WebdavAccount, pass: &str, url: &str, name: &str) -> Result<String, String> {
    let auth = basic_auth(&acc.username, pass);
    let resp = client()
        .get(url)
        .header("Authorization", &auth)
        .send()
        .await
        .map_err(|e| format!("字幕下载失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("字幕下载 HTTP {}", resp.status().as_u16()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join("anibox-subs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '.' { c } else { '_' }).collect();
    let p = dir.join(format!("{}-{}", now_ms(), safe));
    std::fs::write(&p, &bytes).map_err(|e| e.to_string())?;
    Ok(p.to_string_lossy().into_owned())
}

/// 父目录 URL（去掉最后一段）。
fn parent_url(url: &str) -> Option<String> {
    let (scheme_rest, path) = url.split_once("://")?;
    let (host, p) = path.split_once('/')?;
    let trimmed = p.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(cut) => Some(format!("{scheme_rest}://{host}/{}", &trimmed[..cut])),
        // 根下文件：父目录就是站点根
        None => Some(format!("{scheme_rest}://{host}/")),
    }
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_extract_basic() {
        let xml = r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:"><D:response><D:href>/dav/%E5%89%A7/a.mkv</D:href><D:propstat><D:prop><D:displayname>a.mkv</D:displayname><D:resourcetype/><D:getcontentlength>123</D:getcontentlength></D:prop></D:propstat></D:response><D:response><D:href>/dav/dir/</D:href><D:propstat><D:prop><D:displayname>dir</D:displayname><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat></D:response></D:multistatus>"#;
        let blocks = split_responses(xml);
        assert_eq!(blocks.len(), 2);
        let a = parse_propfind_entry(&blocks[0], "https://dav.example.com").unwrap();
        assert_eq!(a.get("name").and_then(|x| x.as_str()), Some("a.mkv"));
        assert_eq!(a.get("kind").and_then(|x| x.as_str()), Some("video"));
        assert_eq!(a.get("size").and_then(|x| x.as_u64()), Some(123));
        let b = parse_propfind_entry(&blocks[1], "https://dav.example.com").unwrap();
        assert_eq!(b.get("isDir").and_then(|x| x.as_bool()), Some(true));
    }

    #[test]
    fn xml_extract_no_prefix() {
        let xml = r#"<multistatus><response><href>/x/b.srt</href><propstat><prop><displayname>b.srt</displayname><resourcetype/></prop></propstat></response></multistatus>"#;
        let blocks = split_responses(xml);
        assert_eq!(blocks.len(), 1);
        let a = parse_propfind_entry(&blocks[0], "https://x.com").unwrap();
        assert_eq!(a.get("kind").and_then(|x| x.as_str()), Some("sub"));
        assert!(a.get("fileId").and_then(|x| x.as_str()).unwrap().starts_with("https://x.com/x/"));
    }

    #[test]
    fn percent_decode_works() {
        assert_eq!(percent_decode("/dav/%E5%89%A7/"), "/dav/剧/");
        assert_eq!(percent_decode("/plain/file.mkv"), "/plain/file.mkv");
    }

    #[test]
    fn parent_url_cuts_last_seg() {
        assert_eq!(
            parent_url("https://d.example/dav/dir/a.mkv").unwrap(),
            "https://d.example/dav/dir"
        );
        assert_eq!(
            parent_url("https://d.example/a.mkv").unwrap(),
            "https://d.example/"
        );
    }

    #[test]
    fn join_url_variants() {
        assert_eq!(join_url("https://a.com/dav", "/x/y.mkv"), "https://a.com/dav/x/y.mkv");
        assert_eq!(join_url("https://a.com/dav/", "x/y.mkv"), "https://a.com/dav/x/y.mkv");
    }
}
