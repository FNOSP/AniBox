//! [lc-1810] 多云盘资源库 —— 网盘统一接入（LumiPlayer 2.2.1 `direct_cloud.rs` 逆向口径，
//! 端点经 `nianzhibai/91`（MIT）与本机实测核对）。
//!
//! 厂商子系统已按盘拆文件：`cloud_123`（K/V 签名 + 元数据缓存）、
//! `cloud_guangya`（设备码登录 + 列表 + 取链 + token 续期）。本文件保留调度面。
//!
//! | 厂商    | 登录 | 凭据 | 列目录 | 直链 |
//! |---------|------|------|--------|------|
//! | 115     | （复用 cloud115 模块，扫码→UID/CID/SEID） | cookie | aps.115.com/natsort/files.php | webapi files/download + 115.com/api/video/m3u8 转码 |
//! | 夸克    | uop.quark.cn cas/ajax getTokenForQrcodeLogin → getServiceTicketByQrcodeToken → su.quark.cn 换 Cookie | cookie | drive-pc.quark.cn/1/clouddrive/file/sort | **POST** file/v2/play（`trans_status=success`+`file_format=m3u8`）/ **POST** file/download `{fids}` |
//! | 百度    | passport getqrcode → channel/unicast 长轮询 → v3/login/main/qrbdusslogin 落 BDUSS/STOKEN | cookie | pan.baidu.com/api/list (dir=路径, app_id=250528) | rest/2.0/xpan/multimedia method=filemetas → dlink |
//! | 阿里    | passport.aliyundrive.com newlogin/qrcode/generate.do → query.do (bizExt base64) → api.aliyundrive.com/token/refresh | token | openapi.alipan.com/adrive/v1.0/openFile/list | openFile/getDownloadUrl → cdn_url |
//! | 123     | user.123pan.cn/api/user/qr-code/generate → qr-code/result | **Bearer token** | **POST** file/list/new（K/V 签名） | **POST** file/download_info（六项元数据 + 签名）→ 跳转链 |
//! | 光鸭    | OAuth 设备码 account.guangyapan.com/v1/auth/device/code → /v1/auth/token | access+refresh token | api.guangyapan.com/userres/v1/file/get_file_list | nd.bizuserres.s/v1/get_res_download_url → signedURL |
//!
//! 统一口径（对齐 LumiPlayer）：
//! - 凭据只落本机 `app_data_dir/direct-cloud-auth.json`（沿用 LumiPlayer 同名文件），绝不上传；
//! - 直链**每次点播现取现用**（各家链接都有 TTL，LumiPlayer 用本地续签代理保活，AniBox v1
//!   先「点播现取」，播放中途过期由用户重按播放键自然续链）；
//! - 列表条目统一 `{fileId, navId, name, isDir, size, kind}`：navId 是「进入下一层」用的导航键
//!   （百度=目录路径，其余=目录 id），fileId 是「取直链」用的文件 id；
//! - 播放与 115/直播同口径：不走 Go 代理、不回传飞牛进度、不落本机播放历史。

use base64::Engine;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 桌面 Chrome UA（LumiPlayer direct_cloud 同款）。
const UA_CHROME: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36";

/// 123 网页版固定头（LumiPlayer：platform=web App-Version=3）。
fn headers_123() -> Vec<(&'static str, String)> {
    vec![("platform", "web".into()), ("App-Version", "3".into())]
}

/// 阿里 openapi 固定 Canary 头（LumiPlayer：client=web,app=adrive,version=v6.8.12）。
const ALI_CANARY: &str = "client=web,app=adrive,version=v6.8.12";

// ───────────────────────── 凭据仓（app_data_dir/direct-cloud-auth.json） ─────────────────────────

/// [lc-1811] app_data_dir 全局快照：续签代理在 accept 循环里没有 AppHandle，
/// setup 时初始化一次，之后凭据读写全部走这里（路径永远一致）。
static APP_DATA_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

pub fn init_app_data(app: &AppHandle) {
    if let Some(d) = app.path().app_data_dir().ok() {
        let _ = APP_DATA_DIR.set(d);
    }
}

/// app_data_dir 全局快照读取（cloud115 的 Cookie 路径同源）。
pub fn app_data_dir() -> Option<std::path::PathBuf> {
    APP_DATA_DIR.get().cloned()
}

fn auth_path() -> Option<std::path::PathBuf> {
    APP_DATA_DIR.get().map(|d| d.join("direct-cloud-auth.json"))
}

fn load_auth() -> serde_json::Map<String, Value> {
    let Some(p) = auth_path() else { return Default::default() };
    let Ok(s) = std::fs::read_to_string(p) else { return Default::default() };
    serde_json::from_str::<Value>(&s)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

pub(crate) fn save_provider_auth(provider: &str, cred: Value) -> Result<(), String> {
    let p = auth_path().ok_or("app_data_dir 未初始化")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let mut all = load_auth();
    all.insert(provider.to_string(), cred);
    let s = serde_json::to_string_pretty(&Value::Object(all.clone()))
        .map_err(|e| format!("序列化凭据失败: {e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写凭据文件失败: {e}"))
}

fn drop_provider_auth(provider: &str) {
    if let Some(p) = auth_path() {
        let mut all = load_auth();
        all.remove(provider);
        if let Ok(s) = serde_json::to_string_pretty(&Value::Object(all)) {
            let _ = std::fs::write(&p, s);
        }
    }
}

pub(crate) fn require_cred(provider: &str, keys: &[&str]) -> Result<Value, String> {
    let all = load_auth();
    let Some(c) = all.get(provider).cloned() else {
        return Err(format!("未登录（请先扫码）"));
    };
    for k in keys {
        let v = c.get(*k).and_then(|x| x.as_str()).unwrap_or("");
        if v.is_empty() {
            return Err(format!("登录凭据不完整（缺 {k}），请重新扫码"));
        }
    }
    Ok(c)
}

// ───────────────────────── HTTP ─────────────────────────

/// 通用 HTTP client（光鸭等模块复用；UA 是桌面 Chrome）。
pub(crate) fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(UA_CHROME)
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("cloud http client")
    })
}

fn client_noredir() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(UA_CHROME)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("cloud noredir http client")
    })
}

async fn get_json(url: &str, hdrs: &[(String, String)]) -> Result<Value, String> {
    let mut req = client().get(url);
    for (k, v) in hdrs {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("响应读取失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("上游 HTTP {}", status.as_u16()));
    }
    // [lc-1902] 百度 channel/unicast 等接口回的是 JSONP（`bd__cbs__xxx({...})`），
    // 这里必须和 post_form 走同一套剥壳逻辑，否则整条扫码轮询永远解析失败。
    jsonp_or_json(&text)
}

/// 带自定义超时的 GET（百度 channel/unicast 是 30s 长轮询，默认 20s 全局超时会把它掐断）。
async fn get_json_timeout(
    url: &str,
    hdrs: &[(String, String)],
    timeout: Duration,
) -> Result<Value, String> {
    let mut req = client().get(url).timeout(timeout);
    for (k, v) in hdrs {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("响应读取失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("上游 HTTP {}", status.as_u16()));
    }
    jsonp_or_json(&text)
}

/// 是否为客户端侧超时（长轮询「没有新事件」在 HTTP 层就长这样，不能当错误处理）。
fn is_timeout(e: &str) -> bool {
    let l = e.to_ascii_lowercase();
    l.contains("timed out") || l.contains("timeout") || l.contains("超时")
}

async fn post_json(url: &str, body: Value, hdrs: &[(String, String)]) -> Result<Value, String> {
    let mut req = client()
        .post(url)
        .header("Content-Type", "application/json")
        .json(&body);
    for (k, v) in hdrs {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("响应读取失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("上游 HTTP {}", status.as_u16()));
    }
    serde_json::from_str(&text).map_err(|_| {
        format!("返回非 JSON：{}", text.chars().take(120).collect::<String>())
    })
}

async fn post_form(url: &str, form: &[(String, String)], hdrs: &[(String, String)]) -> Result<(Value, Vec<String>), String> {
    let mut req = client().post(url).header("Content-Type", "application/x-www-form-urlencoded");
    for (k, v) in hdrs {
        req = req.header(k.as_str(), v.as_str());
    }
    let body: Vec<String> = form.iter().map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v))).collect();
    let resp = req.body(body.join("&")).send().await.map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let cookies: Vec<String> = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok().map(|s| s.to_string()))
        .collect();
    let text = resp.text().await.map_err(|e| format!("响应读取失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("上游 HTTP {}", status.as_u16()));
    }
    let v = jsonp_or_json(&text)?;
    Ok((v, cookies))
}

/// 兼容 JSONP（bd__cbs__xxx({...})）与纯 JSON。
fn jsonp_or_json(text: &str) -> Result<Value, String> {
    let t = text.trim();
    if let Some(s) = t.strip_prefix('(') {
        if let Some(inner) = s.strip_suffix(");").or_else(|| s.strip_suffix(')')) {
            if let Ok(v) = serde_json::from_str::<Value>(inner) {
                return Ok(v);
            }
        }
    }
    if let (Some(a), Some(b)) = (t.find('('), t.rfind(')')) {
        if a < b {
            if let Ok(v) = serde_json::from_str::<Value>(&t[a + 1..b]) {
                return Ok(v);
            }
        }
    }
    serde_json::from_str(t).map_err(|_| format!("返回非 JSON：{}", t.chars().take(120).collect::<String>()))
}

/// 多键宽容取字符串（LumiPlayer 同思路：/a/b 指针优先，再平铺键名）。
fn g_str(v: &Value, pointers: &[&str], keys: &[&str]) -> String {
    for p in pointers {
        if let Some(s) = v.pointer(p).and_then(|x| x.as_str()) {
            return s.to_string();
        }
        if let Some(s) = v.pointer(p).and_then(|x| x.as_i64()).map(|n| n.to_string()) {
            return s;
        }
    }
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            return s.to_string();
        }
        if let Some(n) = v.get(*k).and_then(|x| x.as_i64()) {
            return n.to_string();
        }
    }
    String::new()
}

fn g_u64(v: &Value, keys: &[&str]) -> u64 {
    for k in keys {
        if let Some(n) = v.get(*k).and_then(|x| x.as_u64()) {
            return n;
        }
    }
    0
}

fn g_bool(v: &Value, keys: &[&str]) -> bool {
    for k in keys {
        match v.get(*k) {
            Some(Value::Bool(b)) => return *b,
            Some(Value::Number(n)) if n.as_i64() == Some(1) => return true,
            Some(Value::String(s)) if s == "1" || s == "true" => return true,
            _ => {}
        }
    }
    false
}

/// Set-Cookie 列表 → cookie 头串（取 name=value，丢弃属性；同名只留第一条）。
fn cookie_str(set_cookies: &[String], wanted: Option<&[&str]>) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in set_cookies {
        let pair = raw.split(';').next().unwrap_or("").trim().to_string();
        let Some((name, _)) = pair.split_once('=') else { continue };
        let name = name.trim();
        if let Some(w) = wanted {
            if !w.contains(&name) {
                continue;
            }
        }
        if out.iter().any(|c| c.split_once('=').map(|(n, _)| n) == Some(name)) {
            continue;
        }
        out.push(pair);
    }
    out.join("; ")
}

// ───────────────────────── 扫码会话 Cookie（简易 jar） ─────────────────────────

/// [lc-2231] 扫码链路必须把上游下发的会话 Cookie 带着走。
///
/// 两个真实的失败案例（2026-11-18 用户实测）：
/// - **夸克**：`getTokenForQrcodeLogin` 下发 `_UP_*` 会话 Cookie，后面的
///   `getServiceTicketByQrcodeToken` 与 `pan.quark.cn/account/info` 都要它。
///   不带 → 拿不到 service_ticket / 换不出 `__pus`；
/// - **百度**：`getqrcode` 下发 `BAIDUID`，`qrbdusslogin` 缺它就把 `v` 判成
///   310005「验证信息已过期」——表现正是「手机点了确认，桌面报错换不到 BDUSS」。
///
/// reqwest 的 `cookies` feature 没开（也不想为一个扫码流程引入 jar 依赖），
/// 这里就用手写的最小实现：只存 name=value，按上游返回顺序覆盖同名项。
/// 会话 Cookie 全部随 `QrState` 一起编码下发，**不进日志、不落盘**。
#[derive(Default, Clone)]
pub(crate) struct CookieJar(pub(crate) Vec<(String, String)>);

