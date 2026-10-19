//! [webview2] WebView2 Runtime 检测 + 应用内「下载安装」兜底入口。
//!
//! 为什么需要它：Tauri 在 Windows 上依赖 WebView2 Runtime（Evergreen）。
//! 系统缺失时 `WebView2Loader` 创建环境失败，进程要么直接退出、要么白屏 ——
//! 此时**应用内的任何 UI 都不会出现**，所以这里提供的是**双保险**：
//!   1. 应用能跑起来（已安装但版本过旧 / 检测误报 / 修复场景）→ 设置「关于」页展示状态 + 下载按钮；
//!   2. 应用完全起不来 → 仓库根目录的 `安装WebView2.cmd`（调用 `tools/ensure-webview2.ps1`）先装运行时。
//!
//! 检测口径（两者任一命中即视为已安装，版本优先取注册表）：
//!   · 注册表 EdgeUpdate 客户端（WebView2 Evergreen 的稳定登记位置）：
//!       HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F1E7B9A0-...}\pv
//!       HKLM\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F1E7B9A0-...}\pv
//!     实现上**不硬编码完整 GUID**：枚举 Clients 下的子键，取 `name` 含 "WebView2" 的项
//!     （GUID 前缀 `{F1E7B9A0` 作为补充判据），避免微软更换 Client ID 后检测整体失效。
//!   · 安装目录：
//!       %ProgramFiles(x86)%\Microsoft\EdgeWebView\Application\<version>\msedgewebview2.exe
//!       %ProgramFiles%\Microsoft\EdgeWebView\Application\<version>\msedgewebview2.exe
//!       %LOCALAPPDATA%\Microsoft\EdgeWebView\Application\<version>\msedgewebview2.exe（按用户安装）
//!
//! 下载地址：官方 Evergreen Bootstrapper（短链，始终指向最新版）：
//!   https://go.microsoft.com/fwlink/p/?LinkId=2124703
//! 另附官方 x64 离线安装包直链（体积大、适合无网/内网分发）：
//!   https://go.microsoft.com/fwlink/?linkid=2124701
//! 说明：微软**没有**提供官方国内镜像；第三方镜像（各类 ghproxy/网盘）可用性不稳定且
//! 可能被篡改，故不内置。国内直连 go.microsoft.com 一般可达；若确实不可达，请用离线包
//! 或企业内网分发。安装包的 `webviewInstallMode` 同样走官方地址（见 tauri.conf.json）。

use serde::Serialize;

/// 官方 Evergreen Bootstrapper（约 1.8 MB，运行后按需下载完整运行时）。
pub const DOWNLOAD_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";
/// 官方 Evergreen 离线安装包（x64，约 150 MB；无网环境用）。
pub const STANDALONE_X64_URL: &str = "https://go.microsoft.com/fwlink/?linkid=2124701";
/// 官方文档（手动下载入口 / 版本说明）。
pub const DOCS_URL: &str = "https://learn.microsoft.com/zh-cn/microsoft-edge/webview2/";
/// Tauri v2 要求的最低 WebView2 版本（低于它时安装包/应用行为不可靠）。
/// 来源：Tauri v2 文档的 Windows 支持矩阵（WebView2 109+）。
pub const MINIMUM_VERSION: &str = "109.0.0.0";

/// 前端契约（camelCase）。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Webview2Status {
    /// 是否检测到可用的 WebView2 Runtime。
    pub installed: bool,
    /// 已安装但版本低于 [`MINIMUM_VERSION`]（需要引导升级）。
    pub outdated: bool,
    /// 版本号（注册表 `pv` 优先；否则取安装目录名）。检测不到时为空串。
    pub version: String,
    /// 本项目要求的最低版本（前端文案用）。
    pub minimum_version: String,
    /// `msedgewebview2.exe` 全路径；仅注册表命中时可能为空串。
    pub path: String,
    /// 官方 Evergreen Bootstrapper 下载地址。
    pub download_url: String,
    /// 官方 x64 离线安装包地址。
    pub standalone_url: String,
    /// 官方文档地址。
    pub docs_url: String,
    /// 命中来源：`registry` / `directory` / `none` / `not-required`（非 Windows）。
    pub source: String,
    /// 当前平台：`windows` / 其他。
    pub platform: String,
}

impl Webview2Status {
    /// 非 Windows 平台：WebView2 不是依赖项（仅在非 Windows 构建中调用）。
    #[allow(dead_code)]
    fn not_required() -> Self {
        Self {
            installed: true,
            outdated: false,
            version: String::new(),
            minimum_version: MINIMUM_VERSION.to_string(),
            path: String::new(),
            download_url: String::new(),
            standalone_url: String::new(),
            docs_url: DOCS_URL.to_string(),
            source: "not-required".to_string(),
            platform: std::env::consts::OS.to_string(),
        }
    }

