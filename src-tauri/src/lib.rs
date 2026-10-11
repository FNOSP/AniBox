//! Fntv-Plus Tauri2 PoC —— 自绘 UI + 直调飞牛 API + mpv 双层窗口内嵌。
//! 命令面向前端：配置 / 登录 / 媒体库 / 播放控制。

mod authx;
mod config;
mod fnapi;
mod imgproto;
mod mpv;
mod proxy;
mod tmdb;
mod win_embed;

use config::Config;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
use tokio::sync::{Mutex, RwLock};

pub struct AppState {
    pub config: RwLock<Config>,
    pub mpv: Mutex<Option<Arc<mpv::MpvSession>>>,
    pub proxy_child: Mutex<Option<std::process::Child>>,
    /// 播放占位区域的物理像素矩形 (x, y, w, h)
    pub bounds: std::sync::Mutex<(i32, i32, i32, i32)>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            config: RwLock::new(Config::default()),
            mpv: Mutex::new(None),
            proxy_child: Mutex::new(None),
            bounds: std::sync::Mutex::new((0, 0, 800, 450)),
        }
    }
}

// ───────────────────────── 配置 ─────────────────────────

#[tauri::command]
async fn get_config(state: State<'_, AppState>) -> Result<Config, String> {
    Ok(state.config.read().await.clone())
}

#[tauri::command]
async fn save_config(app: AppHandle, state: State<'_, AppState>, config: Config) -> Result<(), String> {
    config::save(&app, &config).await?;
    *state.config.write().await = config;
    Ok(())
}

// ───────────────────────── 登录 / 会话 ─────────────────────────

#[tauri::command]
async fn login(
    app: AppHandle,
    state: State<'_, AppState>,
    server: String,
    username: String,
    password: String,
) -> Result<Value, String> {
    let client = fnapi::new_client();
    let server = fnapi::normalize_server(&server);
    let data = client
        .call(
            &server,
            "/v/api/v1/login",
            "POST",
            "",
            Some(json!({ "app_name": "trimemedia-web", "username": username, "password": password })),
        )
        .await?;
    let token = data
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "登录响应缺少 token".to_string())?
        .to_string();

    // 固化真实基址：80 端口会把 API 302 到实际端口（如 :17777），图片/播放都依赖它
    let server = client.resolve_final_base(&server, &token).await;

    let mut cfg = state.config.read().await.clone();
    cfg.server = server;
    cfg.username = username;
    cfg.token = token;
    config::save(&app, &cfg).await?;
    *state.config.write().await = cfg.clone();
    Ok(json!({ "ok": true }))
}

#[tauri::command]
async fn logout(state: State<'_, AppState>) -> Result<(), String> {
    let mut cfg = state.config.write().await;
    cfg.token = String::new();
    Ok(())
}

#[tauri::command]
async fn check_session(state: State<'_, AppState>, app: AppHandle) -> Result<Value, String> {
    let mut cfg = state.config.read().await.clone();
    if cfg.token.is_empty() || cfg.server.is_empty() {
        return Err("未登录".into());
    }
    let client = fnapi::new_client();
    let data = client
        .call(&cfg.server, "/v/api/v1/user/info", "GET", &cfg.token, None)
        .await?;
    // 会话有效时顺手把重定向后的真实基址固化（老配置可能还停在 80 端口）
    let final_base = client.resolve_final_base(&cfg.server, &cfg.token).await;
    if final_base != cfg.server {
        cfg.server = final_base;
        let _ = config::save(&app, &cfg);
        *state.config.write().await = cfg;
    }
    Ok(data)
}

// ───────────────────────── 媒体库 / 条目 ─────────────────────────

#[tauri::command]
async fn get_media_libs(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(&cfg.server, "/v/api/v1/mediadb/list", "GET", &cfg.token, None)
        .await
}

