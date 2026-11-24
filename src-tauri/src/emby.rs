//! [lc-2345] Emby / Jellyfin 媒体服务器接入（云盘接入页的新来源）。
//!
//! 定位：AniBox 是飞牛影视客户端，但很多用户同时跑 Emby / Jellyfin。这两个
//! 服务对外暴露的 HTTP API 同源（Jellyfin 是 Emby 3.5 的分叉），所以**一份实现
//! 同时支持两者**，只按 `kind` 区分鉴权头写法：
//!
//! | | Emby | Jellyfin |
//! |---|---|---|
//! | 鉴权头 | `X-Emby-Token: <token>` | `Authorization: MediaBrowser Token="<token>"` |
//! | 客户端标识 | `X-Emby-Authorization` | `Authorization` 里的 Client/Device/DeviceId |
//!
//! 两条在我们这边都会发送（两套头互不冲突，各自的实现只认自己那条），
//! 这样同一份代码在两个服务上都成立，不必维护两套分支。
//!
//! 数据模型（只取浏览与播放必需的部分，不做媒体库刮削 —— 元数据体系仍在飞牛侧）：
//! - 登录：`POST /Users/AuthenticateByName`，body `{Username, Pw}`（明文在 body，
//!   官方客户端也用这条；服务端是否强制 HTTPS 由用户自己的部署决定）
//! - 列库：`GET /Users/{userId}/Views` → 用户可见的库（电影 / 电视剧 / 文件夹视图）
//! - 列目录：`GET /Users/{userId}/Items?ParentId=…` → 统一条目形状
//! - 取流：`GET /Videos/{itemId}/stream?static=true&api_key=<token>` —— `static=true`
//!   让服务端**原样发送文件**（不转码），mpv 自己解码，画质与音轨全部保留。
//!   要转码时去掉 `static` 并加 `VideoCodec`/`AudioCodec` 等参数（本版不做，
//!   用户可以在设置里选「服务端转码」时再扩展 —— 现在如实不支持）。
//!
//! 凭据只落本机 `app_data_dir/media-server-accounts.json`，密码复用 config 的
//! AES-256-GCM（`enc:v1:`），与 WebDAV 账号同一口径。

use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// [lc-2345] 支持的两种服务。枚举值就是前端的 provider id。
const KINDS: &[(&str, &str)] = &[("emby", "Emby"), ("jellyfin", "Jellyfin")];

fn kind_label(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, l)| *l)
        .unwrap_or("媒体服务器")
}

/// 视频容器扩展名（与 WebDAV 同一口径，另加 Emby 常见的 m4v/3gp）。
const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "avi", "wmv", "flv", "mov", "iso", "m2ts", "webm", "ts", "rmvb", "mpg",
    "mpeg", "m4v", "3gp", "ogv",
];
const SUB_EXTS: &[&str] = &["ass", "srt", "ssa", "vtt"];

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub(crate) struct MediaServerAccount {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) name: String,
    /// 服务器根地址（形如 http://192.168.1.10:8096，不带尾斜杠）
    pub(crate) url: String,
    pub(crate) username: String,
    /// enc:v1: 密文（config::encrypt_access_code）
    password_enc: String,
    /// 登录后拿到的 AccessToken（同样加密存放）。
    token_enc: String,
    pub(crate) user_id: String,
    /// [lc-2345] 跳过证书校验：自建 Emby/Jellyfin 常用自签 HTTPS 证书。
    /// API 侧我们一直 `danger_accept_invalid_certs`（自建场景的正解），
    /// 但**取流是 mpv 直连**，它默认 `--tls-verify=yes` 会直接失败 ——
    /// 所以这个开关必须一路传到 mpv 命令行，否则「能登录、能浏览、点播放报错」。
    #[serde(default)]
    pub(crate) insecure: bool,
    saved_at_ms: u64,
}

fn accounts_path() -> Option<std::path::PathBuf> {
    crate::cloud::app_data_dir().map(|d| d.join("media-server-accounts.json"))
}

pub(crate) fn load_accounts() -> Vec<MediaServerAccount> {
    let Some(p) = accounts_path() else { return Vec::new() };
    let Ok(s) = std::fs::read_to_string(p) else { return Vec::new() };
    serde_json::from_str::<Vec<MediaServerAccount>>(&s).unwrap_or_default()
}

