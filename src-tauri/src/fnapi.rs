//! 飞牛影视 API 客户端。对齐 Fntv-Plus `src/modules/fn_api/request.ts`：
//! - POST/PUT 在 body 末尾追加 nonce 防重放（签名对最终 body 字节计算）
//! - 鉴权双通道：Authorization 头 + Cookie: mode=relay; Trim-MC-token=<token>
//! - 3xx 手动跟随（重签新路径）；code==5000 invalid sign 重试；code!=0 视为业务错误
//! - 自签证书：内网 NAS 预期自签，PoC 直接 accept_invalid_certs（对齐 lc-286 私有地址免验）

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/* ───────────────────────── 网关 Cookie 会话 ─────────────────────────
 * AniBox 是纯 Rust HTTP 客户端（不像旧项目 Electron 有 persist:fntv 会话）。
 * fnOS 的「应用访问码」网关授权靠 Set-Cookie 建立，后续 /v/api/ 请求必须带上，
 * 否则会被门禁拦成门户 HTML。这里用进程内按 host 保存的简单 Cookie 表等价实现：
 *   · 每个响应（含 3xx）都调用 capture_cookies 落表；
 *   · 发请求时把同 host 的 Cookie 拼进 Cookie 头（Trim-MC-token / mode 由显式逻辑负责，
 *     这里跳过，避免重复与覆盖）。
 * 只保存 name=value，不落盘、不写日志。 */
fn cookie_store() -> &'static Mutex<BTreeMap<String, BTreeMap<String, String>>> {
    static STORE: OnceLock<Mutex<BTreeMap<String, BTreeMap<String, String>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn url_host(url: &str) -> Option<String> {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

/// 记录响应中的 Set-Cookie（按 host 保存）。敏感值只驻留内存。
pub(crate) fn capture_cookies(url: &str, headers: &reqwest::header::HeaderMap) {
    let Some(host) = url_host(url) else { return };
    let Ok(mut store) = cookie_store().lock() else { return };
    let entry = store.entry(host).or_default();
    for value in headers.get_all(reqwest::header::SET_COOKIE).iter() {
        let Ok(raw) = value.to_str() else { continue };
        let first = raw.split(';').next().unwrap_or("").trim();
        let Some(eq) = first.find('=') else { continue };
        let name = first[..eq].trim();
        let val = first[eq + 1..].trim();
        if name.is_empty() {
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        if val.is_empty() || lower.contains("max-age=0") {
            entry.remove(name);
        } else {
            entry.insert(name.to_string(), val.to_string());
        }
    }
}

/// 取该 URL 同 host 的 Cookie 串（不含显式鉴权字段）。
pub(crate) fn gateway_cookies(url: &str) -> String {
    let Some(host) = url_host(url) else { return String::new() };
    let Ok(store) = cookie_store().lock() else { return String::new() };
    let Some(map) = store.get(&host) else { return String::new() };
    map.iter()
        .filter(|(name, _)| {
            !name.eq_ignore_ascii_case("Trim-MC-token") && !name.eq_ignore_ascii_case("mode")
        })
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/* ───────────────────────── fnOS 端口自动探测 ─────────────────────────
 * 对齐旧项目 auth.ts detectFnOsPort：飞牛影视挂在 fnOS 系统 Web 端口 /v 下，
 * 系统默认 5666 / 5667，公测遗留 8000 / 8001，80 仅作重定向兜底。
 * 仅当用户裸填「私有地址且未带端口」时触发；命中返回端口，否则 None 回落手填。 */
fn is_private_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    if h == "localhost" || h == "127.0.0.1" || h == "::1" {
        return true;
    }
    let octets: Vec<u8> = h.split('.').filter_map(|p| p.parse::<u8>().ok()).collect();
    if octets.len() == 4 {
        return matches!(octets[0], 10 | 127)
            || (octets[0] == 172 && (16..=31).contains(&octets[1]))
            || (octets[0] == 192 && octets[1] == 168);
    }
    h.starts_with("fe80:") || h.starts_with("fc") || h.starts_with("fd")
}

async fn probe_port(client: &reqwest::Client, host: &str, port: u16) -> bool {
    let url = format!("http://{host}:{port}/v");
    let Ok(resp) = client.get(&url).send().await else { return false };
    if resp.status().as_u16() >= 500 {
        return false;
    }
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ct.is_empty() && !ct.contains("text/html") {
        return false;
    }
    let body = resp.text().await.unwrap_or_default();
    let lower = body.to_ascii_lowercase();
    lower.contains("fnos") || lower.contains("trim") || body.contains("飞牛")
}

pub async fn detect_fn_os_port(raw: &str) -> Option<u16> {
    let s = raw.trim().replace('：', ":");
    let s = s.trim_start_matches("http://").trim_start_matches("https://");
    let hostport = s.split('/').next().unwrap_or("");
    // 已带端口 / IPv6（含冒号）不探测
    if hostport.contains(':') || hostport.is_empty() || !is_private_host(hostport) {
        return None;
    }
    let candidates = [5666u16, 8000, 5667, 8001, 80];
    let Ok(client) = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_millis(1500))
        .build()
    else {
        return None;
    };
    let mut set = tokio::task::JoinSet::new();
    for port in candidates {
        let client = client.clone();
        let host = hostport.to_string();
        set.spawn(async move { (port, probe_port(&client, &host, port).await) });
    }
    let mut hit = std::collections::HashSet::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((port, true)) = joined {
            hit.insert(port);
        }
    }
    candidates.iter().copied().find(|p| hit.contains(p))
}

