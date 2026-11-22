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
/// [lc-2295] 用 Arc 持有：调用方**取出引用后立刻放锁**，FFI 调用本身不持锁 ——
/// 详见 `with_core` 的注释（此前正是这里把红果的详情请求串行到了超时）。
static CORE: OnceLock<Mutex<Option<std::sync::Arc<libloading::Library>>>> = OnceLock::new();

fn core_slot() -> &'static Mutex<Option<std::sync::Arc<libloading::Library>>> {
    CORE.get_or_init(|| Mutex::new(None))
}

static READY: AtomicBool = AtomicBool::new(false);

/// [lc-2180] initialize 响应里的站源能力清单（guoapp v2 架构：站源 = JS 模块，
/// 可用集合随随包模块而定，不再是核心写死）。前端据此动态渲染站源 chips。
static SOURCES: OnceLock<Value> = OnceLock::new();

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

/// [lc-2116] 连播队列：记录「本剧 + 全部分集 + 当前播放索引」。
/// end-file(eof) 时据此自动解析并播放下一集 —— 短剧一集只有 1~3 分钟，用户不该每集手动点。
/// 只在 duanju_play 成功起播后写入；其他车道（飞牛/IPTV/云盘）播放时不置位，
/// 因此不会误续播。
struct AutoNext {
    drama: Value,
    chapters: Vec<Value>,
    index: i64,
}

static AUTO_NEXT: OnceLock<Mutex<Option<AutoNext>>> = OnceLock::new();

/// [lc-2123] 「短剧会话活跃」标记：time-pos 观察回调只在它为 true 时写短剧历史。
/// 比只依赖 AutoNext 更严格 —— AutoNext 可能在用户停止后残留，而这个标记由
/// play_impl 显式置位、由 clear_auto_next（停止/其他车道起播）清除。
static DJ_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// [lc-2132] 连播重入锁：同一时刻只允许**一个**连播流程在飞。
///
/// 不加会无限循环：`eof-reached` 在 loadfile 完成后到下一集真正开始播放之前**仍为 true**
/// （mpv 只在开始播放新文件后才把它复位），期间属性可能多次刷新 → 每次都触发
/// `auto_play_next` → 疯狂推进集数（实测刷屏几十次「已自动播放下一集」，画面乱跳）。
static AUTO_NEXT_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn auto_next_slot() -> &'static Mutex<Option<AutoNext>> {
    AUTO_NEXT.get_or_init(|| Mutex::new(None))
}

/// 上一次播放会话（resolve 会开本地流会话；下一次 resolve 前释放旧的，防泄漏）。
static LAST_SESSION: OnceLock<Mutex<String>> = OnceLock::new();

fn last_session_slot() -> &'static Mutex<String> {
    LAST_SESSION.get_or_init(|| Mutex::new(String::new()))
}

/// 借出 DLL 引用执行一次调用。
///
/// [lc-2295] **槽锁只保护「取句柄」，FFI 调用本身绝不持锁**。
/// 这里曾经写成 `let guard = …lock(); f(lib)` —— guard 活到函数结束，等于把**全部**
/// 核心调用串行化在一把全局锁上。短剧页一挂载就会并发打十几个命令（站源健康探测、
/// 分类、目录、详情…），而探测里死源要 26~60s 才超时：详情请求排在后面，
/// 12s 前端兜底一到就报「详情加载超时」。实测同一部红果剧直连核心 `detail` 只要
/// 0.22~0.40s（连测 5 部全部成功），慢的从来不是核心。
/// 现在先把 `Arc<Library>` 克隆出来再放锁，Go 核心内部自带并发安全与按 session 取消。
fn with_core<T>(f: impl FnOnce(&libloading::Library) -> Result<T, String>) -> Result<T, String> {
    let lib = {
        let guard = core_slot().lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .ok_or_else(|| "短剧核心尚未初始化".to_string())?
            .clone()
        // guard 在此处 drop：FFI 调用在锁外进行
    };
    f(&lib)
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
        *guard = Some(std::sync::Arc::new(lib));
    }
    Ok(())
}

/// [lc-2234] initialize 串行锁：短剧页挂载时会**并发**打十几个命令（sources 清单、
/// 健康探测、账号状态…），它们同时看到 READY=false 就各自跑一遍 initialize +
/// deploy_modules —— 每次 initialize 都要扫一遍数据目录，并发起来拖慢首屏，
/// 表现就是「一直显示加载中」。用异步锁保证只跑一次，其余等结果。
static INIT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// initialize（幂等）。成功后置 READY，之后的调用不再重复 initialize。
async fn ensure_ready(app: &tauri::AppHandle) -> Result<(), String> {
    if READY.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    // 抢到锁后再查一次 READY（等锁期间别人可能已经初始化完）
    let _guard = INIT_LOCK.lock().await;
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
    // [lc-2180] guoapp v2 核心的 initialize 需要独立缓存目录（封面/剧库/榜单缓存，
    // 上游 Dart 侧传 getApplicationCacheDirectory）。
    let cache_dir = data_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| data_dir.clone())
        .join("duanju-core-cache");
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = std::fs::create_dir_all(&dl_dir);
    let _ = std::fs::create_dir_all(&cache_dir);
    // [lc-2180] v2 核心只从数据目录 modules/ 加载 JS 站源，先按版本部署随包模块。
    deploy_modules(app, &data_dir);
    ensure_loaded(app)?;
    let body = json!({
        "action": "initialize",
        "directory": data_dir.to_string_lossy(),
        "cache": cache_dir.to_string_lossy(),
        "downloads": dl_dir.to_string_lossy(),
    });
    let resp = call_async(body).await?;
    // 记下站源清单（id/name/available/adult/…）。call_async 已解包 envelope（成功时
    // 返回 data 本体），sources 就在顶层；初始化失败时不置 READY，下次再试。
    if let Some(sources) = resp.get("sources").cloned() {
        let _ = SOURCES.set(sources);
    }
    READY.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// [lc-2180] 把随包站源模块（third_party/duanju/modules/<id>/…）部署进核心数据目录
