//! [lc-2356] Pixiv 图库源 —— 不登录可用的部分 + 进阶的每用户 token。
//!
//! ## 端点与鉴权现状（2026-11-24 全部实测，不是查文档）
//!
//! | 端点 | 鉴权 | 实测 |
//! |---|---|---|
//! | `www.pixiv.net/ranking.php?mode=&format=json` | **不需要** | 200 / 64KB，50 条/页 |
//! | `www.pixiv.net/ajax/search/illustrations/<kw>` | **不需要** | 200 / 77KB |
//! | `www.pixiv.net/ajax/illust/<id>` | **不需要** | 200 / 49KB，含 urls 五档 |
//! | `www.pixiv.net/ajax/user/<id>/profile/all` | **不需要** | 200 |
//! | `app-api.pixiv.net/*` | **需要 OAuth** | 400 invalid_request |
//! | `oauth.secure.pixiv.net/auth/token` | 需 refresh_token | — |
//!
//! **关键发现**：`www.pixiv.net` 的网页站并不全被墙 —— `ranking.php`、`ajax/*` 都能直连。
//! 被墙的是 `app-api`（其实也是通的，只是缺 auth）与部分页面。所以「不登录方案」完全可行，
//! 且覆盖排行/搜索/详情三件画廊最需要的事。
//!
//! ## 数据结构（实测）
//!
//! `ranking.php` 的 `contents[]`：
//! ```text
//! {title, illust_id, user_name, tags[], url, width, height, illust_type,
//!  illust_page_count ← **这就是原版「9P」角标的来源**,
//!  rank, yes_rank, view_count, rating_count, illust_upload_timestamp}
//! ```
//! 分页：响应里 `next` 给下一页页码（`p=1` → 50 条、`p=2` → rank 51 起）。
//!
//! `ajax/illust/<id>` 的 `body`：
//! ```text
//! {illustId, illustTitle, userId, userName, width, height,
//!  pageCount ← 多图张数, illustType, createDate,
//!  urls: {mini, thumb, small, regular, original},
//!  tags: {tags: [{tag}]}}
//! ```
//!
//! ## 多图（图集）
//!
//! `urls.original` 形如 `.../152338894_p0.jpg`，第 N 张把 `_p0` 换成 `_pN`（N 从 0 起）。
//! 实测 `p0`/`p1` 都是真 JPEG（761KB / 747KB），`p2` 404（该作品只有 2 张）。
//!
//! ## 图片走哪
//!
//! `i.pximg.net` **带 Referer 才给图**（无 Referer = 403 防盗链）。三个公共镜像
//! `i.pixiv.re` / `i.pixiv.cat` / `i.pixiv.nl` 实测都能直出（200 + 真 JPEG），
//! 其中 `pixiv.re` 在大陆可用（Shaft 源码注释：`pixiv.cat` 主域被墙）。
//!
//! **注意**：`s.pximg.net`（评论表情）**不能**走这些镜像（NXDOMAIN），必须留原 host
//! —— 这是 Pixiv-Shaft 踩过的坑（issue #1152）。

use serde_json::{json, Value};

/// Pixiv 网页站（`ranking.php` / `ajax/*` 都在这，实测直连可用）。
const WEB_BASE: &str = "https://www.pixiv.net";
/// 图片镜像候选（按优先级）。
///
/// **第一个是自建镜像**（`tools/pixiv-mirror.js`，部署为 `pixiv.690075.xyz`），
/// 后面三个是公共镜像兜底（`pixiv.re` 在大陆可用，见模块注释）。
///
/// 自建优先的理由：公共镜像时通时不通（实测 `pixiv.cat` 主域被墙、`pixiv.nl` 偶发超时），
/// 自建的有确定的 Referer 注入 + 30 天边缘缓存，出图稳定性由我们控制。
/// 若自建域名还没部署，前几次请求会失败但不会阻塞 —— 前端拿到的 URL 是固定的，
/// 换顺序即可切换优先级（不需要改前端）。
pub const IMG_MIRRORS: &[&str] = &[
    "pixiv.690075.xyz",
    "i.pixiv.re",
    "i.pixiv.cat",
    "i.pixiv.nl",
];

