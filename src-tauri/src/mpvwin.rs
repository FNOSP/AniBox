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
//!
//! [lc-1949] 覆盖层输入架构重做（用户定论：「点击就是要老久才有反应」不可接受）：
//!
//!   · 覆盖层**常驻可点**：WS_EX_TRANSPARENT 从此**不再被动态切换、也不再被设置**
//!     （lc-1939~1948 的「探针 + 拨穿透位」整套机制退役：16ms 探针、热区上报、粘滞、
//!      hover-bridge 命中）。穿透位静态不存在 ⇒ 控件 `pointerdown` 即执行，链路里
//!      **没有任何等待探针翻转的环节**，点击延迟从构造上归零。
//!   · **视频区输入转发**：覆盖层收到的、落点不在控件上的鼠标事件由前端分流后经
//!     `forward_mouse` PostMessage 给 mpv 锚窗（WM_MOUSEMOVE / LBUTTONDOWN /
//!     LBUTTONUP / LBUTTONDBLCLK / RBUTTONDOWN / RBUTTONUP / MBUTTON* / MOUSEWHEEL），
//!     mpv 像 Windows 队列里的真实输入一样处理（单击暂停、双击全屏、拖动窗口、滚轮
//!     seek 全是 mpv 原生行为）。「拖动」不另走 HTCAPTION：mpv 以
//!     `--input-builtin-dragging=yes` 启动，其内部实现正是
//!     `ReleaseCapture + SendMessage(WM_NCLBUTTONDOWN, HTCAPTION)`，且在 mpv 自己的
//!     线程里做、不跨进程同步阻塞我们的主线程 —— 比我们从外面发 HTCAPTION 更顺。
//!   · 「在不在控件上」由前端在事件时决定（`closest("[data-chrome-hit]")`），Rust
//!     不再做几何猜测；`overlay-cursor` / `overlay-geometry` 两条探针事件随之退役，
//!     控件续显由 WebView 自身的 mousemove 驱动（覆盖层可点后天然可达）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows_sys::Win32::System::SystemServices::{MK_LBUTTON, MK_MBUTTON, MK_RBUTTON};
// MK_CONTROL / MK_SHIFT 只在前端传来的 flags 位里出现（8 / 4），单测用它们验证透传
#[cfg(test)]
use windows_sys::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, IsZoomed, PostMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, GWLP_HWNDPARENT, HWND_NOTOPMOST, HWND_TOPMOST, SW_HIDE, SW_MAXIMIZE, SW_MINIMIZE,
    SW_RESTORE, SW_SHOW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

/// 当前已收编的 mpv 顶层窗口（isize，全仓 HWND 约定；None = 未收编 / 已失效清理）。
static MPV_HWND: Mutex<Option<isize>> = Mutex::new(None);
/// 上一次主窗口物理位置（Moved 求位移差用；None = 首跳，忽略绝对跳变）。
static LAST_MAIN_POS: Mutex<Option<(i32, i32)>> = Mutex::new(None);

// ───────────────────── [lc-1946] 顶栏窗控的真实窗口状态 ─────────────────────
//
// 此前置顶 / 最小化 / 最大化只写 mpv 的 `ontop` / `window-minimized` /
// `window-maximized` 属性：mpv 是**无边框、被我们按矩形钉死的顶层窗**，这些属性
// 要么改了看不出变化，要么要等 mpv IPC 回包（用户实测 mpv 忙时 stop 1.5s 都不回，
// 窗口按钮就完全没有反馈）。这三组状态改为直接拨 Win32 窗口，并且立刻返回。

/// 播放器是否处于置顶（mpv + 覆盖层成对拨 TOPMOST）。起播时重置为 true。
static PLAYER_ONTOP: AtomicBool = AtomicBool::new(true);
/// 播放器是否已最大化（以 mpv 锚窗的 IsZoomed 为准；最大化期间忽略矩形重钉）。
static PLAYER_MAXIMIZED: AtomicBool = AtomicBool::new(false);
/// 最小化按钮把播放层收起来后需要等主窗恢复再放回来（on_window_event 消费）。
static PLAYER_MIN_RESTORE: AtomicBool = AtomicBool::new(false);
/// 退出流程是否已在跑：重复点击/ESC 不再叠第二条 2s 级的清理链。
static EXIT_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

// ───────────────────── [lc-1947] 真全屏（窗口级状态） ─────────────────────
//
// 根因见 lc-1947 记录：底栏全屏按钮走 `mpv_set_ui_fullscreen` → 只写 mpv 的
// `fullscreen` 属性。mpv 确实会把自己的窗口放到显示器矩形（实测 `0:0:3840:2160`），
// 但：① 它不会给窗口加 TOPMOST，任务栏（topmost）压在视频上 → 观感「不是真全屏」；
//     ② 退出由 mpv 自己还原，和我们对锚窗做的 `SW_MAXIMIZE` 状态会打架（实测退出后
//        窗口落在 `-12:-12:2568:1703`，不是进入前的矩形）；
//     ③ 覆盖层只是被动跟随，热区矩形（窗口内**绝对**坐标）不会因窗口几何变化而作废 →
//        全屏后探针仍在用进入前那份矩形，右上角翻不成可点态，点击全部落到 mpv。
// 这里把全屏提升成覆盖「mpv 锚窗 + 覆盖层」的窗口级状态。**mpv 的 `fullscreen` 属性
// 仍是唯一开关**（按钮 / 双击画面 / ENTER / ESC 四条路径天然一致），切换时统一做：
//   · 几何 = 显示器矩形（MonitorFromWindow + rcMonitor，物理像素，含任务栏区域）；
//   · 两层同处置顶，退出时恢复用户「置顶」意图；
//   · 保存 / 恢复进入前的锚窗矩形与最大化状态；
//   · 几何变化后立刻作废热区并通知前端重报（兜底带按新尺寸生效，首击即命中）。

/// 播放器是否处于真全屏（mpv `fullscreen` 属性的窗口级镜像）。
static PLAYER_FULLSCREEN: AtomicBool = AtomicBool::new(false);
/// 进全屏前的锚窗矩形 `(x, y, w, h)` + 是否最大化（物理像素；None = 没有可用还原态）。
static FULLSCREEN_SAVED: Mutex<Option<(i32, i32, i32, i32, bool)>> = Mutex::new(None);
/// 跟随循环在**非全屏**时持续记录的锚窗矩形。mpv 的 `fullscreen` 属性事件可能晚于
/// 它自己的 resize，不能靠「事件到达那一刻的 GetWindowRect」当进入前的矩形。
static LAST_NORMAL_RECT: Mutex<Option<(i32, i32, i32, i32)>> = Mutex::new(None);

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
/// [lc-1924] UI 覆盖层是否成功建成**透明**窗。透明成功 → UI 层 owned by mpv（浮在视频上）；
/// 透明失败（WebView2 拒绝）→ 退回旧方向（mpv owned by UI），宁可控件不可见也不能用不透明层盖住画面。
static OVERLAY_TRANSPARENT: AtomicBool = AtomicBool::new(false);
/// [lc-1939] 覆盖层 UI 是否可用（透明建窗成功）。false = 退回 mpv 进程内 Lua OSC。
static OVERLAY_UI_READY: AtomicBool = AtomicBool::new(false);
/// [lc-1939] 33ms 几何跟随循环是否已在跑（起播前预启动 + 收编后再调，需幂等）。
static OVERLAY_FOLLOW_RUNNING: AtomicBool = AtomicBool::new(false);

