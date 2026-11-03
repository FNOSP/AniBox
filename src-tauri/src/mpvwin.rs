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
//! [lc-1964] **根治「全屏后全部按钮失效（点击漏给 mpv 变播放/暂停）」**（实机日志
//! %TEMP%/anibox.log + anibox-mpv.log 法证定案）：
//!
//!   · **真根因 = lc-1952 的 z 序「自愈」方向反了**。Win32 `SetWindowPos` 的
//!     `hWndInsertAfter` 语义是「被定位窗插到该窗**正下方**」（实证：三窗受控带 +
//!     GetWindow 探测，SetWindowPos(B, insertAfter=A) 后 next(A)==B）。于是
//!     `place_overlay_above_anchor(overlay, anchor)` 实际把覆盖层插到锚窗**正下方**；
//!     33ms 跟随循环每拍复检 `GW_HWNDPREV` 发现「仍未紧贴锚窗上方」就再插一次 ——
//!     自愈变成了**每 33ms 主动把覆盖层压回 mpv 之下**。全屏一旦触发 mpv VO 的
//!     SetWindowPos（进全屏 / ontop_mode.lua 暂停联动），覆盖层就被钉死在视频层
//!     之下直到退出全屏：chrome 整层收不到任何真实命中，全部点击物理落进 mpv
//!     （实机日志：两次全屏期间 `fntv_osc-click` + `cycle pause/fullscreen`，
//!      hover 事务 ack 全部 applied=true —— 穿透位拨的是窗下之窗，自然无效）。
//!     窗口模式之所以正常：z 序自愈只挂在全屏分支里，起播时「锚窗先 TOPMOST、
//!     覆盖层后 TOPMOST」的自然带序（覆盖层在带顶）一直没被破坏。
//!   · 修复（对齐 Polaris `apply_screen_saver_level`）：
//!     ① 全屏期覆盖层用 **NOTOPMOST→TOPMOST 重置法**抬到 TOPMOST 带最顶
//!        （`overlay_raise_screen_saver_level`；已 TOPMOST 的窗再调 HWND_TOPMOST
//!        不保证带内重排，先摘带再回带才强制重排）；
//!     ② `place_overlay_above_anchor` 改为 `InsertAfter=GetWindow(锚窗,GW_HWNDPREV)`
//!        （锚窗已在带顶时用 HWND_TOP），窗口模式与退出全屏的带内定位用；
//!     ③ 33ms 跟随循环：全屏违例 → 屏保级抬顶；窗口模式新增同带 z 序断言（此前
//!        只在全屏分支里有，窗口模式 ontop 拨动后无人兜底）。
//!   · **架构保险（D）**：Rust 探针侧新增**控件带几何热区**——前端 1s 心跳上报
//!     `[data-chrome-hit]` 矩形（`mpv_overlay_hotzones`），探针每拍直接对「光标落在
//!     控件带内」翻可点位，完全不依赖 WebView elementFromPoint；前端从未上报时用
//!     顶栏/底栏固定带兜底。WebView 判定链全挂时控件仍可点。
//!
//! [lc-1974] **全屏废除穿透机制（用户第 N 次实证后的模式级修法）**：lc-1964 的
//! z 序自愈 + 探针热区直翻上线后用户依旧「全屏全部按钮失效、点击变播放/暂停」。
//! 实机日志法证（%TEMP%/anibox.log + anibox-mpv.log，全屏 app 31.4s~44.3s 段）：
//!   · mpv 侧 `cycle pause` 实证点击物理漏入 mpv；
//!   · hover 事务 ack 全部 ok=true（穿透位读回真值，位确实翻过）—— 问题不在拨位；
//!   · `全屏中锚窗丢失 TOPMOST → 复位` 每 33ms 一条贯穿整个全屏：mpv 的
//!     ontop_mode.lua 反复摘锚窗 TOPMOST，跟随循环用 HWND_TOPMOST 重插把它放回
//!     **带顶（盖到覆盖层之上）**，旧 z 序断言带 band_consistent 门，锚窗刚被抬回
//!     的那一拍跳过修复 —— 「mpv 在上」的窗口期反复出现，真实点击落进 mpv；
//!   · `探针热区直翻可点` 全程零触发：chrome 隐藏时前端 1s 心跳上报空表且新鲜，
//!     `hotzones_evaluate` 恒走 reported+空表 → 恒不命中，保险丝（D）从未参战。
//! 结论：穿透默认态在全屏下没有一环是可靠的，而全屏**根本不需要拖窗** —— 穿透的
//! 全部收益为零、全部风险照单全收。结构性修法（正确性优先）：
//!   ① **全屏输入锁**（`FULLSCREEN_INPUT_LOCK`）：进全屏即锁，穿透位锁定为「恒摘除」；
//!     `overlay_apply_input_passthrough` 裁决拒绝任何置位请求（ok=true，不触发前端
//!     重试风暴）；hover=false 请求 ack 按请求值谎报成功，真实状态由锁保证。
//!   ② **覆盖层整窗可点**：视频区点击由前端按 `data-chrome-hit` 分流，经既有
//!     `mpv_forward_mouse`（lc-1949 PostMessage 链原样复用）投给 mpv —— 单击暂停/
//!     双击全屏/滚轮照常是 mpv 原生行为；全屏不需要拖窗，「慢半拍」可接受。
//!   ③ **跟随循环全屏分支重排**：锚窗 TOPMOST 复位后**同拍**复检 z 序并屏保级抬顶
//!    （去掉 band_consistent 门，消除「锚窗刚抬回、覆盖层还没抬」的漏点窗口期），
//!    并每拍自愈穿透位残留。
//!   ④ **窗口模式不动**：Polaris 穿透 + hover + 探针热区直翻照旧（实测是好的）。
//!   ⑤ 埋点：每次穿透位实写打 `[lc-1974] 穿透位 0→1 reason=...`（before/after+原因，
//!     变化才打）；全屏期探针 WindowFromPoint 命中窗口变化时打 hwnd+类名 ——
//!     下次复现一眼定案「点击落进了哪个窗」。
//!
//! [lc-1981] **33ms 拉锯定案 + 根治**（用户复现日志：`[lc-1949] 全屏中锚窗丢失
//! TOPMOST → 复位` 每 33~40ms 一条贯穿全屏 + 控件全失效）。法证结论：
//!   · 用户跑的二进制是 **lc-1949 时代产物**（日志标签为证；lc-1974 修复 03:44 才提交，
//!     `[lc-1974]` 埋点一条都没有）—— 但体感失效是真实的，旧机制本身就是坏的；
//!   · 拉锯双方：mpv 的 `ontop_mode.lua`（暂停联动把 `ontop` 置 false = 对锚窗
//!     `SetWindowPos(HWND_NOTOPMOST)`）摘锚窗 TOPMOST ↔ 跟随循环每拍发现「锚窗丢
//!     TOPMOST」就 `HWND_TOPMOST` 重插复位。重插把锚窗放到 **TOPMOST 带顶（盖到
//!     覆盖层之上）**，而 lc-1949 时代的全屏分支没有任何 z 序复检 —— 锚窗被我们
//!     亲手抬到覆盖层之上后稳定反转，覆盖层整层被埋，点击全漏给 mpv。这就是
//!     「全屏后全部按钮失效、点击变播放/暂停」的机制级定案。
//! 根治（不再有「谁在 30Hz 打架」的土壤）：
//!   ① **跟随循环全屏分支只动覆盖层**：删除逐拍「锚窗 TOPMOST 复位」（刷屏日志
//!     与带顶重插一并消失）；z 序保障改为**相对锚窗断言**（覆盖层必须压在锚窗
//!     之上，不满位才修：跨带失守走屏保级抬顶、同带不贴位走带内 InsertAfter），
//!     锚窗在任何路径下都**永不被我们抬起**。
//!   ② **锚窗置顶交给 mpv 自己**：进全屏一次性下发 `script-message fntv-ontop-fs on`
//!     → `ontop_mode.lua`（本地脚本）锁定 `ontop=true` 并停用暂停置底联动；退出
//!     全屏发 `off` 恢复三态联动。任务栏遮挡由 Windows 对前台满屏窗口的自动隐藏
//!     兜底（实测依据不足，留待实机确认，不再为此引入拉锯）。
//!   ③ **`effective_topmost` 恒真强制退役**：全屏期用户「取消置顶」意图照实生效
//!     （置顶按钮不再被全屏态覆写），退出全屏按 PLAYER_ONTOP 恢复（原样保留）。
//!   ④ **诊断快照 `[lc-1981]`**：全屏期探针每拍算一份自足快照（光标物理坐标、
//!     WindowFromPoint 命中 hwnd+类名+标题+归类「覆盖层/mpv锚窗/其他」、覆盖层与
//!     锚窗的实际 z 序关系（GetWindow 链实测）、两层矩形、覆盖层穿透位、全屏/输入
//!     锁标志），状态变化即打、无变化 2s 心跳；每次 ex-style 写入与 z 序操作
//!     （topmost 拨位 / 屏保级抬顶 / 带内插顶）都打 before→after + 原因。下次复现
//!     按行定案，不再猜。
//!
//! [lc-1952] 实机回归两修：
//!   · **全屏后控件全点不中**：mpv 的 VO 在全屏切换时 SetWindowPos 自己，把锚窗提到
//!     覆盖层之上（对已 TOPMOST 的覆盖层再调 HWND_TOPMOST 不保证带内重排，owner 关系
//!     也不即时兜底）—— 此后真实鼠标命中全部落进 mpv（实机日志：裸 WM_MOUSEMOVE
//!     500+ 条/秒直达 mpv），覆盖层整层收不到事件。lc-1952 试图用 `InsertAfter=锚窗`
//!     显式带内定位修复 —— 但方向反了（见 [lc-1964]），lc-1964 推翻重做。
//!   · **最小化漏出黑色背景**：旧顺序先收播放层再动主窗（主窗若被真实显示过，黑底
//!     播放页整个露出）+ 旧 `SW_SHOWMINNOACTIVE` 先行对隐藏窗物化恢复态帧。修复 =
//!     主窗先最小化（此刻播放层还盖着）+ `minimize_show_steps`（先 SW_MINIMIZE 进
//!     最小化态、再 SW_SHOWMINNOACTIVE 补任务栏条目，全程无恢复态帧）。
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
//!
//! [lc-1960] 输入架构按 Polaris 法证结论**二次重做**（lc-1949 实机两症状：
//! 「全屏后任意按钮都变成播放暂停」「控件反应慢半拍还经常无反应」）：
//!
//!   · **穿透默认态**：覆盖层回到 Polaris 的本相 —— `WS_EX_TRANSPARENT` 在控件隐藏
//!     （绝大多数时间）**恒置位**，真实鼠标从 OS 层直达 mpv（单击暂停/双击全屏/拖动
//!     全走 mpv 原生 input.conf），WebView 收不到、也**不需要**转发任何空白区事件。
//!     lc-1949「常驻可点」把整面鼠标事件流灌进 WebView2 再逐条 invoke 转发，才是
//!     「慢半拍/无反应」的结构根源；而常驻可点要求覆盖层对 mpv 保持全时 z 序优势，
//!     全屏切换瞬间任何一次失守，点击就整体漏给 mpv 变成暂停 —— 症状一的根源。
//!   · **hover 事务协议**（Polaris mpv-overlay-hover 同款）：光标悬在控件上时前端
//!     `invoke('mpv_overlay_hover', {hover, generation, requestSeq})` → Rust 拨
//!     WS_EX_TRANSPARENT 并**读回真实 ex-style 位**作 ack（tauri 的
//!     set_ignore_cursor_events 会把 LAYERED 一起摘掉 lc-1924 的教训，这里全程
//!     原生 SetWindowLongPtrW）；前端按 generation/requestSeq 对账 + 40/120/300/700ms
//!     退避重试，穿透态永远不漂移。
//!   · **Rust 光标探针**（Polaris [overlay_probe] 同款，50ms GetCursorPos）：穿透态下
//!     WebView 收不到任何鼠标事件，控件续显与 hover 判定由 Rust 推送
//!     `overlay-force-hover-check`（物理坐标 + surface 尺寸 + inside）与
//!     `overlay-mouse-activity` 驱动；`inside=false` 是「真离开播放器」的权威信号。
//!   · 50ms 翻位窗口内的一击不丢：控件在 hover 翻位前收到的点击会漏给 mpv（= 一次
//!     正常的空白区暂停语义），与 Polaris 行为一致；持续悬停时翻位先行，点击零等待。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::System::SystemServices::{MK_LBUTTON, MK_MBUTTON, MK_RBUTTON};
// MK_CONTROL / MK_SHIFT 只在前端传来的 flags 位里出现（8 / 4），单测用它们验证透传
#[cfg(test)]
use windows_sys::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetCursorPos, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed,
    PostMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WindowFromPoint, GWL_EXSTYLE,
    GWLP_HWNDPARENT, GW_HWNDNEXT, GW_HWNDPREV, HWND_NOTOPMOST, HWND_TOPMOST, SW_HIDE, SW_MAXIMIZE,
    SW_MINIMIZE, SW_RESTORE, SW_SHOW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
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
/// [lc-1974] 全屏输入锁：全屏期穿透位（WS_EX_TRANSPARENT）**锁定为摘除**。
///
/// 用户第 N 次实证（%TEMP%/anibox.log + anibox-mpv.log 法证，见模块头 [lc-1974]）：
/// 穿透默认态 + hover 链在全屏下任一环抖动（mpv ontop_mode.lua 每拍摘锚窗 TOPMOST
/// 触发 z 序拉锯、elementFromPoint 映射抖动、心跳空表），点击就漏进 mpv 变播放/暂停。
/// 全屏**根本不需要拖窗**，穿透机制的全部收益在全屏下为零 —— 结构性修法：
/// 全屏期把穿透位锁死在「可点」，覆盖层整窗收输入；视频区点击由前端按
/// `data-chrome-hit` 分流经 `mpv_forward_mouse` 转给 mpv（单击暂停/双击全屏/滚轮
/// 全是 mpv 原生行为）。窗口模式保持 Polaris 穿透 + hover 不动（实测是好的）。
static FULLSCREEN_INPUT_LOCK: AtomicBool = AtomicBool::new(false);
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
/// [lc-1960] 50ms 光标探针是否已在跑（与跟随循环同生命周期，幂等）。
static OVERLAY_PROBE_RUNNING: AtomicBool = AtomicBool::new(false);