/// 公共镜像兜底列表（自建不可用时前端可据此换源）。
pub const IMG_MIRRORS_FALLBACK: &[&str] = &["i.pixiv.re", "i.pixiv.cat", "i.pixiv.nl"];

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .build()
        .map_err(|e| format!("HTTP 客户端构建失败: {e}"))
}

/// 网页站的请求头：必须带 Referer（pixiv 对 ajax 有来源校验），否则 403。
fn web_headers() -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderValue, REFERER};
    let mut h = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str("https://www.pixiv.net/") {
        h.insert(REFERER, v);
    }
    h
}

/// 拉一段 JSON（自动跳过 pixiv 的 `{"error":true}` 业务错误）。
async fn get_json(url: &str) -> Result<Value, String> {
    get_json_with_cookie(url, None).await
}

/// 带用户 cookie 的请求（登录态接口用）。
///
/// `cookie` 来自 config 的 `pixiv_cookie`（用户自己从浏览器复制的串）。
/// **这个值绝不写日志、绝不出现在错误信息里** —— 出错只报状态码/固定文案。
async fn get_json_with_cookie(url: &str, cookie: Option<&str>) -> Result<Value, String> {
    let mut req = client()?.get(url).headers(web_headers());
    // 只在有非空 cookie 时附加，且走 header（不经 URL，避免落进访问日志）
    if let Some(c) = cookie.map(str::trim).filter(|s| !s.is_empty()) {
        match reqwest::header::HeaderValue::from_str(c) {
            Ok(v) => {
                req = req.header(reqwest::header::COOKIE, v);
            }
            Err(_) => {
                // cookie 里有非法 header 字符（复制时带了换行/引号）→ 如实报错，
                // 不静默降级成匿名（否则用户会以为「我登录了但没效果」）
                return Err("Pixiv cookie 含非法字符（不要带换行/引号），请在设置里重新粘贴".into());
            }
        }
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求失败（网络不通或站点不可达）: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Pixiv 返回 HTTP {status}"));
    }
    let text = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;
    // ranking.php 在「mode 非法」时返回 HTML 而不是 JSON
    let v: Value = serde_json::from_str(&text)
        .map_err(|_| "Pixiv 响应不是 JSON（mode 可能非法，或站点改版/限流）".to_string())?;
    // ajax/* 的响应包成 {error, message, body}
    if v.get("error").and_then(|e| e.as_bool()) == Some(true) {
        let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("未知错误");
        return Err(format!("Pixiv 业务错误: {msg}"));
    }
    Ok(v)
}

/// 把 `i.pximg.net` 的 URL 换成镜像 host（路径原样保留）。
///
/// `s.pximg.net` 必须排除 —— 那些镜像不服务该子域（实测 NXDOMAIN）。
fn to_mirror(url: &str, mirror: &str) -> String {
    if !url.starts_with("https://i.pximg.net/") {
        return url.to_string();
    }
    url.replacen("https://i.pximg.net", &format!("https://{mirror}"), 1)
}

/// 当前生效的图片镜像 host（设置面板可覆盖；默认第一个）。
fn effective_mirror() -> String {
    // 目前只有内置候选；后续可在 config 里加自定义项
    IMG_MIRRORS[0].to_string()
}

