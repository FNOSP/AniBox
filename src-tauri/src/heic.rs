//! [lc-2081] HEIC 封面解码（短剧车道）。
//!
//! 红果站源的封面是字节数字 CDN 的 `*.heic`（HEVC-in-HEIF）——WebView2/Chromium
//! 不支持 HEIC 解码，直出会整片裂图。本模块把 HEIC 解成 JPEG：
//!   1. **容器解析**（纯 Rust，逻辑照抄 guoapp `image_heif.go`）：ftyp/meta/pitm/
//!      iinf/iprp[ipco+ipma]/iloc → 提取 hvc1 item 的 HEVC AnnexB 裸流；
//!   2. **解码**：libde265（随包 `third_party/duanju/libde265-0.dll`，仅 ~470KB，
//!      依赖 libgcc/libwinpthread 同目录）；DLL 缺失/加载失败时返回 None 降级为
//!      原样透传（前端显示占位，绝不 panic）；
//!   3. **编码**：YUV420P → RGB24（BT.601 full-range，与探针口径一致）→ image crate 编 JPEG。
//!
//! 探针验证：tests/spikes/heic_de265_probe.py（400×572 真实封面全链路 PASS）。

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

/// HEIC 判定：ftyp box 的 brand 含 heic/heix/hevc/hevx（guoapp isHEICImage 同口径）。
pub fn is_heic(data: &[u8]) -> bool {
    if data.len() < 16 || &data[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if size < 16 || size > 256 || size > data.len() {
        return false;
    }
    let mut off = 8;
    while off + 4 <= size {
        if off != 12 {
            let b = &data[off..off + 4];
            if b == b"heic" || b == b"heix" || b == b"hevc" || b == b"hevx" {
                return true;
            }
        }
        off += 4;
    }
    false
}

// ─────────────────────── ISOBMFF 容器解析（照抄 guoapp image_heif.go） ───────────────────────

struct Box_<'a> {
    kind: [u8; 4],
    data: &'a [u8],
}

fn parse_boxes(mut d: &[u8]) -> Result<Vec<Box_<'_>>, String> {
    let mut out = Vec::new();
    while !d.is_empty() {
        if d.len() < 8 || out.len() >= 4096 {
            return Err("HEIC 容器结构无效".into());
        }
        let mut size = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as u64;
        let mut header = 8usize;
        if size == 1 {
            if d.len() < 16 {
                return Err("HEIC 容器头不完整".into());
            }
            let mut b = [0u8; 8];
            b.copy_from_slice(&d[8..16]);
            size = u64::from_be_bytes(b);
            header = 16;
        } else if size == 0 {
            size = d.len() as u64;
        }
        if size < header as u64 || size > d.len() as u64 {
            return Err("HEIC 容器长度无效".into());
        }
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&d[4..8]);
        out.push(Box_ { kind, data: &d[header..size as usize] });
        d = &d[size as usize..];
    }
    Ok(out)
}

fn find_box<'a>(boxes: &'a [Box_<'a>], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes.iter().find(|b| &b.kind == kind).map(|b| b.data)
}

struct Reader<'a> {
    d: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if n > self.d.len() {
            return Err("HEIC 数据不完整".into());
        }
        let (a, b) = self.d.split_at(n);
        self.d = b;
        Ok(a)
    }
    fn uint(&mut self, n: usize) -> Result<u64, String> {
        if n > 8 {
            return Err("HEIC 字段长度无效".into());
        }
        let bytes = self.take(n)?;
        let mut v = 0u64;
        for b in bytes {
            v = (v << 8) | *b as u64;
        }
        Ok(v)
    }
    fn err(&self) -> bool {
        false // take/uint 失败即返回 Err，无需粘滞标志
    }
}