impl CookieJar {
    fn absorb(&mut self, set_cookies: &[String]) {
        for raw in set_cookies {
            let pair = raw.split(';').next().unwrap_or("").trim();
            let Some((name, value)) = pair.split_once('=') else { continue };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            match self.0.iter_mut().find(|(k, _)| k == name) {
                Some(slot) => slot.1 = value.trim().to_string(),
                None => self.0.push((name.to_string(), value.trim().to_string())),
            }
        }
    }

    /// 只要这些名字（顺序按 `wanted`），用于挑出品凭据。
    fn pick(&self, wanted: &[&str]) -> Vec<(String, String)> {
        wanted
            .iter()
            .filter_map(|w| {
                self.0
                    .iter()
                    .find(|(k, v)| k == w && !v.is_empty())
                    .map(|(k, v)| (k.clone(), v.clone()))
            })
            .collect()
    }

    fn header(&self) -> String {
        self.0
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn is_empty(&self) -> bool {
        self.0.iter().all(|(_, v)| v.is_empty())
    }

    fn to_json(&self) -> Value {
        Value::Array(
            self.0
                .iter()
                .map(|(k, v)| json!({ "name": k, "value": v }))
                .collect(),
        )
    }

    fn from_json(v: &Value) -> Self {
        let mut jar = CookieJar::default();
        if let Some(arr) = v.as_array() {
            for it in arr {
                let name = it.get("name").and_then(|x| x.as_str()).unwrap_or("");
                let value = it.get("value").and_then(|x| x.as_str()).unwrap_or("");
                if !name.is_empty() {
                    jar.0.push((name.to_string(), value.to_string()));
                }
            }
        }
        jar
    }
}

/// 收一条响应的 Set-Cookie 进 jar。
fn absorb_set_cookie(jar: &mut CookieJar, resp: &reqwest::Response) {
    let setc: Vec<String> = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok().map(|s| s.to_string()))
        .collect();
    jar.absorb(&setc);
}

/// 夸克 Cookie 轮换（实测：任意响应都可能轮换 `__puus` / `__pus`）。
/// 只替换这两个键的值，其余原样保留；不做合并就继续用旧串 → 播一会儿就 401。
pub(crate) fn merge_quark_cookies(current: &str, rotated: &[(String, String)]) -> Option<String> {
    if rotated.is_empty() {
        return None;
    }
    let mut parts: Vec<(String, String)> = current
        .split(';')
        .filter_map(|p| {
            let p = p.trim();
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let mut changed = false;
    for (name, value) in rotated {
        if value.is_empty() {
            continue;
        }
        match parts.iter_mut().find(|(k, _)| k == name) {
            Some(slot) => {
                if &slot.1 != value {
                    slot.1 = value.clone();
                    changed = true;
                }
            }
            None => {
                parts.push((name.clone(), value.clone()));
                changed = true;
            }
        }
    }
    if !changed {
        return None;
    }
    Some(
        parts
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// 夸克业务 POST：POST + JSON（GET 一律 405），顺带处理 Cookie 轮换并写回凭据仓。
async fn quark_post(path: &str, body: Value, cookies: &str) -> Result<Value, String> {
    let url = format!("https://drive-pc.quark.cn/1/clouddrive{path}?pr=ucpro&fr=pc");
    let resp = client()
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Cookie", cookies)
        .header("Referer", "https://pan.quark.cn/")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("夸克请求失败: {e}"))?;
    let status = resp.status();
    let rotated: Vec<(String, String)> = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .filter_map(|raw| {
            let pair = raw.split(';').next()?.trim();
            let (k, v) = pair.split_once('=')?;
            let k = k.trim();
            if ["__puus", "__pus", "__kp", "__ktd"].contains(&k) {
                Some((k.to_string(), v.trim().to_string()))
            } else {
                None
            }
        })
        .collect();
    if let Some(next) = merge_quark_cookies(cookies, &rotated) {
        persist_quark_cookies(&next);
    }
    let text = resp.text().await.map_err(|e| format!("夸克响应读取失败: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "夸克 HTTP {}（登录态可能失效）: {}",
            status.as_u16(),
            text.chars().take(160).collect::<String>()
        ));
    }
    serde_json::from_str(&text).map_err(|_| {
        format!("夸克返回非 JSON：{}", text.chars().take(120).collect::<String>())
    })
}

/// 轮换后的 Cookie 写回凭据仓（保留 userName / 其它字段）。
pub(crate) fn persist_quark_cookies(cookies: &str) {
    let all = load_auth();
    let mut cred = all.get("quark").cloned().unwrap_or_else(|| json!({}));
    if let Some(obj) = cred.as_object_mut() {
        obj.insert("cookies".to_string(), json!(cookies));
        obj.insert("saved_at_ms".to_string(), json!(now_ms()));
    }
    if let Err(e) = save_provider_auth("quark", cred) {
        crate::libmpv::trace(&format!("[quark] Cookie 轮换写盘失败: {e}"));
    }
}

/// 文本链接 → SVG 二维码 data URI（与 bili_qr 同款渲染；光鸭模块复用）。
pub(crate) fn qr_svg_data_uri(text: &str) -> Result<String, String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| format!("生成二维码失败: {e}"))?;
    let s = code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build();
    Ok(format!("data:image/svg+xml;base64,{}", STANDARD.encode(s.as_bytes())))
}

use base64::engine::general_purpose::STANDARD;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 光鸭等子模块需要的当前时间（避免每个模块各写一份）。
pub(crate) fn now_ms_pub() -> u64 {
    now_ms()
}

// ───────────────────────── 光鸭设备码状态串 ─────────────────────────

/// 光鸭登录状态串：device_code + 设备指纹 + 过期时间。
/// 设备 id 必须跟着 state 走 —— 确认 token 时上游按指纹绑定，换了指纹换不出 token。
#[derive(serde::Serialize, serde::Deserialize)]
struct GuangyaState {
    device_code: String,
    device_id: String,
    exp_ms: u64,
}

pub(crate) fn pack_guangya_state(device_code: &str, device_id: &str, ttl_ms: u64) -> String {
    let st = GuangyaState {
        device_code: device_code.to_string(),
        device_id: device_id.to_string(),
        exp_ms: now_ms() + ttl_ms,
    };
    STANDARD.encode(serde_json::to_string(&st).unwrap_or_default().as_bytes())
}

/// 解出 (device_code, device_id, 是否已过期)。光鸭的码只有 120s，
/// 过期本身就是正常轮询结果（status=3），不作为 Err 抛出。
pub(crate) fn unpack_guangya_state(s: &str) -> Result<(String, String, bool), String> {
    let raw = STANDARD
        .decode(s.trim())
        .map_err(|_| "光鸭登录状态无效，请刷新二维码".to_string())?;
    let st: GuangyaState =
        serde_json::from_slice(&raw).map_err(|_| "光鸭登录状态无效，请刷新二维码".to_string())?;
    Ok((st.device_code, st.device_id, now_ms() > st.exp_ms))
}

// ───────────────────────── 厂商清单 / 状态 ─────────────────────────

pub const PROVIDERS: &[(&str, &str)] = &[
    ("115", "115 网盘"),
    ("quark", "夸克网盘"),
    ("baidu", "百度网盘"),
    ("aliyun", "阿里云盘"),
    ("123", "123 云盘"),
    // [lc-2230] 光鸭：设备码登录（官方 web client_id），端点经 91（MIT）与实测核对
    ("guangyapan", "光鸭网盘"),
];