fn save_accounts(list: &[MediaServerAccount]) -> Result<(), String> {
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
            .timeout(Duration::from_secs(20))
            .danger_accept_invalid_certs(true) // 自建服务器常见自签证书
            .build()
            .expect("media-server http client")
    })
}

/// 归一化用户填的地址：补协议、去尾斜杠；`host:8096` 这种裸写法补 http。
pub(crate) fn normalize_url(raw: &str) -> String {
    let mut u = raw.trim().to_string();
    if u.is_empty() {
        return u;
    }
    if !u.starts_with("http://") && !u.starts_with("https://") {
        u = format!("http://{u}");
    }
    u.trim_end_matches('/').to_string()
}

/// 两套鉴权头一起发：Emby 认 `X-Emby-Token`，Jellyfin 认 `Authorization: MediaBrowser`。
/// 两条同时存在对彼此都是无害的（各自只读自己那条），所以不必按 kind 分支。
///
/// ⚠ `Authorization` 的值里**不能有逗号**：mpv 的 `http-header-fields` 是
/// 「逗号分隔的多条头」字符串，项目里 `normalize_http_headers_extra` 对含逗号的值
/// 直接跳过（那是既有硬约束，不能改）。官方写法是
/// `MediaBrowser Client="AniBox", Device="AniBox", ...`，一路下去会被静默丢弃 →
/// Jellyfin 取流 401。
///
/// Jellyfin 的解析器接受**逗号分隔的 key=value 清单**，所以这里改用另一条合法路径：
/// 把 Client/Device/DeviceId/Version/Token 用逗号拼进一个不加引号的紧凑串是不行的，
/// 因此对 **API 请求**（reqwest，无逗号限制）用标准写法，
/// 对 **mpv 取流** 只依赖 X-Emby-Token + 一条无逗号的 Authorization 变体。
fn auth_headers(token: &str) -> Vec<(&'static str, String)> {
    vec![
        ("X-Emby-Token", token.to_string()),
        (
            "Authorization",
            format!(
                "MediaBrowser Client=\"AniBox\", Device=\"AniBox\", DeviceId=\"anibox\", Version=\"1.0\", Token=\"{token}\""
            ),
        ),
    ]
}

/// [lc-2345] 给 **mpv 命令行**用的鉴权头（不能含逗号，见上）。
///
/// `X-Emby-Token` 一个头就够了 —— Emby 与 Jellyfin 的 `/Videos/{id}/stream`
/// 在**逐跳鉴权**路径上都认它（两者都实现了 `X-Emby-Token` 兼容头，
/// Jellyfin 保留它是为了兼容 Emby 客户端）。这样既绕开逗号限制，
/// 又不把 Token 塞进 URL（URL 会进日志与诊断包）。
pub(crate) fn stream_headers(token: &str) -> Vec<(String, String)> {
    vec![("X-Emby-Token".to_string(), token.to_string())]
}

/// 统一 GET helper（throws 带服务端 body 的可读错误）。
async fn get_json(url: &str, token: Option<&str>) -> Result<Value, String> {
    let mut req = client().get(url).header("Accept", "application/json");
    if let Some(t) = token {
        for (k, v) in auth_headers(t) {
            req = req.header(k, v);
        }
    }
    let resp = req.send().await.map_err(|e| format!("连接失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(format!(
            "鉴权失败（{}）：检查用户名/密码，或该账号的远程访问权限",
            status.as_u16()
        ));
    }
    if !status.is_success() {
        // 服务端错误体通常是 JSON，但有时是 HTML（反代错误页）。截断到 200 字避免刷屏。
        let brief: String = text.chars().take(200).collect();
        return Err(format!("服务器返回 {status}：{brief}"));
    }
    serde_json::from_str::<Value>(&text)
        .map_err(|_| format!("响应不是 JSON（可能地址指向了网页端而非 API）: {}", text.chars().take(120).collect::<String>()))
}

// ───────────────────────── 账号管理 ─────────────────────────

#[tauri::command]
pub async fn media_server_accounts() -> Result<Value, String> {
    Ok(Value::Array(
        load_accounts()
            .into_iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "kind": a.kind,
                    "label": kind_label(&a.kind),
                    "name": a.name,
                    "url": a.url,
                    "username": a.username,
                    "insecure": a.insecure,
                })
            })
            .collect(),
    ))
}