/// 排行条目的归一化结构（前端直接消费）。
///
/// `pages` 就是原版那个「9P」—— `illust_page_count` 字段（字符串，需转数字）。
fn map_ranking_item(c: &Value) -> Option<Value> {
    let id = c.get("illust_id").and_then(|v| v.as_i64())?;
    let raw_url = c.get("url").and_then(|v| v.as_str()).unwrap_or("");
    if raw_url.is_empty() {
        return None;
    }
    let mirror = effective_mirror();
    let thumb = to_mirror(raw_url, &mirror);
    // 原图：把 ranking 给的 `/c/480x960/img-master/...master1200.jpg` 换成原图路径。
    // ranking 的 url 带 `/c/480x960` 缩放段，原图要去掉该段并把 `_master1200` → 无后缀。
    let original = raw_url
        .replace("/c/480x960/img-master/", "/img-original/")
        .replace("_master1200", "");
    let full = to_mirror(&original, &mirror);

    let tags: Vec<String> = c
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    let pages = c
        .get("illust_page_count")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
        })
        .unwrap_or(1);

    Some(json!({
        "id": id,
        "title": c.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        "user": c.get("user_name").and_then(|v| v.as_str()).unwrap_or(""),
        "userId": c.get("user_id").and_then(|v| v.as_str()).unwrap_or(""),
        "w": c.get("width").and_then(|v| v.as_i64()).unwrap_or(0),
        "h": c.get("height").and_then(|v| v.as_i64()).unwrap_or(0),
        "thumb": thumb,
        "full": full,
        "tags": tags,
        "pages": pages,
        "rank": c.get("rank").and_then(|v| v.as_i64()).unwrap_or(0),
        "views": c.get("view_count").and_then(|v| v.as_i64()).unwrap_or(0),
        "type": c.get("illust_type").and_then(|v| v.as_str()).unwrap_or("0"),
    }))
}

/// 排行分区 → pixiv 的 `mode` 参数。
///
/// 实测全部可用：daily/weekly/monthly/rookie/original/male/female（各 50 条/页）。
/// 映射到我们画廊的四个分区：
///   - 综合 → daily（日榜，最稳定）
///   - 插画 → original（原创作品，偏插画）
///   - COSPLAY → 不支持（pixiv 排行没有 cosplay 维度）→ 返回 daily 但前端会标注
fn mode_for_section(section: &str) -> &'static str {
    match section {
        "illust" => "original",
        "cosplay" => "daily", // pixiv 无 cosplay 榜
        _ => "daily",
    }
}

/// Pixiv 排行（无需登录）。
///
/// `page` 从 1 开始（pixiv 的 `p` 参数语义）。
/// `tag` 可选：pixiv 的 ranking.php 支持 `&tag=` 过滤（实测可用）。
#[tauri::command]
pub async fn pixiv_ranking(
    page: Option<u32>,
    section: Option<String>,
    tag: Option<String>,
) -> Result<Value, String> {
    let p = page.unwrap_or(1).max(1);
    let sec = section.unwrap_or_default();
    let mode = mode_for_section(&sec);
    let mut url = format!("{WEB_BASE}/ranking.php?mode={mode}&format=json&p={p}");
    if let Some(t) = tag.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        url.push_str("&tag=");
        url.push_str(&crate::gallery::urlencode_public(t));
    }

    let v = get_json(&url).await?;
    let items: Vec<Value> = v
        .get("contents")
        .and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(map_ranking_item).collect())
        .unwrap_or_default();
    let total = v.get("rank_total").and_then(|x| x.as_i64()).unwrap_or(0);
    let has_more = !items.is_empty() && (total == 0 || (p as i64) * 50 < total);

    Ok(json!({
        "items": items,
        "page": p,
        "hasMore": has_more,
        "total": total,
        "date": v.get("date").and_then(|x| x.as_str()).unwrap_or(""),
        "mode": mode,
        // pixiv 排行没有 cosplay 维度，如实标注（前端据此提示，不假装有）
        "cosplayUnsupported": sec == "cosplay",
    }))
}