pub struct FnClient {
    /// [nas-subtitle] pub(crate)：字幕下载（/subtitle/dl 返回二进制）要复用同一套
    /// 客户端（默认头 x-trim-client / 超时 / 自签证书放行），call() 只回 JSON 不够用。
    pub(crate) http: reqwest::Client,
}

/// 服务器地址归一化：宽容处理用户输入（思路对齐主仓库 auth.ts）——
/// 全角冒号转半角、去引号/空白、无协议头自动补 http://、去掉端口后的路径。
/// 用户输入 `100.66.1.2`（裸 IP）即可用，等价 http://100.66.1.2（默认 80 端口）。
pub fn normalize_server(input: &str) -> String {
    let mut s = input.trim().to_string();
    s = s.replace('：', ":");
    s = s
        .trim_matches(|c: char| c == '"' || c == '\'' || c == ' ')
        .to_string();
    let lower = s.to_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        s = format!("http://{}", s);
    }
    if let Some(idx) = s.find("://") {
        let rest = &s[idx + 3..];
        if let Some(slash) = rest.find('/') {
            s = format!("{}//{}", &s[..idx + 3], &rest[..slash]);
        }
    }
    s
}

pub fn new_client() -> FnClient {
    // x-trim-client 头缺失时服务端会隐藏 IPTV 媒体库（飞鲸 auth_interceptor 同坑）
    let mut extra = reqwest::header::HeaderMap::new();
    extra.insert("x-trim-client", reqwest::header::HeaderValue::from_static("web"));
    extra.insert(
        "x-trim-client-version",
        reqwest::header::HeaderValue::from_static("616"),
    );
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(20))
        .default_headers(extra)
        .build()
        .expect("reqwest client build");
    FnClient { http }
}

/// 拆绝对 Location 为 (base, path)；相对 Location 返回 (base, location)
fn split_redirect(base: &str, location: &str) -> (String, String) {
    if let Some(idx) = location.find("://") {
        let after = &location[idx + 3..];
        match after.find('/') {
            Some(slash) => (format!("{}//{}", &location[..idx + 3], &after[..slash]), after[slash..].to_string()),
            None => (location.to_string(), "/".to_string()),
        }
    } else {
        (base.to_string(), location.to_string())
    }
}

/// GET 查询串的签名载荷。
///
/// 飞牛 `Authx` 的 dataMd5 规则（对齐 FlyNarwhal `auth_interceptor.dart`）：
/// - 有 JSON body → md5(实际发送的 body 序列化串)
/// - 无 body 但有 query → md5(按 key 升序拼接的 `k=v&k=v`，**用原始值，不做 URL 编码**)
/// - 都没有 → md5("")
///
/// 因此 GET 带参请求不能复用 `call()`（它按 md5("") 签名），需要单独带上这里的 sign。
#[derive(Clone)]
struct QuerySpec {
    /// 真正拼进 URL 的查询串（已 URL 编码，不含前导 `?`）
    url: String,
    /// 参与签名的原始 `k=v&k=v` 串（key 升序）
    sign: String,
}

fn build_query(query: &[(&str, String)]) -> QuerySpec {
    let mut pairs: Vec<(&str, &String)> = query.iter().map(|(k, v)| (*k, v)).collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    let sign = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let url = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    QuerySpec { url, sign }
}