/// 提取主图片（hvc1）的 HEVC AnnexB 裸流 + 图像宽高。
pub fn extract_annexb(data: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    const MAX: usize = 20 << 20;
    if data.len() > MAX || !is_heic(data) {
        return Err("不是支持的 HEIC 封面".into());
    }
    let top = parse_boxes(data)?;
    let meta = find_box(&top, b"meta").ok_or("HEIC 缺少图片元数据")?;
    if meta.is_empty() || meta[0] != 0 {
        return Err("HEIC 缺少图片元数据".into());
    }
    let children = parse_boxes(&meta[4..])?;

    // pitm → 主图片 ID
    let pitm = Reader { d: find_box(&children, b"pitm").ok_or("HEIC 缺少主图片标识")? };
    let mut r = pitm;
    let version = r.uint(1)?;
    r.take(3)?;
    let id_size = if version == 1 { 4 } else { 2 };
    let item_id = r.uint(id_size)?;
    if version > 1 || item_id == 0 {
        return Err("HEIC 主图片标识无效".into());
    }

    // iinf → 确认 item 类型是 hvc1（独立 HEVC 图片）
    let iinf = Reader { d: find_box(&children, b"iinf").ok_or("HEIC 图片信息缺失")? };
    let mut ir = iinf;
    let version = ir.uint(1)?;
    ir.take(3)?;
    let count_size = if version == 1 { 4 } else { 2 };
    let count = ir.uint(count_size)?;
    if version > 1 {
        return Err("HEIC 图片信息无效".into());
    }
    let infe_boxes = parse_boxes(ir.take(ir.d.len())?)?;
    if count != infe_boxes.len() as u64 {
        return Err("HEIC 图片信息不完整".into());
    }
    let mut item_kind_ok = false;
    for b in &infe_boxes {
        if &b.kind != b"infe" {
            continue;
        }
        let mut rd = Reader { d: b.data };
        let v = rd.uint(1)?;
        rd.take(3)?;
        let isz = if v == 3 { 4 } else { 2 };
        let id = rd.uint(isz)?;
        let protected = rd.uint(2)?;
        let kind = rd.take(4)?;
        if id == item_id {
            if v != 2 && v != 3 {
                return Err("HEIC 封面 item 版本不支持".into());
            }
            if protected != 0 || &kind != b"hvc1" {
                return Err("HEIC 封面不是独立的 HEVC 图片".into());
            }
            item_kind_ok = true;
            break;
        }
    }
    if !item_kind_ok {
        return Err("HEIC 主图片不存在".into());
    }

    // iprp → ipco 属性表 + ipma 关联（hvcC 配置 / ispe 尺寸 / irot 旋转）
    let iprp = find_box(&children, b"iprp").ok_or("HEIC 属性容器缺失")?;
    let iprp_boxes = parse_boxes(iprp)?;
    let ipco = parse_boxes(find_box(&iprp_boxes, b"ipco").ok_or("HEIC 属性表缺失")?)?;
    let ipma = Reader { d: find_box(&iprp_boxes, b"ipma").ok_or("HEIC 属性关联缺失")? };
    let mut rr = ipma;
    let version = rr.uint(1)?;
    let flags = rr.uint(3)?;
    let count = rr.uint(4)?;
    if version > 1 || count > 4096 {
        return Err("HEIC 图片属性无效".into());
    }
    let mut config: Option<&[u8]> = None;
    let mut width = 0u64;
    let mut height = 0u64;
    for _ in 0..count {
        let idsz = if version == 1 { 4 } else { 2 };
        let id = rr.uint(idsz)?;
        let associations = rr.uint(1)?;
        for _ in 0..associations {
            let mut index = rr.uint(1)? & 127;
            if flags & 1 != 0 {
                index = (index << 8) | rr.uint(1)?;
            }
            if id != item_id || index == 0 {
                continue;
            }
            let property = ipco.get(index as usize - 1).ok_or("HEIC 属性索引越界")?;
            match &property.kind {
                b"hvcC" => config = Some(property.data),
                b"ispe" => {
                    if property.data.len() != 12 {
                        return Err("HEIC 图片尺寸无效".into());
                    }
                    width = u32::from_be_bytes([property.data[4], property.data[5], property.data[6], property.data[7]]) as u64;
                    height = u32::from_be_bytes([property.data[8], property.data[9], property.data[10], property.data[11]]) as u64;
                }
                _ => {}
            }
        }
    }
    let config = match config {
        Some(c) if c.len() >= 23 => c,
        _ => return Err("HEIC 编码信息或图片尺寸无效".into()),
    };
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err("HEIC 图片尺寸无效".into());
    }

    // iloc → item 数据（method 0=mdat 偏移 / 1=idat 内联）
    let iloc = Reader { d: find_box(&children, b"iloc").ok_or("HEIC 图片位置缺失")? };
    let mut lr = iloc;
    let version = lr.uint(1)?;
    lr.take(3)?;
    let lengths = lr.uint(1)?;
    let offsets = lr.uint(1)?;
    let offset_size = (lengths >> 4) as usize;
    let length_size = (lengths & 15) as usize;
    let base_size = (offsets >> 4) as usize;
    let mut index_size = 0usize;
    if version > 0 {
        index_size = (offsets & 15) as usize;
    }
    let count_size = if version == 2 { 4 } else { 2 };
    let count = lr.uint(count_size)?;
    if version > 2 || count > 4096 || offset_size > 8 || length_size > 8 || base_size > 8 || index_size > 8 {
        return Err("HEIC 图片位置无效".into());
    }
    let mut item = Vec::new();
    for _ in 0..count {
        let id = lr.uint(count_size)?;
        let mut method = 0u64;
        if version > 0 {
            method = lr.uint(2)? & 15;
        }
        let reference = lr.uint(2)?;
        let base = lr.uint(base_size)?;
        let extents = lr.uint(2)?;
        if extents > 4096 || (id == item_id && (reference != 0 || method > 1)) {
            return Err("HEIC 不支持外部图片引用".into());
        }
        for _ in 0..extents {
            lr.uint(index_size)?;
            let offset = lr.uint(offset_size)?;
            let length = lr.uint(length_size)?;
            if id != item_id {
                continue;
            }
            let source: &[u8] = if method == 1 {
                find_box(&children, b"idat").unwrap_or(&[])
            } else {
                data
            };
            if base > source.len() as u64 || offset > source.len() as u64 - base {
                return Err("HEIC 图片偏移越界".into());
            }
            let start = (base + offset) as usize;
            let length = if length == 0 { source.len() - start } else { length as usize };
            if length > source.len() - start || length > MAX - item.len() {
                return Err("HEIC 图片长度越界".into());
            }
            item.extend_from_slice(&source[start..start + length]);
        }
    }
    if item.is_empty() {
        return Err("HEIC 图片数据缺失".into());
    }

    // hvcC + item NAL → AnnexB（起止码 + VPS/SPS/PPS 数组 + 切片）
    if config[0] != 1 {
        return Err("HEIC 解码配置无效".into());
    }
    let mut annexb = Vec::with_capacity(item.len() + 64);
    let mut push_nal = |out: &mut Vec<u8>, nal: &[u8]| -> Result<(), String> {
        if nal.len() < 2 || nal.len() > MAX - 4 - out.len() {
            return Err("HEIC 编码数据长度无效".into());
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        Ok(())
    };
    let mut rd = Reader { d: &config[23..] };
    for _ in 0..config[22] {
        rd.uint(1)?;
        let count = rd.uint(2)?;
        if count > 4096 {
            return Err("HEIC 解码配置过大".into());
        }
        for _ in 0..count {
            let size = rd.uint(2)? as usize;
            let nal = rd.take(size)?;
            push_nal(&mut annexb, nal)?;
        }
    }
    if !rd.d.is_empty() || annexb.is_empty() {
        return Err("HEIC 解码配置不完整".into());
    }
    let length_size = ((config[21] & 3) + 1) as usize;
    let mut rd = Reader { d: &item };
    let mut slice_count = 0usize;
    while !rd.d.is_empty() {
        let size = rd.uint(length_size)? as usize;
        if size > rd.d.len() || slice_count >= 4096 {
            return Err("HEIC 图片编码不完整".into());
        }
        let nal = rd.take(size)?;
        push_nal(&mut annexb, nal)?;
        slice_count += 1;
    }
    Ok((annexb, width as u32, height as u32))
}