/// 条目分页查询（形状经测试 NAS 实测 + FlyNarwhal 源码互证）：
/// - 库根浏览：只传 ancestor_guid（parent_guid 传库 guid 服务端返回 0 条）；
///   实测同库白名单 151 条 vs 无过滤 2635 条剧集混排 —— Episode 全靠白名单滤掉，
///   tags.type 白名单不含 Season/Episode（对齐 FlyNarwhal libraryBrowseValues）。
/// - 文件夹钻取（未识别 Directory/fv_*）：只传 parent_guid，exclude_grouped_video=0
///   （FlyNarwhal 注释：钻取带 1 会让原始视频文件夹返回空），sort_title ASC 对齐网页。
#[tauri::command]
async fn get_items(
    state: State<'_, AppState>,
    ancestor_guid: Option<String>,
    parent_guid: Option<String>,
    page: Option<i64>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let browse_types = json!(["Movie", "TV", "Directory", "Video", "LiveChannel"]);
    let body = if let Some(parent) = parent_guid {
        json!({
            "parent_guid": parent,
            "exclude_grouped_video": 0,
            "sort_type": "ASC",
            "sort_column": "sort_title",
            "page_size": 60,
            "page": page.unwrap_or(1),
            "tags": { "type": browse_types }
        })
    } else {
        json!({
            "ancestor_guid": ancestor_guid,
            "exclude_grouped_video": 1,
            "sort_type": "DESC",
            "sort_column": "create_time",
            "page_size": 60,
            "page": page.unwrap_or(1),
            "tags": { "type": browse_types }
        })
    };
    client
        .call(&cfg.server, "/v/api/v1/item/list", "POST", &cfg.token, Some(body))
        .await
}

/// 继续观看：GET /v/api/v1/play/list（camelCase 字段）
#[tauri::command]
async fn get_continue_watching(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(&cfg.server, "/v/api/v1/play/list", "GET", &cfg.token, None)
        .await
}

/// 季列表：GET /v/api/v1/season/list/{剧guid}
/// 注意：episode/list 的入参是**季** guid —— 传剧 guid 实测返回 data:null。
#[tauri::command]
async fn get_seasons(state: State<'_, AppState>, guid: String) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let path = format!("/v/api/v1/season/list/{}", guid);
    client.call(&cfg.server, &path, "GET", &cfg.token, None).await
}

/// 集列表：GET /v/api/v1/episode/list/{guid} —— guid 必须是季 guid（见 get_seasons 注释）
#[tauri::command]
async fn get_episodes(state: State<'_, AppState>, guid: String) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let path = format!("/v/api/v1/episode/list/{}", guid);
    client.call(&cfg.server, &path, "GET", &cfg.token, None).await
}

/// 媒体统计：GET /v/api/v1/mediadb/sum —— 一个接口给全所有计数
/// （库 guid→数量、favorite、movie/tv/live/video 分类数、total）
#[tauri::command]
async fn get_media_sum(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(&cfg.server, "/v/api/v1/mediadb/sum", "GET", &cfg.token, None)
        .await
}

/// 收藏列表：POST /v/api/v1/favorite/list（响应 {total,list} 同 item/list）
#[tauri::command]
async fn get_favorites(
    state: State<'_, AppState>,
    page: Option<i64>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(
            &cfg.server,
            "/v/api/v1/favorite/list",
            "POST",
            &cfg.token,
            Some(json!({
                "tags": { "type": [] },
                "sort_type": "DESC",
                "sort_column": "create_time",
                "exclude_grouped_video": 1,
                "page": page.unwrap_or(1),
                "page_size": 60
            })),
        )
        .await
}

/// 全局分类浏览：不带 ancestor/parent，按 tags.type 过滤（电影/电视节目/直播/其他）
#[tauri::command]
async fn get_items_global(
    state: State<'_, AppState>,
    type_filter: Option<Vec<String>>,
    page: Option<i64>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(
            &cfg.server,
            "/v/api/v1/item/list",
            "POST",
            &cfg.token,
            Some(json!({
                "tags": { "type": type_filter.unwrap_or_default() },
                "sort_type": "DESC",
                "sort_column": "create_time",
                "exclude_grouped_video": 1,
                "page": page.unwrap_or(1),
                "page_size": 60
            })),
        )
        .await
}

/// 条目详情：端点与响应字段对齐 FlyNarwhal（GET /v/api/v1/item/{guid}，camelCase 字段）
#[tauri::command]
async fn get_item_detail(state: State<'_, AppState>, guid: String) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let path = format!("/v/api/v1/item/{}", guid);
    client.call(&cfg.server, &path, "GET", &cfg.token, None).await
}