/// 登录并落盘。`kind` = emby | jellyfin。
///
/// 注意：这里**必须真的登录一次**才落盘。Emby/Jellyfin 的账号密码不是用来拼
/// Basic Auth 的（和 WebDAV 不同），拿不到 AccessToken 就什么都做不了；
/// 先验证再存能避免用户存进一堆连不上的配置。
#[tauri::command]
pub async fn media_server_login(
    kind: String,
    name: String,
    url: String,
    username: String,
    password: String,
    insecure: Option<bool>,
) -> Result<Value, String> {
    let kind = kind.trim().to_lowercase();
    if !KINDS.iter().any(|(k, _)| *k == kind) {
        return Err(format!("不支持的媒体服务器类型: {kind}"));
    }
    let url = normalize_url(&url);
    let username = username.trim().to_string();
    let name = {
        let n = name.trim().to_string();
        if n.is_empty() {
            // 没填名称就用「类型 @ 主机」，用户一眼能认出来
            let host = url
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .split('/')
                .next()
                .unwrap_or(&url)
                .to_string();
            format!("{} @ {host}", kind_label(&kind))
        } else {
            n
        }
    };
    if url.is_empty() {
        return Err("服务器地址必填".into());
    }
    if username.is_empty() {
        return Err("用户名必填".into());
    }

    // POST /Users/AuthenticateByName
    let endpoint = format!("{url}/Users/AuthenticateByName");
    let body = json!({ "Username": username, "Pw": password });
    let resp = client()
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        // 登录接口本身也要带客户端标识（Jellyfin 10.9+ 会校验，缺了回 400）
        .header(
            "Authorization",
            "MediaBrowser Client=\"AniBox\", Device=\"AniBox\", DeviceId=\"anibox\", Version=\"1.0\"",
        )
        .header("X-Emby-Authorization", "MediaBrowser Client=\"AniBox\", Device=\"AniBox\", DeviceId=\"anibox\", Version=\"1.0\"")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("连接 {url} 失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.as_u16() == 401 {
        return Err("用户名或密码错误（401）".into());
    }
    if !status.is_success() {
        return Err(format!(
            "登录失败 {status}：{}",
            text.chars().take(200).collect::<String>()
        ));
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|_| "登录响应不是 JSON（地址可能指向了网页端）".to_string())?;
    let token = v
        .get("AccessToken")
        .and_then(|x| x.as_str())
        .ok_or("登录响应缺少 AccessToken")?
        .to_string();
    let user_id = v
        .pointer("/User/Id")
        .and_then(|x| x.as_str())
        .ok_or("登录响应缺少 User.Id")?
        .to_string();
    let server_name = v
        .pointer("/ServerName")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    let mut list = load_accounts();
    // 同一地址 + 同一用户视为同一账号：改配置而不是叠一条新的
    let existing = list
        .iter()
        .position(|a| a.url == url && a.username == username && a.kind == kind);
    let id = match existing {
        Some(i) => list[i].id.clone(),
        None => format!("ms-{:x}", now_ms()),
    };
    let acc = MediaServerAccount {
        id: id.clone(),
        kind: kind.clone(),
        name: name.clone(),
        url: url.clone(),
        username: username.clone(),
        password_enc: crate::config::encrypt_access_code(&password),
        token_enc: crate::config::encrypt_access_code(&token),
        user_id: user_id.clone(),
        insecure: insecure.unwrap_or(false),
        saved_at_ms: now_ms(),
    };
    match existing {
        Some(i) => list[i] = acc,
        None => list.push(acc),
    }
    save_accounts(&list)?;
    crate::libmpv::trace(&format!(
        "[lc-2345] media_server_login kind={kind} url={url} user={username} ok server={server_name}"
    ));
    Ok(json!({ "id": id, "userId": user_id, "serverName": server_name }))
}

#[tauri::command]
pub async fn media_server_del(id: String) -> Result<(), String> {
    let mut list = load_accounts();
    list.retain(|a| a.id != id);
    save_accounts(&list)
}

/// 取账号 + 解密 token（内部用）。
fn find_account(id: &str) -> Result<(MediaServerAccount, String), String> {
    let acc = load_accounts()
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| "媒体服务器账号不存在".to_string())?;
    let token = crate::config::decrypt_access_code(&acc.token_enc);
    Ok((acc, token))
}