// ─────────────── [lc-1949] 视频区鼠标转发（替代探针穿透拨位）───────────────
//
// 覆盖层常驻可点后，落点不在控件上的鼠标事件必须由我们转给 mpv，否则 mpv 收不到
// 任何输入（它的窗口被覆盖层整面盖住，Windows 不会跨过覆盖层把命中给它）。
// 转发 = PostMessage 标准 WM_* 消息，mpv 的 win32 后端按真实输入处理：
// 单击暂停 / 双击全屏 / 拖动（--input-builtin-dragging）/ 滚轮 seek 全是原生行为。
//
// 为什么不用 WM_NCHITTEST→HTTRANSPARENT 让系统替我们转发：HTTRANSPARENT 只对
// **同一线程**的窗口下传（Win32 文档原文，Wine 同实现），mpv 是独立进程收不到；
// SetWindowRgn 会把覆盖层裁出洞（底部对比渐变会被裁坏）。逐事件 PostMessage 是
// 跨进程场景下唯一可行的转发通道，且它**不经过任何轮询** —— 延迟就是一次
// PostMessage（微秒级），与旧探针「最坏等 16ms 翻位 + 首击丢失竞态」不同代。

/// [lc-1949] 纯逻辑：把一次鼠标事件打包成 `(message, wParam, lParam)`。
/// `kind` 与前端 `mpv_forward_mouse` 命令的 kind 一致；`(x, y)` 为 mpv 客户区
/// 物理像素（两窗同 rect，一般等于覆盖层窗口内坐标）；`win_origin` 是 mpv 窗口
/// 屏幕原点（WM_MOUSEWHEEL 的 lParam 按约定要给**屏幕**坐标）。
/// 测试可见、供 [`forward_mouse`] 调用。wParam/lParam 用 `isize`（windows-sys 的
/// WPARAM/LPARAM 通道类型就是 isize，PostMessageW 直接收）。
pub fn pack_mouse_message(
    kind: &str,
    x: i32,
    y: i32,
    flags: u32,
    delta: i32,
    win_origin: (i32, i32),
) -> Option<(u32, isize, isize)> {
    // lParam = MAKELPARAM(x, y)：低 16 位 x、高 16 位 y（补码按 u16 截断，负坐标合法）
    let lp = |px: i32, py: i32| -> isize {
        (((py as i16 as u16 as u32) << 16) | (px as i16 as u16 as u32)) as isize
    };
    let lparam = lp(x, y);
    match kind {
        "move" => Some((WM_MOUSEMOVE, flags as isize, lparam)),
        "down" => Some((WM_LBUTTONDOWN, (flags | MK_LBUTTON) as isize, lparam)),
        "up" => Some((WM_LBUTTONUP, flags as isize, lparam)),
        "dbl" => Some((WM_LBUTTONDBLCLK, (flags | MK_LBUTTON) as isize, lparam)),
        "rdown" => Some((WM_RBUTTONDOWN, (flags | MK_RBUTTON) as isize, lparam)),
        "rup" => Some((WM_RBUTTONUP, flags as isize, lparam)),
        "mdown" => Some((WM_MBUTTONDOWN, (flags | MK_MBUTTON) as isize, lparam)),
        "mup" => Some((WM_MBUTTONUP, flags as isize, lparam)),
        "wheel" => {
            // WM_MOUSEWHEEL：wParam 高字 = 滚动增量（WHEEL_DELTA 的倍数），
            // lParam = **屏幕**坐标（OS 约定；mpv 自己 ScreenToClient 换回客户区）
            let d = delta.clamp(-32768, 32767) as i16 as u16 as u32;
            Some((
                WM_MOUSEWHEEL,
                ((d << 16) | flags) as isize,
                lp(win_origin.0 + x, win_origin.1 + y),
            ))
        }
        _ => None,
    }
}

/// [lc-1949] 把覆盖层分流的视频区鼠标事件转发给 mpv 锚窗（同步、不排队、不等待）。
/// 返回 false = 没有 mpv 窗口 / kind 不识别 / PostMessage 失败；调用方（命令层）
/// 把 false 静默吞掉即可 —— 转发失败只意味着「这次点击不走 mpv」，不影响 UI。
pub fn forward_mouse(kind: &str, x: i32, y: i32, flags: u32, delta: i32) -> bool {
    let Some(h) = get_hwnd() else {
        return false;
    };
    let mut origin = (0, 0);
    unsafe {
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if IsWindow(h as _) != 0 && GetWindowRect(h as _, &mut rc) != 0 {
            origin = (rc.left, rc.top);
        } else {
            return false;
        }
    }
    let Some((msg, wparam, lparam)) = pack_mouse_message(kind, x, y, flags, delta, origin) else {
        return false;
    };
    unsafe { PostMessageW(h as _, msg, wparam as usize, lparam) != 0 }
}

/// [lc-1947] 记录「非全屏」时的锚窗矩形（进全屏时以它为准保存还原态）。
fn record_normal_rect(x: i32, y: i32, w: i32, h: i32) {
    if w <= 0 || h <= 0 || PLAYER_FULLSCREEN.load(Ordering::SeqCst) {
        return;
    }
    let mut g = LAST_NORMAL_RECT.lock().unwrap_or_else(|e| e.into_inner());
    if *g != Some((x, y, w, h)) {
        *g = Some((x, y, w, h));
    }
}

pub fn set_overlay_transparent(ok: bool) {
    OVERLAY_TRANSPARENT.store(ok, Ordering::SeqCst);
    OVERLAY_UI_READY.store(ok, Ordering::SeqCst);
}

pub fn overlay_ui_ready() -> bool {
    OVERLAY_UI_READY.load(Ordering::SeqCst)
}

/// [lc-1939/1949] 覆盖层窗样式：NOACTIVATE（点击不抢焦点）+ TOOLWINDOW（不进任务栏/
/// Alt-Tab）+ **LAYERED**（WebView2 的逐像素透明靠它合成）。
///
/// [lc-1949] 这是覆盖层 ex-style 的**唯一**写入点，且永远不含 `WS_EX_TRANSPARENT`：
/// 覆盖层常驻可点，视频区点击由前端分流后 PostMessage 转给 mpv（见 [`forward_mouse`]）。
/// 旧「探针 + 动态拨穿透位」机制全部退役 —— 也不能改用 tauri 的
/// `set_ignore_cursor_events`（tao 把 TRANSPARENT 与 LAYERED 置成一组，会连透明一起
/// 摘掉 → 黑块盖死视频，lc-1924~1931 的真根因）。
pub fn overlay_style_noactivate(hwnd: isize) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return;
    }
    unsafe {
        let ex = GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE);
        let next = ex
            | (WS_EX_NOACTIVATE as isize)
            | (WS_EX_TOOLWINDOW as isize)
            | (WS_EX_LAYERED as isize);
        if next != ex {
            SetWindowLongPtrW(hwnd as _, GWL_EXSTYLE, next);
        }
    }
}

/// [lc-1942] 覆盖层临时可聚焦：磨砂菜单里的文本输入（字幕搜索）需要键盘。
///
/// 覆盖层常规态常驻 `WS_EX_NOACTIVATE`（点控件不抢 mpv 焦点，Polaris 同款）；
/// 只有输入框真正获得焦点期间才摘掉这一位，失焦/关闭面板立刻装回。
/// 只动 NOACTIVATE，`WS_EX_LAYERED` / `WS_EX_TOOLWINDOW` 保持不变 ——
/// lc-1939 的逐像素透明与 lc-1940/1941 的显隐路径都不受影响。
pub fn overlay_set_focusable(hwnd: isize, on: bool) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return;
    }
    unsafe {
        let ex = GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE);
        let next = if on {
            ex & !(WS_EX_NOACTIVATE as isize)
        } else {
            ex | (WS_EX_NOACTIVATE as isize)
        };
        if next != ex {
            SetWindowLongPtrW(hwnd as _, GWL_EXSTYLE, next);
        }
    }
}

/// [lc-1949] 置顶覆盖层。ex-style 只含 NOACTIVATE + TOOLWINDOW + LAYERED，一次成型
/// （见 `overlay_style_noactivate`）：`WS_EX_TRANSPARENT` 从此不在任何路径上被设置
/// 或摘除 —— 旧「探针拨穿透位」整套机制退役，覆盖层常驻可点。
pub fn overlay_set_topmost(hwnd: isize) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return;
    }
    unsafe {
        SetWindowPos(
            hwnd as _,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// [lc-1939] 显示覆盖层但**不激活**（Tauri show() 会抢前台，播放器窗不允许）。
pub fn overlay_show_noactivate(hwnd: isize) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return;
    }
    unsafe {
        ShowWindow(hwnd as _, SW_SHOWNOACTIVATE);
    }
}