// ─────────────── [lc-1960] hover 事务协议状态（Polaris generation/seq 同款）───────────────

/// hover 协议的代际：每次起播接线（show_player_overlay）递增，旧代际的前端状态机
/// 全部作废重对账。u64，进程内单调。
static HOVER_GENERATION: AtomicU64 = AtomicU64::new(0);
/// 最近一次落地（或已见）的 requestSeq：旧 seq 的迟到请求不落地，只 ack 真实态。
static HOVER_LAST_SEQ: AtomicU64 = AtomicU64::new(0);
/// 探针最近上报的「光标是否在覆盖层窗口内」（inside），供对账与日志。
static PROBE_INSIDE: AtomicBool = AtomicBool::new(false);
/// [lc-1964] 前端最近一次 hover 事务请求的目标态（menu 打开/拖动锁期间前端会主动
/// 请求恒可交互）。探针热区自动翻位退出时必须尊重它，不能和前端抢穿透位。
static FRONTEND_HOVER_LAST: AtomicBool = AtomicBool::new(false);
/// [lc-1964] 探针热区自动翻位是否处于「已翻可点」态（Rust 自己翻的才由 Rust 还原，
/// 避免覆盖前端主动请求的可交互态）。
static AUTO_HOVER_ACTIVE: AtomicBool = AtomicBool::new(false);

// ─────────────── [lc-1964] 控件带几何热区（Rust 直翻可点位的保险丝）───────────────
//
// 前端 1s 心跳把 `[data-chrome-hit]` 矩形（CSS 视口坐标）+ 视口尺寸上报到
// `overlay_set_hotzones`；探针每拍把「光标窗口内物理坐标」按 surface/clientWidth
// 比例换算后直接命中测试，命中即翻掉 WS_EX_TRANSPARENT —— 不发事件、不等前端
// elementFromPoint。WebView 判定链全挂（JS 死 / 事件丢 / 映射错）时控件仍可点。
// 前端从未上报（页面加载即死）→ 退回顶栏/底栏固定带；上报过但停更（>3s，即
// 3 次心跳未达 = JS 挂了，chrome 冻结在最后可见态）→ 同样退回固定带。

/// 控件带矩形（CSS 视口坐标 `[x, y, w, h]`），前端心跳上报。
static HOTZONES_CSS: Mutex<Vec<[f32; 4]>> = Mutex::new(Vec::new());
/// 上报时的 CSS 视口尺寸（物理↔CSS 换算基准）。
static HOTZONES_VIEWPORT: Mutex<(f32, f32)> = Mutex::new((0.0, 0.0));
/// 最近一次上报时刻；None = 前端从未上报（用固定带兜底）。
static HOTZONES_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
/// 心跳容忍窗：超过它视为前端停更（退回固定带）。3s = chrome 自动隐藏(3s)的量级，
/// 期间足够容忍 GC/卡顿丢几拍心跳。
const HOTZONES_FRESH: std::time::Duration = std::time::Duration::from_secs(3);

/// [lc-1964] 前端上报控件带矩形（`mpv_overlay_hotzones` 命令落地）。
/// `viewport = (w, h)` 为上报时的 CSS 视口尺寸；(0,0) = 视口未知（视为从未上报）。
pub fn overlay_set_hotzones(zones: Vec<[f32; 4]>, viewport: (f32, f32)) {
    *HOTZONES_CSS.lock().unwrap_or_else(|e| e.into_inner()) = zones;
    *HOTZONES_VIEWPORT.lock().unwrap_or_else(|e| e.into_inner()) = viewport;
    *HOTZONES_AT.lock().unwrap_or_else(|e| e.into_inner()) =
        if viewport.0 > 0.0 && viewport.1 > 0.0 {
            Some(std::time::Instant::now())
        } else {
            None
        };
}

/// [lc-1964] 纯逻辑：固定兜底带（顶栏 + 底栏）。surface 为窗口内物理像素尺寸。
/// 顶带 0..14%（右上窗控 + 顶栏），底带 74%..100%（底栏 dock + 音量条）。
/// 比例而非固定像素：全屏 3840x2160 与窗口 2520x1575 共用一套判据。
pub fn fallback_band_hit(surface: (i32, i32), px: i32, py: i32) -> bool {
    let (w, h) = (surface.0.max(1), surface.1.max(1));
    let top = (h as f64 * 0.14) as i32;
    let bottom_start = (h as f64 * 0.74) as i32;
    px >= 0 && px < w && (py >= 0 && py < top || py >= bottom_start && py < h)
}

/// [lc-1964] 纯逻辑：上报热区命中测试。`zones` 为 CSS 视口矩形，`viewport` 为上报
/// 时视口尺寸，`surface` 为当前窗口物理尺寸，`(px, py)` 为光标窗口内物理坐标。
/// 换算 = 物理点 × (viewport / surface) → CSS 点（与前端 probePoint 同一比例，
/// 方向相反：这里是把物理坐标拉回 CSS 栅格）。
pub fn hotzone_hit(
    zones: &[[f32; 4]],
    viewport: (f32, f32),
    surface: (i32, i32),
    px: i32,
    py: i32,
) -> bool {
    if zones.is_empty() || viewport.0 <= 0.0 || viewport.1 <= 0.0 || surface.0 <= 0 || surface.1 <= 0
    {
        return false;
    }
    let cx = (px as f32) * viewport.0 / surface.0 as f32;
    let cy = (py as f32) * viewport.1 / surface.1 as f32;
    zones
        .iter()
        .any(|[x, y, w, h]| cx >= *x && cx < x + w && cy >= *y && cy < y + h)
}

/// [lc-1964] 探针每拍的热区裁决：返回 `(是否命中控件带, 命中来源描述)`。
/// 前端从未上报或心跳停更 → 固定带兜底；否则用上报矩形（chrome 隐藏时前端上报
/// 空表 → 恒不命中，点击照旧穿透给 mpv）。
pub fn hotzones_evaluate(surface: (i32, i32), px: i32, py: i32) -> (bool, &'static str) {
    let at = *HOTZONES_AT.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = matches!(at, Some(t) if t.elapsed() < HOTZONES_FRESH);
    if fresh {
        let zones = HOTZONES_CSS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let vp = *HOTZONES_VIEWPORT.lock().unwrap_or_else(|e| e.into_inner());
        (hotzone_hit(&zones, vp, surface, px, py), "reported")
    } else {
        (fallback_band_hit(surface, px, py), "fallback")
    }
}

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

/// [lc-1939/1949] 覆盖层窗样式基座：NOACTIVATE（点击不抢焦点）+ TOOLWINDOW（不进任务栏/
/// Alt-Tab）+ **LAYERED**（WebView2 的逐像素透明靠它合成）。
///
/// [lc-1960] 穿透位（WS_EX_TRANSPARENT）不在这里写 —— 它由
/// [`overlay_apply_input_passthrough`] 单点动态管理（Polaris 穿透默认态）。
/// 也不能改用 tauri 的 `set_ignore_cursor_events`（tao 把 TRANSPARENT 与 LAYERED
/// 置成一组，会连透明一起摘掉 → 黑块盖死视频，lc-1924~1931 的真根因）。
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
            crate::libmpv::trace(&format!(
                "[lc-1981] ex-style hwnd=0x{hwnd:X} 0x{ex:X}→0x{next:X} reason=style-base（NOACTIVATE|TOOLWINDOW|LAYERED 一次成型）"
            ));
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
            crate::libmpv::trace(&format!(
                "[lc-1981] ex-style hwnd=0x{hwnd:X} NOACTIVATE {}→{} reason={}",
                i32::from(ex & (WS_EX_NOACTIVATE as isize) != 0),
                i32::from(next & (WS_EX_NOACTIVATE as isize) != 0),
                if on { "focusable-on" } else { "focusable-off" }
            ));
        }
    }
}

// ─────────────── [lc-1960] 穿透位单一写入点 + hover 事务 ───────────────

/// [lc-1974] 纯逻辑：全屏输入锁下某一次「拨穿透」请求的裁决。
/// 返回 `(是否允许写穿透位, 是否视为成功)`。
///
///   · 摘穿透（passthrough=false）恒放行 —— 锁定期也要能把位摘掉（进全屏自愈用）；
///   · 锁定期置穿透被拒绝（写=false）但**视为成功** —— 锁本身就是权威结果，
///     调用方（hover 状态机）若按失败处理会进 40/120/300/700ms 重试风暴；
///   · 非锁定期照旧放行（窗口模式 Polaris 语义，实测是好的，不动）。
///
/// 单测双向锚定：旧路径（无锁概念，置穿透恒放行）对「锁定期拒绝」断言必 FAIL。
pub fn passthrough_request_adjudication(passthrough: bool, lock: bool) -> (bool, bool) {
    if !passthrough {
        return (true, true);
    }
    if lock {
        return (false, true);
    }
    (true, true)
}

/// [lc-1960] 覆盖层穿透位的**唯一**写入点：拨/摘 `WS_EX_TRANSPARENT`，然后**读回
/// 真实 ex-style 位**。返回 `(实际穿透态, 是否达成目标态)`。
///
/// [lc-1974] 全屏输入锁：`FULLSCREEN_INPUT_LOCK` 置位期间，任何「置穿透」请求都被
/// 裁决拒绝（位保持摘除、ok=true）；`reason` 记入位变化埋点，每次实写都留
/// `[lc-1974]` 日志（before/after 位 + 原因），复现一眼定案。
///
/// 为什么不用 tauri 的 `set_ignore_cursor_events`：tao 把 TRANSPARENT 与 LAYERED
/// 绑成一组拨动（lc-1924~1931 的黑块真根因），这里全程原生 SetWindowLongPtrW，
/// NOACTIVATE / TOOLWINDOW / LAYERED 三位保底永远不丢。
pub fn overlay_apply_input_passthrough(hwnd: isize, passthrough: bool, reason: &str) -> (bool, bool) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return (true, false);
    }
    let lock = FULLSCREEN_INPUT_LOCK.load(Ordering::SeqCst);
    let (write_allowed, ok) = passthrough_request_adjudication(passthrough, lock);
    unsafe {
        let ex = GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE);
        let before = ex & (WS_EX_TRANSPARENT as isize) != 0;
        let base = ex
            | (WS_EX_NOACTIVATE as isize)
            | (WS_EX_TOOLWINDOW as isize)
            | (WS_EX_LAYERED as isize);
        let next = if !passthrough {
            base & !(WS_EX_TRANSPARENT as isize)
        } else if write_allowed {
            base | (WS_EX_TRANSPARENT as isize)
        } else {
            base
        };
        if next != ex {
            SetWindowLongPtrW(hwnd as _, GWL_EXSTYLE, next);
        }
        let now = GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE);
        let applied = now & (WS_EX_TRANSPARENT as isize) != 0;
        if applied != before {
            crate::libmpv::trace(&format!(
                "[lc-1974] 穿透位 {}→{} reason={reason}{}",
                i32::from(before),
                i32::from(applied),
                if lock { "（全屏输入锁）" } else { "" }
            ));
        }
        (applied, ok)
    }
}

