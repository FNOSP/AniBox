//! [lc-1907] mpv 窗口 owner 收编（原型 v1）：让 mpv 独立窗口视觉上"挂"进主窗口。
//!
//! 三件事：
//!   1. adopt：mpv 起来后把它按 PID 找到的可见顶层窗口 `SetWindowLongPtrW(GWLP_HWNDPARENT)`
//!      挂成主窗口的 **owned window**（Windows 语义：owned 永远压在 owner 之上、owner 最小化
//!      一起收、owner 销毁一起销毁），再叠 DWM 系统圆角（复用 lib.rs 的 winvisual）。
//!      只动"从属关系"，**不做** SetParent / WS_CHILD / 挖洞（内嵌是另一条路线，另案）；
//!      也不动 mpv 窗口的 ex-style。
//!   2. follow：主窗口 Moved 时按**位移差**平移 mpv 窗口 —— Windows 不会让 owned 窗口自动
//!      跟随 owner 移动，必须自己补。只跟随移动；Resized / ScaleFactorChanged（缩放跟随）
//!      v1 刻意不做：改 mpv 窗口大小容易和用户手动调整 mpv 窗口打架。全屏 / 最大化主窗口
//!      同样会触发 Moved，v1 接受跟随（实测问题大再加判断）。
//!   3. 槽：记录当前已收编的 mpv HWND，供 Moved 分支查询；mpv 退出（IsWindow 失效）时
//!      静默清理，不影响后续新会话重新收编。
//!
//! 全程零 IPC 依赖、失败只打 trace 不报错：任何一步失败，行为都退化为今天的独立窗口。

use std::sync::Mutex;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    SetWindowLongPtrW, SetWindowPos, GWLP_HWNDPARENT, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
};

/// 当前已收编的 mpv 顶层窗口（isize，全仓 HWND 约定；None = 未收编 / 已失效清理）。
static MPV_HWND: Mutex<Option<isize>> = Mutex::new(None);
/// 上一次主窗口物理位置（Moved 求位移差用；None = 首跳，忽略绝对跳变）。
static LAST_MAIN_POS: Mutex<Option<(i32, i32)>> = Mutex::new(None);

// ───────────────────────── 查找 mpv 顶层窗口 ─────────────────────────

struct EnumCtx {
    pid: u32,
    found: Option<isize>,
}

/// EnumWindows 回调：按 PID 比对 + 只认可见窗口，命中即停（返回 0 终止枚举）。
unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = unsafe { &mut *(lparam as *mut EnumCtx) };
    let mut proc_id: u32 = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut proc_id);
        if proc_id == ctx.pid && IsWindowVisible(hwnd) != 0 {
            ctx.found = Some(hwnd as isize);
            return 0;
        }
    }
    1
}

/// 按 PID 找 mpv 的可见顶层窗口（mpv 同 PID 的可见顶层窗口通常只有一个，取第一个）。
pub fn find_mpv_hwnd(pid: u32) -> Option<isize> {
    let mut ctx = EnumCtx { pid, found: None };
    unsafe {
        // 返回 0 有两种可能：真出错，或回调提前返回 0（找到了）——以后者为准，直接看 found。
        EnumWindows(Some(enum_proc), &mut ctx as *mut EnumCtx as LPARAM);
    }
    ctx.found
}

// ───────────────────────── 收编 / 槽 ─────────────────────────

/// 把 mpv 顶层窗口挂成 `owner_hwnd` 的 owned window 并加 DWM 系统圆角。
/// 成功后把 mpv HWND 写入槽（follow_move 的查询源）。
pub fn adopt(pid: u32, owner_hwnd: isize) -> Result<isize, String> {
    let hwnd = find_mpv_hwnd(pid).ok_or_else(|| "未找到 mpv 的可见顶层窗口".to_string())?;
    unsafe {
        // GWLP_HWNDPARENT：设 owner（返回旧 owner）。mpv 与主窗口同线程无要求，
        // 跨线程设 owner 是允许的（影响的是消息路由与生命周期，不影响谁的线程建窗）。
        let old = SetWindowLongPtrW(hwnd as _, GWLP_HWNDPARENT, owner_hwnd);
        crate::libmpv::trace(&format!(
            "[lc-1907] GWLP_HWNDPARENT：{old} → {owner_hwnd}（mpv hwnd={hwnd} pid={pid}）"
        ));
    }
    // DWM 系统圆角：复用主窗口同一套调用（DWMWCP_ROUND + 关 DWM 边框）。
    // mpv 本来 border=no，边框关不关无感；圆角让它与主窗口观感一致。
    crate::winvisual::set_system_round_corner(hwnd);
    set_hwnd(hwnd);
    Ok(hwnd)
}

/// 写槽（收编成功后调用；重收编时覆盖旧值）。
pub fn set_hwnd(hwnd: isize) {
    MPV_HWND
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(hwnd);
}

/// 清槽（mpv 退出 / 窗口失效时）。
pub fn clear_hwnd() {
    *MPV_HWND.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// 读槽：仅当窗口仍有效（IsWindow）才返回；失效则静默清槽并回 None。
pub fn get_hwnd() -> Option<isize> {
    let mut guard = MPV_HWND.lock().unwrap_or_else(|e| e.into_inner());
    match *guard {
        Some(h) if unsafe { IsWindow(h as _) != 0 } => Some(h),
        Some(h) => {
            // mpv 已退出 / 句柄失效：静默清理，等下次 launch 重新收编
            crate::libmpv::trace(&format!("[lc-1907] 槽内 mpv hwnd={h} 已失效，清槽"));
            *guard = None;
            None
        }
        None => None,
    }
}

// ───────────────────────── 跟随移动 ─────────────────────────

/// 按位移差平移 mpv 窗口（物理像素）。窗口已失效（IsWindow 为假）时静默清槽。
/// 用 GetWindowRect 取当前位置 + SWP_NOSIZE：SetWindowPos 的 x/y 是**绝对坐标**，
/// 所以"跟随"= 当前位置 + 位移差，而不是把位移差直接当坐标传。
pub fn follow_move(mpv_hwnd: isize, dx: i32, dy: i32) {
    if dx == 0 && dy == 0 {
        return;
    }
    unsafe {
        if IsWindow(mpv_hwnd as _) == 0 {
            clear_hwnd();
            return;
        }
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(mpv_hwnd as _, &mut rc) == 0 {
            return;
        }
        SetWindowPos(
            mpv_hwnd as _,
            std::ptr::null_mut(),
            rc.left + dx,
            rc.top + dy,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE,
        );
    }
}

/// 主窗口 Moved 入口（lib.rs `.on_window_event` 调用，同步上下文）：
/// 与上次主窗口物理位置求位移差后平移已收编的 mpv 窗口。
/// 首跳（上次位置为 None）忽略——应用启动 / 从托盘恢复时的绝对跳变不应拖动 mpv。
/// Moved 事件拖动期高频触发，这里**不做逐事件 trace**（会刷爆诊断日志）。
pub fn on_main_moved(x: i32, y: i32) {
    let mut last = LAST_MAIN_POS.lock().unwrap_or_else(|e| e.into_inner());
    let Some((lx, ly)) = *last else {
        *last = Some((x, y));
        return;
    };
    *last = Some((x, y));
    let (dx, dy) = (x - lx, y - ly);
    drop(last);
    if dx == 0 && dy == 0 {
        return;
    }
    if let Some(h) = get_hwnd() {
        follow_move(h, dx, dy);
    }
}
