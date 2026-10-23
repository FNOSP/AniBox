//! [lc-1747] webrs 的常驻隐藏 WebView 单例服务（嗅探 / 反爬验证共用基建）。
//!
//! 背景（历史失败根因）：旧实现「每次嗅探 / 验证都新建 + 销毁隐藏 WebView2 窗」——
//! 建窗路径偶发 ERROR_INVALID_STATE(0x8007139F)，失败时容易泄漏孤儿窗，多次泄漏后
//! 耗尽资源把进程拖崩（lc-1613/lc-1624 各修过一茬握手与重试，但「每请求建/销窗」
//! 的模式本身脆弱）。本模块把两个隐藏窗改为**懒创建、常驻**的单例：
//!
//!   · `webrs-sniffer` —— 直链嗅探。initialization_script 装**无任务参数**的嗅探钩子
//!     （WebView2 的 AddScriptToExecuteOnDocumentCreated 对每次导航的新文档自动重放，
//!     load 前后都覆盖），单例复用天然可靠；任务差异（legacy iframe 回收）由 Rust 轮询读取。
//!   · `webrs-captcha` —— 反爬验证。脚本按规则配置不同，**不进** initialization_script，
//!     由调用方导航后 eval 注入 + 轮询补注（脚本是 DOM 扫描器而非网络猴补，晚注入无损）。
//!
//! 生命周期约定：
//!   · 任务结束**不销毁**窗 —— `park_webview` 导航 `about:blank` 释放页面资源并隐藏；
//!   · App 退出统一销毁（lib.rs `RunEvent::Exit` → `shutdown_webviews`）；
//!   · 窗意外死亡（进程异常 / 用户关掉可见验证窗）：登记失效 → 下次任务自动重建
//!     （`create_hidden_webview` 保留 lc-1613 换 label 重试 + lc-1624「交不回就自毁」握手）。
//!
//! 线程模型：`ensure_persistent` 全程持有进程内 tokio 互斥锁（两个单例共用一把——
//! 建窗本身在主线程串行执行，共用锁只影响建窗瞬间，不吞并发）；同一单例的任务级
//! 串行由调用方保证（嗅探任务锁排队、验证会话单席位互斥）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};

/// 嗅探常驻单例窗 label（重试变体 `-r1` 由 create_hidden_webview 派生）。
pub(crate) const SNIFFER_LABEL: &str = "webrs-sniffer";
/// 反爬验证常驻单例窗 label。
pub(crate) const CAPTCHA_LABEL: &str = "webrs-captcha";

/// [lc-1747] 常驻窗固定 UA。clearance cookie 与签发它的 UA 绑定——单例窗跨任务复用，
/// UA 必须恒定不变；收割时存 `navigator.userAgent`（= 本 UA），HTTP 重放用同一个，
/// 「验证 UA 覆盖规则 UA」的闭环天然一致。规则自带的 userAgent 只影响纯 HTTP 请求。
pub(crate) const WEBVIEW_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

// ─────────────── [lc-1613] 建窗参数对齐主窗口（防 0x8007139F，红线勿动） ───────────────
//
// 根因：主窗口（tauri.conf.json `app.windows[0].additionalBrowserArgs`）设了
// `--autoplay-policy=no-user-gesture-required --disable-features=msWebOOUI,msPdfOOUI`，
// 而追加创建的隐藏窗口（嗅探 / 验证）没设该项，落到 wry 的默认值
// `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`。
//
// WebView2 规则（见 wry `web_context.rs` 注释 + MS 文档）：**同一 user data folder 下
// 若已存在运行中的实例，且两次 Environment 的 CoreWebView2EnvironmentOptions 不一致，
// 则新的 CreateCoreWebView2EnvironmentWithOptions 直接失败**，返回
// ERROR_INVALID_STATE(0x8007139F)「组或资源的状态不是执行请求操作的正确状态」。
// 主窗口是进程里第一个 environment（所以能建），追加窗口 options 不同 ⇒ 每次必挂。
//
// 修法：追加窗口**逐字对齐主窗口的 additionalBrowserArgs**（运行期从配置读，避免硬编码，
// 改了 tauri.conf.json 也不会再错位）。

/// 主窗口（配置里首个设了 additionalBrowserArgs 的窗口）的浏览器参数。
/// 返回 None 表示主窗口也没设 ⇒ 追加窗口同样保持 None（两边都用 wry 默认值，天然一致）。
pub(crate) fn main_window_browser_args(app: &AppHandle) -> Option<String> {
    app.config()
        .app
        .windows
        .iter()
        .find_map(|w| w.additional_browser_args.clone())
}

