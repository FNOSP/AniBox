//! [lc-2417] 图片偏好（艺术图）的 Tauri 命令层。
//!
//! 对应 WWPlayer 的 `electron/preload.cjs` 暴露的
//!   · `window.wwPlayer.readDetailArtwork(owner)`  → `read_detail_artwork`
//!   · `window.wwPlayer.setDetailArtwork({owner,role,src})` → `set_detail_artwork`
//!
//! 存储细节见 `detail_artwork.rs`（照源实现复刻）。
//! 这里只管：路径解析、取图（复用本项目 `/__tmdb/` 代理链路）、返回给前端的 URL。

use std::path::PathBuf;
use tauri::Manager;

use crate::detail_artwork;

/// 本地偏好图目录：`app_data_dir/detail-artwork/<sha256(owner)>/<role>.webp`
/// （源实现：`path.join(directory, id, role.webp)`）。
fn store_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("取应用数据目录失败: {e}"))?;
    Ok(base.join("detail-artwork"))
}

/// 读取已设偏好。返回 `{ role: 绝对路径 }`（不含未设置的项）。
/// 前端拿到路径后再 `convertFileSrc` 换成可用 URL。
#[tauri::command]
pub async fn read_detail_artwork(
    app: tauri::AppHandle,
    owner: String,
) -> Result<serde_json::Value, String> {
    let dir = store_dir(&app)?;
    let picks = detail_artwork::read_picks(&dir, &owner)?;
    let mut map = serde_json::Map::new();
    for (role, path) in picks {
        map.insert(
            role,
            serde_json::Value::String(path.to_string_lossy().to_string()),
        );
    }
    Ok(serde_json::Value::Object(map))
}

/// 设置 / 清除偏好。
///
/// 参数与源实现完全一致：
///   · `src == None` → **恢复默认**（删除该角色的本地文件）
///   · `src == Some(url)` → 下载 → 转码 3840/quality94 webp → 原子写
///
/// 返回新文件的绝对路径（清除时返回空串），与源实现 `return await localUrl(...)` / `return ""` 对齐。
#[tauri::command]
pub async fn set_detail_artwork(
    app: tauri::AppHandle,
    owner: String,
    role: String,
    src: Option<String>,
) -> Result<String, String> {
    let dir = store_dir(&app)?;
    if !detail_artwork::is_valid_role(&role) {
        return Err("无效的艺术图分类".to_string());
    }
    let Some(src) = src else {
        detail_artwork::clear_pick(&dir, &owner, &role)?;
        return Ok(String::new());
    };
    let url = resolve_source(src.trim())?;

    let (_, bytes) = fetch_image_bytes(&url).await?;
    let path = tokio::task::spawn_blocking(move || {
        detail_artwork::save_pick(&dir, &owner, &role, &bytes)
    })
    .await
    .map_err(|e| format!("转码任务失败: {e}"))??;
    Ok(path.to_string_lossy().to_string())
}

/// [lc-2418] 把前端给的 `src` 解析成**后端可直接下载的 URL**。
///
/// 起因（用户报障：「设置偏好设不上 / 不支持的艺术图来源」）：
/// 前端传的是 `tmdbImg(p, "original")` 的结果，而它在本项目里是
///   · Windows: `http://fntvimg.localhost/__tmdb/t/p/original/abc.jpg`
///   · 其它:    `fntvimg://localhost/__tmdb/t/p/original/abc.jpg`
/// 这是**给 WebView 用的自定义协议**（由 `imgproto::handle` 拦截），
/// reqwest 根本没有这个主机名 —— 于是被下面的 scheme 白名单挡掉。
/// 即使放行也下载不到：那不是真实可解析的域名。
///
/// 正确做法：前端传 TMDB 的 file_path（`/abc.jpg`），后端自己拼镜像地址。
/// 兼容三种输入，便于前端渐进迁移与手工调试：
///   1. TMDB file_path：`/abc.jpg`          → `{TMDB_IMG_BASE}/img/t/p/original/abc.jpg`
///   2. 本项目的代理路径：`/__tmdb/t/p/w780/abc.jpg` → 去掉前缀后同上
///   3. 真实 http(s) URL：原样使用（指向外部图床时）
pub fn resolve_source(src: &str) -> Result<String, String> {
    let s = src.trim();
    if s.is_empty() {
        return Err("艺术图地址为空".to_string());
    }
    /* ⓪ WebView 内部协议必须先于所有分支判掉 —— 它有两种形态，
       其中 `http://fntvimg.localhost/...` 会被下面的 http 分支"看起来合法"地接收，
       然后 reqwest 解析失败（那个主机名只有 WebView 认识）。
       错误信息要给**可操作**的指引，而不是笼统的"不支持"（本次报障的核心痛点）。 */
    if s.starts_with("fntvimg://") || s.contains("fntvimg.localhost") {
        return Err(
            "不支持的艺术图来源：这是应用内部的图片协议地址（只有界面能读，后台下载不到），\
             保存时请传 TMDB 的 file_path（形如 /abc.jpg）"
                .to_string(),
        );
    }
    // ③ 真实 http(s)：原样用
    if s.starts_with("https://") || s.starts_with("http://") {
        return Ok(s.to_string());
    }
    /* 其他带协议头的（data: / file: / ww-image: …）一律拒绝：
       它们既不是真实可下载的 URL，也不该被当成路径 —— 否则
       `data:image/png;base64,...` 会被拼成 `.../img/t/p/original/data:image/...` 这种垃圾。 */
    if s.contains(':') && !s.starts_with('/') {
        return Err(format!("不支持的艺术图来源：{src}"));
    }
    // ①② 路径形态 → 拼镜像
    let mut p: String = s.to_string();
    if let Some(rest) = p.strip_prefix("/__tmdb") {
        p = rest.to_string(); // `/__tmdb/t/p/w780/x.jpg` → `/t/p/w780/x.jpg`
    }
    if !p.starts_with('/') {
        p = format!("/{p}"); // 容错：`abc.jpg` → `/abc.jpg`
    }
    // 已经是 `t/p/<size>/...` 形态就不再套一层
    let tail = if p.starts_with("/t/p/") {
        p
    } else {
        format!("/t/p/original{p}")
    };
    Ok(format!("{}/img{tail}", crate::imgproto::TMDB_IMG_BASE))
}

