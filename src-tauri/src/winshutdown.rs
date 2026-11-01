//! [lc-1900] Windows 会话结束（关机 / 重启 / 注销）兜底窗口。
//!
//! 为什么需要：tao 0.37 的 Windows 事件循环**不处理** `WM_QUERYENDSESSION`
//! （`platform_impl/windows/event_loop.rs` 里明确注释「还没实现」），到 `WM_ENDSESSION`
//! 时直接 `std::process::exit(0)`。也就是说关机时 AniBox 的 `RunEvent::Exit` 清理钩子
//! **根本不会执行**，mpv / PotPlayer 会留成孤儿进程挂在会话里；同时窗口若不在时限内
//! 响应 `WM_QUERYENDSESSION`，Windows 就会把它列进「这些应用阻止关机」。
//!
//! 做法：在 setup 阶段（主线程）建一个**隐藏的顶层窗口**，只用来接收会话结束广播：
//!   · `WM_QUERYENDSESSION`：标记 `session_ending` 后立刻返回 TRUE（允许关机）。
//!     这里刻意不做任何耗时操作、也不 spawn 外部进程 —— 拖过时限就会被系统判「阻止关机」。
//!   · `WM_ENDSESSION`：确认真的关机（wParam != 0）时硬杀已登记的子进程
//!     （`winproc::kill_tracked`，纯 TerminateProcess，微秒级）。
//!
//! 最后一道保险在 `winproc::assign_kill_on_close_job`：mpv 挂在 KILL_ON_JOB_CLOSE 的 job
//! 上，即使本窗口还没来得及处理消息（tao 先走到 `process::exit(0)`），mpv 也会被一起收掉。

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, TRUE, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, WNDCLASSW,
};

const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn session_watch_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_QUERYENDSESSION => {
            // 只置标记 + 放行，别在这里做任何重活（系统给的时间窗很短）。
            crate::winproc::mark_session_ending();
            TRUE as LRESULT
        }
        WM_ENDSESSION => {
            // wParam != 0 表示会话确实要结束（FALSE = 关机被取消，此时不必清进程）。
            if wparam != 0 {
                crate::winproc::kill_tracked();
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 注册会话结束监听窗口。只应在主线程（Tauri `setup`）调用一次。
pub fn install() {
    let class = wide("AniBoxSessionEndWatch");
    let title = wide("AniBoxSessionEndWatch");
    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(session_watch_proc);
        wc.hInstance = hinstance;
        wc.lpszClassName = class.as_ptr();
        // 返回 0 通常是「类名已注册」，不影响建窗。
        RegisterClassW(&wc);
        // 无 WS_VISIBLE 的顶层窗口：不显示、但照样收得到 HWND_BROADCAST 的会话结束消息。
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            crate::libmpv::trace("[lc-1900] 会话结束监听窗口创建失败（仅影响关机兜底）");
        } else {
            crate::libmpv::trace("[lc-1900] 会话结束监听窗口已注册");
        }
    }
}