/// `modules/<id>/`。guoapp v2 核心只从数据目录的 modules/ 加载 JS 站源；随包目录是
/// 只读资源，因此按 manifest 版本择优复制：
///   · 数据目录没有该模块 → 复制随包版；
///   · 随包版本更高 → 整目录替换（红果取流接口适配随模块版本走，必须跟新）；
///   · 数据目录版本相同/更高 → 不动（不降级）。
fn deploy_modules(app: &tauri::AppHandle, data_dir: &std::path::Path) {
    let Some(src_root) = crate::config::duanju_modules_dir(app) else {
        return;
    };
    let dst_root = data_dir.join("modules");
    let Ok(entries) = std::fs::read_dir(&src_root) else {
        return;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if !path.is_dir() {
            continue;
        }
        let id = ent.file_name().to_string_lossy().into_owned();
        let bundled = read_module_version(&path);
        let installed = read_module_version(&dst_root.join(&id));
        if version_le(&bundled, &installed) {
            continue;
        }
        let dst = dst_root.join(&id);
        let _ = std::fs::remove_dir_all(&dst);
        copy_tree(&path, &dst);
        crate::libmpv::trace(&format!(
            "[duanju] 站源模块 {id} 部署 {} -> {}",
            bundled, installed
        ));
    }
}

/// 读模块 manifest 的 version 字段（缺文件/非法 JSON 返回空串 = 视为最低版本）。
fn read_module_version(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|m| m.get("version").and_then(|v| v.as_str()).map(str::to_owned))
        .unwrap_or_default()
}

/// 版本比较（guoapp 模块版本形如 1.0.2；不可解析的段按 0，空串即 0.0.0）。
fn version_le(a: &str, b: &str) -> bool {
    parse_ver(a) <= parse_ver(b)
}

fn parse_ver(v: &str) -> (u64, u64, u64) {
    let mut out = [0u64; 3];
    for (i, seg) in v.split('.').take(3).enumerate() {
        out[i] = seg.trim().parse().unwrap_or(0);
    }
    (out[0], out[1], out[2])
}

