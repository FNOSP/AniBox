//! [lc-2354] 画廊：外网二次元图库（safebooru）+ 本地图片扫描。
//!
//! ## 为什么是 safebooru
//!
//! AniCh 的画廊本质是「社区帖子流 → 瀑布流图片」（`home/views/artwork.dart` 与 `cosplay.dart`，
//! 数据来自它自己的 `/latest?type=artwork|cosplay` protobuf 接口）。**它后端已死**（三家域名全
//! 不可达，见 [[anich-research]]），所以照抄接口没有意义。
//!
//! 实测可用的替代（2026-11-24）：
//!   - ✅ **safebooru.org**：`page=dapi&s=post&q=index&json=1` 零认证、分页正常
//!     （`pid` 递增、id 递减）、字段齐全（`preview_url`/`sample_url`/`file_url`/`width`/`height`/`tags`）
//!   - ❌ konachan / yande.re / danbooru / gelbooru：全部超时（本机网络下不可达）
//!
//! safebooru 只收录 **safe** 分级（这是它存在的意义，与 danbooru 的区别），
//! 但仍额外传 `rating:general` 兜底（部分老图 rating 字段为空）。
//!
//! ## 两个 tab 的数据来源
//!
//! 1. **图库**（外网）：safebooru API 直连。默认按热度（`sort:score`）拉，支持 tag 过滤。
//! 2. **本地**：扫本机图片缓存目录（`app_data_dir/cache/img`，即 fntvimg 落盘的那批）
//!    + 用户可指定的额外目录。**只读，绝不改名/移动/删除**。
//!
//! ## 为什么本地 tab 要有独立命令而不是复用 fntvimg
//!
//! fntvimg 是「按已知路径取单张图」的代理；画廊需要的是「先列出有哪些图、各自宽高多少」。
//! 两者职责不同（一个取内容、一个列举），混在一起会让 fntvimg 的缓存语义变复杂。
//! 但**显示仍走 fntvimg**（新增 `/__local/<绝对路径>` 分支），避免前端直接碰文件系统。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// safebooru 单页最大条数（API 上限 1000，取 40 控制首屏体积与响应时间）。
const PAGE_LIMIT: usize = 40;
/// 本地扫描的单目录最多返回条数（防用户指定超大目录把 UI 卡死）。
const LOCAL_MAX: usize = 500;
/// 认作图片的扩展名（与小写后比较）。
const IMG_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"];
/// 跳过小于此字节数的文件（图标/占位图，画廊里没意义）。
const MIN_BYTES: u64 = 12 * 1024;

fn is_image(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    IMG_EXTS.contains(&ext.as_str())
}

/// 解析 safebooru 响应为前端要的扁平结构。
///
/// 只保留画廊真正用得上的字段 —— 原始条目有 24 个字段（`directory`/`hash`/`owner`/
/// `parent_id`/`change`/`has_notes`…），全透传会让 IPC 载荷翻几倍且前端一个都不读。
fn map_post(p: &Value) -> Option<Value> {
    let id = p.get("id")?.as_i64()?;
    let preview = p.get("preview_url").and_then(|v| v.as_str()).unwrap_or("");
    let sample = p.get("sample_url").and_then(|v| v.as_str()).unwrap_or("");
    let file = p.get("file_url").and_then(|v| v.as_str()).unwrap_or("");
    // 至少要有一张能显示的图
    let thumb = if !preview.is_empty() { preview } else if !sample.is_empty() { sample } else { file };
    if thumb.is_empty() {
        return None;
    }
    // 原图：优先 file_url；safebooru 的 file_url 是完整 URL，但极老的条目可能是裸文件名
    // （字段里另有 directory/hash/image 三个碎片）。裸名拼不出来就退回 sample。
    let full = if file.starts_with("http") {
        file.to_string()
    } else if sample.starts_with("http") {
        sample.to_string()
    } else {
        thumb.to_string()
    };
    let w = p.get("width").and_then(|v| v.as_i64()).unwrap_or(0);
    let h = p.get("height").and_then(|v| v.as_i64()).unwrap_or(0);
    Some(json!({
        "id": id,
        // 瀑布流按宽高比排版；0 表示未知（前端退化为固定比例）
        "w": w,
        "h": h,
        "thumb": thumb,
        "full": full,
        "tags": p.get("tags").and_then(|v| v.as_str()).unwrap_or(""),
        "score": p.get("score").and_then(|v| v.as_i64()).unwrap_or(0),
    }))
}

