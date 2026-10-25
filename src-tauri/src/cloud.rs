//! [lc-1810] 多云盘资源库 —— 7 家网盘统一接入（LumiPlayer 2.2.1 `direct_cloud.rs` 逆向口径）。
//!
//! 端点证据来源：lumiplayer-tauri.exe 字符串池（导出于 `tests/lumi-ascii-2M.txt`），
//! 每个端点都能在字符串池里找到原文。各厂商一览：
//!
//! | 厂商    | 登录 | 凭据 | 列目录 | 直链 |
//! |---------|------|------|--------|------|
//! | 115     | （复用 cloud115 模块，扫码→UID/CID/SEID） | cookie | aps.115.com/natsort/files.php | webapi files/download + 115.com/api/video/m3u8 转码 |
//! | 夸克    | uop.quark.cn cas/ajax getTokenForQrcodeLogin → getServiceTicketByQrcodeToken → su.quark.cn 换 Cookie | cookie | drive-pc.quark.cn/1/clouddrive/file/sort (pr=ucpro&fr=pc) | file/v2/play（video_list m3u8）/ file/download (fids, 302) |
//! | 百度    | passport getqrcode → channel/unicast 长轮询 → v3/login/main/qrbdusslogin 落 BDUSS/STOKEN | cookie | pan.baidu.com/api/list (dir=路径, app_id=250528) | rest/2.0/xpan/multimedia method=filemetas → dlink |
//! | 阿里    | passport.aliyundrive.com newlogin/qrcode/generate.do → query.do (bizExt base64) → api.aliyundrive.com/token/refresh | token | openapi.alipan.com/adrive/v1.0/openFile/list | openFile/getDownloadUrl → cdn_url |
//! | 天翼    | open.e.189.cn logbox/oauth2 appConf→getUUID→image.do 二维码 → qrcodeLoginState.do → loginUrl 换 SESSION | cookie | cloud.189.cn/api/open/file/listFiles.action | api/open/file/batchGetFileDownloadUrl.action |
//! | 123     | user.123pan.cn/api/user/qr-code/generate → qr-code/result | cookie | api.123278.com/b/api/file/list/new | file/download_info → yun.123pan.cn/Origin 跳转链 |
//! | 光鸭    | account.guangyapan.com/v1/auth/device_code（OAuth 设备码，client_id=aMe-8VSlkrbQXpUR8） | token | api.guangyapan.com nd.biz/userres.s/v1/file/get_file_list | v1/get_res_download_url → signedURL |
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

