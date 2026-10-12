//! mpv 会话：libmpv 进程内嵌入（去进程化）。
//! 历史：子进程 spawn 方案在应用内必死（exit 1/0 静默，lc-074~079 八项假说全排除未解），
//! 改用 libmpv-2.dll 进程内加载——视频 wid 嵌入子窗口，控制走 C API，事件经 tauri emit。
//! 前端协议不变：mpv-event {event:"property-change", id, data}；命令语义同 JSON IPC。

use serde_json::{json, Value};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::libmpv;

pub struct MpvSession {
    pub embed_hwnd: isize,
    handle: Mutex<*mut c_void>,
    dll_dir: String,
}

// mpv_handle 的 command/observe/wait_event 跨线程可用（client.h 线程安全集合）
unsafe impl Send for MpvSession {}
unsafe impl Sync for MpvSession {}

/// 观察 id ↔ 属性名（前端 PlayerView 的 switch 依赖这些 id）
const OBSERVED: [(u64, &str, c_int); 6] = [
    (1, "time-pos", libmpv::MPV_FORMAT_DOUBLE),
    (2, "duration", libmpv::MPV_FORMAT_DOUBLE),
    (3, "pause", libmpv::MPV_FORMAT_FLAG),
    (4, "speed", libmpv::MPV_FORMAT_DOUBLE),
    (5, "volume", libmpv::MPV_FORMAT_DOUBLE),
    // 6：不走前端属性通道，只用于把 uosc 的全屏请求镜像到 Tauri 窗口（见事件线程）
    (6, "fullscreen", libmpv::MPV_FORMAT_FLAG),
];

impl MpvSession {
    /// 创建 libmpv 实例并 wid 嵌入子窗口（实例常驻，播放/停止只切播放状态）
    pub async fn launch(
        app: AppHandle,
        mpv_path: &str,
        embed_hwnd: isize,
    ) -> Result<Arc<MpvSession>, String> {
        let dll_dir = std::path::Path::new(mpv_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let api = libmpv::api(&dll_dir)?;

        // 本轮诊断日志重建：记录 pid + 开关组合（多轮 A/B 不互相污染）
        let env_snapshot = ["FNTV_MPV_VO", "FNTV_MPV_HWDEC", "FNTV_MPV_GPUAPI", "FNTV_MPV_GPU_CTX", "FNTV_MPV_EXTRA"]
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| format!("{}={}", k, v)))
            .collect::<Vec<_>>()
            .join(" ");
        libmpv::trace_reset(&format!("launch pid={} {}", std::process::id(), env_snapshot));
        libmpv::trace("step1: dll loaded, calling mpv_create");
        let h = unsafe { (api.create)() };
        if h.is_null() {
            return Err("mpv_create 失败".into());
        }

        let wid_opt = CString::new(format!("{}", embed_hwnd)).unwrap();
        let wid_key = CString::new("wid").unwrap();
        unsafe { (api.set_option_string)(h, wid_key.as_ptr(), wid_opt.as_ptr()) };
        // mpv 内部日志恒定写文件（主仓库同款），崩溃/异常时留现场
        let mpv_log_path = std::env::temp_dir().join("fntv-mpv.log").to_string_lossy().to_string();
        for (k, v) in [
            // D3D11 视频输出（lc-082 定案）：wid 子窗口 + gpu-next + d3d11va 硬解在本进程内完全正常
            // （日志实证 first video frame shown）。此前「任何 D3D 输出都崩」是误判：
            // 真因＝事件线程按预期格式解引用 data=NULL 的 property-change，Rust2024 的
            // null 解引用前置检查 panic → 非 unwind abort（见下方事件循环注释）。
            ("vo", "gpu-next"),
            ("hwdec", "auto-safe"),
            ("terminal", "no"),
            // 注意：不再禁 osc/input —— 底栏与鼠标键盘交互交给主仓库 portable_config（uosc）
            ("force-window", "immediate"),
            ("idle", "yes"),
            ("keep-open", "yes"),
            ("volume", "100"),
            ("log-file", mpv_log_path.as_str()),
        ] {
            let k = CString::new(k).unwrap();
            let v = CString::new(v).unwrap();
            unsafe { (api.set_option_string)(h, k.as_ptr(), v.as_ptr()) };
        }

