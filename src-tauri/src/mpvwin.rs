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
//!
//! [lc-1909] 软件内播放（内嵌模式）：mpv 独立窗口被精确钉到前端上报的视频区域矩形上，
//! 视觉上嵌在主窗口内。与 lc-1907 的 owner 收编是**两条独立路径**：
//!   · owner 收编（`mpv_window_owner=true`，实验开关，默认 false）：independent 窗口挂 owner
//!     + Moved 跟随，窗口形态保持原生；
//!   · 内嵌（`player_embed_enter/rect/leave`，前端仅在 player_open_mode="app" 时调用）：
//!     同样挂 owner（保 z-order：owned 永远压在主窗口之上），但窗口矩形由前端矩形上报
//!     精确控制；收编成功后**先隐藏**，首个矩形上报时才定位 + 显示（独立窗口不闪现）。
//!     仍不做 SetParent / WS_CHILD / 挖洞，也不动 mpv 窗口 ex-style。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_HWNDPARENT, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
};

/// 当前已收编的 mpv 顶层窗口（isize，全仓 HWND 约定；None = 未收编 / 已失效清理）。
static MPV_HWND: Mutex<Option<isize>> = Mutex::new(None);
/// 上一次主窗口物理位置（Moved 求位移差用；None = 首跳，忽略绝对跳变）。
static LAST_MAIN_POS: Mutex<Option<(i32, i32)>> = Mutex::new(None);

// ───────────────────────── [lc-1909] 内嵌模式静态槽 ─────────────────────────

/// 内嵌会话激活标记：`player_embed_enter` 置位 / `player_embed_leave` 清除。
static EMBED_ACTIVE: AtomicBool = AtomicBool::new(false);
/// mpv 窗口当前是否被内嵌流程隐藏（enter 收编成功后隐藏，首个矩形上报时定位 + 显示）。
static EMBED_HIDDEN: AtomicBool = AtomicBool::new(false);
/// Moved 跟随开关：**仅** mpv_window_owner=true 的收编路径（mpv.rs `adopt_window_owner`）
/// 成功后置 true。默认 false = 独立窗口模式不跟随（lc-1906 原行为）；
/// 内嵌跟随不依赖它（走 EMBED_ACTIVE）。
static OWNER_FOLLOW: AtomicBool = AtomicBool::new(false);
/// 收编完成前上报的内嵌矩形缓存（**屏幕物理坐标**）：hwnd 未就绪时 rect 命令先落这里，
/// `embed_adopt` 成功后隐藏补施。高频 rect 命令只做一次 Mutex 写，足够轻量。
static EMBED_RECT: Mutex<Option<(i32, i32, i32, i32)>> = Mutex::new(None);
/// [lc-1911] 进内嵌前的 mpv `script-opts` 原串（leave 时还原，独立窗口的 uosc 顶栏要回来）。
static SAVED_SCRIPT_OPTS: Mutex<Option<String>> = Mutex::new(None);

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
    crate::libmpv::trace(&format!("[lc-1907] 收编：mpv hwnd={hwnd} pid={pid}"));
    adopt_hwnd(hwnd, owner_hwnd);
    Ok(hwnd)
}

/// 对已知 hwnd 挂 owner + DWM 系统圆角 + 写槽（`adopt` 与 `embed_adopt` 共用本体）。
fn adopt_hwnd(hwnd: isize, owner_hwnd: isize) {
    unsafe {
        // GWLP_HWNDPARENT：设 owner（返回旧 owner）。mpv 与主窗口同线程无要求，
        // 跨线程设 owner 是允许的（影响的是消息路由与生命周期，不影响谁的线程建窗）。
        let old = SetWindowLongPtrW(hwnd as _, GWLP_HWNDPARENT, owner_hwnd);
        crate::libmpv::trace(&format!(
            "[lc-1907] GWLP_HWNDPARENT：{old} → {owner_hwnd}（mpv hwnd={hwnd}）"
        ));
    }
    // DWM 系统圆角：复用主窗口同一套调用（DWMWCP_ROUND + 关 DWM 边框）。
    // mpv 本来 border=no，边框关不关无感；圆角让它与主窗口观感一致。
    crate::winvisual::set_system_round_corner(hwnd);
    set_hwnd(hwnd);
}

/// 写槽（收编成功后调用；重收编时覆盖旧值）。
pub fn set_hwnd(hwnd: isize) {
    MPV_HWND
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(hwnd);
}