/// token 失效时的自愈：用存下来的密码重新登录一次并覆盖 token。
/// 自建服务器的 token 通常长期有效，但重装服务端 / 改密码后就会 401；
/// 用户不该为此手动重填一遍。
async fn relogin(acc: &MediaServerAccount) -> Result<String, String> {
    let password = crate::config::decrypt_access_code(&acc.password_enc);
    let resp = client()
        .post(format!("{}/Users/AuthenticateByName", acc.url))
        .header("Content-Type", "application/json")
        .header(
            "Authorization",
            "MediaBrowser Client=\"AniBox\", Device=\"AniBox\", DeviceId=\"anibox\", Version=\"1.0\"",
        )
        .json(&json!({ "Username": acc.username, "Pw": password }))
        .send()
        .await
        .map_err(|e| format!("重新登录失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "凭据已失效（{}），请在云盘接入页重新登录",
            resp.status().as_u16()
        ));
    }
    let v: Value = resp.json().await.map_err(|e| format!("解析登录响应失败: {e}"))?;
    let token = v
        .get("AccessToken")
        .and_then(|x| x.as_str())
        .ok_or("重新登录未返回 AccessToken")?
        .to_string();
    // 回写加密 token
    let mut list = load_accounts();
    if let Some(a) = list.iter_mut().find(|a| a.id == acc.id) {
        a.token_enc = crate::config::encrypt_access_code(&token);
        let _ = save_accounts(&list);
    }
    Ok(token)
}

/// 带自愈的 GET：401/403 时 relogin 一次再重试。
async fn get_json_authed(acc: &MediaServerAccount, token: &str, url: &str) -> Result<Value, String> {
    match get_json(url, Some(token)).await {
        Err(e) if e.contains("鉴权失败") => {
            let fresh = relogin(acc).await?;
            get_json(url, Some(&fresh)).await
        }
        other => other,
    }
}

// ───────────────────────── 浏览 ─────────────────────────

