//! Win32 子窗口嵌入：在 Tauri 主窗口内创建一个 WS_CHILD 窗口交给 mpv `--wid` 渲染，
//! 并用 SetWindowPos(HWND_BOTTOM) 把它压到 WebView 之下 —— 透明 WebView 顶层画控制栏，
//! 视频区域在 HTML 里保持透明即可看到 mpv 画面（双层窗口架构，见 resource/wiki/Tauri2迁移方案.md）。

#![cfg(windows)]

use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos, ShowWindow,
    CS_HREDRAW, CS_VREDRAW, HWND_TOP, HTTRANSPARENT, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, SW_HIDE, SW_SHOW, WM_NCHITTEST, WNDCLASSW, WS_CHILD, WS_CLIPSIBLINGS,
    WS_VISIBLE,
};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};

static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();
const CLASS_NAME: &str = "FntvMpvHost\0";

/// 鼠标穿透：窗口置于 WebView 之上，但命中测试返回 HTTRANSPARENT，
/// 让悬停/点击/双击（如舞台双击全屏）继续由下方页面处理。
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST {
        return HTTRANSPARENT as LRESULT;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn ensure_class() {
    CLASS_REGISTERED.get_or_init(|| unsafe {
        let cls: Vec<u16> = CLASS_NAME.encode_utf16().collect();
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
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

/// 移动/缩放到指定物理像素矩形，并压到 WebView 之上
///
/// lc-082 实测：WebView2 窗口化承载时其渲染子窗（Chrome_WidgetWin_*/Intermediate D3D Window）
/// 始终盖在同一父窗口的兄弟子窗上，页面再怎么透明也透不出「下面的」自定义子窗口
/// （页面隐藏后视频仍不可见）。因此 mpv 窗口必须置于 WebView **之上**；
/// 配合 WM_NCHITTEST=HTTRANSPARENT 鼠标穿透，页面照常收到悬停/点击事件。
pub fn set_bounds(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            HWND_TOP,
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

/// 重新置顶（WebView 重绘/重排后偶尔会插到上面，显示时补一次）
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

// 保留类型引用，避免未使用告警（DefWindowProc 签名需要）
#[allow(dead_code)]
fn _sig(_: fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) {}