/// [lc-1940] 隐藏覆盖层 —— **必须**用原始 ShowWindow，不能用 `WebviewWindow::hide()`。
///
/// 黑屏根因（lc-1939 回归）：窗口以 `visible(false)` 建出，tao 缓存的
/// `WindowFlags::VISIBLE` 恒为 false；lc-1939 又改用原始 `ShowWindow(SW_SHOWNOACTIVATE)`
/// 显示（绕开 tao），缓存与实际状态彻底脱钩。此时 Tauri 的 `hide()` 只是把 VISIBLE
/// 再置一次 false —— diff 为空，tao `apply_diff` 在 `window_state.rs` 里**直接 return**，
/// 一句 `ShowWindow(SW_HIDE)` 都不会发。于是启动预建的覆盖层一直盖在主窗上：
/// 透明 WebView 未加载内容时就是整屏黑窗（用户看到的「启动直接黑屏 mpv 窗口」）。
///
/// 结论：覆盖层的显隐全部走这里，和 tao 的缓存彻底解耦；绝不再对覆盖层调 Tauri hide/show。
pub fn overlay_hide(hwnd: isize) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return;
    }
    unsafe {
        ShowWindow(hwnd as _, SW_HIDE);
    }
}

// ───────────────────── [lc-1946] 顶栏窗控（真实窗口，零 mpv IPC） ─────────────────────

/// 单独拨一个窗口的 TOPMOST 位（不动几何 / 不激活）。
fn set_window_topmost(hwnd: isize, on: bool) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return;
    }
    unsafe {
        SetWindowPos(
            hwnd as _,
            if on { HWND_TOPMOST } else { HWND_NOTOPMOST },
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// 窗口当前是否带 TOPMOST（读 ex-style 真值，不缓存）。
fn is_window_topmost(hwnd: isize) -> bool {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return false;
    }
    unsafe { GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE) & (WS_EX_TOPMOST as isize) != 0 }
}

/// [lc-1946] 置顶：mpv 锚窗 + 覆盖层成对拨 TOPMOST/NOTOPMOST。
///
/// 只拨 mpv 的 `ontop` 属性没用：覆盖层自己一直是 TOPMOST，用户看不到任何变化；
/// 两个窗口一起拨才是「整个播放器置顶 / 取消置顶」。`overlay_hwnd` 由前端所在窗口
/// 自己的 hwnd 传入（命令里取），mpv 锚窗从槽里读。
pub fn set_player_ontop(overlay_hwnd: isize, on: bool) {
    PLAYER_ONTOP.store(on, Ordering::SeqCst);
    // [lc-1947] 全屏期间强制置顶：否则视频层会被任务栏（topmost）压住，看起来「不是真全屏」。
    // 用户的「置顶」意图照常记录，退出全屏时再按它生效。
    let apply = effective_topmost(on, PLAYER_FULLSCREEN.load(Ordering::SeqCst));
    set_window_topmost(get_hwnd().unwrap_or(0), apply);
    set_window_topmost(overlay_hwnd, apply);
}

/// [lc-1947] 纯逻辑：窗口实际是否置顶。全屏期间恒为 true（压过任务栏）。
pub fn effective_topmost(intent: bool, fullscreen: bool) -> bool {
    intent || fullscreen
}

pub fn player_ontop() -> bool {
    PLAYER_ONTOP.load(Ordering::SeqCst)
}

/// 起播时重置为「置顶态」（`show_player_overlay` 会把覆盖层拨 TOPMOST）。
pub fn reset_player_ontop() {
    PLAYER_ONTOP.store(true, Ordering::SeqCst);
}

/// 已收编的 mpv 锚窗是否处于最大化。
pub fn player_maximized() -> bool {
    PLAYER_MAXIMIZED.load(Ordering::SeqCst)
}

/// 清最大化标记（退嵌 / 重新钉位时调用）。
pub fn clear_player_maximized() {
    PLAYER_MAXIMIZED.store(false, Ordering::SeqCst);
}

/// [lc-1946] 退嵌前若锚窗仍最大化，先 `SW_RESTORE`。
/// 隐藏一个 maximized 窗口不会清状态（下一次起播会被 SHOWNOACTIVATE 原样放出来），
/// 所以必须在收窗之前还原。用 `IsZoomed` 直接读窗口，**不**依赖静态标记
/// （`embed_set_active(false)` 会先把它清零）。
pub fn restore_player_window_if_zoomed() {
    if let Some(h) = get_hwnd() {
        if unsafe { IsWindow(h as _) } != 0 && unsafe { IsZoomed(h as _) } != 0 {
            unsafe {
                ShowWindow(h as _, SW_RESTORE);
            }
            crate::libmpv::trace("[lc-1946] 退出前还原最大化锚窗");
        }
    }
    PLAYER_MAXIMIZED.store(false, Ordering::SeqCst);
}

/// [lc-1946] 最大化 / 还原：对 mpv 锚窗做 Win32 `SW_MAXIMIZE` / `SW_RESTORE`。
///
/// [lc-1948] 覆盖层右上角按钮已改走「真全屏」路径（mpv `fullscreen` 属性 →
/// [`on_player_fullscreen_changed`]），**不再调用本函数**：`SW_MAXIMIZE` 只铺
/// 工作区（任务栏露着），且 mpv 对最大化窗口做边框补偿会把窗口矩形挪出屏幕
/// （实测 `-11,-11 3840x2160`，右/下各露一条缝）—— 这不是全屏，也不该由全屏
/// 按钮触发。本命令保留给外部/兼容调用；全屏中仍然拒绝（两种「铺满」叠在一起
/// 无法正确还原）。
///
/// 覆盖层是 mpv 的 owned window，且 33ms 跟随循环按 mpv 矩形贴位 —— 锚窗一变，
/// 控制层下一拍就跟过去，所以「最大化同时作用于覆盖层」由现有跟随循环天然保证，
/// 不需要再对覆盖层单独 ShowWindow（那会和跟随循环抢几何）。
/// 返回操作后的最大化状态，供前端换图标。
pub fn toggle_player_maximize() -> bool {
    let Some(h) = get_hwnd() else {
        return false;
    };
    if unsafe { IsWindow(h as _) } == 0 {
        return false;
    }
    // [lc-1947] 全屏中不参与最大化：两种「铺满屏幕」的窗口状态叠在一起无法正确还原
    if PLAYER_FULLSCREEN.load(Ordering::SeqCst) {
        crate::libmpv::trace("[lc-1947] 全屏中忽略最大化切换");
        return player_maximized();
    }
    let zoomed = unsafe { IsZoomed(h as _) } != 0;
    unsafe {
        ShowWindow(h as _, if zoomed { SW_RESTORE } else { SW_MAXIMIZE });
    }
    let next = unsafe { IsZoomed(h as _) } != 0;
    PLAYER_MAXIMIZED.store(next, Ordering::SeqCst);
    crate::libmpv::trace(&format!(
        "[lc-1946] 最大化切换 maximized={next}（mpv hwnd={h}）"
    ));
    next
}

/// [lc-1946] 最小化标记：主窗从任务栏恢复时把它消费掉，重新显示播放层。
pub fn request_minimize_restore() {
    PLAYER_MIN_RESTORE.store(true, Ordering::SeqCst);
}

/// 取走最小化标记（true = 这次主窗恢复要重新显示播放层）。
pub fn take_minimize_restore() -> bool {
    PLAYER_MIN_RESTORE.swap(false, Ordering::SeqCst)
}

pub fn clear_minimize_restore() {
    PLAYER_MIN_RESTORE.store(false, Ordering::SeqCst);
}