/// 轮播数据：全局白名单查询（对齐主仓库 itemListApi.ts —— type:['Movie','TV']、
/// 最近更新在前、排除未识别/直播/个人视频），不限定库。
#[tauri::command]
async fn get_carousel_items(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(
            &cfg.server,
            "/v/api/v1/item/list",
            "POST",
            &cfg.token,
            Some(json!({
                "tags": { "type": ["Movie", "TV"] },
                "sort_type": "DESC",
                "sort_column": "create_time",
                "exclude_grouped_video": 1,
                "page": 1,
                "page_size": 24
            })),
        )
        .await
}

// ───────────────────────── 播放 / mpv 控制 ─────────────────────────

/// 创建 mpv 内嵌子窗口；句柄 0（创建失败）必须拒绝，否则 mpv --wid=0 会弹出独立窗口
fn new_embed(parent_hwnd: isize) -> Result<isize, String> {
    let embed = win_embed::create_child(parent_hwnd);
    if embed == 0 {
        return Err("创建 mpv 内嵌子窗口失败".into());
    }
    Ok(embed)
}

#[tauri::command]
async fn play_item(state: State<'_, AppState>, app: AppHandle, guid: String) -> Result<(), String> {
    let cfg = state.config.read().await.clone();
    if cfg.token.is_empty() {
        return Err("未登录".into());
    }
    proxy::ensure(&cfg.proxy_path, &state.proxy_child).await?;

    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    let parent_hwnd = main.hwnd().map_err(|e| e.to_string())?.0 as isize;

    let mut slot = state.mpv.lock().await;
    let session = match slot.as_ref() {
        Some(m) if !m.is_dead().await => m.clone(),
        Some(m) => {
            m.shutdown().await;
            mpv::MpvSession::launch(app, &cfg.mpv_path, new_embed(parent_hwnd)?).await?
        }
        None => {
            mpv::MpvSession::launch(app, &cfg.mpv_path, new_embed(parent_hwnd)?).await?
        }
    };
    *slot = Some(session.clone());
    drop(slot);

    // 应用当前 bounds（登页时前端会更新），再载入
    let (x, y, w, h) = *state.bounds.lock().unwrap();
    win_embed::set_bounds(session.embed_hwnd, x, y, w, h);
    win_embed::set_visible(session.embed_hwnd, true);

    let url = proxy::play_url(&cfg, &guid);
    session.command(json!(["loadfile", url, "replace"])).await?;
    Ok(())
}

#[tauri::command]
async fn player_set_bounds(
    state: State<'_, AppState>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(), String> {
    let b = (x as i32, y as i32, w as i32, h as i32);
    *state.bounds.lock().unwrap() = b;
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        win_embed::set_bounds(m.embed_hwnd, b.0, b.1, b.2, b.3);
    }
    Ok(())
}

#[tauri::command]
async fn player_set_visible(state: State<'_, AppState>, visible: bool) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        win_embed::set_visible(m.embed_hwnd, visible);
        if visible {
            win_embed::keep_bottom(m.embed_hwnd);
        }
    }
    Ok(())
}

#[tauri::command]
async fn player_stop(state: State<'_, AppState>) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        let _ = m.command(json!(["stop"])).await;
    }
    Ok(())
}

#[tauri::command]
async fn mpv_toggle_pause(state: State<'_, AppState>) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        m.command(json!(["cycle", "pause"])).await?;
    }
    Ok(())
}

#[tauri::command]
async fn mpv_seek(state: State<'_, AppState>, seconds: f64, absolute: bool) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        let mode = if absolute { "absolute" } else { "relative" };
        m.command(json!(["seek", seconds, mode])).await?;
    }
    Ok(())
}

#[tauri::command]
async fn mpv_set_volume(state: State<'_, AppState>, volume: f64) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        m.command(json!(["set_property", "volume", volume])).await?;
    }
    Ok(())
}

#[tauri::command]
async fn mpv_set_rate(state: State<'_, AppState>, rate: f64) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        m.command(json!(["set_property", "speed", rate])).await?;
    }
    Ok(())
}

