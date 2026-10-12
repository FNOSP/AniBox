//! fntvimg 自定义协议：带会话鉴权代理 NAS 图片。
//!
//! 为什么必须走代理：WebView 里 <img> 直连 NAS 时是 tauri:// 源，**不带 fnOS 会话 Cookie**，
//! 而 /v/api/v1/sys/img 需要鉴权（浏览器里靠同源 Cookie）→ 图片全裂。
//! 另一个坑：fnOS 80 端口会把 /v/api/v1/* 302 到实际端口，**跨 host 重定向会剥鉴权头**，
//! 所以这里禁止自动重定向、手动带 Cookie 跟随；未鉴权时 NAS 会返回 200+JSON，识别后按 404 处理。
//! 前端用 `http://fntvimg.localhost/<poster 路径>` 引用（Windows 下自定义协议即此形态）。

use std::sync::OnceLock;
use tauri::http::{Request, Response};
use tauri::{AppHandle, Manager};

use crate::AppState;

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("img client build")
    })
}

/// [lc-113] 图片内存缓存。主仓库（Electron）是渲染进程直连 NAS（带会话 Cookie），只一跳；
/// 我们是 WebView → 本进程 fntvimg 代理 → NAS，多一跳且每张海报一次完整往返，
/// 所以首屏后延明显。这里按路径缓存 (content-type, bytes)，命中直接返回。
const IMG_CACHE_MAX: usize = 400;
const IMG_CACHE_MAX_BYTES: usize = 192 << 20; // 约 192MB 上限

fn img_cache() -> &'static std::sync::Mutex<Option<Vec<(String, String, Vec<u8>)>>> {
    static C: OnceLock<std::sync::Mutex<Option<Vec<(String, String, Vec<u8>)>>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(None))
}

fn cache_get(path: &str) -> Option<(String, Vec<u8>)> {
    let mut g = img_cache().lock().ok()?;
    let v = g.as_mut()?;
    let idx = v.iter().position(|(p, _, _)| p == path)?;
    let (p, ct, b) = v[idx].clone();
    v[idx] = (p, ct.clone(), b.clone());
    Some((ct, b))
}

fn cache_put(path: String, ct: String, bytes: Vec<u8>) {
    let Ok(mut g) = img_cache().lock() else { return };
    let v = g.get_or_insert_with(Vec::new);
    if v.iter().any(|(p, _, _)| *p == path) {
        return;
    }
    v.push((path, ct, bytes));
    while v.len() > IMG_CACHE_MAX
        || v.iter().map(|(_, _, b)| b.len()).sum::<usize>() > IMG_CACHE_MAX_BYTES
    {
        v.remove(0);
    }
}

/// [lc-114] 图片并发闸门：首页一次会打出**几百张**海报请求，全部经本代理转发 NAS。
/// 不限并发时 NAS/网关会被压出 200+HTML 的错误页（表现为图片全挂、变慢，还把并发中的
/// 媒体库 API 挤成"非 JSON"）。这里限制同时在飞的请求数，超出的排队。
fn img_gate() -> &'static tokio::sync::Semaphore {
    static G: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    G.get_or_init(|| tokio::sync::Semaphore::new(6))
}

pub async fn handle(app: AppHandle, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    // Windows 形态：http://fntvimg.localhost/<poster 路径> → path 即海报相对路径
    let path = request.uri().path().to_string();
    if path.is_empty() || path == "/" {
        return Response::builder().status(400).body(b"missing path".to_vec()).unwrap();
    }
    // [lc-114] 拿到闸门许可再发上游请求（限流，避免把 NAS/网关压出错误页）
    let _permit = img_gate().acquire().await.ok();
    let cfg = app.state::<AppState>().config.read().await.clone();
    if cfg.server.is_empty() {
        return Response::builder().status(503).body(b"not logged in".to_vec()).unwrap();
    }
    // [lc-113] 命中内存缓存直接返回，省掉一次 NAS 往返
    if let Some((ct, bytes)) = cache_get(&path) {
        return Response::builder()
            .status(200)
            .header("Content-Type", ct)
            .header("Cache-Control", "max-age=86400")
            .body(bytes)
            .unwrap_or_else(|_| Response::builder().status(500).body(Vec::new()).unwrap());
    }
    let mut current = format!(
        "{}{}{}",
        cfg.server.trim_end_matches('/'),
        "/v/api/v1/sys/img",
        path
    );

    // 手动跟随重定向（带上鉴权头；最多 4 跳）
    let mut final_resp = None;
    for _ in 0..4 {
        let r = http_client()
            .get(&current)
            .header("Authorization", cfg.token.clone())
            .header(
                "Cookie",
                format!("mode=relay; Trim-MC-token={}", cfg.token),
            )
            .send()
            .await;
        match r {
            Ok(r) => {
                let st = r.status().as_u16();
                if matches!(st, 301 | 302 | 307 | 308) {
                    match r
                        .headers()
                        .get("location")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string())
                    {
                        Some(loc) if !loc.is_empty() => {
                            current = loc;
                            continue;
                        }
                        _ => break,
                    }
                }
                final_resp = Some(r);
                break;
            }
            Err(e) => {
                return Response::builder()
                    .status(502)
                    .header("Content-Type", "text/plain")
                    .body(format!("img proxy: {}", e).into_bytes())
                    .unwrap_or_else(|_| Response::builder().status(500).body(Vec::new()).unwrap());
            }
        }
    }

    match final_resp {
        Some(r) => {
            let status = r.status();
            let ct = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/jpeg")
                .to_lowercase();
            let bytes = r.bytes().await.unwrap_or_default().to_vec();
            // 未鉴权/被打到错误页时 NAS 会返回 200+JSON/HTML（不是图片）。
            // [lc-114] 这种情况优先按**可重试的临时故障**处理（限流/网关页），返回 503 + Retry-After，
            // 让浏览器稍后自动重试；只有明确不是图片时才回 404。
            if ct.contains("application/json") || ct.contains("text/html") {
                let looks_like_login = bytes.len() < 20_000 && (bytes.starts_with(b"<!DOCTYPE") || bytes.starts_with(b"<html"));
                if looks_like_login {
                    return Response::builder()
                        .status(503)
                        .header("Retry-After", "2")
                        .header("Content-Type", "text/plain")
                        .body(b"upstream returned html (rate-limited?)".to_vec())
                        .unwrap_or_else(|_| Response::builder().status(503).body(Vec::new()).unwrap());
                }
                return Response::builder()
                    .status(404)
                    .header("Content-Type", "text/plain")
                    .body(b"unauthorized or missing".to_vec())
                    .unwrap();
            }
            // [lc-113] 成功取到图片后写入内存缓存（后续同图秒回）
            cache_put(path.clone(), ct.clone(), bytes.clone());
            Response::builder()
                .status(status)
                .header("Content-Type", ct)
                .header("Cache-Control", "max-age=86400")
                .body(bytes)
                .unwrap_or_else(|_| Response::builder().status(500).body(Vec::new()).unwrap())
        }
        None => Response::builder()
            .status(502)
            .header("Content-Type", "text/plain")
            .body(b"img proxy: redirect loop".to_vec())
            .unwrap_or_else(|_| Response::builder().status(500).body(Vec::new()).unwrap()),
    }
}
