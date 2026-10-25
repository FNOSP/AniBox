//! [lc-1857] N 卡 Smooth Motion 一键注册 —— 免开 NVIDIA App，直接写驱动程序配置。
//!
//! 原理：NVIDIA App 的「程序设置」最终落在我们驱动的 DRS（Driver Settings API）库里，
//! 官方暴露 NVAPI 接口可编程读写。本模块用最小 FFI（nvapi64.dll 的 QueryInterface +
//! DRS 公开函数 ID，均出自 nvapi.h）完成：
//!   建/找 profile「AniBox」→ 挂 mpv.exe 应用 → 按**设置名**查 Smooth Motion 的
//!   settingId（驱动按名字注册，候选名列表逐个试）→ SetSetting(U32=1) → SaveSettings。
//!
//! 不碰全局 profile、不删任何既有配置；失败逐级返回诊断文本（调用方 toast 给用户，
//! 失败时回落手动引导文案）。

#![cfg(target_os = "windows")]

use std::ffi::c_void;

type NvU32 = u32;
type Ptr = *mut c_void;

// nvapi.h 公开函数 ID
const FID_INITIALIZE: u32 = 0x0150_E828;
const FID_UNLOAD: u32 = 0xD22B_DD7E; // NvAPI_Unload
const FID_DRS_CREATE_SESSION: u32 = 0x0694_D52E;
const FID_DRS_LOAD_SETTINGS: u32 = 0x375D_BD6B;
const FID_DRS_SAVE_SETTINGS: u32 = 0xFCBC_7E14;
const FID_DRS_DESTROY_SESSION: u32 = 0xDAD9_CFF8;
const FID_DRS_FIND_PROFILE_BY_NAME: u32 = 0x7E4A_9A0B;
const FID_DRS_CREATE_PROFILE: u32 = 0xCC17_6068;
const FID_DRS_CREATE_APPLICATION: u32 = 0x4347_A9DE;
const FID_DRS_SET_SETTING: u32 = 0x577D_D202;
const FID_DRS_GET_SETTING_ID_FROM_NAME: u32 = 0x0CB7_309D; // nvapi-sys nvid 表核对（旧值 0x8858BD36 是别的函数，调用即崩）

const UNICODE_MAX: usize = 2048;
const BINARY_BUF: usize = 4096;

/// NvAPI_UnicodeString = [u16; 2048]（NUL 结尾宽字符）
type UnicodeStr = [u16; UNICODE_MAX];

fn to_unicode(s: &str) -> Box<UnicodeStr> {
    let mut buf: Box<UnicodeStr> = Box::new([0u16; UNICODE_MAX]);
    for (i, ch) in s.encode_utf16().take(UNICODE_MAX - 1).enumerate() {
        buf[i] = ch;
    }
    buf
}

fn from_unicode(buf: &[u16; UNICODE_MAX]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf16_lossy(&buf[..end])
}

/// NVDRS_PROFILE（nvapi.h v1：version/profileName/gpuSupport/isPredefined/numOfApps/numOfSettings）
#[repr(C)]
struct NvDrsProfile {
    version: NvU32,
    profile_name: UnicodeStr,
    gpu_support: NvU32,
    is_predefined: NvU32,
    num_of_apps: NvU32,
    num_of_settings: NvU32,
}

/// NVDRS_APPLICATION（v1：version/isPredefined/appName/userFriendlyName/launcher）
#[repr(C)]
struct NvDrsApplication {
    version: NvU32,
    is_predefined: NvU32,
    app_name: UnicodeStr,
    user_friendly_name: UnicodeStr,
    launcher: UnicodeStr,
}

/// NVDRS_SETTING（v1）：version/settingId/location/isCurrentPredefined/isPredefinedValid/
/// settingName/settingType + 当前值 union（u32/unicode/binary）+ 预定义值 union。
#[repr(C)]
struct NvDrsSetting {
    version: NvU32,
    setting_id: NvU32,
    setting_location: NvU32,
    is_current_predefined: NvU32,
    is_predefined_valid: NvU32,
    setting_name: UnicodeStr,
    setting_type: NvU32, // 0=binary 1=dword 2=unicode(wstring)
    u32_current: NvU32,
    unicode_current: UnicodeStr,
    binary_current_size: NvU32,
    binary_current: [u8; BINARY_BUF],
    u32_predefined: NvU32,
    unicode_predefined: UnicodeStr,
    binary_predefined_size: NvU32,
    binary_predefined: [u8; BINARY_BUF],
}

const NVDRS_SETTING_TYPE_DWORD: NvU32 = 1;

fn make_ver(size: usize, ver: u32) -> NvU32 {
    ((size as NvU32) & 0xFFFF) | (ver << 16)
}