    fn missing() -> Self {
        Self {
            installed: false,
            outdated: false,
            version: String::new(),
            minimum_version: MINIMUM_VERSION.to_string(),
            path: String::new(),
            download_url: DOWNLOAD_URL.to_string(),
            standalone_url: STANDALONE_X64_URL.to_string(),
            docs_url: DOCS_URL.to_string(),
            source: "none".to_string(),
            platform: "windows".to_string(),
        }
    }
}

/// 检测本机 WebView2 Runtime。
///
/// 纯读取、无副作用；失败（权限/注册表不可读）按「未检测到」处理并回退到目录探测，
/// 不向前端抛错 —— 检测失败不应该让设置面板整体报错。
#[tauri::command]
pub fn check_webview2() -> Webview2Status {
    #[cfg(target_os = "windows")]
    {
        detect_windows()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Webview2Status::not_required()
    }
}

// ───────────────────────── Windows 实现 ─────────────────────────

#[cfg(target_os = "windows")]
fn detect_windows() -> Webview2Status {
    let mut status = Webview2Status::missing();

    // 1) 注册表：版本最权威（目录可能存在多版本残留）。
    if let Some((version, _name)) = registry_webview2() {
        status.installed = true;
        status.version = version;
        status.source = "registry".to_string();
    }

    // 2) 安装目录：补 `path`；注册表没读到版本时用目录名兜底。
    if let Some((exe, dir_version)) = find_install_exe() {
        status.installed = true;
        status.path = exe;
        if status.version.is_empty() {
            status.version = dir_version;
            status.source = "directory".to_string();
        }
    }

    status.installed = status.installed || !status.version.is_empty();
    status.outdated = status.installed
        && !status.version.is_empty()
        && version_key(&status.version) < version_key(MINIMUM_VERSION);

    status
}

/// 枚举 EdgeUpdate 客户端键，返回 (pv, name)。
///
/// 先查 WOW6432Node（x64 系统上 WebView2 固定装在这里），再查原生 64 位视图；
/// 两侧都可能有其它 Edge 通道（Stable/Beta/Dev/Canary），按 `name` 过滤。
#[cfg(target_os = "windows")]
fn registry_webview2() -> Option<(String, String)> {
    const ROOTS: [&str; 2] = [
        r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients",
        r"SOFTWARE\Microsoft\EdgeUpdate\Clients",
    ];

    let mut best: Option<(Vec<u64>, String, String)> = None;
    for root in ROOTS {
        for (key_name, name, pv) in enumerate_clients(root) {
            if !is_webview2_client(&key_name, &name) || pv.is_empty() {
                continue;
            }
            let vk = version_key(&pv);
            let better = match &best {
                None => true,
                Some((cur, _, _)) => vk > *cur,
            };
            if better {
                best = Some((vk, pv, name));
            }
        }
    }
    best.map(|(_, pv, name)| (pv, name))
}

/// 判断某个 EdgeUpdate 客户端项是不是 WebView2。
///
/// 主判据是 `name` 里含 "webview2"（不同语言/版本下大小写可能不同）；
/// 次判据是 Client ID 前缀 `{F1E7B9A0`（WebView2 Evergreen 的历史 GUID 前缀）。
#[cfg(target_os = "windows")]
fn is_webview2_client(key_name: &str, name: &str) -> bool {
    name.to_ascii_lowercase().contains("webview2")
        || key_name.to_ascii_uppercase().starts_with("{F1E7B9A0")
}

/// 测试里在非 Windows 平台也会调用，故不加 `cfg`。
#[allow(dead_code)]
#[cfg(not(target_os = "windows"))]
fn is_webview2_client(key_name: &str, name: &str) -> bool {
    name.to_ascii_lowercase().contains("webview2")
        || key_name.to_ascii_uppercase().starts_with("{F1E7B9A0")
}

#[cfg(target_os = "windows")]
fn enumerate_clients(root: &str) -> Vec<(String, String, String)> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    };

    let to_wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let mut out = Vec::new();

    let root_w = to_wide(root);
    let mut hroot: HKEY = std::ptr::null_mut();
    let rc = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            root_w.as_ptr(),
            0,
            KEY_READ,
            &mut hroot,
        )
    };
    if rc != 0 {
        return out;
    }

    let mut index = 0u32;
    loop {
        let mut name_buf = [0u16; 512];
        let mut name_len = name_buf.len() as u32;
        let rc = unsafe {
            RegEnumKeyExW(
                hroot,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            // ERROR_NO_MORE_ITEMS(259) 或其它错误：停止枚举即可。
            break;
        }
        index += 1;
        let sub = String::from_utf16_lossy(&name_buf[..name_len as usize]);
        if sub.is_empty() {
            continue;
        }

        let mut hsub: HKEY = std::ptr::null_mut();
        let sub_w = to_wide(&sub);
        let rc = unsafe { RegOpenKeyExW(hroot, sub_w.as_ptr(), 0, KEY_READ, &mut hsub) };
        if rc != 0 {
            continue;
        }
        let name = query_string(hsub, "name");
        let pv = query_string(hsub, "pv");
        unsafe { RegCloseKey(hsub) };
        out.push((sub, name, pv));
    }

    unsafe { RegCloseKey(hroot) };
    out
}

