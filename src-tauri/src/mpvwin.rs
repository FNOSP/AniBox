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
    SendMessageW, SetWindowLongPtrW, SetWindowPos, GWLP_HWNDPARENT, ICON_BIG, ICON_SMALL,
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, WM_SETICON,
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
// ───────────────────────── 窗口图标 ─────────────────────────

/// [lc-2334] 把 mpv 窗口的图标换成 AniBox 自己的。
///
/// 为什么要在 Windows 层面做：mpv **没有** `--icon` 之类的选项（`--list-options` 里
/// 没有任何 icon 项），它的窗口图标取自 exe 资源。所以从我们的 exe 里把图标取出来，
/// 用 `WM_SETICON` 塞给 mpv 的窗口。
///
/// 两个尺寸都要设：
///   · `ICON_BIG`   —— Alt+Tab 切换器、窗口预览
///   · `ICON_SMALL` —— 标题栏左上角、任务栏
/// 只设一个的话另一个位置还是 mpv 原本的图标。
///
/// 句柄归窗口所有 —— mpv 退出时系统会随窗口一起释放，所以这里**不**调用 DestroyIcon
/// （提前销毁会让窗口在下次重绘时拿到失效句柄，表现为图标变白块）。
pub fn set_window_icon(hwnd: isize) -> Result<(), String> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{LoadImageW, IMAGE_ICON, LR_SHARED};

    if hwnd == 0 || unsafe { IsWindow(hwnd as HWND) } == 0 {
        return Err("mpv 窗口无效".into());
    }

    // 取**本进程主模块**作资源来源。
    //
    // 这里刻意不只用 `GetModuleHandleW(null)`：它的语义是「当前进程的主模块」，
    // 理论上等于自己，但实测对比发现 `LoadImage(0, 32512)` 在**别的宿主进程**里
    // 拿到的是那个宿主的图标（pwsh 里测到 65579 = pwsh 自己的图标），说明 0 会被
    // 解释成「当前进程的 exe」而非我们想要的模块。为了让资源来源毫无歧义，
    // 先拿到自己的 exe 完整路径，再用 LoadLibraryEx(LOAD_LIBRARY_AS_DATAFILE)
    // 显式打开它 —— 这样加载的一定是 AniBox 自己的图标。
    //
    // 为什么用 LOAD_LIBRARY_AS_DATAFILE：只读资源、不执行也不初始化 DLL 入口，
    // 而且**不会**影响已经在跑的主模块。
    let mut path = [0u16; 512];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), path.as_mut_ptr(), path.len() as u32) };
    let work_dir = if n > 0 { Some(&path[..n as usize]) } else { None };

    let module = unsafe {
        use windows_sys::Win32::System::LibraryLoader::LoadLibraryExW;
        const LOAD_LIBRARY_AS_DATAFILE: u32 = 0x0000_0002;
        match work_dir {
            Some(p) => LoadLibraryExW(p.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_AS_DATAFILE),
            None => GetModuleHandleW(std::ptr::null()),
        }
    };
    if module.is_null() {
        return Err("打不开本进程 exe 的资源".into());
    }

    // ⚠️ 必须用**图标组** id 32512（IDI_APPLICATION），不是 1。
    // 本仓 exe 的资源树（自写 PE 解析器实测）：
    //   TYPE 3  (ICON)        id 1..7     七张单张位图（16/24/32/48/64/128/256）
    //   TYPE 14 (GROUP_ICON)  id 32512    指向上面七张的「图标组」
    // 用 id=1 只能拿到那张 16×16，WM_SETICON 给大图标位时系统会拉伸 → Alt+Tab 糊。
    const ICON_GROUP_ID: usize = 32512;

    let load = |size: i32| unsafe {
        LoadImageW(module, ICON_GROUP_ID as *const u16, IMAGE_ICON, size, size, LR_SHARED)
    };
    let big = load(32);
    let small = load(16);
    crate::libmpv::trace(&format!(
        "[lc-2334] 图标加载 module={module:?} big={big:?} small={small:?}"
    ));
    if big.is_null() && small.is_null() {
        return Err("exe 资源里没有图标组".into());
    }
    unsafe {
        let w = hwnd as HWND;
        if !big.is_null() {
            SendMessageW(w, WM_SETICON, ICON_BIG as usize, big as isize);
        }
        if !small.is_null() {
            SendMessageW(w, WM_SETICON, ICON_SMALL as usize, small as isize);
        }
    }
    Ok(())
}

/// 读回窗口当前的大图标句柄（0 = 未设置）。用于校验我们设的有没有被覆盖。
pub fn read_big_icon(hwnd: isize) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_GETICON;
    if hwnd == 0 || unsafe { IsWindow(hwnd as HWND) } == 0 {
        return 0;
    }
    unsafe { SendMessageW(hwnd as HWND, WM_GETICON, ICON_BIG as usize, 0) as isize }
}

/// 按 PID 找窗口并设图标（幂等、失败只 trace —— 图标不该成为播放的阻塞项）。
pub fn apply_icon_to_mpv(pid: u32) -> bool {
    let Some(hwnd) = find_mpv_hwnd(pid) else {
        crate::libmpv::trace("[lc-2334] 设图标：找不到 mpv 窗口");
        return false;
    };
    match set_window_icon(hwnd) {
        Ok(()) => {
            let back = read_big_icon(hwnd);
            crate::libmpv::trace(&format!(
                "[lc-2334] 已设图标 hwnd=0x{hwnd:x}，读回={back}（非 0 = 生效）"
            ));
            true
        }
        Err(e) => {
            crate::libmpv::trace(&format!("[lc-2334] 设图标失败：{e}"));
            false
        }
    }
}

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
