//! [library-cover] 首页「媒体库」卡片的**自定义封面**存储。
//!
//! 落盘位置（都在应用数据目录，与 config.json 同级）：
//! ```text
//! <app_data_dir>/library-covers/<短hash>-<毫秒时间戳>.<ext>   ← 用户上传的图片副本
//! <app_data_dir>/library-covers.json                          ← { "<libGuid>": "<文件名>" }
//! ```
//!
//! 设计要点：
//!   · **不改动用户原图**：前端只把选中文件的字节读出来，这里另存一份副本；原文件留在原处。
//!   · 文件名由「库 guid 的短 hash + 时间戳 + 按文件头识别出的扩展名」生成，
//!     每次上传都换新名 ⇒ 浏览器/WebView 不会命中旧图缓存；旧文件在换图/恢复默认时删除。
//!   · 图片经 fntvimg 自定义协议的 `/__cover/<文件名>` 读取（见 imgproto.rs），
//!     不需要把 10MB 的图塞进 localStorage。
//!   · 类型不信任前端 MIME/扩展名，一律按**文件头魔数**判定（见 [`detect_ext`]）。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

/// 单张封面允许的最大字节数（用户约定 ≤10MB）。
pub const MAX_COVER_BYTES: usize = 10 * 1024 * 1024;
/// 自定义封面索引文件名（与 config.json 同级）。
const MAP_FILE: &str = "library-covers.json";
/// 封面副本目录名。
const COVER_DIR: &str = "library-covers";

/// 索引读写的进程内互斥：两个库同时换封面时避免互相覆盖 JSON。
static MAP_LOCK: Mutex<()> = Mutex::new(());

fn base_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("无法定位应用数据目录：{e}"))
}

fn covers_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(base_dir(app)?.join(COVER_DIR))
}

fn map_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(base_dir(app)?.join(MAP_FILE))
}

fn load_map(app: &AppHandle) -> BTreeMap<String, String> {
    let Ok(p) = map_path(app) else {
        return BTreeMap::new();
    };
    let Ok(txt) = std::fs::read_to_string(p) else {
        return BTreeMap::new();
    };
    serde_json::from_str::<BTreeMap<String, String>>(&txt).unwrap_or_default()
}

fn save_map(app: &AppHandle, map: &BTreeMap<String, String>) -> Result<(), String> {
    let p = map_path(app)?;
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let txt = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, txt).map_err(|e| format!("写入封面索引失败：{e}"))?;
    if std::fs::rename(&tmp, &p).is_err() {
        // Windows 的 rename 不能覆盖已存在文件：先删再改名。
        let _ = std::fs::remove_file(&p);
        std::fs::rename(&tmp, &p).map_err(|e| format!("替换封面索引失败：{e}"))?;
    }
    Ok(())
}

/// 文件名白名单：只允许本模块生成出来的形态，杜绝 `..` / 路径分隔符 / 控制字符。
fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn short_hash(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let d = h.finalize();
    let mut out = String::with_capacity(16);
    for b in d.iter().take(8) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 按文件头识别图片类型。返回落盘用的扩展名；无法识别 → `None`。
fn detect_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    if bytes.starts_with(b"BM") {
        return Some("bmp");
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && (&bytes[8..12] == b"avif" || &bytes[8..12] == b"avis")
    {
        return Some("avif");
    }
    None
}

/// 扩展名 → Content-Type（`imgproto` 直接回给 WebView）。
pub fn content_type_for(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        _ => "image/jpeg",
    }
}

fn url_of(file: &str) -> String {
    format!("/__cover/{file}")
}

/// 当前所有自定义封面：`{ libGuid: "/__cover/<file>" }`。
pub fn custom_urls(app: &AppHandle) -> Value {
    let map = load_map(app);
    let out: BTreeMap<String, String> = map.into_iter().map(|(k, f)| (k, url_of(&f))).collect();
    json!(out)
}

/// 按文件名读取封面副本（供 `/__cover/*` 协议调用）。名字不合法 / 文件不存在 → `None`。
pub fn read_cover(app: &AppHandle, name: &str) -> Option<(String, Vec<u8>)> {
    if !safe_file_name(name) {
        return None;
    }
    let path = covers_dir(app).ok()?.join(name);
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some((content_type_for(name).to_string(), bytes))
}

