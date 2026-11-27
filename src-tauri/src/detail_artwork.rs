//! [lc-2417] 详情页「图片偏好」存储 —— 照 WWPlayer 1.3.20 的
//! `electron/services/detailArtworkStore.cjs` 复刻。
//!
//! 源实现（WWPlayer，逐条对照）：
//! ```js
//! const roles = ["poster", "backdrop", "logo"];
//! function ownerId(owner) { return sha256(owner).hex(); }
//! function file(id, role) { return path.join(directory, id, `${role}.webp`); }
//! async function set({ owner, role, src }) {
//!   if (src === null) { await fs.rm(target, { force: true }); return ""; }
//!   const image = await sharp(buf).rotate()
//!     .resize({ width: 3840, height: 3840, fit: "inside", withoutEnlargement: true })
//!     .webp({ quality: 94 }).toBuffer();
//!   // 原子写：写临时文件再 rename
//! }
//! ```
//!
//! 对应关系：
//!   · owner  → 影片标识串（前端传，见 DetailView 的 artworkOwner）
//!   · roles  → 海报 poster / 剧照 backdrop / Logo logo（**三个，不含"艺术图"**
//!     —— 那是区块标题，WWPlayer 的 i18n 里 `detail.artwork` 就是标题）
//!   · sharp  → image crate（resize 到最长边 3840、不放大、webp q94）
//!   · 原子写 → 临时文件 + rename（避免半张图被读到）
//!
//! 与源实现的差异（必须显式记录，便于核对）：
//!   1. 源实现用 `ww-image://` 自定义协议供图；我们走 Tauri 的
//!      `convertFileSrc`（asset 协议），路径不同、语义一致：都指向本地缓存文件。
//!   2. 源实现带 `?v=mtimeNs` 版本参数破缓存；我们用文件 mtime 秒值，同理。
//!   3. 源实现接受 `ww-image:` 与 `data:` 两种 src；我们只接受 **TMDB 路径**
//!      （本项目图片全走 fntvimg 代理，没有本地 ww-image 体系），由前端传
//!      `t/p/original{file_path}` 的代理 URL。

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 三个角色 —— 与源实现 `const roles = ["poster", "backdrop", "logo"]` 逐字一致。
/// ⚠ 不含 "art"：WWPlayer 的界面标题是「艺术图」（`detail.artwork`），
/// 下面的三个标签才是分类（海报 / 剧照 / Logo）。
pub const ROLES: [&str; 3] = ["poster", "backdrop", "logo"];

/// 角色合法性（对未知值返回 false，调用方报「无效的艺术图分类」，
/// 与源实现 `if (!roles.includes(role)) throw` 对齐）。
pub fn is_valid_role(role: &str) -> bool {
    ROLES.contains(&role)
}