/// 播放层在最小化时被收起：mpv 锚窗沿用原矩形重新显示，覆盖层回到它上面。
/// 顺序：先让 mpv 回来（`embed_apply_rect` 会处理 SW_HIDE / iconic 两种被收起状态），
/// 再显示覆盖层；几何对齐交给 33ms 跟随循环。
pub fn resume_embed_windows(overlay_hwnd: isize) {
    if !embed_is_active() {
        return;
    }
    if let Some(h) = get_hwnd() {
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { GetWindowRect(h as _, &mut rc) } != 0 {
            embed_apply_rect(h, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top);
        }
    }
    overlay_show_noactivate(overlay_hwnd);
    crate::libmpv::trace("[lc-1946] 主窗恢复 → 播放层重新显示");
}

// ───────────────────── [lc-1947] 真全屏：窗口级切换 ─────────────────────

/// [lc-1947] 纯逻辑：退出全屏的恢复步骤（执行与单测共用同一份决策）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenRestoreStep {
    /// 先清掉最大化/全屏样式（对 maximized 窗口直接 SetWindowPos 只改还原矩形）
    Restore,
    /// 回到进入全屏前的锚窗矩形
    MoveTo(i32, i32, i32, i32),
    /// 进入前是最大化 → 重新最大化
    Maximize,
    /// 恢复用户「置顶」意图
    Topmost(bool),
}

/// [lc-1947] 纯逻辑：退出全屏要按什么顺序恢复。
/// 最大化必须在 MoveTo **之后**：SW_MAXIMIZE 会让系统覆盖还原矩形，顺序反了就丢矩形。
pub fn fullscreen_restore_steps(
    rect: (i32, i32, i32, i32),
    zoomed: bool,
    ontop: bool,
) -> Vec<FullscreenRestoreStep> {
    let mut steps = vec![
        FullscreenRestoreStep::Restore,
        FullscreenRestoreStep::MoveTo(rect.0, rect.1, rect.2, rect.3),
    ];
    if zoomed {
        steps.push(FullscreenRestoreStep::Maximize);
    }
    steps.push(FullscreenRestoreStep::Topmost(ontop));
    steps
}

pub fn player_fullscreen() -> bool {
    PLAYER_FULLSCREEN.load(Ordering::SeqCst)
}

/// 新会话开始时清全屏状态（上一次播放若在全屏中退出，标记不能带到下一次）。
pub fn reset_player_fullscreen() {
    PLAYER_FULLSCREEN.store(false, Ordering::SeqCst);
    *FULLSCREEN_SAVED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn overlay_hwnd_of(app: &tauri::AppHandle) -> isize {
    use tauri::Manager;
    app.get_webview_window("mpv_overlay")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as isize)
        .unwrap_or(0)
}

/// 取窗口所在显示器的**完整矩形**（`rcMonitor`，物理像素）。
///
/// 必须用 `rcMonitor` 而不是 `rcWork`：实现要求「铺满显示器（不是工作区）」，
/// 任务栏那一条也要盖住（置顶由 [`effective_topmost`] 保证）。
fn monitor_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    if hwnd == 0 {
        return None;
    }
    unsafe {
        let mon = MonitorFromWindow(hwnd as _, MONITOR_DEFAULTTONEAREST);
        if mon.is_null() {
            return None;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        Some((
            mi.rcMonitor.left,
            mi.rcMonitor.top,
            mi.rcMonitor.right - mi.rcMonitor.left,
            mi.rcMonitor.bottom - mi.rcMonitor.top,
        ))
    }
}

/// 只挪几何（不激活、不改 z 序）；z 序由 [`set_window_topmost`] 单独负责。
fn move_window(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return;
    }
    unsafe {
        SetWindowPos(
            hwnd as _,
            std::ptr::null_mut(),
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

/// 全屏几何断言（幂等）：锚窗没铺满显示器就重新摆，两层都置顶。
fn ensure_fullscreen_geometry(app: &tauri::AppHandle) {
    if !PLAYER_FULLSCREEN.load(Ordering::SeqCst) {
        return;
    }
    let mpv = get_hwnd().unwrap_or(0);
    let overlay = overlay_hwnd_of(app);
    let Some((mx, my, mw, mh)) = monitor_rect(if mpv != 0 { mpv } else { overlay }) else {
        return;
    };
    if mpv != 0 {
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let ok = unsafe { GetWindowRect(mpv as _, &mut rc) } != 0;
        if !ok
            || rc.left != mx
            || rc.top != my
            || rc.right - rc.left != mw
            || rc.bottom - rc.top != mh
        {
            move_window(mpv, mx, my, mw, mh);
            crate::libmpv::trace(&format!(
                "[lc-1947] 全屏几何断言：锚窗未铺满显示器 → 重新定位 {mw}x{mh}@{mx},{my}"
            ));
        }
    }
    move_window(overlay, mx, my, mw, mh);
    set_window_topmost(mpv, true);
    set_window_topmost(overlay, true);
}

/// [lc-1947] mpv `fullscreen` 属性变化的窗口级处理（由 mpv.rs 的属性观察线程调用）。
///
/// 这是全屏状态**唯一**的入口：按钮 / 双击画面 / ENTER / ESC 全都只改 mpv 属性，
/// 所以四条路径的几何、置顶、还原行为天然一致。
pub fn on_player_fullscreen_changed(app: &tauri::AppHandle, on: bool) {
    if PLAYER_FULLSCREEN.swap(on, Ordering::SeqCst) == on {
        return;
    }
    let mpv = get_hwnd().unwrap_or(0);
    let overlay = overlay_hwnd_of(app);
    if on {
        // ① 保存进入前的还原态。LAST_NORMAL_RECT 由跟随循环/探针在非全屏时持续刷新，
        //    不依赖「属性事件到达这一刻」的窗口矩形（mpv 的 resize 可能已经发生）。
        let mut saved_rect = *LAST_NORMAL_RECT.lock().unwrap_or_else(|e| e.into_inner());
        if saved_rect.is_none() && overlay != 0 {
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if unsafe { GetWindowRect(overlay as _, &mut rc) } != 0 && rc.right > rc.left {
                saved_rect = Some((rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top));
            }
        }
        let zoomed = mpv != 0 && unsafe { IsZoomed(mpv as _) } != 0;
        *FULLSCREEN_SAVED.lock().unwrap_or_else(|e| e.into_inner()) =
            saved_rect.map(|(x, y, w, h)| (x, y, w, h, zoomed));
        PLAYER_MAXIMIZED.store(false, Ordering::SeqCst);
        match saved_rect {
            Some((x, y, w, h)) => crate::libmpv::trace(&format!(
                "[lc-1947] 进入全屏：保存还原矩形 {w}x{h}@{x},{y} maximized={zoomed}"
            )),
            None => crate::libmpv::trace("[lc-1947] 进入全屏：没有可用的还原矩形"),
        }
        // ② 从最大化样式里拉出来（maximized 状态下 SetWindowPos 只改还原矩形，视觉不动）
        if mpv != 0 && zoomed {
            unsafe {
                ShowWindow(mpv as _, SW_RESTORE);
            }
        }
        let Some((mx, my, mw, mh)) = monitor_rect(if mpv != 0 { mpv } else { overlay }) else {
            crate::libmpv::trace("[lc-1947] 进入全屏失败：取不到显示器矩形");
            return;
        };
        move_window(mpv, mx, my, mw, mh);
        move_window(overlay, mx, my, mw, mh);
        set_window_topmost(mpv, true);
        set_window_topmost(overlay, true);
        crate::libmpv::trace(&format!(
            "[lc-1947] 进入全屏：显示器矩形 {mw}x{mh}@{mx},{my}（锚窗 + 覆盖层 TOPMOST）"
        ));
        // ③ mpv 的 VO 线程可能稍后才处理自己的 fullscreen（实测 ~40ms），再断言一次几何
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            ensure_fullscreen_geometry(&app2);
        });
    } else {
        // 退出：按保存的矩形 / 最大化状态权威恢复（mpv 自己也会还原，但可能晚 40ms，
        // 且它还原的是「自己记录的」窗口状态，不一定等于我们进入前的矩形）。
        let saved = FULLSCREEN_SAVED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some((x, y, w, h, zoomed)) = saved else {
            crate::libmpv::trace("[lc-1947] 退出全屏：没有保存的还原矩形（跳过几何恢复）");
            return;
        };
        let ontop = PLAYER_ONTOP.load(Ordering::SeqCst);
        for step in fullscreen_restore_steps((x, y, w, h), zoomed, ontop) {
            match step {
                FullscreenRestoreStep::Restore => {
                    if mpv != 0 {
                        unsafe {
                            ShowWindow(mpv as _, SW_RESTORE);
                        }
                    }
                }
                FullscreenRestoreStep::MoveTo(x, y, w, h) => {
                    move_window(mpv, x, y, w, h);
                    move_window(overlay, x, y, w, h);
                }
                FullscreenRestoreStep::Maximize => {
                    if mpv != 0 {
                        unsafe {
                            ShowWindow(mpv as _, SW_MAXIMIZE);
                        }
                    }
                }
                FullscreenRestoreStep::Topmost(on) => {
                    set_window_topmost(mpv, on);
                    set_window_topmost(overlay, on);
                }
            }
        }
        PLAYER_MAXIMIZED.store(zoomed, Ordering::SeqCst);
        crate::libmpv::trace(&format!(
            "[lc-1947] 退出全屏：恢复 {w}x{h}@{x},{y} maximized={zoomed} ontop={ontop}"
        ));
        // mpv 的还原可能在其 VO 线程稍后执行（会把窗口拉回它自己的矩形）：再断言一次
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(90)).await;
            if PLAYER_FULLSCREEN.load(Ordering::SeqCst) {
                return;
            }
            let mpv = get_hwnd().unwrap_or(0);
            let overlay = overlay_hwnd_of(&app2);
            move_window(mpv, x, y, w, h);
            move_window(overlay, x, y, w, h);
            if zoomed && mpv != 0 {
                unsafe {
                    ShowWindow(mpv as _, SW_MAXIMIZE);
                }
            }
            let ontop = PLAYER_ONTOP.load(Ordering::SeqCst);
            set_window_topmost(mpv, ontop);
            set_window_topmost(overlay, ontop);
        });
    }
}