/// [lc-1613] 把建窗失败翻译成带排查建议的文案（识别 ERROR_INVALID_STATE 0x8007139F）。
pub(crate) fn describe_webview_error(err: &tauri::Error) -> String {
    let raw = err.to_string();
    let invalid_state = raw.contains("8007139F")
        || raw.contains("8007139f")
        || raw.contains("-2147019873") // 0x8007139F 的有符号 i32 形式
        || raw.contains("正确状态")
        || raw.contains("not in the correct state");
    if invalid_state {
        format!(
            "{raw}\n（WebView2 ERROR_INVALID_STATE：常见于「同一用户数据目录下 Environment \
             选项不一致」或「上次退出残留 WebView2 进程占用用户数据目录」。排查：① 任务管理器\
             结束所有 msedgewebview2.exe 后重开应用；② 到「设置→应用」修复/更新 Microsoft \
             Edge WebView2 Runtime；③ 若装了会拦截进程的安全软件，请放行本应用。）"
        )
    } else {
        raw
    }
}

/// [lc-1613] 在主线程序列上创建隐藏 WebView（从 webrs.rs 原样迁入）。
/// `make(app, label)` 负责按给定 label 组装并 build 窗口，最多被调用 2 次；
/// 第 2 次换新 label（`{base}-r1`）以规避上一次残留 / 瞬时状态。
/// 成功返回**实际使用的 label**（调用方据此找窗）；失败返回可诊断错误。
///
/// [lc-1624] 防「孤儿隐藏窗」（这是「点几次播放就崩」的关键成因之一）：
/// 等待方一旦超时（20s）就给 `aborted` 置位；主线程闭包**建成后**把 label 交回等待方，
/// 交不回（等待方已放弃）或已 aborted，就**立刻把刚建的窗销毁**——单例模式下这条握手
/// 只在建窗失败 / 等待超时的兜底路径生效，正常路径不再频繁建销窗。
///
/// 同时不在主线程里 sleep 重试退避（主线程就是事件循环，占住它会拖慢其它已排队的
/// 建窗 / 销毁，把 20s 超时逼出来、制造孤儿窗）；换 label 本身已能绕开 0x8007139F 的
/// EnvironmentOptions 冲突。
pub(crate) async fn create_hidden_webview<F>(
    app: &AppHandle,
    base_label: &str,
    make: F,
) -> Result<String, String>
where
    F: Fn(&AppHandle, &str) -> Result<(), tauri::Error> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
    let app2 = app.clone();
    let base = base_label.to_string();
    let aborted = Arc::new(AtomicBool::new(false));
    let aborted2 = aborted.clone();
    app.run_on_main_thread(move || {
        let mut last_err = String::from("创建 WebView 失败");
        for attempt in 0..2u32 {
            let label = if attempt == 0 {
                base.clone()
            } else {
                format!("{base}-r{attempt}")
            };
            // 清掉可能残留的同名窗口（只有此前创建成功过才会有）
            if let Some(w) = app2.get_webview_window(&label) {
                let _ = w.destroy();
            }
            match make(&app2, &label) {
                Ok(()) => {
                    // [lc-1618] 埋点：建窗成功也留痕。
                    // [lc-1624] 但**先**看等待方还要不要这个窗：交不回 / 已放弃 → 立即自毁。
                    let delivered = tx.send(Ok(label.clone())).is_ok();
                    if delivered && !aborted2.load(Ordering::Relaxed) {
                        crate::libmpv::trace(&format!(
                            "[webrs] 隐藏窗创建成功 label={label}（第 {} 次尝试）",
                            attempt + 1
                        ));
                    } else {
                        if let Some(w) = app2.get_webview_window(&label) {
                            let _ = w.destroy();
                        }
                        crate::libmpv::trace(&format!(
                            "[webrs] 隐藏窗创建成功但等待方已放弃 → 立即销毁 label={label}"
                        ));
                    }
                    return;
                }
                Err(e) => {
                    last_err = describe_webview_error(&e);
                    crate::libmpv::trace(&format!(
                        "[webrs] 隐藏窗创建失败 label={label}（第 {} 次尝试）: {last_err}",
                        attempt + 1
                    ));
                }
            }
        }
        let _ = tx.send(Err(last_err));
    })
    .map_err(|e| format!("窗口调度失败: {e}"))?;

    let aborted3 = aborted.clone();
    let res = tokio::task::spawn_blocking(move || {
        match rx.recv_timeout(std::time::Duration::from_secs(20)) {
            Ok(v) => v,
            Err(_) => {
                // 等待方放弃：置位，让主线程闭包若稍后建成就自毁，别留孤儿窗
                aborted3.store(true, Ordering::Relaxed);
                Err("创建 WebView 超时（20s）".to_string())
            }
        }
    })
    .await
    .map_err(|e| format!("等待窗口失败: {e}"))?;

    if res.is_err() {
        // 兜底：两个可能用过的 label 都再 best-effort 清一次（闭包可能刚建好、置位晚一拍）
        for l in [base_label.to_string(), format!("{base_label}-r1")] {
            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Some(w) = app2.get_webview_window(&l) {
                    let _ = w.destroy();
                }
            });
        }
    }
    res
}

