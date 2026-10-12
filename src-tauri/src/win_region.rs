//! 窗口区域裁洞（WebView 挖洞）—— 参照 LumiPlayer 的 nativeVideoUiRegion 方案。
//!
//! 目标：把 WebView2 各层窗口的**窗口区域**挖掉「视频占位矩形」那一块，
//! 让位于其下方的 `FntvMpvHost`（mpv `--wid` 渲染目标）从洞里透出来。
//!
//! 为什么必须是「区域」而不是「透明」（lc-082 已实测）：WebView2 的渲染子窗
//! （Chrome_WidgetWin_0/1、Chrome_RenderWidgetHostHWND、Intermediate D3D Window）
//! 恒盖同父兄弟子窗，页面 alpha 透明也透不出下方窗口。而 `SetWindowRgn` 是
//! GDI/DWM 层的**真裁剪**：既裁绘制，也裁命中测试 —— 洞里的鼠标事件会直接透传给
//! 下方的 mpv 宿主窗，因此 mpv 原生底栏（uosc）仍可交互。这正是 lc-087 保住
//! 原生 uosc 与「HTML 做主层」得以共存的原因。
//!
//! 只在 **WebView 子窗** 上设区域：主窗一旦设区域，DWM 圆角/亚克力失效
//! （lc-029/lc-083 教训），且会把 mpv 宿主窗一起裁掉，反而看不见。
//!
//! 开关：`FNTV_CUTOUT=0` 关闭（退回 lc-087 的「视频置顶」形态）。

#![cfg(windows)]

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, MapWindowPoints, SetWindowRgn,
    RGN_DIFF,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetClientRect, GetParent, GetWindow, IsWindow, GW_CHILD, GW_HWNDNEXT,
};

/// WebView2 窗口链的类名（lc-082 用 EnumChildWindows 实测抓到的全集）
const WEBVIEW_CLASSES: [&str; 5] = [
    "WRY_WEBVIEW",
    "Chrome_WidgetWin_0",
    "Chrome_WidgetWin_1",
    "Chrome_RenderWidgetHostHWND",
    "Intermediate D3D Window",
];