/// [lc-1947] 退出播放前若还在全屏：把锚窗还原到进入前的矩形/最大化状态，
/// 之后才能安全地 `SW_HIDE`（否则下一次起播会继承一块全屏几何）。
pub fn restore_player_window_from_fullscreen() {
    if !PLAYER_FULLSCREEN.swap(false, Ordering::SeqCst) {
        return;
    }
    let mpv = get_hwnd().unwrap_or(0);
    let saved = FULLSCREEN_SAVED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if mpv != 0 {
        unsafe {
            ShowWindow(mpv as _, SW_RESTORE);
        }
    }
    match saved {
        Some((x, y, w, h, zoomed)) => {
            move_window(mpv, x, y, w, h);
            if zoomed && mpv != 0 {
                unsafe {
                    ShowWindow(mpv as _, SW_MAXIMIZE);
                }
            }
            PLAYER_MAXIMIZED.store(zoomed, Ordering::SeqCst);
            crate::libmpv::trace("[lc-1947] 退出前还原全屏几何（锚窗回到进入前矩形/最大化态）");
        }
        None => crate::libmpv::trace("[lc-1947] 退出前退出全屏：无保存矩形，只清窗口样式"),
    }
}

/// [lc-1947] 最小化到任务栏：**先显示为最小化**，再最小化。
///
/// 根因：播放期主窗被 `m.hide()` 隐藏（mpv.rs / player.rs 起播路径），对**隐藏窗口**
/// 调 `minimize()`（tao → `ShowWindow(SW_MINIMIZE)`）不会产生任务栏条目 ——
/// 旧实现于是「覆盖层 + mpv 都收起 + 主窗隐藏着被最小化」= 整个程序从桌面消失。
/// `SW_SHOWMINNOACTIVE` 一步完成「可见 + 最小化 + 不激活」，不会闪一下主界面；
/// 返回 `(IsWindowVisible, IsIconic)` 供调用方写进日志做证据。
pub fn minimize_main_to_taskbar(hwnd: isize) -> (bool, bool) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return (false, false);
    }
    unsafe {
        if IsWindowVisible(hwnd as _) == 0 {
            ShowWindow(hwnd as _, SW_SHOWMINNOACTIVE);
        }
        if IsIconic(hwnd as _) == 0 {
            ShowWindow(hwnd as _, SW_MINIMIZE);
        }
        (
            IsWindowVisible(hwnd as _) != 0,
            IsIconic(hwnd as _) != 0,
        )
    }
}

/// 退出流程去重：true = 本次调用真正开始跑退出（之前没有在跑）。
pub fn begin_exit_flow() -> bool {
    !EXIT_IN_FLIGHT.swap(true, Ordering::SeqCst)
}

/// 退出流程收尾 / 新会话开始时清标记。
pub fn end_exit_flow() {
    EXIT_IN_FLIGHT.store(false, Ordering::SeqCst);
}

/// [lc-1911] 进内嵌前的 mpv `script-opts` 原串（leave 时还原，独立窗口的 uosc 顶栏要回来）。
static SAVED_SCRIPT_OPTS: Mutex<Option<String>> = Mutex::new(None);

// ───────────────────── [lc-1941] 退出路径的窗口兜底 ─────────────────────

/// [lc-1941] 窗口**真实**可见性（IsWindowVisible，不读 tao 缓存）。
/// tao 的 `is_visible()` 只是 `WindowFlags::VISIBLE` 缓存，覆盖层在这套架构里早就不经过
/// tao 显隐（见 `overlay_hide`），缓存可能和真实状态脱钩 —— 判断"主窗回来了没有"只能问系统。
pub fn window_is_visible(hwnd: isize) -> bool {
    hwnd != 0 && unsafe { IsWindow(hwnd as _) != 0 && IsWindowVisible(hwnd as _) != 0 }
}

/// [lc-1941] 强制显示窗口（原始 ShowWindow，和 tao 缓存解耦）。
/// 用于关闭播放器后唤回主窗：先 Tauri `show()` 同步缓存，这里再补一发系统调用，
/// 任何一条路径把主窗留在隐藏态都能被拉回来。已可见时不动（避免多余激活/闪烁）。
///
/// 不用 SW_SHOWNORMAL：它会把**已最大化**的窗口按"原始尺寸"还原（退出播放器后主窗
/// 突然变小）。这里只做"可见性"修复、不改窗口状态：最小化才 SW_RESTORE，其余 SW_SHOW
/// （SW_SHOW 保留最大化/还原态，也不强制激活 —— 激活交给后面的 set_focus）。
pub fn window_force_show(hwnd: isize) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return;
    }
    unsafe {
        if IsWindowVisible(hwnd as _) == 0 {
            if IsIconic(hwnd as _) != 0 {
                ShowWindow(hwnd as _, SW_RESTORE);
            } else {
                ShowWindow(hwnd as _, SW_SHOW);
            }
        } else if IsIconic(hwnd as _) != 0 {
            // [lc-1947] `IsWindowVisible` 对最小化窗口同样为真：可见但仍是最小化时
            // 必须真正 SW_RESTORE（最小化按钮用原始 SW_SHOWMINNOACTIVE 后退出播放，
            // 这里若什么都不做，主窗会留在任务栏最小化态）。
            ShowWindow(hwnd as _, SW_RESTORE);
        }
    }
}

