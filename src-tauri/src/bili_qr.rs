//! [bili-qr] B 站二维码扫码登录。
//!
//! 流程（与 passport 网页端一致）：
//!   1. `bili_qr_generate`：GET `.../web/qrcode/generate` 拿 `url` + `qrcode_key`，
//!      并用 `qrcode` crate 在本地把 `url` 渲染成 SVG，返回可直接塞进 `<img src>` 的
//!      `data:image/svg+xml;base64,...`（前端不引入任何二维码依赖）。
//!   2. `bili_qr_poll`：GET `.../web/qrcode/poll?qrcode_key=...`，按 `data.code` 判状态：
//!      0=成功 / 86101=未扫码 / 86090=已扫码待确认 / 86038=已过期。
//!      成功时从响应 `Set-Cookie` 提取登录 Cookie，按 `bili_account.rs` 既有格式
//!      原子写入 `<uosc_danmaku 脚本目录>/bili_cookie.txt`。
//!
//! 安全：Cookie 只用于落盘，**绝不写日志**；返回体里也不回显 Cookie 内容。

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tauri::State;

use crate::AppState;
use crate::bili_account;

const QR_GENERATE_URL: &str =
    "https://passport.bilibili.com/x/passport-login/web/qrcode/generate?source=main-fe-header";
const QR_POLL_BASE: &str =
    "https://passport.bilibili.com/x/passport-login/web/qrcode/poll";
/// 落盘需要保留的 Cookie 名（顺序即写入顺序）。
const WANTED_COOKIES: &[&str] = &[
    "SESSDATA",
    "bili_jct",
    "DedeUserID",
    "DedeUserID__ckMd5",
    "sid",
];

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|e| format!("构造请求失败: {e}"))
}

/// 把登录跳转 URL 渲染成 SVG data URI。
fn url_to_svg_data_uri(url: &str) -> Result<String, String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(url.as_bytes()).map_err(|e| format!("生成二维码失败: {e}"))?;
    let svg = code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(svg.as_bytes())
    ))
}

/// 从多条 `Set-Cookie` 里按名字提取 `name=value`（丢弃 Path/Expires 等属性）。
fn extract_cookies(headers: &reqwest::header::HeaderMap) -> String {
    let mut out: Vec<String> = Vec::new();
    for hv in headers.get_all(reqwest::header::SET_COOKIE) {
        let Ok(raw) = hv.to_str() else { continue };
        let pair = raw.split(';').next().unwrap_or("").trim();
        let Some((name, _)) = pair.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !WANTED_COOKIES.contains(&name) {
            continue;
        }
        // 同名只保留第一条
        if out.iter().any(|c| c.split_once('=').map(|(n, _)| n) == Some(name)) {
            continue;
        }
        out.push(pair.to_string());
    }
    out.join("; ")
}

/// 生成登录二维码：`{ url, qrcodeKey, image }`。
#[tauri::command]
pub async fn bili_qr_generate() -> Result<Value, String> {
    let client = http_client()?;
    let resp = client
        .get(QR_GENERATE_URL)
        .header("User-Agent", "Mozilla/5.0")
        .header("Referer", "https://www.bilibili.com")
        .send()
        .await
        .map_err(|e| format!("请求 B 站二维码失败: {e}"))?;

    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("B 站二维码返回内容无法解析: {e}"))?;
    let top_code = v.get("code").and_then(|x| x.as_i64()).unwrap_or(-1);
    if top_code != 0 {
        let msg = v
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or("B 站二维码接口返回错误");
        return Err(msg.to_string());
    }

    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let url = data
        .get("url")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "B 站二维码返回缺少 url".to_string())?
        .to_string();
    let qrcode_key = data
        .get("qrcode_key")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "B 站二维码返回缺少 qrcode_key".to_string())?
        .to_string();

    let image = url_to_svg_data_uri(&url)?;
    Ok(json!({
        "url": url,
        "qrcodeKey": qrcode_key,
        "image": image,
    }))
}

