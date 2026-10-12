//! Fntv-Plus Tauri2 PoC —— 自绘 UI + 直调飞牛 API + mpv 双层窗口内嵌。
//! 命令面向前端：配置 / 登录 / 媒体库 / 播放控制。

mod authx;
mod config;
mod fnapi;
mod libmpv;
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
    /// 最近一次前端主题（player_stop 后恢复 SWCA 染色用）
    pub last_light: std::sync::Mutex<Option<bool>>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            config: RwLock::new(Config::default()),
            mpv: Mutex::new(None),
            proxy_child: Mutex::new(None),
            bounds: std::sync::Mutex::new((0, 0, 800, 450)),
            last_light: std::sync::Mutex::new(None),
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

/// 在主线程执行 Win32 窗口操作：tao 窗口/子窗口必须在有消息泵的线程上创建与操作，
/// 否则窗口不处理消息，mpv 的 D3D 交换链初始化会挂死（unable to create window → VO 全灭）
fn on_main<R: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce() -> R + Send + 'static,
) -> Result<R, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| format!("主线程调度失败: {}", e))?;
    rx.recv_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| format!("主线程执行超时: {}", e))
}

/// 创建 mpv 内嵌子窗口（主线程）；句柄 0（创建失败）必须拒绝，否则 mpv --wid=0 会弹出独立窗口
fn new_embed(app: &AppHandle, parent_hwnd: isize) -> Result<isize, String> {
    let embed = on_main(app, move || win_embed::create_child(parent_hwnd))?;
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
            let embed = new_embed(&app, parent_hwnd)?;
            mpv::MpvSession::launch(app.clone(), &cfg.mpv_path, embed).await?
        }
        None => {
            let embed = new_embed(&app, parent_hwnd)?;
            mpv::MpvSession::launch(app.clone(), &cfg.mpv_path, embed).await?
        }
    };
    *slot = Some(session.clone());
    drop(slot);

    // 应用当前 bounds（登页时前端会更新），再载入（窗口操作一律主线程）
    let (x, y, w, h) = *state.bounds.lock().unwrap();
    let eh = session.embed_hwnd;
    on_main(&app, move || {
        win_embed::set_bounds(eh, x, y, w, h);
        win_embed::set_visible(eh, true);
    })?;

    let url = proxy::play_url(&cfg, &guid);
    session.command(json!(["loadfile", url, "replace"])).await?;

    // [lc-085] 不再在播放期禁用窗口模糊（原 lc-081 的 clear_backdrop 是为了救 spawn 时代
    // 「进播放页整机卡死」；现在视频是进程内 D3D 输出，实测保留主题底更符合预期——
    // 用户反馈「一播放详情页就变半透明」正是底板被清掉后页面透出桌面所致）。player_stop 里
    // 的恢复逻辑保留无副作用（重复设置同值）。

    Ok(())
}

/// [lc-086] 悬停浮层控制条：真实 WS_CHILD 子窗（parent_raw）+ 独立 WebView，挂在主窗上 →
/// 随主窗移动/裁剪、不进任务栏；浮在 mpv 视频窗之上，鼠标移入视频才出现（mpv/uosc 观感）。
/// 必须另开窗口的理由：视频是原生窗、浮在 WebView 之上，主页面里的 HTML 盖不到它。
const BAR_H_CSS: f64 = 54.0;

fn ensure_bar(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    if let Some(w) = app.get_webview_window("playerbar") {
        return Ok(w);
    }
    let main = app.get_webview_window("main").ok_or("主窗口不存在")?;
    let hwnd = main.hwnd().map_err(|e| e.to_string())?;
    let win = tauri::WebviewWindowBuilder::new(
        app,
        "playerbar",
        tauri::WebviewUrl::App("bar.html".into()),
    )
    .parent_raw(hwnd)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .skip_taskbar(true)
    .resizable(false)
    .visible(false)
    .build()
    .map_err(|e| format!("创建播放控制条窗口失败: {}", e))?;
    Ok(win)
}

/// 把控制条贴到视频区底部（x/y/w/h 与 player_set_bounds 同源：相对主窗客户区的物理像素）
fn place_bar(app: &AppHandle, x: f64, y: f64, w: f64, h: f64) {
    let Ok(bar) = ensure_bar(app) else { return };
    let dpr = app
        .get_webview_window("main")
        .and_then(|m| m.scale_factor().ok())
        .unwrap_or(1.0);
    let bar_h = (BAR_H_CSS * dpr).round().max(1.0);
    let _ = bar.set_size(tauri::PhysicalSize::new(w.max(16.0) as u32, bar_h as u32));
    let _ = bar.set_position(tauri::PhysicalPosition::new(x, (y + h - bar_h).max(0.0)));
}