/// hover 事务的 ack（前端对账字段与 Polaris 逐一同名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoverAck {
    /// 服务端当前代际（前端落后则据此重对账）。
    pub generation: u64,
    /// 穿透位读回后的真实「可交互」态（true = 控件可收点击）。
    pub applied_hover: bool,
    /// 本次请求是否真正落地（seq 落后 / 拨位失败 = false，前端进退避重试）。
    pub ok: bool,
}

/// hover 协议当前代际。
pub fn hover_generation() -> u64 {
    HOVER_GENERATION.load(Ordering::SeqCst)
}

/// [lc-1960] 只读：覆盖层当前真实穿透位（对账/快照用，不写窗口）。
pub fn overlay_input_passthrough_now(hwnd: isize) -> bool {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return true;
    }
    unsafe { GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE) & (WS_EX_TRANSPARENT as isize) != 0 }
}

/// 新会话起播接线：代际 +1（旧代际前端状态机作废），seq 清零。
pub fn hover_bump_generation() -> u64 {
    let g = HOVER_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    HOVER_LAST_SEQ.store(0, Ordering::SeqCst);
    g
}

/// [lc-1960] 纯逻辑：hover 请求的 seq 单调门。返回 `(是否放行, 新的已见 seq)`。
///
/// `request_seq` 落后于已见 seq = 迟到请求（前端重试与探针事件并发），不放行 ——
/// 旧决策不得覆盖新决策。0 视为「未带 seq」恒放行（不推进已见水位）。
pub fn hover_seq_gate(last_seq: u64, request_seq: u64) -> (bool, u64) {
    if request_seq == 0 {
        return (true, last_seq);
    }
    if request_seq < last_seq {
        return (false, last_seq);
    }
    (true, request_seq)
}

/// [lc-1960] hover 事务落地：把穿透位拨到 `!hover` 并读回。
///
/// seq 单调守门（[`hover_seq_gate`]）：迟到的旧请求不落地（只 ack 真实态）。
/// [lc-1964] 同时记录前端最近请求的目标态（探针热区自动翻位退出时要尊重它）。
/// [lc-1974] 全屏输入锁期间，「拨回穿透」（hover=false）的请求被锁拒绝不落地，
/// 但 ack 按**请求值**谎报成功 —— 否则前端会对一条注定被锁的请求进退避重试风暴；
/// 真实窗口状态由锁保证恒为「可点」，与 ack 语义（hover 期望已达成）一致。
pub fn hover_apply(overlay_hwnd: isize, hover: bool, request_seq: u64) -> HoverAck {
    FRONTEND_HOVER_LAST.store(hover, Ordering::SeqCst);
    let last = HOVER_LAST_SEQ.load(Ordering::SeqCst);
    let (allowed, new_last) = hover_seq_gate(last, request_seq);
    if new_last != last {
        HOVER_LAST_SEQ.store(new_last, Ordering::SeqCst);
    }
    let locked = !hover && FULLSCREEN_INPUT_LOCK.load(Ordering::SeqCst);
    let (actual_pass, ok) = overlay_apply_input_passthrough(overlay_hwnd, !hover, "hover");
    let applied_hover = if locked { hover } else { !actual_pass };
    let ack = HoverAck {
        generation: hover_generation(),
        applied_hover,
        ok: ok && allowed,
    };
    crate::libmpv::trace(&format!(
        "[lat][hover] seq={request_seq} gen={} hover={hover} -> applied={} ok={}{}",
        ack.generation,
        ack.applied_hover,
        ack.ok,
        if allowed { "" } else { " (stale)" }
    ));
    ack
}

/// 探针 inside 最近值（对账/日志用）。
pub fn probe_inside() -> bool {
    PROBE_INSIDE.load(Ordering::SeqCst)
}

fn probe_set_inside(v: bool) {
    PROBE_INSIDE.store(v, Ordering::SeqCst);
}

/// [lc-1960] 50ms 光标探针（Polaris [overlay_probe] 同款）：穿透态下 WebView 收不到
/// 任何鼠标事件，控件续显与 hover 判定全靠 Rust 推送。
///
///   · 光标移动或出入覆盖层矩形 → `overlay-force-hover-check`
///     { physicalX, physicalY, surfacePhysicalWidth, surfacePhysicalHeight, inside }
///     （前端把物理坐标映射到 CSS 视口后 elementFromPoint 判定是否悬在控件上）；
///   · 光标在覆盖层内移动 → 每秒一条 `overlay-mouse-activity` 心跳（控件续显兜底，
///     与 Polaris 的 1s heartbeat 同义）；
///   · `inside=false` 是「真离开播放器」的权威信号（Polaris 拍板二同款）。
///
/// 生命周期与 33ms 跟随循环一致（内嵌会话期间运行，幂等启动）。50ms 采样节拍
/// 固定不变 —— 探针只负责「把光标事实推给前端」，不做任何决策，决策权在前端
/// elementFromPoint（DOM 是热区的唯一真源，与 Polaris 一致，杜绝旧 lc-1939 热区
/// 矩形上报的漂移问题）。
pub fn spawn_overlay_probe(app: tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    if OVERLAY_PROBE_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut last_point: Option<(i32, i32)> = None;
        let mut last_inside = false;
        let mut last_heartbeat = std::time::Instant::now() - std::time::Duration::from_secs(2);
        // [lc-1981] 全屏诊断快照去重状态：上一条快照正文 + 上次打点时刻。
        // 正文变化即打（reason=change）；无变化 2s 心跳（reason=heartbeat）。
        let mut last_snap_body: Option<String> = None;
        let mut last_snap_log = std::time::Instant::now() - std::time::Duration::from_secs(2);
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if !crate::mpvwin::embed_is_active() {
                break;
            }
            let Some(win) = app.get_webview_window("mpv_overlay") else {
                continue;
            };
            if !crate::mpvwin::overlay_ui_ready() {
                continue;
            }
            let Ok(h) = win.hwnd() else { continue };
            let hwnd = h.0 as isize;
            unsafe {
                if IsWindow(hwnd as _) == 0 || IsWindowVisible(hwnd as _) == 0 || IsIconic(hwnd as _) != 0
                {
                    continue;
                }
                let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                if GetWindowRect(hwnd as _, &mut rc) == 0 {
                    continue;
                }
                let mut pt = POINT { x: 0, y: 0 };
                if GetCursorPos(&mut pt) == 0 {
                    continue;
                }
                let inside = pt.x >= rc.left
                    && pt.x < rc.right
                    && pt.y >= rc.top
                    && pt.y < rc.bottom;
                // [lc-1974] 全屏输入锁：穿透位已锁定为「恒可点」，探针**停写穿透位**
                //（热区直翻 / 还原两路都停，锁是唯一权威）。
                // [lc-1981] 全屏诊断快照：每拍算一份自足状态快照 —— 光标物理坐标、
                // WindowFromPoint 命中（hwnd + 类名 + 标题 + 归类「光标下是谁」）、
                // 覆盖层与锚窗的实测 z 序关系（GetWindow 链走，不推断）、两层矩形、
                // 覆盖层穿透位、全屏 / 输入锁标志。状态变化即打、无变化 2s 心跳。
                //（取代 [lc-1974] 的命中变化打点：命中已并入快照键，变化必打。）
                if FULLSCREEN_INPUT_LOCK.load(Ordering::SeqCst) {
                    let hit = WindowFromPoint(pt);
                    let mut buf = [0u16; 64];
                    let n = GetClassNameW(hit, buf.as_mut_ptr(), 64);
                    let class = String::from_utf16_lossy(&buf[..n.max(0).min(64) as usize]);
                    let mut tbuf = [0u16; 128];
                    let tn = GetWindowTextW(hit, tbuf.as_mut_ptr(), 128);
                    let title = String::from_utf16_lossy(&tbuf[..tn.max(0).min(128) as usize]);
                    let anchor = get_hwnd().unwrap_or(0);
                    let mut m = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    let mut o = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    if anchor != 0 {
                        unsafe { GetWindowRect(anchor as _, &mut m) };
                    }
                    unsafe { GetWindowRect(hwnd as _, &mut o) };
                    let transp = overlay_input_passthrough_now(hwnd);
                    let fs = PLAYER_FULLSCREEN.load(Ordering::SeqCst);
                    let lock = FULLSCREEN_INPUT_LOCK.load(Ordering::SeqCst);
                    let rel = zorder_relation(hwnd, anchor);
                    let rel_tag = match rel {
                        ZorderRelation::OverlayAbove(d) => format!("overlay-above(+{d})"),
                        ZorderRelation::AnchorAbove(d) => format!("ANCHOR-ABOVE(+{d})"),
                        ZorderRelation::Unknown => "unknown".to_string(),
                    };
                    let body = format!(
                        "cursor=({x},{y}) hit=0x{hit_x:X} class=\"{class}\" title=\"{title}\" hit_is={kind} z={rel_tag} ov_rect=({},{},{},{}) mpv_rect=({},{},{},{}) transp={} fs={} lock={}",
                        o.left,
                        o.top,
                        o.right,
                        o.bottom,
                        m.left,
                        m.top,
                        m.right,
                        m.bottom,
                        i32::from(transp),
                        i32::from(fs),
                        i32::from(lock),
                        x = pt.x,
                        y = pt.y,
                        hit_x = hit as usize,
                        kind = classify_hit(hit as isize, hwnd, anchor),
                    );
                    let changed = last_snap_body.as_deref() != Some(body.as_str());
                    let elapsed = last_snap_log.elapsed().as_millis();
                    if snapshot_should_log(changed, elapsed, 2000) {
                        last_snap_body = Some(body.clone());
                        last_snap_log = std::time::Instant::now();
                        crate::libmpv::trace(&format!(
                            "[lc-1981] fs-snap reason={} {}",
                            if changed { "change" } else { "heartbeat" },
                            body
                        ));
                    }
                } else if inside {
                    // [lc-1964] 控件带热区直翻（架构保险 D）：每拍都判，不依赖前端
                    // elementFromPoint。命中 → 直接翻可点位；离开且是 Rust 自己翻的 →
                    // 还原穿透（前端主动请求的可交互态不被抢，见 FRONTEND_HOVER_LAST）。
                    let (zone_hit, zone_src) = hotzones_evaluate(
                        (rc.right - rc.left, rc.bottom - rc.top),
                        pt.x - rc.left,
                        pt.y - rc.top,
                    );
                    if zone_hit && overlay_input_passthrough_now(hwnd) {
                        overlay_apply_input_passthrough(hwnd, false, "probe-zone");
                        AUTO_HOVER_ACTIVE.store(true, Ordering::SeqCst);
                        crate::libmpv::trace(&format!(
                            "[lc-1964] 探针热区直翻可点 x={} y={} src={zone_src}（不依赖 elementFromPoint）",
                            pt.x - rc.left,
                            pt.y - rc.top
                        ));
                    } else if !zone_hit
                        && AUTO_HOVER_ACTIVE.swap(false, Ordering::SeqCst)
                        && !FRONTEND_HOVER_LAST.load(Ordering::SeqCst)
                    {
                        overlay_apply_input_passthrough(hwnd, true, "probe-restore");
                        crate::libmpv::trace("[lc-1964] 探针热区离开 → 穿透还原");
                    }
                } else if AUTO_HOVER_ACTIVE.swap(false, Ordering::SeqCst)
                    && !FRONTEND_HOVER_LAST.load(Ordering::SeqCst)
                {
                    overlay_apply_input_passthrough(hwnd, true, "probe-restore");
                    crate::libmpv::trace("[lc-1964] 光标离开覆盖层 → 热区穿透还原");
                }
                let moved = last_point != Some((pt.x, pt.y));
                let inside_changed = inside != last_inside;
                if moved {
                    last_point = Some((pt.x, pt.y));
                }
                if inside_changed {
                    last_inside = inside;
                    probe_set_inside(inside);
                }
                if !moved && !inside_changed {
                    continue;
                }
                let surface_w = rc.right - rc.left;
                let surface_h = rc.bottom - rc.top;
                if moved && inside && last_heartbeat.elapsed() >= std::time::Duration::from_secs(1) {
                    last_heartbeat = std::time::Instant::now();
                    let _ = app.emit("overlay-mouse-activity", ());
                }
                if moved || inside_changed {
                    // [lc-1960] physicalX/Y = **窗口内**物理坐标（Polaris 同语义：前端按
                    // surface 尺寸与 CSS clientWidth 比例映射到 elementFromPoint 坐标）。
                    let _ = app.emit(
                        "overlay-force-hover-check",
                        serde_json::json!({
                            "physicalX": pt.x - rc.left,
                            "physicalY": pt.y - rc.top,
                            "surfacePhysicalWidth": surface_w,
                            "surfacePhysicalHeight": surface_h,
                            "inside": inside,
                        }),
                    );
                }
            }
        }
        OVERLAY_PROBE_RUNNING.store(false, Ordering::SeqCst);
        crate::libmpv::trace("[lc-1960] 光标探针退出（内嵌会话结束）");
    });
}