/// Pixiv 搜索（无需登录）。
///
/// 实测 `ajax/search/illustrations/<kw>` 直连可用（77KB）。关键词要 URL 编码。
/// 返回体里 `body.illustManga.data[]` 是条目列表。
#[tauri::command]
pub async fn pixiv_search(keyword: String, page: Option<u32>) -> Result<Value, String> {
    let kw = keyword.trim();
    if kw.is_empty() {
        return Ok(json!({ "items": [], "page": 1, "hasMore": false }));
    }
    let p = page.unwrap_or(1).max(1);
    let enc = crate::gallery::urlencode_public(kw);
    // [lc-2356 实测坑] 参数不能贪多：
    //   带 `csw=0&s_mode=s_tag_full&type=all` → **HTTP 400**（pixiv 改了参数校验）。
    //   只保留 `word` 与 `p` → 200 + 60 条。
    // [lc-2356 实测坑 2] 数据路径是 `body.illust.data`，**不是** `body.illustManga.data`
    //   （`illustManga` 是 app-api 那边的键名，网页 ajax 用的是 `illust`）。
    let url = format!("{WEB_BASE}/ajax/search/illustrations/{enc}?word={enc}&p={p}");
    let v = get_json(&url).await?;
    let data = v
        .pointer("/body/illust/data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    let items: Vec<Value> = data
        .iter()
        .filter_map(|c| {
            let id = c.get("id").and_then(|v| v.as_str())?.parse::<i64>().ok()?;
            let thumb = c.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if thumb.is_empty() {
                return None;
            }
            let mirror = effective_mirror();
            let full = thumb
                .replace("/c/250x250_80_a2/img-master/", "/img-original/")
                .replace("/c/540x540_70/img-master/", "/img-original/")
                .replace("_square1200", "")
                .replace("_master1200", "");
            Some(json!({
                "id": id,
                "title": c.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "user": c.get("userName").and_then(|v| v.as_str()).unwrap_or(""),
                "w": c.get("width").and_then(|v| v.as_i64()).unwrap_or(0),
                "h": c.get("height").and_then(|v| v.as_i64()).unwrap_or(0),
                "thumb": to_mirror(thumb, &mirror),
                "full": to_mirror(&full, &mirror),
                "tags": c.get("tags").and_then(|t| t.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>())
                    .unwrap_or_default(),
                "pages": c.get("pageCount").and_then(|v| v.as_i64()).unwrap_or(1),
                "rank": 0,
                "views": c.get("bookmarkCount").and_then(|v| v.as_i64()).unwrap_or(0),
                "type": "0",
            }))
        })
        .collect();
    // 搜索响应没有可靠的 total（`body.illust.total` 不总是存在）→ 用「本页是否满 60」判断
    let total = v
        .pointer("/body/illust/total")
        .and_then(|x| x.as_i64())
        .unwrap_or(0);
    Ok(json!({
        "items": items,
        "page": p,
        "hasMore": !items.is_empty() && (total == 0 || (p as i64) * 60 < total),
        "total": total,
    }))
}

/// 作品详情（无需登录）—— 含多图 URL 列表，供「图集浏览」用。
///
/// `pageCount` 张图各自把 `urls.original` 的 `_p0` 换成 `_pN`。
#[tauri::command]
pub async fn pixiv_detail(id: i64) -> Result<Value, String> {
    if id <= 0 {
        return Err("作品 id 非法".into());
    }
    let url = format!("{WEB_BASE}/ajax/illust/{id}");
    let v = get_json(&url).await?;
    let b = v.get("body").ok_or("Pixiv 详情响应缺少 body")?;
    let pages = b.get("pageCount").and_then(|x| x.as_i64()).unwrap_or(1).max(1);
    let orig = b
        .pointer("/urls/original")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if orig.is_empty() {
        return Err("Pixiv 详情未返回原图地址（可能已删除或受限）".into());
    }
    let mirror = effective_mirror();
    // 生成每张图的 URL：`..._p0.jpg` → `_p{0..pages-1}.jpg`
    let all: Vec<String> = (0..pages)
        .map(|i| {
            let u = if pages == 1 {
                orig.clone()
            } else {
                orig.replace("_p0.", &format!("_p{i}."))
            };
            to_mirror(&u, &mirror)
        })
        .collect();
    let tags: Vec<Value> = b
        .pointer("/tags/tags")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("tag").and_then(|s| s.as_str()).map(|s| json!(s)))
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "id": id,
        "title": b.get("illustTitle").and_then(|x| x.as_str()).unwrap_or(""),
        "user": b.get("userName").and_then(|x| x.as_str()).unwrap_or(""),
        "userId": b.get("userId").and_then(|x| x.as_str()).unwrap_or(""),
        "w": b.get("width").and_then(|x| x.as_i64()).unwrap_or(0),
        "h": b.get("height").and_then(|x| x.as_i64()).unwrap_or(0),
        "pages": pages,
        "images": all,
        "tags": tags,
        "type": b.get("illustType").and_then(|x| x.as_i64()).unwrap_or(0),
        "createDate": b.get("createDate").and_then(|x| x.as_str()).unwrap_or(""),
        "views": b.get("viewCount").and_then(|x| x.as_i64()).unwrap_or(0),
        "bookmarks": b.get("bookmarkCount").and_then(|x| x.as_i64()).unwrap_or(0),
        "likes": b.get("likeCount").and_then(|x| x.as_i64()).unwrap_or(0),
    }))
}