/// [lc-1947] 真实隐藏窗口（与 tao 的 VISIBLE 缓存解耦）。
///
/// 起播时主窗除了 `m.hide()` 还要补这一发：`player_window_minimize` 用原始
/// `SW_SHOWMINNOACTIVE` 把主窗「显示为最小化」（为了任务栏入口），tao 的缓存仍是
/// 隐藏态 —— 用户下次直接换集时 `m.hide()` 的 diff 为空、一句 ShowWindow 都不发，
/// 主窗会留在任务栏上。与 [`window_force_show`] 配对使用。
pub fn window_force_hide(hwnd: isize) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return;
    }
    if unsafe { IsWindowVisible(hwnd as _) } != 0 {
        unsafe {
            ShowWindow(hwnd as _, SW_HIDE);
        }
    }
}

struct HideCtx {
    pid: u32,
    found: Option<isize>,
}

/// EnumWindows 回调：把该 PID 的**可见**顶层窗口逐个 SW_HIDE（不提前终止，全部收起）。
unsafe extern "system" fn hide_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = unsafe { &mut *(lparam as *mut HideCtx) };
    let mut proc_id: u32 = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut proc_id);
        if proc_id == ctx.pid && IsWindowVisible(hwnd) != 0 {
            ShowWindow(hwnd, SW_HIDE);
            ctx.found = Some(hwnd as isize);
        }
    }
    1
}

/// [lc-1941] 按会话 PID 隐藏 mpv 的全部可见顶层窗口，返回最后命中的句柄。
///
/// 为什么单靠槽里的 hwnd 不够（关闭黑屏的真根因）：
/// mpv 收到 `stop` 后进入 idle，并在几十毫秒后由 `vo/gpu-next/win32` 重新 reconfig
/// 自己的窗口（实测 `%TEMP%\anibox-mpv.log`：`Run command: stop` @62.774 →
/// `Window size changed to: 660:292:2520:1575` @62.823，窗口被重新 show）。
/// 此前退出流程只在 stop 之后 SW_HIDE 一次，随即被 mpv 自己覆盖 → 留在屏幕上的是
/// **满屏纯黑的 idle mpv 窗口**（还压在刚刚唤回的主窗之上），用户看到的就是
/// 「点关闭直接整屏全黑、回不到软件界面」，且后续点击全被这个黑窗吃掉。
///
/// 这里按 PID 枚举（**不 taskkill、不碰其它 mpv 实例**），调用方可重复调用直到窗口稳定隐藏。
pub fn hide_visible_windows_for_pid(pid: u32) -> Option<isize> {
    let mut ctx = HideCtx { pid, found: None };
    unsafe {
        EnumWindows(Some(hide_proc), &mut ctx as *mut HideCtx as LPARAM);
    }
    ctx.found
}

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
///
/// [lc-1924] **owner 方向翻转**（对齐 Polaris 活体结构）：
/// 之前是「mpv 挂到 ui 窗下」（ui=owner → mpv 作为 owned 永远压在 ui 之上 →
/// 覆盖层里的控件被视频盖住，永远看不见，只能靠不透明黑底当挡板）。
/// Polaris 实测是反的：`Tauri Window "MPV Overlay"` 的 GW_OWNER = **mpv 窗口**，
/// 即 UI 层是 mpv 的 owned window —— 于是 UI 永远浮在视频之上，
/// 且 mpv 原生拖动（begin-vo-dragging）时 UI 只是轻量跟随，不会让视频等 UI。
/// 这里 `ui_hwnd` = 播放器 UI 覆盖层窗口；mpv 保持独立顶层窗（可被用户原生拖动）。
fn adopt_hwnd(mpv_hwnd: isize, ui_hwnd: isize) {
    // 透明没建成：回退旧方向（mpv 挂到 UI 下）——UI 层被视频盖住，但不会盖住画面
    if !OVERLAY_TRANSPARENT.load(Ordering::SeqCst) {
        unsafe {
            let old = SetWindowLongPtrW(mpv_hwnd as _, GWLP_HWNDPARENT, ui_hwnd);
            crate::libmpv::trace(&format!(
                "[lc-1924] 覆盖层非透明 → 回退旧方向：mpv owner={ui_hwnd}（旧 {old}）"
            ));
        }
        crate::winvisual::set_system_round_corner(mpv_hwnd);
        set_hwnd(mpv_hwnd);
        return;
    }
    unsafe {
        if ui_hwnd != 0 && IsWindow(ui_hwnd as _) != 0 {
            let old = SetWindowLongPtrW(ui_hwnd as _, GWLP_HWNDPARENT, mpv_hwnd);
            crate::libmpv::trace(&format!(
                "[lc-1924] UI 层 owner：{old} → mpv {mpv_hwnd}（ui hwnd={ui_hwnd}）"
            ));
        }
    }
    // DWM 系统圆角：复用主窗口同一套调用（DWMWCP_ROUND + 关 DWM 边框）。
    // mpv 本来 border=no，边框关不关无感；圆角让它与主窗口观感一致。
    crate::winvisual::set_system_round_corner(mpv_hwnd);
    set_hwnd(mpv_hwnd);
    // [lc-1946] 收编后让 mpv 锚窗与覆盖层同处置顶态（顶栏「置顶」的默认值）。
    // 只拨覆盖层的话，取消置顶时视频窗仍会被别的应用盖住。
    set_window_topmost(mpv_hwnd, PLAYER_ONTOP.load(Ordering::SeqCst));
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
        // [lc-1946] 会话结束：最小化待恢复 / 最大化标记都不该带到下一次播放
        PLAYER_MIN_RESTORE.store(false, Ordering::SeqCst);
        PLAYER_MAXIMIZED.store(false, Ordering::SeqCst);
        // [lc-1947] 全屏标记 / 还原矩形同样不能跨会话
        PLAYER_FULLSCREEN.store(false, Ordering::SeqCst);
        *FULLSCREEN_SAVED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *LAST_NORMAL_RECT.lock().unwrap_or_else(|e| e.into_inner()) = None;
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

/// [lc-1948] 纯逻辑：跟随循环一拍该做什么（执行与单测共用同一份决策）。
///
/// 全屏态必须**挂起**「锚窗→覆盖层」的抄写：mpv 的 VO 在进/退全屏瞬间会先跳回
/// 它自己记忆的窗口矩形（实测 `-11,-11 3851x2171`，即最大化残留），旧逻辑会把
/// 这份矩形原样抄给覆盖层 → 两层几何来回拉锯、热区反复作废/重报（全屏后控件
/// 点不中的温床，lc-1947 的 120ms 一次性断言兜不住后续每一拍）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowAction {
    /// 非全屏：维持原行为 —— 把覆盖层抄到锚窗矩形。
    Copy,
    /// 全屏且两层之一偏离显示器矩形：两层都按显示器矩形复位（x, y, w, h）。
    Assert(i32, i32, i32, i32),
    /// 全屏且两层已贴合显示器矩形：不动（省一次 syscall）。
    Hold,
}

/// [lc-1948] 跟随循环决策。锚窗/覆盖层传 `(left, top, right, bottom)`，
/// 显示器传 `(x, y, w, h)`（[`monitor_rect`] 的返回形态）。
pub fn follow_loop_action(
    fullscreen: bool,
    anchor: (i32, i32, i32, i32),
    overlay: (i32, i32, i32, i32),
    monitor: (i32, i32, i32, i32),
) -> FollowAction {
    if !fullscreen {
        return FollowAction::Copy;
    }
    let (mx, my, mw, mh) = monitor;
    let mon = (mx, my, mx + mw, my + mh);
    if anchor == mon && overlay == mon {
        FollowAction::Hold
    } else {
        FollowAction::Assert(mx, my, mw, mh)
    }
}