/// 控制条显隐（页面在鼠标移入视频/控制条时置 true，静置后置 false）
#[tauri::command]
async fn bar_show(app: AppHandle, state: State<'_, AppState>, visible: bool) -> Result<(), String> {
    let bar = ensure_bar(&app)?;
    if visible {
        let b = *state.bounds.lock().unwrap();
        place_bar(&app, b.0 as f64, b.1 as f64, b.2 as f64, b.3 as f64);
        bar.show().map_err(|e| e.to_string())?;
        // 抬到最上层：mpv 视频宿主窗也是子窗，必须压过它（视频窗自身 hit-transparent，点击不受影响）
        if let Ok(h) = bar.hwnd() {
            win_embed::keep_top(h.0 as isize);
        }
    } else {
        bar.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn player_set_bounds(
    app: AppHandle,
    state: State<'_, AppState>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(), String> {
    let b = (x as i32, y as i32, w as i32, h as i32);
    *state.bounds.lock().unwrap() = b;
    place_bar(&app, x, y, w, h); // 视频区变化时控制条跟随（未创建/未显示都无害）
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        let eh = m.embed_hwnd;
        // app 来自 command 参数
        on_main(&app, move || win_embed::set_bounds(eh, b.0, b.1, b.2, b.3))?;
    }
    Ok(())
}

#[tauri::command]
async fn player_set_visible(
    app: AppHandle,
    state: State<'_, AppState>,
    visible: bool,
) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        let eh = m.embed_hwnd;
        // app 来自 command 参数
        on_main(&app, move || {
            win_embed::set_visible(eh, visible);
            if visible {
                win_embed::keep_top(eh);
            }
        })?;
    }
    Ok(())
}

#[tauri::command]
async fn player_stop(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if let Ok(bar) = ensure_bar(&app) {
        let _ = bar.hide();
    }
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        let _ = m.command(json!(["stop"])).await;
    }
    // 播放结束：恢复主题染色（play_item 时被禁用的 SWCA 模糊）
    #[cfg(target_os = "windows")]
    if let Some(light) = *state.last_light.lock().unwrap() {
        use winvisual::{hwnd_isize_wv, set_backdrop};
        if let Some(win) = app.get_webview_window("main") {
            if let Some(hwnd) = hwnd_isize_wv(&win) {
                set_backdrop(hwnd, !light);
            }
        }
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

/// [lc-085] 输入转发：视频是原生子窗（浮在 WebView 之上）且 libmpv 内嵌模式下 mpv 不自行
/// 接收鼠标/键盘（它建的嵌入子窗是 disabled 的）——鼠标事件由页面代收后经此命令转成 mpv 的
/// `mouse` / `keypress` 命令。uosc 底栏出现、左键暂停、右键菜单、滚轮音量、双击全屏全靠这条通道。
/// 坐标语义：相对视频窗客户区的**设备像素**（前端已按 dpr 换算）。
/// 按钮编码同 mpv：0=左 1=中 2=右；负数=释放（-1 左 / -2 中 / -3 右）；滚轮 3=上 4=下。
#[tauri::command]
async fn mpv_input(
    state: State<'_, AppState>,
    kind: String,
    x: f64,
    y: f64,
    button: Option<i64>,
    key: Option<String>,
) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        let cmd = match kind.as_str() {
            "move" => json!(["mouse", x as i64, y as i64]),
            "button" | "wheel" => json!(["mouse", x as i64, y as i64, button.unwrap_or(0)]),
            "key" => json!(["keypress", key.unwrap_or_default()]),
            _ => return Ok(()),
        };
        if kind != "move" {
            libmpv::trace(&format!("input {} -> {:?}", kind, cmd)); // 低频事件留痕（move 太频繁不记）
        }
        // 鼠标位置偶发越界等返回非 success，属正常，不向页面冒错
        let _ = m.command(cmd).await;
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
    /// 关闭系统底板（DWMSBT_NONE）—— mpv 播放期间禁用全窗模糊，消合成开销
    pub fn clear_backdrop(hwnd: isize) {
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        unsafe {
            let none: u32 = 1; // DWMSBT_NONE
            DwmSetWindowAttribute(
                hwnd as _,
                38, // DWMWA_SYSTEMBACKDROP_TYPE
                &none as *const u32 as *const core::ffi::c_void,
                4,
            );
        }
        disable_dwm_border(hwnd);
    }

    /// SWCA 会重新启用 DWM 边框（且边框跟随 SetWindowRgn 圆角轮廓 → 圆角外白线），必须在其后关掉
    pub fn disable_dwm_border(hwnd: isize) {
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        let color_none: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
        unsafe {
            DwmSetWindowAttribute(
                hwnd as _,
                34, // DWMWA_BORDER_COLOR
                &color_none as *const u32 as *const core::ffi::c_void,
                4,
            );
        }
    }

    /// DWM 系统亚克力底板（DWMSBT_TRANSIENTWINDOW）：DWM 原生绘制、无 SWCA 的 1px 白边；
    /// 明暗由 DWMWA_USE_IMMERSIVE_DARK_MODE 跟随应用主题
    pub fn set_backdrop(hwnd: isize, dark: bool) {
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        unsafe {
            let dark_flag: u32 = if dark { 1 } else { 0 };
            DwmSetWindowAttribute(
                hwnd as _,
                20, // DWMWA_USE_IMMERSIVE_DARK_MODE
                &dark_flag as *const u32 as *const core::ffi::c_void,
                4,
            );
            let backdrop: u32 = 3; // DWMSBT_TRANSIENTWINDOW（亚克力）
            DwmSetWindowAttribute(
                hwnd as _,
                38, // DWMWA_SYSTEMBACKDROP_TYPE
                &backdrop as *const u32 as *const core::ffi::c_void,
                4,
            );
        }
        disable_dwm_border(hwnd);
    }

    /// DWM 系统圆角（DWMWCP_ROUND）：抗锯齿完美、自动跟随窗口尺寸；
    /// SWCA 模糊层与 webview 一起被系统圆角化（SetWindowRgn 硬边裁剪有锯齿，弃用）
    pub fn set_system_round_corner(hwnd: isize) {
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        let pref: u32 = 2; // DWMWCP_ROUND
        unsafe {
            DwmSetWindowAttribute(
                hwnd as _,
                33, // DWMWA_WINDOW_CORNER_PREFERENCE
                &pref as *const u32 as *const core::ffi::c_void,
                4,
            );
        }
        disable_dwm_border(hwnd);
    }

    pub fn hwnd_isize(win: &tauri::Window) -> Option<isize> {
        win.hwnd().ok().map(|h| h.0 as isize)
    }

    pub fn hwnd_isize_wv(win: &tauri::WebviewWindow) -> Option<isize> {
        win.hwnd().ok().map(|h| h.0 as isize)
    }
}