// ───────────────────────── 入口 ─────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
// ───────────────────────── 窗口视觉：SWCA 动态模糊 + OS 级圆角 ─────────────────────────
// windowEffects 的 acrylic 在 Win11 走 DWM 系统底板（自带系统圆角/边框，lc-029 已弃用）。
// 这里改用 SWCA（SetWindowCompositionAttribute， undocumented API，无系统边框副作用），
// 再用 SetWindowRgn 把整个窗口（含模糊层）裁成圆角矩形——模糊不会从方角露出。

#[cfg(target_os = "windows")]
mod winvisual {
    const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
    const WCA_ACCENT_POLICY: u32 = 19;

    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        gradient_color: u32, // ABGR
        animation_id: u32,
    }

    #[repr(C)]
    struct WinCompAttribData {
        attribute: u32,
        data: *mut AccentPolicy,
        size: usize,
    }

    /// 主题感知染色（ABGR）：浅色近白、深色近黑，alpha 控制玻璃厚度
    pub fn tint_for(light: bool) -> u32 {
        if light {
            0x50FEFBFA // #FAFBFE @ 31%
        } else {
            0x591F1614 // #14161F @ 35%
        }
    }

    fn module_handle(name: &str) -> isize {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { GetModuleHandleW(wide.as_ptr()) as isize }
    }

    fn get_proc(module: isize, name: &str) -> Option<unsafe extern "system" fn() -> isize> {
        use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
        let narrow: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
        unsafe { GetProcAddress(module as _, narrow.as_ptr()) }
    }

    pub fn set_acrylic(hwnd: isize, tint_abgr: u32) {
        unsafe {
            let f = get_proc(module_handle("user32.dll"), "SetWindowCompositionAttribute");
            if let Some(f) = f {
                type SetFn = unsafe extern "system" fn(isize, *mut WinCompAttribData) -> i32;
                let set: SetFn = std::mem::transmute(f);
                let mut policy = AccentPolicy {
                    state: ACCENT_ENABLE_ACRYLICBLURBEHIND,
                    flags: 0,
                    gradient_color: tint_abgr,
                    animation_id: 0,
                };
                let mut data = WinCompAttribData {
                    attribute: WCA_ACCENT_POLICY,
                    data: &mut policy,
                    size: std::mem::size_of::<AccentPolicy>(),
                };
                set(hwnd, &mut data);
            }
        }
    }

    /// 窗口级圆角裁剪（物理像素半径）：SWCA 模糊层与 webview 一起被裁进圆角
    pub fn round_window(hwnd: isize, width: i32, height: i32, radius: i32) {
        use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
        unsafe {
            let rgn = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius * 2, radius * 2);
            if !rgn.is_null() {
                SetWindowRgn(hwnd as _, rgn, 1);
            }
        }
    }

    pub fn hwnd_isize(win: &tauri::Window) -> Option<isize> {
        win.hwnd().ok().map(|h| h.0 as isize)
    }

    pub fn hwnd_isize_wv(win: &tauri::WebviewWindow) -> Option<isize> {
        win.hwnd().ok().map(|h| h.0 as isize)
    }
}