/// 在隐藏窗里 eval 一段 JS 表达式并取回 JSON 序列化结果（超时 5s 返回 None）。
/// 表达式异常时返回 null（Windows 下 eval 的异常会被吞掉，故 JS 里自带 try/catch）。
pub(crate) async fn eval_json(w: &tauri::WebviewWindow, expr: &str) -> Option<String> {
    let js = format!("(function(){{ try {{ return {expr}; }} catch (e) {{ return null; }} }})()");
    // 用 std mpsc（Sender::send 取 &self，闭包可 Fn 重复调用；oneshot 会 move self 不适用）
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if w
        .eval_with_callback(js, move |s| {
            let _ = tx.send(s);
        })
        .is_err()
    {
        return None;
    }
    tokio::task::spawn_blocking(move || rx.recv_timeout(std::time::Duration::from_secs(5)).ok())
        .await
        .ok()
        .flatten()
}

// ───────────────────────── [lc-1747] 常驻单例管理 ─────────────────────────

/// 单例登记：base label → 当前实际 label（create_hidden_webview 重试可能换 `-r1`）。
fn persistent_slots() -> &'static Mutex<HashMap<String, String>> {
    static SLOTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 建窗互斥：防止两个任务同时发现「窗缺失」而并发重建——后一个会 `destroy` 掉
/// 前一个刚建好的同名窗（create_hidden_webview 的清残留步骤），互相拆台。
fn ensure_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// 获取常驻单例窗的 label（懒创建；窗意外死亡时自动重建）。
///
/// 返回 `(label, created)`：
/// · `created=true` 本次新建（`make` 闭包里已带初始 URL，调用方不必再导航）；
/// · `created=false` 复用已存在的窗（调用方自行 `navigate` 到本任务目标页——
///   initialization_script 由 WebView2 在每次导航时自动重放，无需重新绑定）。
///
/// `make` 仅在需要建窗时被调用（复用路径不调用），组装要求见各调用方：
/// 必须传与主窗口逐字一致的 `additional_browser_args`（见 `main_window_browser_args`）。
pub(crate) async fn ensure_persistent<F>(
    app: &AppHandle,
    base_label: &str,
    make: F,
) -> Result<(String, bool), String>
where
    F: Fn(&AppHandle, &str) -> Result<(), tauri::Error> + Send + 'static,
{
    let _guard = ensure_lock().lock().await;
    // 快路径：登记在册且窗对象还活着 → 直接复用（绝大多数请求走这条，零建窗开销）
    if let Ok(slots) = persistent_slots().lock() {
        if let Some(label) = slots.get(base_label) {
            if app.get_webview_window(label).is_some() {
                return Ok((label.clone(), false));
            }
        }
    }
    // 慢路径：没有登记或窗已死（进程异常 / 被用户关闭 / 上次建窗失败）→ 重建。
    // 先 best-effort 清掉 base 与 -r1 残留（create_hidden_webview 内部也会清 base）。
    for l in [base_label.to_string(), format!("{base_label}-r1")] {
        if let Some(w) = app.get_webview_window(&l) {
            let _ = w.destroy();
        }
    }
    let label = create_hidden_webview(app, base_label, make).await?;
    if let Ok(mut slots) = persistent_slots().lock() {
        slots.insert(base_label.to_string(), label.clone());
    }
    Ok((label, true))
}

/// [lc-1747] 任务收尾「泊车」：导航 `about:blank` 释放页面资源（视频 / DOM / JS 上下文），
/// 并隐藏窗。**不销毁**窗 —— 单例常驻，下次任务直接复用。窗已不存在则静默跳过
/// （下次 ensure_persistent 会自动重建）。
pub(crate) fn park_webview(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.hide();
        if let Ok(u) = url::Url::parse("about:blank") {
            let _ = w.navigate(u);
        }
    }
}

/// [lc-1747] App 退出统一清理（lib.rs `RunEvent::Exit` 调用）：销毁全部常驻单例窗
/// （含建窗重试可能用过的 `-r1` 变体）。best-effort，失败不阻塞退出。
pub(crate) fn shutdown_webviews(app: &AppHandle) {
    for base in [SNIFFER_LABEL, CAPTCHA_LABEL] {
        for l in [base.to_string(), format!("{base}-r1")] {
            if let Some(w) = app.get_webview_window(&l) {
                let _ = w.destroy();
            }
        }
    }
    if let Ok(mut slots) = persistent_slots().lock() {
        slots.clear();
    }
}
