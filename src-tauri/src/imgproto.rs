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

pub async fn handle(app: AppHandle, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    // Windows 形态：http://fntvimg.localhost/<poster 路径> → path 即海报相对路径
    let path = request.uri().path().to_string();
    if path.is_empty() || path == "/" {
        return Response::builder().status(400).body(b"missing path".to_vec()).unwrap();
    }
    let cfg = app.state::<AppState>().config.read().await.clone();
    if cfg.server.is_empty() {
        return Response::builder().status(503).body(b"not logged in".to_vec()).unwrap();
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
            // 未鉴权时 NAS 返回 200+JSON（不是图片），按 404 处理
            if ct.contains("application/json") || ct.contains("text/html") {
                return Response::builder()
                    .status(404)
                    .header("Content-Type", "text/plain")
                    .body(b"unauthorized or missing".to_vec())
                    .unwrap();
            }
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