/// [lc-1915] 跟随循环（Polaris 同款架构的另一半）：拖动画面 = mpv 自己动（窗口拖拽），
/// 本循环 33ms 轮询 mpv 矩形，把 `mpv_overlay` 窗口贴到 mpv 矩形上（z 序插在 mpv 正后方）。
/// mpv 是锚、控制层窗是随动 —— 与 Polaris 的 WorkAreaListener 同思路。
pub fn spawn_overlay_follow(app: tauri::AppHandle) {
    use tauri::Manager;
    if OVERLAY_FOLLOW_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
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
                // [lc-1949]（原探针职责）全屏期间守住「压过任务栏」：mpv 的 ontop_mode.lua
                // 会在暂停时把 ontop 置 false（= 对锚窗 SetWindowPos(NOTOPMOST)），视频
                // 就会掉到任务栏下面。33ms 循环发现 TOPMOST 位丢了立刻复位。
                if PLAYER_FULLSCREEN.load(Ordering::SeqCst) {
                    if !is_window_topmost(mh as _) {
                        set_window_topmost(mh as _, true);
                        crate::libmpv::trace("[lc-1949] 全屏中锚窗丢失 TOPMOST → 复位");
                    }
                    if !is_window_topmost(ov as _) {
                        set_window_topmost(ov as _, true);
                    }
                }
                let mut m = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                let mut o = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                if GetWindowRect(mh as _, &mut m) == 0 || GetWindowRect(ov as _, &mut o) == 0 {
                    continue;
                }
                // [lc-1947] 非全屏时持续记录锚窗矩形：进全屏时用它当还原态
                //（mpv 的 fullscreen 属性事件可能晚于它自己的 resize）。
                record_normal_rect(m.left, m.top, m.right - m.left, m.bottom - m.top);
                // [lc-1948] 全屏态挂起抄写，改为把两层断言在显示器矩形上（见 FollowAction）。
                match follow_loop_action(
                    PLAYER_FULLSCREEN.load(Ordering::SeqCst),
                    (m.left, m.top, m.right, m.bottom),
                    (o.left, o.top, o.right, o.bottom),
                    monitor_rect(mh as _).unwrap_or((m.left, m.top, m.right - m.left, m.bottom - m.top)),
                ) {
                    FollowAction::Hold => {}
                    FollowAction::Assert(fx, fy, fw, fh) => {
                        if m.left != fx
                            || m.top != fy
                            || m.right - m.left != fw
                            || m.bottom - m.top != fh
                        {
                            move_window(mh as _, fx, fy, fw, fh);
                            crate::libmpv::trace(
                                "[lc-1948] 全屏跟随：锚窗偏离显示器矩形 → 复位",
                            );
                        }
                        if o.left != fx || o.top != fy || o.right - o.left != fw || o.bottom - o.top != fh
                        {
                            SetWindowPos(
                                ov as _,
                                std::ptr::null_mut(),
                                fx,
                                fy,
                                fw,
                                fh,
                                SWP_NOACTIVATE | SWP_NOZORDER,
                            );
                        }
                    }
                    FollowAction::Copy => {
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
                            // [lc-1925] 覆盖层已是透明窗（无黑底可残影），不再隐藏 ——
                            // 隐藏会让控件「过一会才出现」，得不偿失。
                            let _ = moving;
                            stable_since = None;
                            // [lc-1924] z 序交给 owner 关系（owned 恒在 owner 之上），这里只做几何同步
                            SetWindowPos(
                                ov as _,
                                std::ptr::null_mut(),
                                m.left,
                                m.top,
                                m.right - m.left,
                                m.bottom - m.top,
                                SWP_NOACTIVATE | SWP_NOZORDER,
                            );
                            continue;
                        }
                    }
                }
                // 已对齐：若因移动被隐藏，稳定 220ms 后恢复显示（拖完即回显控件）
                // [lc-1940] 只有 UI 就绪时才允许回显 —— 否则（Lua OSC 兜底 / 非透明）
                // 这里绝不能把覆盖层重新露出来变成黑块。
                if hidden_for_move && crate::mpvwin::overlay_ui_ready() {
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
        OVERLAY_FOLLOW_RUNNING.store(false, Ordering::SeqCst);
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
        // [lc-1948] 全屏态挂起重钉：几何由显示器矩形独占（跟随循环每拍断言 +
        // ensure_fullscreen_geometry），任何迟到的视频区矩形都不得把锚窗拽回旧矩形。
        // 显示/还原 iconic 的处理必须在门之前做完（最小化恢复路径仍依赖它）。
        if PLAYER_FULLSCREEN.load(Ordering::SeqCst) {
            return;
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────── [lc-1949] 视频区鼠标转发（替代探针拨位）───────────────

    /// [lc-1949] 消息打包：单击按下 = WM_LBUTTONDOWN + MK_LBUTTON，lParam 低字 x 高字 y。
    #[test]
    fn forward_pack_left_down() {
        let (msg, wp, lp) = pack_mouse_message("down", 320, 240, 0, 0, (0, 0)).unwrap();
        assert_eq!(msg, WM_LBUTTONDOWN);
        assert_eq!(wp, MK_LBUTTON as isize, "按下必须带 MK_LBUTTON");
        assert_eq!(lp & 0xFFFF, 320, "lParam 低 16 位 = x");
        assert_eq!(lp >> 16, 240, "lParam 高 16 位 = y");
    }

    /// [lc-1949] 移动/抬起/双击的消息号与 MK 标志语义；Ctrl/Shift 从前端原样透传。
    #[test]
    fn forward_pack_move_up_dbl_flags() {
        let (msg, wp, _) = pack_mouse_message("move", 10, 20, MK_CONTROL, 0, (0, 0)).unwrap();
        assert_eq!(msg, WM_MOUSEMOVE);
        assert_eq!(wp, MK_CONTROL as isize);
        let (msg, wp, _) = pack_mouse_message("up", 10, 20, MK_CONTROL, 0, (0, 0)).unwrap();
        assert_eq!(msg, WM_LBUTTONUP);
        assert_eq!(wp, MK_CONTROL as isize, "抬起时不带按钮位");
        let (msg, wp, _) = pack_mouse_message("dbl", 10, 20, 0, 0, (0, 0)).unwrap();
        assert_eq!(msg, WM_LBUTTONDBLCLK, "双击必须发 LBUTTONDBLCLK（PostMessage 不会自动合成）");
        assert_eq!(wp, MK_LBUTTON as isize);
        let (_, wp, _) = pack_mouse_message("down", 0, 0, MK_SHIFT, 0, (0, 0)).unwrap();
        assert_eq!(wp, (MK_SHIFT | MK_LBUTTON) as isize, "修饰键与按钮位共存");
    }

    /// [lc-1949] 右键 / 中键 / 滚轮：滚轮 wParam 高字 = 增量，lParam = **屏幕**坐标
    /// （mpv 按 OS 约定 ScreenToClient 换回客户区），客户区原点由 win_origin 平移。
    #[test]
    fn forward_pack_right_mid_wheel() {
        let (msg, wp, _) = pack_mouse_message("rdown", 5, 6, 0, 0, (0, 0)).unwrap();
        assert_eq!(msg, WM_RBUTTONDOWN);
        assert_eq!(wp, MK_RBUTTON as isize);
        let (msg, _, _) = pack_mouse_message("rup", 5, 6, 0, 0, (0, 0)).unwrap();
        assert_eq!(msg, WM_RBUTTONUP);
        let (msg, wp, _) = pack_mouse_message("mdown", 5, 6, 0, 0, (0, 0)).unwrap();
        assert_eq!(msg, WM_MBUTTONDOWN);
        assert_eq!(wp, MK_MBUTTON as isize);
        // 向上滚一格：delta=+120
        let (msg, wp, lp) = pack_mouse_message("wheel", 30, 40, MK_CONTROL, 120, (100, 200)).unwrap();
        assert_eq!(msg, WM_MOUSEWHEEL);
        assert_eq!(wp >> 16, 120, "wParam 高 16 位 = 滚动增量");
        assert_eq!(wp & 0xFFFF, MK_CONTROL as isize);
        assert_eq!(lp & 0xFFFF, 130, "wheel lParam = 屏幕坐标（客户区 + 窗口原点）");
        assert_eq!(lp >> 16, 240);
        // 向下滚：负增量按补码进高字
        let (_, wp, _) = pack_mouse_message("wheel", 30, 40, 0, -120, (0, 0)).unwrap();
        assert_eq!(wp >> 16, 65416, "-120 的 16 位补码（65536 - 120）");
    }

    /// [lc-1949] 负坐标（多屏负向排列）的 lParam 打包：按 u16 截断补码，与 MAKELPARAM 一致。
    #[test]
    fn forward_pack_negative_coords() {
        let (_, _, lp) = pack_mouse_message("move", -5, -3, 0, 0, (0, 0)).unwrap();
        assert_eq!(lp & 0xFFFF, 65531);
        assert_eq!(lp >> 16, 65533);
        // 未知 kind 拒绝（前端拼错的 kind 不该变成任意消息）
        assert!(pack_mouse_message("explode", 0, 0, 0, 0, (0, 0)).is_none());
    }

    /// [lc-1949] 双向验证：旧架构的「点击先等探针把穿透位拨过来」路径在新架构里
    /// **不存在**——打包函数只做纯消息换算，没有任何时间/轮询/状态输入。
    /// 固化手段：断言输入完全决定输出（同输入两次打包结果逐位相同）。
    #[test]
    fn forward_pack_is_stateless() {
        let a = pack_mouse_message("down", 100, 100, 0, 0, (0, 0));
        let b = pack_mouse_message("down", 100, 100, 0, 0, (0, 0));
        assert_eq!(a, b, "转发不依赖任何探针/时间状态：同输入必须同输出");
    }

    /// [lc-1946] 退出流程去重：连点关闭 / 关闭后又按 Esc 只跑第一条清理链。
    #[test]
    fn exit_flow_dedup() {
        end_exit_flow();
        assert!(begin_exit_flow(), "第一次退出应当开始");
        assert!(!begin_exit_flow(), "退出进行中，重复请求必须被挡住");
        end_exit_flow();
        assert!(begin_exit_flow(), "收尾后允许下一次退出");
        end_exit_flow();
    }

    /// [lc-1946] 最小化 → 主窗恢复：标记只消费一次。
    #[test]
    fn minimize_restore_flag_roundtrip() {
        clear_minimize_restore();
        assert!(!take_minimize_restore(), "无标记时不重新显示播放层");
        request_minimize_restore();
        assert!(take_minimize_restore(), "恢复主窗时要放回播放层");
        assert!(!take_minimize_restore(), "只消费一次");
    }

    /// [lc-1946] 置顶默认值：每次起播回到 true；hwnd=0 时只改状态不碰窗口。
    #[test]
    fn ontop_state_defaults_to_true() {
        reset_player_ontop();
        assert!(player_ontop(), "起播默认置顶（覆盖层 + mpv 锚窗成对 TOPMOST）");
        set_player_ontop(0, false);
        assert!(!player_ontop(), "取消置顶后状态为 false");
        reset_player_ontop();
        assert!(player_ontop(), "下一次起播回到置顶");
    }

    /// [lc-1947] 全屏期间恒置顶（压过任务栏）；退出后回到用户「置顶」意图。
    #[test]
    fn fullscreen_forces_topmost() {
        assert!(effective_topmost(false, true), "全屏中即使取消置顶也必须 TOPMOST（任务栏）");
        assert!(effective_topmost(true, true));
        assert!(!effective_topmost(false, false), "非全屏时按用户意图");
        assert!(effective_topmost(true, false));
    }

    /// [lc-1947] 退出全屏的恢复步骤：先清样式 → 回到进入前矩形 → 按需最大化 → 恢复置顶意图。
    #[test]
    fn fullscreen_restore_order() {
        let steps = fullscreen_restore_steps((660, 292, 2520, 1575), false, true);
        assert_eq!(
            steps,
            vec![
                FullscreenRestoreStep::Restore,
                FullscreenRestoreStep::MoveTo(660, 292, 2520, 1575),
                FullscreenRestoreStep::Topmost(true),
            ]
        );
        let steps = fullscreen_restore_steps((0, 0, 3840, 2160), true, false);
        assert_eq!(
            steps,
            vec![
                FullscreenRestoreStep::Restore,
                FullscreenRestoreStep::MoveTo(0, 0, 3840, 2160),
                FullscreenRestoreStep::Maximize,
                FullscreenRestoreStep::Topmost(false),
            ]
        );
        // 最大化必须排在 MoveTo 之后：SW_MAXIMIZE 会让系统用工作区几何覆盖还原矩形
        let mi = steps
            .iter()
            .position(|s| matches!(s, FullscreenRestoreStep::Maximize))
            .unwrap();
        let mp = steps
            .iter()
            .position(|s| matches!(s, FullscreenRestoreStep::MoveTo(..)))
            .unwrap();
        assert!(mp < mi, "MoveTo 先于 Maximize");
    }

    /// [lc-1948] 跟随循环决策矩阵：非全屏维持抄写；全屏一律断言/保持，绝不抄写。
    #[test]
    fn follow_loop_fullscreen_gate() {
        let mon = (0, 0, 3840, 2160);
        let anchor_on = (0, 0, 3840, 2160);
        let overlay_on = (0, 0, 3840, 2160);
        // 非全屏：即使两层恰好等于显示器矩形，也维持原「抄写」行为
        assert_eq!(
            follow_loop_action(false, anchor_on, overlay_on, mon),
            FollowAction::Copy
        );
        // 全屏 + 两层就位：不动
        assert_eq!(
            follow_loop_action(true, anchor_on, overlay_on, mon),
            FollowAction::Hold
        );
        // 全屏 + 覆盖层被 mpv 的记忆矩形带偏（实测 3851x2171@-11,-11）→ 复位
        assert_eq!(
            follow_loop_action(true, anchor_on, (-11, -11, 3840, 2160), mon),
            FollowAction::Assert(0, 0, 3840, 2160)
        );
        // 全屏 + 锚窗被 mpv 的 VO 拖回它记忆的矩形 → 复位
        assert_eq!(
            follow_loop_action(true, (-11, -11, 3851, 2171), anchor_on, mon),
            FollowAction::Assert(0, 0, 3840, 2160)
        );
        // 非全屏 + 锚窗偏离：照旧抄写（跟随语义不变）
        assert_eq!(
            follow_loop_action(false, (660, 292, 3180, 1867), (0, 0, 3840, 2160), mon),
            FollowAction::Copy
        );
        // 显示器矩形按 (x,y,w,h) 换算成 (l,t,r,b) 后再比较：多屏负坐标排列也成立
        assert_eq!(
            follow_loop_action(true, (-1920, 0, 0, 2160), (-1920, 0, 0, 2160), (-1920, 0, 1920, 2160)),
            FollowAction::Hold
        );
    }

    /// [lc-1948] 双向验证：lc-1947 的旧决策（只要两层矩形不一致就抄写、不看全屏态）
    /// 对同样的输入会做出什么 —— 它会把 mpv 记忆矩形抄给覆盖层，违反全屏不变量，
    /// 正是本次要修的行为。此测试固化「旧逻辑 FAIL / 新逻辑 PASS」的分界。
    #[test]
    fn follow_loop_old_behavior_fails_fullscreen_invariant() {
        let mon = (0, 0, 3840, 2160);
        // 旧逻辑：fullscreen 无关，锚窗≠覆盖层 → 抄写
        let old_action = |anchor: (i32, i32, i32, i32), overlay: (i32, i32, i32, i32)| {
            if anchor != overlay {
                "copy"
            } else {
                "noop"
            }
        };
        // 全屏中 mpv VO 把锚窗拖回记忆矩形（-11,-11 3851x2171），覆盖层还在显示器矩形上
        let anchor = (-11, -11, 3851, 2171);
        let overlay = (-11, -11, 3840, 2160);
        assert_eq!(old_action(anchor, overlay), "copy", "旧逻辑会抄写（违反全屏不变量）");
        assert_ne!(
            follow_loop_action(true, anchor, overlay, mon),
            FollowAction::Copy,
            "新逻辑在全屏态必须拒绝抄写"
        );
        assert!(matches!(
            follow_loop_action(true, anchor, overlay, mon),
            FollowAction::Assert(..)
        ));
    }
}