/// 下载图片字节。复用与 `/__tmdb/` 相同的上游抓取实现（超时/UA/重试口径一致），
/// 只是不经磁盘缓存 —— 这张图要立刻转码落成本地 webp，缓存原图没有意义。
async fn fetch_image_bytes(url: &str) -> Result<(String, Vec<u8>), String> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let c = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("AniBox/0.1")
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    });
    let resp = c
        .get(url)
        .header("accept", "image/*,*/*")
        .send()
        .await
        .map_err(|e| format!("艺术图下载失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("艺术图下载失败 (HTTP {})", status.as_u16()));
    }
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("艺术图读取失败: {e}"))?
        .to_vec();
    if bytes.is_empty() {
        return Err("艺术图内容为空".to_string());
    }
    Ok((ct, bytes))
}

#[cfg(test)]
mod tests {
    use super::resolve_source;

    /// [lc-2418] 用户报障「设置偏好设不上 / 不支持的艺术图来源」的回归测试。
    ///
    /// 根因：前端曾传 `tmdbImg(p,"original")` 的结果，那是 **WebView 内部协议**
    /// （`http://fntvimg.localhost/...` / `fntvimg://localhost/...`），
    /// reqwest 解析不了 → 被 scheme 白名单挡掉。
    /// 正确输入是 TMDB file_path（`/abc.jpg`），由后端拼镜像地址。
    #[test]
    fn resolve_source_accepts_tmdb_file_path() {
        // ① 纯 TMDB file_path（前端现在传的形态）
        assert_eq!(
            resolve_source("/abc123.jpg").unwrap(),
            "https://tmdb.690075.xyz/img/t/p/original/abc123.jpg"
        );
        // ② 已经是 t/p/<size> 形态：不再套一层
        assert_eq!(
            resolve_source("/t/p/w780/abc.jpg").unwrap(),
            "https://tmdb.690075.xyz/img/t/p/w780/abc.jpg"
        );
        // ③ 本项目代理路径：去掉 /__tmdb 前缀后处理
        assert_eq!(
            resolve_source("/__tmdb/t/p/w780/abc.jpg").unwrap(),
            "https://tmdb.690075.xyz/img/t/p/w780/abc.jpg"
        );
        // ④ 无前导斜杠也接受（容错）
        assert_eq!(
            resolve_source("abc.jpg").unwrap(),
            "https://tmdb.690075.xyz/img/t/p/original/abc.jpg"
        );
    }

    /// WebView 内部协议地址必须给出**可操作**的报错，而不是笼统的"不支持"。
    /// 这是本次 bug 的核心：用户看到的「不支持的艺术图来源」无法定位问题。
    #[test]
    fn resolve_source_rejects_webview_protocol_with_hint() {
        for bad in [
            "http://fntvimg.localhost/__tmdb/t/p/original/a.jpg",
            "fntvimg://localhost/__tmdb/t/p/original/a.jpg",
        ] {
            let err = resolve_source(bad).unwrap_err();
            // 报错必须给出**可操作**的指引，而不是笼统的"不支持"
            assert!(
                err.contains("内部") || err.contains("WebView"),
                "报错要说明原因：{err}"
            );
            assert!(err.contains("file_path"), "报错要说怎么办：{err}");
        }
    }

    /// 真实外部图床 URL 仍然放行（未来接别的来源时不至于又要改这里）
    #[test]
    fn resolve_source_passes_through_real_http() {
        assert_eq!(
            resolve_source("https://image.tmdb.org/t/p/original/a.jpg").unwrap(),
            "https://image.tmdb.org/t/p/original/a.jpg"
        );
        assert_eq!(
            resolve_source("http://127.0.0.1:8791/a.jpg").unwrap(),
            "http://127.0.0.1:8791/a.jpg"
        );
    }