/// 递归复制目录（模块现只有 manifest.json + main.js，写通用以便上游日后加文件）。
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    if !src.is_dir() {
        return;
    }
    let _ = std::fs::create_dir_all(dst);
    let Ok(entries) = std::fs::read_dir(src) else {
        return;
    };
    for ent in entries.flatten() {
        let from = ent.path();
        let to = dst.join(ent.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else if let Err(e) = std::fs::copy(&from, &to) {
            crate::libmpv::trace(&format!("[duanju] 模块文件 {} 复制失败: {e}", from.display()));
        }
    }
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

/// 核心信息（前端短剧页挂载时调）：初始化核心并回版本/站源能力。
/// [lc-2180] sources = initialize 响应的站源清单（id/name/available/adult/…），
/// 前端据此渲染站源 chips（guoapp v2 起站源集合随随包模块变化，不能写死）。
#[tauri::command]
pub async fn duanju_init(app: tauri::AppHandle) -> Result<Value, String> {
    ensure_ready(&app).await?;
    let info = call_async(json!({ "action": "checkUpdate", "version": "" }))
        .await
        .unwrap_or(Value::Null);
    Ok(json!({
        "ready": true,
        "info": info,
        "sources": SOURCES.get().cloned().unwrap_or(Value::Null),
    }))
}

/// 站源分类列表。
#[tauri::command]
pub async fn duanju_categories(app: tauri::AppHandle, source: String) -> Result<Value, String> {
    ensure_ready(&app).await?;
    call_async(next_call_body("categories", json!({ "source": source, "force": false }))).await
}

/// 目录/搜索。query 非空 = 搜索；category 空 = 全部。
/// [lc-2234] force=true 跳过核心缓存强制重拉（用户在界面上点「刷新」时用）。
#[tauri::command]
pub async fn duanju_catalog(
    app: tauri::AppHandle,
    source: String,
    category: Option<String>,
    page: i64,
    query: Option<String>,
    force: Option<bool>,
) -> Result<Value, String> {
    ensure_ready(&app).await?;
    call_async(next_call_body(
        "catalog",
        json!({
            "source": source,
            "category": category.unwrap_or_default(),
            "page": page.clamp(1, 999),
            "query": query.unwrap_or_default(),
            "force": force.unwrap_or(false),
        }),
    ))
    .await
}

/// [lc-2234] 只读核心的本地目录缓存（`cached` op，**纯内存/磁盘，不打网络**）。
/// 前端用「先显示缓存 → 后台再刷新」消除每次进短剧页的等待：
///   · Fresh=true  → 缓存 <15 分钟，直接用，无需刷新；
///   · Fresh=false → 有旧数据但已过期，先渲染出来，再后台 catalog 刷新。
#[tauri::command]
pub async fn duanju_cached(
    app: tauri::AppHandle,
    source: String,
    category: Option<String>,
) -> Result<Value, String> {
    ensure_ready(&app).await?;
    call_async(next_call_body(
        "cached",
        json!({ "source": source, "category": category.unwrap_or_default() }),
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

// ───────────────────────── 青空（sorani）站源账号 ─────────────────────────
// [lc-2181] 青空模块播放必须登录（accessToken），上游在「设置 › 站源 › 站源账号」里
// 走 账号密码 → 滑块拼图验证（AJ-Captcha）→ login 的流程，会话持久化在核心数据目录。
// op 协议照抄上游 Dart 仓库层（core_bridge.dart 的 soraniAccount 系列）：
//   {"action":"soraniAccount"}                              → {loggedIn, name}
//   {"action":"soraniAccount","command":"captcha"}          → {id, backgroundImage,
//                                                 templateImage(dataURI), 宽高×2}
//   {"action":"soraniAccount","command":"login","account":
//      {identifier, password, captchaId, track:{bgImageWidth,bgImageHeight,
//       templateImageWidth,templateImageHeight,startTime,stopTime,trackList}}} → 同 status
//   {"action":"soraniAccount","command":"logout"}           → 同 status
// 账号 op 不进播放节流阀，无需 sequence。

/// 账号状态（loggedIn/name）。
#[tauri::command]
pub async fn duanju_account_status(app: tauri::AppHandle) -> Result<Value, String> {
    ensure_ready(&app).await?;
    call_async(json!({ "action": "soraniAccount" })).await
}

/// 滑块验证码（背景图 + 拼图块，data URI 可直接给 <img>）。
#[tauri::command]
pub async fn duanju_captcha(app: tauri::AppHandle) -> Result<Value, String> {
    ensure_ready(&app).await?;
    call_async(json!({ "action": "soraniAccount", "command": "captcha" })).await
}

/// 登录（identifier=用户名或邮箱；track=滑块轨迹，形状照抄上游 SoraniCaptchaTrack）。
#[tauri::command]
pub async fn duanju_login(
    app: tauri::AppHandle,
    identifier: String,
    password: String,
    captcha_id: String,
    track: Value,
) -> Result<Value, String> {
    ensure_ready(&app).await?;
    call_async(json!({
        "action": "soraniAccount",
        "command": "login",
        "account": {
            "identifier": identifier,
            "password": password,
            "captchaId": captcha_id,
            "track": track,
        },
    }))
    .await
}

/// 退出登录。
#[tauri::command]
pub async fn duanju_logout(app: tauri::AppHandle) -> Result<Value, String> {
    ensure_ready(&app).await?;
    call_async(json!({ "action": "soraniAccount", "command": "logout" })).await
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
    // [lc-2116] 全部分集（前端详情页已有）：写入连播队列，播完自动接下一集。
    // 缺省为空 = 不连播（只有一集或调用方未提供）。
    chapters: Option<Vec<Value>>,
) -> Result<Value, String> {
    play_impl(&app, &state, drama, chapter, index, quality, title, chapters).await
}

/// [lc-2116] 播放主体（与命令分离，供连播复用；不再依赖 tauri::State 类型）。
async fn play_impl(
    app: &tauri::AppHandle,
    state: &crate::AppState,
    drama: Value,
    chapter: Value,
    index: i64,
    quality: Option<i64>,
    title: Option<String>,
    chapters: Option<Vec<Value>>,
) -> Result<Value, String> {
    // [lc-2238] chapter 后面拉弹幕还要用（取 episodeId/source），不能 move 进 resolve
    let plan = duanju_resolve(app.clone(), drama.clone(), chapter.clone(), index, quality).await?;
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
    let mut http_headers = crate::player::HttpHeaders::new(referer, ua);
    // [lc-2101] 短剧 = 竖屏片源：播放器窗口按视频真实比例自适应（零黑边）
    http_headers.vertical = true;
    // extra 逐条塞进 HttpHeaders（referer/ua 已单列）
    for (k, v) in extra {
        if k.eq_ignore_ascii_case("referer") || k.eq_ignore_ascii_case("user-agent") {
            continue;
        }
        http_headers.extra.push((k, v));
    }
    // [lc-2100] CENC 解密：红果 HEVC 流是加密的，核心返回 `decryptionKey` 时必须交给
    // mpv 的 lavf demuxer（`decryption_key=<hex>`，并放开 crypto 协议白名单），否则
    // mpv 解出乱码 NAL（"Invalid NAL unit size"）→ 黑屏、进度直接到末尾。
    // Dart 客户端同口径（playback_session.dart 的 demuxer-lavf-o）。
    if let Some(key) = plan
        .get("decryptionKey")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        http_headers.mpv_opts.push(format!(
            "--demuxer-lavf-o=decryption_key={key},protocol_whitelist=[http,https,tcp,tls,crypto,data,file]"
        ));
        crate::libmpv::trace(&format!("[duanju] CENC 解密密钥已下发（{} 位 hex）", key.len()));
    }
    // [lc-2129] 用户要求**还原底栏中间按钮**（上一轮关掉 ±10 秒键导致布局错位）。
    // 短剧改为：播完在左下角显示「下一集」大按钮（见 fntv_next_ep.lua），不再自动切集。
    set_modernz_jump_buttons(app, true);
    // [lc-2238] 弹幕：resolve 已建好播放会话 → 按本集 episodeId 拉弹幕并生成 ASS。
    // 走缓存优先（同集重看/连播回头都不重复请求）；**拉取失败不影响播放**，只记日志。
    // 必须在本集 launch **之前**完成，才能随命令行一起起播。
    //
    // 轨道口径：短剧是核心本地流（`127.0.0.1`），没有 NAS/外挂字幕通道，主字幕轨是空的，
    // 所以弹幕直接走主轨。**不能动 secondary-sid** ——随包 uosc_danmaku 脚本事后自己
    // sub-add 并管理副字幕轨，我们再设一次会与它抢位、出现双份弹幕。
    //
    // 耗时：实测首拉 2.1s / 8 页 / 531 条（缓存命中 0ms）。为不让起播被慢站源拖住，
    // 整体加 8s 上限 —— 超时就先起播、弹幕下次再看（缓存会保留已拉到的部分）。
    let danmaku_path = match tokio::time::timeout(
        std::time::Duration::from_secs(8),
        prepare_danmaku(app, &plan, &chapter),
    )
    .await
    {
        Ok(p) => p,
        Err(_) => {
            crate::libmpv::trace("[duanju] 弹幕拉取超时（8s），本集先不挂弹幕");
            None
        }
    };
    if let Some(ass_path) = &danmaku_path {
        let p = ass_path.to_string_lossy().into_owned();
        http_headers.mpv_opts.push(format!("--sub-files={p}"));
        crate::libmpv::trace(&format!("[duanju] 弹幕已挂载：{p}"));
    }
    // [lc-2241] 倍速持久化：短剧必须自己算剧级键 ——
    // `fnos_progress::current_speed_key()` 只在飞牛播放链路（lib.rs::play_item）里被设置，
    // 短剧走的是核心本地流（`http://127.0.0.1:端口/…`），`parse_item_guid` 解析不出 guid，
    // 于是 speed_key 恒为空串 → `playback_speed::speed_for` 直接返回 None →
    // 每次起播都被 mpv.rs 套成 1.0（用户报「每次重置成 1 倍速」）。
    // 这里按「剧」登记：同一部剧的所有集共用一个键，切集/连播沿用，换剧各用各的。
    // 必须在 launch 之前设，mpv.rs 收到 path 事件时才会立即按它查记录并 set speed。
    {
        let series = drama
            .get("id")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|id| format!("series:{id}"));
        if let Some(key) = series {
            let item = chapter
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or_default();
            crate::fnos_progress::begin_with_speed_key("", item, &key);
        }
    }
    let t0 = std::time::Instant::now();
    crate::libmpv::trace("[duanju] → player::launch 开始");    crate::player::launch(
        app,
        state,
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
    .await;
    // [lc-2131] 诊断：launch 是连播链路里最可疑的阻塞点（复用会话时要发一串等回包的
    // IPC 命令；mpv 处于 eof/keep-open 状态时命令队列可能很慢，实测隔了 45s 才返回）。
    crate::libmpv::trace(&format!(
        "[duanju] launch 返回 ok（总耗时 {:?}）",
        t0.elapsed()
    ));
    // [lc-2104] 竖屏调窗改为**主动执行**（不依赖 file-loaded 事件回调）：
    // 事件方案有两个坑——① 会话复用时事件可能已消费过；② 调窗分支依赖会话内的原子
    // 标志，旧会话（升级前启动）标志不存在。这里在 launch 返回后轮询 video-params，
    // 就绪即按「锁高、宽自适应」设窗口几何，任何会话状态都成立。
    // [lc-2132] 竖屏调窗**放到后台任务**，不阻塞 play_impl 返回。
    // 原先 `wait_video_size` 最多轮询 6s（每条 get_property 都要等回包），连播时这段时间
    // 会叠加在 auto_play_next 里 → 前端迟迟收不到「已起播」，用户以为没反应。
    // 调窗是纯视觉优化，晚几秒无感；失败也不影响播放。
    if let Some(sess) = {
        let slot = state.mpv.lock().await;
        slot.as_ref().cloned()
    } {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Some((vw, vh)) = wait_video_size(&sess).await {
                apply_vertical_geometry(&app2, &sess, vw, vh).await;
            }
        });
    }
    // [lc-2116] 记录连播队列。**语义关键**：
    //   · Some(list) = 用户手动点播（前端总会传全部分集）→ 重建队列；
    //   · None       = 连播内部调用（auto_play_next 自己已经推进了 index）→ **不动队列**，
    //                  否则会把 index 重置回当前值、或把队列清空导致只连播一集。
    if let Some(list) = chapters {
        let mut slot = auto_next_slot().lock().unwrap_or_else(|e| e.into_inner());
        *slot = if list.len() > 1 {
            Some(AutoNext {
                drama: drama.clone(),
                chapters: list,
                index,
            })
        } else {
            None
        };
    }    // [lc-2142] 告诉随包 Lua 有没有下一集：写 next-episode = 下一集标题 / 空串。
    // `fntv_next_hint.lua` 据此在**播完时**于控制栏中间播放键上方浮出提示，
    // 告诉用户「点这个按钮看下一集」——不新增任何按钮，只做指向性提示。
    {
        let label = {
            let slot = auto_next_slot().lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_ref() {
                Some(q) if q.index + 1 < q.chapters.len() as i64 => q.chapters
                    [(q.index + 1) as usize]
                    .get("title")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                _ => String::new(),
            }
        };
        if let Some(sess) = {
            let slot = state.mpv.lock().await;
            slot.as_ref().cloned()
        } {
            let _ = sess
                .command_no_reply(json!(["set", "user-data/anibox/next-episode", label]))
                .await;
        }
    }
    // [lc-2123] 标记短剧会话活跃（time-pos 观察回调据此写历史）
    DJ_ACTIVE.store(true, std::sync::atomic::Ordering::Release);
    // [lc-2123] 起播即记一条观看记录（独立存储，不走软件主历史）。
    // pos 从这里开始为 0，实际进度由前端在播放中/退出时回写（duanju_play 返回后前端
    // 拿不到 mpv 的 time-pos，所以「继续观看」的精度由前端定时回写保证）。
    record_history(app, &drama, index, 0.0, 0.0);
    Ok(json!({
        "url": url,
        "session": plan.get("session").cloned().unwrap_or(json!("")),
        // [lc-2238] 弹幕是否已挂载（前端据此提示「本集暂无弹幕」）
        "danmaku": danmaku_path.is_some(),
    }))
}

/// [lc-2116] 播完自动接下一集（mpv end-file reason=eof 时由 mpv.rs 调用）。
///
/// 设计要点：
/// · **按需解析**：只在切到那一集时才 resolve —— 短剧动辄 70 集，全部预解析等于 70 次
///   网络往返，且核心的本地流会话是单例，预解析会互相顶掉；
/// · 静态槽 `Option<AutoNext>` 只在 duanju_play 成功起播后置位，其他车道播放时为空 ⇒
///   不会误续播；
/// · 末集：清空队列并返回 Ok(false)，不循环不重播；
/// · 解析/起播失败：清空队列（不无限重试，避免坏源反复拉流）。
pub async fn auto_play_next(
    app: &tauri::AppHandle,
    state: &crate::AppState,
) -> Result<bool, String> {
    // [lc-2132] 重入保护：拿不到锁说明上一集的连播还在进行中，直接忽略这次触发。
    // compare_exchange 保证原子性（不能用 load+store，两个线程会同时通过）。
    if std::sync::atomic::AtomicBool::compare_exchange(
        &AUTO_NEXT_BUSY,
        false,
        true,
        std::sync::atomic::Ordering::AcqRel,
        std::sync::atomic::Ordering::Acquire,
    )
    .is_err()
    {
        crate::libmpv::trace("[duanju] 连播进行中，忽略重复触发");
        return Ok(false);
    }
    // 释放由 _guard 兜底（任何 return 路径都会解锁）
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            AUTO_NEXT_BUSY.store(false, std::sync::atomic::Ordering::Release);
        }
    }
    let _guard = Guard;

    let Some((drama, chapter, index)) = ({
        let mut slot = auto_next_slot().lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_mut() {
            Some(q) if q.index + 1 < q.chapters.len() as i64 => {
                q.index += 1;
                Some((q.drama.clone(), q.chapters[q.index as usize].clone(), q.index))
            }
            Some(_) => {
                *slot = None; // 已到末集：播完即停
                None
            }
            None => None,
        }
    }) else {
        return Ok(false);
    };

    // [lc-2238] 标题必须**剧名 + 集名**（用户报「标题名称变成集数了」）。
    // 原实现优先用 chapter.title（=「第2集」），剧名只在 chapter.title 缺失时才出现 →
    // 自动连播后窗口标题/OSD 只剩集数。改为剧名在前、集名在后，缺剧名才退回集名。
    let drama_title = drama
        .get("title")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let chapter_title = chapter
        .get("title")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("第{}集", index + 1));
    let title = Some(match drama_title {
        Some(dt) => format!("{dt} {chapter_title}"),
        None => chapter_title,
    });
    // 注意：连播时不再重写队列（chapters 传 None），否则会把 index 重置
    let r = play_impl(app, state, drama, chapter, index, None, title, None).await;
    if r.is_err() {
        *auto_next_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    r.map(|_| true)
}

