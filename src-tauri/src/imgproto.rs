//! fntvimg 自定义协议：带会话鉴权代理 NAS 图片。
//!
//! 为什么必须走代理：WebView 里 <img> 直连 NAS 时是 tauri:// 源，**不带 fnOS 会话 Cookie**，
//! 而 /v/api/v1/sys/img 需要鉴权（浏览器里靠同源 Cookie）→ 401/HTML → 图片全部裂图。
//! Rust 侧 reqwest 代取（Cookie+Authorization、自签证书放行），前端用
//! `http://fntvimg.localhost/<poster 路径>` 引用（Windows 下自定义协议即此形态）。

use std::sync::OnceLock;
use tauri::http::{Request, Response};
use tauri::{AppHandle, Manager};

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
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
    let url = format!(
        "{}{}{}",
        cfg.server.trim_end_matches('/'),
        "/v/api/v1/sys/img",
        path
    );
    let out = http_client()
        .get(&url)
        .header("Authorization", cfg.token.clone())
        .header(
            "Cookie",
            format!("mode=relay; Trim-MC-token={}", cfg.token),
        )
        .send()
        .await;
    match out {
        Ok(r) => {
            let status = r.status();
            let ct = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/jpeg")
                .to_string();
            let bytes = r.bytes().await.unwrap_or_default().to_vec();
            Response::builder()
                .status(status)
                .header("Content-Type", ct)
                .header("Cache-Control", "max-age=86400")
                .body(bytes)
                .unwrap_or_else(|_| Response::builder().status(500).body(Vec::new()).unwrap())
        }
        Err(e) => Response::builder()
            .status(502)
            .header("Content-Type", "text/plain")
            .body(format!("img proxy: {}", e).into_bytes())
            .unwrap_or_else(|_| Response::builder().status(500).body(Vec::new()).unwrap()),
    }
}

// 引用 AppState 类型（仅类型路径，避免循环 use 告警差异）
use crate::AppState;
