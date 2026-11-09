//! [lc-2078] 短剧车道：guoapp「红果鉴」Go 核心（duanju_core）FFI 接入。
//!
//! 架构：guoapp 的全部站源逻辑在其 Go c-shared 核心（duanju_core.dll，GPL-3.0，
//! 与本仓库同许可）里，对外的唯一入口是 `DuanjuRequest(JSON) -> JSON` 单调用 +
//! `DuanjuFree` 释放返回串。op 集合见上游 `native/core/app_runtime.go::nativeDispatch`
//! （catalog / categories / detail / resolve / release / …）。
//!
//! 本模块只做三件事：加载 DLL、把 Tauri 命令桥成 FFI 调用、把播放计划（本地流
//! URL + 头）接进 `player::launch`。数据形状（nativeDrama / nativeCategory /
//! Chapter / nativePlan）照抄上游，前端直接消费 JSON，不做中间翻译。
//!
//! 并发：DuanjuRequest 内部由 Go runtime 调度、上游自带锁与取消，可并行调用；
//! 但 initialize 必须先行（核心未就绪时所有 op 返回「应用核心尚未就绪」）。
//! 所有 FFI 调用走 `spawn_blocking`（同步阻塞，不得占用 async 线程）。

use serde_json::{json, Value};
use std::ffi::{c_char, CStr, CString};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use tauri::Manager;

/// 进程内常驻的 DLL 句柄（不卸载；libloading 的 Library 在 Windows 上 Send+Sync）。
static CORE: OnceLock<Mutex<Option<libloading::Library>>> = OnceLock::new();

fn core_slot() -> &'static Mutex<Option<libloading::Library>> {
    CORE.get_or_init(|| Mutex::new(None))
}

static READY: AtomicBool = AtomicBool::new(false);

/// [lc-2097] 请求序号：上游 nativeBeginPlayback 要求节流阀 `sequence` **严格递增**
/// （`sequence <= engine.playbackSequence` 直接返回 context.Canceled）。Dart 客户端用
/// `++_playbackSequence`，本模块同理——每类会进节流阀的 action 各自递增。
/// 键=action 名。
static SEQUENCES: OnceLock<Mutex<std::collections::HashMap<&'static str, i64>>> = OnceLock::new();

fn sequences_slot() -> &'static Mutex<std::collections::HashMap<&'static str, i64>> {
    SEQUENCES.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 取下一个序号（严格递增）。
fn next_sequence(action: &'static str) -> i64 {
    let mut slot = sequences_slot().lock().unwrap_or_else(|e| e.into_inner());
    let cur = slot.entry(action).or_insert(0);
    *cur += 1;
    *cur
}

/// 上一次播放会话（resolve 会开本地流会话；下一次 resolve 前释放旧的，防泄漏）。
static LAST_SESSION: OnceLock<Mutex<String>> = OnceLock::new();

fn last_session_slot() -> &'static Mutex<String> {
    LAST_SESSION.get_or_init(|| Mutex::new(String::new()))
}

/// 借出静态槽里的 DLL 引用执行一次调用。槽锁只保护「取句柄」，不串行化
/// FFI 调用本身（Go 核心内部自带并发安全与取消）。
fn with_core<T>(f: impl FnOnce(&libloading::Library) -> Result<T, String>) -> Result<T, String> {
    let guard = core_slot().lock().unwrap_or_else(|e| e.into_inner());
    let lib = guard
        .as_ref()
        .ok_or_else(|| "短剧核心尚未初始化".to_string())?;
    f(lib)
}

/// 调用核心并解析 envelope `{ok, error|data}`；ok=false 时 Err(error)。
unsafe fn core_call(lib: &libloading::Library, body: &str) -> Result<Value, String> {
    let req = lib
        .get::<libloading::Symbol<unsafe extern "C" fn(*const c_char) -> *mut c_char>>(
            b"DuanjuRequest\0",
        )
        .map_err(|e| format!("短剧核心缺少入口 DuanjuRequest: {e}"))?;
    let input = CString::new(body).map_err(|_| "请求含空字节")?;
    let ptr = req(input.as_ptr());
    if ptr.is_null() {
        return Err("短剧核心没有返回结果".into());
    }
    let out = CStr::from_ptr(ptr).to_string_lossy().into_owned();
    let free = lib
        .get::<libloading::Symbol<unsafe extern "C" fn(*mut c_char)>>(b"DuanjuFree\0")
        .map_err(|e| format!("短剧核心缺少入口 DuanjuFree: {e}"))?;
    free(ptr);
    let v: Value = serde_json::from_str(&out).map_err(|e| format!("短剧核心返回无法解析: {e}"))?;
    if v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) {
        Ok(v.get("data").cloned().unwrap_or(Value::Null))
    } else {
        Err(v
            .get("error")
            .and_then(|x| x.as_str())
            .unwrap_or("短剧核心返回未知错误")
            .to_string())
    }
}