/// [lc-1913] 把 mpv 窗口满屏钉到播放窗矩形上（物理像素，绝对定位，SWP_NOACTIVATE
/// 不抢焦点）。[lc-1921] 去掉 overscan：钉位只在收编时执行一次，播放窗此后被动跟随
/// mpv，不存在需要盖缝的双向同步（overscan 曾在 Resized 正反馈里导致无限放大）。
pub fn pin_to_owner(owner_hwnd: isize) {
    let Some(h) = get_hwnd() else { return };
    unsafe {
        if IsWindow(h as _) == 0 || IsWindow(owner_hwnd as _) == 0 {
            return;
        }
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(owner_hwnd as _, &mut rc) == 0 {
            return;
        }
        SetWindowPos(
            h as _,
            std::ptr::null_mut(),
            rc.left,
            rc.top,
            rc.right - rc.left,
            rc.bottom - rc.top,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
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

// ───────────────────────── [lc-1909] 内嵌模式：收编 / 矩形钉位 ─────────────────────────

/// 内嵌会话是否激活（player_embed_enter 置位 / leave 清除）。
pub fn embed_is_active() -> bool {
    EMBED_ACTIVE.load(Ordering::SeqCst)
}

/// 置 / 清内嵌激活标记；清除时一并丢弃待补施矩形与隐藏标记（退嵌后不留残态）。
pub fn embed_set_active(on: bool) {
    EMBED_ACTIVE.store(on, Ordering::SeqCst);
    if !on {
        *EMBED_RECT.lock().unwrap_or_else(|e| e.into_inner()) = None;
        EMBED_HIDDEN.store(false, Ordering::SeqCst);
    }
}

/// Moved 跟随开关（仅 mpv_window_owner=true 的收编路径置 true，见 OWNER_FOLLOW 注释）。
pub fn set_owner_follow(on: bool) {
    OWNER_FOLLOW.store(on, Ordering::SeqCst);
}

/// [lc-1911] 存 / 取进内嵌前的 mpv `script-opts` 原串（leave 还原用；None=之前没存过）。
pub fn save_script_opts(orig: String) {
    *SAVED_SCRIPT_OPTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(orig);
}

pub fn take_saved_script_opts() -> Option<String> {
    SAVED_SCRIPT_OPTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// [lc-1915] 跟随循环（Polaris 同款架构的另一半）：拖动画面 = mpv 自己动（窗口拖拽），
/// 本循环 33ms 轮询 mpv 矩形，把 `mpv_overlay` 窗口贴到 mpv 矩形上（z 序插在 mpv 正后方）。
/// mpv 是锚、控制层窗是随动 —— 与 Polaris 的 WorkAreaListener 同思路。
pub fn spawn_overlay_follow(app: tauri::AppHandle) {
    use tauri::Manager;
    tauri::async_runtime::spawn(async move {
        // [lc-1923] 残影修复：覆盖层是**不透明**窗（WebView2 不透明渲染 + 控制层纯黑底），
        // 拖动 mpv（原生拖窗，DWM 直接搬运视频窗）时它最多慢 1~2 帧 —— 那一两帧里
        // 旧的「黑块 + 旧控件」就留在原地，用户看到的就是「拖动后有原来的界面残留」。
        // Polaris 没有这个问题的根因是它的控件画在 mpv 里（osc-cinema Lua/ASS），覆盖层
        // 只是透明面板；我们保留 WebView 覆盖层，改为**移动期间直接隐藏它**：
        // 一旦检测到 mpv 矩形与覆盖层不一致就 SW_HIDE，稳定 220ms 后再 SW_SHOWNOACTIVATE。
        // 隐藏态没有任何像素可残影，且拖动中本来也点不到返回键。
        let mut hidden_for_move = false;
        let mut stable_since: Option<std::time::Instant> = None;
        while crate::mpvwin::embed_is_active() {
            tokio::time::sleep(std::time::Duration::from_millis(33)).await;
            if !crate::mpvwin::embed_is_active() {
                break;
            }
            let (Some(mh), Some(ov)) = (
                get_hwnd(),
                app.get_webview_window("mpv_overlay")
                    .and_then(|w| w.hwnd().ok().map(|h| h.0 as isize)),
            ) else {
                continue;
            };
            unsafe {
                if IsWindow(mh as _) == 0 || IsWindow(ov as _) == 0 {
                    continue;
                }
                let mut m = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                let mut o = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                if GetWindowRect(mh as _, &mut m) == 0 || GetWindowRect(ov as _, &mut o) == 0 {
                    continue;
                }
                // 位置/尺寸对齐 + z 序插到 mpv 正后方（InsertAfter=mpv）
                let same = m.left == o.left
                    && m.top == o.top
                    && m.right == o.right
                    && m.bottom == o.bottom;
                if !same {
                    // 位移超过 2px 才算「在动」（避免 DPI 取整抖动导致反复隐藏/显示）
                    let moving = (m.left - o.left).abs() > 2
                        || (m.top - o.top).abs() > 2
                        || (m.right - o.right).abs() > 2
                        || (m.bottom - o.bottom).abs() > 2;
                    if moving && !hidden_for_move {
                        ShowWindow(ov as _, SW_HIDE);
                        hidden_for_move = true;
                    }
                    stable_since = None;
                    SetWindowPos(
                        ov as _,
                        mh as _,
                        m.left,
                        m.top,
                        m.right - m.left,
                        m.bottom - m.top,
                        SWP_NOACTIVATE,
                    );
                    continue;
                }
                // 已对齐：若因移动被隐藏，稳定 220ms 后恢复显示（拖完即回显控件）
                if hidden_for_move {
                    let now = std::time::Instant::now();
                    match stable_since {
                        Some(t) if now.duration_since(t).as_millis() >= 220 => {
                            ShowWindow(ov as _, SW_SHOWNOACTIVATE);
                            hidden_for_move = false;
                            stable_since = None;
                        }
                        Some(_) => {}
                        None => stable_since = Some(now),
                    }
                }
            }
        }
    });
}

/// 隐藏 mpv 窗口（enter 收编后 / leave stop=true），并标记隐藏态供矩形首报时显示。
pub fn embed_hide(hwnd: isize) {
    unsafe {
        if IsWindow(hwnd as _) == 0 {
            return;
        }
        ShowWindow(hwnd as _, SW_HIDE);
    }
    EMBED_HIDDEN.store(true, Ordering::SeqCst);
}

/// 缓存收编完成前上报的矩形（屏幕物理坐标）。
pub fn embed_set_pending_rect(x: i32, y: i32, w: i32, h: i32) {
    *EMBED_RECT.lock().unwrap_or_else(|e| e.into_inner()) = Some((x, y, w, h));
}

fn embed_take_pending_rect() -> Option<(i32, i32, i32, i32)> {
    EMBED_RECT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// [lc-1909] 内嵌收编一步到位：找到（或复用槽内）mpv 窗口 → **先隐藏**（首个矩形上报前
/// 独立窗口不得闪现）→ 挂 owner + DWM 圆角 + 写槽 → 补施收编前已上报的矩形（含显示）。
/// 顺序刻意把「隐藏」放在「写槽」之前：写槽之后 rect 命令才可能拿到 hwnd，届时窗口必已隐藏。
pub fn embed_adopt(pid: u32, owner_hwnd: isize) -> Result<isize, String> {
    // 复用槽内仍有效的 hwnd（退嵌 stop=true 后 mpv 窗口被隐藏，EnumWindows 的
    // IsWindowVisible 过滤找不到隐藏窗口，此时槽是唯一线索）；否则按 PID 枚举可见顶层窗口。
    let hwnd = match get_hwnd() {
        Some(h) => h,
        None => find_mpv_hwnd(pid).ok_or_else(|| "未找到 mpv 的可见顶层窗口".to_string())?,
    };
    embed_hide(hwnd);
    adopt_hwnd(hwnd, owner_hwnd);
    crate::libmpv::trace(&format!(
        "[lc-1909] 内嵌收编：mpv hwnd={hwnd} pid={pid}（已隐藏，待矩形上报）"
    ));
    // 收编期间前端已上报过的矩形：隐藏完成后立即定位 + 显示
    if let Some((x, y, w, h)) = embed_take_pending_rect() {
        embed_apply_rect(hwnd, x, y, w, h);
    }
    Ok(hwnd)
}

/// [lc-1909] 把内嵌矩形（屏幕物理坐标）钉到 mpv 窗口上；此前被隐藏则顺带显示。
/// 高频调用路径：只做 SetWindowPos（绝对定位，SWP_NOACTIVATE|SWP_NOZORDER），
/// 幂等轻量；窗口仍隐藏时先定位后显示，避免在旧位置闪现。
/// [lc-1910] app 模式 mpv 以 `--window-minimized=yes` 最小化启动（防闪窗）：
/// 最小化（iconic）窗口对 SetWindowPos 的显示路径不生效，需要 SW_SHOWNOACTIVATE
/// 还原 —— 还原到「最近尺寸/位置」且不夺焦点；定位在还原**之后**做（iconic 态
/// SetWindowPos 改的是还原矩形，先还原再钉位保证矩形不丢）。
pub fn embed_apply_rect(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    // [lc-1911] 双保险：leave 之后迟到的 rect 一律忽略 —— 否则 swap 掉 EMBED_HIDDEN
    // 会把刚隐藏的 mpv 窗口重新顶出来（表现就是「返回后 mpv 还以小窗挂在桌面上」）。
    if !embed_is_active() {
        return;
    }
    unsafe {
        if IsWindow(hwnd as _) == 0 {
            clear_hwnd();
            return;
        }
        if EMBED_HIDDEN.swap(false, Ordering::SeqCst) || IsIconic(hwnd as _) != 0 {
            ShowWindow(hwnd as _, SW_SHOWNOACTIVATE);
        }
        SetWindowPos(hwnd as _, std::ptr::null_mut(), x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER);
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
    // [lc-1909] 内嵌激活 → 按位移差平移 mpv（拖动零延迟跟随，不依赖 owner 跟随开关）；
    // 非内嵌 → 仅 mpv_window_owner=true 的收编路径（OWNER_FOLLOW）才跟随。
    // 独立窗口模式（默认）完全不动 mpv 窗口（lc-1906 原行为）。
    if !embed_is_active() && !OWNER_FOLLOW.load(Ordering::SeqCst) {
        return;
    }
    if let Some(h) = get_hwnd() {
        follow_move(h, dx, dy);
    }
}
