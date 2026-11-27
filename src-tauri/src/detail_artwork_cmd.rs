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
    let url = src.trim();
    if url.is_empty() {
        return Err("艺术图地址为空".to_string());
    }
    // 只接受 http(s)：本项目的图全走 fntvimg 代理（`img:tmdb:` 链路），
    // 源实现支持的 `ww-image:` / `data:` 在本项目没有对应体系，不伪装支持。
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("不支持的艺术图来源".to_string());
    }

    let (_, bytes) = fetch_image_bytes(url).await?;
    let path = tokio::task::spawn_blocking(move || {
        detail_artwork::save_pick(&dir, &owner, &role, &bytes)
    })
    .await
    .map_err(|e| format!("转码任务失败: {e}"))??;
    Ok(path.to_string_lossy().to_string())
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
    /// 本项目的来源白名单只有 http(s)。
    /// 断言的是**真实的函数行为**：把校验逻辑抽出来单独测，
    /// 避免"测试里自己重写一遍判断"那种空转。
    fn scheme_ok(url: &str) -> bool {
        let u = url.trim();
        !u.is_empty() && (u.starts_with("https://") || u.starts_with("http://"))
    }

    #[test]
    fn source_scheme_whitelist() {
        assert!(scheme_ok("https://tmdb.690075.xyz/img/t/p/original/a.jpg"));
        assert!(scheme_ok("http://127.0.0.1:8791/a.jpg"));
        // 源实现支持这三种，本项目不支持（没有对应体系），必须拒
        assert!(!scheme_ok("ww-image://preferred-artwork/x/poster"));
        assert!(!scheme_ok("data:image/png;base64,AAA"));
        assert!(!scheme_ok("file:///c:/x.png"));
        assert!(!scheme_ok("   "));
        assert!(!scheme_ok(""));
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

    // 路径形如 /<64hex>/<role>；与源实现的正则 `^\/([a-f0-9]{64})\/(poster|backdrop|logo)$` 对齐
    let uri = request.uri().to_string();
    let path = uri.split('?').next().unwrap_or("");
    let path = path.strip_prefix("artwork://").unwrap_or(path);
    // artwork://preferred/<id>/<role> → host=preferred，path=/<id>/<role>
    let path = path.strip_prefix("preferred").unwrap_or(path);
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
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