#[tauri::command]
fn set_acrylic_tint(window: tauri::Window, light: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use winvisual::{hwnd_isize, set_acrylic, tint_for};
        if let Some(hwnd) = hwnd_isize(&window) {
            set_acrylic(hwnd, tint_for(light));
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (window, light);
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .register_asynchronous_uri_scheme_protocol("fntvimg", |ctx, request, responder| {
            // 带会话鉴权的图片代理：WebView 里 <img> 直连 NAS 不带 Cookie 会全裂图
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                let response = imgproto::handle(app, request).await;
                responder.respond(response);
            });
        })
        .on_window_event(|window, event| {
            // 窗口尺寸变化时重设圆角区域；SetWindowRgn 开销大，拖拽中限频 + 尾随落定
            #[cfg(target_os = "windows")]
            if let tauri::WindowEvent::Resized(size) = event {
                use std::sync::atomic::{AtomicBool, Ordering};
                use std::time::{Duration, Instant};
                use winvisual::{hwnd_isize, round_window};
                static LAST_APPLY: std::sync::Mutex<(i32, i32, Option<Instant>)> =
                    std::sync::Mutex::new((0, 0, None));
                static TRAILING: AtomicBool = AtomicBool::new(false);
                let (w, h) = (size.width as i32, size.height as i32);
                let mut last = LAST_APPLY.lock().unwrap();
                if (last.0, last.1) == (w, h) {
                    return;
                }
                let due = match last.2 {
                    None => true,
                    Some(t) => t.elapsed() >= Duration::from_millis(120),
                };
                if due {
                    if let Some(hwnd) = hwnd_isize(window) {
                        let scale = window.scale_factor().unwrap_or(1.0);
                        round_window(hwnd, w, h, (16.0 * scale) as i32);
                    }
                    *last = (w, h, Some(Instant::now()));
                } else if !TRAILING.swap(true, Ordering::SeqCst) {
                    let win2 = window.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(160)).await;
                        TRAILING.store(false, Ordering::SeqCst);
                        if let Some(hwnd) = hwnd_isize(&win2) {
                            let sz = win2.inner_size().unwrap_or_default();
                            let scale = win2.scale_factor().unwrap_or(1.0);
                            round_window(hwnd, sz.width as i32, sz.height as i32, (16.0 * scale) as i32);
                            *LAST_APPLY.lock().unwrap() =
                                (sz.width as i32, sz.height as i32, Some(Instant::now()));
                        }
                    });
                }
            }
            let _ = window;
        })
        .setup(|app| {
            // 启动时从磁盘读回配置（token/路径），否则每次重启都要重新登录
            let cfg = tauri::async_runtime::block_on(config::load(app.handle()));
            *app.state::<AppState>().config.blocking_write() = cfg;
            // Win11 会给窗口画一圈系统细边框 —— 用户眼中的「外面又包了一层线条」，
            // DWMWA_BORDER_COLOR(34) 设为 DWMWA_COLOR_NONE(0xFFFFFFFE) 关掉（旧系统调用失败无副作用）
            #[cfg(target_os = "windows")]
            if let Some(win) = app.get_webview_window("main") {
                use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
                if let Ok(hwnd) = win.hwnd() {
                    let color_none: u32 = 0xFFFF_FFFE;
                    unsafe {
                        DwmSetWindowAttribute(
                            hwnd.0 as _,
                            34, // DWMWA_BORDER_COLOR
                            &color_none as *const u32 as *const core::ffi::c_void,
                            4,
                        );
                    }
                }
            }
            // SWCA 动态模糊 + 窗口级圆角（默认浅色 tint；主题切换由前端 set_acrylic_tint 更新）
            #[cfg(target_os = "windows")]
            if let Some(win) = app.get_webview_window("main") {
                use winvisual::{hwnd_isize_wv, round_window, set_acrylic, tint_for};
                if let Some(hwnd) = hwnd_isize_wv(&win) {
                    set_acrylic(hwnd, tint_for(true));
                    let size = win.inner_size().unwrap_or_default();
                    let scale = win.scale_factor().unwrap_or(1.0);
                    round_window(hwnd, size.width as i32, size.height as i32, (16.0 * scale) as i32);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_acrylic_tint,
            tmdb::tmdb_show_details,
            get_config,
            save_config,
            login,
            logout,
            check_session,
            get_media_libs,
            get_media_sum,
            get_favorites,
            get_items_global,
            get_items,
            get_continue_watching,
            get_seasons,
            get_episodes,
            get_item_detail,
            get_carousel_items,
            play_item,
            player_set_bounds,
            player_set_visible,
            player_stop,
            mpv_toggle_pause,
            mpv_seek,
            mpv_set_volume,
            mpv_set_rate
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                // 退出时清场：mpv 子进程 + 嵌入窗口 + Go 代理
                let st = app_handle.state::<AppState>();
                if let Ok(mut slot) = st.mpv.try_lock() {
                    if let Some(m) = slot.as_ref() {
                        let _ = tauri::async_runtime::block_on(m.shutdown());
                    }
                    *slot = None;
                }
                let mut pc = st.proxy_child.blocking_lock();
                if let Some(c) = pc.as_mut() {
                    let _ = c.kill();
                }
            }
        });
}