// ───────────────────────── Tauri 命令 ─────────────────────────

/// 读取「libGuid → 自定义封面 URL」映射（首页启动时调用）。
#[tauri::command]
pub fn get_library_covers(app: AppHandle) -> Result<Value, String> {
    Ok(custom_urls(&app))
}

/// 保存自定义封面。`data_b64` 是前端 `FileReader.readAsDataURL()` 去掉前缀后的 base64。
/// 返回 `{ url: "/__cover/<file>", file }`，前端直接把它当作卡片封面地址。
#[tauri::command]
pub fn save_library_cover(app: AppHandle, lib_id: String, data_b64: String) -> Result<Value, String> {
    let lib_id = lib_id.trim().to_string();
    if lib_id.is_empty() {
        return Err("媒体库 ID 为空".into());
    }
    let bytes = B64
        .decode(data_b64.as_bytes())
        .map_err(|_| "封面数据不是合法 base64".to_string())?;
    if bytes.is_empty() {
        return Err("封面数据为空".into());
    }
    if bytes.len() > MAX_COVER_BYTES {
        return Err("封面图片不能超过 10MB".into());
    }
    let ext = detect_ext(&bytes)
        .ok_or_else(|| "不支持的图片格式（支持 JPG/PNG/WebP/GIF/BMP/AVIF）".to_string())?;

    let dir = covers_dir(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建封面目录失败：{e}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let file = format!("{}-{}.{}", short_hash(&lib_id), now, ext);
    std::fs::write(dir.join(&file), &bytes).map_err(|e| format!("保存封面失败：{e}"))?;

    let _guard = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = load_map(&app);
    if let Some(old) = map.insert(lib_id, file.clone()) {
        if old != file {
            let _ = std::fs::remove_file(dir.join(&old));
        }
    }
    save_map(&app, &map)?;
    Ok(json!({ "url": url_of(&file), "file": file }))
}

/// 恢复默认封面：删掉该库的自定义副本与索引项（库本身、原图都不动）。
#[tauri::command]
pub fn reset_library_cover(app: AppHandle, lib_id: String) -> Result<Value, String> {
    let _guard = MAP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = load_map(&app);
    if let Some(old) = map.remove(&lib_id) {
        if let Ok(dir) = covers_dir(&app) {
            let _ = std::fs::remove_file(dir.join(&old));
        }
        save_map(&app, &map)?;
    }
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::{content_type_for, detect_ext, safe_file_name, short_hash};

    #[test]
    fn detects_supported_image_headers() {
        assert_eq!(detect_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(
            detect_ext(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]),
            Some("png")
        );
        assert_eq!(detect_ext(b"GIF89a......"), Some("gif"));
        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        webp.extend_from_slice(b"VP8 ");
        assert_eq!(detect_ext(&webp), Some("webp"));
        assert_eq!(detect_ext(b"BM....."), Some("bmp"));
        assert_eq!(detect_ext(b"\0\0\0\x18ftypavif"), Some("avif"));
        assert_eq!(detect_ext(b"<html>not an image"), None);
        assert_eq!(detect_ext(b""), None);
    }

    #[test]
    fn rejects_unsafe_file_names() {
        assert!(safe_file_name("abcdef01-1700000000000.jpg"));
        assert!(!safe_file_name("../secret.jpg"));
        assert!(!safe_file_name("a/b.jpg"));
        assert!(!safe_file_name("a\\b.jpg"));
        assert!(!safe_file_name(""));
        assert!(!safe_file_name("x y.jpg"));
        assert!(!safe_file_name(&"a".repeat(200)));
    }

    #[test]
    fn hash_is_stable_and_short() {
        let a = short_hash("lib-guid-123");
        assert_eq!(a.len(), 16);
        assert_eq!(a, short_hash("lib-guid-123"));
        assert_ne!(a, short_hash("lib-guid-124"));
    }

    #[test]
    fn content_types_follow_extension() {
        assert_eq!(content_type_for("a.png"), "image/png");
        assert_eq!(content_type_for("a.WEBP"), "image/webp");
        assert_eq!(content_type_for("a.jpg"), "image/jpeg");
        assert_eq!(content_type_for("noext"), "image/jpeg");
    }
}