/// 读一个 REG_SZ 值（空/不存在/类型不符都返回空串）。
#[cfg(target_os = "windows")]
fn query_string(hkey: windows_sys::Win32::System::Registry::HKEY, value: &str) -> String {
    use windows_sys::Win32::System::Registry::{RegQueryValueExW, REG_SZ};

    let to_wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let name_w = to_wide(value);

    let mut ty: u32 = 0;
    let mut len: u32 = 0;
    // 第一次取长度（数据为 NULL 是合法的）。
    let rc = unsafe {
        RegQueryValueExW(
            hkey,
            name_w.as_ptr(),
            std::ptr::null(),
            &mut ty,
            std::ptr::null_mut(),
            &mut len,
        )
    };
    if rc != 0 || len == 0 || len > 64 * 1024 {
        return String::new();
    }

    let mut buf = vec![0u8; len as usize];
    let rc = unsafe {
        RegQueryValueExW(
            hkey,
            name_w.as_ptr(),
            std::ptr::null(),
            &mut ty,
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    if rc != 0 {
        return String::new();
    }
    // 只接受字符串类型；其它类型（DWORD 等）忽略。
    if ty != REG_SZ && ty != 2 /* REG_EXPAND_SZ */ {
        return String::new();
    }
    let mut len = (len as usize).min(buf.len());
    // 去掉结尾的 NUL（可能有多个）。
    while len > 0 && buf[len - 1] == 0 {
        len -= 1;
    }
    let u16s: Vec<u16> = buf[..len - (len % 2)]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u16s).trim().to_string()
}

/// 在常见安装根目录下找 `<version>\msedgewebview2.exe`，返回 (exe 路径, 版本目录名)。
#[cfg(target_os = "windows")]
fn find_install_exe() -> Option<(String, String)> {
    use std::path::PathBuf;

    let mut roots: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(var) {
            if !base.trim().is_empty() {
                roots.push(PathBuf::from(base).join(r"Microsoft\EdgeWebView\Application"));
            }
        }
    }

    let mut best: Option<(Vec<u64>, String, String)> = None;
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let exe = path.join("msedgewebview2.exe");
            if !exe.is_file() {
                continue;
            }
            let dir_version = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let vk = version_key(&dir_version);
            let better = match &best {
                None => true,
                Some((cur, _, _)) => vk > *cur,
            };
            if better {
                best = Some((
                    vk,
                    exe.to_string_lossy().to_string(),
                    dir_version,
                ));
            }
        }
    }
    best.map(|(_, exe, ver)| (exe, ver))
}

/// 把 `120.0.2210.91` 之类的版本串转成可比较的数字序列；非数字段按 0 处理。
#[allow(dead_code)]
fn version_key(v: &str) -> Vec<u64> {
    v.trim()
        .split('.')
        .map(|p| {
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

// ───────────────────────── 测试 ─────────────────────────
//
// 只测纯函数（不碰注册表/磁盘），保证 `cargo test --lib` 在任何机器上都能跑。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_key_orders_numerically() {
        assert!(version_key("120.0.2210.91") > version_key("120.0.2210.9"));
        assert!(version_key("120.0.2210.91") > version_key("119.0.3000.1"));
        assert_eq!(version_key("1.2.3.4"), vec![1, 2, 3, 4]);
        assert_eq!(version_key(""), vec![0]);
        // 非数字后缀只取前导数字（"91-beta" → 91）
        assert_eq!(version_key("91-beta"), vec![91]);
    }

    #[test]
    fn webview2_client_matching() {
        // 主判据：显示名
        assert!(is_webview2_client(
            "{00000000-0000-0000-0000-000000000000}",
            "Microsoft Edge WebView2 Runtime"
        ));
        assert!(is_webview2_client("anything", "webview2 runtime"));
        // 次判据：历史 GUID 前缀
        assert!(is_webview2_client("{F1E7B9A0-8D4C-4D3A-BA5C-000000000000}", ""));
        // 其它 Edge 通道不应误判
        assert!(!is_webview2_client(
            "{56EB18F8-B008-4CBD-B6D2-8C97FE7E9062}",
            "Microsoft Edge"
        ));
    }

    #[test]
    fn non_windows_status_is_not_required() {
        // 仅校验结构默认值：非 Windows 平台不要求 WebView2。
        let s = Webview2Status::not_required();
        assert!(s.installed);
        assert_eq!(s.source, "not-required");
        assert!(s.download_url.is_empty());
    }

    #[test]
    fn missing_status_exposes_download_urls() {
        let s = Webview2Status::missing();
        assert!(!s.installed);
        assert_eq!(s.download_url, DOWNLOAD_URL);
        assert_eq!(s.standalone_url, STANDALONE_X64_URL);
        assert!(s.download_url.starts_with("https://"));
    }
}
