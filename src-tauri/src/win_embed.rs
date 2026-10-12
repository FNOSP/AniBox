//! Win32 子窗口嵌入：在 Tauri 主窗口内创建一个 WS_CHILD 窗口交给 mpv `--wid` 渲染。
//!
//! 层级（lc-088 定案，取 LumiPlayer 的「WebView 挖洞」方案）：
//! - **默认（裁洞形态）**：宿主窗沉到最底（HWND_BOTTOM），由 `win_region` 在 WebView2
//!   各层窗口上挖出「视频区」区域 → 视频从洞里透出，而 WebView 仍是**逻辑最上层**，
//!   页面 HTML 可以正常盖住视频区之外的一切，弹窗/抽屉不再需要手工「先藏视频」。
//!   洞对鼠标同样是穿透的 → mpv 依旧拿得到鼠标，原生 uosc 照常可用。
//! - **回退（`FNTV_CUTOUT=0`）**：宿主窗置顶（HWND_TOP，lc-082 结论的旧形态）——
//!   此时必须靠 `keep_top` 抢层级，且 HTML 盖不到视频。

#![cfg(windows)]

use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClassNameW, GetWindow, IsWindow,
    RegisterClassW, SetWindowPos, ShowWindow, CS_HREDRAW, CS_VREDRAW, GW_CHILD, GW_HWNDNEXT,
    HWND_BOTTOM, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE,
    SW_SHOW, WNDCLASSW, WS_CHILD, WS_CLIPSIBLINGS, WS_VISIBLE,
};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};

static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();
const CLASS_NAME: &str = "FntvMpvHost\0";

/// [lc-087] 不做鼠标穿透：内嵌的是**独立 mpv.exe 进程**，它自带消息循环、会把自己的子窗建在
/// 这个宿主窗里并正常接收鼠标（uosc 底栏/右键菜单/滚轮/双击全屏都是 mpv 原生行为）。
/// （进程内 libmpv 时代必须要的 HTTRANSPARENT 代收方案已随 lc-085/086 退役。）
///
/// [lc-088] 裁洞形态下宿主窗沉到 WebView **之下**，但洞是 SetWindowRgn 的**真区域**——
/// 区域同时裁命中测试，所以落在洞里的鼠标依然由本窗（进而由 mpv）接收，无需消息转发。
fn ensure_class() {
    CLASS_REGISTERED.get_or_init(|| unsafe {
        let cls: Vec<u16> = CLASS_NAME.encode_utf16().collect();
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(DefWindowProcW),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(), // 不自动擦底，交给 mpv 填充
            lpszMenuName: std::ptr::null(),
            lpszClassName: cls.as_ptr(),
        };
        RegisterClassW(&wc);
    });
}

pub fn create_child(parent_hwnd: isize) -> isize {
    ensure_class();
    unsafe {
        let cls: Vec<u16> = CLASS_NAME.encode_utf16().collect();
        let title: Vec<u16> = "\0".encode_utf16().collect();
        let hwnd = CreateWindowExW(
            0,
            cls.as_ptr(),
            title.as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            800,
            450,
            parent_hwnd as HWND,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );
        hwnd as isize
    }
}