    #[test]
    fn resolve_source_rejects_empty_and_unknown() {
        assert!(resolve_source("").is_err());
        assert!(resolve_source("   ").is_err());
        // 相对路径之外的东西（无协议、无前导斜杠）
        assert!(resolve_source("data:image/png;base64,AAA").is_err());
        assert!(resolve_source("file:///c:/x.png").is_err());
    }
}

// ───────────────────── `artwork` 自定义协议（供前端 <img src> 直读本地 webp） ─────────────────────
//
// 源实现用 `ww-image://preferred-artwork/<id>/<role>?v=mtimeNs`；
// 这里用 `artwork://preferred/<id>/<role>?v=<mtime>`，语义一致：
// 一个只读、只认「已存偏好文件」的协议，避免把整个 app_data_dir 暴露给 webview。
//
// 为什么不走 Tauri 的 asset 协议：那需要在 tauri.conf.json 配 assetProtocol scope，
// 且会把目录开放给所有本地文件；本项目已有 `fntvimg` / `fntvweb` 两个自定义协议，
// 照同一套模式加一个最省事，权限面也最小。

use tauri::http::{Request, Response};

pub async fn handle_request(app: tauri::AppHandle, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let not_found = || Response::builder().status(404).body(Vec::new()).unwrap_or_else(|_| Response::new(Vec::new()));

    /* [lc-2419/2421] 取 `uri().path()` 而不是 `to_string()` 再手工 strip。
       自定义协议在两端形态不同（与 fntvimg 一致，见 imgproto.rs 顶部注释）：
         · Windows：`http://artwork.localhost/<id>/<role>`  ← 前端在 Windows 必须用这个
         · 其它：  `artwork://localhost/<id>/<role>`
       两种形态下 `uri().path()` 都是 `/<id>/<role>`，所以这里只需要解析路径。
       ⚠ [lc-2421] 前端之前拼的是 `artwork://preferred/...` —— Windows 上
       WebView2 不把 `scheme://host/` 路由到自定义协议处理器（只认
       `http://<scheme>.localhost/`），于是偏好图加载失败、表现为裂图。
       修复在前端；这里做成对两种形态都健壮（也接受多余的 host 段，见下面的过滤）。 */
    let path = request.uri().path().to_string();
    let mut parts: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    // 容错：`artwork://preferred/<id>/<role>` 形态（旧版前端/手工调试）多一个 host 段
    if parts.len() == 3 && (parts[0] == "preferred" || parts[0] == "localhost") {
        parts.remove(0);
    }
    if parts.len() != 2 {
        return not_found();
    }
    let (id, role) = (parts[0], parts[1]);
    if id.len() != 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) || !detail_artwork::is_valid_role(role) {
        return not_found();
    }
    let Ok(dir) = store_dir(&app) else { return not_found() };
    let file = dir.join(id).join(format!("{role}.webp"));
    let Ok(bytes) = std::fs::read(&file) else { return not_found() };

    Response::builder()
        .status(200)
        .header("content-type", "image/webp")
        .header("cache-control", "no-cache") // 文件名带 ?v=，靠版本参数破缓存
        .body(bytes)
        .unwrap_or_else(|_| not_found())
}

/// [lc-2420] 批量读偏好：首页轮播一次要显示 8+ 部片子，逐个 invoke 太慢
/// （每个都过一次 IPC + 磁盘 stat）。
///
/// 入参 `owners` 是 owner 串数组（前端用 `artworkOwner()` 生成，与详情页同一套），
/// 返回 `{ owner: { role: 绝对路径 } }`（没有偏好的 owner 不出现在结果里）。
///
/// ⚠ 为什么要按 owner 批量而不是按 guid：owner 是前端算的（`["tmdb","tv",206630]`），
/// 后端不认识 guid ↔ tmdb 的映射关系，也不该认识（那是 tmdb.rs 的职责）。
/// 由前端把 carousel 条目映射成 owner 串再一起问，职责最干净。
#[tauri::command]
pub async fn read_detail_artwork_batch(
    app: tauri::AppHandle,
    owners: Vec<String>,
) -> Result<serde_json::Value, String> {
    let dir = store_dir(&app)?;
    let mut out = serde_json::Map::new();
    for owner in owners {
        // 单个 owner 出错（如超长/空串）不该让整批失败
        let Ok(picks) = detail_artwork::read_picks(&dir, &owner) else {
            continue;
        };
        if picks.is_empty() {
            continue;
        }
        let mut m = serde_json::Map::new();
        for (role, path) in picks {
            m.insert(
                role,
                serde_json::Value::String(path.to_string_lossy().to_string()),
            );
        }
        out.insert(owner, serde_json::Value::Object(m));
    }
    Ok(serde_json::Value::Object(out))
}