/// 裁洞开关（默认开；`FNTV_CUTOUT=0` 关闭 → 退回视频置顶形态）
pub fn cutout_enabled() -> bool {
    !matches!(std::env::var("FNTV_CUTOUT").ok().as_deref(), Some("0"))
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// 该窗口是否落在任一「排除窗」（mpv 宿主 / playerbar）的子树里
fn under_excluded(hwnd: HWND, excluded: &[isize]) -> bool {
    if excluded.is_empty() {
        return false;
    }
    let mut cur = hwnd;
    let mut guard = 0;
    while !cur.is_null() && guard < 64 {
        if excluded.contains(&(cur as isize)) {
            return true;
        }
        cur = unsafe { GetParent(cur) };
        guard += 1;
    }
    false
}

/// 深度优先收集主窗下所有 WebView2 层窗口（排除 mpv 宿主/playerbar 子树）
fn collect_targets(main: HWND, excluded: &[isize]) -> Vec<(HWND, String)> {
    let mut out = Vec::new();
    let mut stack = Vec::new();
    unsafe {
        let mut c = GetWindow(main, GW_CHILD);
        while !c.is_null() {
            stack.push(c);
            c = GetWindow(c, GW_HWNDNEXT);
        }
    }
    while let Some(h) = stack.pop() {
        let cls = class_of(h);
        if WEBVIEW_CLASSES.contains(&cls.as_str()) && !under_excluded(h, excluded) {
            out.push((h, cls));
        }
        unsafe {
            let mut c = GetWindow(h, GW_CHILD);
            while !c.is_null() {
                stack.push(c);
                c = GetWindow(c, GW_HWNDNEXT);
            }
        }
    }
    out
}

/// 给单个目标窗设置「客户区 − 洞」区域；洞与客户区不重叠时清除区域（避免无谓裁剪）
unsafe fn set_one(target: HWND, main: HWND, hole: Option<(i32, i32, i32, i32)>, radius: i32) -> String {
    let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if unsafe { GetClientRect(target, &mut rc) } == 0 {
        return "GetClientRect 失败".into();
    }
    let (cw, ch) = (rc.right - rc.left, rc.bottom - rc.top);
    if cw <= 0 || ch <= 0 {
        return "客户区为空，跳过".into();
    }

    let Some((x, y, w, h)) = hole else {
        unsafe { SetWindowRgn(target, std::ptr::null_mut(), 1) };
        return format!("清除区域 {}x{}", cw, ch);
    };

    // 洞矩形由主窗客户区坐标换算到目标窗客户区坐标
    let mut pt = POINT { x, y };
    unsafe { MapWindowPoints(main, target, &mut pt, 1) };
    let (lx, ly) = (pt.x, pt.y);

    // 与目标客户区求交
    let (ix, iy) = (lx.max(0), ly.max(0));
    let (ir, ib) = ((lx + w).min(cw), (ly + h).min(ch));
    if ir <= ix || ib <= iy {
        unsafe { SetWindowRgn(target, std::ptr::null_mut(), 1) };
        return "洞不重叠，清除区域".into();
    }
    let full_cover = ix == 0 && iy == 0 && ir >= cw && ib >= ch;

    unsafe {
        let full = CreateRectRgn(0, 0, cw, ch);
        let hole_rgn = if radius > 0 {
            // CreateRoundRectRgn 的椭圆宽高是「整个圆角椭圆」的尺寸 → 半径 r 传 2r
            CreateRoundRectRgn(ix, iy, ir + 1, ib + 1, radius * 2, radius * 2)
        } else {
            CreateRectRgn(ix, iy, ir + 1, ib + 1)
        };
        if full.is_null() || hole_rgn.is_null() {
            if !full.is_null() {
                DeleteObject(full as _);
            }
            if !hole_rgn.is_null() {
                DeleteObject(hole_rgn as _);
            }
            return "CreateRgn 失败".into();
        }
        CombineRgn(full, full, hole_rgn, RGN_DIFF);
        DeleteObject(hole_rgn as _); // 中间区域用完即删；full 交给 SetWindowRgn 托管
        if SetWindowRgn(target, full, 1) == 0 {
            DeleteObject(full as _); // 失败则系统不接管，自己回收
            return "SetWindowRgn 失败".into();
        }
        if full_cover {
            return format!("洞覆盖全窗 {}x{}（WebView 完全让位）", cw, ch);
        }
        format!("裁洞 ok {}x{} 洞=({},{},{},{})", cw, ch, ix, iy, ir - ix, ib - iy)
    }
}

/// 应用裁洞：`hole` = 视频占位矩形（**主窗客户区**坐标，物理像素）；`None` = 清除全部裁洞。
/// 返回逐窗结果文本（诊断日志用）。
pub fn apply_cutout(
    main_hwnd: isize,
    excluded: &[isize],
    hole: Option<(i32, i32, i32, i32)>,
    radius: i32,
) -> Result<Vec<String>, String> {
    let main = main_hwnd as HWND;
    if main.is_null() || unsafe { IsWindow(main) } == 0 {
        return Err("主窗句柄无效".into());
    }
    let targets = collect_targets(main, excluded);
    if targets.is_empty() {
        return Err("未找到 WebView2 目标窗（类名枚举为空）".into());
    }
    let mut log = Vec::with_capacity(targets.len());
    for (h, cls) in targets {
        let r = unsafe { set_one(h, main, hole, radius) };
        log.push(format!("{}#{:#x} {}", cls, h as usize, r));
    }
    Ok(log)
}

/// 仅诊断：列出当前识别到的 WebView2 目标窗（不设区域）
pub fn list_targets(main_hwnd: isize, excluded: &[isize]) -> Vec<String> {
    let main = main_hwnd as HWND;
    if main.is_null() || unsafe { IsWindow(main) } == 0 {
        return vec!["主窗句柄无效".into()];
    }
    collect_targets(main, excluded)
        .into_iter()
        .map(|(h, c)| format!("{}#{:#x}", c, h as usize))
        .collect()
}