/// [lc-1960] 置顶覆盖层。ex-style 基座 = NOACTIVATE + TOOLWINDOW + LAYERED
/// （`overlay_style_noactivate` 一次成型）；穿透位由
/// [`overlay_apply_input_passthrough`] 单点管理，本函数不碰它。
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
///
/// [lc-1981] 每次 z 序操作都留 before→after + 原因。**注意带顶语义**：对已 TOPMOST
/// 的窗再调 `HWND_TOPMOST` 会把它重插到 TOPMOST 带顶 —— 若覆盖层也在该带就会瞬间
/// 被盖住（lc-1949 拉锯的推手之一），调用方必须先 [`is_window_topmost`] 缺位才拨，
/// 或明确接受带顶重排（全屏进入瞬间等一次性时机）。
fn set_window_topmost(hwnd: isize, on: bool, reason: &str) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return;
    }
    unsafe {
        let before = is_window_topmost(hwnd);
        SetWindowPos(
            hwnd as _,
            if on { HWND_TOPMOST } else { HWND_NOTOPMOST },
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let after = is_window_topmost(hwnd);
        crate::libmpv::trace(&format!(
            "[lc-1981] topmost hwnd=0x{hwnd:X} {}→{} reason={reason}",
            i32::from(before),
            i32::from(after)
        ));
    }
}

/// 窗口当前是否带 TOPMOST（读 ex-style 真值，不缓存）。
fn is_window_topmost(hwnd: isize) -> bool {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return false;
    }
    unsafe { GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE) & (WS_EX_TOPMOST as isize) != 0 }
}

/// [lc-1952] 纯逻辑：覆盖层是否已紧贴在锚窗正上方。
///
/// `prev_of_anchor` = `GetWindow(锚窗, GW_HWNDPREV)`（z 序里压在锚窗上面的那一个）。
pub fn overlay_zorder_ok(prev_of_anchor: isize, overlay: isize) -> bool {
    overlay != 0 && prev_of_anchor == overlay
}

/// [lc-1964] 纯逻辑：把覆盖层插到「锚窗正上方」该用的 `hWndInsertAfter` 目标。
///
/// Win32 语义（实证定案，见模块头 [lc-1964]）：`SetWindowPos(被定位窗,
/// hWndInsertAfter)` 把被定位窗插到 hWndInsertAfter **正下方**。所以旧写法
/// `InsertAfter=锚窗` 实际把覆盖层压到锚窗之下 —— 全屏按钮全失效的真根因。
/// 正确做法：插到「锚窗当前上面那一个」的正下方 = 紧贴锚窗上方；锚窗已在带顶
/// （prev == 0）时目标 = HWND_TOP（带顶哨兵，返回 0）。
///
/// 单测双向锚定：旧逻辑（目标 = 锚窗本身）对本函数的返回值必然不同。
pub fn overlay_insert_target(prev_of_anchor: isize) -> isize {
    prev_of_anchor // prev == 0 时恰好就是 HWND_TOP(0)，两个分支自然合一
}

/// [lc-1964] 把覆盖层插到锚窗正上方（不激活、不动几何、不动 TOPMOST 位）。
///
/// 实现 = `InsertAfter(GetWindow(锚窗, GW_HWNDPREV))`：插到「锚窗上面那一个」的
/// 下方，正好落在锚窗上方；锚窗已在带顶时 `GW_HWNDPREV` 返回 0 = HWND_TOP 哨兵，
/// 覆盖层被送到带顶（仍紧贴锚窗上方）。
/// [lc-1981] 每次 z 序操作都留 before→after + 原因。
/// 返回是否执行了 SetWindowPos（窗口无效 / hwnd 为 0 时 false）。
fn place_overlay_above_anchor(overlay: isize, anchor: isize, reason: &str) -> bool {
    if overlay == 0
        || anchor == 0
        || unsafe { IsWindow(overlay as _) } == 0
        || unsafe { IsWindow(anchor as _) } == 0
    {
        return false;
    }
    let prev_of_anchor = unsafe { GetWindow(anchor as _, GW_HWNDPREV) } as isize;
    let target = overlay_insert_target(prev_of_anchor);
    unsafe {
        let ok = SetWindowPos(
            overlay as _,
            target as _,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        ) != 0;
        let now_prev = unsafe { GetWindow(anchor as _, GW_HWNDPREV) } as isize;
        crate::libmpv::trace(&format!(
            "[lc-1981] 带内插顶 overlay=0x{overlay:X} insert_after=0x{target:X} → 复核锚窗上方=0x{now_prev:X} ok={ok} reason={reason}"
        ));
        ok
    }
}

/// [lc-1964] 把覆盖层抬到 TOPMOST 带最顶（Polaris `apply_screen_saver_level` 同款）。
///
/// 已 TOPMOST 的窗再调 `HWND_TOPMOST` 不保证带内重排；先 `HWND_NOTOPMOST` 摘带再
/// `HWND_TOPMOST` 回带，系统强制把它插到带顶 —— 全屏期覆盖层因此恒在 mpv 锚窗
/// （带内任何位置）之上，真实鼠标命中永远先到覆盖层。mpv 的 VO / ontop_mode.lua
/// 再怎么 SetWindowPos 自己，最坏 33ms 后被本函数抬回来。
/// [lc-1981] 本函数**只作用于覆盖层**，锚窗永远不被抬起；每次调用留 before→after。
/// 返回是否两次 SetWindowPos 都成功。
fn overlay_raise_screen_saver_level(hwnd: isize, reason: &str) -> bool {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return false;
    }
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    unsafe {
        let before = is_window_topmost(hwnd);
        let down = SetWindowPos(hwnd as _, HWND_NOTOPMOST, 0, 0, 0, 0, flags) != 0;
        let up = SetWindowPos(hwnd as _, HWND_TOPMOST, 0, 0, 0, 0, flags) != 0;
        let after = is_window_topmost(hwnd);
        crate::libmpv::trace(&format!(
            "[lc-1981] 屏保级抬顶 hwnd=0x{hwnd:X} topmost {}→{}（NOTOPMOST→TOPMOST 重插带顶） ok={} reason={reason}",
            i32::from(before),
            i32::from(after),
            i32::from(down && up)
        ));
        down && up
    }
}

// ─────────────── [lc-1981] 全屏 z 序断言（相对锚窗）+ 诊断快照纯逻辑 ───────────────

/// [lc-1981] 全屏期「覆盖层必须压在锚窗之上」的一拍裁决（纯逻辑，执行与单测共用）。
///
/// 不变量：**覆盖层恒在锚窗上方**。修复动作只作用于覆盖层（屏保级抬顶 / 带内插顶），
/// 锚窗在任何分支下都**不被触碰** —— 旧「每拍强推锚窗 TOPMOST」逻辑正是把锚窗重插到
/// TOPMOST 带顶、盖住覆盖层的元凶（见模块头 [lc-1981]）。
///
/// 判定矩阵（`prev_of_anchor` = `GetWindow(锚窗, GW_HWNDPREV)`，锚窗正上方那一个）：
///   · 锚窗 TOPMOST + 覆盖层非 TOPMOST → 覆盖层整体在锚窗之下 → 抬顶（回带 + 带顶）；
///   · 锚窗非 TOPMOST + 覆盖层 TOPMOST → TOPMOST 带整体在锚窗之上 → **Ok，不动**
///    （lua 暂停置底 / mpv 掉带都落这里；旧逻辑在这里逐拍强推 = 30Hz 拉锯）；
///   · 两窗同带（都 TOPMOST 或都非 TOPMOST）→ 覆盖层必须紧贴锚窗正上方，不贴位：
///     同 TOPMOST 带走屏保级抬顶（mpv 重回带顶后的权威修正，下一拍收敛为 Ok），
///     同普通带走带内 InsertAfter。
///
/// 单测双向锚定：仿真器里跑「旧每拍强推锚窗」对本不变量必 FAIL，本函数 PASS。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenZorderAction {
    /// 覆盖层已在锚窗之上（含跨带：锚窗掉出 TOPMOST 带时，TOPMOST 带整体都在其上）。
    Ok,
    /// 覆盖层失守 → 屏保级抬顶（NOTOPMOST→TOPMOST，只动覆盖层）。
    RaiseOverlay,
    /// 两窗同在普通带且覆盖层不贴位 → 带内 InsertAfter 拉回锚窗正上方。
    InsertOverlayAboveAnchor,
}

pub fn fullscreen_zorder_action(
    anchor_topmost: bool,
    overlay_topmost: bool,
    prev_of_anchor: isize,
    overlay: isize,
) -> FullscreenZorderAction {
    if !overlay_topmost {
        if anchor_topmost {
            // 锚窗在带、覆盖层不在带 = 覆盖层必在锚窗之下 → 抬顶（回带 + 带顶）
            return FullscreenZorderAction::RaiseOverlay;
        }
        // 两窗同在普通带：贴位校验，不贴则带内拉回
        return if overlay_zorder_ok(prev_of_anchor, overlay) {
            FullscreenZorderAction::Ok
        } else {
            FullscreenZorderAction::InsertOverlayAboveAnchor
        };
    }
    if !anchor_topmost {
        // 锚窗掉出 TOPMOST 带（ontop_mode.lua 暂停置底 / mpv VO）：整个 TOPMOST 带
        // 都在锚窗之上，覆盖层天然满足不变量 —— **不动锚窗也不动覆盖层**。
        return FullscreenZorderAction::Ok;
    }
    if overlay_zorder_ok(prev_of_anchor, overlay) {
        FullscreenZorderAction::Ok
    } else {
        // mpv 重回 TOPMOST 带顶（SetWindowPos(HWND_TOPMOST)）把覆盖层压下 → 抬回带顶
        FullscreenZorderAction::RaiseOverlay
    }
}

/// [lc-1981] 覆盖层与锚窗的实测 z 序关系（GetWindow 链走结果，不推断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZorderRelation {
    /// 覆盖层在锚窗上方，u32 = 中间隔的窗口数（0 = 紧贴）。
    OverlayAbove(u32),
    /// 锚窗在覆盖层上方，u32 = 中间隔的窗口数（0 = 紧贴）—— 不变量被破坏。
    AnchorAbove(u32),
    /// 两窗不在同一条链上（必有一方失效/销毁），不变量无法证明 = 视为失守。
    Unknown,
}

impl ZorderRelation {
    /// 不变量「覆盖层在锚窗之上」是否成立。Unknown 不能证明成立 = 不成立（从严）。
    pub fn overlay_is_above(&self) -> bool {
        matches!(self, ZorderRelation::OverlayAbove(_))
    }
}

/// [lc-1981] 纯逻辑：由「从覆盖层向下走 d1 步见到锚窗 / 从锚窗向下走 d2 步见到覆盖层」
/// 的两个实测深度给出关系判定。执行侧负责真实链走（[`zorder_relation`]）。
pub fn zorder_relation_from_walks(
    below_overlay: Option<u32>,
    below_anchor: Option<u32>,
) -> ZorderRelation {
    match (below_overlay, below_anchor) {
        (Some(d), _) => ZorderRelation::OverlayAbove(d),
        (None, Some(d)) => ZorderRelation::AnchorAbove(d),
        (None, None) => ZorderRelation::Unknown,
    }
}

/// [lc-1981] 实测覆盖层与锚窗的 z 序关系：沿 `GW_HWNDNEXT` 向下走链（≤64 步封顶）。
/// 链走是权威事实，快照里写的是「走出来的」结果，不是任何缓存的推断。
fn zorder_relation(overlay: isize, anchor: isize) -> ZorderRelation {
    if overlay == 0 || anchor == 0 || overlay == anchor {
        return ZorderRelation::Unknown;
    }
    let below_overlay = walk_down_to(overlay, anchor);
    if below_overlay.is_some() {
        return zorder_relation_from_walks(below_overlay, None);
    }
    let below_anchor = walk_down_to(anchor, overlay);
    zorder_relation_from_walks(None, below_anchor)
}