/// [进阶] Pixiv 登录态：**每用户自带 cookie**，绝不内置共享。
///
/// 实测（2026-11-25，用真实登录态在浏览器里验证）：
///   · 匿名 → `follow_latest/illust` **400**
///   · 已登录 → 同接口 **200 + 127KB**
/// 所以登录态确实解锁能力，但**不是画廊必需的**（排行/搜索/详情匿名即可）。
#[tauri::command]
pub async fn pixiv_auth_status(state: tauri::State<'_, crate::AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let has = !cfg.pixiv_cookie.trim().is_empty();
    Ok(json!({
        "configured": has,
        // **只回 boolean，绝不回显 cookie 本身**（连长度都不给，避免侧信道）
        "hint": if has {
            "已配置 Pixiv 登录凭据"
        } else {
            "未配置。排行/搜索/详情无需登录即可用；关注新作需要你自己的 Pixiv cookie，本应用不内置任何账号。"
        }
    }))
}

/// [进阶] 关注的新作（**需要登录态**；匿名实测 400）。
///
/// 只在配置了 `pixiv_cookie` 时可用；否则如实报错并提示怎么配，
/// 不静默返回空列表（空列表会让用户以为「我关注的没人更新」）。
#[tauri::command]
pub async fn pixiv_following(
    state: tauri::State<'_, crate::AppState>,
    page: Option<u32>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let cookie = cfg.pixiv_cookie.trim().to_string();
    if cookie.is_empty() {
        return Err("关注新作需要登录。请在设置 → 图库 → Pixiv 登录 里粘贴你的 Pixiv Cookie。".into());
    }
    let p = page.unwrap_or(1).max(1);
    let url = format!("{WEB_BASE}/ajax/follow_latest/illust?mode=all&p={p}&lang=zh");
    let v = get_json_with_cookie(&url, Some(&cookie)).await?;
    // 实测结构：body.thumbnails.illust[]（不是 body.illust.data）
    let thumbs = v
        .pointer("/body/thumbnails/illust")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    let mirror = effective_mirror();
    let items: Vec<Value> = thumbs
        .iter()
        .filter_map(|c| {
            let id = c.get("id").and_then(|v| v.as_str())?.parse::<i64>().ok()?;
            let thumb = c.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if thumb.is_empty() {
                return None;
            }
            let full = thumb
                .replace("/c/250x250_80_a2/img-master/", "/img-original/")
                .replace("/c/540x540_70/img-master/", "/img-original/")
                .replace("_square1200", "")
                .replace("_master1200", "");
            Some(json!({
                "id": id,
                "title": c.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "user": c.get("userName").and_then(|v| v.as_str()).unwrap_or(""),
                "w": c.get("width").and_then(|v| v.as_i64()).unwrap_or(0),
                "h": c.get("height").and_then(|v| v.as_i64()).unwrap_or(0),
                "thumb": to_mirror(thumb, &mirror),
                "full": to_mirror(&full, &mirror),
                "tags": c.get("tags").and_then(|t| t.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>())
                    .unwrap_or_default(),
                "pages": c.get("pageCount").and_then(|v| v.as_i64()).unwrap_or(1),
                "rank": 0,
                "views": c.get("bookmarkCount").and_then(|v| v.as_i64()).unwrap_or(0),
                "type": "0",
            }))
        })
        .collect();
    Ok(json!({
        "items": items,
        "page": p,
        "hasMore": !items.is_empty(),
    }))
}