/// 同步 JSON 调用（调用方必须已在 spawn_blocking 里）。
fn json_call(body: &Value) -> Result<Value, String> {
    let text = serde_json::to_string(body).map_err(|e| format!("请求序列化失败: {e}"))?;
    with_core(|lib| unsafe { core_call(lib, &text) })
}

/// 异步 JSON 调用（spawn_blocking 包装）。
async fn call_async(body: Value) -> Result<Value, String> {
    tokio::task::spawn_blocking(move || json_call(&body))
        .await
        .map_err(|e| format!("短剧核心调用任务失败: {e}"))?
}

/// 加载 DLL（幂等）。失败给出可操作的中文错误。
fn ensure_loaded(app: &tauri::AppHandle) -> Result<(), String> {
    {
        let guard = core_slot().lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return Ok(());
        }
    }
    let path = crate::config::duanju_core_path(app).ok_or_else(|| {
        "短剧核心缺失（third_party/duanju/duanju_core.dll）。运行 python tools/fetch-duanju-core.py 补齐"
            .to_string()
    })?;
    let lib = unsafe { libloading::Library::new(&path) }
        .map_err(|e| format!("加载短剧核心失败（{}）: {e}", path.display()))?;
    let mut guard = core_slot().lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(lib);
    }
    Ok(())
}

/// initialize（幂等）。成功后置 READY，之后的调用不再重复 initialize。
async fn ensure_ready(app: &tauri::AppHandle) -> Result<(), String> {
    if READY.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "无法定位数据目录".to_string())?
        .join("duanju-core");
    let dl_dir = data_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| data_dir.clone())
        .join("duanju-downloads");
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = std::fs::create_dir_all(&dl_dir);
    ensure_loaded(app)?;
    let body = json!({
        "action": "initialize",
        "directory": data_dir.to_string_lossy(),
        "downloads": dl_dir.to_string_lossy(),
    });
    call_async(body).await?;
    READY.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}

fn next_call_body(action: &str, mut extra: Value) -> Value {
    if !extra.is_object() {
        extra = json!({});
    }
    let obj = extra.as_object_mut().unwrap();
    obj.insert("action".into(), json!(action));
    extra
}

/// 释放上一次播放会话（有的话）。失败静默——释放只是回收本地流会话。
async fn release_last_session() {
    let old = {
        let mut slot = last_session_slot().lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *slot)
    };
    if old.is_empty() {
        return;
    }
    let mut body = next_call_body("release", json!({ "playbackSession": old }));
    if let Some(o) = body.as_object_mut() {
        o.insert("sequence".into(), json!(next_sequence("playback")));
    }
    let _ = call_async(body).await;
}

fn remember_session(plan: &Value) {
    if let Some(s) = plan.get("session").and_then(|x| x.as_str()) {
        let mut slot = last_session_slot().lock().unwrap_or_else(|e| e.into_inner());
        *slot = s.to_string();
    }
}

// ───────────────────────── Tauri 命令 ─────────────────────────

/// 核心信息（前端短剧页挂载时调）：初始化核心并回版本/站源可用性。
#[tauri::command]
pub async fn duanju_init(app: tauri::AppHandle) -> Result<Value, String> {
    ensure_ready(&app).await?;
    let info = call_async(json!({ "action": "checkUpdate", "version": "" }))
        .await
        .unwrap_or(Value::Null);
    Ok(json!({ "ready": true, "info": info }))
}

/// 站源分类列表。
#[tauri::command]
pub async fn duanju_categories(app: tauri::AppHandle, source: String) -> Result<Value, String> {
    ensure_ready(&app).await?;
    call_async(next_call_body("categories", json!({ "source": source, "force": false }))).await
}