type FnQueryInterface = unsafe extern "system" fn(id: NvU32) -> Ptr;
type FnSimple = unsafe extern "system" fn() -> i32;
type FnDrsCreateSession = unsafe extern "system" fn(session: *mut Ptr) -> i32;
type FnDrsLoadSettings = unsafe extern "system" fn(session: Ptr) -> i32;
type FnDrsSaveSettings = unsafe extern "system" fn(session: Ptr) -> i32;
type FnDrsDestroySession = unsafe extern "system" fn(session: Ptr) -> i32;
type FnDrsFindProfileByName = unsafe extern "system" fn(session: Ptr, name: *const u16, profile: *mut NvDrsProfile) -> i32;
type FnDrsCreateProfile = unsafe extern "system" fn(session: Ptr, profile: *mut NvDrsProfile) -> i32;
type FnDrsCreateApplication = unsafe extern "system" fn(session: Ptr, profile: Ptr, app: *mut NvDrsApplication) -> i32;
type FnDrsSetSetting = unsafe extern "system" fn(session: Ptr, profile: Ptr, setting: *mut NvDrsSetting) -> i32;
type FnDrsGetSettingIdFromName = unsafe extern "system" fn(name: *const u16, id: *mut NvU32) -> i32;

/// 探测 Smooth Motion 的 settingId：候选名逐个问驱动。
/// 返回 (命中的名字, id)。驱动不认识任何候选名时返回 None（NVIDIA 版本差异/未发布）。
pub fn probe_setting_id() -> Option<(String, NvU32)> {
    let candidates = [
        "Smooth Motion",
        "Video Smooth Motion",
        "RTX Video Smooth Motion",
        "SmoothMotion",
        "RTX Smooth Motion",
        "Frame Generation",
    ];
    unsafe {
        eprintln!("[nvapi-probe] loading nvapi64.dll");
        let lib = libloading_nvapi()?;
        eprintln!("[nvapi-probe] library loaded, resolving QueryInterface");
        let q: FnQueryInterface = std::mem::transmute(lib.query_interface);
        eprintln!("[nvapi-probe] resolving Initialize");
        let initialize: FnSimple = std::mem::transmute(q(FID_INITIALIZE));
        eprintln!("[nvapi-probe] Initialize ptr={:p}", initialize as Ptr);
        let st = initialize();
        eprintln!("[nvapi-probe] Initialize -> 0x{st:08X}");
        if st != 0 {
            return None;
        }
        let get_id: FnDrsGetSettingIdFromName = std::mem::transmute(q(FID_DRS_GET_SETTING_ID_FROM_NAME));
        eprintln!("[nvapi-probe] GetSettingIdFromName ptr={:p}", get_id as Ptr);
        let mut found = None;
        for name in candidates {
            let wname = to_unicode(name);
            let mut id: NvU32 = 0;
            eprintln!("[nvapi-probe] querying name={name:?}");
            let status = get_id(wname.as_ptr(), &mut id);
            eprintln!("[nvapi-probe]   -> status=0x{status:08X} id=0x{id:08X}");
            if status == 0 && id != 0 {
                found = Some((name.to_string(), id));
                break;
            }
        }
        let unload: FnSimple = std::mem::transmute(q(FID_UNLOAD));
        let _ = unload();
        found
    }
}

/// 最小 nvapi64.dll 加载器（库句柄进程内持有）
struct NvapiLib {
    _lib: libloading::Library,
    query_interface: Ptr,
}

fn libloading_nvapi() -> Option<NvapiLib> {
    unsafe {
        let lib = libloading::Library::new("nvapi64.dll").ok()?;
        let q: FnQueryInterface = std::mem::transmute(lib.get::<FnQueryInterface>(b"nvapi_QueryInterface\0").ok()?);
        Some(NvapiLib { _lib: lib, query_interface: std::mem::transmute(q) })
    }
}

