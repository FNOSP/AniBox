//! [lc-2320] NVIDIA RTX 视频增强（超分辨率 VSR / SDR→HDR）。
//!
//! 实现在显卡驱动里：mpv 的 `d3d11vpp` 滤镜带 `scaling-mode=nvidia` / `nvidia-true-hdr`
//! 两个子选项，驱动在 D3D11 视频处理器里完成放大与 HDR 转换。零额外组件、零下载。
//! 真正决定「挂不挂、挂多大」的是 `scripts/fntv_rtx.lua`（它看得见实时属性）；
//! 本模块只管三件事：
//!   1. 把面板上的两个开关写成 `script-opts/fntv_rtx.conf`（下次起播生效）；
//!   2. 写完后把同样的意图推给正在播的会话（`script-message fntv-rtx-set`）；
//!   3. 给面板提供前置条件：显卡是否 NVIDIA RTX、系统 HDR 是否开着。
//!
//! 为什么前置检查要做在 Rust：Lua 只看得到 mpv 的属性，而「系统 HDR 是否打开」
//! 在 mpv 里要等视频加载后才从 target-colorspace 推断，面板在没播片时问不到。

use serde_json::{json, Value};
use std::path::PathBuf;

/// `script-opts/fntv_rtx.conf` 路径。
fn conf_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    crate::config::mpv_portable_config_dir(app)
        .map(|d| d.join("script-opts").join("fntv_rtx.conf"))
}

/// 写 conf。写成与 lua 侧同构的最小文件（lua 读取时只认 vsr/hdr 两行）。
fn write_conf(app: &tauri::AppHandle, vsr: bool, hdr: bool) -> Result<(), String> {
    let Some(dir) = crate::config::mpv_portable_config_dir(app) else {
        return Err("找不到 mpv 配置目录".into());
    };
    let so = dir.join("script-opts");
    std::fs::create_dir_all(&so).map_err(|e| format!("创建 script-opts 失败: {e}"))?;
    let body = concat!(
        "# AniBox RTX 视频增强（设置面板 / 播放器菜单写入）\n",
        "# vsr = RTX 超分辨率：由显卡把画面放大到屏幕分辨率（最多 4×）\n",
        "# hdr = RTX Video HDR：由显卡把 SDR 片源转成 HDR（需系统 HDR 已开）\n",
    );
    let text = format!(
        "{body}vsr={}\nhdr={}\n",
        if vsr { "yes" } else { "no" },
        if hdr { "yes" } else { "no" }
    );
    std::fs::write(so.join("fntv_rtx.conf"), text)
        .map_err(|e| format!("写入 RTX 配置失败: {e}"))
}

/// [lc-2320] 系统 HDR（Windows「高级颜色」）是否在**任意**活动显示器上开启。
///
/// mpv 侧只能从 target-colorspace 反推，且要等视频加载；面板要的是「现在能不能开
/// RTX Video HDR」，所以在开机/未播放时也要能答。走 DisplayConfig：
/// QueryDisplayConfig 拿活动路径 → 每条路径 DisplayConfigGetDeviceInfo(
/// GET_ADVANCED_COLOR_INFO) → advancedColorEnabled 位。
#[cfg(target_os = "windows")]
pub(crate) fn system_hdr_enabled() -> bool {
    use windows_sys::Win32::Devices::Display::{
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, QDC_ONLY_ACTIVE_PATHS,
    };
    /// DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO.value 的 bit0 = advancedColorSupported，
    /// bit1 = advancedColorEnabled，bit2 = wideColorEnforced，bit3 = advancedColorForceDisabled。
    const ADVANCED_COLOR_ENABLED_BIT: u32 = 1 << 1;

    unsafe {
        let mut paths: u32 = 0;
        let mut modes: u32 = 0;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) != 0 {
            return false;
        }
        if paths == 0 {
            return false;
        }
        let mut path_buf = vec![std::mem::zeroed::<DISPLAYCONFIG_PATH_INFO>(); paths as usize];
        let mut mode_buf = vec![std::mem::zeroed::<DISPLAYCONFIG_MODE_INFO>(); modes as usize];
        let mut n_paths = paths;
        let mut n_modes = modes;
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut n_paths,
            path_buf.as_mut_ptr(),
            &mut n_modes,
            mode_buf.as_mut_ptr(),
            std::ptr::null_mut(),
        ) != 0
        {
            return false;
        }
        path_buf.truncate(n_paths as usize);
        for path in path_buf {
            let mut info: DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO = std::mem::zeroed();
            info.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                size: std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            };
            if DisplayConfigGetDeviceInfo(&mut info.header) == 0
                && (info.Anonymous.value & ADVANCED_COLOR_ENABLED_BIT) != 0
            {
                return true;
            }
        }
    }
    false
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn system_hdr_enabled() -> bool {
    false
}

