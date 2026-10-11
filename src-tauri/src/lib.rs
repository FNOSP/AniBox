//! Fntv-Plus Tauri2 PoC —— 自绘 UI + 直调飞牛 API + mpv 双层窗口内嵌。
//! 命令面向前端：配置 / 登录 / 媒体库 / 播放控制。

mod authx;
mod config;
mod fnapi;
mod imgproto;
mod mpv;
mod proxy;
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
            let embed = win_embed::create_child(parent_hwnd);
            mpv::MpvSession::launch(app, &cfg.mpv_path, embed).await?
        }
        None => {
            let embed = win_embed::create_child(parent_hwnd);
            mpv::MpvSession::launch(app, &cfg.mpv_path, embed).await?
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