/// 一键注册：profile「AniBox」+ mpv.exe 应用 + Smooth Motion 开关。
/// 返回 Ok(诊断文本) / Err(诊断文本)。
pub fn ensure_smooth_motion(mpv_path: &str) -> Result<String, String> {
    unsafe {
        let Some(lib) = libloading_nvapi() else {
            return Err("未找到 nvapi64.dll（需要已安装 NVIDIA 驱动）".into());
        };
        let q: FnQueryInterface = std::mem::transmute(lib.query_interface);
        let initialize: FnSimple = std::mem::transmute(q(FID_INITIALIZE));
        if initialize() != 0 {
            return Err("NVAPI 初始化失败".into());
        }
        let Some((sm_name, sm_id)) = probe_setting_id() else {
            return Err("当前驱动未注册 Smooth Motion 设置项（需 RTX 50 系 + 较新驱动 + NVIDIA App 安装）".into());
        };

        let create_session: FnDrsCreateSession = std::mem::transmute(q(FID_DRS_CREATE_SESSION));
        let load_settings: FnDrsLoadSettings = std::mem::transmute(q(FID_DRS_LOAD_SETTINGS));
        let save_settings: FnDrsSaveSettings = std::mem::transmute(q(FID_DRS_SAVE_SETTINGS));
        let destroy_session: FnDrsDestroySession = std::mem::transmute(q(FID_DRS_DESTROY_SESSION));
        let find_profile: FnDrsFindProfileByName = std::mem::transmute(q(FID_DRS_FIND_PROFILE_BY_NAME));
        let create_profile: FnDrsCreateProfile = std::mem::transmute(q(FID_DRS_CREATE_PROFILE));
        let create_app: FnDrsCreateApplication = std::mem::transmute(q(FID_DRS_CREATE_APPLICATION));
        let set_setting: FnDrsSetSetting = std::mem::transmute(q(FID_DRS_SET_SETTING));

        let mut session: Ptr = std::ptr::null_mut();
        let st = create_session(&mut session);
        if st != 0 {
            return Err(format!("DRS CreateSession 失败: 0x{st:08X}"));
        }
        let result = (|| -> Result<String, String> {
            let st = load_settings(session);
            if st != 0 {
                return Err(format!("DRS LoadSettings 失败: 0x{st:08X}"));
            }

            // 找/建 profile「AniBox」
            let profile_name = to_unicode("AniBox");
            let mut profile = zero_profile();
            let mut found = find_profile(session, profile_name.as_ptr(), &mut profile) == 0;
            if !found {
                profile = zero_profile();
                profile.version = make_ver(std::mem::size_of::<NvDrsProfile>(), 1);
                profile.profile_name = *profile_name;
                let st = create_profile(session, &mut profile);
                if st != 0 {
                    return Err(format!("DRS CreateProfile 失败: 0x{st:08X}"));
                }
                // CreateProfile 后需重新 Find 拿到带句柄语义的 profile 结构
                let mut re = zero_profile();
                if find_profile(session, profile_name.as_ptr(), &mut re) == 0 {
                    profile = re;
                    found = true;
                }
            }
            if !found {
                return Err("DRS profile 查找/创建失败".into());
            }

            // 挂 mpv.exe：CreateApplication 幂等（同 profile+同名重复添加驱动拒绝且不新建，
            // 错误码不区分场景一律不阻断——SetSetting 按 profile 生效与 app 条目数无关）
            let exe_name = to_unicode(mpv_path);
            let mut new_app = zero_app();
            new_app.version = make_ver(std::mem::size_of::<NvDrsApplication>(), 1);
            new_app.app_name = *exe_name;
            new_app.user_friendly_name = *to_unicode("AniBox mpv");
            let st = create_app(session, &mut profile as *mut _ as Ptr, &mut new_app);
            if st != 0 {
                // 已存在等场景继续走 SetSetting
                println!("[nvidia_sm] CreateApplication 非零返回 0x{st:08X}（多为已存在，继续）");
            }

            // 写 Smooth Motion = 1
            let mut setting = zero_setting();
            setting.version = make_ver(std::mem::size_of::<NvDrsSetting>(), 1);
            setting.setting_id = sm_id;
            setting.setting_type = NVDRS_SETTING_TYPE_DWORD;
            setting.u32_current = 1;
            setting.setting_name = *to_unicode(&sm_name);
            let st = set_setting(session, &mut profile as *mut _ as Ptr, &mut setting);
            if st != 0 {
                return Err(format!("DRS SetSetting({sm_name}=1) 失败: 0x{st:08X}"));
            }
            let st = save_settings(session);
            if st != 0 {
                return Err(format!("DRS SaveSettings 失败: 0x{st:08X}"));
            }
            Ok(format!("已写入驱动程序配置：profile=AniBox · {mpv_path} · {sm_name}=1"))
        })();

        let _ = destroy_session(session);
        result
    }
}

fn zero_profile() -> NvDrsProfile {
    NvDrsProfile {
        version: make_ver(std::mem::size_of::<NvDrsProfile>(), 1),
        profile_name: [0u16; UNICODE_MAX],
        gpu_support: 0,
        is_predefined: 0,
        num_of_apps: 0,
        num_of_settings: 0,
    }
}

fn zero_app() -> NvDrsApplication {
    NvDrsApplication {
        version: make_ver(std::mem::size_of::<NvDrsApplication>(), 1),
        is_predefined: 0,
        app_name: [0u16; UNICODE_MAX],
        user_friendly_name: [0u16; UNICODE_MAX],
        launcher: [0u16; UNICODE_MAX],
    }
}

fn zero_setting() -> NvDrsSetting {
    NvDrsSetting {
        version: make_ver(std::mem::size_of::<NvDrsSetting>(), 1),
        setting_id: 0,
        setting_location: 0,
        is_current_predefined: 0,
        is_predefined_valid: 0,
        setting_name: [0u16; UNICODE_MAX],
        setting_type: NVDRS_SETTING_TYPE_DWORD,
        u32_current: 0,
        unicode_current: [0u16; UNICODE_MAX],
        binary_current_size: 0,
        binary_current: [0u8; BINARY_BUF],
        u32_predefined: 0,
        unicode_predefined: [0u16; UNICODE_MAX],
        binary_predefined_size: 0,
        binary_predefined: [0u8; BINARY_BUF],
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "windows")]
    fn probe_smooth_motion_id() {
        match super::probe_setting_id() {
            Some((name, id)) => println!("LC1857 HIT: settingId=0x{id:08X} name={name:?}"),
            None => println!("LC1857 MISS: 驱动不认识任何候选设置名"),
        }
    }
}
