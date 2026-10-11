//! Fntv-Plus Tauri2 PoC —— 自绘 UI + 直调飞牛 API + mpv 双层窗口内嵌。
//! 命令面向前端：配置 / 登录 / 媒体库 / 播放控制。

mod authx;
mod config;
mod fnapi;
mod mpv;
mod proxy;
mod win_embed;

use config::Config;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
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

    let mut cfg = state.config.read().await.clone();
    cfg.server = server.trim_end_matches('/').to_string();
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
async fn check_session(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    if cfg.token.is_empty() || cfg.server.is_empty() {
        return Err("未登录".into());
    }
    let client = fnapi::new_client();
    client
        .call(&cfg.server, "/v/api/v1/user/info", "GET", &cfg.token, None)
        .await
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

#[tauri::command]
async fn get_items(
    state: State<'_, AppState>,
    parent_guid: String,
    exclude_folder: Option<i64>,
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
                "parent_guid": parent_guid,
                "exclude_folder": exclude_folder.unwrap_or(0),
                "sort_column": "sort_title",
                "sort_type": "ASC"
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
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            login,
            logout,
            check_session,
            get_media_libs,
            get_items,
            get_item_detail,
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
                if let Ok(mut pc) = st.proxy_child.blocking_lock() {
                    if let Some(c) = pc.as_mut() {
                        let _ = c.kill();
                    }
                }
            }
        });
}
