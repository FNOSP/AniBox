//! [lc-1779] 115 网盘直连：扫码登录 + 文件浏览 + 播放（直链优先，不兼容编码走
//! `115.com/api/video/m3u8` **服务端转码 HLS**，本地零转码 CPU）。
//!
//! 端点口径来自 LumiPlayer 2.2.1 逆向（direct_cloud.rs 的字符串证据）+ 115 web 端公开行为：
//!   · 扫码：qrcodeapi.115.com/api/1.0/web/1.0/{token,qrcode,status}
//!           → passportapi.115.com/app/1.0/web/1.0/login/qrcode 抓 Set-Cookie（UID/CID/SEID）
//!   · 列表：aps.115.com/natsort/files.php（UA 带 115Browser 标识；pickcode = 字段 pc）
//!   · 直链：webapi.115.com/files/download?pickcode=…（JSON 带 url / 302 Location 两种形态）
//!   · 转码：115.com/api/video/m3u8?pickcode=…（返回 .m3u8?definition=… 的 HLS 地址）
//!
//! 安全口径：Cookie 只落本机 `app_data_dir/cloud115.json`（与登录凭据同级待遇），
//! 绝不入库、不进日志；未登录调用一律明确报错而不是空转。
//!
//! 与 fnOS 播放链路的关系：不走 Go 代理、不回传飞牛进度（同直播直链口径），
//! 播放历史也不落本机记录（115 条目没有 fnOS guid，记录页点开无法回放——
//! 等后续做统一「云盘来源」记录时再接入）。

use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 115 对 web 端口有 UA 校验的接口（aps/proapi）认 "115Browser" 标识；
/// qrcodeapi/webapi 较宽松，统一带上无害。
const UA_115: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) 115Browser/27.0.5.7 Chrome/122.0.0.0 Safari/537.36";

const LIST_LIMIT: usize = 100;

// ───────────────────────── Cookie 存取（app_data_dir/cloud115.json） ─────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct CookieJar {
    uid: String,
    cid: String,
    seid: String,
    user_name: String,
    saved_at_ms: u64,
}

fn jar_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("cloud115.json"))
}

fn load_jar(app: &AppHandle) -> Option<CookieJar> {
    let p = jar_path(app)?;
    let s = std::fs::read_to_string(p).ok()?;
    let j: CookieJar = serde_json::from_str(&s).ok()?;
    if j.uid.is_empty() || j.cid.is_empty() || j.seid.is_empty() {
        return None;
    }
    Some(j)
}

fn save_jar(app: &AppHandle, jar: &CookieJar) -> Result<(), String> {
    let p = jar_path(app).ok_or("拿不到 app_data_dir")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let s = serde_json::to_string_pretty(jar).map_err(|e| format!("序列化 Cookie 失败: {e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写 Cookie 文件失败: {e}"))
}

fn require_jar(app: &AppHandle) -> Result<CookieJar, String> {
    load_jar(app).ok_or_else(|| "未登录 115（请先扫码）".into())
}

fn cookie_header(jar: &CookieJar) -> String {
    format!("UID={}; CID={}; SEID={}", jar.uid, jar.cid, jar.seid)
}

// ───────────────────────── HTTP ─────────────────────────

fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(UA_115)
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("cloud115 http client")
    })
}

/// 不跟随重定向的专用 client（直链接口 302 = Location 即答案，跟丢就没线索了）。
fn client_noredir() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(UA_115)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("cloud115 noredir http client")
    })
}

