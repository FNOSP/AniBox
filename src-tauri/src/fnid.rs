//! [lc-1851] FN ID（飞牛 ID / FN Connect）远程登录 —— 自旧项目 Fntv-Plus 移植。
//!
//! 协议（旧项目 `fnid_login.ts` 实测沉淀，1292 行 Electron 版的 Rust 化重写）：
//!   1. OAuth 窗口加载 `https://5ddd.com/{fnId}`（飞牛官方 FN Connect 入口，fnId 用户在
//!      飞牛后台获取）；用户在飞牛 ID 系统页登录自己的账号。
//!   2. 5ddd.com 重定向到用户 NAS（官方中继 `{fnId}.fnos.net` 子域 / 直连 / IPv6）。
//!      中继隧道建立可能慢（实测数十秒），注入脚本轮询 `/v/api/v1/sys/config` 确认可达。
//!   3. 注入脚本取 `data.nas_oauth.app_id` 后导航到 NAS `/signin` 自动点「授权」→
//!      `POST /oauthapi/authorize` 响应里拿 code（fetch/XHR hook 捕获；兜底 URL 回跳
//!      `/v/oauth/result?code=`）。
//!   4. `POST {base}/v/api/v1/auth` body `{source:"Trim-NAS", code}` → `{data:{token}}`。
//!   5. token 即 Trim-MC-token —— 与账号密码登录同源，写 config 后全链路（fnapi 签名/
//!      播放/进度回传）直接复用。
//!
//! Electron 版与 Tauri 版的差异：旧版靠 BrowserWindow 双 session 互拷 cookie；Tauri 2 的
//! WebView 与 Rust 侧用**伪导航信号**通信（`http://fnid.anibox/{base|sysconfig|code|...}`，
//! on_navigation 拦截返回 false）——与 webrs.rs 资源嗅探同款机制，零 cookie 搬运。
//! 访问码门禁：登录页可选填，注入脚本遇「请输入访问码」页自动填码提交（旧 PR #157 同款）。
//!
//! 诊断：所有关键步骤都打 `[fnid]` 前缀日志到 `%TEMP%\anibox.log`（见 `flog`）。
//!
//! 已知边界：官方中继隧道极慢时（>240s）超时退出；窗口被用户手关即取消。

use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::{config, fnapi, AppState};

/// 伪导航信号域（on_navigation 拦截、永不真实加载）。
const SIGNAL_HOST: &str = "fnid.anibox";

/// 统一诊断日志：`%TEMP%\anibox.log`（debug 开关默认开），前缀 `[fnid]`。
fn flog(msg: impl AsRef<str>) {
    crate::libmpv::trace_mod("fnid", &format!("[fnid] {}", msg.as_ref()));
}

#[derive(Default, Clone)]
struct FnIdCapture {
    /// sys/config 轮询确认可达的 NAS 基址（scheme://host[:port]）
    base: Option<String>,
    /// OAuth 授权码
    code: Option<String>,
}

