//! [lc-1851] FN ID（飞牛 ID / FN Connect）远程登录 —— 自旧项目 Fntv-Plus 移植。
//!
//! 协议（旧项目 `fnid_login.ts` 实测沉淀，1292 行 Electron 版的 Rust 化重写）：
//!   1. OAuth 窗口加载 `https://5ddd.com/{fnId}`（飞牛官方 FN Connect 入口，fnId 用户在
//!      飞牛后台获取）；用户在飞牛 ID 系统页登录自己的账号。
//!   2. 5ddd.com 重定向到用户 NAS（官方中继 `{fnId}.fnos.net` 子域 / 直连 / IPv6）。
//!      中继隧道建立可能慢（实测数十秒），注入脚本轮询 `/v/api/v1/sys/config` 确认可达。
//!   3. 注入脚本导航到 NAS `/signin` 自动点「授权」→ `POST /oauthapi/authorize` 响应里
//!      拿 code（fetch/XHR hook 捕获；兜底 URL 回跳 `/v/oauth/result?code=`）。
//!   4. `POST {base}/v/api/v1/auth` body `{source:"Trim-NAS", code}` → `{data:{token}}`。
//!   5. token 即 Trim-MC-token —— 与账号密码登录同源，写 config 后全链路（fnapi 签名/
//!      播放/进度回传）直接复用。
//!
//! Electron 版与 Tauri 版的差异：旧版靠 BrowserWindow 双 session 互拷 cookie；Tauri 2 的
//! WebView 与 Rust 侧用**伪导航信号**通信（`http://fnid.anibox/base|code?...`，
//! on_navigation 拦截返回 false）——与 webrs.rs 资源嗅探同款机制，零 cookie 搬运。
//! 访问码门禁：登录页可选填，注入脚本遇「请输入访问码」页自动填码提交（旧 PR #157 同款）。
//!
//! 已知边界：官方中继隧道极慢时（>240s）超时退出；窗口被用户手关即取消。

use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::{config, fnapi, AppState};

/// 伪导航信号域（on_navigation 拦截、永不真实加载）。
const SIGNAL_HOST: &str = "fnid.anibox";

#[derive(Default, Clone)]
struct FnIdCapture {
    /// sys/config 轮询确认可达的 NAS 基址（scheme://host[:port]）
    base: Option<String>,
    /// OAuth 授权码
    code: Option<String>,
}

