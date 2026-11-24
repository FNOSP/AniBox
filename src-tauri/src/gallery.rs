//! [lc-2363] 图库的**本地图片扫描** + 通用工具。
//!
//! ## 现在只服务 Pixiv 图库页的「本地」分区
//!
//! 早前这里还有 safebooru（第三方图库）的排行/搜索/tag 联想，用户明确要求
//! 「cos 和之前那个源都不要，只要 pixiv」后**已全部删除**——包括前端的三个分区
//! 与后端 `gallery_online` / `gallery_tags` 两个命令。
//!
//! 保留的部分：
//!   · `gallery_local`：扫本机图片缓存（`app_data_dir/cache/img`）+ 用户指定目录。
//!     **只读，绝不改名/移动/删除**。
//!   · `read_local_image`：供 fntvimg 的 `/__gallery/` 分支读盘。
//!   · `urlencode_public`：`pixiv.rs` 复用的 URL 编码（避免两处各写一份）。
//!
//! ## 为什么本地分区要有独立命令而不是复用 fntvimg
//!
//! fntvimg 是「按已知路径取单张图」的代理；本地分区需要的是「先列出有哪些图、
//! 各自多大」。两者职责不同（一个取内容、一个列举）。但**显示仍走 fntvimg**
//! （`/__gallery/` 分支），避免前端直接碰文件系统。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 本地扫描的单目录最多返回条数（防用户指定超大目录把 UI 卡死）。
const LOCAL_MAX: usize = 500;
/// 认作图片的扩展名（与小写后比较）。
const IMG_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"];
/// 跳过小于此字节数的文件（图标/占位图，画廊里没意义）。
const MIN_BYTES: u64 = 12 * 1024;

pub(crate) fn urlencode_public(s: &str) -> String {
    urlencode(s)
}

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
fn is_image(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    IMG_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

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
    fn read_local_image_rejects_non_image_and_relative() {
        assert!(read_local_image("relative/a.jpg").is_err());
        assert!(read_local_image("C:/Windows/notepad.exe").is_err());
    }
}