// ─────────────────────── libde265 FFI（随包 DLL，动态加载） ───────────────────────

struct De265 {
    _lib: libloading::Library,
    new_decoder: unsafe extern "C" fn() -> *mut c_void,
    free_decoder: unsafe extern "C" fn(*mut c_void),
    push_data: unsafe extern "C" fn(*mut c_void, *const c_void, usize, i64, *mut c_void) -> c_int,
    flush_data: unsafe extern "C" fn(*mut c_void) -> c_int,
    decode: unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int,
    get_next_picture: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    get_image_width: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    get_image_height: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    get_image_plane: unsafe extern "C" fn(*mut c_void, c_int, *mut c_int) -> *mut u8,
    get_error_text: unsafe extern "C" fn(c_int) -> *const c_char,
}

unsafe impl Send for De265 {}
unsafe impl Sync for De265 {}

static DE265: OnceLock<Option<De265>> = OnceLock::new();

/// 加载 libde265（third_party/duanju/ 下；SetDllDirectory 让 libgcc/winpthread 依赖可达）。
fn de265_lib_in(dir: &std::path::Path) -> Option<&'static De265> {
    DE265
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            unsafe {
                use windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW;
                let wide: Vec<u16> = dir.as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
                SetDllDirectoryW(wide.as_ptr());
            }
            let path = dir.join("libde265-0.dll");
            let lib = unsafe { libloading::Library::new(&path) }.ok()?;
            Some(De265 {
                new_decoder: *unsafe { lib.get(b"de265_new_decoder\0") }.ok()?,
                free_decoder: *unsafe { lib.get(b"de265_free_decoder\0") }.ok()?,
                push_data: *unsafe { lib.get(b"de265_push_data\0") }.ok()?,
                flush_data: *unsafe { lib.get(b"de265_flush_data\0") }.ok()?,
                decode: *unsafe { lib.get(b"de265_decode\0") }.ok()?,
                get_next_picture: *unsafe { lib.get(b"de265_get_next_picture\0") }.ok()?,
                get_image_width: *unsafe { lib.get(b"de265_get_image_width\0") }.ok()?,
                get_image_height: *unsafe { lib.get(b"de265_get_image_height\0") }.ok()?,
                get_image_plane: *unsafe { lib.get(b"de265_get_image_plane\0") }.ok()?,
                get_error_text: *unsafe { lib.get(b"de265_get_error_text\0") }.ok()?,
                _lib: lib,
            })
        })
        .as_ref()
}

