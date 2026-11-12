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

fn auto_next_slot() -> &'static Mutex<Option<AutoNext>> {
    AUTO_NEXT.get_or_init(|| Mutex::new(None))
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
    let plan = duanju_resolve(app.clone(), drama.clone(), chapter, index, quality).await?;
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
    // [lc-2128] 短剧不需要 ±10 秒跳转键（一集 1~3 分钟，跳 10 秒没意义；用户要求去掉）。
    // 做法：改 modernz.conf 的 jump_buttons，modernz 的 read_options 回调会
    // request_tick() 重绘（实测它能响应配置文件变更），因此**无需重启 mpv**。
    // 非短剧起播时会由 restore_modernz_defaults() 恢复。
    set_modernz_jump_buttons(app, false);
    crate::player::launch(
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
    .await?;
    // [lc-2104] 竖屏调窗改为**主动执行**（不依赖 file-loaded 事件回调）：
    // 事件方案有两个坑——① 会话复用时事件可能已消费过；② 调窗分支依赖会话内的原子
    // 标志，旧会话（升级前启动）标志不存在。这里在 launch 返回后轮询 video-params，
    // 就绪即按「锁高、宽自适应」设窗口几何，任何会话状态都成立。
    if let Some(sess) = {
        let slot = state.mpv.lock().await;
        slot.as_ref().cloned()
    } {
        if let Some((vw, vh)) = wait_video_size(&sess).await {
            apply_vertical_geometry(app, &sess, vw, vh).await;
        }
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
    }
    // [lc-2123] 标记短剧会话活跃（time-pos 观察回调据此写历史）
    DJ_ACTIVE.store(true, std::sync::atomic::Ordering::Release);
    // [lc-2123] 起播即记一条观看记录（独立存储，不走软件主历史）。
    // pos 从这里开始为 0，实际进度由前端在播放中/退出时回写（duanju_play 返回后前端
    // 拿不到 mpv 的 time-pos，所以「继续观看」的精度由前端定时回写保证）。
    record_history(app, &drama, index, 0.0, 0.0);
    Ok(json!({ "url": url, "session": plan.get("session").cloned().unwrap_or(json!("")) }))
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

    let title = chapter
        .get("title")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .or_else(|| {
            drama
                .get("title")
                .and_then(|x| x.as_str())
                .map(|t| format!("{t} 第{}集", index + 1))
        });
    // 注意：连播时不再重写队列（chapters 传 None），否则会把 index 重置
    let r = play_impl(app, state, drama, chapter, index, None, title, None).await;
    if r.is_err() {
        *auto_next_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    r.map(|_| true)
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