/// 拉一页外网图库。
///
/// `page` 从 0 开始（safebooru 的 `pid` 语义就是 0 基）。
/// `tags` 为空时用内置的安全 tag 组合；用户传入的 tag 会**追加**在安全 tag 之后
/// （不是替换）—— 防止用户无意中把 `rating:general` 去掉。
#[tauri::command]
pub async fn gallery_online(
    page: Option<u32>,
    tags: Option<String>,
) -> Result<Value, String> {
    let pid = page.unwrap_or(0);
    let user_tags = tags.unwrap_or_default();
    let user_tags = user_tags.trim();

    // 安全兜底 + 排序。
    //
    // [lc-2354 实测坑] 排序语法是 **`sort:score`** 不是 `order:score` ——
    // safebooru 对不认识的 meta-tag 不报错，而是**静默返回空 body**（HTTP 200 + 0 字节）。
    // 写成 `order:score` 会让接口「看起来正常但一张图都没有」，极难排查。
    let mut tag_q = String::from("rating:general sort:score");
    if !user_tags.is_empty() {
        // 标签里出现空白就按 AND 拼；safebooru 用空格分隔多 tag
        tag_q.push(' ');
        tag_q.push_str(user_tags);
    }

    let url = format!(
        "https://safebooru.org/index.php?page=dapi&s=post&q=index&json=1&limit={}&pid={}&tags={}",
        PAGE_LIMIT,
        pid,
        urlencode(&tag_q)
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("AniBox/1.0 (gallery)")
        .build()
        .map_err(|e| format!("HTTP 客户端构建失败: {e}"))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("图库请求失败（网络不通或站点不可达）: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("图库返回 HTTP {}", resp.status()));
    }
    let text = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;

    // safebooru 在「无结果」时返回空 body 或 `[]`，都不是错误
    let text = text.trim();
    if text.is_empty() {
        return Ok(json!({ "items": [], "page": pid, "hasMore": false }));
    }
    let arr: Vec<Value> = serde_json::from_str(text).map_err(|e| {
        // 站点返回 HTML（维护页/风控页）时给出可读提示，而不是裸 JSON 错误
        format!("图库响应非 JSON（可能站点维护或限流）: {e}")
    })?;
    let items: Vec<Value> = arr.iter().filter_map(map_post).collect();
    let has_more = arr.len() >= PAGE_LIMIT;
    Ok(json!({
        "items": items,
        "page": pid,
        "hasMore": has_more,
    }))
}

/// 极简 URL 编码（只处理 tag 里可能出现的字符：空格、冒号、括号、点、下划线）。
/// 不引入额外依赖 —— 这些 tag 的字符集很小，且我们只拼查询串的一个值。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 本地图片扫描结果条目。
fn map_local(path: &Path, meta: &std::fs::Metadata) -> Option<Value> {
    let name = path.file_name()?.to_str()?.to_string();
    let size = meta.len();
    if size < MIN_BYTES {
        return None;
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(json!({
        "name": name,
        // 绝对路径：前端交给 fntvimg 的 /__local/ 分支去读
        "path": path.to_string_lossy(),
        "size": size,
        "mtime": modified,
    }))
}

/// 递归收集目录下的图片（深度限制 4，避免误选根目录时全盘遍历）。
fn collect_images(dir: &Path, out: &mut Vec<Value>, depth: u32) {
    if out.len() >= LOCAL_MAX || depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    // 先收集再排序：read_dir 顺序是文件系统给的，不稳定，前端分页/对比会看到抖动
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if out.len() >= LOCAL_MAX {
            return;
        }
        let Ok(meta) = std::fs::metadata(&p) else {
            continue;
        };
        if meta.is_dir() {
            collect_images(&p, out, depth + 1);
        } else if meta.is_file() && is_image(&p) {
            if let Some(item) = map_local(&p, &meta) {
                out.push(item);
            }
        }
    }
}

/// 列出本地图片。
///
/// `dir` 留空 = 只扫应用自己的图片缓存（`app_data_dir/cache/img`，fntvimg 落盘的那批）。
/// 给了 `dir` = 额外扫该目录（**只读**，不会写/改/删任何文件）。
///
/// 返回里 `cacheDir` 让前端能显示「这 256 张来自应用缓存」，`truncated` 提示是否触顶。
#[tauri::command]
pub async fn gallery_local(
    app: tauri::AppHandle,
    dir: Option<String>,
) -> Result<Value, String> {
    use tauri::Manager;

    let mut items: Vec<Value> = Vec::new();
    let mut dirs_scanned: Vec<String> = Vec::new();

    // 1) 应用图片缓存（存在的才扫）
    if let Ok(base) = app.path().app_data_dir() {
        let cache = base.join("cache").join("img");
        if cache.is_dir() {
            dirs_scanned.push(cache.to_string_lossy().to_string());
            collect_images(&cache, &mut items, 0);
        }
    }

    // 2) 用户指定目录
    if let Some(d) = dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let p = Path::new(d);
        if p.is_dir() {
            dirs_scanned.push(p.to_string_lossy().to_string());
            collect_images(p, &mut items, 0);
        } else {
            return Err(format!("目录不存在或不是目录: {d}"));
        }
    }

    let truncated = items.len() >= LOCAL_MAX;

    // 按修改时间倒序（最新的在前），符合「相册」直觉
    items.sort_by(|a, b| {
        let ma = a.get("mtime").and_then(|v| v.as_u64()).unwrap_or(0);
        let mb = b.get("mtime").and_then(|v| v.as_u64()).unwrap_or(0);
        mb.cmp(&ma)
    });

    Ok(json!({
        "items": items,
        "dirsScanned": dirs_scanned,
        "truncated": truncated,
        "max": LOCAL_MAX,
    }))
}