/// dev 诊断：eval 进来的 JS 用它把观测写回诊断日志（可视验证/自动化反馈通道）
#[tauri::command]
fn dev_log(msg: String) {
    libmpv::trace(&format!("JS: {}", msg));
}

#[tauri::command]
fn set_acrylic_tint(
    window: tauri::Window,
    state: State<'_, AppState>,
    light: bool,
) -> Result<(), String> {
    *state.last_light.lock().unwrap() = Some(light);
    #[cfg(target_os = "windows")]
    {
        use winvisual::{hwnd_isize, set_backdrop};
        if let Some(hwnd) = hwnd_isize(&window) {
            set_backdrop(hwnd, !light);
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

        .setup(|app| {
            // VEH 崩溃记录器：任意致命异常（D3D 视频输出崩因定位）写出异常码+故障模块+偏移，
            // 落 %TEMP%/fntv-libmpv.log（lc-082 视频输出专项）
            libmpv::install_crash_logger();
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
            // dev 期开 DevTools 诊断口（9222），供外部 CDP 检查 DOM；正式打包不受影响（debug_assertions）
            #[cfg(debug_assertions)]
            if let Some(wv) = app.get_webview_window("main") {
                let _ = wv.open_devtools();
            }
            // SWCA 动态模糊 + 窗口级圆角（默认浅色 tint；主题切换由前端 set_acrylic_tint 更新）
            #[cfg(target_os = "windows")]
            if let Some(win) = app.get_webview_window("main") {
                use winvisual::{hwnd_isize_wv, set_backdrop, set_system_round_corner};
                if let Some(hwnd) = hwnd_isize_wv(&win) {
                    set_backdrop(hwnd, false); // 默认浅色
                    set_system_round_corner(hwnd);
                }
            }
            // 窗口以 visible:false 创建：延迟到透明化/圆角区域/模糊全部就位后再显示，
            // 消除首帧残留（圆角外露白/旧画面）；show 后再补一次区域（尺寸可能已变）
            let h2 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                if let Some(win) = h2.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            });
            // dev 无人值守复现钩子（lc-082 视频输出崩溃专项）：FNTV_AUTOPLAY_TEST=<guid> 时
            // 启动 5s 后直接走与前端相同的 play_item 真链路（含 bounds 预置），结果写诊断日志。
            #[cfg(debug_assertions)]
            if let Ok(guid) = std::env::var("FNTV_AUTOPLAY_TEST") {
                let h3 = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
                    libmpv::trace("AUTOPLAY: set bounds");
                    match player_set_bounds(h3.clone(), h3.state::<AppState>(), 120.0, 120.0, 960.0, 540.0).await {
                        Ok(_) => libmpv::trace("AUTOPLAY: play_item"),
                        Err(e) => libmpv::trace(&format!("AUTOPLAY bounds err: {}", e)),
                    }
                    match play_item(h3.state::<AppState>(), h3.clone(), guid).await {
                        Ok(_) => libmpv::trace("AUTOPLAY: play_item ok"),
                        Err(e) => libmpv::trace(&format!("AUTOPLAY play_item err: {}", e)),
                    }
                });
            }
            // dev 诊断钩子：FNTV_EVAL_JS=<js> 启动 5s 后在主 webview 执行（配合 AUTOPLAY 做可视验证）
            #[cfg(debug_assertions)]
            if let Ok(js) = std::env::var("FNTV_EVAL_JS") {
                let h4 = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
                    if let Some(win) = h4.get_webview_window("main") {
                        let _ = win.eval(&js);
                        libmpv::trace("EVAL_JS executed");
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_acrylic_tint,
            dev_log,
            bar_show,
            mpv_input,
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