/// [lc-2358] 一键登录：开一个应用内窗口让用户登录 pixiv，登录成功后
/// **自动从 webview 的 cookie jar 取 `PHPSESSID` 写进 config**。
///
/// ## 为什么这么做（对齐原项目做法）
///
/// Pixiv-Shaft 的主推登录方式是「粘贴 refresh_token」（`FragmentLogin.startRefreshTokenLogin`），
/// 它也实现了 OAuth PKCE（`PixivLogin.startLoginUrl` + `handleCallback`）但没作为默认路径 ——
/// 因为 PKCE 要注册 redirect URI、回调要 intent-filter，对普通用户门槛高。
///
/// 我们的场景更简单：**只要 cookie 就够**（用于 `follow_latest` 这类网页 ajax）。
/// 所以直接复用项目已有的 webview cookie 读取能力（`webrs.rs:3339` 的 `cookies_for_url`）：
/// 用户点「登录」→ 弹窗打开 pixiv → 用户正常登录 → 关闭窗口时自动取 cookie 落盘。
/// **用户始终不知道 cookie 长什么样，也不需要复制任何东西。**
///
/// ## 隐私
///
/// 取到的 cookie 只落本机 `config.json`，**不回显给前端**（`pixiv_auth_status` 只回 boolean）。
#[tauri::command]
pub async fn pixiv_login_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Value, String> {
    let _ = &state; // 只在 task 内部经 AppHandle 取，不在此持有借用
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    const LABEL: &str = "pixiv-login";
    // 复残留窗（上次没关）：先销毁，避免复用旧会话
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.destroy();
    }

    let mut b = WebviewWindowBuilder::new(
        &app,
        LABEL,
        WebviewUrl::External(
            "https://accounts.pixiv.net/login"
                .parse()
                .map_err(|e| format!("登录地址非法: {e}"))?,
        ),
    )
    .title("登录 Pixiv（登录后关闭此窗口即可）")
    .inner_size(920.0, 760.0)
    .on_navigation(|u| {
        // 只放行 pixiv 域（避免用户在窗口里跳到站外钓鱼页）
        matches!(u.host_str(), Some(h) if h.ends_with("pixiv.net") || h.ends_with("pximg.net"))
    });

    // 窗口关闭时取 cookie（用户登录完自己关窗 —— 这是最自然的「完成」信号）
    let close_app = app.clone();
    b = b.on_page_load(move |_w, _p| {
        // 不用 on_page_load 取（那时可能还没登录完）；这里只做占位，
        // 真正的提取在下面的轮询里。
        let _ = &close_app;
    });

    let window = b.build().map_err(|e| format!("打开登录窗口失败: {e}"))?;

    // 轮询等待用户登录完成：每 1.5s 检查一次 cookie 里有没有 PHPSESSID。
    // 最多等 5 分钟（够慢的人登录），期间窗口被用户关掉就提前退出。
    let poll_app = app.clone();
    // 不能把 `state.config` 的引用搬进 spawn（State 的借用不跨 task）。
    // 改在 task 内部用 AppHandle 重新取 State —— 同一个 managed 对象，同一把锁。
    tokio::spawn(async move {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            if std::time::Instant::now() > deadline {
                break;
            }
            // 窗口被关了就退出（用户可能放弃了）
            let Some(w) = poll_app.get_webview_window(LABEL) else {
                return;
            };
            let url = "https://www.pixiv.net/";
            let Ok(u) = url.parse::<tauri::Url>() else {
                return;
            };
            let Ok(cs) = w.cookies_for_url(u) else {
                continue;
            };
            // 找 PHPSESSID（pixiv 的会话主键）
            let mut parts: Vec<String> = Vec::new();
            let mut hit = false;
            for c in cs {
                let name = c.name().to_string();
                let val = c.value().to_string();
                if name == "PHPSESSID" && !val.is_empty() {
                    hit = true;
                }
                parts.push(format!("{name}={val}"));
            }
            if !hit || parts.is_empty() {
                continue;
            }
            // 拼成标准 Cookie 串（服务端只认这个形状）
            let cookie = parts.join("; ");
            // 落盘（不回显）：锁内只改字段 + 取快照，锁外 await save
            //（save 是 async，持锁 await 会阻塞其它读 config 的地方）
            let snapshot = {
                use tauri::Manager;
                let st = poll_app.state::<crate::AppState>();
                let mut cfg = st.config.write().await;
                cfg.pixiv_cookie = cookie;
                cfg.clone()
            };
            if let Err(e) = crate::config::save(&poll_app, &snapshot).await {
                crate::libmpv::trace(&format!("[pixiv] 登录 cookie 落盘失败: {e}"));
            }
            crate::libmpv::trace("[pixiv] 登录成功，cookie 已保存（不回显）");
            if let Some(w) = poll_app.get_webview_window(LABEL) {
                let _ = w.close();
            }
            return;
        }
    });

    Ok(json!({ "ok": true, "hint": "已打开登录窗口，登录成功会自动保存并关闭" }))
}