async fn get_json_with_cookie(url: &str, jar: &CookieJar) -> Result<Value, String> {
    let resp = client()
        .get(url)
        .header(reqwest::header::COOKIE, cookie_header(jar))
        .header(reqwest::header::REFERER, "https://115.com/")
        .send()
        .await
        .map_err(|e| format!("115 请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("115 响应读取失败: {e}"))?;
    if !status.is_success() {
        // 405/403 通常是 Cookie 失效
        return Err(format!("115 上游 {}（Cookie 可能已失效，请重新扫码）", status.as_u16()));
    }
    serde_json::from_str(&text)
        .map_err(|_| format!("115 返回了非 JSON 内容（登录态可能已失效）：{}", text.chars().take(120).collect::<String>()))
}

// ───────────────────────── 登录（扫码） ─────────────────────────

/// 开始扫码：返回二维码图片 URL（前端 <img> 直显）+ 轮询三元组。
/// 实测（2026-10）：必须 **GET**（POST 一律 302 到 www.115.com）；响应 data 里
/// 直接带 qrcode 图片地址；time 是数字（str/i64 两种都兼容）。
#[tauri::command]
pub async fn cloud115_qr_start() -> Result<Value, String> {
    let resp = client()
        .get("https://qrcodeapi.115.com/api/1.0/web/1.0/token")
        .send()
        .await
        .map_err(|e| format!("获取二维码 token 失败: {e}"))?;
    let v: Value = resp.json().await.map_err(|e| format!("解析 token 响应失败: {e}"))?;
    let d = v.get("data").ok_or("token 响应缺 data")?;
    let field = |name: &str| -> String {
        d.get(name)
            .and_then(|x| {
                x.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| x.as_i64().map(|n| n.to_string()))
            })
            .unwrap_or_default()
    };
    let uid = field("uid");
    let time = field("time");
    let sign = field("sign");
    if uid.is_empty() || time.is_empty() || sign.is_empty() {
        return Err(format!("token 响应字段不齐: {v}"));
    }
    // 二维码图片 = qrcode 端点**直接返回 PNG 二进制**（实测 2026-10），前端 <img> 直指即可；
    // token 响应里的 `qrcode` 字段是扫码链接（115.com/scan/…）不是图片，不要用。
    let png = format!("https://qrcodeapi.115.com/api/1.0/web/1.0/qrcode?uid={uid}");
    Ok(json!({ "qrcode": png, "uid": uid, "time": time, "sign": sign }))
}

/// 轮询扫码状态。status：0=待扫码 1=已扫待确认 2=已确认（此时完成登录并落盘 Cookie）。
#[tauri::command]
pub async fn cloud115_qr_poll(
    app: AppHandle,
    uid: String,
    time: String,
    sign: String,
) -> Result<Value, String> {
    let (status, cookies) = qr_poll_core(&uid, &time, &sign).await?;
    if status != 2 {
        return Ok(json!({ "status": status, "ok": false }));
    }
    let (uid_c, cid_c, seid_c, user_name) =
        cookies.ok_or("扫码已确认但未取回 Cookie（内部状态不一致）")?;
    let jar = CookieJar {
        uid: uid_c,
        cid: cid_c,
        seid: seid_c,
        user_name,
        saved_at_ms: now_ms(),
    };
    save_jar(&app, &jar)?;
    Ok(json!({ "status": 2, "ok": true, "userName": jar.user_name }))
}

/// 轮询核心（不碰 AppHandle，便于实测）：status==2 时四元组 Some((UID, CID, SEID, user_name))。
async fn qr_poll_core(
    uid: &str,
    time: &str,
    sign: &str,
) -> Result<(i64, Option<(String, String, String, String)>), String> {
    let resp = client()
        .get(format!(
            "https://qrcodeapi.115.com/api/1.0/web/1.0/status?uid={uid}&time={time}&sign={sign}"
        ))
        .send()
        .await
        .map_err(|e| format!("查询扫码状态失败: {e}"))?;
    let v: Value = resp.json().await.map_err(|e| format!("解析扫码状态失败: {e}"))?;
    // 实测（2026-10）：未扫码时返回 {state:false, code:90038, message:"请扫描二维码"}
    // （没有 data.status）；已扫待确认/已确认才有状态值。做宽容映射，-1 才视为异常。
    let status = v
        .pointer("/data/status")
        .and_then(|x| x.as_i64())
        .or_else(|| v.get("status").and_then(|x| x.as_i64()))
        .unwrap_or_else(|| {
            if v.get("state").and_then(|s| s.as_bool()) == Some(true) {
                2
            } else if v.get("code").and_then(|c| c.as_i64()) == Some(90038) {
                0
            } else {
                let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
                if msg.contains("确认") || msg.contains("已扫") {
                    1
                } else {
                    0
                }
            }
        });
    if status != 2 {
        return Ok((status, None));
    }
    // 已确认：换 Cookie
    let login_url = format!(
        "https://passportapi.115.com/app/1.0/web/1.0/login/qrcode?account={uid}&app=web&v=1.0&time={time}&sign={sign}"
    );
    let resp = client_noredir()
        .get(&login_url)
        .send()
        .await
        .map_err(|e| format!("登录请求失败: {e}"))?;
    let (mut uid_c, mut cid_c, mut seid_c) = (String::new(), String::new(), String::new());
    for val in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        let s = val.to_str().unwrap_or("");
        let mut it = s.split(';');
        let pair = it.next().unwrap_or("");
        let mut kv = pair.splitn(2, '=');
        let name = kv.next().unwrap_or("").trim();
        let value = kv.next().unwrap_or("").trim();
        match name {
            "UID" => uid_c = value.to_string(),
            "CID" => cid_c = value.to_string(),
            "SEID" => seid_c = value.to_string(),
            _ => {}
        }
    }
    if uid_c.is_empty() || cid_c.is_empty() || seid_c.is_empty() {
        return Err("扫码已确认但没拿到完整 Cookie（UID/CID/SEID），请重试".into());
    }
    let body = resp.text().await.unwrap_or_default();
    let user_name = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|b| {
            b.pointer("/data/user_name")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();
    Ok((2, Some((uid_c, cid_c, seid_c, user_name))))
}

/// 登录状态（不打网络，只看本地 Cookie + 名称）。
#[tauri::command]
pub async fn cloud115_status(app: AppHandle) -> Result<Value, String> {
    let logged = load_jar(&app).is_some();
    let user = load_jar(&app).map(|j| j.user_name).unwrap_or_default();
    Ok(json!({ "logged": logged, "userName": user }))
}

#[tauri::command]
pub async fn cloud115_logout(app: AppHandle) -> Result<(), String> {
    if let Some(p) = jar_path(&app) {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

// ───────────────────────── 文件浏览 ─────────────────────────

const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "avi", "wmv", "flv", "mov", "iso", "m2ts", "webm", "ts", "rmvb", "mpg",
];

/// 列目录。cid=0 为根；返回条目（文件夹 + 视频/音频文件）、总数、是否还有下一页。
#[tauri::command]
pub async fn cloud115_list(
    app: AppHandle,
    cid: Option<String>,
    offset: Option<u32>,
) -> Result<Value, String> {
    let jar = require_jar(&app)?;
    let cid = cid.filter(|c| !c.is_empty()).unwrap_or_else(|| "0".into());
    let offset = offset.unwrap_or(0);
    let url = format!(
        "https://aps.115.com/natsort/files.php?aid=1&cid={cid}&o=user_ptime&asc=0&offset={offset}\
&show_dir=1&limit={LIST_LIMIT}&code=&scid=&snap=0&natsort=1&record_open_time=1\
&count_folders=1&type=&source=&format=json&fc_mix=1"
    );
    let v = get_json_with_cookie(&url, &jar).await?;
    if v.get("state").and_then(|s| s.as_bool()) != Some(true) {
        let err = v.get("error").and_then(|e| e.as_str()).unwrap_or("未知错误");
        return Err(format!("115 列目录失败: {err}（Cookie 失效请重新扫码）"));
    }
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let count = data.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
    let mut items = Vec::new();
    if let Some(list) = data.get("list").and_then(|l| l.as_array()) {
        for it in list {
            let name = it.get("n").and_then(|n| n.as_str()).unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            // pickcode（pc）存在 = 文件；缺失 = 文件夹
            let pc = it.get("pc").and_then(|p| p.as_str()).unwrap_or("");
            let is_dir = pc.is_empty();
            let fid = it
                .get("fid")
                .and_then(|f| {
                    f.as_str()
                        .map(|s| s.to_string())
                        .or_else(|| f.as_i64().map(|x| x.to_string()))
                })
                .unwrap_or_default();
            let size = it.get("s").and_then(|s| s.as_u64()).unwrap_or(0);
            let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let vtype = it.get("vtype").and_then(|x| x.as_str()).unwrap_or("");
            let is_video = !is_dir
                && (!vtype.is_empty()
                    || VIDEO_EXTS.contains(&ext.as_str()));
            let is_audio = !is_dir && !is_video && vtype.starts_with("audio");
            if is_dir || is_video || is_audio {
                items.push(json!({
                    "fileId": fid,
                    "name": name,
                    "isDir": is_dir,
                    "size": size,
                    "pickcode": if is_dir { "" } else { pc },
                    "kind": if is_dir { "dir" } else if is_video { "video" } else { "audio" },
                }));
            }
        }
    }
    let has_more = (offset as u64 + LIST_LIMIT as u64) < count;
    Ok(json!({
        "cid": cid,
        "count": count,
        "offset": offset,
        "hasMore": has_more,
        "items": items,
    }))
}

// ───────────────────────── 播放 ─────────────────────────

/// 取可播地址。hls=false → 直链（webapi files/download，JSON url 或 302 Location）；
/// hls=true → 115 服务端转码 HLS（115.com/api/video/m3u8，返回 …m3u8?definition=…）。
async fn resolve_play_url(jar: &CookieJar, pickcode: &str, hls: bool) -> Result<String, String> {
    if hls {
        let t = ts();
        let url = format!("https://115.com/api/video/m3u8?pickcode={pickcode}&share_id=0&pc=0&_={t}");
        let v = get_json_with_cookie(&url, jar).await?;
        let u = v
            .pointer("/data/url")
            .and_then(|x| x.as_str())
            .or_else(|| v.get("url").and_then(|x| x.as_str()))
            .unwrap_or("")
            .to_string();
        if u.is_empty() || !u.contains(".m3u8") {
            let err = v.get("error").and_then(|e| e.as_str()).unwrap_or("未返回 HLS 地址");
            return Err(format!("115 转码地址获取失败: {err}"));
        }
        return Ok(u);
    }
    // 直链：先按「不跟随重定向」打一发，302 的 Location 就是答案；
    // 200 JSON 则解析 url 字段（历史上有 url / file_url / fileUrl 多种形态）。
    let t = ts();
    let url = format!("https://webapi.115.com/files/download?pickcode={pickcode}&_={t}");
    let resp = client_noredir()
        .get(&url)
        .header(reqwest::header::COOKIE, cookie_header(jar))
        .header(reqwest::header::REFERER, "https://115.com/")
        .send()
        .await
        .map_err(|e| format!("115 直链请求失败: {e}"))?;
    if resp.status().is_redirection() {
        if let Some(loc) = resp.headers().get(reqwest::header::LOCATION) {
            let loc = loc.to_str().unwrap_or("").to_string();
            if loc.starts_with("http") {
                return Ok(loc);
            }
        }
        return Err("115 直链 302 但缺 Location".into());
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("115 直链上游 {}", status.as_u16()));
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| {
        format!("115 直链返回非 JSON（登录态可能失效）：{}", text.chars().take(120).collect::<String>())
    })?;
    let u = ["url", "file_url", "fileUrl", "file_url_302"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .or_else(|| v.pointer("/data/url").and_then(|x| x.as_str()))
        .unwrap_or("")
        .to_string();
    if u.is_empty() {
        let err = v.get("error").and_then(|e| e.as_str()).unwrap_or("未返回地址");
        return Err(format!("115 直链获取失败: {err}"));
    }
    Ok(u)
}

/// 播放 115 文件：解析地址（直链/转码 HLS）→ 单条目 M3U8 → player::launch（mpv 复用会话）。
/// 与直播同口径：不走 Go 代理、不回传飞牛进度、不落本机播放历史。
#[tauri::command]
pub async fn cloud115_play(
    app: AppHandle,
    state: tauri::State<'_, crate::AppState>,
    pickcode: String,
    name: String,
    hls: Option<bool>,
) -> Result<Value, String> {
    let jar = require_jar(&app)?;
    if pickcode.trim().is_empty() {
        return Err("pickcode 为空".into());
    }
    let use_hls = hls.unwrap_or(false);
    let _ = app.emit("mpv-load", json!({ "phase": "start" }));
    let url = resolve_play_url(&jar, pickcode.trim(), use_hls).await?;
    let cfg = state.config.read().await.clone();
    // 直链失败（非 VIP 限速到不可播 / 编码不兼容）时前端可自动降级 hls=true 重试，
    // 这里把成功用的模式带回，便于前端提示「当前：转码播放」。
    crate::fnos_progress::begin("", "");
    crate::player::launch(
        &app,
        &state,
        &cfg,
        &url,
        &name,
        0.0,
        0.0,
        Some(&crate::player::HttpHeaders::new(
            Some("https://115.com/".into()),
            Some(UA_115.into()),
        )),
        &[],
        None,
        true,
    )
    .await?;
    Ok(json!({ "url": url, "hls": use_hls }))
}

// ───────────────────────── 小工具 ─────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ts() -> u64 {
    now_ms() / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set-Cookie 多值解析：只取 UID/CID/SEID 的第一段 pair，忽略属性。
    #[test]
    fn cookie_shape_parsing_would_pick_three() {
        // 直接验证 jar 往返（网络部分无法单测，登录链路靠qr_start/qr_poll 实测）
        let jar = CookieJar {
            uid: "u1".into(),
            cid: "c1".into(),
            seid: "s1".into(),
            user_name: "测试".into(),
            saved_at_ms: 1,
        };
        assert_eq!(cookie_header(&jar), "UID=u1; CID=c1; SEID=s1");
        let s = serde_json::to_string(&jar).unwrap();
        let back: CookieJar = serde_json::from_str(&s).unwrap();
        assert_eq!(back.user_name, "测试");
    }

    #[test]
    fn video_ext_matrix() {
        for (name, video) in [
            ("a.MKV", true),
            ("b.mp4", true),
            ("c.txt", false),
            ("d.简体&英文.ass", false),
            ("noext", false),
        ] {
            let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let is_video = name != "noext" && VIDEO_EXTS.contains(&ext.as_str());
            assert_eq!(is_video, video, "{name}");
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// 实测（网络）：token → 二维码 → status 轮询整条参数链（不扫码，status 应为 0/1）。
    /// 真正的 Cookie 捕获（status==2）只能由用户扫码完成，无法自动化。
    /// #[ignore]：依赖外网，离线构建/CI 不跑；验证时 `cargo test --lib cloud115 -- --ignored`。
    #[tokio::test]
    #[ignore]
    async fn qr_chain_live() {
        let resp = client()
            .get("https://qrcodeapi.115.com/api/1.0/web/1.0/token")
            .send()
            .await
            .expect("token 请求应通（GET）");
        let v: Value = resp.json().await.expect("token 应为 JSON");
        let d = v.get("data").expect("token 应带 data").clone();
        let field = |n: &str| -> String {
            d.get(n)
                .and_then(|x| {
                    x.as_str()
                        .map(|s| s.to_string())
                        .or_else(|| x.as_i64().map(|x| x.to_string()))
                })
                .unwrap_or_default()
        };
        let (uid, time, sign) = (field("uid"), field("time"), field("sign"));
        assert!(!uid.is_empty() && !time.is_empty() && !sign.is_empty(), "token 字段: {d}");

        // qrcode 端点直接回 PNG 二进制（magic bytes 验证）
        let qr = client()
            .get(format!("https://qrcodeapi.115.com/api/1.0/web/1.0/qrcode?uid={uid}"))
            .send()
            .await
            .expect("qrcode 请求应通");
        let bytes = qr.bytes().await.expect("qrcode 应有 body");
        assert_eq!(&bytes[..4], &[0x89u8, b'P', b'N', b'G'], "qrcode 应为 PNG 二进制");

        let (status, cookies) = qr_poll_core(&uid, &time, &sign)
            .await
            .expect("status 轮询应通（参数正确性由本断言背书）");
        assert!((0..=1).contains(&status), "未扫码 status 应为 0/1，实际 {status}");
        assert!(cookies.is_none());
        println!("QR 链路 OK: status={status}（0=待扫码）");
    }
}