impl FnClient {
    pub async fn call(
        &self,
        base: &str,
        path: &str,
        method: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let base = normalize_server(base);
        // [lc-114] 可重试判定：NAS 偶发返回 **200 + HTML**（被限流/网关错误页/弹回登录页），
        // 此时 JSON 解析失败。这属于临时故障而非参数错误 —— 短退避重试两次即可恢复；
        // 之前直接抛「HTTP 200 返回非 JSON 数据」，首页会连带把媒体库/条目全判失败。
        let mut last_err = String::new();
        for (i, backoff_ms) in [0u64, 400, 1200].iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(*backoff_ms)).await;
            }
            match self
                .call_inner(&base, path, method, token, body.clone(), None, 0)
                .await
            {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let retryable = e.contains("返回非 JSON") || e.contains("接口返回了网页");
                    if !retryable {
                        return Err(e);
                    }
                    last_err = e;
                }
            }
        }
        Err(format!("{last_err}（已重试 2 次，疑似 NAS 限流或会话被弹回登录页）"))
    }

    /// GET + 查询串（按飞牛规则对 query 签名）。用于 `/search/list?q=...` 这类
    /// 官方网页端以 GET 参数调用的只读接口。
    pub async fn call_get_query(
        &self,
        base: &str,
        path: &str,
        token: &str,
        query: &[(&str, String)],
    ) -> Result<Value, String> {
        let base = normalize_server(base);
        let spec = build_query(query);
        let mut last_err = String::new();
        for (i, backoff_ms) in [0u64, 400, 1200].iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(*backoff_ms)).await;
            }
            match self
                .call_inner(&base, path, "GET", token, None, Some(spec.clone()), 0)
                .await
            {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let retryable = e.contains("返回非 JSON") || e.contains("接口返回了网页");
                    if !retryable {
                        return Err(e);
                    }
                    last_err = e;
                }
            }
        }
        Err(format!("{last_err}（已重试 2 次，疑似 NAS 限流或会话被弹回登录页）"))
    }

    /// 跟随 3xx 探测最终基址：fnOS 80 端口会把 /v/api/v1/* 302 到实际端口（实测 :17777）。
    /// 重定向跨 host 时 reqwest/浏览器都会剥鉴权头，因此必须在登录/会话检查时把最终基址固化下来。
    pub async fn resolve_final_base(&self, base: &str, token: &str) -> String {
        let mut current = normalize_server(base);
        for _ in 0..4 {
            let url = format!("{}/v/api/v1/user/info", current);
            let mut cookie = format!("mode=relay; Trim-MC-token={}", token);
            let gateway = gateway_cookies(&url);
            if !gateway.is_empty() {
                cookie.push_str("; ");
                cookie.push_str(&gateway);
            }
            let resp = self
                .http
                .get(&url)
                .header("Authorization", token)
                .header("Cookie", cookie)
                .send()
                .await;
            let resp = match resp {
                Ok(r) => r,
                Err(_) => return current,
            };
            capture_cookies(&url, resp.headers());
            let st = resp.status().as_u16();
            if matches!(st, 301 | 302 | 307 | 308) {
                let loc = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                if loc.is_empty() {
                    return current;
                }
                let (nb, _np) = split_redirect(&current, &loc);
                current = nb;
            } else {
                return current;
            }
        }
        current
    }

    fn call_inner<'a>(
        &'a self,
        base: &'a str,
        path: &'a str,
        method: &'a str,
        token: &'a str,
        body: Option<Value>,
        query: Option<QuerySpec>,
        depth: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send + 'a>>
    {
        Box::pin(async move {
        if depth > 3 {
            return Err("重定向次数过多".into());
        }        // POST/PUT/PATCH 追加 nonce 并以最终序列化字符串为签名依据（字节级一致，见模块注释）
        // [lc-1629] 原来只认 POST —— 飞牛写接口里 `DELETE /scrap/:guid`（解除匹配）等非 GET 方法
        //   之前会被当成 GET 发出去（req 分支写死 post/get），必须按 method 正确构造。
        //   带 body 的方法 = POST/PUT/PATCH（飞牛约定它们都要 nonce）；GET 无 body。
        // [lc-1654] DELETE 也可能带 JSON body：飞牛 `DELETE /item/watched` 实测需要
        //   `{ item_guid }`，网页端同样对 DELETE 的 body 参与签名。仅在调用方显式传 body
        //   时才带，既有「无 body 的 DELETE」行为不变。
        let has_body = method.eq_ignore_ascii_case("post")
            || method.eq_ignore_ascii_case("put")
            || method.eq_ignore_ascii_case("patch")
            || (method.eq_ignore_ascii_case("delete") && body.is_some());
        let body_json: String = if has_body {
            let mut obj = body
                .as_ref()
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            obj.insert("nonce".into(), json!(crate::authx::rand6()));
            Value::Object(obj).to_string()
        } else {
            String::new()
        };

        // 签名依据：有 query 用 query 的原始 k=v 串，否则用 body（无 body 即 md5("")）。
        let (url, sign_data) = match &query {
            Some(q) if !q.url.is_empty() => (format!("{}{}?{}", base, path, q.url), q.sign.clone()),
            _ => (format!("{}{}", base, path), body_json.clone()),
        };
        let authx = crate::authx::authx(path, if sign_data.is_empty() { None } else { Some(sign_data.as_str()) });
        // 显式鉴权 Cookie + 网关授权 Cookie（访问码）合并；后者来自 Set-Cookie 会话表。
        let mut cookie = if token.is_empty() {
            "mode=relay".to_string()
        } else {
            format!("mode=relay; Trim-MC-token={}", token)
        };
        let gateway = gateway_cookies(&url);
        if !gateway.is_empty() {
            cookie.push_str("; ");
            cookie.push_str(&gateway);
        }

        // [lc-1629] 按 method 正确构造请求（原来只区分 post/get，PUT/DELETE 会被错发成 GET）。
        //   reqwest 的 post/put/patch/get/delete 都返回 RequestBuilder，故可统一 match。
        let mut req = match method.to_ascii_uppercase().as_str() {
            "POST" => self.http.post(&url).body(body_json),
            "PUT" => self.http.put(&url).body(body_json),
            "PATCH" => self.http.patch(&url).body(body_json),
            "DELETE" => self.http.delete(&url).body(body_json),
            _ => self.http.get(&url),
        };
        req = req
            .header("Content-Type", "application/json")
            .header("Authorization", token)
            .header("Cookie", cookie)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) anibox");
        // [authx-inject] 未注入飞牛签名材料时不加 Authx 头，请求照常发出（网关按未签名处理）。
        match authx {
            Some(a) => req = req.header("Authx", a),
            None => crate::libmpv::trace(&format!(
                "[fnapi] 未注入飞牛签名材料（ANIBOX_FN_AUTHX_KEY/SECRET），本次请求不带 Authx 头: {path}"
            )),
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_builder() {
                "服务器地址格式无效。示例：http://192.168.31.170:18888 或 100.66.1.2:18888（英文冒号；不带端口时默认 80）".to_string()
            } else if e.is_connect() {
                "无法连接服务器：请确认 NAS 地址与端口、本机与 NAS 网络互通（Tailscale 需已连接）、防火墙放行".to_string()
            } else if e.is_timeout() {
                "连接超时：网络不可达或 NAS 未响应".to_string()
            } else {
                format!("网络错误: {}", e)
            }
        })?;
        // 先记录 Set-Cookie（访问码网关授权 / 会话 Cookie），再处理状态与重定向
        capture_cookies(&url, resp.headers());
        let status = resp.status().as_u16();

        if matches!(status, 301 | 302 | 307 | 308) {
            let loc = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            if !loc.is_empty() {
                let (nb, np) = split_redirect(base, &loc);
                return self.call_inner(&nb, &np, method, token, body, query, depth + 1).await;
            }
        }

        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let text = resp.text().await.unwrap_or_default();
        if ct.contains("text/html") {
            return Err("接口返回了网页(HTML)而非 JSON——服务器地址/端口可能填错".into());
        }
        let v: Value =
            serde_json::from_str(&text).map_err(|_| format!("HTTP {} 返回非 JSON 数据", status))?;

        let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code == 5000 {
            // invalid sign：随机数/时序偶发冲突，重试一次（对齐 request.ts 重试逻辑）
            return self.call_inner(base, path, method, token, body, query, depth).await;
        }
        if code != 0 {
            // [lc-1654] 飞牛部分未实现路由回的是 `{"message":"Not Implemented..."}`（没有 `msg`），
            //   只读 `msg` 会把真实原因吞成「未知业务错误」，降级判定也就无从谈起。
            let msg = v
                .get("msg")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("未知业务错误")
                .to_string();
            return Err(msg);
        }
        Ok(v.get("data").cloned().unwrap_or(Value::Null))
        })
    }
}