/// 注入脚本：每页自动重放（initialization_script 语义）。
///
/// 用占位符替换而非 `format!`，避免 JS 里的 `{` / `}` 全部转义（易错且不可读）。
/// access_code 由 Rust 侧序列化进字面量（None → null，脚本内判空跳过）。
fn inject_script(access_code_js: &str) -> String {
    let script = r##"(function() {
    if (window.__anibox_fnid) return;
    window.__anibox_fnid = true;
    var ACCESS_CODE = __ACCESS_CODE__;
    var SIGNAL = 'http://__SIGNAL_HOST__';

    // 伪导航发信号；Rust on_navigation 拦截（返回 false），页面留在原地继续跑。
    // 注意：同一拍内连续给 location.href 赋值会互相覆盖，所以「发信号 + 真正导航」
    // 必须跨拍（见 pollBase 里的 setTimeout）。
    function sig(path, params) {
        try {
            var q = new URLSearchParams(params || {}).toString();
            location.href = SIGNAL + path + (q ? ('?' + q) : '');
        } catch (e) {}
    }
    function isPortal() { return (location.hostname || '').toLowerCase().indexOf('5ddd.com') !== -1; }
    function isLogin()  { return location.href.indexOf('/login') !== -1; }
    function isSignin() { return location.href.indexOf('/signin') !== -1; }

    // 候选基址（对齐旧项目 baseUrl 选择）：
    //   · 官方中继子域 {fnId}.fnos.net / 直连 IP → 当前 origin；
    //   · 5ddd.com 代理（URL 形如 https://5ddd.com/{fnId}/...）→ origin + /{fnId}（保留路径前缀）。
    function candidateBases() {
        var list = [];
        var seg = (location.pathname || '/').split('/').filter(Boolean);
        var skip = { v: 1, api: 1, login: 1, signin: 1 };
        if (seg.length > 0 && !skip[seg[0]] && seg[0].length >= 6) {
            list.push(location.origin + '/' + seg[0]);
        }
        list.push(location.origin);
        return list;
    }

    // 1) sys/config 轮询 → 取 nas_oauth.app_id → 主动跳 /signin 授权页
    //    （对齐旧项目 fetchSysConfigOnce + handleMessage('SysConfig')）
    function pollBase() {
        if (isLogin() || isSignin()) return;
        var host = (location.hostname || '').toLowerCase();
        if (host === 'fnos.net') return; // 中继门户根只返回门户 HTML；真实 NAS 在子域
        var isRelay = host.slice(-9) === '.fnos.net';
        if (isRelay) { try { document.cookie = 'mode=relay; path=/'; } catch (e) {} }
        var bases = candidateBases();
        // 中继子域隧道慢（40×1.5s≈60s）；5ddd 代理/直连快速判定
        var max = isRelay ? 40 : (isPortal() ? 12 : 6);
        var delay = isRelay ? 1500 : 700;
        var n = 0;
        var lastBody = '';

        function handleConfig(base, t) {
            var root = null;
            try { root = JSON.parse(t); } catch (e) { root = null; }
            // ★ 旧项目契约：bodyJson.data.nas_oauth.app_id（务必取 .data 层级，否则永远拿不到 app_id）
            var cfg = (root && root.data) ? root.data : root;
            var oauth = (cfg && cfg.nas_oauth) ? cfg.nas_oauth : null;
            var appId = (oauth && oauth.app_id) ? String(oauth.app_id) : '';
            if (!appId) return false; // 无 app_id → 视为未就绪，继续重试
            sig('/sysconfig', { u: base, a: appId, t: t.slice(0, 300) });
            var navigated = false;
            try { navigated = sessionStorage.getItem('__anibox_signin') === '1'; } catch (e) {}
            if (!navigated) {
                try { sessionStorage.setItem('__anibox_signin', '1'); } catch (e) {}
                var redirectUri = base + '/v/oauth/result';
                var target = base + '/signin?client_id=' + encodeURIComponent(appId)
                    + '&redirect_uri=' + encodeURIComponent(redirectUri);
                // 先发 /sysconfig 信号（这一拍），下一拍再真正导航（同一拍连续赋值会互相覆盖）
                setTimeout(function() { location.href = target; }, 150);
            }
            return true;
        }

        function tick() {
            n++;
            var i = 0;
            (function next() {
                if (i >= bases.length) {
                    if (n < max) { setTimeout(tick, delay * Math.min(n, 6)); return; }
                    sig('/sysconfig-fail', { u: bases.join(' '), t: lastBody.slice(0, 300) });
                    return;
                }
                var base = bases[i++];
                fetch(base + '/v/api/v1/sys/config', { credentials: 'include' })
                    .then(function(r) { return r.text(); })
                    .then(function(t) {
                        t = (t || '').trim();
                        lastBody = t;
                        if (t.charAt(0) === '{' && handleConfig(base, t)) return;
                        next();
                    })
                    .catch(function() { next(); });
            })();
        }
        tick();
    }

    // 2) code 捕获：fetch/XHR hook /oauthapi/authorize 响应；URL 回跳兜底
    function sendCode(t) {
        if (window.__anibox_code_sent) return;
        var code = '';
        try {
            var j = JSON.parse(t || '{}');
            if (j && j.data && j.data.code) code = String(j.data.code);
            else if (j && j.code) code = String(j.code);
        } catch (e) { code = ''; }
        if (!code) {
            var m = (t || '').match(/"code"\s*:\s*"([^"]+)"/);
            if (m) code = m[1];
        }
        if (code) {
            window.__anibox_code_sent = true;
            sig('/code', { c: code, u: location.origin });
        } else {
            sig('/authorize-fail', { u: location.origin, t: (t || '').slice(0, 300) });
        }
    }

    // hook 必须同步安装（document 创建即执行，先于页面自身脚本），否则可能漏掉早期请求
    (function hook() {
        var of = window.fetch;
        if (of) {
            window.fetch = function() {
                var url = '';
                try { url = (arguments[0] && arguments[0].url) || arguments[0] || ''; } catch (e) {}
                var p = of.apply(this, arguments);
                try {
                    if (String(url).indexOf('/oauthapi/authorize') !== -1)
                        p.then(function(r) {
                            try { r.clone().text().then(sendCode); } catch (e) {}
                        }).catch(function() {});
                } catch (e) {}
                return p;
            };
        }
        var oo = XMLHttpRequest.prototype.open;
        XMLHttpRequest.prototype.open = function(m, u) {
            this.addEventListener('load', function() {
                try {
                    if (String(u).indexOf('/oauthapi/authorize') !== -1) sendCode(this.responseText);
                } catch (e) {}
            });
            return oo.apply(this, arguments);
        };
        // URL 回跳兜底：/v/oauth/result?code=xxx
        var mq = location.href.match(/[?&]code=([^&]+)/);
        if (mq && location.href.indexOf('/oauth/result') !== -1) {
            window.__anibox_code_sent = true;
            sig('/code', { c: decodeURIComponent(mq[1]), u: location.origin });
        }
    })();

    // 3) /signin 授权页自动点「授权」（对齐旧项目 tryAutoAuthorize，200ms×20）
    (function autoSign() {
        if (location.href.indexOf('/signin') === -1) return;
        var n = 0;
        var timer = setInterval(function() {
            if (++n > 40) { clearInterval(timer); sig('/autosign-fail', { u: location.href }); return; }
            var els = document.querySelectorAll('button, [role="button"], .submit, [class*="confirm"], [class*="authorize"]');
            for (var i = 0; i < els.length; i++) {
                var txt = ((els[i].innerText || els[i].textContent || '') + '').trim();
                if (txt.indexOf('授权') !== -1 || /^(Allow|Authorize)$/i.test(txt)) {
                    els[i].click();
                    clearInterval(timer);
                    // 延迟上报，避免伪导航信号与点击触发的 authorize 请求抢同一拍
                    setTimeout(function() { sig('/autoclick', { u: location.href }); }, 400);
                    return;
                }
            }
        }, 300);
    })();

    // 4) fnOS「请输入访问码」网关页自动填码（旧 PR #157 移植）
    (function fillAccessCode() {
        if (!ACCESS_CODE) return;
        var n = 0;
        var timer = setInterval(function() {
            if (++n > 12) { clearInterval(timer); return; }
            var input = document.querySelector('#access-code-input, input[name="access-code"]');
            var form = input ? input.closest('form') : null;
            var title = document.querySelector('#page-title');
            if (!input || !form || !title || title.textContent.trim() !== '请输入访问码') return;
            clearInterval(timer);
            var setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
            setter.call(input, ACCESS_CODE);
            input.dispatchEvent(new Event('input', { bubbles: true }));
            input.dispatchEvent(new Event('change', { bubbles: true }));
            sig('/accesscode', { u: location.href });
            setTimeout(function() {
                if (typeof form.requestSubmit === 'function') form.requestSubmit();
                else {
                    var s = form.querySelector('button[type="submit"], input[type="submit"]');
                    if (s) s.click();
                }
            }, 120);
        }, 400);
    })();

    setTimeout(pollBase, 400);
})();"##;
    script
        .replace("__ACCESS_CODE__", access_code_js)
        .replace("__SIGNAL_HOST__", SIGNAL_HOST)
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
    flog(format!(
        "开始 FN ID 登录: fnId={fn_id} entry={entry_url} hasAccessCode={}",
        !access_code.is_empty()
    ));

    let capture: Arc<Mutex<FnIdCapture>> = Arc::new(Mutex::new(FnIdCapture::default()));
    let nav_capture = Arc::clone(&capture);

    let label = "fnid-oauth";
    // 复用残留窗（上次超时未关）：先销毁保证初始化脚本/状态干净
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.destroy();
    }

    let mut b = WebviewWindowBuilder::new(&app, label, WebviewUrl::External(entry_url))
        .title("FN ID 登录 · AniBox")
        .inner_size(900.0, 720.0)
        .initialization_script(&inject_script(&access_code_js))
        .on_navigation(move |u| {
            let path = u.path().to_string();
            let host = u.host_str().unwrap_or("").to_string();

            // ① 伪导航信号（注入脚本 → Rust）
            if host == SIGNAL_HOST {
                let mut base = None;
                let mut code = None;
                let mut app_id = None;
                let mut snippet = None;
                for (k, v) in u.query_pairs() {
                    match k.as_ref() {
                        "u" => base = Some(v.into_owned()),
                        "c" => code = Some(v.into_owned()),
                        "a" => app_id = Some(v.into_owned()),
                        "t" => snippet = Some(v.into_owned()),
                        _ => {}
                    }
                }
                {
                    let mut st = nav_capture.lock().unwrap();
                    if let Some(bs) = &base {
                        st.base = Some(bs.clone());
                    }
                    if let Some(c) = &code {
                        st.code = Some(c.clone());
                    }
                }
                match path.as_str() {
                    "/sysconfig" => flog(format!(
                        "注入脚本上报 sys/config: base={:?} app_id={:?} body={:?}",
                        base, app_id, snippet
                    )),
                    "/code" => flog(format!(
                        "注入脚本捕获授权 code（长度 {}）base={:?}",
                        code.as_ref().map(|c| c.len()).unwrap_or(0),
                        base
                    )),
                    "/sysconfig-fail" => flog(format!(
                        "sys/config 轮询耗尽仍无有效 app_id（或非 JSON）: bases={:?} lastBody={:?}",
                        base, snippet
                    )),
                    "/base" => flog(format!("注入脚本上报 base={base:?}")),
                    "/accesscode" => flog(format!(
                        "检测到访问码网关页并已填码提交: {}",
                        base.as_deref().unwrap_or("")
                    )),
                    "/autoclick" => flog(format!(
                        "已点击 /signin 授权按钮: {}",
                        base.as_deref().unwrap_or("")
                    )),
                    "/autosign-fail" => flog("未能找到 /signin 授权按钮（40 次轮询耗尽）"),
                    "/authorize-fail" => flog(format!(
                        "/oauthapi/authorize 响应未含 code: {:?}",
                        snippet
                    )),
                    other => flog(format!("未知信号: {other}")),
                }
                return false; // 伪导航，取消
            }

            // ② 授权后 URL 直带回跳 code（不经过 XHR hook 的场景）
            if path.contains("/oauth/result") {
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
                    flog("拦截 /oauth/result 回跳，已从 URL 取到 code");
                } else {
                    flog("拦截 /oauth/result 回跳，但 URL 无 code 参数");
                }
                return false;
            }

            // ③ 其余导航一律放行（含 /signin 授权页、中继子域、5ddd 门户）
            flog(format!("放行导航: {}", u.as_str()));
            true
        });

    // 与主窗口传同一套 additionalBrowserArgs（WebView2 一致性，红线同 webrs_webview）
    if let Some(args) = crate::webrs_webview::main_window_browser_args(&app) {
        b = b.additional_browser_args(&args);
    }
    b.build().map_err(|e| format!("创建 FN ID 登录窗口失败: {e}"))?;

    // 等待捕获（240s：中继隧道实测可达数十秒，留足余量）；窗口被手关即取消
    let deadline = Instant::now() + Duration::from_secs(240);
    let (mut base, code) = loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if app.get_webview_window(label).is_none() {
            flog("OAuth 窗口被关闭，取消登录");
            return Err("FN ID 登录已取消（窗口被关闭）".into());
        }
        let st = capture.lock().unwrap().clone();
        if let (Some(b), Some(c)) = (st.base, st.code) {
            flog(format!("已拿到 base={b} 与 code，开始换取 token"));
            break (b, c);
        }
        if Instant::now() > deadline {
            let st = capture.lock().unwrap().clone();
            flog(format!(
                "FN ID 登录超时 240s：base={:?} code={:?}（code 为空说明没走到 /signin 授权或未捕获 authorize 响应）",
                st.base, st.code.map(|c| c.len())
            ));
            if let Some(w) = app.get_webview_window(label) {
                let _ = w.destroy();
            }
            return Err(
                "FN ID 登录超时：未能通过官方中继完成授权（可稍后重试，或改用服务器地址直连）"
                    .into(),
            );
        }
    };

    // [访问码] 与账号密码登录一致：换 token 前先向 fnOS 网关预验证并建立会话，
    // 否则门禁会把 /v/api/v1/auth 拦成门户 HTML。被拒绝 → 明确报错；网络问题 → 继续（窗口内已填码）。
    if !access_code.is_empty() {
        match crate::access_code::establish(&base, &access_code).await {
            Ok(resolved) if !resolved.is_empty() => {
                if resolved != base {
                    flog(format!("访问码验证解析到真实源头: {base} → {resolved}"));
                    base = resolved;
                }
            }
            Ok(_) => {}
            Err(crate::access_code::AccessCodeError::Rejected) => {
                if let Some(w) = app.get_webview_window(label) {
                    let _ = w.destroy();
                }
                return Err("访问码错误，请检查后重试。".into());
            }
            Err(crate::access_code::AccessCodeError::Network(e)) => {
                flog(format!("访问码会话建立失败（继续 OAuth 流程）: {e}"));
            }
        }
    }

    // code 换 token（与账号密码登录同一条 fnapi 链）
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
        .map_err(|e| {
            flog(format!("授权码换取 token 失败: {e}"));
            format!("授权码换取 token 失败: {e}")
        })?;
    let token = data
        .pointer("/data/token")
        .and_then(|t| t.as_str())
        .or_else(|| data.get("token").and_then(|t| t.as_str()))
        .ok_or_else(|| {
            flog(format!("授权响应缺少 token: {data}"));
            format!("授权响应缺少 token: {data}")
        })?
        .to_string();
    flog("已获取 token");

    // 固化真实基址（80 → 实际 API 端口，同账号密码登录）
    let server = client.resolve_final_base(&base, &token).await;
    flog(format!("最终服务地址: {server}"));

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
    flog("FN ID 登录完成，配置已保存");
    Ok(json!({ "ok": true }))
}