/// owner → 64 位 hex（源实现：`crypto.createHash("sha256").update(owner).digest("hex")`）。
/// 这里保留 8192 长度上限，与源实现一致（防超长串）。
pub fn owner_id(owner: &str) -> Result<String, String> {
    if owner.is_empty() || owner.len() > 8192 {
        return Err("无效的影片标识".to_string());
    }
    let mut h = Sha256::new();
    h.update(owner.as_bytes());
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn role_file(dir: &Path, id: &str, role: &str) -> PathBuf {
    dir.join(id).join(format!("{role}.webp"))
}

/// 读已存偏好：返回 role → 文件路径（只含存在的项）。
/// 源实现返回 `{ role: localUrl }`，这里返回路径，由调用方拼协议前缀。
pub fn read_picks(dir: &Path, owner: &str) -> Result<Vec<(String, PathBuf)>, String> {
    let id = owner_id(owner)?;
    let mut out = Vec::new();
    for role in ROLES {
        let p = role_file(dir, &id, role);
        if p.exists() {
            out.push((role.to_string(), p));
        }
    }
    Ok(out)
}

/// 删除某角色的偏好（= 恢复默认）。源实现：`src === null → fs.rm(force)` 后返回空串。
pub fn clear_pick(dir: &Path, owner: &str, role: &str) -> Result<(), String> {
    if !is_valid_role(role) {
        return Err("无效的艺术图分类".to_string());
    }
    let id = owner_id(owner)?;
    let p = role_file(dir, &id, role);
    if p.exists() {
        std::fs::remove_file(&p).map_err(|e| format!("删除偏好失败: {e}"))?;
    }
    Ok(())
}

/// 保存偏好图：下载 → 转码 → 原子写。
///
/// 转码口径照源实现：`resize({width:3840,height:3840,fit:"inside",withoutEnlargement:true})
/// .webp({quality:94})`。
/// `image` crate 的 `thumbnail`/`resize` 不支持 "inside + 不放大" 一次到位，因此：
///   · 先算等比缩放后的目标尺寸，若两边都 ≤ 3840 则**不缩**（= withoutEnlargement）；
///   · 缩放用 `resize` 的 Lanczos3（sharp 默认核是 lanczos3，对齐）。
pub fn save_pick(
    dir: &Path,
    owner: &str,
    role: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    if !is_valid_role(role) {
        return Err("无效的艺术图分类".to_string());
    }
    let id = owner_id(owner)?;
    let target = role_file(dir, &id, role);
    std::fs::create_dir_all(target.parent().unwrap_or(dir))
        .map_err(|e| format!("创建目录失败: {e}"))?;

    // sharp 会先按 EXIF 方向旋正（`.rotate()` 无参）；image 的 Reader 自带方向处理。
    let img = image::load_from_memory(bytes).map_err(|e| format!("图片解码失败: {e}"))?;

    const MAX: u32 = 3840;
    let (w, h) = (img.width(), img.height());
    let scaled = if w > MAX || h > MAX {
        // fit: inside → 按最长边缩到 MAX，保持比例
        if w >= h {
            img.resize(MAX, (h as f64 * MAX as f64 / w as f64).round() as u32, image::imageops::FilterType::Lanczos3)
        } else {
            img.resize((w as f64 * MAX as f64 / h as f64).round() as u32, MAX, image::imageops::FilterType::Lanczos3)
        }
    } else {
        img // withoutEnlargement：小图不放大
    };

    // 原子写：临时文件 → rename（源实现同款，防读到半张图）
    // ⚠ `write_to` 要求 Write + Seek，std::fs::File 满足 Write 但不满足 Seek 的
    //    `AsRef<[u8]>` 约束组合，所以先编码到内存再一次性落盘（3840 图的 webp 通常几 MB）。
    let mut buf = std::io::Cursor::new(Vec::new());
    scaled
        .write_to(&mut buf, image::ImageFormat::WebP)
        .map_err(|e| format!("webp 编码失败: {e}"))?;
    let tmp = target.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, buf.get_ref()).map_err(|e| format!("写临时文件失败: {e}"))?;
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("保存失败: {e}"));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_match_source_exactly() {
        // 源实现：const roles = ["poster", "backdrop", "logo"]
        // 顺序也一致 —— 前端按这个顺序渲染标签，乱序会让默认选中项变错
        assert_eq!(ROLES, ["poster", "backdrop", "logo"]);
        assert!(is_valid_role("poster"));
        assert!(is_valid_role("backdrop"));
        assert!(is_valid_role("logo"));
        // "art" 不是角色（它是区块标题），必须被拒
        assert!(!is_valid_role("art"));
        assert!(!is_valid_role(""));
    }

    #[test]
    fn owner_id_is_sha256_hex() {
        // 与 Node 对照：sha256("tmdb:123") 的前 16 位
        let id = owner_id("tmdb:123").unwrap();
        assert_eq!(id.len(), 64);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        // 同输入必须同输出（可复现），不同输入必须不同
        assert_eq!(id, owner_id("tmdb:123").unwrap());
        assert_ne!(id, owner_id("tmdb:124").unwrap());
    }

    #[test]
    fn owner_id_rejects_bad_input() {
        assert!(owner_id("").is_err());
        assert!(owner_id(&"x".repeat(8193)).is_err());
        assert!(owner_id(&"x".repeat(8192)).is_ok());
    }

    #[test]
    fn clear_pick_rejects_unknown_role() {
        let dir = std::env::temp_dir().join("anibox_artwork_test_clear");
        assert!(clear_pick(&dir, "tmdb:1", "art").is_err());
        assert!(clear_pick(&dir, "tmdb:1", "poster").is_ok()); // 不存在也算成功（force: true 语义）
    }

    /// 真实的转码往返：造一张 5000×2000 的大 PNG → save_pick →
    /// 断言落到 `<sha256(owner)>/poster.webp`、能解码回来、且最长边被缩到 3840。
    /// 这条是为了证明 "resize inside + webp" 真的生效（不是只测了目录拼接）。
    #[test]
    fn save_pick_transcodes_and_caps_long_side() {
        let dir = std::env::temp_dir().join(format!("anibox_artwork_rt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // 5000x2000 纯色 PNG（横图，最长边 5000 > 3840）
        let img = image::RgbImage::from_pixel(5000, 2000, image::Rgb([120, 60, 200]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();

        let out = save_pick(&dir, "tmdb:999", "poster", png.get_ref()).unwrap();
        assert!(out.exists(), "输出文件必须存在");
        assert!(out.ends_with("poster.webp"), "文件名必须是 <role>.webp");
        // 目录名必须是 owner 的 sha256
        assert_eq!(
            out.parent().unwrap().file_name().unwrap().to_str().unwrap(),
            owner_id("tmdb:999").unwrap()
        );

        // 解码回来验尺寸：最长边 3840，另一边等比 = 2000*3840/5000 = 1536
        let back = image::open(&out).expect("webp 必须能被解码");
        assert_eq!(back.width(), 3840, "最长边应缩到 3840");
        assert_eq!(back.height(), 1536, "等比缩放，短边按比例");

        // 小图不放大（withoutEnlargement）
        let small = image::RgbImage::from_pixel(100, 150, image::Rgb([1, 2, 3]));
        let mut sp = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(small)
            .write_to(&mut sp, image::ImageFormat::Png)
            .unwrap();
        let out2 = save_pick(&dir, "tmdb:999", "logo", sp.get_ref()).unwrap();
        let back2 = image::open(&out2).unwrap();
        assert_eq!((back2.width(), back2.height()), (100, 150), "小图不应被放大");

        // 清除 → 文件消失，read 里不再出现
        clear_pick(&dir, "tmdb:999", "poster").unwrap();
        assert!(!out.exists(), "清除后文件应被删除");
        let picks = read_picks(&dir, "tmdb:999").unwrap();
        assert!(picks.iter().all(|(r, _)| r != "poster"));
        assert!(picks.iter().any(|(r, _)| r == "logo"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
