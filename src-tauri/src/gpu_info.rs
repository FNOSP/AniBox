//! [lc-2244] 显卡名探测（进程内 Win32 `EnumDisplayDevicesW`）。
//!
//! 原先走 `pwsh Get-ItemProperty` 查注册表显示适配器类，问题与解压同源：pwsh 是
//! PowerShell 7，Windows 不自带。干净机器上探测直接 ENOENT，`detect_gpu_name()`
//! 默默返回空串 —— 设置面板「当前显卡：xxx」整行消失，用户看不到 RIFE 的显卡门槛
//! 提示，却没有任何报错可循。
//!
//! 改走 user32 的 `EnumDisplayDevicesW`：系统自带 API、零外部进程、随取随用。
//! 非 Windows 平台返回空串（该提示只对 Windows 的 AI 组件有意义）。

/// 返回主显示适配器名（如 `NVIDIA GeForce RTX 5070`）。取不到返回空串。
///
/// 只认**物理挂载**的适配器：跳过 `DISPLAY_DEVICE_MIRRORING_DRIVER`（镜像驱动，
/// 名字常是空或 "Mirror Driver"）与 `DISPLAY_DEVICE_ATTACHED_TO_DESKTOP` 之外的项。
/// 多卡机器取第一块，与原先注册表实现的取数口径一致。
#[cfg(target_os = "windows")]
pub fn gpu_name() -> String {
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayDevicesW, DISPLAY_DEVICEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP,
        DISPLAY_DEVICE_MIRRORING_DRIVER,
    };

    let mut out = String::new();
    let mut i = 0u32;
    loop {
        let mut dd: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
        dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
        let ok = unsafe { EnumDisplayDevicesW(std::ptr::null(), i, &mut dd, 0) };
        if ok == 0 {
            break; // 枚举结束
        }
        i += 1;

        let flags = dd.StateFlags;
        if flags & DISPLAY_DEVICE_MIRRORING_DRIVER != 0 {
            continue;
        }
        if flags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
            continue;
        }
        // DeviceString 是显卡名（UTF-16、NUL 结尾、定长缓冲）
        let name = decode_wide(&dd.DeviceString);
        if name.is_empty() {
            continue;
        }
        let low = name.to_lowercase();
        // 虚拟 / 远程 / 基础显示适配器一律跳过（与原先的过滤口径一致）
        if low.contains("virtual") || low.contains("remote") || low.contains("basic") {
            continue;
        }
        out = name;
        break;
    }
    out
}

#[cfg(not(target_os = "windows"))]
pub fn gpu_name() -> String {
    String::new()
}

/// 定长 UTF-16 缓冲 → String（在首个 NUL 处截断）。
#[cfg(target_os = "windows")]
fn decode_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn decode_wide_stops_at_nul() {
        let mut buf = [0u16; 8];
        for (i, c) in "RTX 5070".encode_utf16().enumerate() {
            buf[i] = c;
        }
        assert_eq!(decode_wide(&buf), "RTX 5070");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn decode_wide_all_zero_is_empty() {
        assert_eq!(decode_wide(&[0u16; 16]), "");
    }

    #[test]
    fn gpu_name_never_panics() {
        // 只是取值，任何机器（含无显卡的 CI）都不得 panic。
        let _ = gpu_name();
    }
}