fn save_provider_auth(provider: &str, cred: Value) -> Result<(), String> {
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

fn require_cred(provider: &str, keys: &[&str]) -> Result<Value, String> {
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

fn client() -> &'static reqwest::Client {
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
    serde_json::from_str(&text).map_err(|_| {
        format!("返回非 JSON：{}", text.chars().take(120).collect::<String>())
    })
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

/// 文本链接 → SVG 二维码 data URI（与 bili_qr 同款渲染）。
fn qr_svg_data_uri(text: &str) -> Result<String, String> {
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

// ───────────────────────── 厂商清单 / 状态 ─────────────────────────

pub const PROVIDERS: &[(&str, &str)] = &[
    ("115", "115 网盘"),
    ("quark", "夸克网盘"),
    ("baidu", "百度网盘"),
    ("aliyun", "阿里云盘"),
    ("tianyi", "天翼云盘"),
    ("123", "123 云盘"),
    ("guangya", "光鸭云盘"),
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

#[tauri::command]
pub async fn cloud_logout(app: AppHandle, provider: String) -> Result<(), String> {
    if provider == "115" {
        return crate::cloud115::cloud115_logout(app).await;
    }
    drop_provider_auth(&provider);
    Ok(())
}

// ───────────────────────── 扫码登录（各厂商） ─────────────────────────

/// 扫码状态串（厂商上下文 + 过期时间），qr_start 生成、qr_poll 透传。
#[derive(serde::Serialize, serde::Deserialize)]
struct QrState {
    provider: String,
    data: Value,
    exp_ms: u64,
}

impl QrState {
    fn pack(&self) -> String {
        STANDARD.encode(serde_json::to_string(self).unwrap_or_default().as_bytes())
    }
    fn unpack(s: &str) -> Result<QrState, String> {
        let raw = STANDARD
            .decode(s.trim())
            .map_err(|_| "扫码状态无效，请刷新二维码".to_string())?;
        let st: QrState = serde_json::from_slice(&raw).map_err(|_| "扫码状态无效，请刷新二维码".to_string())?;
        if now_ms() > st.exp_ms {
            return Err("二维码已过期，请刷新".into());
        }
        Ok(st)
    }
}

const QR_TTL_MS: u64 = 5 * 60 * 1000;

#[tauri::command]
pub async fn cloud_qr_start(provider: String) -> Result<Value, String> {
    let (data, qrcode, hint) = match provider.as_str() {
        // 夸克：getTokenForQrcodeLogin → data.members.token（实测 2026-10：
        // {status:2000000, data:{members:{token}}}，status 2000000 = OK）
        "quark" => {
            let rid: String = (0..16).map(|_| (b'a' + rand::random::<u8>() % 26) as char).collect();
            let v = get_json(
                &format!("https://uop.quark.cn/cas/ajax/getTokenForQrcodeLogin?client_id=532&v=1.2&request_id={rid}"),
                &[("Referer".into(), "https://pan.quark.cn/".into())],
            )
            .await?;
            let token = g_str(&v, &["/data/members/token"], &["token"]);
            if token.is_empty() {
                return Err(format!("夸克二维码 token 获取失败: {v}"));
            }
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
        "baidu" => {
            let v = get_json(
                "https://passport.baidu.com/v2/api/getqrcode?lp=pc&qrloginfromapi=1&v3=1&tpl=netdisk",
                &[("Referer".into(), "https://pan.baidu.com/".into())],
            )
            .await?;
            let sign = g_str(&v, &["/sign"], &["sign"]);
            let imgurl = g_str(&v, &["/imgurl"], &["imgurl"]);
            if sign.is_empty() {
                return Err(format!("百度二维码获取失败: {v}"));
            }
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
            (
                json!({ "t": t, "ck": ck }),
                qr_svg_data_uri(&url)?,
                "打开阿里云盘 App 扫一扫登录".to_string(),
            )
        }
        // 天翼：appConf.do（lt/paramId/REQID）→ getUUID.do → image.do?uuid= 直接出二维码图
        "tianyi" => {
            let (conf, _) = post_form(
                "https://open.e.189.cn/api/logbox/oauth2/appConf.do",
                &[("version".into(), "2.0".into()), ("appKey".into(), "cloudlt".into())],
                &[("Referer".into(), "https://cloud.189.cn/".into())],
            )
            .await?;
            let lt = g_str(&conf, &["/data/lt"], &["lt"]);
            let param_id = g_str(&conf, &["/data/paramId"], &["paramId"]);
            let req_id = g_str(&conf, &["/data/REQID", "/data/reqId"], &["REQID", "reqId"]);
            let uuid_resp = post_form(
                "https://open.e.189.cn/api/logbox/oauth2/getUUID.do",
                &[
                    ("appId".into(), "cloudlt".into()),
                    ("version".into(), "2.0".into()),
                ],
                &[("Referer".into(), "https://open.e.189.cn/".into())],
            )
            .await?;
            let uuid = g_str(&uuid_resp.0, &["/data/uuid"], &["uuid"]);
            if uuid.is_empty() {
                return Err(format!("天翼二维码 uuid 获取失败: {}", uuid_resp.0));
            }
            (
                json!({ "lt": lt, "paramId": param_id, "reqId": req_id, "uuid": uuid }),
                format!("https://open.e.189.cn/api/logbox/oauth2/image.do?uuid={uuid}"),
                "打开天翼云盘 App 扫一扫登录".to_string(),
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
        // 光鸭：OAuth 设备码 → verification_uri_complete 本地渲二维码
        "guangya" => {
            let v = post_json(
                "https://account.guangyapan.com/v1/auth/device/code",
                json!({ "client_id": "aMe-8VSlkrbQXpUR8" }),
                &[
                    ("x-client-id".into(), "aMe-8VSlkrbQXpUR8".into()),
                    ("x-client-version".into(), "0.0.1".into()),
                    ("Referer".into(), "https://www.guangyapan.com/".into()),
                ],
            )
            .await?;
            let device_code = g_str(&v, &["/device_code", "/data/device_code"], &["device_code"]);
            let verify = g_str(
                &v,
                &["/verification_uri_complete", "/data/verification_uri_complete"],
                &["verification_uri_complete"],
            );
            if device_code.is_empty() || verify.is_empty() {
                return Err(format!("光鸭云盘设备码获取失败: {v}"));
            }
            (
                json!({ "device_code": device_code }),
                qr_svg_data_uri(&verify)?,
                "打开光鸭云盘 App 扫一扫登录".to_string(),
            )
        }
        _ => return Err(format!("不支持的网盘: {provider}")),
    };
    let st = QrState { provider: provider.clone(), data, exp_ms: now_ms() + QR_TTL_MS };
    Ok(json!({ "qrcode": qrcode, "state": st.pack(), "hint": hint }))
}

/// 轮询扫码状态：status 0=待扫码 1=已扫待确认 2=已确认（凭据已落盘）。
#[tauri::command]
pub async fn cloud_qr_poll(_app: AppHandle, provider: String, state: String) -> Result<Value, String> {
    if provider == "115" {
        // 115 走自家模块，前端按旧三元组直调 cloud115_qr_poll；这里只为统一入口兜底
        return Ok(json!({ "status": 0, "ok": false }));
    }
    let st = QrState::unpack(&state)?;
    if st.provider != provider {
        return Err("扫码状态与厂商不匹配，请刷新二维码".into());
    }
    let (status, user_name, cred) = poll_provider(&provider, &st.data).await?;
    if status != 2 {
        return Ok(json!({ "status": status, "ok": false }));
    }
    let cred = cred.ok_or("扫码已确认但未取回凭据（内部状态不一致）")?;
    save_provider_auth(&provider, cred.clone())?;
    let user_name = user_name.unwrap_or_default();
    Ok(json!({ "status": 2, "ok": true, "userName": user_name }))
}

/// 各厂商轮询核心：返回 (status, 用户名, 凭据 JSON)。
async fn poll_provider(
    provider: &str,
    d: &Value,
) -> Result<(i64, Option<String>, Option<Value>), String> {
    match provider {
        // 夸克：getServiceTicketByQrcodeToken → 确认后 data.ticket（实测未扫时
        // {status:50004001,"Query result is empty"}，无 data）
        "quark" => {
            let token = g_str(d, &["/token"], &["token"]);
            let v = get_json(
                &format!(
                    "https://uop.quark.cn/cas/ajax/getServiceTicketByQrcodeToken?client_id=532&v=1.2&token={token}"
                ),
                &[("Referer".into(), "https://pan.quark.cn/".into())],
            )
            .await?;
            let ticket = g_str(&v, &["/data/ticket"], &["ticket"]);
            if ticket.is_empty() {
                // 已扫待确认 vs 待扫码：响应无稳定区分（state 字段实测不存在），一律待扫码
                return Ok((0, None, None));
            }
            // su.quark.cn 换 Cookie（302 链上挂 __pus/__puus 等，全部收下）
            let su = format!(
                "https://su.quark.cn/4_eMHBJ?token={ticket}&client_id=532&ssb=weblogin&uc_param_str=&uc_biz_str=S%3Acustom%7COPT%3ASAREA%400%7COPT%3AIMMERSIVE%401%7COPT%3ABACK_BTN_STYLE%400"
            );
            let resp = client_noredir()
                .get(&su)
                .header("Referer", "https://pan.quark.cn/")
                .send()
                .await
                .map_err(|e| format!("夸克 Cookie 兑换失败: {e}"))?;
            let mut setc: Vec<String> = resp
                .headers()
                .get_all(reqwest::header::SET_COOKIE)
                .iter()
                .filter_map(|h| h.to_str().ok().map(|s| s.to_string()))
                .collect();
            // 跟一跳 302（su → pan.quark.cn 落 Cookie 的链路）
            if let Some(loc) = resp.headers().get(reqwest::header::LOCATION).and_then(|l| l.to_str().ok()) {
                if loc.starts_with("http") {
                    if let Ok(r2) = client_noredir().get(loc).header("Referer", "https://pan.quark.cn/").send().await {
                        setc.extend(
                            r2.headers()
                                .get_all(reqwest::header::SET_COOKIE)
                                .iter()
                                .filter_map(|h| h.to_str().ok().map(|s| s.to_string())),
                        );
                    }
                }
            }
            let cookies = cookie_str(&setc, None);
            if cookies.is_empty() {
                return Err("夸克扫码确认但未拿到 Cookie".into());
            }
            let cred = json!({ "cookies": cookies, "user_name": "夸克网盘", "saved_at_ms": now_ms() });
            Ok((2, Some("夸克网盘".into()), Some(cred)))
        }
        // 百度：channel/unicast 长轮询（JSONP）→ 确认后 qrbdusslogin 落 BDUSS/STOKEN
        "baidu" => {
            let sign = g_str(d, &["/sign"], &["sign"]);
            let tt = now_ms();
            let url = format!(
                "https://passport.baidu.com/channel/unicast?channel_id={sign}&tpl=netdisk&apiver=v3&tt={tt}&callback=bd__cbs__anibox&_={tt}"
            );
            let v = get_json(&url, &[("Referer".into(), "https://pan.baidu.com/".into())]).await?;
            let errno = v.get("errno").and_then(|x| x.as_i64()).unwrap_or(-1);
            if errno != 0 {
                return Ok((0, None, None));
            }
            // channel_v.v 是 JSON 数组字符串 ["{v}"]；空 = 未确认
            let Some(raw) = v.pointer("/channel_v/v").and_then(|x| x.as_str()).map(|s| s.to_string()) else {
                return Ok((0, None, None));
            };
            let arr: Value = serde_json::from_str(&raw).unwrap_or(json!([]));
            let qrv = arr.get(0).and_then(|x| x.as_str()).unwrap_or("").to_string();
            if qrv.is_empty() {
                return Ok((1, None, None));
            }
            // 已确认：qrbdusslogin 换 Cookie（Set-Cookie BDUSS/STOKEN/PTOKEN/bduss_bfess…）
            let login_url = format!(
                "https://passport.baidu.com/v3/login/main/qrbdusslogin?v={qrv}&bduss=&u=https%3A%2F%2Fpan.baidu.com%2Fdisk%2Fmain&qrloginfromapi=1&tpl=netdisk"
            );
            let resp = client_noredir()
                .get(&login_url)
                .header("Referer", "https://pan.baidu.com/")
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
                return Err("百度扫码确认但未拿到 BDUSS".into());
            }
            let cred = json!({ "cookies": cookies, "user_name": "百度网盘", "saved_at_ms": now_ms() });
            Ok((2, Some("百度网盘".into()), Some(cred)))
        }
        // 阿里：query.do → qrCodeStatus NEW/SCANED/CONFIRMED/EXPIRED；CONFIRMED 解 bizExt → refresh_token → token/refresh
        "aliyun" => {
            let t = g_str(d, &["/t"], &["t"]);
            let ck = g_str(d, &["/ck"], &["ck"]);
            let mut url = format!(
                "https://passport.aliyundrive.com/newlogin/qrcode/query.do?appName=aliyun_drive&fromSite=52&appEntrance=request&t={t}"
            );
            if !ck.is_empty() {
                url.push_str(&format!("&ck={ck}"));
            }
            let v = get_json(&url, &[("Referer".into(), "https://www.alipan.com/".into())]).await?;
            let status = g_str(&v, &["/content/data/qrCodeStatus"], &["qrCodeStatus"]);
            match status.as_str() {
                "CONFIRMED" => {}
                "SCANED" => return Ok((1, None, None)),
                "EXPIRED" => return Err("二维码已过期，请刷新".into()),
                _ => return Ok((0, None, None)),
            }
            let biz = v.pointer("/content/data/bizExt").and_then(|x| x.as_str()).unwrap_or("");
            if biz.is_empty() {
                return Err("阿里扫码确认但缺 bizExt".into());
            }
            let decoded = STANDARD
                .decode(biz.trim())
                .map_err(|e| format!("bizExt 解码失败: {e}"))?;
            let bv: Value = serde_json::from_slice(&decoded).map_err(|e| format!("bizExt 解析失败: {e}"))?;
            let refresh_token = bv
                .pointer("/pds_login_result/refreshToken")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if refresh_token.is_empty() {
                return Err("bizExt 里没有 refreshToken".into());
            }
            let cred = aliyun_refresh(&refresh_token).await?;
            let user_name = cred.get("user_name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            Ok((2, Some(user_name), Some(cred)))
        }
        // 天翼：qrcodeLoginState.do 轮询 → 成功返回 loginUrl（跟随落 SESSION cookie）
        "tianyi" => {
            let lt = g_str(d, &["/lt"], &["lt"]);
            let param_id = g_str(d, &["/paramId"], &["paramId"]);
            let req_id = g_str(d, &["/reqId"], &["reqId"]);
            let uuid = g_str(d, &["/uuid"], &["uuid"]);
            let (v, setc) = post_form(
                "https://open.e.189.cn/api/logbox/oauth2/qrcodeLoginState.do",
                &[
                    ("REQID".into(), req_id.clone()),
                    ("lt".into(), lt.clone()),
                    ("paramId".into(), param_id.clone()),
                    ("appId".into(), "cloudlt".into()),
                    ("uuid".into(), uuid.clone()),
                ],
                &[("Referer".into(), "https://open.e.189.cn/".into())],
            )
            .await?;
            // 确认态：data.loginUrl 出现（或 state==9）。其余全部当待扫码/待确认。
            let login_url = g_str(&v, &["/data/loginUrl", "/loginUrl"], &["loginUrl"]);
            let state = g_str(&v, &["/data/state", "/state"], &["state"]);
            if login_url.is_empty() {
                let scanned = state == "1" || state == "2";
                return Ok((if scanned { 1 } else { 0 }, None, None));
            }
            // 跟 loginUrl 链收 cloud.189.cn 域 Cookie
            let mut setc_all = setc;
            let mut cur = login_url;
            for _ in 0..4 {
                let resp = client_noredir()
                    .get(&cur)
                    .header("Referer", "https://cloud.189.cn/")
                    .send()
                    .await
                    .map_err(|e| format!("天翼登录跳转失败: {e}"))?;
                setc_all.extend(
                    resp.headers()
                        .get_all(reqwest::header::SET_COOKIE)
                        .iter()
                        .filter_map(|h| h.to_str().ok().map(|s| s.to_string())),
                );
                match resp.headers().get(reqwest::header::LOCATION).and_then(|l| l.to_str().ok()) {
                    Some(loc) if loc.starts_with("http") => cur = loc.to_string(),
                    _ => break,
                }
            }
            let cookies = cookie_str(&setc_all, None);
            if cookies.is_empty() {
                return Err("天翼扫码确认但未拿到 Cookie".into());
            }
            let cred = json!({ "cookies": cookies, "user_name": "天翼云盘", "saved_at_ms": now_ms() });
            Ok((2, Some("天翼云盘".into()), Some(cred)))
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
                return Ok((2, Some(cred.get("user_name").and_then(|x| x.as_str()).unwrap_or("123 云盘").to_string()), Some(cred)));
            }
            // loginStatus：实测假 uniID = 4（无效/过期）；1=待扫 2=已扫待确认（按业界惯例）
            Ok((if status == 2 { 1 } else { 0 }, None, None))
        }
        // 光鸭：OAuth 设备码轮询 grant_type=urn:ietf:params:oauth:grant-type:device_code
        "guangya" => {
            let device_code = g_str(d, &["/device_code"], &["device_code"]);
            let v = post_json(
                "https://account.guangyapan.com/v1/auth/token",
                json!({
                    "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                    "device_code": device_code,
                    "client_id": "aMe-8VSlkrbQXpUR8"
                }),
                &[
                    ("x-client-id".into(), "aMe-8VSlkrbQXpUR8".into()),
                    ("x-client-version".into(), "0.0.1".into()),
                    ("Referer".into(), "https://www.guangyapan.com/".into()),
                ],
            )
            .await?;
            let err = g_str(&v, &["/error"], &["error"]);
            if !err.is_empty() {
                // OAuth 设备码没有「已扫未确认」态：pending/slow_down 都按待扫码报
                if err.contains("pending") || err.contains("slow") || err.contains("expired_token") {
                    return Ok((0, None, None));
                }
                return Err(format!("光鸭云盘登录被拒: {err}"));
            }
            let access = g_str(&v, &["/access_token"], &["access_token"]);
            if access.is_empty() {
                return Ok((0, None, None));
            }
            let cred = json!({
                "access_token": access,
                "refresh_token": g_str(&v, &["/refresh_token"], &["refresh_token"]),
                "user_name": "光鸭云盘",
                "saved_at_ms": now_ms(),
            });
            Ok((2, Some("光鸭云盘".into()), Some(cred)))
        }
        _ => Err(format!("不支持的网盘: {provider}")),
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
        // 天翼：listFiles.action（cursor = 页号；文件夹在 fileListAO.folderList）
        "tianyi" => {
            let cred = require_cred("tianyi", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            let page: u32 = if cursor.is_empty() { 1 } else { cursor.parse().unwrap_or(1) };
            let folder = if fid.is_empty() { "-11" } else { fid.as_str() };
            let url = format!(
                "https://cloud.189.cn/api/open/file/listFiles.action?folderId={}&mediaType=&pageNum={page}&pageSize=500&iconOption=5&fileName=&descending=true",
                urlencoding::encode(folder)
            );
            let v = get_json(
                &url,
                &[
                    ("Cookie".into(), cookies.to_string()),
                    ("Accept".into(), "application/json, text/plain, */*".into()),
                    ("X-Requested-With".into(), "XMLHttpRequest".into()),
                    ("Referer".into(), "https://cloud.189.cn/web/main/file/folder/".into()),
                ],
            )
            .await?;
            if v.pointer("/res_code").and_then(|x| x.as_i64()) == Some(-1) {
                return Err("天翼登录已过期，请重新扫码".into());
            }
            let mut out = Vec::new();
            for (arr, is_dir) in [
                (v.pointer("/fileListAO/folderList"), true),
                (v.pointer("/fileListAO/fileList"), false),
            ] {
                let Some(list) = arr.and_then(|x| x.as_array()) else { continue };
                for it in list {
                    let name = g_str(it, &["/name"], &["name"]);
                    if name.is_empty() {
                        continue;
                    }
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
                    out.push(json!({
                        "fileId": g_str(it, &["/id"], &["id"]),
                        "navId": g_str(it, &["/id"], &["id"]),
                        "name": name,
                        "isDir": is_dir,
                        "size": g_u64(it, &["size"]),
                        "kind": kind,
                    }));
                }
            }
            let next_cursor = if out.len() >= 500 { (page + 1).to_string() } else { String::new() };
            Ok(json!({ "items": out, "nextCursor": next_cursor }))
        }
        // 123：file/list/new（cursor = next）
        "123" => {
            let cred = require_cred("123", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            let parent = if fid.is_empty() { "0" } else { fid.as_str() };
            let mut url = format!(
                "https://api.123278.com/b/api/file/list/new?limit=100&parentFileId={}&orderBy=file_name&orderDirection=asc&trashed=0",
                urlencoding::encode(parent)
            );
            if !cursor.is_empty() {
                url.push_str(&format!("&next={}", urlencoding::encode(&cursor)));
            }
            let mut hdrs: Vec<(String, String)> = headers_123()
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            hdrs.push(("Cookie".into(), cookies.to_string()));
            hdrs.push(("Referer".into(), "https://yun.123pan.cn/".into()));
            let v = get_json(&url, &hdrs).await?;
            if v.get("code").and_then(|x| x.as_i64()).unwrap_or(0) != 0 && v.get("data").is_none() {
                return Err(format!("123 列目录被拒: {v}"));
            }
            let list = v
                .pointer("/data/fileList")
                .or_else(|| v.pointer("/data/InfoList"))
                .or_else(|| v.pointer("/data/list"))
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            let next = g_str(&v, &["/data/next", "/data/Next"], &["next", "Next"]);
            let out: Vec<Value> = list
                .iter()
                .map(|it| {
                    let is_dir = g_u64(it, &["Type", "type", "fileType"]) == 1
                        || g_str(it, &["/type"], &["type"]) == "folder";
                    let name = g_str(it, &["/Filename", "/filename", "/fileName", "/name"], &[]);
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
            Ok(json!({ "items": out, "nextCursor": next }))
        }
        // 光鸭：nd.biz/userres.s/v1/file/get_file_list（cursor = page）
        "guangya" => {
            let cred = require_cred("guangya", &["access_token"])?;
            let token = cred.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let page: u32 = if cursor.is_empty() { 1 } else { cursor.parse().unwrap_or(1) };
            let v = post_json(
                "https://api.guangyapan.com/nd.biz/userres.s/v1/file/get_file_list",
                json!({
                    "page": page,
                    "pageSize": 100,
                    "parentFolderId": if fid.is_empty() { "0" } else { fid.as_str() },
                }),
                &[
                    ("Authorization".into(), format!("Bearer {token}")),
                    ("x-client-id".into(), "aMe-8VSlkrbQXpUR8".into()),
                    ("Referer".into(), "https://www.guangyapan.com/".into()),
                ],
            )
            .await?;
            if g_str(&v, &["/code"], &["code"]) != "success" && v.get("data").is_none() {
                return Err(format!("光鸭列目录被拒: {v}"));
            }
            let list = v
                .pointer("/data/list")
                .or_else(|| v.pointer("/data/items"))
                .or_else(|| v.pointer("/list"))
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            let out: Vec<Value> = list
                .iter()
                .map(|it| {
                    let is_dir = g_bool(it, &["is_dir", "isDir", "isFolder"])
                        || g_str(it, &["/type"], &["type"]) == "folder";
                    let name = g_str(it, &["/name", "/fileName", "/file_name"], &["name", "fileName"]);
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
                        "fileId": g_str(it, &["/id", "/fileId", "/file_id"], &["id", "fileId"]),
                        "navId": g_str(it, &["/id", "/folderId", "/fileId"], &["id", "folderId"]),
                        "name": name,
                        "isDir": is_dir,
                        "size": g_u64(it, &["size", "fileSize", "Size"]),
                        "kind": kind,
                    })
                })
                .collect();
            let next_cursor = if list.len() >= 100 { (page + 1).to_string() } else { String::new() };
            Ok(json!({ "items": out, "nextCursor": next_cursor }))
        }
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
        // 115：cloud115 模块的解析（直链 webapi download / 转码 115.com m3u8）
        "115" => {
            let jar = crate::cloud115::load_jar().ok_or("未登录 115（请先扫码）")?;
            let u = crate::cloud115::resolve_play_url(&jar, file_id, prefer_hls).await?;
            Ok((u, prefer_hls, "https://115.com/".into(), crate::cloud115::UA_115.to_string()))
        }
        // 夸克：prefer_hls → v2/play（video_list m3u8）；直链 → file/download（302）
        "quark" => {
            let cred = require_cred("quark", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            if prefer_hls {
                let hdrs = vec![
                    ("Cookie".to_string(), cookies.to_string()),
                    ("Referer".into(), "https://pan.quark.cn/".into()),
                ];
                let url = format!(
                    "https://drive-pc.quark.cn/1/clouddrive/file/v2/play?pr=ucpro&fr=pc&fids={}&resolutions=normal%2Chigh%2Csuper%2C2k%2C4k&supports=fmp4_av%2Cm3u8%2Cdolby_vision",
                    urlencoding::encode(file_id)
                );
                let v = get_json(&url, &hdrs).await?;
                let mut best = String::new();
                if let Some(list) = v.pointer("/data/video_list").and_then(|x| x.as_array()) {
                    for it in list {
                        let u = g_str(it, &["/video_info/url", "/url"], &["url"]);
                        if u.contains(".m3u8") {
                            best = u;
                        }
                    }
                }
                if !best.is_empty() {
                    return Ok((best, true, "https://pan.quark.cn/".into(), UA_CHROME.into()));
                }
                return Err("夸克未返回转码 HLS（可能非会员或文件不可预览）".into());
            }
            // 直链：file/download 302
            let resp = client_noredir()
                .get(format!(
                    "https://drive-pc.quark.cn/1/clouddrive/file/download?pr=ucpro&fr=pc&fids={}",
                    urlencoding::encode(file_id)
                ))
                .header("Cookie", cookies)
                .header("Referer", "https://pan.quark.cn/")
                .send()
                .await
                .map_err(|e| format!("夸克直链请求失败: {e}"))?;
            if let Some(loc) = resp.headers().get(reqwest::header::LOCATION).and_then(|l| l.to_str().ok()) {
                if loc.starts_with("http") {
                    return Ok((loc.to_string(), false, "https://pan.quark.cn/".into(), UA_CHROME.into()));
                }
            }
            Err("夸克未返回可播地址（会员限制或文件不可预览）".into())
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
        // 天翼：batchGetFileDownloadUrl.action（fileIds[]）
        "tianyi" => {
            let cred = require_cred("tianyi", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            let url = format!(
                "https://cloud.189.cn/api/open/file/batchGetFileDownloadUrl.action?fileIds%5B%5D={}",
                urlencoding::encode(file_id)
            );
            let v = get_json(
                &url,
                &[
                    ("Cookie".into(), cookies.to_string()),
                    ("Accept".into(), "application/json, text/plain, */*".into()),
                    ("X-Requested-With".into(), "XMLHttpRequest".into()),
                    ("Referer".into(), "https://cloud.189.cn/web/main".into()),
                ],
            )
            .await?;
            let dl = g_str(
                &v,
                &["/fileDownloadUrlV2", "/fileDownloadUrl"],
                &["fileDownloadUrlV2", "fileDownloadUrl"],
            );
            if dl.is_empty() {
                return Err(format!("天翼未返回下载地址: {v}"));
            }
            Ok((dl, false, "https://cloud.189.cn/".into(), UA_CHROME.into()))
        }
        // 123：file/download_info → DownloadUrl → yun.123pan.cn/Origin 跳转链（跟到最终直链）
        "123" => {
            let cred = require_cred("123", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            let mut hdrs: Vec<(String, String)> = headers_123()
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            hdrs.push(("Cookie".into(), cookies.to_string()));
            hdrs.push(("Referer".into(), "https://yun.123pan.cn/".into()));
            let v = get_json(
                &format!(
                    "https://api.123278.com/b/api/file/download_info?fileId={}&os=web",
                    urlencoding::encode(file_id)
                ),
                &hdrs,
            )
            .await?;
            let dl = g_str(
                &v,
                &["/data/FileDownloadUrl", "/data/DownloadUrl", "/FileDownloadUrl"],
                &["FileDownloadUrl", "DownloadUrl"],
            );
            if dl.is_empty() {
                return Err(format!("123 未返回下载地址: {v}"));
            }
            // 跳转链跟到底（LumiPlayer「media handoff」）：Range bytes=0-0 探活 + Location 链
            let mut cur = dl;
            let mut final_url = cur.clone();
            for _ in 0..8 {
                let resp = client_noredir()
                    .get(&cur)
                    .header("Range", "bytes=0-0")
                    .header("Referer", "https://yun.123pan.cn/")
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
            Ok((final_url, false, "https://yun.123pan.cn/".into(), UA_CHROME.into()))
        }
        // 光鸭：nd.biz/userres.s/v1/get_res_download_url → signedURL
        "guangya" => {
            let cred = require_cred("guangya", &["access_token"])?;
            let token = cred.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let v = post_json(
                "https://api.guangyapan.com/nd.biz/userres.s/v1/get_res_download_url",
                json!({ "fileId": file_id }),
                &[
                    ("Authorization".into(), format!("Bearer {token}")),
                    ("x-client-id".into(), "aMe-8VSlkrbQXpUR8".into()),
                    ("Referer".into(), "https://www.guangyapan.com/".into()),
                ],
            )
            .await?;
            let url = g_str(&v, &["/data/signedURL", "/data/signedUrl", "/signedURL"], &["signedURL", "signedUrl"]);
            if url.is_empty() {
                return Err(format!("光鸭未返回签名直链: {v}"));
            }
            Ok((url, false, "https://www.guangyapan.com/".into(), UA_CHROME.into()))
        }
        _ => Err(format!("不支持的网盘: {provider}")),
    }
}

/// 续签代理入口（cloud_proxy::renew 调）：只要新直链，不要 HLS。
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
            (u, Some(crate::player::HttpHeaders::new(Some(referer), Some(ua))))
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
/// 天翼/光鸭字符串池无 rename 证据，不实现）。
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
        // 123：b/api/file/rename {fileId, fileName}
        "123" => {
            let cred = require_cred("123", &["cookies"])?;
            let cookies = cred.get("cookies").and_then(|x| x.as_str()).unwrap_or("");
            let mut hdrs: Vec<(String, String)> = headers_123()
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            hdrs.push(("Cookie".into(), cookies.to_string()));
            hdrs.push(("Referer".into(), "https://yun.123pan.cn/".into()));
            let fid = file_id.parse::<i64>().unwrap_or_else(|_| {
                // 非纯数字 id（防御）：透传字符串会导致上游 4xx，直接报参数错
                0
            });
            if fid == 0 {
                return Err("123 文件 id 非法".into());
            }
            let v = post_json(
                "https://api.123278.com/b/api/file/rename",
                json!({ "fileId": fid, "fileName": new_name }),
                &hdrs,
            )
            .await?;
            if v.get("code").and_then(|x| x.as_i64()).unwrap_or(-1) != 0 {
                return Err(format!("123 改名被拒: {v}"));
            }
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

    #[test]
    fn qr_state_roundtrip() {
        let st = QrState { provider: "quark".into(), data: json!({ "token": "t1" }), exp_ms: now_ms() + 1000 };
        let packed = st.pack();
        let back = QrState::unpack(&packed).unwrap();
        assert_eq!(back.provider, "quark");
        assert_eq!(back.data.get("token").and_then(|x| x.as_str()), Some("t1"));
        let expired = QrState { provider: "quark".into(), data: json!({}), exp_ms: 1 };
        assert!(QrState::unpack(&expired.pack()).is_err());
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