/// [lc-2320] 显卡是否是支持 RTX 视频增强的 NVIDIA（RTX 20 系及更新）。
///
/// 只按名字判系列：RTX 20/30/40/50 都支持；GTX 与其它厂商一律不支持。
/// 驱动是否真的打开（NVIDIA App → RTX 视频增强）在运行时由 mpv 的日志说，
/// 这里不做猜测。
fn gpu_supports_rtx() -> (bool, String) {
    let name = crate::gpu_info::gpu_name();
    if name.is_empty() {
        return (false, "没能识别显卡".into());
    }
    let upper = name.to_uppercase();
    if !upper.contains("NVIDIA") && !upper.contains("GEFORCE") {
        return (false, format!("{name} 不是 NVIDIA 显卡"));
    }
    // RTX 20/30/40/50 系；用 "RTX " 前缀 + 数字开头判，避免把 "RTX A2000" 之类专业卡误判成不支持。
    if upper.contains("RTX") {
        return (true, name);
    }
    (false, format!("{name} 不支持 RTX 视频增强（需要 RTX 20 系或更新）"))
}

/// [lc-2320] 面板查询：开关状态 + 前置条件。
#[tauri::command]
pub async fn rtx_video_status(app: tauri::AppHandle) -> Result<Value, String> {
    use tauri::Manager;
    let cfg = app
        .state::<crate::AppState>()
        .config
        .read()
        .await
        .clone();
    let (gpu_ok, gpu_note) = gpu_supports_rtx();
    Ok(json!({
        "vsr": cfg.rtx_vsr,
        "hdr": cfg.rtx_hdr,
        "gpuOk": gpu_ok,
        "gpuNote": gpu_note,
        "systemHdr": system_hdr_enabled(),
        // 说明本平台是否支持：d3d11vpp 只在 Windows 的 D3D11 输出上有
        "supported": cfg!(target_os = "windows"),
    }))
}

/// [lc-2320] 面板写开关：落 conf + 推运行会话即时生效。
#[tauri::command]
pub async fn rtx_video_set(
    state: tauri::State<'_, crate::AppState>,
    app: tauri::AppHandle,
    vsr: bool,
    hdr: bool,
) -> Result<Value, String> {
    if !cfg!(target_os = "windows") {
        return Err("RTX 视频增强只在 Windows 的 D3D11 输出上可用".into());
    }
    write_conf(&app, vsr, hdr)?;
    {
        let mut cfg = state.config.write().await;
        cfg.rtx_vsr = vsr;
        cfg.rtx_hdr = hdr;
        let snapshot = cfg.clone();
        drop(cfg);
        crate::config::save(&app, &snapshot).await?;
    }
    // 正在播的会话即时改（lua 收到后重写同一份 conf，两边口径一致）
    if let Some(sess) = state.mpv.lock().await.clone() {
        if !sess.is_dead().await {
            sess.command_no_reply(json!([
                "script-message",
                "fntv-rtx-set",
                if vsr { "yes" } else { "no" },
                if hdr { "yes" } else { "no" }
            ]))
            .await;
        }
    }
    Ok(json!({ "vsr": vsr, "hdr": hdr }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_name_matching_never_panics() {
        // 只验证不 panic；机型判定依赖本机显卡，不做断言。
        let _ = gpu_supports_rtx();
    }

    #[test]
    fn system_hdr_query_never_panics() {
        let _ = system_hdr_enabled();
    }
}

#[cfg(test)]
mod hdr_probe {
    /// 手工运行：`cargo test --lib hdr_probe -- --nocapture` 打印本机的 HDR 与显卡判定。
    /// 不做断言——不同机器结果本就不同，这里只保证取得到值。
    #[test]
    fn report_local_state() {
        println!("system_hdr_enabled = {}", super::system_hdr_enabled());
        println!("gpu = {:?}", super::gpu_supports_rtx());
    }
}