// ───────────────────────── [lc-2238] 弹幕 ─────────────────────────

/// 拉本集弹幕并写成 ASS，返回可交给 mpv 的路径；无弹幕/不支持/失败都返回 None。
///
/// 会话来源：`plan.session`（resolve 刚建立）。核心把弹幕绑定在这个播放会话上，
/// 所以**必须在 resolve 之后、release 之前**调用。
async fn prepare_danmaku(
    app: &tauri::AppHandle,
    plan: &Value,
    chapter: &Value,
) -> Option<std::path::PathBuf> {
    let session = plan.get("session").and_then(|x| x.as_str())?.to_string();
    let episode_id = chapter.get("id").and_then(|x| x.as_str())?.to_string();
    let source = chapter
        .get("source")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .or_else(|| episode_id.split(':').next().map(str::to_string))
        .unwrap_or_else(|| "unknown".into());
    let cache_root = app
        .path()
        .app_data_dir()
        .ok()?
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("duanju-core-cache");
    let path = crate::duanju_danmaku::cache_path(&cache_root, &source, &episode_id);
    // 1) 缓存命中直接用（同集重看不再请求；AS S 里已含全量弹幕）
    if crate::duanju_danmaku::read_cache(&path).is_some() {
        return Some(path);
    }
    // 2) 分页拉取。实测语义（探针确认，勿凭直觉改）：
    //    · 每页**最多返回 90 条**（大窗口也是一样），`nextMs` = 下一页起点，
    //      不是窗口边界 —— 传 `startMs=0,durationMs=600000` 只回前 90 条、nextMs=30000；
    //    · `total` 是**整季**总数（6884），不能拿来当单集终止条件；
    //    · 短剧单集实际只有 2~3 分钟，弹幕时间戳最大值约 180s。
    //    终止条件：nextMs 不再前进 / 连续两页没有新条目（越过本集内容）/ 到达硬上限。
    let mut items: Vec<crate::duanju_danmaku::DanmakuItem> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut start: i64 = 0;
    let mut total: i64 = 0;
    let mut pages = 0usize;
    let mut dry = 0usize; // 连续没有新条目的页数
    let mut max_t: i64 = 0; // 已见弹幕的最大时间戳（用于判断越过本集）
    loop {
        if pages >= 40 || dry >= 2 {
            break;
        }
        pages += 1;
        let seq = next_sequence("danmaku");
        let body = json!({
            "action": "danmaku",
            "playbackSession": session,
            "session": session,
            "sequence": seq,
            "startMs": start,
            "durationMs": start + 30_000,
        });
        let page = match call_async(body).await {
            Ok(v) => v,
            Err(e) => {
                crate::libmpv::trace(&format!("[duanju] 弹幕拉取停止（第 {pages} 页）：{e}"));
                break;
            }
        };
        let (got, next, tot) = crate::duanju_danmaku::parse_page(&page);
        if tot > 0 {
            total = tot;
        }
        let before = items.len();
        for it in got {
            max_t = max_t.max(it.time_ms);
            // 去重键：时间+文本（分页边界条目可能重复）
            let key = (it.time_ms, it.text.clone());
            if seen.insert(key) {
                items.push(it);
            }
        }
        dry = if items.len() == before { dry + 1 } else { 0 };
        match next {
            Some(n) if n > start => start = n,
            _ => break,
        }
    }
    if items.is_empty() {
        crate::libmpv::trace(&format!(
            "[duanju] 本集无弹幕（{episode_id}，全季 total={total}，查了 {pages} 页）"
        ));
        return None;
    }
    items.sort_by_key(|x| x.time_ms);
    crate::libmpv::trace(&format!(
        "[duanju] 弹幕就绪：{episode_id} {} 条（全季 total={total}，{pages} 页，末条 {}s）",
        items.len(),
        max_t / 1000
    ));
    // 分辨率只影响滚动轨道布局，1080x1920 是竖屏短剧的通用基准，mpv 会自适应缩放
    let ass = crate::duanju_danmaku::build_ass(&items, 1080, 1920);
    if let Err(e) = crate::duanju_danmaku::write_cache(&path, &ass) {
        crate::libmpv::trace(&format!("[duanju] 弹幕缓存写入失败（不影响播放）：{e}"));
    }
    crate::libmpv::trace(&format!(
        "[duanju] 弹幕就绪：{episode_id} 共 {} 条（total={total}，{pages} 页）",
        items.len()
    ));
    Some(path)
}