/// 从 `from` 沿 GW_HWNDNEXT 向下走，返回首次遇到 `target` 的步数（0 = 紧邻）。
fn walk_down_to(from: isize, target: isize) -> Option<u32> {
    if from == 0 || target == 0 {
        return None;
    }
    unsafe {
        let mut h = GetWindow(from as _, GW_HWNDNEXT) as isize;
        let mut d: u32 = 0;
        while h != 0 && d < 64 {
            if h == target {
                return Some(d);
            }
            h = GetWindow(h as _, GW_HWNDNEXT) as isize;
            d += 1;
        }
    }
    None
}

/// [lc-1981] 纯逻辑：WindowFromPoint 命中窗口归类 —— 快照直接写明光标下是谁。
pub fn classify_hit(hit: isize, overlay: isize, anchor: isize) -> &'static str {
    if hit == 0 {
        "none"
    } else if hit == overlay {
        "overlay"
    } else if hit == anchor {
        "mpv-anchor"
    } else {
        "other"
    }
}

/// [lc-1981] 纯逻辑：快照该不该打 —— 状态变化即打；无变化 `heartbeat_ms` 心跳。
pub fn snapshot_should_log(key_changed: bool, since_last_log_ms: u128, heartbeat_ms: u128) -> bool {
    key_changed || since_last_log_ms >= heartbeat_ms
}

/// [lc-1946] 置顶：mpv 锚窗 + 覆盖层成对拨 TOPMOST/NOTOPMOST。
///
/// 只拨 mpv 的 `ontop` 属性没用：覆盖层自己一直是 TOPMOST，用户看不到任何变化；
/// 两个窗口一起拨才是「整个播放器置顶 / 取消置顶」。`overlay_hwnd` 由前端所在窗口
/// 自己的 hwnd 传入（命令里取），mpv 锚窗从槽里读。
///
/// [lc-1981] lc-1947 的「全屏期恒真强制」（effective_topmost）退役：用户在全屏期
/// 点「取消置顶」照实生效，不再被覆写回 TOPMOST。全屏期锚窗的日常置顶由 mpv 自己
/// 维护（进全屏一次性 `fntv-ontop-fs on` 抑制 ontop_mode.lua 的暂停置底）；退出
/// 全屏按 [`PLAYER_ONTOP`] 恢复意图（fullscreen_restore_steps 原样保留）。
pub fn set_player_ontop(overlay_hwnd: isize, on: bool) {
    PLAYER_ONTOP.store(on, Ordering::SeqCst);
    set_window_topmost(get_hwnd().unwrap_or(0), on, "pin-button");
    set_window_topmost(overlay_hwnd, on, "pin-button");
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
/// [lc-1974] 输入锁同样不能跨会话：残留会把窗口模式的穿透默认态锁死。
pub fn reset_player_fullscreen() {
    PLAYER_FULLSCREEN.store(false, Ordering::SeqCst);
    FULLSCREEN_INPUT_LOCK.store(false, Ordering::SeqCst);
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
/// 任务栏那一条也要盖住（全屏期锚窗 + 覆盖层 TOPMOST；[lc-1981] 起锚窗置顶由
/// mpv 一次性接管、跟随循环只保覆盖层在锚窗之上）。
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

/// 全屏几何断言（幂等）：**只修覆盖层**，锚窗几何归 mpv 自己（lc-1960 对齐 Polaris
/// 不变量：全屏期间 mpv 的 VO 是锚窗几何的唯一写者，我们从外面抢写就是 lc-1947/1948
/// 「-11,-11 3851x2171 拉锯」的根源）。覆盖层没铺满显示器就重摆；
/// [lc-1981] 锚窗 TOPMOST 一概不碰（置顶归 mpv + 一次性 `fntv-ontop-fs` 抑制）；
/// [lc-1964] 覆盖层抬到 TOPMOST 带最顶（屏保级，恒压锚窗之上）。
fn ensure_fullscreen_geometry(app: &tauri::AppHandle) {
    if !PLAYER_FULLSCREEN.load(Ordering::SeqCst) {
        return;
    }
    let mpv = get_hwnd().unwrap_or(0);
    let overlay = overlay_hwnd_of(app);
    let Some((mx, my, mw, mh)) = monitor_rect(if mpv != 0 { mpv } else { overlay }) else {
        return;
    };
    move_window(overlay, mx, my, mw, mh);
    if overlay != 0 && !is_window_topmost(overlay as _) {
        set_window_topmost(overlay, true, "fullscreen-reassert");
    }
    // [lc-1964] 屏保级抬顶：已 TOPMOST 的覆盖层再调 HWND_TOPMOST 不保证带内重排，
    // NOTOPMOST→TOPMOST 重置法强制回带顶（旧 InsertAfter=锚窗 方向反了，见模块头）。
    // mpv 的 VO 若在这 120ms 里把自己抬到带顶，这里同样把它压回覆盖层之下。
    overlay_raise_screen_saver_level(overlay, "fullscreen-reassert");
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
    // [lc-1974] 输入锁最先落位：全屏期穿透位锁定为「恒可点」，此后任何路径
    // （hover 事务 / 探针 / 会话接线）都不得把 TRANSPARENT 置回 —— 竞口收敛在
    // overlay_apply_input_passthrough 的裁决里，这里的显式拨位只负责「第一拍就位」。
    FULLSCREEN_INPUT_LOCK.store(on, Ordering::SeqCst);
    if on {
        // [lc-1974] 进全屏第一拍就摘穿透：控件从全屏第 0ms 起可点，不等 hover 链。
        let (applied, ok) = overlay_apply_input_passthrough(overlay, false, "fullscreen-enter");
        crate::libmpv::trace(&format!(
            "[lc-1974] 进入全屏：输入锁 ON + 穿透位强制摘除（applied={} ok={}）→ 覆盖层整窗可点，视频区点击由前端 mpv_forward_mouse 转发",
            i32::from(applied),
            i32::from(ok)
        ));
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
        // [lc-1960] 不再替 mpv 摆全屏矩形：`fullscreen` 属性下发后 mpv 的 VO 自己把
        // 窗口放到显示器矩形（lc-1947 实测 0:0:3840:2160），我们从外面抢写就是随后
        // 与它拉锯（-11,-11 3851x2171）的根源。锚窗几何 = mpv 单一写者（Polaris 同款）。
        move_window(overlay, mx, my, mw, mh);
        // [lc-1981] 锚窗置顶只做**缺位补拨**（一次性）：对已 TOPMOST 的锚窗再调
        // HWND_TOPMOST 会把它重插到带顶、盖到覆盖层之上 —— 旧逐拍强推正是 30Hz 拉锯
        // 与「覆盖层被埋」的元凶。此后锚窗置顶交给 mpv 自己（下方 fntv-ontop-fs 抑制）。
        if mpv != 0 && !is_window_topmost(mpv as _) {
            set_window_topmost(mpv, true, "fullscreen-enter");
        }
        set_window_topmost(overlay, true, "fullscreen-enter");
        // [lc-1964] 屏保级抬顶：覆盖层强制到 TOPMOST 带最顶。mpv 的 VO 随后处理全屏
        // 时会 SetWindowPos 自己（可能把它抬到覆盖层之上）—— 那一步由 33ms 跟随循环
        // 的相对锚窗断言兜住（最坏 33ms）。旧 `InsertAfter=锚窗` 方向反了（会把覆盖层
        // 压到锚窗之下，全屏按钮全失效的真根因），见模块头 [lc-1964]。
        overlay_raise_screen_saver_level(overlay, "fullscreen-enter");
        // [lc-1981] 全屏期 ontop 抑制：`ontop_mode.lua` 收到 `fntv-ontop-fs on` 后
        // 锁定 `ontop=true` 并停用「暂停置底」联动 —— lua 不再摘锚窗 TOPMOST，
        // 拉锯从源头消失（脚本本地改，不入库）。
        send_ontop_suppression(app, true);
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
        // [lc-1981] 解除 ontop_mode.lua 的全屏抑制，恢复三态置顶联动（暂停置底照常）。
        send_ontop_suppression(app, false);
        // [lc-1974] 退出全屏：先解锁并恢复窗口模式的穿透默认态（Polaris 语义）。
        // 尊重进行中的 hover（光标正悬在控件上时不把穿透拨回来，交 hover 链管理）；
        // 若全屏期间前端把 hover 请求短路了（FRONTEND_HOVER_LAST 停在 true），这里
        // 只晚一个探针事件 —— 前端在下一条 mousemove/probe 边沿会把账对齐。
        FULLSCREEN_INPUT_LOCK.store(false, Ordering::SeqCst);
        if !FRONTEND_HOVER_LAST.load(Ordering::SeqCst) {
            let (applied, ok) = overlay_apply_input_passthrough(overlay, true, "fullscreen-exit");
            crate::libmpv::trace(&format!(
                "[lc-1974] 退出全屏：输入锁 OFF + 穿透默认态恢复（applied={} ok={}）",
                i32::from(applied),
                i32::from(ok)
            ));
        } else {
            crate::libmpv::trace(
                "[lc-1974] 退出全屏：输入锁 OFF（光标悬在控件上，穿透位交 hover 链管理）",
            );
        }
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
                    set_window_topmost(mpv, on, "fullscreen-exit");
                    set_window_topmost(overlay, on, "fullscreen-exit");
                    // [lc-1952] 退出后覆盖层同样要压回锚窗正上方（带内显式定位）
                    place_overlay_above_anchor(overlay, mpv, "fullscreen-exit");
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
            set_window_topmost(mpv, ontop, "fullscreen-exit-reassert");
            set_window_topmost(overlay, ontop, "fullscreen-exit-reassert");
            // [lc-1952] mpv 迟到的还原可能把它自己抬到覆盖层之上：带内显式拉回
            place_overlay_above_anchor(overlay, mpv, "fullscreen-exit-reassert");
        });
    }
}

/// [lc-1981] 全屏期 `ontop_mode.lua` 暂停置底抑制开关（script-message 广播）。
///
/// `on`：脚本锁定 `ontop=true` 并停用 pause/idle 联动（锚窗不再被 lua 摘 TOPMOST，
/// 30Hz 拉锯从源头消失）；`off`：恢复三态联动。发送不等待回包 —— 窗口级动作
/// （几何 / z 序）不依赖它完成，200ms 拿不到 mpv 会话锁就放弃。
fn send_ontop_suppression(app: &tauri::AppHandle, on: bool) {
    use tauri::Manager;
    let app = app.clone();
    let arg = if on { "on" } else { "off" };
    tauri::async_runtime::spawn(async move {
        let state = app.state::<crate::AppState>();
        let session = tokio::time::timeout(std::time::Duration::from_millis(200), state.mpv.lock())
            .await
            .ok()
            .and_then(|g| g.clone());
        if let Some(m) = session {
            m.command_no_reply(serde_json::json!(["script-message", "fntv-ontop-fs", arg]))
                .await;
            crate::libmpv::trace(&format!("[lc-1981] ontop_mode 抑制 {arg} 已下发"));
        } else {
            crate::libmpv::trace("[lc-1981] ontop_mode 抑制下发失败（mpv 会话不可用）");
        }
    });
}

/// [lc-1947] 退出播放前若还在全屏：把锚窗还原到进入前的矩形/最大化状态，
/// 之后才能安全地 `SW_HIDE`（否则下一次起播会继承一块全屏几何）。
pub fn restore_player_window_from_fullscreen() {
    if !PLAYER_FULLSCREEN.swap(false, Ordering::SeqCst) {
        return;
    }
    // [lc-1974] 全屏态被退出流程直接终结时输入锁也必须解锁（穿透默认态归还窗口模式）。
    FULLSCREEN_INPUT_LOCK.store(false, Ordering::SeqCst);
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

/// [lc-1952] 纯逻辑：最小化主窗的 `ShowWindow` 步骤（执行与单测共用同一份决策）。
///
/// ① 先 `SW_MINIMIZE`：对**隐藏**窗只进最小化态、保持隐藏（lc-1947 实测：此时不产生
///    任务栏条目），DWM 不会为它物化任何恢复态帧；
/// ② 再 `SW_SHOWMINNOACTIVE`：显示的就是 iconic 形态本身（任务栏条目出现），全程
///    不存在「先按恢复矩形渲染一帧黑色播放页、再缩下去」的中间帧。
///
/// 旧顺序（先 `SW_SHOWMINNOACTIVE` 再 `SW_MINIMIZE`）对隐藏窗会先按恢复态物化一帧；
/// 叠加 lib.rs 侧「先收播放层再动主窗」的旧顺序（主窗若在此前恢复流程中被真实显示过，
/// 收掉两层的一瞬黑色播放页整个露出）—— 就是用户看到的「最小化后漏出黑色背景残留」。
/// 本函数 + lib.rs 的新顺序（主窗先最小化、此刻播放层还盖着）双保险。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinimizeStep {
    /// 进最小化态（隐藏窗保持隐藏，不产生视觉帧）
    Minimize,
    /// 显示为最小化（iconic 形态，产生任务栏条目）
    ShowMinNoActive,
}

