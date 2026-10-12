//! Win32 子窗口嵌入：在 Tauri 主窗口内创建一个 WS_CHILD 窗口交给 mpv `--wid` 渲染，
//! 并用 SetWindowPos(HWND_BOTTOM) 把它压到 WebView 之下 —— 透明 WebView 顶层画控制栏，
//! 视频区域在 HTML 里保持透明即可看到 mpv 画面（双层窗口架构，见 resource/wiki/Tauri2迁移方案.md）。

#![cfg(windows)]

use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos, ShowWindow,
    CS_HREDRAW, CS_VREDRAW, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SW_HIDE, SW_SHOW, WNDCLASSW, WS_CHILD, WS_CLIPSIBLINGS, WS_VISIBLE,
};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};

static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();
const CLASS_NAME: &str = "FntvMpvHost\0";

/// [lc-087] 不做鼠标穿透：内嵌的是**独立 mpv.exe 进程**，它自带消息循环、会把自己的子窗建在
/// 这个宿主窗里并正常接收鼠标（uosc 底栏/右键菜单/滚轮/双击全屏都是 mpv 原生行为）。
/// （进程内 libmpv 时代必须要的 HTTRANSPARENT 代收方案已随 lc-085/086 退役。）
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

/// 移动/缩放到指定物理像素矩形，并压到 WebView 之上
///
/// lc-082 实测：WebView2 窗口化承载时其渲染子窗（Chrome_WidgetWin_*/Intermediate D3D Window）
/// 始终盖在同一父窗口的兄弟子窗上，页面再怎么透明也透不出「下面的」自定义子窗口
/// （页面隐藏后视频仍不可见）。因此 mpv 窗口必须置于 WebView **之上**。
/// lc-083：不再做鼠标穿透（HTTRANSPARENT）——底栏/交互交给 mpv 侧的 uosc（主仓库同款），
/// uosc 需要鼠标事件（左键暂停、双击全屏、右键菜单、悬停出底栏）。
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