/// 手动拉取弹幕（前端/调试用）：只返回统计，ASS 路径由 prepare_danmaku 负责。
#[tauri::command]
pub async fn duanju_danmaku_info(
    app: tauri::AppHandle,
    plan: Value,
    chapter: Value,
) -> Result<Value, String> {
    ensure_ready(&app).await?;
    let p = prepare_danmaku(&app, &plan, &chapter).await;
    Ok(json!({
        "hasDanmaku": p.is_some(),
        "assPath": p.map(|x| x.to_string_lossy().into_owned()),
    }))
}

/// [lc-2123] 取当前短剧会话的（剧摘要, 集索引）——mpv 的 time-pos 观察回调用它写进度。
/// 非短剧播放时返回 None。**只读**，不改队列。
pub fn current_playing() -> Option<(Value, i64)> {
    if !DJ_ACTIVE.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    let slot = auto_next_slot().lock().unwrap_or_else(|e| e.into_inner());
    slot.as_ref().map(|q| (q.drama.clone(), q.index))
}

/// [lc-2128] 设置 modernz 底栏的 ±10 秒跳转键显隐（短剧关、其它开）。
///
/// 直接改 `script-opts/modernz.conf` 的单行值：modernz 的 `opt.read_options` 会监听
/// 配置文件变化并在回调里 `request_tick()` 重绘，所以运行中的 mpv 也会即时生效
/// （无需重启会话）。文件里没有该键就追加一行。
fn set_modernz_jump_buttons(app: &tauri::AppHandle, on: bool) {
    const NL: char = '\u{0a}';
    let Some(base) = crate::config::mpv_portable_config_dir(app) else {
        return;
    };
    let path = base.join("script-opts").join("modernz.conf");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return; // 配置文件不存在（非自带 mpv）：不处理
    };
    let want = format!("jump_buttons={}", if on { "yes" } else { "no" });
    let mut out = String::with_capacity(text.len() + 32);
    let mut replaced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("jump_buttons=") {
            out.push_str(&want);
            out.push(NL);
            replaced = true;
        } else {
            out.push_str(line);
            out.push(NL);
        }
    }
    if !replaced {
        out.push_str(&want);
        out.push(NL);
    }
    if out != text {
        let _ = std::fs::write(&path, out);
        crate::libmpv::trace(&format!("[duanju] modernz {want}"));
    }
}