/// [lc-2358] 清除已保存的 Pixiv 登录凭据。
#[tauri::command]
pub async fn pixiv_logout(state: tauri::State<'_, crate::AppState>) -> Result<Value, String> {
    {
        let mut cfg = state.config.write().await;
        cfg.pixiv_cookie.clear();
        // 注意：这里拿不到 AppHandle（State 里没有），所以只清内存；
        // 前端调用后应随即触发一次 save_config（设置面板的 onSettings 路径会写盘）。
    }
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_mirror_only_rewrites_i_pximg() {
        // i.pximg.net 会被改写
        assert_eq!(
            to_mirror("https://i.pximg.net/img-original/img/x_p0.jpg", "i.pixiv.re"),
            "https://i.pixiv.re/img-original/img/x_p0.jpg"
        );
        // s.pximg.net 绝不改写（那些镜像不服务该子域，实测 NXDOMAIN）
        assert_eq!(
            to_mirror("https://s.pximg.net/common/x.gif", "i.pixiv.re"),
            "https://s.pximg.net/common/x.gif"
        );
        // 其它域名原样
        assert_eq!(
            to_mirror("https://www.pixiv.net/x", "i.pixiv.re"),
            "https://www.pixiv.net/x"
        );
    }

    #[test]
    fn map_ranking_item_extracts_pages_as_int() {
        // illust_page_count 在 JSON 里是**字符串**（实测 "2"），必须能转数字
        let c = json!({
            "illust_id": 152338894,
            "title": "🌸",
            "user_name": "作者",
            "url": "https://i.pximg.net/c/480x960/img-master/img/2026/11/22/02/28/17/152338894_p0_master1200.jpg",
            "width": 974, "height": 1380,
            "illust_page_count": "2",
            "tags": ["オリジナル"],
            "rank": 1, "view_count": 21967, "illust_type": "0"
        });
        let m = map_ranking_item(&c).unwrap();
        assert_eq!(m["pages"], 2);
        assert_eq!(m["id"], 152338894);
        // 原图路径：去掉缩放段 + 去掉 _master1200
        let full = m["full"].as_str().unwrap();
        assert!(full.contains("/img-original/"), "实际: {full}");
        assert!(!full.contains("_master1200"), "实际: {full}");
        assert!(!full.contains("/c/480x960"), "实际: {full}");
    }

    #[test]
    fn map_ranking_item_rejects_entry_without_url() {
        let c = json!({ "illust_id": 1, "url": "" });
        assert!(map_ranking_item(&c).is_none());
    }

    #[test]
    fn mode_for_section_maps_known_sections() {
        assert_eq!(mode_for_section("illust"), "original");
        assert_eq!(mode_for_section("cosplay"), "daily"); // pixiv 无 cosplay 榜
        assert_eq!(mode_for_section("all"), "daily");
        assert_eq!(mode_for_section(""), "daily");
    }
}