        // [lc-083] 复用主仓库 mpv 便携配置（uosc 底栏/菜单/fntv_* 脚本/字体/input.conf 原样）：
        // libmpv 默认不读任何配置文件，必须显式 config=yes + config-dir 才会加载。
        // 与主仓库 mpv.exe（同目录 portable_config 自动生效）行为对齐；显式 API 选项优先于配置文件。
        let portable = std::path::Path::new(&dll_dir).join("portable_config");
        if portable.is_dir() {
            let dir = portable.to_string_lossy().replace('\\', "/");
            for (k, v) in [
                ("config", "yes".to_string()),
                ("config-dir", dir),
                // libmpv 默认不自动加载 scripts/ 目录（与 mpv.exe 不同）——显式打开，
                // 否则 uosc 底栏/菜单以及 fntv_* 脚本全都不生效
                ("load-scripts", "yes".to_string()),
            ] {
                let k = CString::new(k).unwrap();
                let v = CString::new(v).unwrap();
                unsafe { (api.set_option_string)(h, k.as_ptr(), v.as_ptr()) };
            }
            libmpv::trace(&format!("portable_config enabled: {}", portable.display()));
        }

        // 诊断开关（免重编译 A/B，lc-082 攻视频输出崩溃用）：
        // FNTV_MPV_VO / FNTV_MPV_HWDEC / FNTV_MPV_GPUAPI / FNTV_MPV_GPU_CTX / FNTV_MPV_EXTRA("k=v;k=v")
        let mut env_opts: Vec<(String, String)> = Vec::new();
        for (env_k, opt_k) in [
            ("FNTV_MPV_VO", "vo"),
            ("FNTV_MPV_HWDEC", "hwdec"),
            ("FNTV_MPV_GPUAPI", "gpu-api"),
            ("FNTV_MPV_GPU_CTX", "gpu-context"),
        ] {
            if let Ok(v) = std::env::var(env_k) {
                env_opts.push((opt_k.to_string(), v));
            }
        }
        if let Ok(v) = std::env::var("FNTV_MPV_EXTRA") {
            for pair in v.split(';').filter(|s| !s.trim().is_empty()) {
                if let Some((k, val)) = pair.split_once('=') {
                    env_opts.push((k.trim().to_string(), val.trim().to_string()));
                }
            }
        }
        for (k, v) in &env_opts {
            let k = CString::new(k.as_str()).unwrap();
            let v = CString::new(v.as_str()).unwrap();
            unsafe { (api.set_option_string)(h, k.as_ptr(), v.as_ptr()) };
        }
        if !env_opts.is_empty() {
            libmpv::trace(&format!("env opts applied: {:?}", env_opts));
        }

        libmpv::trace(&format!("step2: options set, calling initialize h={:?}", h));
        let err = unsafe { (api.initialize)(h) };
        if err != 0 {
            unsafe { (api.terminate_destroy)(h) };
            return Err(format!("mpv_initialize 失败: {}", libmpv::err_str(api, err)));
        }

        libmpv::trace(&format!("step3: initialize ok, spawning event thread h={:?}", h));
        let session = Arc::new(MpvSession {
            embed_hwnd,
            handle: Mutex::new(h),
            dll_dir: dll_dir.clone(),
        });

        // 观察属性（id 与前端 switch 对齐）
        for (id, name, fmt) in OBSERVED {
            let n = CString::new(name).unwrap();
            unsafe { (api.observe_property)(h, id, n.as_ptr(), fmt) };
        }

