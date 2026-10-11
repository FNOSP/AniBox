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
const OBSERVED: [(u64, &str, c_int); 5] = [
    (1, "time-pos", libmpv::MPV_FORMAT_DOUBLE),
    (2, "duration", libmpv::MPV_FORMAT_DOUBLE),
    (3, "pause", libmpv::MPV_FORMAT_FLAG),
    (4, "speed", libmpv::MPV_FORMAT_DOUBLE),
    (5, "volume", libmpv::MPV_FORMAT_DOUBLE),
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

        { let _ = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("fntv-libmpv.log")).and_then(|mut f| std::io::Write::write_all(&mut f, format!("{}
", "step1: dll loaded, calling mpv_create").as_bytes())); }
        let h = unsafe { (api.create)() };
        if h.is_null() {
            return Err("mpv_create 失败".into());
        }

        let wid_opt = CString::new(format!("{}", embed_hwnd)).unwrap();
        let wid_key = CString::new("wid").unwrap();
        unsafe { (api.set_option_string)(h, wid_key.as_ptr(), wid_opt.as_ptr()) };
        for (k, v) in [
            // ⚠ 已知问题（lc-081）：任何 D3D 视频输出（gpu-next/gpu/direct3d × wid/自建窗）在
            // Tauri 进程内 access violation 崩溃（WebView2 --disable-gpu 也崩）——平台级冲突待专项。
            // 暂用默认 vo=libmpv：解码/音频正常、无画面（应用稳定优先）。下轮攻 render API(GL) 或崩溃转储。

            ("terminal", "no"),
            ("osc", "no"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("force-window", "immediate"),
            ("idle", "yes"),
            ("keep-open", "yes"),
            ("volume", "100"),
        ] {
            let k = CString::new(k).unwrap();
            let v = CString::new(v).unwrap();
            unsafe { (api.set_option_string)(h, k.as_ptr(), v.as_ptr()) };
        }

        { let _ = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("fntv-libmpv.log")).and_then(|mut f| std::io::Write::write_all(&mut f, format!("{}
", format!("step2: wid set, calling initialize h={:?}", h)).as_bytes())); }
        let err = unsafe { (api.initialize)(h) };
        if err != 0 {
            unsafe { (api.terminate_destroy)(h) };
            return Err(format!("mpv_initialize 失败: {}", libmpv::err_str(api, err)));
        }

        { let _ = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("fntv-libmpv.log")).and_then(|mut f| std::io::Write::write_all(&mut f, format!("{}
", format!("step3: initialize ok, spawning event thread h={:?}", h)).as_bytes())); }
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
        { let _ = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("fntv-libmpv.log")).and_then(|mut f| std::io::Write::write_all(&mut f, format!("{}
", "step4: event thread entering loop").as_bytes())); }
        let app2 = app.clone();
        let sess_for_api = session.clone();
        let sess2 = session.clone();
        std::thread::spawn(move || loop {
            let h = sess2.handle();
            if h.is_null() {
                break;
            }
            let api = libmpv::api(&sess_for_api.dll_dir).expect("libmpv api");
            libmpv::trace("before wait_event");
            let ev = unsafe { (api.wait_event)(h, 0.25) };
            libmpv::trace("after wait_event");
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
            let Some((id, _, fmt)) = OBSERVED.iter().find(|(_, n, _)| *n == name) else {
                continue;
            };
            let data = unsafe {
                match *fmt {
                    libmpv::MPV_FORMAT_DOUBLE => json!((*(prop.data as *const f64)) as f64),
                    libmpv::MPV_FORMAT_FLAG => json!((*(prop.data as *const c_int)) != 0),
                    _ => Value::Null,
                }
            };
            let _ = app2.emit(
                "mpv-event",
                json!({ "event": "property-change", "id": id, "data": data }),
            );
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
        libmpv::trace(&format!("before command {:?}", &cstrs.iter().map(|c| c.to_string_lossy()).collect::<Vec<_>>()));
        let err = unsafe { (api.command)(self.handle(), ptrs.as_mut_ptr()) };
        libmpv::trace(&format!("after command err={}", err));
        if err != 0 {
            return Err(libmpv::err_str(api, err));
        }
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