/// 注入脚本：每页自动重放（initialization_script 语义）。access_code 由 Rust 侧
/// 序列化进字面量（None → null，脚本内判空跳过）。
fn inject_script(access_code_js: &str) -> String {
    format!(
        r#"(function() {{
    if (window.__anibox_fnid) return;
    window.__anibox_fnid = true;
    var ACCESS_CODE = {access_code_js};

    function go(path, params) {{
        var q = new URLSearchParams(params).toString();
        // 伪导航发信号；Rust on_navigation 拦截（返回 false），页面留在原地继续跑
        location.href = 'http://{SIGNAL_HOST}' + path + '?' + q;
    }}
    function isPortal() {{ return (location.hostname || '').toLowerCase().indexOf('5ddd.com') !== -1; }}
    function isLogin() {{ return location.href.indexOf('/login') !== -1; }}

    // 1) sys/config 轮询：确认 NAS 可达并把基址报回（旧项目同款退避节奏）
    function pollBase() {{
        if (isPortal() || isLogin()) return;
        var host = (location.hostname || '').toLowerCase();
        if (host === 'fnos.net') return; // 中继门户根只返回门户 HTML；真实 NAS 在子域
        var isRelay = host.endsWith('.fnos.net');
        var max = isRelay ? 40 : 6, delay = isRelay ? 1500 : 700, n = 0;
        (function tick() {{
            n++;
            fetch('/v/api/v1/sys/config', {{ credentials: 'include' }})
                .then(function(r) {{ return r.text(); }})
                .then(function(t) {{
                    t = (t || '').trim();
                    if (t.charAt(0) === '{{') {{
                        if (!window.__anibox_base_sent) {{
                            window.__anibox_base_sent = true;
                            go('/base', {{ u: location.origin }});
                        }}
                        return;
                    }}
                    if (n < max) setTimeout(tick, delay * Math.min(n, 6));
                }})
                .catch(function() {{ if (n < max) setTimeout(tick, delay * Math.min(n, 6)); }});
        }})();
    }}

    // 2) code 捕获：fetch/XHR hook /oauthapi/authorize 响应；URL 回跳兜底
    function sendCode(t) {{
        if (window.__anibox_code_sent) return;
        var m = (t || '').match(/"code"\s*:\s*"([^"]+)"/);
        if (m) {{
            window.__anibox_code_sent = true;
            go('/code', {{ c: m[1], u: location.origin }});
        }}
    }}
    function hook() {{
        var of = window.fetch;
        if (of) {{
            window.fetch = function() {{
                var p = of.apply(this, arguments);
                var url = (arguments[0] && arguments[0].url) || arguments[0] || '';
                p.then(function(r) {{
                    try {{
                        if (String(url).indexOf('/oauthapi/authorize') !== -1)
                            r.clone().text().then(sendCode);
                    }} catch (e) {{}}
                }}).catch(function() {{}});
                return p;
            }};
        }}
        var oo = XMLHttpRequest.prototype.open;
        XMLHttpRequest.prototype.open = function(m, u) {{
            this.addEventListener('load', function() {{
                try {{
                    if (String(u).indexOf('/oauthapi/authorize') !== -1) sendCode(this.responseText);
                }} catch (e) {{}}
            }});
            return oo.apply(this, arguments);
        }};
        // URL 回跳兜底：/v/oauth/result?code=xxx
        var mq = location.href.match(/[?&]code=([^&]+)/);
        if (mq && location.href.indexOf('/oauth/result') !== -1) {{
            window.__anibox_code_sent = true;
            go('/code', {{ c: decodeURIComponent(mq[1]), u: location.origin }});
        }}
    }}

    // 3) /signin 授权页自动点「授权」（旧项目 tryAutoAuthorize 同款轮询）
    function autoSign() {{
        if (location.href.indexOf('/signin') === -1) return;
        var n = 0;
        var t = setInterval(function() {{
            if (++n > 40) {{ clearInterval(t); return; }}
            var els = document.querySelectorAll('button, .submit, [class*="confirm"], [class*="authorize"]');
            for (var i = 0; i < els.length; i++) {{
                var txt = (els[i].textContent || '').trim();
                if (txt.indexOf('授权') !== -1 || /^(Allow|Authorize)$/i.test(txt)) {{
                    els[i].click();
                    clearInterval(t);
                    return;
                }}
            }}
        }}, 500);
    }}

    // 4) fnOS「请输入访问码」网关页自动填码（旧 PR #157 移植）
    function fillAccessCode() {{
        if (!ACCESS_CODE) return;
        var input = document.querySelector('#access-code-input, input[name="access-code"]');
        var form = input ? input.closest('form') : null;
        var title = document.querySelector('#page-title');
        if (!input || !form || !title || title.textContent.trim() !== '请输入访问码') return;
        var setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
        setter.call(input, ACCESS_CODE);
        input.dispatchEvent(new Event('input', {{ bubbles: true }}));
        input.dispatchEvent(new Event('change', {{ bubbles: true }}));
        if (typeof form.requestSubmit === 'function') form.requestSubmit();
        else {{
            var s = form.querySelector('button[type="submit"], input[type="submit"]');
            if (s) s.click();
        }}
    }}

    setTimeout(function() {{ pollBase(); hook(); autoSign(); fillAccessCode(); }}, 500);
}});"#,
        access_code_js = access_code_js,
        SIGNAL_HOST = SIGNAL_HOST,
    )
}