/// 目录/搜索。query 非空 = 搜索；category 空 = 全部。
#[tauri::command]
pub async fn duanju_catalog(
    app: tauri::AppHandle,
    source: String,
    category: Option<String>,
    page: i64,
    query: Option<String>,
) -> Result<Value, String> {
    ensure_ready(&app).await?;
    call_async(next_call_body(
        "catalog",
        json!({
            "source": source,
            "category": category.unwrap_or_default(),
            "page": page.clamp(1, 999),
            "query": query.unwrap_or_default(),
            "force": false,
        }),
    ))
    .await
}

/// 详情（含全部分集）。drama = 目录项原样回传（nativeDrama JSON）。
#[tauri::command]
pub async fn duanju_detail(app: tauri::AppHandle, drama: Value) -> Result<Value, String> {
    ensure_ready(&app).await?;
    call_async(next_call_body("detail", json!({ "drama": drama }))).await
}

/// 解析播放计划（本地流 URL + 头）。index 从 0 起（上游 1 起，这里 +1 传入）。
#[tauri::command]
pub async fn duanju_resolve(
    app: tauri::AppHandle,
    drama: Value,
    chapter: Value,
    index: i64,
    quality: Option<i64>,
) -> Result<Value, String> {
    ensure_ready(&app).await?;
    release_last_session().await;
    // [lc-2097] sequence 必须**严格递增**（上游节流阀：seq <= 上次 → 立即 context.Canceled）
    let mut body = next_call_body(
        "resolve",
        json!({
            "drama": drama,
            "chapter": chapter,
            "index": index + 1,
            "quality": quality.unwrap_or(0),
            "force": false,
        }),
    );
    if let Some(o) = body.as_object_mut() {
        o.insert("sequence".into(), json!(next_sequence("playback")));
    }
    let plan = call_async(body).await?;
    remember_session(&plan);
    Ok(plan)
}

/// 释放播放会话（播放器关闭时前端可选调用）。
#[tauri::command]
pub async fn duanju_release(_app: tauri::AppHandle, session: String) -> Result<(), String> {
    {
        let mut slot = last_session_slot().lock().unwrap_or_else(|e| e.into_inner());
        slot.clear();
    }
    if session.is_empty() {
        return Ok(());
    }
    let mut body = next_call_body("release", json!({ "playbackSession": session }));
    if let Some(o) = body.as_object_mut() {
        o.insert("sequence".into(), json!(next_sequence("playback")));
    }
    call_async(body).await?;
    Ok(())
}

/// 播放：resolve → player::launch（本地流 URL + Referer/UA 头）。
#[tauri::command]
pub async fn duanju_play(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    drama: Value,
    chapter: Value,
    index: i64,
    quality: Option<i64>,
    title: Option<String>,
) -> Result<Value, String> {
    let plan = duanju_resolve(app.clone(), drama, chapter, index, quality).await?;
    let url = plan
        .get("url")
        .and_then(|x| x.as_str())
        .filter(|x| !x.is_empty())
        .ok_or_else(|| "短剧核心未返回播放地址".to_string())?
        .to_string();
    let headers = plan.get("headers").cloned().unwrap_or(json!({}));
    let referer = headers.get("Referer").and_then(|x| x.as_str()).map(String::from);
    let ua = headers
        .get("User-Agent")
        .and_then(|x| x.as_str())
        .map(String::from);
    let extra: Vec<(String, String)> = headers
        .as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| {
                    !k.eq_ignore_ascii_case("referer") && !k.eq_ignore_ascii_case("user-agent")
                })
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let cfg = state.config.read().await.clone();
    let display = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "短剧".to_string());
    let http_headers = crate::player::HttpHeaders::new(referer, ua);
    // extra 逐条塞进 HttpHeaders（referer/ua 已单列）
    let mut http_headers = http_headers;
    for (k, v) in extra {
        if k.eq_ignore_ascii_case("referer") || k.eq_ignore_ascii_case("user-agent") {
            continue;
        }
        http_headers.extra.push((k, v));
    }
    crate::player::launch(
        &app,
        &state,
        &cfg,
        &url,
        &display,
        0.0,
        0.0,
        Some(&http_headers),
        &[],
        None,
        true,
    )
    .await?;
    Ok(json!({ "url": url, "session": plan.get("session").cloned().unwrap_or(json!("")) }))
}