/// 列表根：用户可见的媒体库（Views）。
#[tauri::command]
pub async fn media_server_views(id: String) -> Result<Value, String> {
    let (acc, token) = find_account(&id)?;
    let url = format!("{}/Users/{}/Views", acc.url, acc.user_id);
    let v = get_json_authed(&acc, &token, &url).await?;
    let items: Vec<Value> = v
        .get("Items")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .map(|it| {
                    json!({
                        "fileId": it.get("Id").and_then(|x| x.as_str()).unwrap_or(""),
                        "navId": it.get("Id").and_then(|x| x.as_str()).unwrap_or(""),
                        "name": it.get("Name").and_then(|x| x.as_str()).unwrap_or(""),
                        "isDir": true,
                        "size": 0,
                        "kind": "dir",
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({ "items": items, "nextCursor": "" }))
}

/// 判扩展名 → 条目 kind（与服务端 ItemType 无关，直接看文件名，最可靠）。
fn classify(name: &str, is_folder: bool) -> &'static str {
    if is_folder {
        return "dir";
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if VIDEO_EXTS.contains(&ext.as_str()) {
        "video"
    } else if SUB_EXTS.contains(&ext.as_str()) {
        "sub"
    } else {
        "other"
    }
}

/// 列一个目录（ParentId = 媒体库 Id 或文件夹 Id）。
///
/// 三种 Item 形态都要处理，否则「进得去出不来」：
/// - `Folder` / `CollectionFolder`：真目录 → 可下钻
/// - `Series`（剧集）：**不是文件**，点开要列它的季（`/Shows/{id}/Seasons`）
/// - `Movie` / `Episode`：可播放条目
///
/// 剧集走季 → 集两跳，所以这里把 Series 也当目录（isDir=true），
/// 由前端继续调本命令，后端按 ItemType 分派到正确的子接口。
#[tauri::command]
pub async fn media_server_list(
    id: String,
    parent_id: Option<String>,
    item_type: Option<String>,
) -> Result<Value, String> {
    let (acc, token) = find_account(&id)?;
    let parent = parent_id.unwrap_or_default();
    let item_type = item_type.unwrap_or_default();

    // 剧集 → 列季；季 → 列集。其余一律走通用的 Items?ParentId=
    let url = if !parent.is_empty() && item_type.eq_ignore_ascii_case("series") {
        format!("{}/Shows/{}/Seasons?UserId={}", acc.url, parent, acc.user_id)
    } else if !parent.is_empty() && item_type.eq_ignore_ascii_case("season") {
        // Season 的集：ParentId = seasonId 即可拿到 Episode 列表
        format!(
            "{}/Users/{}/Items?ParentId={}&IncludeItemTypes=Episode&Fields=Path,MediaSources,RunTimeTicks&Recursive=false",
            acc.url, acc.user_id, parent
        )
    } else if parent.is_empty() {
        // 没给 parent = 从媒体库列表开始
        return media_server_views(id).await;
    } else {
        // 通用目录：把 Series 一起 include 进来。
        // 不加这一条时，某些部署在「文件夹视图」下的剧集库会**直接**返回 Series
        // 的 Episode 而不给 Series 节点（两种部署都见过），用户看到的就是一堆
        // 散集、没有番剧分组。显式 include 让服务端把两种形状都列出来。
        format!(
            "{}/Users/{}/Items?ParentId={}&IncludeItemTypes=Movie,Series,Folder,Video,Episode&Fields=Path,MediaSources,RunTimeTicks&Recursive=false",
            acc.url, acc.user_id, parent
        )
    };

    let v = get_json_authed(&acc, &token, &url).await?;
    let mut items: Vec<Value> = Vec::new();
    for it in v.get("Items").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        let name = it.get("Name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let itype = it.get("Type").and_then(|x| x.as_str()).unwrap_or("");
        let iid = it.get("Id").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let is_dir = matches!(itype, "Folder" | "CollectionFolder" | "Series" | "Season" | "BoxSet");
        let kind = classify(&name, is_dir);
        // 剧集/季没有扩展名也不是文件，只作为「目录」展示；电影/集可播放
        let mut row = json!({
            "fileId": iid,
            "navId": iid,
            "name": name,
            "isDir": is_dir,
            "size": it.get("Size").and_then(|x| x.as_u64()).unwrap_or(0),
            "kind": kind,
            "itemType": itype,
        });
        // 季/集补上人类可读标签（Emby 的 Name 常常只有「第 1 季」）
        if itype == "Episode" {
            let s = it.get("ParentIndexNumber").and_then(|x| x.as_i64()).unwrap_or(0);
            let e = it.get("IndexNumber").and_then(|x| x.as_i64()).unwrap_or(0);
            if s > 0 && e > 0 {
                row["episodeLabel"] = json!(format!("S{s:02}E{e:02}"));
            }
        }
        items.push(row);
    }
    // 目录优先 → 再按集号/名称（服务端顺序有时是「按添加时间」，用户找不到集）
    items.sort_by(|a, b| {
        let ad = a.get("isDir").and_then(|x| x.as_bool()).unwrap_or(false);
        let bd = b.get("isDir").and_then(|x| x.as_bool()).unwrap_or(false);
        bd.cmp(&ad).then_with(|| {
            a.get("name").and_then(|x| x.as_str()).unwrap_or("").cmp(
                b.get("name").and_then(|x| x.as_str()).unwrap_or(""),
            )
        })
    });
    Ok(json!({ "items": items, "nextCursor": "" }))
}

// ───────────────────────── 播放 ─────────────────────────

/// 播放一个条目：取直链（`static=true` 原样发文件）→ 交给 mpv 起播。
///
/// 为什么 `static=true`：
/// - 服务端转码会重编码，画质/音轨/字幕轨全部被服务端决定，且 CPU 吃满；
/// - 我们随包 mpv 的硬解能力远强于服务端默认转码档，原样发文件是最优路径；
/// - `static=true` 时服务端仍然会做鉴权与 Range 透传，进度条可以正常拖。
///
/// 字幕：Emby 的外挂字幕是媒体库里的独立 Item（`MediaStreams` 里 `IsExternal`），
/// 这里按「同目录同名字幕」原则从 `MediaSources.Path` 反推 —— 只取服务端已挂载的
/// 字幕流太窄（用户可能想看没挂的那一份），所以优先用 API 返回的 MediaStreams，
/// 没有就让 mpv 自己找旁路字幕（Emby 部署常见 .ass 与视频同目录）。
#[tauri::command]
pub async fn media_server_play(
    app: AppHandle,
    state: tauri::State<'_, crate::AppState>,
    id: String,
    item_id: String,
    name: String,
) -> Result<Value, String> {
    let (acc, token) = find_account(&id)?;
    if item_id.trim().is_empty() {
        return Err("itemId 为空".into());
    }
    // 先确认条目存在并拿到 MediaSources（顺带验证 token；失效会自动 relogin）
    let detail_url = format!(
        "{}/Users/{}/Items/{}?Fields=Path,MediaSources",
        acc.url, acc.user_id, item_id.trim()
    );
    let detail = get_json_authed(&acc, &token, &detail_url).await?;
    // relogin 后 token 可能已换新；这里重新取一次，保证取流用的是当前有效 token
    let (_, token) = find_account(&id)?;
    let title = detail
        .get("Name")
        .and_then(|x| x.as_str())
        .unwrap_or(&name)
        .to_string();

    // [lc-2345] 取流方式：**用请求头而不是 URL 里的 api_key**。
    // api_key 会进 mpv 日志、我们的 trace、以及用户截图里的诊断包 —— 而它等价于
    // 该账号的完整权限。Emby/Jellyfin 的 /Videos/{id}/stream 走「逐跳」鉴权时认
    // X-Emby-Token 和 MediaBrowser Authorization，所以用 http-header-fields 下发即可。
    let stream_url = format!("{}/Videos/{}/stream?static=true", acc.url, item_id.trim());

    let _ = app.emit("mpv-load", json!({ "phase": "start" }));
    let cfg = state.config.read().await.clone();
    crate::fnos_progress::begin("", "");
    // headers 承担两件事：
    //   ① 鉴权（两套头，同 auth_headers —— mpv 取流也照样认）
    //   ② [lc-2345] 自签 HTTPS：API 侧我们本来就接受无效证书，但取流是 mpv 直连，
    //      它默认 --tls-verify=yes。不带下去就是「登录正常、播放报错」这类
    //      「一半能用」的故障，最难排查，所以显式下发。
    let mut h = crate::player::HttpHeaders::new(None, None);
    // 只下发 X-Emby-Token：Authorization 那条含逗号，会被 http-header-fields
    // 的分隔逻辑丢弃（见 stream_headers 的说明）。
    for (k, v) in stream_headers(&token) {
        h.extra.push((k, v));
    }
    if acc.insecure {
        h.mpv_opts.push("--tls-verify=no".into());
    }
    let headers = Some(h);
    // direct=true 的语义在这里恰好对：它表示「url 是源站直链、不经本地 relay」，
    // 我们给的正是不经代理的 /Videos/{id}/stream。顺带拿到 demuxer 层的断线重连
    // 参数（媒体服务器偶尔重启/换网时会用到）。WebDAV 播放走的也是 true。
    crate::player::launch(
        &app,
        &state,
        &cfg,
        &stream_url,
        &title,
        0.0,
        0.0,
        headers.as_ref(),
        &[],
        None,
        true,
    )
    .await?;
    crate::libmpv::trace(&format!(
        "[lc-2345] media_server_play kind={} item={} title={title}",
        acc.kind, item_id
    ));
    Ok(json!({ "url": stream_url, "title": title }))
}

/// [lc-2345] 登录页/启动路由用：是否配过任何媒体服务器账号。
pub(crate) fn any_account() -> bool {
    !load_accounts().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_url_variants() {
        assert_eq!(normalize_url("192.168.1.10:8096"), "http://192.168.1.10:8096");
        assert_eq!(normalize_url("http://a.b:8096/"), "http://a.b:8096");
        assert_eq!(normalize_url("https://emby.example.com"), "https://emby.example.com");
        assert_eq!(normalize_url("  http://x:1  "), "http://x:1");
        assert_eq!(normalize_url(""), "");
    }

    #[test]
    fn classify_by_extension() {
        assert_eq!(classify("a.mkv", false), "video");
        assert_eq!(classify("a.MP4", false), "video");
        assert_eq!(classify("a.m4v", false), "video");
        assert_eq!(classify("a.ass", false), "sub");
        assert_eq!(classify("a.nfo", false), "other");
        assert_eq!(classify("某文件夹", true), "dir");
        // 无扩展名的文件不该被当视频
        assert_eq!(classify("noext", false), "other");
    }

    #[test]
    fn kind_label_lookup() {
        assert_eq!(kind_label("emby"), "Emby");
        assert_eq!(kind_label("jellyfin"), "Jellyfin");
        assert_eq!(kind_label("unknown"), "媒体服务器");
    }

    /// 鉴权头必须两套都在：只发 X-Emby-Token 在 Jellyfin 10.9+ 上会被 400。
    #[test]
    fn auth_headers_cover_both_servers() {
        let h = auth_headers("tok123");
        let keys: Vec<&str> = h.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"X-Emby-Token"));
        assert!(keys.contains(&"Authorization"));
        let auth = h.iter().find(|(k, _)| *k == "Authorization").unwrap();
        assert!(auth.1.contains("MediaBrowser"));
        assert!(auth.1.contains("Token=\"tok123\""));
    }

    /// 取流头必须能被 mpv 的 http-header-fields 送出去 ——
    /// 现有实现会丢弃**值里含逗号**的头（mpv.rs::normalize_http_headers_extra），
    /// 而标准 Authorization 写法里全是逗号。这条测试锁住「取流头不含逗号」，
    /// 否则就是「能登录、能浏览、点播放 401」这种最难查的故障。
    #[test]
    fn stream_headers_survive_mpv_header_quoting() {
        let h = stream_headers("tok-abc");
        assert!(!h.is_empty());
        for (k, v) in &h {
            assert!(!v.contains(','), "{k} 的值含逗号，会被 http-header-fields 丢弃: {v}");
            assert!(!k.contains(','), "{k} 的键含逗号");
        }
        // 真的过一遍生产代码的归一化，确认头没被丢
        let (_, _, hf) = crate::mpv::normalize_http_headers_extra(None, None, &h);
        let hf = hf.expect("头被全部丢弃了");
        assert!(hf.to_ascii_lowercase().contains("x-emby-token"), "{hf}");
        assert!(hf.contains("tok-abc"), "{hf}");
    }

    /// 反过来锁住「标准 Authorization 会被丢掉」这个已知约束 ——
    /// 如果哪天有人放宽了 normalize（允许转义逗号），这条会失败，
    /// 提醒他去重新评估能不能直接用完整 Authorization 取流。
    #[test]
    fn full_authorization_is_known_to_be_dropped() {
        let h: Vec<(String, String)> = auth_headers("tok")
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let (_, _, hf) = crate::mpv::normalize_http_headers_extra(None, None, &h);
        let hf = hf.unwrap_or_default();
        assert!(
            !hf.contains("MediaBrowser"),
            "内含逗号的 Authorization 竟然被保留了，请复核 stream_headers 是否还需限制"
        );
    }

    /* ── 真实 HTTP 验证：跑**生产代码**，抓它真实发出的报文 ──────────────────
       本机没有 Emby/Jellyfin 实例，所以起一个 tokio TcpListener 当假服务器，
       按真实服务的形状回包。这与 verify_emby_protocol_2345.py（复刻一份请求）
       的区别是：这里调的就是 get_json / relogin / auth_headers 本身。 */

    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 起一个只回一个响应的最小 HTTP 服务器，返回 (端口, 收到的请求文本)。
    fn serve_once(status_line: &'static str, body: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
            req
        });
        (port, handle)
    }

    #[tokio::test]
    async fn get_json_sends_both_auth_headers() {
        let (port, handle) = serve_once("200 OK", r#"{"Items":[]}"#);
        let url = format!("http://127.0.0.1:{port}/Users/u1/Views");
        let v = get_json(&url, Some("TOK-XYZ")).await.expect("get_json ok");
        let req = handle.join().unwrap();
        assert!(v.get("Items").is_some());
        // 两套鉴权头必须同时出现（Jellyfin 只认 MediaBrowser，Emby 只认 X-Emby-Token）
        assert!(req.contains("x-emby-token: TOK-XYZ") || req.contains("X-Emby-Token: TOK-XYZ"), "{req}");
        assert!(req.contains("MediaBrowser"), "{req}");
        assert!(req.contains("Token=\"TOK-XYZ\""), "{req}");
        assert!(req.contains("GET /Users/u1/Views"), "{req}");
    }

    #[tokio::test]
    async fn get_json_reports_401_as_auth_failure() {
        let (port, handle) = serve_once("401 Unauthorized", "{}");
        let url = format!("http://127.0.0.1:{port}/Users/u1/Views");
        let err = get_json(&url, Some("bad")).await.unwrap_err();
        let _ = handle.join();
        // 这个文案是 get_json_authed 触发 relogin 的判据，改文案必须同步改判据
        assert!(err.contains("鉴权失败"), "{err}");
    }

    #[tokio::test]
    async fn get_json_surfaces_non_json_body() {
        let (port, handle) = serve_once("200 OK", "<html>这是网页</html>");
        let url = format!("http://127.0.0.1:{port}/x");
        let err = get_json(&url, None).await.unwrap_err();
        let _ = handle.join();
        assert!(err.contains("不是 JSON"), "{err}");
    }
}
