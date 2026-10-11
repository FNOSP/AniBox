//! libmpv 动态 FFI —— 进程内嵌入 mpv（去进程化）。
//! 背景：应用内 spawn mpv.exe 存在未定位的平台级不兼容（lc-074~079 全实验矩阵），
//! 改用 libmpv-2.dll（fntv-mpv 分发自带）进程内加载：
//!   - 视频 --wid 嵌入子窗口（libmpv 在同进程建 D3D，无跨进程窗口问题）
//!   - 控制走 C API（mpv_command/observe_property），语义对齐原 JSON IPC 前端协议
//!   - 主仓库 Lua 资产（弹幕/smart_skip）后续经 --script 挂载
//! 手写 FFI + GetProcAddress 动态加载：无 .lib 依赖、零构建配置。

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::OnceLock;

pub const MPV_FORMAT_STRING: c_int = 1;
pub const MPV_FORMAT_FLAG: c_int = 3;
pub const MPV_FORMAT_DOUBLE: c_int = 5;
pub const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;
pub const MPV_EVENT_SHUTDOWN: c_int = 1;

#[repr(C)]
pub struct MpvEvent {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvEventProperty {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

pub struct Api {
    pub create: unsafe extern "system" fn() -> *mut c_void,
    pub initialize: unsafe extern "system" fn(*mut c_void) -> c_int,
    pub set_option_string: unsafe extern "system" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    pub command: unsafe extern "system" fn(*mut c_void, *mut *mut c_char) -> c_int,
    pub observe_property: unsafe extern "system" fn(*mut c_void, u64, *const c_char, c_int) -> c_int,
    pub wait_event: unsafe extern "system" fn(*mut c_void, f64) -> *mut MpvEvent,
    pub terminate_destroy: unsafe extern "system" fn(*mut c_void),
    pub free: unsafe extern "system" fn(*mut c_void),
    pub error_string: unsafe extern "system" fn(c_int) -> *const c_char,
}

static API: OnceLock<Option<Api>> = OnceLock::new();

/// 从 mpv_path 同目录加载 libmpv-2.dll（GetProcAddress 动态解析）
pub fn api(dll_dir: &str) -> Result<&'static Api, String> {
    API.get_or_init(|| {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW};
        // LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR：libmpv 的依赖（ffmpeg 等同目录 DLL）按其自身目录解析，
        // 否则从 exe 目录加载时依赖搜索路径不含 fntv-mpv → 依赖缺失加载失败
        const DLL_LOAD_DIR: u32 = 0x100;
        const DEFAULT_DIRS: u32 = 0x1000;
        let dll = format!("{}\\libmpv-2.dll\0", dll_dir.trim_end_matches('/').trim_end_matches('\\'));
        let wide: Vec<u16> = dll.encode_utf16().collect();
        let h = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), DLL_LOAD_DIR | DEFAULT_DIRS) };
        if h.is_null() {
            return None;
        }
        let get = |name: &str| -> Option<*mut c_void> {
            let n: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
            unsafe { GetProcAddress(h, n.as_ptr()).map(|f| f as *mut c_void) }
        };
        Some(Api {
            create: unsafe { std::mem::transmute(get("mpv_create")?) },
            initialize: unsafe { std::mem::transmute(get("mpv_initialize")?) },
            set_option_string: unsafe { std::mem::transmute(get("mpv_set_option_string")?) },
            command: unsafe { std::mem::transmute(get("mpv_command")?) },
            observe_property: unsafe { std::mem::transmute(get("mpv_observe_property")?) },
            wait_event: unsafe { std::mem::transmute(get("mpv_wait_event")?) },
            terminate_destroy: unsafe { std::mem::transmute(get("mpv_terminate_destroy")?) },
            free: unsafe { std::mem::transmute(get("mpv_free")?) },
            error_string: unsafe { std::mem::transmute(get("mpv_error_string")?) },
        })
    })
    .as_ref()
    .ok_or_else(|| "加载 libmpv-2.dll 失败（fntv-mpv 分发不完整？）".to_string())
}

/// 诊断日志（崩溃点定位用）
pub fn trace(msg: &str) {
    let _ = std::fs::OpenOptions::new().create(true).append(true)
        .open(std::env::temp_dir().join("fntv-libmpv.log"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{}
", msg).as_bytes()));
}

pub fn err_str(api: &Api, code: c_int) -> String {
    unsafe {
        let p = (api.error_string)(code);
        if p.is_null() {
            format!("mpv error {}", code)
        } else {
            CStr::from_ptr(p).to_string_lossy().to_string()
        }
    }
}