#[tauri::command]
pub async fn cloud_providers(app: AppHandle) -> Result<Value, String> {
    let all = load_auth();
    let mut out = Vec::new();
    for (id, label) in PROVIDERS {
        let (logged, user) = if *id == "115" {
            let s = crate::cloud115::cloud115_status(app.clone()).await.unwrap_or_default();
            (
                s.get("logged").and_then(|x| x.as_bool()).unwrap_or(false),
                s.get("userName").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            )
        } else {
            match all.get(*id) {
                Some(c) => (
                    true,
                    c.get("user_name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                ),
                None => (false, String::new()),
            }
        };
        out.push(json!({ "id": id, "label": label, "logged": logged, "userName": user }));
    }
    Ok(Value::Array(out))
}

/// [lc-1889] 登录页「内容来源」状态：飞牛是否已登录 + 已接入的网盘 + 是否配过 WebDAV。
/// 网盘优先改造后，启动路由据此决定「直接进首页（游客/网盘态）」还是「停在登录页」——
/// 只要任一来源可用就不再拿飞牛会话失效把整个 App 挡在门外。
#[tauri::command]
pub async fn cloud_sources_status(
    app: AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Value, String> {
    let fnos = !state.config.read().await.token.is_empty();
    let all = load_auth();
    let mut clouds = Vec::new();
    for (id, _) in PROVIDERS {
        let logged = if *id == "115" {
            crate::cloud115::cloud115_status(app.clone())
                .await
                .unwrap_or_default()
                .get("logged")
                .and_then(|x| x.as_bool())
                .unwrap_or(false)
        } else {
            all.contains_key(*id)
        };
        if logged {
            clouds.push(id.to_string());
        }
    }
    let webdav = !crate::webdav::load_accounts().is_empty();
    let any = fnos || !clouds.is_empty() || webdav;
    Ok(json!({ "fnos": fnos, "clouds": clouds, "webdav": webdav, "any": any }))
}

#[tauri::command]
pub async fn cloud_logout(app: AppHandle, provider: String) -> Result<(), String> {
    if provider == "115" {
        return crate::cloud115::cloud115_logout(app).await;
    }
    drop_provider_auth(&provider);
    Ok(())
}

/// [lc-2232] Cookie 手工登录（百度扫码链路的兜底，也是所有第三方百度工具的通用做法）。
///
/// 背景：百度扫码的 `qrbdusslogin` 对任何 v 都回 `310005`（实测空 v / 假 v / 真 v 同一个码），
/// 该码是通用失败码，无法据此定位协议差异；而 BaiduPCS-Go / BaiduPCS-Py 等成熟实现
/// **都只支持 Cookie 导入**。所以这里提供可验证的路径：用户从浏览器复制完整 Cookie 贴进来，
/// 我们调 `xpan/nas?method=uinfo` 验一次，通过才落盘。
///
/// Cookie 必须含 `BDUSS`；`STOKEN` 只有分享/秒传用得上，缺失不致命。
#[tauri::command]
pub async fn cloud_login_cookie(provider: String, cookies: String) -> Result<Value, String> {
    let raw = cookies.trim().to_string();
    if raw.is_empty() {
        return Err("Cookie 不能为空".into());
    }
    // 容错：用户常把整个 `Cookie: xxx` 头（带前缀）或 cURL 片段粘进来
    let cleaned = raw
        .strip_prefix("Cookie:")
        .or_else(|| raw.strip_prefix("cookie:"))
        .unwrap_or(&raw)
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();

    match provider.as_str() {
        "baidu" => {
            if !cleaned.contains("BDUSS=") {
                return Err("百度 Cookie 里没有 BDUSS=，请从 pan.baidu.com 的请求头完整复制".into());
            }
            // 验一次：uinfo 是最轻的鉴权接口，errno=0 才算真登录
            let v = get_json(
                "https://pan.baidu.com/rest/2.0/xpan/nas?method=uinfo&app_id=250528",
                &[
                    ("Cookie".into(), cleaned.clone()),
                    ("Referer".into(), "https://pan.baidu.com/disk/main".into()),
                ],
            )
            .await?;
            let errno = v.get("errno").and_then(|x| x.as_i64()).unwrap_or(-1);
            if errno != 0 {
                let msg = v.get("errmsg").and_then(|x| x.as_str()).unwrap_or("");
                return Err(format!(
                    "百度 Cookie 校验失败（errno={errno} {msg}）。请确认已登录 pan.baidu.com 且复制的是完整 Cookie 头"
                ));
            }
            let name = v
                .get("baidu_name")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| v.get("netdisk_name").and_then(|x| x.as_str()))
                .unwrap_or("百度网盘")
                .to_string();
            save_provider_auth(
                "baidu",
                json!({ "cookies": cleaned, "user_name": name, "saved_at_ms": now_ms() }),
            )?;
            crate::libmpv::trace("[lc-2232] baidu Cookie 登录成功");
            Ok(json!({ "ok": true, "userName": name }))
        }
        // 其它家保留扩展位：夸克/115 已能扫码，阿里也能扫码，不需要手工路径
        other => Err(format!("{other} 暂不支持 Cookie 手工登录（已有扫码登录）")),
    }
}

// ───────────────────────── 扫码登录（各厂商） ─────────────────────────

/// 扫码状态串（厂商上下文 + 过期时间），qr_start 生成、qr_poll 透传。
#[derive(serde::Serialize, serde::Deserialize)]
struct QrState {
    provider: String,
    data: Value,
    exp_ms: u64,
    /// [lc-2231] 扫码期间上游下发的会话 Cookie（夸克 `_UP_*`、百度 `BAIDUID`…）。
    /// 必须跟着状态串在 start → poll 之间传递：它们只活在这几十秒里，
    /// 丢掉就会「手机点了确认，桌面报错换不到凭据」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jar: Option<Value>,
}

/// 二维码有效期：与上游各家（115/夸克/百度/阿里/123）的 5 分钟一致。
/// [lc-1902] 115 模块复用同一常量，避免前后端「倒计时」和「状态串 TTL」两套数字。
pub(crate) const QR_TTL_MS: u64 = 5 * 60 * 1000;

/// 状态 3 = 二维码已过期（前端据此自动换一张，而不是把轮询打死）。
pub(crate) const QR_EXPIRED: i64 = 3;
/// [lc-1903] 状态串的宽限窗：本地 TTL 只是「展示用保守值」（上游各家真实有效期不一，
/// 阿里实测 15 分钟）。到点后仍允许再查 90s，由上游判定真过期（status=3）再换码——
/// 否则用户恰好在最后一刻点确认，码会被本地规则刷掉，表现为「手机确认了桌面没反应」。
pub(crate) const QR_GRACE_MS: u64 = 90 * 1000;
const QR_EXPIRED_MSG: &str = "二维码已过期，请刷新";

impl QrState {
    fn pack(&self) -> String {
        STANDARD.encode(serde_json::to_string(self).unwrap_or_default().as_bytes())
    }
    fn unpack(s: &str) -> Result<QrState, String> {
        let raw = STANDARD
            .decode(s.trim())
            .map_err(|_| "扫码状态无效，请刷新二维码".to_string())?;
        let st: QrState = serde_json::from_slice(&raw).map_err(|_| "扫码状态无效，请刷新二维码".to_string())?;
        if now_ms() > st.exp_ms.saturating_add(QR_GRACE_MS) {
            return Err(QR_EXPIRED_MSG.into());
        }
        Ok(st)
    }
}

#[tauri::command]
pub async fn cloud_qr_start(provider: String) -> Result<Value, String> {
    // [lc-2230] 光鸭是 OAuth 设备码（不是二维码图片接口），单独走一套；
    // 对外返回形状保持一致，前端不需要分支。
    if provider == "guangyapan" {
        return crate::cloud_guangya::qr_start().await;
    }
    let mut jar = CookieJar::default();
    let (data, qrcode, hint) = match provider.as_str() {
        // 夸克：getTokenForQrcodeLogin → data.members.token（实测 2026-11：
        // {status:2000000, data:{members:{token}}}，status 2000000 = OK）
        // [lc-2231] 这一步必须 **收下 `_UP_*` 会话 Cookie**：后面的 service_ticket
        // 查询与账号换取都要它，不带就是「扫了没反应」。
        "quark" => {
            let rid: String = (0..16).map(|_| (b'a' + rand::random::<u8>() % 26) as char).collect();
            let url = format!(
                "https://uop.quark.cn/cas/ajax/getTokenForQrcodeLogin?client_id=532&v=1.2&request_id={rid}"
            );
            let resp = client()
                .get(&url)
                .header("Accept", "application/json, text/plain, */*")
                .header("Origin", "https://pan.quark.cn")
                .header("Referer", "https://pan.quark.cn/")
                .send()
                .await
                .map_err(|e| format!("夸克二维码请求失败: {e}"))?;
            let status = resp.status();
            if !status.is_success() {
                return Err(format!("夸克二维码上游 HTTP {}", status.as_u16()));
            }
            let setc: Vec<String> = resp
                .headers()
                .get_all(reqwest::header::SET_COOKIE)
                .iter()
                .filter_map(|h| h.to_str().ok().map(|s| s.to_string()))
                .collect();
            jar.absorb(&setc);
            let text = resp.text().await.map_err(|e| format!("夸克响应读取失败: {e}"))?;
            let v = jsonp_or_json(&text)?;
            let token = g_str(&v, &["/data/members/token"], &["token"]);
            if token.is_empty() {
                return Err(format!("夸克二维码 token 获取失败: {v}"));
            }
            crate::libmpv::trace(&format!(
                "[lc-2231] quark qr_start: 会话 Cookie {} 条, token {} 字符",
                jar.0.len(),
                token.len()
            ));
            let su = format!(
                "https://su.quark.cn/4_eMHBJ?token={token}&client_id=532&ssb=weblogin&uc_param_str=&uc_biz_str=S%3Acustom%7COPT%3ASAREA%400%7COPT%3AIMMERSIVE%401%7COPT%3ABACK_BTN_STYLE%400"
            );
            (
                json!({ "token": token }),
                qr_svg_data_uri(&su)?,
                "打开夸克 App（或夸克网盘 App）扫一扫登录".to_string(),
            )
        }
        // 百度：getqrcode → imgurl（passport 直接出的二维码图）+ sign（channel_id）
        // [lc-2231] `BAIDUID` 会话 Cookie 必须收下：qrbdusslogin 缺它就把 v 判成
        // 310005「验证信息已过期」——用户看到的正是「手机点了确认，桌面报错」。
        "baidu" => {
            let resp = client()
                .get("https://passport.baidu.com/v2/api/getqrcode?lp=pc&qrloginfromapi=1&v3=1&tpl=netdisk")
                .header("Referer", "https://pan.baidu.com/")
                .send()
                .await
                .map_err(|e| format!("百度二维码请求失败: {e}"))?;
            let status = resp.status();
            if !status.is_success() {
                return Err(format!("百度二维码上游 HTTP {}", status.as_u16()));
            }
            let setc: Vec<String> = resp
                .headers()
                .get_all(reqwest::header::SET_COOKIE)
                .iter()
                .filter_map(|h| h.to_str().ok().map(|s| s.to_string()))
                .collect();
            // 只留 BAIDUID 系列：PASSID/UBI 是每次请求都新发的，留着反而撑大状态串
            for raw in &setc {
                let pair = raw.split(';').next().unwrap_or("").trim();
                if let Some((name, _)) = pair.split_once('=') {
                    if name.trim() == "BAIDUID" || name.trim() == "BAIDUID_BFESS" {
                        jar.absorb(std::slice::from_ref(raw));
                    }
                }
            }
            let text = resp.text().await.map_err(|e| format!("百度响应读取失败: {e}"))?;
            let v = jsonp_or_json(&text)?;
            let sign = g_str(&v, &["/sign"], &["sign"]);
            let imgurl = g_str(&v, &["/imgurl"], &["imgurl"]);
            if sign.is_empty() {
                return Err(format!("百度二维码获取失败: {v}"));
            }
            crate::libmpv::trace(&format!(
                "[lc-2231] baidu qr_start: 会话 Cookie {} 条（{}）",
                jar.0.len(),
                jar.0.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(",")
            ));
            let qr = if imgurl.starts_with("http") {
                imgurl
            } else if imgurl.starts_with("passport.") {
                // 实测 imgurl 可能不带协议头（passport.baidu.com/v2/api/qrcode?sign=…）
                format!("https://{imgurl}")
            } else {
                format!("https://passport.baidu.com/v2/api/qrcode?sign={sign}&lp=pc")
            };
            (
                json!({ "sign": sign }),
                qr,
                "打开百度网盘 App 扫一扫登录".to_string(),
            )
        }
        // 阿里：newlogin/qrcode/generate.do → codeContent（二维码内容文本，本地渲）
        // 实测响应 content.data = {t, codeContent, ck, resultCode, processFinished}
        "aliyun" => {
            let v = get_json(
                "https://passport.aliyundrive.com/newlogin/qrcode/generate.do?appName=aliyun_drive&fromSite=52&appEntrance=request",
                &[("Referer".into(), "https://www.alipan.com/".into())],
            )
            .await?;
            let url = g_str(&v, &["/content/data/codeContent", "/content/data/qrCodeUrl"], &["codeContent", "qrCodeUrl"]);
            let t = g_str(&v, &["/content/data/t"], &["t"]);
            let ck = g_str(&v, &["/content/data/ck"], &["ck"]);
            if url.is_empty() || t.is_empty() {
                return Err(format!("阿里云盘二维码获取失败: {v}"));
            }
            // [lc-2232] 诊断：阿里「刚出码就过期」的历史排查里，日志缺了最关键的一环 ——
            // 取码到底成没成功、t/ck 有没有拿到。这里记长度与指纹（不落原值）：
            //   · t 是**数字**（如 1795009644916），长度 13；拿到 0 长度说明字段层级变了；
            //   · ck 是 32 位 hex。
            crate::libmpv::trace(&format!(
                "[lc-2232] aliyun qr_start: t={}位 ck={}位 codeContent={} status={} resultCode={}",
                t.len(),
                ck.len(),
                if url.contains("qrcodeCheck") { "qrcodeCheck" } else { "其它" },
                v.pointer("/content/status").and_then(|x| x.as_i64()).map(|x| x.to_string()).unwrap_or("-".into()),
                v.pointer("/content/data/resultCode").and_then(|x| x.as_i64()).map(|x| x.to_string()).unwrap_or("-".into()),
            ));
            (
                json!({ "t": t, "ck": ck }),
                qr_svg_data_uri(&url)?,
                "打开阿里云盘 App 扫一扫登录".to_string(),
            )
        }
        // 123：qr-code/generate → {url, uniID}（实测 2026-10）；二维码内容 = url?uniID=
        "123" => {
            let mut hdrs: Vec<(String, String)> = headers_123()
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            hdrs.push(("Referer".into(), "https://user.123pan.cn/centerlogin".into()));
            let v = get_json("https://user.123pan.cn/api/user/qr-code/generate", &hdrs).await?;
            let uni_id = g_str(&v, &["/data/uniID", "/data/gid"], &["uniID", "gid"]);
            if uni_id.is_empty() {
                return Err(format!("123 云盘二维码获取失败: {v}"));
            }
            let page = g_str(&v, &["/data/url"], &["url"]);
            let content = if page.starts_with("http") {
                format!("{page}?uniID={uni_id}")
            } else {
                format!("https://yun.123pan.cn/wx-app-login.html?uniID={uni_id}")
            };
            (
                json!({ "uniID": uni_id }),
                qr_svg_data_uri(&content)?,
                "打开 123 云盘 App 扫一扫登录".to_string(),
            )
        }
        _ => return Err(format!("不支持的网盘: {provider}")),
    };
    let st = QrState {
        provider: provider.clone(),
        data,
        exp_ms: now_ms() + QR_TTL_MS,
        // [lc-2231] 会话 Cookie 随状态串走；空 jar 不写（状态串更短）
        jar: if jar.is_empty() { None } else { Some(jar.to_json()) },
    };
    Ok(json!({
        "qrcode": qrcode,
        "state": st.pack(),
        "hint": hint,
        // 前端据此显示倒计时并在到点时自动换码（与 QrState 同一个 TTL，不会各说各话）
        "expiresAtMs": st.exp_ms,
        "ttlMs": QR_TTL_MS,
    }))
}

/// 轮询扫码状态：status 0=待扫码 1=已扫待确认 2=已确认（凭据已落盘）。
#[tauri::command]
pub async fn cloud_qr_poll(_app: AppHandle, provider: String, state: String) -> Result<Value, String> {
    if provider == "115" {
        // 115 走自家模块，前端按旧三元组直调 cloud115_qr_poll；这里只为统一入口兜底
        return Ok(json!({ "status": 0, "ok": false }));
    }
    if provider == "guangyapan" {
        return crate::cloud_guangya::qr_poll(&state).await;
    }
    let st = match QrState::unpack(&state) {
        Ok(st) => st,
        // [lc-1902] 过期是正常状态而不是错误：作为 status=3 回给前端自动换码，
        // 不要抛 Err 把轮询打断（旧行为会停在「二维码已过期，请刷新」死等用户手点）。
        Err(e) if e == QR_EXPIRED_MSG => {
            return Ok(json!({ "status": QR_EXPIRED, "expired": true, "ok": false }))
        }
        Err(e) => return Err(e),
    };
    if st.provider != provider {
        return Err("扫码状态与厂商不匹配，请刷新二维码".into());
    }
    let jar = st.jar.as_ref().map(CookieJar::from_json).unwrap_or_default();
    let (status, user_name, cred) = poll_provider(&provider, &st.data, &jar).await?;
    // [lc-1903] 只打「有变化」的状态（status=0 待扫码每 2s 一拍，不刷屏）；不打凭据本身
    if status != 0 {
        crate::libmpv::trace(&format!("[lc-1903] qr_poll provider={provider} status={status}"));
    }
    if status == QR_EXPIRED {
        return Ok(json!({ "status": QR_EXPIRED, "expired": true, "ok": false }));
    }
    if status != 2 {
        return Ok(json!({ "status": status, "ok": false }));
    }
    let cred = cred.ok_or("扫码已确认但未取回凭据（内部状态不一致）")?;
    save_provider_auth(&provider, cred.clone())?;
    crate::libmpv::trace(&format!("[lc-1903] qr_poll provider={provider} 凭据已落盘"));
    let user_name = user_name.unwrap_or_default();
    Ok(json!({ "status": 2, "ok": true, "userName": user_name }))
}

/// 各厂商轮询核心：返回 (status, 用户名, 凭据 JSON)。
async fn poll_provider(
    provider: &str,
    d: &Value,
    jar: &CookieJar,
) -> Result<(i64, Option<String>, Option<Value>), String> {
    match provider {
        // 夸克：getServiceTicketByQrcodeToken → 确认后 `data.members.service_ticket`
        // （注意字段层级：**不是** `data.ticket`；未扫时 {status:50004001,"Query result is empty"}）
        // [lc-2231] 整条链都要带 start 阶段拿到的 `_UP_*` 会话 Cookie，否则上游不认票。
        "quark" => {
            let token = g_str(d, &["/token"], &["token"]);
            let rid: String = (0..16).map(|_| (b'a' + rand::random::<u8>() % 26) as char).collect();
            let url = format!(
                "https://uop.quark.cn/cas/ajax/getServiceTicketByQrcodeToken?client_id=532&v=1.2&token={}&request_id={rid}",
                urlencoding::encode(&token)
            );
            let resp = client()
                .get(&url)
                .header("Accept", "application/json, text/plain, */*")
                .header("Origin", "https://pan.quark.cn")
                .header("Referer", "https://pan.quark.cn/")
                .header("Cookie", jar.header())
                .send()
                .await
                .map_err(|e| format!("夸克轮询失败: {e}"))?;
            let text = resp.text().await.map_err(|e| format!("夸克响应读取失败: {e}"))?;
            let v = jsonp_or_json(&text)?;
            let status_code = v.get("status").and_then(|x| x.as_i64()).unwrap_or(-1);
            // 2000000 = 已确认；50004001 = 等待扫码/确认；50004002 = 过期
            if status_code == 50004002 {
                return Ok((QR_EXPIRED, None, None));
            }
            let ticket = g_str(&v, &["/data/members/service_ticket", "/data/ticket"], &["service_ticket", "ticket"]);
            if ticket.is_empty() {
                // [lc-2231] status=2000000 = 用户已确认，此时没有票就是异常：
                // 静默回「待扫码」会让界面一直转圈（旧实现就是这样，用户永远等不到结果）。
                if status_code == 2000000 {
                    crate::libmpv::trace(&format!(
                        "[lc-2231] quark 已确认(status=2000000)但取不到 service_ticket：{}",
                        text.chars().take(200).collect::<String>()
                    ));
                    return Err("夸克已确认登录，但上游未返回 service_ticket，请刷新二维码重试".into());
                }
                return Ok((0, None, None));
            }
            // 换凭据：pan.quark.cn/account/info?st=<service_ticket>（会话 Cookie 必须带上，
            // 上游才把 `__pus`/`__puus` 写进 Set-Cookie）
            let info_url = format!(
                "https://pan.quark.cn/account/info?st={}",
                urlencoding::encode(&ticket)
            );
            let info = client()
                .get(&info_url)
                .header("Accept", "application/json, text/plain, */*")
                .header("Referer", "https://pan.quark.cn/")
                .header("Cookie", jar.header())
                .send()
                .await
                .map_err(|e| format!("夸克账号换取失败: {e}"))?;
            let mut full_jar = jar.clone();
            absorb_set_cookie(&mut full_jar, &info);
            let info_text = info.text().await.map_err(|e| format!("夸克账号响应读取失败: {e}"))?;
            let info_v = jsonp_or_json(&info_text)?;
            if info_v.get("success").and_then(|x| x.as_bool()) != Some(true) {
                let msg = info_v.get("msg").and_then(|x| x.as_str()).unwrap_or("");
                let code = info_v.get("code").and_then(|x| x.as_str()).unwrap_or("");
                crate::libmpv::trace(&format!(
                    "[lc-2231] quark 账号换取被拒 code={code} msg={msg}（会话 Cookie {} 条）",
                    full_jar.0.len()
                ));
                return Err(format!("夸克账号换取失败（{code} {msg}），请重新扫码"));
            }
            // 品凭据 = `__pus` / `__puus`（其余 `_UP_*` 是登录过程用的，播业务用不到）
            let cred_pairs = full_jar.pick(&["__pus", "__puus", "__kp", "__ktd"]);
            if cred_pairs.is_empty() {
                let names: Vec<&str> = full_jar.0.iter().map(|(k, _)| k.as_str()).collect();
                crate::libmpv::trace(&format!(
                    "[lc-2231] quark 换取成功但缺 __pus/__puus（现有 {}）",
                    names.join(",")
                ));
                return Err("夸克账号换取成功，但未下发网盘凭据（__pus），请重新扫码".into());
            }
            let cookies = cred_pairs
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            crate::libmpv::trace(&format!(
                "[lc-2231] quark 凭据已取得（{}）",
                cred_pairs.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(",")
            ));
            let uname = info_v
                .pointer("/data/nickname")
                .and_then(|x| x.as_str())
                .or_else(|| info_v.pointer("/data/user_name").and_then(|x| x.as_str()))
                .unwrap_or("夸克网盘")
                .to_string();
            let cred = json!({ "cookies": cookies, "user_name": uname, "saved_at_ms": now_ms() });
            Ok((2, Some(uname), Some(cred)))
        }
        // 百度：channel/unicast 长轮询（JSONP）→ 确认后 qrbdusslogin 落 BDUSS/STOKEN
        "baidu" => {
            let sign = g_str(d, &["/sign"], &["sign"]);
            let tt = now_ms();
            let url = format!(
                "https://passport.baidu.com/channel/unicast?channel_id={sign}&tpl=netdisk&apiver=v3&tt={tt}&callback=bd__cbs__anibox&_={tt}"
            );
            // [lc-1902] 这个接口是 **30s 长轮询**（实测空轮询 30.6s 才回 `bd__cbs__anibox({"errno":1})`）：
            // ① 响应是 JSONP，必须剥壳（get_json 已统一走 jsonp_or_json）；
            // ② 默认 20s 超时会先把它掐死，所以单独放宽到 40s；
            // ③ 超时 = 这段时间没人扫码（正常空轮询），按「待扫码」继续下一轮，不能当错误中断。
            let v = match get_json_timeout(
                &url,
                &[
                    ("Referer".into(), "https://pan.baidu.com/".into()),
                    // [lc-2231] 会话 Cookie 必须带上：unicast 与 qrbdusslogin 都靠它
                    // 把「这次扫码」和 getqrcode 那次请求关联起来。
                    ("Cookie".into(), jar.header()),
                ],
                Duration::from_secs(40),
            )
            .await
            {
                Ok(v) => v,
                Err(e) if is_timeout(&e) => return Ok((0, None, None)),
                Err(e) => return Err(e),
            };
            let errno = v.get("errno").and_then(|x| x.as_i64()).unwrap_or(-1);
            if errno != 0 {
                // errno=1 = 这一轮没人扫码（长轮询超时/空轮询），属正常
                return Ok((0, None, None));
            }
            // [lc-1903] 真凶：`channel_v` 是 **JSON 字符串**（不是 JSON 对象），内容形如
            // `"{\"v\":\"<临时 bduss>\",\"status\":1,\"u\":null}"`。旧代码用
            // `v.pointer("/channel_v/v")` 直接在字符串里找子键，永远取不到 → 手机点了确认
            // 桌面依旧当「待扫码」，界面一直停在等待（本轮用户实测症状）。
            let has_channel = v
                .get("channel_v")
                .and_then(|x| x.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !has_channel {
                return Ok((0, None, None));
            }
            let Some(qrv) = baidu_confirm_v(&v) else {
                // errno=0 但没有 v = 已扫码未确认
                crate::libmpv::trace("[lc-1903] baidu poll: errno=0 无 v → 已扫待确认");
                return Ok((1, None, None));
            };
            crate::libmpv::trace(&format!(
                "[lc-1903] baidu poll: 已确认（v 长度 {}）→ qrbdusslogin 换 Cookie",
                qrv.len()
            ));
            // 已确认：qrbdusslogin 换 Cookie（Set-Cookie BDUSS/STOKEN/PTOKEN/bduss_bfess…）
            // [lc-1903] v 是 base64 风格临时票，可能含 `+` `/` `=`，必须 URL 编码后再拼 query，
            // 否则上游收到被截断的 v，一律回 310005「验证信息已过期」→ 表现为「确认了也登不上」。
            let login_url = format!(
                "https://passport.baidu.com/v3/login/main/qrbdusslogin?v={}&bduss=&u=https%3A%2F%2Fpan.baidu.com%2Fdisk%2Fmain&qrloginfromapi=1&tpl=netdisk&apiver=v3",
                urlencoding::encode(&qrv)
            );
            let resp = client_noredir()
                .get(&login_url)
                .header("Referer", "https://pan.baidu.com/")
                // [lc-2231] 真凶：**缺这个 Cookie 就被上游判 310005「验证信息已过期」**，
                // 哪怕 v 完全正确。实测对照（同一个假 v）：不带 BAIDUID → 5 条 Set-Cookie +
                // errInfo.no=310005；带 BAIDUID → 3 条，仍是 310005（因为票本身假的）。
                // 结论：BAIDUID 是必要条件，缺了必然失败；带上之后才轮到 v 是否有效。
                .header("Cookie", jar.header())
                .send()
                .await
                .map_err(|e| format!("百度 Cookie 兑换失败: {e}"))?;
            let setc: Vec<String> = resp
                .headers()
                .get_all(reqwest::header::SET_COOKIE)
                .iter()
                .filter_map(|h| h.to_str().ok().map(|s| s.to_string()))
                .collect();
            let cookies = cookie_str(
                &setc,
                Some(&["BDUSS", "bduss_bfess", "STOKEN", "PTOKEN", "PANSCPANWEB", "panweb"]),
            );
            if !cookies.contains("BDUSS=") {
                // [lc-2231] 把上游的拒绝理由打出来：以前只说「Set-Cookie N 条」，
                // 排查时完全看不出是票过期（310005）还是别的。响应体不是合法 JSON
                // （百度在 JSON 里塞了单引号键 + 尾逗号），所以这里只做子串提取。
                let body = resp.text().await.unwrap_or_default();
                let hint = if body.contains("310005") {
                    "310005 验证信息已过期（v 已被用过或超出时效）"
                } else if body.contains("310004") {
                    "310004 二维码已失效"
                } else if body.contains("3100") {
                    "上游判定验证信息无效"
                } else if body.is_empty() {
                    "响应体为空"
                } else {
                    "响应体里没有 BDUSS"
                };
                crate::libmpv::trace(&format!(
                    "[lc-2231] baidu qrbdusslogin 失败：{hint}（Set-Cookie {} 条: {}，会话 Cookie {} 条）",
                    setc.len(),
                    setc.iter()
                        .filter_map(|c| c.split('=').next())
                        .collect::<Vec<_>>()
                        .join(","),
                    jar.0.len()
                ));
                return Err(format!("百度扫码已确认，但换取登录凭据失败（{hint}），请重新扫码"));
            }
            crate::libmpv::trace("[lc-2231] baidu qrbdusslogin 成功，BDUSS 已写入");
            let cred = json!({ "cookies": cookies, "user_name": "百度网盘", "saved_at_ms": now_ms() });
            Ok((2, Some("百度网盘".into()), Some(cred)))
        }
        // 阿里：query.do → qrCodeStatus NEW/SCANED/CONFIRMED/EXPIRED；CONFIRMED 解 bizExt → refresh_token → token/refresh
        "aliyun" => {
            let t = g_str(d, &["/t"], &["t"]);
            let ck = g_str(d, &["/ck"], &["ck"]);
            // [lc-1902] **必须 POST + 表单体**（t/ck 放 body）。实测把 t/ck 拼在 query 上走 GET，
            // 上游一律秒回 `{"qrCodeStatus":"EXPIRED","status":-1}` —— 这就是用户看到的
            // 「二维码刚出来就过期」：不是有效期短，而是查询姿势不对。
            // [lc-2232] **真凶**：`appName/fromSite/appEntrance` 必须出现在 **POST body** 里。
            // 只用 query 传（旧做法）上游一律秒回 `qrCodeStatus=EXPIRED` —— 用户看到的就是
            // 「二维码刚出来就过期」。实测五组对照（2026-11-18）：
            //   GET+query            → EXPIRED
            //   POST 表单（仅 t/ck）  → EXPIRED
            //   POST 表单 + 带 cookie → EXPIRED
            //   POST 表单 + appName 等进 body → **NEW** ✅
            let (v, _) = post_form(
                "https://passport.aliyundrive.com/newlogin/qrcode/query.do",
                &[
                    ("t".into(), t),
                    ("ck".into(), ck),
                    ("appName".into(), "aliyun_drive".into()),
                    ("fromSite".into(), "52".into()),
                    ("appEntrance".into(), "request".into()),
                ],
                &[("Referer".into(), "https://www.alipan.com/".into())],
            )
            .await?;
            let status = g_str(&v, &["/content/data/qrCodeStatus"], &["qrCodeStatus"]);
            // [lc-2232] 诊断：以前这一支只在 status!=0 时打一行「status=3」，看不出上游到底回了什么。
            // 现在把原始判定依据打出来（不含凭据）：状态、status/-1 码、ck 是否轮换。
            // 注意：`hasError`/content.status 是上游给出的失败依据，排查「刚出码就过期」时必须看。
            if status != "NEW" {
                crate::libmpv::trace(&format!(
                    "[lc-2232] aliyun poll: qrCodeStatus={status} content.status={} hasError={} processFinished={}",
                    v.pointer("/content/status").and_then(|x| x.as_i64()).map(|x| x.to_string()).unwrap_or("-".into()),
                    v.get("hasError").and_then(|x| x.as_bool()).map(|x| x.to_string()).unwrap_or("-".into()),
                    v.pointer("/content/data/processFinished").and_then(|x| x.as_bool()).map(|x| x.to_string()).unwrap_or("-".into()),
                ));
            }
            match status.as_str() {
                "CONFIRMED" => {}
                "SCANED" => return Ok((1, None, None)),
                // 上游确实说过期：按 status=3 交给前端自动换码（不是错误）。
                // [lc-2232] 若这里频繁出现而用户还没扫，多半是 t/ck 与生成时不是一对
                // （见 start 分支的 trace：t 若为空/被当成字符串会直接换码失败）。
                "EXPIRED" => return Ok((QR_EXPIRED, None, None)),
                _ => return Ok((0, None, None)),
            }
            let biz = v.pointer("/content/data/bizExt").and_then(|x| x.as_str()).unwrap_or("");
            if biz.is_empty() {
                crate::libmpv::trace("[lc-1903] aliyun CONFIRMED 但响应里没有 bizExt");
                return Err("阿里云盘已确认，但上游未返回登录凭据（bizExt 缺失），请重新扫码".into());
            }
            let decoded = STANDARD
                .decode(biz.trim())
                .map_err(|e| format!("bizExt 解码失败: {e}"))?;
            let bv: Value = serde_json::from_slice(&decoded).map_err(|e| format!("bizExt 解析失败: {e}"))?;
            // [lc-1903] bizExt 解出来是 `pds_login_result`：正常给 refreshToken，部分版本/灰度
            // 直接给 accessToken + defaultDriveId（camelCase 与 snake_case 都出现过）。
            // 旧代码只认 `/pds_login_result/refreshToken`，字段一变就 Err → 前端显示「网络不稳定」。
            let pick = |camel: &str, snake: &str| -> String {
                let p1 = format!("/pds_login_result/{camel}");
                let p2 = format!("/pds_login_result/{snake}");
                g_str(&bv, &[p1.as_str(), p2.as_str()], &[camel, snake])
            };
            let refresh_token = pick("refreshToken", "refresh_token");
            let access_token = pick("accessToken", "access_token");
            let drive_id = pick("defaultDriveId", "default_drive_id");
            let nick = pick("nickName", "nick_name");
            let user_name = pick("userName", "user_name");
            let cred = if !access_token.is_empty() && !drive_id.is_empty() {
                // 上游已经把可用凭据直接给了，省掉一次 token/refresh（少一个失败点）
                crate::libmpv::trace("[lc-1903] aliyun CONFIRMED：bizExt 直接带 accessToken，跳过 token/refresh");
                json!({
                    "access_token": access_token,
                    "refresh_token": refresh_token,
                    "drive_id": drive_id,
                    "user_name": if !nick.is_empty() { nick } else if !user_name.is_empty() { user_name } else { "阿里云盘".to_string() },
                    "saved_at_ms": now_ms(),
                })
            } else if !refresh_token.is_empty() {
                crate::libmpv::trace("[lc-1903] aliyun CONFIRMED：bizExt 只带 refreshToken，走 token/refresh");
                aliyun_refresh(&refresh_token).await?
            } else {
                crate::libmpv::trace(&format!(
                    "[lc-1903] aliyun bizExt 无 access/refresh token（keys: {}）",
                    bv.pointer("/pds_login_result")
                        .and_then(|x| x.as_object())
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(","))
                        .unwrap_or_default()
                ));
                return Err("阿里云盘已确认，但凭据里没有可用 token，请重新扫码".into());
            };
            let user_name = cred.get("user_name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            Ok((2, Some(user_name), Some(cred)))
        }
        // 123：qr-code/result?uniID=（实测轮询端点在；data.loginStatus 判态，
        // 确认时响应携带 Set-Cookie）
        "123" => {
            let uni_id = g_str(d, &["/uniID", "/gid"], &["uniID"]);
            let url = format!("https://user.123pan.cn/api/user/qr-code/result?uniID={}", urlencoding::encode(&uni_id));
            let mut hdrs: Vec<(String, String)> = headers_123()
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            hdrs.push(("Referer".into(), "https://user.123pan.cn/centerlogin".into()));
            let mut req = client().get(&url);
            for (k, v) in &hdrs {
                req = req.header(k.as_str(), v.as_str());
            }
            let resp = req.send().await.map_err(|e| format!("123 轮询失败: {e}"))?;
            let setc: Vec<String> = resp
                .headers()
                .get_all(reqwest::header::SET_COOKIE)
                .iter()
                .filter_map(|h| h.to_str().ok().map(|s| s.to_string()))
                .collect();
            let text = resp.text().await.map_err(|e| format!("123 响应读取失败: {e}"))?;
            let v: Value = serde_json::from_str(&text)
                .unwrap_or_else(|_| json!({ "raw": text.chars().take(120).collect::<String>() }));
            let cookies = cookie_str(&setc, None);
            let status = v.pointer("/data/loginStatus").and_then(|x| x.as_i64()).unwrap_or(0);
            if !cookies.is_empty() && status == 3 {
                let user = g_str(&v, &["/data/userInfo/nickname", "/data/nickname"], &[]);
                let cred = json!({ "cookies": cookies, "user_name": if user.is_empty() { "123 云盘".to_string() } else { user }, "saved_at_ms": now_ms() });
                crate::libmpv::trace("[lc-1903] 123 已确认并拿到 Cookie");
                return Ok((2, Some(cred.get("user_name").and_then(|x| x.as_str()).unwrap_or("123 云盘").to_string()), Some(cred)));
            }
            // [lc-1903] 已确认但本次没带 Cookie：旧代码会落到 status=0（待扫码），界面就一直
            // 停在等待、什么都不发生。打点保留现场，并至少按「已扫待确认」上报。
            if status == 3 {
                crate::libmpv::trace("[lc-1903] 123 loginStatus=3 但本次响应无 Set-Cookie");
                return Ok((1, None, None));
            }
            // loginStatus：实测假 uniID = 4（无效/过期）；1=待扫 2=已扫待确认（按业界惯例）
            // [lc-1902] 4 是上游明确说的「无效/过期」，按 status=3 换码，别再当待扫码空转。
            if status == 4 {
                return Ok((QR_EXPIRED, None, None));
            }
            Ok((if status == 2 { 1 } else { 0 }, None, None))
        }
        _ => Err(format!("不支持的网盘: {provider}")),
    }
}

/// [lc-1903] 解析百度 `channel/unicast` 的 `channel_v`，取出「确认票」v。
///
/// 上游把 channel_v 当作**字符串**下发（双重嵌套 JSON）：
/// `{"errno":0,"channel_v":"{\"v\":\"<临时 bduss>\",\"status\":1,\"u\":null}"}`。
/// 返回 None = 还没确认（未扫 / 已扫待确认）。
fn baidu_confirm_v(v: &Value) -> Option<String> {
    let raw = v.get("channel_v").and_then(|x| x.as_str())?;
    if raw.trim().is_empty() {
        return None;
    }
    let cv: Value = serde_json::from_str(raw).ok()?;
    // 对象形态 {"v":..}；兼容历史上出现过的数组形态 [".."]
    let mut s = g_str(&cv, &["/v"], &["v"]);
    if s.is_empty() {
        s = cv
            .as_array()
            .and_then(|a| a.get(0))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
    }
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

// ───────────────────────── 文件浏览 ─────────────────────────

/// 统一列目录。cursor 语义各厂商不同（页号 / marker / offset），对前端透明：
/// 返回 nextCursor 为空串 = 没有更多。navId = 进入下一层的键（百度=目录路径）。
#[tauri::command]
pub async fn cloud_list(
    app: AppHandle,
    provider: String,
    fid: Option<String>,
    cursor: Option<String>,
) -> Result<Value, String> {
    let fid = fid.filter(|s| !s.is_empty()).unwrap_or_default();
    let cursor = cursor.filter(|s| !s.is_empty()).unwrap_or_default();
    match provider.as_str() {
        // 115：委托自家模块（cursor = offset）
        "115" => {
            let offset: u32 = cursor.parse().unwrap_or(0);
            let r = crate::cloud115::cloud115_list(app, Some(fid.clone()), Some(offset)).await?;
            let has_more = r.get("hasMore").and_then(|x| x.as_bool()).unwrap_or(false);
            let next_cursor = if has_more { (offset + 100).to_string() } else { String::new() };
            return Ok(json!({
                "items": r.get("items").cloned().unwrap_or(json!([])),
                "nextCursor": next_cursor,
                "count": r.get("count").cloned().unwrap_or(json!(0)),
            }));
        }
        // 夸克：file/sort（cursor = 页号，_page 从 1 起）
        "quark" => {
            let cred = require_cred("quark", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            let page: u32 = if cursor.is_empty() { 1 } else { cursor.parse().unwrap_or(1) };
            let url = format!(
                "https://drive-pc.quark.cn/1/clouddrive/file/sort?pr=ucpro&fr=pc&pdir_fid={}&_page={page}&_size=100&_fetch_total=1&_fetch_sub_dirs=0&_sort=file_type%3Aasc%2Cfile_name%3Aasc",
                urlencoding::encode(&fid)
            );
            let v = get_json(
                &url,
                &[
                    ("Cookie".into(), cookies.to_string()),
                    ("Referer".into(), "https://pan.quark.cn/".into()),
                ],
            )
            .await?;
            if v.pointer("/status").and_then(|x| x.as_i64()) != Some(200) && v.get("data").is_none() {
                return Err(format!("夸克列目录被拒: {v}"));
            }
            let list = v
                .pointer("/data/list")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            let next_marker = g_str(&v, &["/data/data/next_marker", "/data/list/data/next_marker"], &["next_marker"]);
            let next_cursor = if next_marker.is_empty() && list.len() >= 100 {
                (page + 1).to_string()
            } else {
                String::new()
            };
            Ok(json!({ "items": quark_items(&list), "nextCursor": next_cursor }))
        }
        // 百度：api/list（dir = 路径；cursor = 页号）
        "baidu" => {
            let cred = require_cred("baidu", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            let page: u32 = if cursor.is_empty() { 1 } else { cursor.parse().unwrap_or(1) };
            let dir = if fid.is_empty() { "/" } else { fid.as_str() };
            let url = format!(
                "https://pan.baidu.com/api/list?order=desc&showemptynum=1&channel=chunlei&clienttype=web&app_id=250528&web=1&dir={}&page={page}&num=100",
                urlencoding::encode(dir)
            );
            let v = get_json(
                &url,
                &[
                    ("Cookie".into(), cookies.to_string()),
                    ("Referer".into(), "https://pan.baidu.com/disk/main".into()),
                ],
            )
            .await?;
            let errno = v.get("errno").and_then(|x| x.as_i64()).unwrap_or(-1);
            if errno == -6 {
                return Err("百度登录已过期，请重新扫码".into());
            }
            if errno != 0 {
                return Err(format!("百度列目录失败 errno={errno}"));
            }
            let list = v.get("list").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let next_cursor = if list.len() >= 100 { (page + 1).to_string() } else { String::new() };
            Ok(json!({ "items": baidu_items(&list), "nextCursor": next_cursor }))
        }
        // 阿里：openFile/list（cursor = next_marker）
        "aliyun" => {
            let cred = require_cred("aliyun", &["access_token", "drive_id"])?;
            let token = cred.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let drive_id = cred.get("drive_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let parent = if fid.is_empty() { "root".to_string() } else { fid.clone() };
            let body = json!({
                "drive_id": drive_id,
                "parent_file_id": parent,
                "limit": 100,
                "next_marker": cursor,
                "order_by": "name",
                "order_direction": "ASC",
            });
            let (v, _token_new) = aliyun_list_call(&token, body).await?;
            let items = v.get("items").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let next_marker = g_str(&v, &["/next_marker"], &["next_marker"]);
            let next_cursor = if next_marker.is_empty() { String::new() } else { next_marker };
            let out: Vec<Value> = items
                .iter()
                .map(|it| {
                    let is_dir = g_str(it, &["/type"], &["type"]) == "folder";
                    let name = g_str(it, &["/name"], &["name"]);
                    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
                    let kind = if is_dir {
                        "dir"
                    } else if ["mkv", "mp4", "avi", "wmv", "flv", "mov", "iso", "m2ts", "webm", "ts", "rmvb", "mpg"].contains(&ext.as_str()) {
                        "video"
                    } else if ["mp3", "m4a", "aac", "wav", "ogg", "flac", "ape"].contains(&ext.as_str()) {
                        "audio"
                    } else {
                        "other"
                    };
                    json!({
                        "fileId": g_str(it, &["/file_id"], &["file_id"]),
                        "navId": g_str(it, &["/file_id"], &["file_id"]),
                        "name": name,
                        "isDir": is_dir,
                        "size": g_u64(it, &["size"]),
                        "kind": kind,
                    })
                })
                .collect();
            Ok(json!({ "items": out, "nextCursor": next_cursor }))
        }
        // 123：POST file/list/new（cursor = next）—— [lc-2230] 认证是 Bearer token + K/V 签名，
        // 不是 Cookie；列目录时顺手把每条文件的下载元数据灌进缓存，取链就不用再翻目录。
        "123" => {
            let parent = if fid.is_empty() { "0" } else { fid.as_str() };
            let page: u32 = if cursor.is_empty() { 1 } else { cursor.parse().unwrap_or(1) };
            let v = crate::cloud_123::p123_post_signed(
                "/file/list/new",
                json!({
                    "driveId": 0,
                    "limit": 100,
                    "next": "0",
                    "orderBy": "file_id",
                    "orderDirection": "desc",
                    "parentFileId": parent,
                    "trashed": false,
                    "SearchData": "",
                    "Page": page,
                    "OnlyLookAbnormalFile": 0,
                    "event": "homeListFile",
                    "operateType": 4,
                    "inDirectSpace": false,
                }),
            )
            .await?;
            crate::cloud_123::cache_from_list_response(&v);
            let list = v
                .pointer("/data/InfoList")
                .or_else(|| v.pointer("/data/fileList"))
                .or_else(|| v.pointer("/data/list"))
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            let total = v.pointer("/data/Total").and_then(|x| x.as_u64()).unwrap_or(0);
            let next = g_str(&v, &["/data/Next", "/data/next"], &["Next", "next"]);
            let next_cursor = if next.is_empty() || next == "-1" {
                String::new()
            } else if total > 0 && (page as u64) * 100 >= total {
                String::new()
            } else {
                (page + 1).to_string()
            };
            let out: Vec<Value> = list
                .iter()
                .map(|it| {
                    let is_dir = g_u64(it, &["Type", "type", "fileType"]) == 1
                        || g_str(it, &["/type"], &["type"]) == "folder";
                    let name = g_str(it, &["/FileName", "/Filename", "/filename", "/fileName", "/name"], &[]);
                    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
                    let kind = if is_dir {
                        "dir"
                    } else if ["mkv", "mp4", "avi", "wmv", "flv", "mov", "iso", "m2ts", "webm", "ts", "rmvb", "mpg"].contains(&ext.as_str()) {
                        "video"
                    } else if ["mp3", "m4a", "aac", "wav", "ogg", "flac", "ape"].contains(&ext.as_str()) {
                        "audio"
                    } else {
                        "other"
                    };
                    json!({
                        "fileId": g_str(it, &["/FileId", "/fileId", "/file_id"], &[]),
                        "navId": g_str(it, &["/FileId", "/fileId", "/file_id"], &[]),
                        "name": name,
                        "isDir": is_dir,
                        "size": g_u64(it, &["Size", "size", "fileSize"]),
                        "kind": kind,
                    })
                })
                .collect();
            Ok(json!({ "items": out, "nextCursor": next_cursor }))
        }
        // 光鸭：设备码登录 + 业务 API（token 续期在 cloud_guangya 内部）
        "guangyapan" => crate::cloud_guangya::list(&fid, &cursor).await,
        _ => Err(format!("不支持的网盘: {provider}")),
    }
}

fn quark_items(list: &[Value]) -> Vec<Value> {
    list.iter()
        .map(|it| {
            let is_dir = g_bool(it, &["dir"]);
            let name = g_str(it, &["/file_name"], &["file_name"]);
            let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let cat = g_str(it, &["/obj_category"], &["obj_category"]);
            let kind = if is_dir {
                "dir"
            } else if cat == "video" || ["mkv", "mp4", "avi", "wmv", "flv", "mov", "iso", "m2ts", "webm", "ts", "rmvb", "mpg"].contains(&ext.as_str()) {
                "video"
            } else if cat == "audio" || ["mp3", "m4a", "aac", "wav", "ogg", "flac", "ape"].contains(&ext.as_str()) {
                "audio"
            } else {
                "other"
            };
            json!({
                "fileId": g_str(it, &["/fid"], &["fid"]),
                "navId": g_str(it, &["/fid"], &["fid"]),
                "name": name,
                "isDir": is_dir,
                "size": g_u64(it, &["file_size", "size"]),
                "kind": kind,
            })
        })
        .collect()
}

fn baidu_items(list: &[Value]) -> Vec<Value> {
    list.iter()
        .map(|it| {
            let is_dir = g_bool(it, &["isdir", "is_dir", "dir"]);
            let name = g_str(it, &["/server_filename"], &["server_filename", "filename"]);
            let path = g_str(it, &["/path"], &["path"]);
            let category = it.get("category").and_then(|x| x.as_i64()).unwrap_or(0);
            let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let kind = if is_dir {
                "dir"
            } else if category == 1 || ["mkv", "mp4", "avi", "wmv", "flv", "mov", "iso", "m2ts", "webm", "ts", "rmvb", "mpg"].contains(&ext.as_str()) {
                "video"
            } else if category == 2 || ["mp3", "m4a", "aac", "wav", "ogg", "flac", "ape"].contains(&ext.as_str()) {
                "audio"
            } else {
                "other"
            };
            json!({
                "fileId": g_str(it, &["/fs_id"], &["fs_id"]),
                "navId": path,
                "name": name,
                "isDir": is_dir,
                "size": g_u64(it, &["size"]),
                "kind": kind,
            })
        })
        .collect()
}

// ───────────────────────── 阿里 token 续期 ─────────────────────────

/// refresh_token → 新 access_token（api.aliyundrive.com/token/refresh，LumiPlayer 口径）。
async fn aliyun_refresh(refresh_token: &str) -> Result<Value, String> {
    let v = post_json(
        "https://api.aliyundrive.com/token/refresh",
        json!({ "refresh_token": refresh_token }),
        &[
            ("X-Canary".into(), ALI_CANARY.into()),
            ("Referer".into(), "https://www.alipan.com/".into()),
        ],
    )
    .await?;
    let access = g_str(&v, &["/access_token"], &["access_token"]);
    let rt = g_str(&v, &["/refresh_token"], &["refresh_token"]);
    if access.is_empty() {
        return Err(format!("阿里 token 刷新被拒: {v}"));
    }
    let drive_id = g_str(&v, &["/default_drive_id"], &["default_drive_id"]);
    let user = g_str(&v, &["/user_name", "/nick_name"], &["user_name", "nick_name"]);
    Ok(json!({
        "access_token": access,
        "refresh_token": if rt.is_empty() { refresh_token.to_string() } else { rt },
        "drive_id": drive_id,
        "user_name": if user.is_empty() { "阿里云盘".to_string() } else { user },
        "saved_at_ms": now_ms(),
    }))
}

/// 阿里带鉴权调用：invalid access_token 时用凭据仓里的 refresh_token 续期重试一次，
/// 成功后把新凭据写回（下次调用直接用新 token）。
async fn aliyun_list_call(
    token: &str,
    body: Value,
) -> Result<(Value, String), String> {
    let hdrs = |t: &str| -> Vec<(String, String)> {
        vec![
            ("Authorization".into(), format!("Bearer {t}")),
            ("X-Canary".into(), ALI_CANARY.into()),
            ("Content-Type".into(), "application/json".into()),
            ("Referer".into(), "https://www.alipan.com/".into()),
        ]
    };
    match post_json("https://openapi.alipan.com/adrive/v1.0/openFile/list", body.clone(), &hdrs(token)).await {
        Ok(v) => Ok((v, token.to_string())),
        Err(e) if e.contains("401") || e.contains("AccessToken") || e.contains("invalid") => {
            let rt = load_auth()
                .get("aliyun")
                .and_then(|c| c.get("refresh_token"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if rt.is_empty() {
                return Err(e);
            }
            let cred = aliyun_refresh(&rt).await?;
            let new_token = cred.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if new_token.is_empty() {
                return Err(e);
            }
            let v = post_json("https://openapi.alipan.com/adrive/v1.0/openFile/list", body, &hdrs(&new_token)).await?;
            let _ = save_provider_auth("aliyun", cred);
            Ok((v, new_token))
        }
        Err(e) => Err(e),
    }
}

// ───────────────────────── 播放 ─────────────────────────

/// [lc-1811] 统一取播放地址：返回 (url, 是否 HLS, Referer, UA)。
/// `prefer_hls`：优先服务端转码（115 m3u8 / 夸克 v2/play / 阿里 live_transcoding）；
/// false 时优先直链。pub(crate) 供续签代理重签复用。
pub(crate) async fn resolve_play(
    provider: &str,
    file_id: &str,
    prefer_hls: bool,
) -> Result<(String, bool, String, String), String> {
    match provider {
        // 115：cloud115 模块的解析（直链 webapi download / 转码 115.com m3u8）。
        // [lc-1828] 直链模式加 Range 探活（bytes=0-0）：编码不兼容/限速的直链 CDN 可能
        // 返回异常——探测失败自动回退 m3u8（服务端转码永远兼容）。
        "115" => {
            // [lc-2230] 统一入口传进来的是 **fid**（列目录的 fileId），而 115 取链只认
            // **pickcode**（字段 pc）。旧代码直接拿 fid 当 pickcode 打，必然失败
            // （只有极老的「115 tab 直接传 pickcode」路径能通）。这里统一查一次。
            let jar = crate::cloud115::load_jar().ok_or("未登录 115（请先扫码）")?;
            let pickcode = crate::cloud115::pickcode_for(&jar, file_id).await?;
            let u = crate::cloud115::resolve_play_url(&jar, &pickcode, prefer_hls).await?;
            if prefer_hls {
                return Ok((u, true, "https://115.com/".into(), crate::cloud115::UA_115.to_string()));
            }
            // 直链探活：Range bytes=0-0，206/200 = 可用；非 2xx = 回退 m3u8
            let probe = client_noredir()
                .get(&u)
                .header("Range", "bytes=0-0")
                .header("Referer", "https://115.com/")
                .header("User-Agent", crate::cloud115::UA_115)
                .send()
                .await;
            let ok = matches!(&probe, Ok(r) if r.status().is_success() || r.status().as_u16() == 206);
            if ok {
                Ok((u, false, "https://115.com/".into(), crate::cloud115::UA_115.to_string()))
            } else {
                crate::libmpv::trace("[cloud] 115 直链探活失败，自动回退服务端转码 m3u8");
                let m3u8 = crate::cloud115::resolve_play_url(&jar, &pickcode, true).await?;
                Ok((m3u8, true, "https://115.com/".into(), crate::cloud115::UA_115.to_string()))
            }
        }
        // 夸克：prefer_hls → v2/play（video_list）；直链 → file/download
        // [lc-2230] 两个接口都是 **POST + JSON body**（实测 GET 一律 405 Method Not Allowed）；
        // 转码地址判定按 `trans_status=="success" && file_format=="m3u8"`，
        // 不能看 URL 里有没有 ".m3u8"（真实地址是 `video-play-*.drive.quark.cn/video?sign=...`）。
        "quark" => {
            let cred = require_cred("quark", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if prefer_hls {
                let v = quark_post(
                    "/file/v2/play",
                    json!({
                        "fid": file_id,
                        "resolutions": "normal,low,high,super,2k,4k",
                        "supports": "fmp4,m3u8",
                    }),
                    &cookies,
                )
                .await?;
                let mut best = String::new();
                if let Some(list) = v.pointer("/data/video_list").and_then(|x| x.as_array()) {
                    for it in list {
                        let status = it.get("trans_status").and_then(|x| x.as_str()).unwrap_or("");
                        let format = it
                            .pointer("/video_info/file_format")
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        let u = g_str(it, &["/video_info/url", "/url"], &["url"]);
                        if status == "success" && format == "m3u8" && !u.is_empty() {
                            best = u;
                        }
                    }
                }
                if best.is_empty() {
                    // 转码中/不可预览：把上游的转码状态原样带出来，别让用户以为是会员问题
                    let states: Vec<String> = v
                        .pointer("/data/video_list")
                        .and_then(|x| x.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|it| it.get("trans_status").and_then(|x| x.as_str()))
                                .map(|s| s.to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    let hint = if states.iter().any(|s| s == "transcoding" || s == "waiting" || s == "pending") {
                        "夸克正在转码，稍后再试（或先用「直链」档）"
                    } else {
                        "夸克未返回可播的转码 HLS（该文件可能不支持在线预览）"
                    };
                    return Err(hint.to_string());
                }
                // [lc-2230] 夸克转码 HLS **必须带账号 Cookie**，缺了上游回 412。
                // mpv 拿不到我们的 Cookie 会话，所以把它作为额外头下发。
                return Ok((
                    best,
                    true,
                    "https://pan.quark.cn/".into(),
                    UA_CHROME.into(),
                ));
            }
            // 直链：POST /file/download {fids:[id]} → data[0].download_url（不是 302）
            let v = quark_post("/file/download", json!({ "fids": [file_id] }), &cookies).await?;
            let url = v
                .pointer("/data/0/download_url")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if url.is_empty() {
                return Err(format!(
                    "夸克未返回可播地址（可能非会员或文件不可预览）: {}",
                    v.get("message").and_then(|x| x.as_str()).unwrap_or("")
                ));
            }
            Ok((url.to_string(), false, "https://pan.quark.cn/".into(), UA_CHROME.into()))
        }
        // 百度：xpan/multimedia filemetas → dlink（mpv 侧需带同款 UA + Referer）
        "baidu" => {
            let cred = require_cred("baidu", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            let url = format!(
                "https://pan.baidu.com/rest/2.0/xpan/multimedia?method=filemetas&fsids=[{}]&thumb=0&extra=1&app_id=250528",
                file_id
            );
            let v = get_json(
                &url,
                &[
                    ("Cookie".into(), cookies.to_string()),
                    ("Referer".into(), "https://pan.baidu.com/disk/main".into()),
                ],
            )
            .await?;
            let errno = v.get("errno").and_then(|x| x.as_i64()).unwrap_or(-1);
            if errno == -6 {
                return Err("百度登录已过期，请重新扫码".into());
            }
            if errno != 0 {
                return Err(format!("百度文件元数据被拒 errno={errno}"));
            }
            let dlink = v
                .pointer("/list/0/dlink")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if dlink.is_empty() {
                return Err("百度未返回 dlink".into());
            }
            Ok((dlink, false, "https://pan.baidu.com/".into(), UA_CHROME.into()))
        }
        // 阿里：直链 openFile/getDownloadUrl → cdn_url；prefer_hls → get_video_preview_play_info
        "aliyun" => {
            let cred = require_cred("aliyun", &["access_token", "drive_id"])?;
            let token = cred.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let drive_id = cred.get("drive_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let hdrs = vec![
                ("Authorization".to_string(), format!("Bearer {token}")),
                ("X-Canary".into(), ALI_CANARY.into()),
                ("Content-Type".into(), "application/json".into()),
                ("Referer".into(), "https://www.alipan.com/".into()),
            ];
            if prefer_hls {
                // [lc-1811] 服务端转码（LumiPlayer 口径：v2/file/get_video_preview_play_info
                // → live_transcoding 任务 → video_list[].url m3u8）
                let v = post_json(
                    "https://api.aliyundrive.com/v2/file/get_video_preview_play_info",
                    json!({ "drive_id": drive_id, "file_id": file_id, "category": "live_transcoding", "template_id": "" }),
                    &hdrs,
                )
                .await?;
                let mut best = String::new();
                if let Some(list) = v.pointer("/video_list").and_then(|x| x.as_array()) {
                    for it in list {
                        let u = g_str(it, &["/url"], &["url"]);
                        if u.contains(".m3u8") {
                            best = u;
                        }
                    }
                }
                if !best.is_empty() {
                    return Ok((best, true, "https://www.alipan.com/".into(), UA_CHROME.into()));
                }
                return Err("阿里未返回转码 HLS".into());
            }
            let v = post_json(
                "https://openapi.alipan.com/adrive/v1.0/openFile/getDownloadUrl",
                json!({ "drive_id": drive_id, "file_id": file_id }),
                &hdrs,
            )
            .await?;
            let url = g_str(&v, &["/cdn_url", "/url"], &["cdn_url", "url"]);
            if url.is_empty() {
                return Err(format!("阿里未返回可播地址: {v}"));
            }
            Ok((url, false, "https://www.alipan.com/".into(), UA_CHROME.into()))
        }
        // 123：POST file/download_info（GET 404）+ 文件元数据六项 + K/V 签名
        "123" => {
            let cred = require_cred("123", &["access_token"])?;
            let token = cred.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let meta = crate::cloud_123::p123_metadata(file_id).await?;
            let body = json!({
                "driveId": 0,
                "etag": meta.etag,
                "fileId": meta.numeric_id,
                "fileName": meta.name,
                "s3keyFlag": meta.s3key_flag,
                "size": meta.size,
                "type": 0,
            });
            let dl = crate::cloud_123::p123_post_signed("/file/download_info", body).await?;
            let raw = g_str(
                &dl,
                &["/data/DownloadUrl", "/data/downloadUrl", "/data/DownloadURL"],
                &[],
            );
            if raw.is_empty() {
                return Err(format!("123 未返回下载地址: {dl}"));
            }
            // 跳转链跟到底（Range bytes=0-0 探活 + Location 链）；params 里可能套一层 base64
            let first = crate::cloud_123::p123_unwrap_params(&raw);
            let mut cur = first;
            let mut final_url = cur.clone();
            for _ in 0..8 {
                let resp = client_noredir()
                    .get(&cur)
                    .header("Range", "bytes=0-0")
                    .header("Referer", "https://www.123pan.com/")
                    .header("User-Agent", crate::cloud_123::p123_ua())
                    .send()
                    .await
                    .map_err(|e| format!("123 直链跳转失败: {e}"))?;
                if let Some(loc) = resp.headers().get(reqwest::header::LOCATION).and_then(|l| l.to_str().ok()) {
                    if loc.starts_with("http") {
                        final_url = loc.to_string();
                        cur = final_url.clone();
                        continue;
                    }
                }
                final_url = cur;
                break;
            }
            Ok((final_url, false, "https://www.123pan.com/".into(), crate::cloud_123::p123_ua().to_string()))
        }
        // 光鸭：签名直链（signedURL 优先），链接不带账号头，可直接播
        "guangyapan" => {
            let u = crate::cloud_guangya::resolve_play(file_id).await?;
            Ok((u, false, String::new(), UA_CHROME.into()))
        }
        _ => Err(format!("不支持的网盘: {provider}")),
    }
}
pub(crate) async fn resolve_play_for_renew(
    provider: &str,
    file_id: &str,
) -> Result<(String, bool, String, String), String> {
    resolve_play(provider, file_id, false).await
}

/// [lc-1811] 播放三档（对齐 LumiPlayer 播放决策）：
/// - mode=""（默认·直链）：解析直链 → 注册续签代理 → mpv 拿稳定本地地址（过期自动换链）
/// - mode="server"（服务端转码）：115 m3u8 / 夸克 v2/play / 阿里 live_transcoding（CDN 直连）
/// - mode="local"（本地转码）：ffmpeg -c copy remux HLS（无服务端转码的源 + 系统有 ffmpeg）
#[tauri::command]
pub async fn cloud_play(
    app: AppHandle,
    state: tauri::State<'_, crate::AppState>,
    provider: String,
    file_id: String,
    name: String,
    mode: Option<String>,
) -> Result<Value, String> {
    if file_id.trim().is_empty() {
        return Err("fileId 为空".into());
    }
    let mode = mode.unwrap_or_default();
    let _ = app.emit("mpv-load", json!({ "phase": "start" }));
    let cfg = state.config.read().await.clone();
    crate::fnos_progress::begin("", "");

    let (url, headers) = match mode.as_str() {
        // 服务端转码：CDN 直连（m3u8 分段长效，无需续签代理）
        "server" => {
            let (u, _, referer, ua) = resolve_play(&provider, file_id.trim(), true).await?;
            let mut h = crate::player::HttpHeaders::new(Some(referer), Some(ua));
            // [lc-2230] 夸克转码 HLS 除签名外还要求账号 Cookie（缺了上游 412）。
            // mpv 直接向上游取清单/分片，只能把 Cookie 作为 http-header-fields 下发；
            // 其它家不需要账号头，别把凭据到处撒。
            if provider == "quark" {
                if let Ok(cred) = require_cred("quark", &["cookies"]) {
                    let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
                    if !cookies.is_empty() {
                        h.extra.push(("Cookie".to_string(), cookies.to_string()));
                    }
                }
            }
            (u, Some(h))
        }
        // 本地 ffmpeg remux HLS：注册会话（代理持有直链+续签）→ m3u8 端点给 mpv
        "local" => {
            let u = crate::cloud_proxy::register_hls(&provider, file_id.trim(), &name).await?;
            (u, None)
        }
        // 默认直链：续签代理（TTL 提前续 + 403 强制重签 = 播放中途换链）
        _ => {
            let u = crate::cloud_proxy::register_direct(&provider, file_id.trim(), &name).await?;
            (u, None)
        }
    };

    crate::player::launch(
        &app,
        &state,
        &cfg,
        &url,
        &name,
        0.0,
        0.0,
        headers.as_ref(),
        &[],
        None,
        true,
    )
    .await?;
    Ok(json!({ "url": url, "mode": mode }))
}

/// [lc-1811] 重命名（LumiPlayer rename 端点口径：夸克/阿里/115/百度/123；
/// 字符串池无 rename 证据的家不实现）。
#[tauri::command]
pub async fn cloud_rename(provider: String, file_id: String, new_name: String) -> Result<(), String> {
    let new_name = new_name.trim().to_string();
    if new_name.is_empty() {
        return Err("新名称不能为空".into());
    }
    if new_name.contains('/') || new_name.contains('\\') || new_name.contains('|') {
        return Err("新名称含有非法字符".into());
    }
    match provider.as_str() {
        // 夸克：file/rename {fid, file_name}
        "quark" => {
            let cred = require_cred("quark", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            let v = post_json(
                "https://drive-pc.quark.cn/1/clouddrive/file/rename?pr=ucpro&fr=pc",
                json!({ "fid": file_id, "file_name": new_name }),
                &[
                    ("Cookie".into(), cookies.to_string()),
                    ("Referer".into(), "https://pan.quark.cn/".into()),
                ],
            )
            .await?;
            if v.pointer("/data/fid").is_none() && v.get("data").is_none() {
                return Err(format!("夸克改名被拒: {v}"));
            }
            Ok(())
        }
        // 阿里：openFile/update {drive_id, file_id, name}
        "aliyun" => {
            let cred = require_cred("aliyun", &["access_token", "drive_id"])?;
            let token = cred.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let drive_id = cred.get("drive_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let v = post_json(
                "https://openapi.alipan.com/adrive/v1.0/openFile/update",
                json!({ "drive_id": drive_id, "file_id": file_id, "name": new_name, "check_name_mode": "refuse" }),
                &[
                    ("Authorization".into(), format!("Bearer {token}")),
                    ("X-Canary".into(), ALI_CANARY.into()),
                    ("Referer".into(), "https://www.alipan.com/".into()),
                ],
            )
            .await?;
            if v.get("name").and_then(|x| x.as_str()) != Some(new_name.as_str()) {
                return Err(format!("阿里改名未生效: {v}"));
            }
            Ok(())
        }
        // 115：files/batch_rename（files_new_name[0]={fid}|{new}）
        "115" => {
            let jar = crate::cloud115::load_jar().ok_or("未登录 115（请先扫码）")?;
            let body = format!(
                "files_new_name[0]={}%7C{}",
                urlencoding::encode(&file_id),
                urlencoding::encode(&new_name)
            );
            let resp = client()
                .post("https://webapi.115.com/files/batch_rename")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .header("Cookie", format!("UID={}; CID={}; SEID={}", jar.uid, jar.cid, jar.seid))
                .header("Referer", "https://115.com/")
                .body(body)
                .send()
                .await
                .map_err(|e| format!("115 改名请求失败: {e}"))?;
            let v: Value = resp.json().await.map_err(|e| format!("115 改名响应解析失败: {e}"))?;
            if v.get("state").and_then(|x| x.as_bool()) != Some(true) {
                return Err(format!("115 改名被拒: {}", v.get("error").and_then(|e| e.as_str()).unwrap_or("未知错误")));
            }
            Ok(())
        }
        // 百度：filemanager?method=rename（path + newname）
        "baidu" => {
            let cred = require_cred("baidu", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            let v = get_json(
                &format!(
                    "https://pan.baidu.com/api/filemanager?method=rename&app_id=250528&opera=rename&path={}&newname={}",
                    urlencoding::encode(&file_id),
                    urlencoding::encode(&new_name)
                ),
                &[
                    ("Cookie".into(), cookies.to_string()),
                    ("Referer".into(), "https://pan.baidu.com/disk/main".into()),
                ],
            )
            .await?;
            let errno = v.get("errno").and_then(|x| x.as_i64()).unwrap_or(-1);
            if errno != 0 {
                return Err(format!("百度改名被拒 errno={errno}"));
            }
            Ok(())
        }
        // 123：b/api/file/rename {fileId, fileName}（[lc-2230] Bearer + K/V 签名，非 Cookie）
        "123" => {
            let fid = file_id.parse::<i64>().unwrap_or(0);
            if fid == 0 {
                return Err("123 文件 id 非法".into());
            }
            let v = crate::cloud_123::p123_post_signed(
                "/file/rename",
                json!({ "fileId": fid, "fileName": new_name }),
            )
            .await?;
            if v.get("code").and_then(|x| x.as_i64()).unwrap_or(0) != 0 {
                return Err(format!("123 改名被拒: {v}"));
            }
            Ok(())
        }
        // 光鸭：/nd.bizuserres.s/v1/file/rename（91 同口径）
        "guangyapan" => {
            crate::cloud_guangya::rename(file_id.as_str(), new_name.as_str()).await?;
            Ok(())
        }
        _ => Err(format!("{provider} 暂不支持改名")),
    }
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_str_picks_names_once() {
        let setc = vec![
            "BDUSS=abc; path=/; domain=.baidu.com".into(),
            "BDUSS=dup; path=/".into(),
            "STOKEN=st1; HttpOnly".into(),
            "BAIDUID=junk".into(),
        ];
        let s = cookie_str(&setc, Some(&["BDUSS", "STOKEN"]));
        assert_eq!(s, "BDUSS=abc; STOKEN=st1");
    }

    #[test]
    fn g_str_pointer_and_keys() {
        let v = json!({ "data": { "token": 42, "name": "x" } });
        assert_eq!(g_str(&v, &["/data/token"], &[]), "42");
        // keys 兜底是「顶层平铺键名」（宽容各家响应包一层/不包一层）
        assert_eq!(g_str(&v, &["/data/name"], &[]), "x");
        let flat = json!({ "token": 7, "name": "y" });
        assert_eq!(g_str(&flat, &["/data/token", "/nope"], &["token"]), "7");
    }

    #[test]
    fn jsonp_parse() {
        let v = jsonp_or_json("bd__cbs__lumi123({\"errno\":0,\"x\":1})").unwrap();
        assert_eq!(v.get("errno").and_then(|x| x.as_i64()), Some(0));
        let v = jsonp_or_json("{\"a\":2}").unwrap();
        assert_eq!(v.get("a").and_then(|x| x.as_i64()), Some(2));
    }

    /// [lc-2231] 扫码会话 Cookie jar：同名覆盖、按名挑选、JSON 往返。
    /// 这条守的是「扫了没反应」的根因：会话 Cookie 必须能在 start→poll 之间传下去。
    #[test]
    fn qr_cookie_jar_roundtrip() {
        let mut jar = CookieJar::default();
        jar.absorb(&[
            "_UP_28A_52_=532; Path=/".to_string(),
            "_UP_D_=pc; Path=/; HttpOnly".to_string(),
            "BAIDUID=abc:FG=1; Expires=Wed, 01 Jan 2031 00:00:00 GMT".to_string(),
        ]);
        assert_eq!(jar.0.len(), 3);
        assert_eq!(jar.header(), "_UP_28A_52_=532; _UP_D_=pc; BAIDUID=abc:FG=1");

        // 同名必须覆盖（上游会轮换值），不能长出重复键
        jar.absorb(&["BAIDUID=rotated:FG=1; Path=/".to_string()]);
        assert_eq!(jar.0.len(), 3);
        assert_eq!(jar.header().matches("BAIDUID=").count(), 1);
        assert!(jar.header().contains("BAIDUID=rotated:FG=1"));

        // 只挑品凭据
        let picked = jar.pick(&["__pus", "__puus", "BAIDUID"]);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].0, "BAIDUID");

        // 空 jar 判定（状态串不该为它多塞一个空数组）
        assert!(CookieJar::default().is_empty());

        // JSON 往返：浏览器/悬空状态都靠它
        let back = CookieJar::from_json(&jar.to_json());
        assert_eq!(back.header(), jar.header());
    }

    /// 夸克的 service_ticket 在 `data.members.service_ticket`（不是 `data.ticket`）。
    /// 旧实现读错层级 → 手机确认了也永远当「待扫码」。
    #[test]
    fn quark_service_ticket_field_level() {
        let confirmed = json!({
            "status": 2000000,
            "data": { "members": { "service_ticket": "st-abc+/=" } }
        });
        assert_eq!(
            g_str(&confirmed, &["/data/members/service_ticket", "/data/ticket"], &["service_ticket", "ticket"]),
            "st-abc+/="
        );
        // 未扫码：无 data，取值必须为空（据此回 status=0）
        let waiting = json!({ "status": 50004001, "message": "Query result is empty" });
        assert!(g_str(&waiting, &["/data/members/service_ticket"], &["service_ticket"]).is_empty());
    }

    #[test]
    fn baidu_channel_v_parsing() {
        // 已确认：channel_v 是「JSON 字符串」，v 里可能带 + / =（必须 URL 编码后再用）
        let confirmed = json!({
            "errno": 0,
            "channel_v": "{\"v\":\"abc+/=\",\"status\":1,\"u\":null}"
        });
        assert_eq!(baidu_confirm_v(&confirmed).as_deref(), Some("abc+/="));
        // 已扫未确认：channel_v 存在但没有 v
        let scanned = json!({ "errno": 0, "channel_v": "{\"status\":1,\"u\":null}" });
        assert!(baidu_confirm_v(&scanned).is_none());
        // 兼容数组形态 [".."]
        let arr = json!({ "channel_v": "[\"xyz\"]" });
        assert_eq!(baidu_confirm_v(&arr).as_deref(), Some("xyz"));
        // 未扫码：errno=1，没有 channel_v
        assert!(baidu_confirm_v(&json!({ "errno": 1 })).is_none());
    }

    #[test]
    fn qr_state_roundtrip() {
        let mut jar = CookieJar::default();
        jar.absorb(&["BAIDUID=abc:FG=1".to_string()]);
        let st = QrState {
            provider: "quark".into(),
            data: json!({ "token": "t1" }),
            exp_ms: now_ms() + 1000,
            jar: Some(jar.to_json()),
        };
        let packed = st.pack();
        let back = QrState::unpack(&packed).unwrap();
        assert_eq!(back.provider, "quark");
        assert_eq!(back.data.get("token").and_then(|x| x.as_str()), Some("t1"));
        // [lc-2231] 会话 Cookie 必须穿过状态串（否则 poll 侧拿不到，扫了等于没扫）
        let back_jar = CookieJar::from_json(back.jar.as_ref().unwrap());
        assert_eq!(back_jar.header(), "BAIDUID=abc:FG=1");
        // 无 jar 的老/空状态串仍要能解（向后兼容）
        let no_jar = QrState {
            provider: "quark".into(),
            data: json!({}),
            exp_ms: now_ms() + 1000,
            jar: None,
        };
        assert!(QrState::unpack(&no_jar.pack()).unwrap().jar.is_none());
        let expired = QrState {
            provider: "quark".into(),
            data: json!({}),
            exp_ms: 1,
            jar: None,
        };
        assert!(QrState::unpack(&expired.pack()).is_err());
    }

    /// [lc-2232] Cookie 粘贴容错：用户常把整个 `Cookie: xxx` 头或带引号的串粘进来。
    /// 这条守的是「贴对了却说没 BDUSS」的体验问题。
    #[test]
    fn cookie_paste_normalization() {
        let norm = |raw: &str| -> String {
            raw.trim()
                .strip_prefix("Cookie:")
                .or_else(|| raw.strip_prefix("cookie:"))
                .unwrap_or(raw.trim())
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string()
        };
        // 裸 Cookie 值原样保留
        assert_eq!(norm("BDUSS=abc; STOKEN=def"), "BDUSS=abc; STOKEN=def");
        // 带 `Cookie:` 前缀（从请求头整行复制）
        assert_eq!(norm("Cookie: BDUSS=abc; STOKEN=def"), "BDUSS=abc; STOKEN=def");
        // 小写前缀
        assert_eq!(norm("cookie:BDUSS=abc"), "BDUSS=abc");
        // 外层引号（cURL / DevTools Copy as cURL 常见）
        assert_eq!(norm("\"BDUSS=abc\""), "BDUSS=abc");
        assert_eq!(norm("'BDUSS=abc'"), "BDUSS=abc");
        // 前后空白
        assert_eq!(norm("  BDUSS=abc  "), "BDUSS=abc");
    }

    #[test]
    fn item_mappers_shape() {
        let q = quark_items(&[json!({ "fid": "f1", "file_name": "a.mp4", "dir": false, "file_size": 5, "obj_category": "video" })]);
        assert_eq!(q[0].get("kind").and_then(|x| x.as_str()), Some("video"));
        let b = baidu_items(&[json!({ "fs_id": 7, "path": "/a", "server_filename": "剧", "isdir": 1 })]);
        assert_eq!(b[0].get("navId").and_then(|x| x.as_str()), Some("/a"));
        assert_eq!(b[0].get("isDir").and_then(|x| x.as_bool()), Some(true));
    }
}