        // 事件轮询线程 → tauri emit（前端协议：{event:"property-change", id, data}）
        libmpv::trace("step4: event thread entering loop");
        let app2 = app.clone();
        let sess_for_api = session.clone();
        let sess2 = session.clone();
        std::thread::spawn(move || {
            let mut fs_reset = false; // 上一次 fullscreen=yes 是我们自己复位的（吞掉随之而来的 false）
            loop {
            let h = sess2.handle();
            if h.is_null() {
                break;
            }
            let api = libmpv::api(&sess_for_api.dll_dir).expect("libmpv api");
            let ev = unsafe { (api.wait_event)(h, 0.25) };
            if ev.is_null() {
                continue;
            }
            let e = unsafe { &*ev };
            if e.event_id == libmpv::MPV_EVENT_SHUTDOWN {
                break;
            }
            if e.event_id != libmpv::MPV_EVENT_PROPERTY_CHANGE {
                continue;
            }
            let prop = unsafe { &*(e.data as *const libmpv::MpvEventProperty) };
            if prop.name.is_null() {
                continue;
            }
            let name = unsafe { CStr::from_ptr(prop.name) }.to_string_lossy().to_string();
            let Some((id, _, _)) = OBSERVED.iter().find(|(_, n, _)| *n == name) else {
                continue;
            };
            // ⚠ 必须按事件自带的 format 解码并防 data=NULL：属性不可用/切换时 mpv 发
            // MPV_FORMAT_NONE + data=null（此前按预期格式解引用 → Rust2024 debug 的
            // "null pointer dereference" 前置检查 panic → 非 unwind abort，掩盖成「D3D 崩溃」）
            let data = unsafe {
                if prop.data.is_null() {
                    Value::Null
                } else {
                    match prop.format {
                        libmpv::MPV_FORMAT_DOUBLE => json!(*(prop.data as *const f64)),
                        libmpv::MPV_FORMAT_FLAG => json!(*(prop.data as *const c_int) != 0),
                        libmpv::MPV_FORMAT_STRING => {
                            json!(CStr::from_ptr(prop.data as *const c_char).to_string_lossy().to_string())
                        }
                        _ => Value::Null,
                    }
                }
            };

            // fullscreen（id 6）：mpv 是 wid 子窗口，让它自己全屏会把画面搞乱 ——
            // 收到 uosc/双击的全屏请求后立刻复位 mpv 的 fullscreen，改为把事件抛给前端
            // 去全屏 Tauri 窗口；我们自己复位产生的那次 false 事件要吞掉（fs_reset 标记）
            if *id == 6 {
                if data == json!(true) {
                    fs_reset = true;
                    let _ = app2.emit("mpv-fullscreen", json!({ "on": true }));
                    let args: Vec<CString> = ["set", "fullscreen", "no"]
                        .iter()
                        .map(|s| CString::new(s.as_bytes().to_vec()).unwrap())
                        .collect();
                    let mut ptrs: Vec<*mut c_char> =
                        args.iter().map(|c| c.as_ptr() as *mut c_char).collect();
                    ptrs.push(std::ptr::null_mut());
                    unsafe { (api.command)(h, ptrs.as_mut_ptr()) };
                } else if fs_reset {
                    fs_reset = false;
                } else {
                    let _ = app2.emit("mpv-fullscreen", json!({ "on": false }));
                }
                continue;
            }
            let _ = app2.emit(
                "mpv-event",
                json!({ "event": "property-change", "id": id, "data": data }),
            );
            }
        });

        Ok(session)
    }

    fn handle(&self) -> *mut c_void {
        *self.handle.lock().unwrap()
    }

    fn api(&self) -> &'static libmpv::Api {
        libmpv::api(&self.dll_dir).expect("libmpv api")
    }

    /// JSON 数组命令 → mpv_command（语义对齐原 JSON IPC）
    pub async fn command(&self, cmd: Value) -> Result<Value, String> {
        let arr = cmd.as_array().ok_or("命令必须是数组")?;
        let cstrs: Vec<CString> = arr
            .iter()
            .map(|v| {
                CString::new(match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
            })
            .collect();
        let mut ptrs: Vec<*mut c_char> = cstrs.iter().map(|c| c.as_ptr() as *mut c_char).collect();
        ptrs.push(std::ptr::null_mut());

        let api = self.api();
        let op = cstrs.first().map(|c| c.to_string_lossy().to_string()).unwrap_or_default();
        let err = unsafe { (api.command)(self.handle(), ptrs.as_mut_ptr()) };
        if err != 0 {
            let msg = libmpv::err_str(api, err);
            libmpv::trace(&format!("command {} failed: {}", op, msg));
            return Err(msg);
        }
        libmpv::trace(&format!("command {}", op));
        Ok(json!({ "error": "success" }))
    }

    /// libmpv 常驻：随应用退出销毁，会话本身不「死亡」
    pub async fn is_dead(&self) -> bool {
        false
    }

    /// 停止播放（回 idle），实例保留
    pub async fn stop(&self) -> Result<(), String> {
        self.command(json!(["stop"])).await.map(|_| ())
    }

    pub async fn shutdown(&self) {
        let h = self.handle();
        if !h.is_null() {
            let api = self.api();
            unsafe { (api.terminate_destroy)(h) };
        }
        *self.handle.lock().unwrap() = std::ptr::null_mut();
        crate::win_embed::destroy(self.embed_hwnd);
    }
}