/// FN ID 登录：打开 OAuth 窗口走 FN Connect 流程，成功后写配置（与账号密码登录同一收尾）。
#[tauri::command]
pub async fn fn_id_login(
    app: AppHandle,
    state: State<'_, AppState>,
    fn_id: String,
    access_code: Option<String>,
) -> Result<Value, String> {
    let fn_id = fn_id.trim().trim_start_matches('/').to_string();
    if fn_id.is_empty() {
        return Err("请填写 FN ID（飞牛后台 FN Connect 页可获取）".into());
    }
    let access_code = access_code.unwrap_or_default().trim().to_string();
    let access_code_js = serde_json::to_string(&access_code).unwrap_or_else(|_| "null".into());

    let entry_url: tauri::Url = format!("https://5ddd.com/{fn_id}")
        .parse()
        .map_err(|e| format!("FN ID 入口地址解析失败: {e}"))?;

    let capture: Arc<Mutex<FnIdCapture>> = Arc::new(Mutex::new(FnIdCapture::default()));
    let nav_capture = Arc::clone(&capture);

    let label = "fnid-oauth";
    // 复用残留窗（上次超时未关）：先销毁保证初始化脚本/状态干净
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.destroy();
    }

    let mut b = WebviewWindowBuilder::new(
        &app,
        label,
        WebviewUrl::External(entry_url),
    )
    .title("FN ID 登录 · AniBox")
    .inner_size(900.0, 720.0)
    .initialization_script(&inject_script(&access_code_js))
    .on_navigation(move |u| {
        // 兜底：授权后 URL 直带回跳 code（不经过 XHR hook 的场景）
        if u.path().contains("/oauth/result") {
            if let Some((_, c)) = u.query_pairs().find(|(k, _)| k == "code") {
                let mut st = nav_capture.lock().unwrap();
                st.code = Some(c.to_string());
                if st.base.is_none() {
                    st.base = Some(format!(
                        "{}://{}",
                        u.scheme(),
                        u.host_str().unwrap_or("")
                    ));
                }
            }
            return false;
        }
        if u.host_str() == Some(SIGNAL_HOST) {
            let mut base = None;
            let mut code = None;
            for (k, v) in u.query_pairs() {
                match k.as_ref() {
                    "u" => base = Some(v.into_owned()),
                    "c" => code = Some(v.into_owned()),
                    _ => {}
                }
            }
            let mut st = nav_capture.lock().unwrap();
            match u.path() {
                "/base" => {
                    if let Some(b) = base {
                        st.base = Some(b);
                    }
                }
                "/code" => {
                    if let Some(c) = code {
                        st.code = Some(c);
                    }
                    if let Some(b) = base {
                        st.base = Some(b);
                    }
                }
                _ => {}
            }
            return false; // 伪导航，取消
        }
        true
    });

    // 与主窗口传同一套 additionalBrowserArgs（WebView2 一致性，红线同 webrs_webview）
    if let Some(args) = crate::webrs_webview::main_window_browser_args(&app) {
        b = b.additional_browser_args(&args);
    }
    b.build().map_err(|e| format!("创建 FN ID 登录窗口失败: {e}"))?;

    // 等待捕获（240s：中继隧道实测可达数十秒，留足余量）；窗口被手关即取消
    let deadline = Instant::now() + Duration::from_secs(240);
    let (base, code) = loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if app.get_webview_window(label).is_none() {
            return Err("FN ID 登录已取消（窗口被关闭）".into());
        }
        let st = capture.lock().unwrap().clone();
        if let (Some(b), Some(c)) = (st.base, st.code) {
            break (b, c);
        }
        if Instant::now() > deadline {
            if let Some(w) = app.get_webview_window(label) {
                let _ = w.destroy();
            }
            return Err(
                "FN ID 登录超时：未能通过官方中继完成授权（可稍后重试，或改用服务器地址直连）"
                    .into(),
            );
        }
    };

    // code 换 token（与账号密码登录同一条 fnapi 链；访问码会话已由注入脚本在窗口内建立）
    let client = fnapi::new_client();
    let data = client
        .call(
            &base,
            "/v/api/v1/auth",
            "POST",
            "",
            Some(json!({ "source": "Trim-NAS", "code": code })),
        )
        .await
        .map_err(|e| format!("授权码换取 token 失败: {e}"))?;
    let token = data
        .pointer("/data/token")
        .and_then(|t| t.as_str())
        .or_else(|| data.get("token").and_then(|t| t.as_str()))
        .ok_or_else(|| format!("授权响应缺少 token: {data}"))?
        .to_string();

    // 固化真实基址（80 → 实际 API 端口，同账号密码登录）
    let server = client.resolve_final_base(&base, &token).await;

    // 收窗
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.destroy();
    }

    // 与 login 命令同一收尾：写配置 + 更新内存态（token 即 Trim-MC-token，全链路复用）
    let mut cfg = state.config.read().await.clone();
    cfg.server = server;
    cfg.username = fn_id;
    cfg.token = token;
    cfg.access_code = access_code;
    config::save(&app, &cfg).await?;
    *state.config.write().await = cfg.clone();
    Ok(json!({ "ok": true }))
}