/// [lc-2127] 队列里是否还有下一集（mpv 读线程判断要不要发 `duanju-eof` 事件）。
pub fn has_next() -> bool {
    if !DJ_ACTIVE.load(std::sync::atomic::Ordering::Acquire) {
        return false;
    }
    let slot = auto_next_slot().lock().unwrap_or_else(|e| e.into_inner());
    slot.as_ref()
        .map(|q| q.index + 1 < q.chapters.len() as i64)
        .unwrap_or(false)
}

/// [lc-2127] 前端收到 `duanju-eof` 事件后调用：在**命令线程**里起播下一集。
///
/// 为什么绕一圈让前端发起：mpv 的 `eof-reached` 回调跑在 IPC **读线程**，在那里面
/// 调任何"发 IPC 命令并等回包"的逻辑都会**死锁**（回包得由同一线程读取）——
/// 实测表现为第 2 集 launch 打出后 file-loaded 永不到来，画面停在最后一帧不动。
#[tauri::command]
pub async fn duanju_auto_next(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<bool, String> {
    match auto_play_next(&app, &state).await {
        Ok(v) => {
            if v {
                crate::libmpv::trace("[duanju] 已自动播放下一集");
            }
            Ok(v)
        }
        Err(e) => {
            crate::libmpv::trace(&format!("[duanju] 自动连播失败（已停止连播）: {e}"));
            Err(e)
        }
    }
}

/// [lc-2116] 清空连播队列（用户切换到别的车道 / 手动停止时用）。
pub fn clear_auto_next() {
    DJ_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    *auto_next_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// [lc-2104] 轮询 `video-params/w|h`（起播后解码器就绪才有值；最多等 ~6s）。
async fn wait_video_size(sess: &crate::mpv::MpvSession) -> Option<(i64, i64)> {
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let w = sess
            .command(serde_json::json!(["get_property", "video-params/w"]))
            .await
            .ok()
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let h = sess
            .command(serde_json::json!(["get_property", "video-params/h"]))
            .await
            .ok()
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if w > 0 && h > 0 {
            return Some((w, h));
        }
    }
    None
}

/// [lc-2104] 竖屏「锁高、宽自适应」：窗口高度 = max(主窗高, 工作区 90%) 且受 94% 封顶，
/// 宽度 = 高度 × 视频真实比例 ⇒ 窗口与视频同比例、mpv 内部零留白（无黑边）。
async fn apply_vertical_geometry(
    app: &tauri::AppHandle,
    sess: &crate::mpv::MpvSession,
    vw: i64,
    vh: i64,
) {
    use tauri::Manager;
    if vw <= 0 || vh <= 0 {
        return;
    }
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let Ok(size) = win.inner_size() else { return };
    let area = win
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| (m.work_area().size.width, m.work_area().size.height));
    let mut h = size.height.max(480) as f64;
    if let Some((_, ah)) = area {
        if ah > 0 {
            let ah = ah as f64;
            h = h.max(ah * 0.90).min(ah * 0.94);
        }
    }
    let aspect = vw as f64 / vh as f64;
    let mut w = h * aspect;
    if let Some((aw, ah)) = area {
        if aw > 0 {
            let max_w = aw as f64 * 0.94;
            if w > max_w {
                w = max_w;
                h = w / aspect;
            }
        }
        if ah > 0 {
            let max_h = ah as f64 * 0.94;
            if h > max_h {
                h = max_h;
                w = h * aspect;
            }
        }
    }
    let (w, h) = (w.round() as u32, h.round() as u32);
    if w == 0 || h == 0 {
        return;
    }
    let centered = win
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let p = m.position();
            let s = m.size();
            let x = p.x + ((s.width as i32 - w as i32) / 2).max(0);
            let y = p.y + ((s.height as i32 - h as i32) / 2).max(0);
            format!("{w}x{h}+{x}+{y}")
        })
        .unwrap_or_else(|| format!("{w}x{h}"));
    crate::libmpv::trace(&format!(
        "[lc-2104] 竖屏锁高调窗：{centered}（视频 {vw}x{vh}，比例 {aspect:.4}）"
    ));
    let _ = sess.command(serde_json::json!(["set", "geometry", centered])).await;
    let _ = sess
        .command(serde_json::json!(["set", "keepaspect-window", "yes"]))
        .await;
}