/// 轮询扫码状态：`{ code, message, ok, uname?, cookieSaved }`。
#[tauri::command]
pub async fn bili_qr_poll(
    state: State<'_, AppState>,
    qrcode_key: String,
) -> Result<Value, String> {
    let client = http_client()?;
    let resp = client
        .get(QR_POLL_BASE)
        .query(&[
            ("qrcode_key", qrcode_key.as_str()),
            ("source", "main-fe-header"),
        ])
        .header("User-Agent", "Mozilla/5.0")
        .header("Referer", "https://www.bilibili.com")
        .send()
        .await
        .map_err(|e| format!("轮询 B 站扫码状态失败: {e}"))?;

    // 登录成功时 Cookie 在响应头里，必须在消费 body 前先取出来。
    let set_cookies = extract_cookies(resp.headers());

    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("B 站扫码状态返回内容无法解析: {e}"))?;

    let top_code = v.get("code").and_then(|x| x.as_i64()).unwrap_or(-1);
    if top_code != 0 {
        let msg = v
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or("B 站扫码接口返回错误");
        return Err(msg.to_string());
    }

    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let code = data.get("code").and_then(|x| x.as_i64()).unwrap_or(-1);
    let message = data
        .get("message")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| match code {
            0 => "登录成功".into(),
            86101 => "未扫码".into(),
            86090 => "已扫码，待确认".into(),
            86038 => "二维码已过期".into(),
            _ => "未知状态".into(),
        });

    if code != 0 {
        return Ok(json!({
            "code": code,
            "message": message,
            "ok": false,
            "uname": Value::Null,
            "cookieSaved": false,
        }));
    }

    // 成功：落盘 Cookie。缺 SESSDATA 说明响应结构变化，按实际返回如实报告。
    let normalized = match bili_account::normalize_cookie(&set_cookies) {
        Ok(c) => c,
        Err(e) => {
            return Ok(json!({
                "code": code,
                "message": format!("登录成功，但未能从响应提取有效 Cookie：{e}"),
                "ok": false,
                "uname": Value::Null,
                "cookieSaved": false,
            }))
        }
    };

    let Some(dir) = bili_account::script_dir_async(&state).await else {
        return Ok(json!({
            "code": code,
            "message": "登录成功，但未找到 uosc_danmaku 脚本目录，Cookie 未保存（请先在「播放器」里选择有效的 mpv 路径）",
            "ok": true,
            "uname": Value::Null,
            "cookieSaved": false,
        }));
    };

    if let Err(e) = bili_account::write_cookie_file(&bili_account::cookie_path(&dir), &normalized) {
        return Ok(json!({
            "code": code,
            "message": format!("登录成功，但保存 Cookie 失败：{e}"),
            "ok": true,
            "uname": Value::Null,
            "cookieSaved": false,
        }));
    }

    // 顺带体检拿用户名（失败不影响登录成功结论）。
    let nav = bili_account::nav_check(&normalized).await;
    let uname = nav
        .get("uname")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());

    Ok(json!({
        "code": code,
        "message": message,
        "ok": true,
        "uname": uname,
        "cookieSaved": true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_data_uri_is_base64_svg() {
        let uri = url_to_svg_data_uri("https://passport.bilibili.com/h5-app/passport/login/scan")
            .expect("qr render");
        let b64 = uri
            .strip_prefix("data:image/svg+xml;base64,")
            .expect("prefix");
        let bytes = STANDARD.decode(b64).expect("base64");
        let svg = String::from_utf8(bytes).expect("utf8");
        assert!(svg.contains("<svg"));
    }

    #[test]
    fn extract_cookies_keeps_only_wanted_pairs() {
        let mut h = reqwest::header::HeaderMap::new();
        h.append(
            reqwest::header::SET_COOKIE,
            "SESSDATA=abc%2C123; Path=/; Domain=.bilibili.com; Secure; HttpOnly".parse().unwrap(),
        );
        h.append(
            reqwest::header::SET_COOKIE,
            "bili_jct=token123; Path=/; Domain=.bilibili.com".parse().unwrap(),
        );
        h.append(
            reqwest::header::SET_COOKIE,
            "DedeUserID=42; Path=/".parse().unwrap(),
        );
        h.append(
            reqwest::header::SET_COOKIE,
            "buvid3=ignore; Path=/".parse().unwrap(),
        );
        let c = extract_cookies(&h);
        assert_eq!(c, "SESSDATA=abc%2C123; bili_jct=token123; DedeUserID=42");
    }
}