pub fn minimize_show_steps(visible: bool, iconic: bool) -> Vec<MinimizeStep> {
    let mut steps = Vec::new();
    if !iconic {
        steps.push(MinimizeStep::Minimize);
    }
    if !visible {
        steps.push(MinimizeStep::ShowMinNoActive);
    }
    steps
}

/// [lc-1947/1952] 最小化到任务栏：**先进最小化态，再显示为最小化**。
/// 返回 `(IsWindowVisible, IsIconic)` 供调用方写进日志做证据。
pub fn minimize_main_to_taskbar(hwnd: isize) -> (bool, bool) {
    if hwnd == 0 || unsafe { IsWindow(hwnd as _) } == 0 {
        return (false, false);
    }
    let visible = unsafe { IsWindowVisible(hwnd as _) } != 0;
    let iconic = unsafe { IsIconic(hwnd as _) } != 0;
    for step in minimize_show_steps(visible, iconic) {
        unsafe {
            match step {
                MinimizeStep::Minimize => {
                    ShowWindow(hwnd as _, SW_MINIMIZE);
                }
                MinimizeStep::ShowMinNoActive => {
                    ShowWindow(hwnd as _, SW_SHOWMINNOACTIVE);
                }
            }
        }
    }
    (
        unsafe { IsWindowVisible(hwnd as _) } != 0,
        unsafe { IsIconic(hwnd as _) } != 0,
    )
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
    set_window_topmost(mpv_hwnd, PLAYER_ONTOP.load(Ordering::SeqCst), "adopt");
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
/// [lc-1974] 会话边界（无论进/出）都清全屏输入锁：锁只允许由
/// `on_player_fullscreen_changed` 在会话内建立，绝不跨会话残留。
pub fn embed_set_active(on: bool) {
    EMBED_ACTIVE.store(on, Ordering::SeqCst);
    FULLSCREEN_INPUT_LOCK.store(false, Ordering::SeqCst);
    if !on {
        *EMBED_RECT.lock().unwrap_or_else(|e| e.into_inner()) = None;
        EMBED_HIDDEN.store(false, Ordering::SeqCst);
        // [lc-1946] 会话结束：最小化待恢复 / 最大化标记都不该带到下一次播放
        PLAYER_MIN_RESTORE.store(false, Ordering::SeqCst);
        PLAYER_MAXIMIZED.store(false, Ordering::SeqCst);
        // [lc-1947] 全屏标记 / 还原矩形同样不能跨会话
        PLAYER_FULLSCREEN.store(false, Ordering::SeqCst);
        // [lc-1974] 输入锁与全屏标记同生命周期
        FULLSCREEN_INPUT_LOCK.store(false, Ordering::SeqCst);
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
    /// 全屏且两层之一偏离显示器矩形：**只**把覆盖层复位到显示器矩形；
    /// 锚窗偏离仅记日志（mpv 的 VO 是锚窗几何唯一写者，lc-1960）。
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
        // [lc-1952] 全屏 z 序自愈的状态沿记录：只在「好 → 坏」翻转时记一条日志，
        // 避免每拍刷屏。
        let mut overlay_zorder_was_ok = true;
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
                // [lc-1981] 全屏分支拉锯根治：本分支**只动覆盖层**，锚窗一概不碰。
                // 旧逻辑（lc-1949/1974）：每拍发现锚窗丢 TOPMOST 就 HWND_TOPMOST 重插
                // 复位 —— 重插把锚窗放到 TOPMOST 带顶、盖到覆盖层之上；mpv 的
                // ontop_mode.lua（暂停置底）反复摘带 → 30Hz 拉锯期间覆盖层反复被埋，
                // 点击全漏给 mpv（用户实测 `[lc-1949] 全屏中锚窗丢失 TOPMOST → 复位`
                // 每 33ms 一条刷屏 + 控件全失效）。新不变量：锚窗置顶由进全屏的一次性
                // `fntv-ontop-fs on`（lua 锁 ontop=true）+ mpv 自己维护；这里只断言
                // 「覆盖层恒在锚窗之上」，不满位才修（见 fullscreen_zorder_action）。
                // [lc-1964] 窗口模式 z 序断言照旧：违例走修正后的带内 InsertAfter。
                // lc-1952 的 InsertAfter=锚窗 方向反了（实证：SetWindowPos(B,
                // insertAfter=A) 把 B 插到 A **正下方**），自愈变成每 33ms 把覆盖层
                // 压回 mpv 之下 —— 全屏按钮全失效的真根因。
                let fullscreen_now = PLAYER_FULLSCREEN.load(Ordering::SeqCst);
                if fullscreen_now {
                    // [lc-1974] 穿透位自愈保留：输入锁下恒为摘除，任何漏网路径在此兜底。
                    if overlay_input_passthrough_now(ov as _) {
                        let (applied, ok) =
                            overlay_apply_input_passthrough(ov, false, "fullscreen-follow");
                        crate::libmpv::trace(&format!(
                            "[lc-1974] 全屏穿透位残留 → 自愈摘除（applied={} ok={}）",
                            i32::from(applied),
                            i32::from(ok)
                        ));
                    }
                    let anchor_top = is_window_topmost(mh as _);
                    let overlay_top = is_window_topmost(ov as _);
                    let prev_of_anchor = unsafe { GetWindow(mh as _, GW_HWNDPREV) } as isize;
                    let action =
                        fullscreen_zorder_action(anchor_top, overlay_top, prev_of_anchor, ov);
                    match action {
                        FullscreenZorderAction::Ok => {}
                        FullscreenZorderAction::RaiseOverlay => {
                            if overlay_zorder_was_ok {
                                crate::libmpv::trace(
                                    "[lc-1964] 全屏覆盖层失守（不在锚窗上方）→ 屏保级抬顶（NOTOPMOST→TOPMOST，只动覆盖层）",
                                );
                            }
                            overlay_raise_screen_saver_level(ov, "fullscreen-follow");
                        }
                        FullscreenZorderAction::InsertOverlayAboveAnchor => {
                            if overlay_zorder_was_ok {
                                crate::libmpv::trace(
                                    "[lc-1964] 全屏覆盖层失守（同带不贴位）→ InsertAfter 拉回锚窗正上方",
                                );
                            }
                            place_overlay_above_anchor(ov, mh, "fullscreen-follow");
                        }
                    }
                    overlay_zorder_was_ok = action == FullscreenZorderAction::Ok;
                } else {
                    // 两个窗不同带（ontop 拨动的过渡瞬间）时带内 InsertAfter 无意义，跳过一拍。
                    let band_consistent =
                        is_window_topmost(mh as _) == is_window_topmost(ov as _);
                    let prev_of_anchor = unsafe { GetWindow(mh as _, GW_HWNDPREV) } as isize;
                    let zorder_ok = band_consistent && overlay_zorder_ok(prev_of_anchor, ov);
                    if !zorder_ok && band_consistent {
                        if overlay_zorder_was_ok {
                            crate::libmpv::trace(
                                "[lc-1964] 窗口覆盖层失守 → InsertAfter(GW_HWNDPREV) 拉回锚窗正上方",
                            );
                        }
                        place_overlay_above_anchor(ov, mh, "window-follow");
                    }
                    overlay_zorder_was_ok = zorder_ok;
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
                        // [lc-1960] 锚窗几何归 mpv 单一写者，跟随循环只修覆盖层：
                        // 全屏过渡期 mpv 的 VO 会按自己的节奏摆窗口，我们从外面抢写
                        // 就是与它拉锯（实测 -11,-11 3851x2171 反复来回）的根源。
                        // 锚窗偏离显示器矩形仅留一条证据日志，不动手。
                        if m.left != fx
                            || m.top != fy
                            || m.right - m.left != fw
                            || m.bottom - m.top != fh
                        {
                            crate::libmpv::trace(&format!(
                                "[lc-1960] 全屏跟随：锚窗偏离显示器矩形（mpv 自摆中，不动手）m={}x{}@{},{}",
                                m.right - m.left, m.bottom - m.top, m.left, m.top
                            ));
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

    // ─────────────── [lc-1964] z 序 InsertAfter 方向 + 控件带热区 ───────────────

    /// [lc-1964] InsertAfter 目标：锚窗已在带顶（GW_HWNDPREV = 0）时目标是 HWND_TOP(0)。
    /// **双向验证**：lc-1952 旧逻辑的目标恒为锚窗本身 —— 对旧代码本断言必 FAIL
    /// （旧目标 = 锚窗 hwnd ≠ 0，且旧方向实际把覆盖层插到锚窗**之下**）。
    #[test]
    fn insert_target_band_top_is_hwnd_top_not_anchor() {
        let anchor: isize = 0x777;
        assert_eq!(
            overlay_insert_target(0),
            0,
            "锚窗在带顶 → 覆盖层插 HWND_TOP（带顶哨兵），绝不许插到锚窗之后（= 之下）"
        );
        assert_ne!(overlay_insert_target(0), anchor, "旧逻辑（InsertAfter=锚窗）方向相反，正是全屏按钮全失效的根因");
    }

    /// [lc-1964] InsertAfter 目标：锚窗上面还有别的窗时，目标 = 那个窗
    /// （插到它正下方 = 紧贴锚窗正上方）。
    #[test]
    fn insert_target_is_prev_of_anchor() {
        assert_eq!(overlay_insert_target(0xABC), 0xABC);
        assert_eq!(overlay_insert_target(-1), -1, "负值句柄原样透传（不可能是哨兵语义）");
    }

    /// [lc-1964] 热区换算：CSS 矩形 × (viewport/surface) 比例命中物理光标点。
    #[test]
    fn hotzone_hit_scales_physical_to_css() {
        // 视口 1280x720 CSS，窗口 3840x2160 物理（3x）；热区 = 底栏 (0, 648, 1280, 72) CSS。
        let zones = [[0.0, 648.0, 1280.0, 72.0]];
        // 物理点 (1920, 2100) → CSS (640, 700) → 命中底栏。
        assert!(hotzone_hit(&zones, (1280.0, 720.0), (3840, 2160), 1920, 2100));
        // 物理点 (1920, 1080) → CSS (640, 360) → 画面中央，不命中。
        assert!(!hotzone_hit(&zones, (1280.0, 720.0), (3840, 2160), 1920, 1080));
        // 空热区（chrome 隐藏时前端上报空表）恒不命中。
        assert!(!hotzone_hit(&[], (1280.0, 720.0), (3840, 2160), 1920, 2100));
        // 视口非法（除零保护）恒不命中。
        assert!(!hotzone_hit(&zones, (0.0, 0.0), (3840, 2160), 1920, 2100));
    }

    /// [lc-1964] 固定兜底带：顶带 0..14%、底带 74%..100% 命中；画面中央不命中。
    #[test]
    fn fallback_band_top_bottom_hit_middle_miss() {
        let surface = (2520, 1575);
        assert!(fallback_band_hit(surface, 1260, 100), "顶带内（右上窗控区域）");
        assert!(fallback_band_hit(surface, 1260, 1500), "底带内（dock 区域）");
        assert!(!fallback_band_hit(surface, 1260, 800), "画面中央绝不兜底");
        assert!(!fallback_band_hit(surface, -5, 100), "越界 x 不命中");
        assert!(!fallback_band_hit(surface, 1260, 1600), "越界 y 不命中");
    }

    /// [lc-1964] hotzones_evaluate 状态机（共享静态，必须在**单条**测试里按序驱动）：
    /// ① 从未上报 → 固定兜底带；② fresh 上报 → 上报矩形接管；③ 空表（chrome 隐藏）
    /// → reported 且恒不命中；④ 视口非法 → 回固定兜底带。
    #[test]
    fn hotzones_evaluate_state_machine() {
        // ① 测试进程初始态 HOTZONES_AT = None → 固定兜底带接管（WebView 全挂保险丝）。
        let (hit, src) = hotzones_evaluate((2520, 1575), 1260, 100);
        assert!(hit, "从未上报 → 固定兜底带必须接管（顶带命中）");
        assert_eq!(src, "fallback");
        let (mid, _) = hotzones_evaluate((2520, 1575), 1260, 800);
        assert!(!mid, "固定兜底带不许覆盖画面中央");
        // ② fresh 上报矩形接管命中判定。
        overlay_set_hotzones(vec![[0.0, 648.0, 1280.0, 72.0]], (1280.0, 720.0));
        let (hit, src) = hotzones_evaluate((3840, 2160), 1920, 2100);
        assert!(hit && src == "reported", "fresh 上报矩形接管命中判定");
        let (miss, _) = hotzones_evaluate((3840, 2160), 1920, 1080);
        assert!(!miss, "上报矩形外的物理点不命中");
        // ③ chrome 隐藏：前端上报空表（视口仍有效）→ 全画面不命中（点击照旧穿透给 mpv）。
        overlay_set_hotzones(vec![], (1280.0, 720.0));
        let (empty, src2) = hotzones_evaluate((3840, 2160), 1920, 2100);
        assert!(!empty && src2 == "reported", "空表 = chrome 隐藏，热区保险丝让位");
        // ④ 视口 (0,0) = 视为从未上报 → 回固定兜底带。
        overlay_set_hotzones(vec![], (0.0, 0.0));
        let (fb, src3) = hotzones_evaluate((2520, 1575), 1260, 100);
        assert!(fb && src3 == "fallback", "视口非法 → 固定兜底带");
    }

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

    /// [lc-1981] 全屏期置顶意图照实生效（lc-1947 的 effective_topmost 恒真强制退役）：
    /// 锚窗置顶的日常维护归 mpv（进全屏一次性 fntv-ontop-fs 抑制），用户「取消置顶」
    /// 不再被覆写；退出全屏按 PLAYER_ONTOP 恢复（fullscreen_restore_steps 单测覆盖）。
    #[test]
    fn fullscreen_no_longer_overrides_pin_intent() {
        // 旧逻辑：effective_topmost(false, true) == true（意图被全屏覆写）。
        // 新逻辑：该强制已删除，set_player_ontop 直通意图（状态位断言）。
        reset_player_ontop();
        set_player_ontop(0, false); // hwnd=0 只改状态不碰窗口
        assert!(!player_ontop(), "全屏期取消置顶意图必须原样记录（不再被恒真强制覆写）");
        reset_player_ontop();
    }

    // ─────────────── [lc-1981] 全屏 z 序断言 + 30Hz 拉锯双向仿真 ───────────────

    /// [lc-1981] 纯逻辑：全屏 z 序裁决矩阵（覆盖层必须压在锚窗之上；锚窗零触碰）。
    #[test]
    fn fullscreen_zorder_decision_matrix() {
        let overlay = 200074isize;
        // 稳态：两窗都 TOPMOST，覆盖层紧贴锚窗上方 → 不动
        assert_eq!(
            fullscreen_zorder_action(true, true, overlay, overlay),
            FullscreenZorderAction::Ok
        );
        // lua 暂停置底：锚窗掉出 TOPMOST 带（覆盖层还在带里）→ **Ok，绝不动锚窗**
        //（旧逻辑在这里逐拍强推锚窗 TOPMOST = 30Hz 拉锯的另一半）
        assert_eq!(
            fullscreen_zorder_action(false, true, 0, overlay),
            FullscreenZorderAction::Ok
        );
        // mpv VO 把锚窗抬回 TOPMOST 带顶（prev=0，覆盖层被压到下面）→ 抬覆盖层
        assert_eq!(
            fullscreen_zorder_action(true, true, 0, overlay),
            FullscreenZorderAction::RaiseOverlay
        );
        // 锚窗在带里、覆盖层掉带（用户在全屏点过取消置顶后又出岔子）→ 抬覆盖层回带顶
        assert_eq!(
            fullscreen_zorder_action(true, false, 0, overlay),
            FullscreenZorderAction::RaiseOverlay
        );
        // 两窗同在普通带且覆盖层紧贴锚窗上方（全屏期用户取消置顶后的稳态）→ 不动
        assert_eq!(
            fullscreen_zorder_action(false, false, overlay, overlay),
            FullscreenZorderAction::Ok
        );
        // 两窗同在普通带且不贴位 → 带内 InsertAfter 拉回
        assert_eq!(
            fullscreen_zorder_action(false, false, 0xABC, overlay),
            FullscreenZorderAction::InsertOverlayAboveAnchor
        );
    }

    /// [lc-1981] 最小 z 序仿真器：单条全局链 + 每窗 TOPMOST 旗（Win32 拓扑）。
    /// index 0 = 最高 z。HWND_TOPMOST → 旗 true + 插到带顶（= 链顶）；
    /// HWND_NOTOPMOST → 旗 false + 插到普通带顶（= 最上面那个非 TOPMOST 窗之上）；
    /// SetWindowPos(h, after) → 插到 after 正下方（旗不变）。
    struct ZorderSim {
        z: Vec<isize>,
        top: std::collections::HashMap<isize, bool>,
    }
    impl ZorderSim {
        fn new() -> Self {
            Self { z: Vec::new(), top: std::collections::HashMap::new() }
        }
        fn pos(&self, h: isize) -> usize {
            self.z.iter().position(|&x| x == h).expect("窗口必须在链上")
        }
        fn is_topmost(&self, h: isize) -> bool {
            *self.top.get(&h).unwrap_or(&false)
        }
        fn remove(&mut self, h: isize) -> bool {
            let was_top = self.is_topmost(h);
            if let Some(p) = self.z.iter().position(|&x| x == h) {
                self.z.remove(p);
            }
            self.top.remove(&h);
            was_top
        }
        fn insert_at(&mut self, h: isize, idx: usize, topmost: bool) {
            let idx = idx.min(self.z.len());
            self.z.insert(idx, h);
            self.top.insert(h, topmost);
        }
        /// HWND_TOPMOST：带顶 = 链顶。
        fn win32_topmost(&mut self, h: isize) {
            let was = self.remove(h);
            let _ = was;
            self.insert_at(h, 0, true);
        }
        /// HWND_NOTOPMOST：普通带顶 = 最上面的非 TOPMOST 窗之前。
        fn win32_notopmost(&mut self, h: isize) {
            let was = self.remove(h);
            let _ = was;
            let idx = self.z.iter().position(|&x| !self.is_topmost(x)).unwrap_or(self.z.len());
            self.insert_at(h, idx, false);
        }
        /// HWND_TOP：保持旗，插到本带顶（TOPMOST 带顶 = 链顶；普通带顶 = 带顶位置）。
        fn win32_top(&mut self, h: isize) {
            let was = self.remove(h);
            if was {
                self.insert_at(h, 0, true);
            } else {
                let idx = self
                    .z
                    .iter()
                    .position(|&x| !self.is_topmost(x))
                    .unwrap_or(self.z.len());
                self.insert_at(h, idx, false);
            }
        }
        /// SetWindowPos(h, after)：插到 after 正下方（旗不变）；after=0 → HWND_TOP。
        fn win32_insert_after(&mut self, h: isize, after: isize) {
            if after == 0 {
                self.win32_top(h);
                return;
            }
            let was = self.remove(h);
            let p = self.pos(after);
            self.insert_at(h, p + 1, was);
        }
        /// GetWindow(h, GW_HWNDPREV)：链上正上方那个（0 = 已在链顶）。
        fn prev_of(&self, h: isize) -> isize {
            let p = self.pos(h);
            if p == 0 {
                0
            } else {
                self.z[p - 1]
            }
        }
        /// a 是否在 b 之上（全局链序，等价「光标命中优先到 a」）。
        fn above(&self, a: isize, b: isize) -> bool {
            a != b && self.pos(a) < self.pos(b)
        }
        /// [lc-1981] 新逻辑一拍：相对锚窗断言，只动覆盖层。
        fn new_follow_tick(&mut self, anchor: isize, overlay: isize) -> FullscreenZorderAction {
            let action = fullscreen_zorder_action(
                self.is_topmost(anchor),
                self.is_topmost(overlay),
                self.prev_of(anchor),
                overlay,
            );
            match action {
                FullscreenZorderAction::Ok => {}
                FullscreenZorderAction::RaiseOverlay => {
                    // overlay_raise_screen_saver_level：NOTOPMOST→TOPMOST（带顶重插）
                    self.win32_notopmost(overlay);
                    self.win32_topmost(overlay);
                }
                FullscreenZorderAction::InsertOverlayAboveAnchor => {
                    let target = self.prev_of(anchor);
                    self.win32_insert_after(overlay, target);
                }
            }
            action
        }
        /// lua 攻击步：ontop_mode.lua 暂停置底（锚窗摘 TOPMOST）。
        fn lua_strip_anchor(&mut self, anchor: isize) {
            self.win32_notopmost(anchor);
        }
        /// mpv VO 反击步：anchor 以 HWND_TOPMOST 回带顶。
        fn mpv_retop(&mut self, anchor: isize) {
            self.win32_topmost(anchor);
        }
    }

    /// [lc-1981] **双向验证（本次修复的核心证据）**：同样的持续攻击（lua 每拍摘锚窗
    /// TOPMOST、mpv 每隔几拍把锚窗抬回带顶）下——
    ///   · 旧逻辑（lc-1949 时代：每拍「锚窗丢失 TOPMOST → 复位」强推 + 只补覆盖层位，
    ///     无 z 序复检）→ 锚窗被我们亲手抬到覆盖层之上且稳定反转 ⇒ 「覆盖层恒在锚窗
    ///     上方」断言 **必 FAIL**（即用户实测的全屏控件全失效）；
    ///   · 新逻辑（fullscreen_zorder_action）→ 每拍终态恒满足断言；修复动作
    ///     （RaiseOverlay / Insert）按构造只对覆盖层做 SetWindowPos，锚窗零触碰，
    ///     lua 摘带那一拍直接 Ok 不动手 —— 拉锯土壤被铲掉。
    #[test]
    fn old_anchor_forcing_fails_overlay_above_anchor_new_passes() {
        let anchor = 0xAA01isize;
        let overlay = 0xBB02isize;
        let build = |sim: &mut ZorderSim| {
            sim.win32_topmost(anchor);
            sim.win32_topmost(overlay); // 覆盖层在带顶（起播自然带序）
            assert!(sim.above(overlay, anchor), "初始态：覆盖层压住锚窗");
        };
        // 攻击剧本：lua 每拍摘锚窗 TOPMOST；每 3 拍 mpv 以 HWND_TOPMOST 抬回带顶
        let mut old_sim = ZorderSim::new();
        build(&mut old_sim);
        let mut new_sim = ZorderSim::new();
        build(&mut new_sim);
        for tick in 0..100u32 {
            old_sim.lua_strip_anchor(anchor);
            new_sim.lua_strip_anchor(anchor);
            if tick % 3 == 2 {
                old_sim.mpv_retop(anchor);
                new_sim.mpv_retop(anchor);
            }
            // —— 旧逻辑一拍：每拍强推锚窗 TOPMOST（=lc-1949 的「丢失 → 复位」）+
            //    只在覆盖层丢位时补位（lc-1949 全屏分支没有 z 序复检）
            if !old_sim.is_topmost(anchor) {
                old_sim.win32_topmost(anchor); // HWND_TOPMOST 重插 = 带顶 = 盖住覆盖层
            }
            if !old_sim.is_topmost(overlay) {
                old_sim.win32_topmost(overlay);
            }
            // —— 新逻辑一拍：相对锚窗断言，只动覆盖层
            new_sim.new_follow_tick(anchor, overlay);
            // 终态不变量：覆盖层必须压住锚窗
            assert!(
                new_sim.above(overlay, anchor),
                "第 {tick} 拍：新逻辑必须满足「覆盖层恒在锚窗上方」"
            );
        }
        // 旧逻辑在同剧本下必然反转（这就是用户全屏失效的机制定案）
        assert!(
            !old_sim.above(overlay, anchor),
            "双向验证：旧「每拍强推锚窗 TOPMOST」逻辑对不变量必 FAIL（锚窗被抬到覆盖层之上）"
        );
    }

    /// [lc-1981] 新逻辑收敛性：mpv 抬回带顶那一拍判 RaiseOverlay，下一拍即 Ok
    ///（不逐拍重施、不触锚窗——与旧逻辑的 30Hz 拉锯量化对比）。
    #[test]
    fn new_zorder_action_converges_after_single_fix() {
        let anchor = 0x1101isize;
        let overlay = 0x2202isize;
        let mut sim = ZorderSim::new();
        sim.win32_topmost(anchor);
        sim.win32_topmost(overlay);
        // mpv VO 抬锚窗回带顶（覆盖层被压下）
        sim.mpv_retop(anchor);
        let a1 = sim.new_follow_tick(anchor, overlay);
        assert_eq!(a1, FullscreenZorderAction::RaiseOverlay, "失守那一拍必须修");
        assert!(sim.above(overlay, anchor));
        let a2 = sim.new_follow_tick(anchor, overlay);
        assert_eq!(a2, FullscreenZorderAction::Ok, "下一拍必须收敛为 Ok（不再有逐拍重施）");
        // lua 暂停置底（锚窗掉带）：一拍 Ok，零修复动作
        sim.lua_strip_anchor(anchor);
        let a3 = sim.new_follow_tick(anchor, overlay);
        assert_eq!(a3, FullscreenZorderAction::Ok, "锚窗掉带 = TOPMOST 带整体在其上，绝不搭理");
        assert!(sim.above(overlay, anchor));
    }

    /// [lc-1981] 纯逻辑：z 序链走判定（Some=步数 / None=未见）。
    #[test]
    fn zorder_relation_from_walks_matrix() {
        let overlay = 0x311isize;
        let anchor = 0x422isize;
        assert_eq!(
            zorder_relation_from_walks(Some(0), None),
            ZorderRelation::OverlayAbove(0),
            "紧贴 = 0 隔"
        );
        assert_eq!(
            zorder_relation_from_walks(Some(3), None),
            ZorderRelation::OverlayAbove(3)
        );
        assert_eq!(
            zorder_relation_from_walks(None, Some(2)),
            ZorderRelation::AnchorAbove(2),
            "锚窗在上 = 不变量被破坏"
        );
        assert_eq!(
            zorder_relation_from_walks(None, None),
            ZorderRelation::Unknown,
            "不同链 = 无法证明成立 = 视为失守"
        );
        // 不变量判定：OverlayAbove 才算成立
        assert!(ZorderRelation::OverlayAbove(0).overlay_is_above());
        assert!(!ZorderRelation::AnchorAbove(0).overlay_is_above());
        assert!(!ZorderRelation::Unknown.overlay_is_above(), "从严：不能证明 = 不成立");
        let _ = (overlay, anchor);
    }

    /// [lc-1981] 纯逻辑：光标命中归类（快照直接写明光标下是谁）。
    #[test]
    fn classify_hit_matrix() {
        let overlay = 0x311isize;
        let anchor = 0x422isize;
        assert_eq!(classify_hit(overlay, overlay, anchor), "overlay");
        assert_eq!(classify_hit(anchor, overlay, anchor), "mpv-anchor");
        assert_eq!(classify_hit(0x999, overlay, anchor), "other");
        assert_eq!(classify_hit(0, overlay, anchor), "none");
    }

    /// [lc-1981] 纯逻辑：快照频率控制 —— 状态变化即打；无变化 2s 心跳；1s 内不打心跳。
    #[test]
    fn snapshot_rate_limit() {
        assert!(snapshot_should_log(true, 0, 2000), "变化立即打");
        assert!(snapshot_should_log(false, 2500, 2000), "无变化超心跳窗口要打");
        assert!(!snapshot_should_log(false, 1500, 2000), "无变化未到心跳不打（防刷屏）");
        assert!(!snapshot_should_log(false, 1999, 2000));
        assert!(snapshot_should_log(false, 2000, 2000));
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

    // ─────────────── [lc-1952] 全屏 z 序自愈 + 最小化黑底修复 ───────────────

    /// [lc-1952] z 序判定：覆盖层必须紧贴锚窗正上方（GW_HWNDPREV 探测）。
    #[test]
    fn overlay_zorder_decision() {
        let overlay = 200074isize;
        // 健康态：锚窗上面那个就是覆盖层
        assert!(overlay_zorder_ok(overlay, overlay), "覆盖层在锚窗正上方");
        // 反转态：锚窗之上是别的窗口（mpv 的 VO 全屏 SetWindowPos 抬自己）
        assert!(!overlay_zorder_ok(789784, overlay), "被 mpv 压住必须判定为坏");
        // 锚窗就在 TOPMOST 带顶（上面没有任何窗口）
        assert!(!overlay_zorder_ok(0, overlay), "上面没有窗口也必须判定为坏");
        // 覆盖层 hwnd 无效（0）
        assert!(!overlay_zorder_ok(0, 0));
        assert!(!overlay_zorder_ok(overlay, 0), "覆盖层无效时恒为坏");
    }

    /// [lc-1952] 双向验证：z 序自愈闭环 —— 坏态判定 → InsertAfter 修正 → 好态判定。
    /// 旧代码没有任何 z 序探测（owner 关系 + TOPMOST 位），同样输入下永远「自以为好」。
    #[test]
    fn overlay_zorder_self_heal_loop() {
        let overlay = 200074isize;
        let mpv = 789784isize;
        // 旧行为（无探测）：z 序已反转，旧代码无从得知 —— 模拟「无修复」的判定恒真
        let old_assumes_ok = |_prev: isize| true;
        assert!(old_assumes_ok(mpv), "旧代码不探测 z 序（这就是 Bug 1 的根因面）");

        // 新行为：探测 → 修正 → 复核
        let mut prev_of_anchor = mpv; // mpv 的 VO 把锚窗抬到带顶，覆盖层被压住
        assert!(!overlay_zorder_ok(prev_of_anchor, overlay), "修复前必须判定为坏");
        // place_overlay_above_anchor 执行后：覆盖层紧贴锚窗上方 → GW_HWNDPREV 变为 overlay
        prev_of_anchor = overlay;
        assert!(overlay_zorder_ok(prev_of_anchor, overlay), "修复后必须判定为好");
    }

    /// [lc-1952] 最小化步骤决策：先 SW_MINIMIZE（隐藏窗保持隐藏、不物化恢复态帧），
    /// 再 SW_SHOWMINNOACTIVE（iconic 形态补任务栏条目）。
    #[test]
    fn minimize_steps_order() {
        // 播放期主窗被 m.hide() 隐藏：先进最小化态，再补显示 —— 顺序不能反
        assert_eq!(
            minimize_show_steps(false, false),
            vec![MinimizeStep::Minimize, MinimizeStep::ShowMinNoActive]
        );
        // 已可见（此前从任务栏恢复过、主窗停在可见态）：普通最小化即可
        assert_eq!(minimize_show_steps(true, false), vec![MinimizeStep::Minimize]);
        // 隐藏但已是最小化态（上一步只执行了一半）
        assert_eq!(minimize_show_steps(false, true), vec![MinimizeStep::ShowMinNoActive]);
        // 可见且已最小化：无事可做（连点最小化不重复发 ShowWindow）
        assert!(minimize_show_steps(true, true).is_empty());
    }

    /// [lc-1952] 双向验证：旧顺序（先 SW_SHOWMINNOACTIVE 再 SW_MINIMIZE）对隐藏窗
    /// 会先按恢复态物化一帧（黑底来源）；新决策的第一个动作必须是 SW_MINIMIZE。
    #[test]
    fn minimize_old_order_had_show_first() {
        // 旧代码的决策（固化病理性顺序作为反例）
        let old_steps = |visible: bool, iconic: bool| {
            let mut v: Vec<&str> = Vec::new();
            if !visible {
                v.push("show_min_no_active"); // 先显示 → 恢复态帧被物化
            }
            if !iconic {
                v.push("minimize");
            }
            v
        };
        assert_eq!(
            old_steps(false, false),
            vec!["show_min_no_active", "minimize"],
            "旧顺序：隐藏窗先被显示（黑底帧来源）"
        );
        // 新决策：隐藏窗的第一个动作是 Minimize（不产生视觉帧）
        let new_steps = minimize_show_steps(false, false);
        assert_eq!(new_steps[0], MinimizeStep::Minimize, "新顺序：先最小化，全程无恢复态帧");
        assert_ne!(
            new_steps,
            vec![MinimizeStep::ShowMinNoActive, MinimizeStep::Minimize],
            "新决策不得退回旧顺序"
        );
    }

    // ─────────────── [lc-1960] hover 事务协议（Polaris generation/seq 同款）───────────────

    /// [lc-1960] seq 单调门：新请求放行并推进水位；迟到（seq 落后）请求必须被挡住。
    #[test]
    fn hover_seq_gate_monotonic() {
        assert_eq!(hover_seq_gate(0, 5), (true, 5), "首请求放行");
        assert_eq!(hover_seq_gate(5, 6), (true, 6), "新请求放行");
        assert_eq!(hover_seq_gate(6, 4), (false, 6), "迟到请求不放行，水位不动");
        assert_eq!(hover_seq_gate(6, 6), (true, 6), "重试同 seq 放行（ack 丢失重发）");
        assert_eq!(hover_seq_gate(6, 0), (true, 6), "未带 seq（0）恒放行且不推水位");
    }

    /// [lc-1960] 双向验证：lc-1949「常驻可点」架构里 hover 请求根本不存在（穿透位
    /// 永不被拨动），迟到请求无从拦截 —— 同输入 (last=6, seq=4) 在旧架构等价于
    /// 「直接写样式」，新旧决策必须可区分。
    #[test]
    fn hover_gate_old_arch_has_no_stale_guard() {
        let old_arch_writes = |_last: u64, _seq: u64| true; // 旧架构无 seq 概念，来者皆写
        assert!(old_arch_writes(6, 4), "旧架构对迟到请求照写（会覆盖新决策）");
        assert_eq!(hover_seq_gate(6, 4).0, false, "新架构必须拦截迟到请求");
    }

    /// [lc-1960] 代际推进 + seq 清零：新会话（bump）后旧代际 seq 不得阻塞新请求。
    #[test]
    fn hover_generation_bump_resets_seq_watermark() {
        hover_bump_generation();
        let g0 = hover_generation();
        assert_eq!(hover_seq_gate(0, 42), (true, 42));
        // 新会话 bump：seq 水位清零 —— 旧请求 seq=42 重新放行（旧代际状态已整体作废）
        let g1 = hover_bump_generation();
        assert!(g1 > g0, "代际必须单调推进");
        assert_eq!(hover_seq_gate(0, 42), (true, 42), "bump 后 seq 水位已清零");
    }

    /// [lc-1960] 无窗口时 hover_apply 的 ack 语义：applied=false + ok=false
    /// （前端据此进 40/120/300/700ms 退避重试，而不是乐观改账）。
    /// 注意：generation/seq 是进程级单例，其它测试可能并发推进 —— 这里只断言
    /// 与「窗口/seq 事实」相关的字段，不断言代际的具体值。
    #[test]
    fn hover_apply_without_window_never_claims_applied() {
        hover_bump_generation();
        let ack = hover_apply(0, true, 7); // hwnd=0：IsWindow 必假
        assert!(!ack.applied_hover, "没有窗口就绝不能谎报已可交互");
        assert!(!ack.ok, "拨位失败 ok 必须为 false（驱动前端重试）");
    }

    // ─────────────── [lc-1974] 全屏输入锁（穿透机制全屏废除）───────────────

    /// [lc-1974] 锁裁决矩阵：摘穿透恒放行（进全屏自愈依赖它）；锁定期置穿透被拒
    /// 绝但视为成功（ok=true —— 否则 hover 状态机会为一条注定被锁的请求进
    /// 40/120/300/700ms 重试风暴）；非锁定期置穿透照旧放行（窗口模式不动）。
    #[test]
    fn fullscreen_input_lock_adjudication() {
        // 锁定期：置穿透被拒（不写），但按成功结算（锁即权威结果）
        assert_eq!(
            passthrough_request_adjudication(true, true),
            (false, true),
            "全屏锁定期置穿透必须被拒绝且不触发重试"
        );
        // 摘穿透恒放行（含锁定期）：进全屏第一拍与跟随循环自愈都靠它
        assert_eq!(passthrough_request_adjudication(false, true), (true, true));
        assert_eq!(passthrough_request_adjudication(false, false), (true, true));
        // 非锁定期：窗口模式 Polaris 语义原样（置穿透放行）
        assert_eq!(passthrough_request_adjudication(true, false), (true, true));
    }

    /// [lc-1974] 双向验证：旧路径（lc-1959/1960 穿透默认态，无锁概念）对「置穿透」
    /// 恒放行 —— 在全屏态它就是「点击漏进 mpv 变播放/暂停」的竞口。对同一输入
    /// （锁定 + 置穿透），旧裁决写、新裁决拒：断言必须可区分。
    #[test]
    fn fullscreen_lock_old_path_would_write() {
        let old_adjudication = |passthrough: bool| (passthrough, passthrough); // 旧路径：来者皆写
        let (old_write, _) = old_adjudication(true);
        assert!(old_write, "旧路径全屏期照写穿透位（Bug 的竞口本身）");
        let (new_write, new_ok) = passthrough_request_adjudication(true, true);
        assert!(!new_write, "新路径锁定期拒绝写");
        assert!(new_ok, "拒绝但按成功结算（无重试风暴）");
        assert_ne!(old_write, new_write, "新旧裁决必须可区分（回归哨兵）");
    }
}