// ───────────────────── [lc-2123] 短剧独立观看记录 ─────────────────────
//
// 需求：观看记录要体现在**短剧详情页**里，且**独立存储、不走软件主观看记录**
// （主记录是飞牛历史 `history.rs`，短剧不属于飞牛资源，混进去会污染主列表）。
//
// 存储：`<app_data>/duanju-history.json`，键 = drama.id（含站源前缀，天然唯一）。
// 结构：{ [dramaId]: { drama: 摘要, index: 集索引(0 基), pos: 秒, dur: 秒, at: 毫秒时间戳 } }
// 只保留最近 MAX 条（按 at 倒序裁剪），避免无限膨胀 —— 短剧站源的剧 ID 会变。

const HISTORY_MAX: usize = 200;

fn history_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    use tauri::Manager;
    Some(app.path().app_data_dir().ok()?.join("duanju-history.json"))
}

fn read_history(app: &tauri::AppHandle) -> serde_json::Map<String, Value> {
    history_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_history(app: &tauri::AppHandle, map: &serde_json::Map<String, Value>) {
    let Some(path) = history_path(app) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // 按 at 倒序裁到上限，再落盘（写入失败静默：进度记录不值得打断播放）
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    entries.sort_by_key(|(_, v)| -v.get("at").and_then(|x| x.as_i64()).unwrap_or(0));
    let mut out = serde_json::Map::new();
    for (k, v) in entries.into_iter().take(HISTORY_MAX) {
        out.insert(k.clone(), v.clone());
    }
    if let Ok(text) = serde_json::to_string(&Value::Object(out)) {
        let _ = std::fs::write(path, text);
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 记录一条观看进度（起播/切集/退出时调用）。`drama` 只存展示所需摘要，避免文件过大。
pub fn record_history(
    app: &tauri::AppHandle,
    drama: &Value,
    index: i64,
    pos_secs: f64,
    dur_secs: f64,
) {
    let Some(id) = drama.get("id").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) else {
        return;
    };
    let mut map = read_history(app);
    let summary = json!({
        "id": id,
        "title": drama.get("title").cloned().unwrap_or(json!("")),
        "cover": drama.get("cover").cloned().unwrap_or(json!("")),
        "source": drama.get("source").cloned().unwrap_or(json!("")),
        "episodes": drama.get("episodes").cloned().unwrap_or(json!(0)),
        "score": drama.get("score").cloned().unwrap_or(json!("")),
        "releaseStatus": drama.get("releaseStatus").cloned().unwrap_or(json!("")),
    });
    map.insert(
        id.to_string(),
        json!({
            "drama": summary,
            "index": index,
            "pos": pos_secs.max(0.0),
            "dur": dur_secs.max(0.0),
            "at": now_ms(),
        }),
    );
    write_history(app, &map);
}

/// [lc-2123] 读取观看记录（按最近观看倒序）。前端详情页/列表直接用。
#[tauri::command]
pub async fn duanju_history(app: tauri::AppHandle) -> Result<Value, String> {
    let map = read_history(&app);
    let mut list: Vec<Value> = map.into_values().collect();
    list.sort_by_key(|v| -v.get("at").and_then(|x| x.as_i64()).unwrap_or(0));
    Ok(Value::Array(list))
}

/// [lc-2123] 读单部剧的进度（详情页「继续观看」用）。未看过返回 null。
#[tauri::command]
pub async fn duanju_history_one(app: tauri::AppHandle, id: String) -> Result<Value, String> {
    let map = read_history(&app);
    Ok(map.get(&id).cloned().unwrap_or(Value::Null))
}

/// [lc-2123] 删除一条记录（用户手动清除）。
#[tauri::command]
pub async fn duanju_history_remove(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let mut map = read_history(&app);
    if map.remove(&id).is_some() {
        write_history(&app, &map);
    }
    Ok(())
}

/// [lc-2123] 清空全部短剧观看记录。
#[tauri::command]
pub async fn duanju_history_clear(app: tauri::AppHandle) -> Result<(), String> {
    let empty = serde_json::Map::new();
    write_history(&app, &empty);
    Ok(())
}

/// [lc-2123] 回写播放进度（前端在播放中/关闭 mpv 前调用）。
/// drama 只需含 id/title/cover 等摘要字段；index 是 0 基集索引。
#[tauri::command]
pub async fn duanju_history_update(
    app: tauri::AppHandle,
    drama: Value,
    index: i64,
    pos: f64,
    dur: f64,
) -> Result<(), String> {
    record_history(&app, &drama, index, pos, dur);
    Ok(())
}

/// [lc-2128] 恢复 modernz 的 ±10 秒跳转键（非短剧起播时调用）。
pub fn restore_modernz_jump_buttons(app: &tauri::AppHandle) {
    set_modernz_jump_buttons(app, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        // 随包模块升级判定：只有随包版本**更高**才替换数据目录里的旧模块。
        assert!(version_le("1.0.1", "1.0.2")); // 旧随包不覆盖已装新模块
        assert!(!version_le("1.0.2", "1.0.1")); // 新随包覆盖旧模块
        assert!(version_le("1.0.2", "1.0.2")); // 同版本不重复部署
        assert!(!version_le("1.1.0", "1.0.9")); // 逐段比较不是字符串比较
        assert!(!version_le("2.0.0", "1.9.9"));
        assert!(!version_le("1.0.2", "")); // 数据目录缺 manifest = 最低版本，随包必部署
        assert!(version_le("", "0.0.1"));
        assert!(!version_le("1.0.10", "1.0.9")); // 数值比较，不是字典序
    }
}