/// 移动/缩放到指定物理像素矩形。
///
/// `on_top = false`（lc-088 裁洞形态，默认）：压到 WebView **之下**（HWND_BOTTOM），
/// 由 `win_region` 在 WebView 上挖洞让视频透出 —— WebView 仍是逻辑最上层。
/// `on_top = true`（`FNTV_CUTOUT=0` 回退形态，lc-082/087）：置顶压过 WebView，
/// 此时必须配合 `keep_top` 反复抢层级，且 HTML 无法覆盖视频。
pub fn set_bounds(hwnd: isize, x: i32, y: i32, w: i32, h: i32, on_top: bool) {
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            if on_top { HWND_TOP } else { HWND_BOTTOM },
            x,
            y,
            w.max(1),
            h.max(1),
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

pub fn set_visible(hwnd: isize, visible: bool) {
    unsafe {
        ShowWindow(hwnd as HWND, if visible { SW_SHOW } else { SW_HIDE });
    }
}

/// 重新置顶（仅供 `FNTV_CUTOUT=0` 回退形态：WebView 重绘/重排后偶尔会插到上面，显示时补一次）。
/// 裁洞形态**绝不能调用**——那会把宿主窗抬到 WebView 之上，洞就失去意义了。
pub fn keep_top(hwnd: isize) {
    unsafe {
        SetWindowPos(hwnd as HWND, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

pub fn destroy(hwnd: isize) {
    unsafe {
        DestroyWindow(hwnd as HWND);
    }
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// [lc-089] 重新启用 mpv 自建的子窗（`class == "mpv"`）。返回 true = 已找到该子窗。
///
/// **根因**：mpv 在 `--wid` 嵌入模式下**主动禁用**自己的窗口 ——
/// `video/out/w32_common.c:2163`：
/// ```c
/// if (w32->parent)
///     EnableWindow(w32->window, 0);
/// ```
/// 而 Windows 的命中测试会**跳过 disabled 窗口**，鼠标永远进不了 mpv，点击全部落到
/// 本宿主窗的 `DefWindowProcW`（什么都不做）→ 原生 uosc 底栏 / 右键菜单 / 双击全屏
/// 全部「点了没反应」（lc-089 已复现：真实点击下 mpv 无任何指令）。
///
/// **为什么可以从外部启用**：
/// ① mpv 只在 VO 初始化时禁一次，之后不会改回（全仓 `EnableWindow` 仅此一处调用）；
/// ② mpv 的 `WM_MOUSEMOVE` / `WM_LBUTTONDOWN/UP` / `WM_MOUSEWHEEL` 处理里**没有**
///    `w32->parent` 守卫（`w32_common.c:1607/1626/1647`；唯一的 parent 守卫在
///    `WM_NCCALCSIZE`(1678)，与输入无关）→ 启用后鼠标即可原生驱动 mpv。
/// 实测 A/B：启用前同一位置真实点击 → mpv 零指令；启用后 → `Run command: cycle pause`。
///
/// 注意：宿主窗创建时带 `WS_CLIPSIBLINGS` 且不含 `WS_DISABLED`，本身一直可命中；
/// 被跳过的是 mpv 那一层子窗。
pub fn enable_mpv_child(host: isize) -> bool {
    let host = host as HWND;
    if host.is_null() || unsafe { IsWindow(host) } == 0 {
        return false;
    }
    unsafe {
        let mut c = GetWindow(host, GW_CHILD);
        while !c.is_null() {
            if class_name(c) == "mpv" {
                if IsWindowEnabled(c) == 0 {
                    EnableWindow(c, 1);
                    crate::libmpv::trace("(mpv-child) 已重新启用 mpv 自建子窗 → 鼠标交互恢复");
                }
                return true;
            }
            c = GetWindow(c, GW_HWNDNEXT);
        }
    }
    false
}

/// 轮询等待 mpv 建好子窗并启用它（mpv 建窗是异步的，VO 初始化完成才出现）。
/// `tries` × 200ms 为最长等待。
pub fn spawn_enable_mpv_child(host: isize, tries: u32) {
    tokio::task::spawn(async move {
        for i in 0..tries {
            if enable_mpv_child(host) {
                if i > 0 {
                    crate::libmpv::trace(&format!("(mpv-child) 第 {} 次探测命中 mpv 子窗", i + 1));
                }
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        crate::libmpv::trace("(mpv-child) 轮询超时：未找到 mpv 子窗，鼠标交互可能不可用");
    });
}

// 保留类型引用，避免未使用告警（DefWindowProc 签名需要）
#[allow(dead_code)]
fn _sig(_: fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) {}