/// HEIC → JPEG（RGB）。任何一步失败返回 None（调用方降级为原样透传）。
pub fn heic_to_jpeg(app: &tauri::AppHandle, data: &[u8]) -> Option<Vec<u8>> {
    let dir = crate::config::duanju_core_path(app)?.parent()?.to_path_buf();
    heic_to_jpeg_in(&dir, data)
}

/// 同上，DLL 目录由调用方给出（单测直击，不依赖 AppHandle）。
pub fn heic_to_jpeg_in(dir: &std::path::Path, data: &[u8]) -> Option<Vec<u8>> {
    let d = de265_lib_in(dir)?;
    let (annexb, _w, _h) = extract_annexb(data).ok()?;
    unsafe {
        let ctx = (d.new_decoder)();
        if ctx.is_null() {
            return None;
        }
        let rc = (d.push_data)(ctx, annexb.as_ptr() as *const c_void, annexb.len(), 0, std::ptr::null_mut());
        if rc != 0 {
            (d.free_decoder)(ctx);
            return None;
        }
        (d.flush_data)(ctx);
        let mut more: c_int = 0;
        for _ in 0..256 {
            let rc = (d.decode)(ctx, &mut more);
            if rc == 7 {
                // DE265_IMAGE_AVAILABLE
                break;
            }
        }
        let img = (d.get_next_picture)(ctx);
        if img.is_null() {
            (d.free_decoder)(ctx);
            return None;
        }
        let w = (d.get_image_width)(img, 0);
        let h = (d.get_image_height)(img, 0);
        if w <= 0 || h <= 0 || w > 4096 || h > 4096 {
            (d.free_decoder)(ctx);
            return None;
        }
        let mut ys: c_int = 0;
        let mut cbs: c_int = 0;
        let mut crs: c_int = 0;
        let y_plane = (d.get_image_plane)(img, 0, &mut ys);
        let cb_plane = (d.get_image_plane)(img, 1, &mut cbs);
        let cr_plane = (d.get_image_plane)(img, 2, &mut crs);
        if y_plane.is_null() || cb_plane.is_null() || cr_plane.is_null() {
            (d.free_decoder)(ctx);
            return None;
        }
        let (w, h) = (w as usize, h as usize);
        let (ys, cbs, crs) = (ys as usize, cbs as usize, crs as usize);
        let y = std::slice::from_raw_parts(y_plane, ys * h);
        let cb = std::slice::from_raw_parts(cb_plane, cbs * (h / 2));
        let cr = std::slice::from_raw_parts(cr_plane, crs * (h / 2));
        // YUV420 → RGB24（BT.601 full-range，与探针口径一致）
        let mut rgb = vec![0u8; w * h * 3];
        for row in 0..h {
            let yrow = &y[row * ys..row * ys + w];
            let crow = row / 2;
            let brow = &cb[crow * cbs..crow * cbs + w / 2];
            let rrow = &cr[crow * crs..crow * crs + w / 2];
            let out = &mut rgb[row * w * 3..(row + 1) * w * 3];
            for col in 0..w {
                let yv = yrow[col] as i32;
                let cbv = brow[col / 2] as i32 - 128;
                let crv = rrow[col / 2] as i32 - 128;
                let r = yv + 1402 * crv / 1000;
                let g = yv - 344 * cbv / 1000 - 714 * crv / 1000;
                let b = yv + 1772 * cbv / 1000;
                out[col * 3] = r.clamp(0, 255) as u8;
                out[col * 3 + 1] = g.clamp(0, 255) as u8;
                out[col * 3 + 2] = b.clamp(0, 255) as u8;
            }
        }
        (d.free_decoder)(ctx);
        let mut out = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82);
        encoder
            .encode(&rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)
            .ok()?;
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_heic() {
        assert!(!is_heic(b"\x00\x00\x00\x18ftypisomiso2"));
        assert!(!is_heic(b"not a heic at all"));
        assert!(extract_annexb(b"tiny").is_err());
    }

    #[test]
    fn decodes_real_sample_when_present() {
        // 样本来自开发期探针（gitignored tests/artifacts），缺失时跳过
        let sample = std::path::Path::new("../../tests/artifacts/color_probe/cover.heic");
        if !sample.is_file() {
            return;
        }
        let data = std::fs::read(sample).unwrap();
        let dir = std::path::Path::new("../../third_party/duanju");
        let out = heic_to_jpeg_in(dir, &data).expect("libde265 应可用并解出 JPEG");
        assert!(out.len() > 5000, "JPEG 输出过小: {}", out.len());
        assert_eq!(&out[..2], b"\xff\xd8", "JPEG 魔数不符");
        assert_eq!(&out[out.len() - 2..], b"\xff\xd9", "JPEG 尾部魔数不符");
    }

    #[test]
    fn rejects_truncated_containers() {
        // ftyp + heic brand 但没有 meta —— 必须报错而不是 panic
        let mut d = Vec::new();
        d.extend_from_slice(&[0, 0, 0, 0x18]);
        d.extend_from_slice(b"ftypheicmif1");
        d.extend_from_slice(&[0u8; 12]);
        assert!(extract_annexb(&d).is_err());
    }
}