/// 读本地图片原始字节（供 fntvimg 的 `/__local/` 分支调用）。
///
/// **安全约束**：只允许读图片扩展名的文件，且路径必须是绝对路径。
/// 这是本地进程内调用、不经过网络，但画廊前端传的是自己刚列出来的路径，
/// 加一道扩展名校验可以挡住「误传任意文件路径」这类意外。
pub fn read_local_image(path: &str) -> Result<(Vec<u8>, String), String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err("只接受绝对路径".into());
    }
    if !is_image(p) {
        return Err("不是图片文件".into());
    }
    let bytes = std::fs::read(p).map_err(|e| format!("读取失败: {e}"))?;
    let mime = match p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("avif") => "image/avif",
        _ => "image/jpeg",
    };
    Ok((bytes, mime.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_image_matches_known_exts() {
        assert!(is_image(Path::new("a.jpg")));
        assert!(is_image(Path::new("a.JPEG")));
        assert!(is_image(Path::new("a.webp")));
        assert!(!is_image(Path::new("a.txt")));
        assert!(!is_image(Path::new("a")));
    }

    #[test]
    fn urlencode_handles_tag_chars() {
        assert_eq!(urlencode("rating:general"), "rating%3Ageneral");
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("touhou_(series)"), "touhou_%28series%29");
        assert_eq!(urlencode("ok-_.~"), "ok-_.~");
    }

    #[test]
    fn map_post_picks_thumb_and_full() {
        let p = json!({
            "id": 123,
            "preview_url": "https://x/p.jpg",
            "sample_url": "https://x/s.jpg",
            "file_url": "https://x/f.jpg",
            "width": 1000, "height": 1500,
            "tags": "a b", "score": 7
        });
        let m = map_post(&p).unwrap();
        assert_eq!(m["thumb"], "https://x/p.jpg");
        assert_eq!(m["full"], "https://x/f.jpg");
        assert_eq!(m["w"], 1000);
        assert_eq!(m["h"], 1500);
    }

    #[test]
    fn map_post_falls_back_when_file_url_is_bare() {
        // 老条目的 file_url 可能是裸文件名 —— 退回 sample 而不是拼出坏 URL
        let p = json!({
            "id": 1,
            "preview_url": "https://x/p.jpg",
            "sample_url": "https://x/s.jpg",
            "file_url": "abc123.jpg",
            "width": 1, "height": 1
        });
        let m = map_post(&p).unwrap();
        assert_eq!(m["full"], "https://x/s.jpg");
    }

    #[test]
    fn map_post_rejects_entry_without_any_image() {
        let p = json!({ "id": 1, "preview_url": "", "sample_url": "", "file_url": "" });
        assert!(map_post(&p).is_none());
    }

    #[test]
    fn read_local_image_rejects_non_image_and_relative() {
        assert!(read_local_image("relative/a.jpg").is_err());
        assert!(read_local_image("C:/Windows/notepad.exe").is_err());
    }
}
