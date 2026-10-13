//! fntvimg 自定义协议：带会话鉴权代理 NAS 图片 + **TMDB / Bangumi 图片的落盘缓存代理**。
//!
//! Windows 形态：`http://fntvimg.localhost/<path>`；mac 形态：`fntvimg://localhost/<path>`。
//! 路径前缀决定上游：
//!   · `/__tmdb/<rest>`  → `https://tmdb.690075.xyz/img/<rest>`（TMDB 图片域，**免签名**）
//!   · `/__bgm/<rest>`   → `https://lain.bgm.tv/<rest>`（Bangumi 图片域；明文 http 由前端升 https）
//!   · 其余              → NAS `…/v/api/v1/sys/img<path>`（需会话鉴权，见下）
//! （前缀刻意用 `__tmdb` / `__bgm`：URL 路径里没有需要转义的字符，且 NAS 的图片相对路径
//!   形如 `/f5/04/xxx.webp`，不可能撞上这两个前缀。）
//!
//! 为什么必须走代理（NAS 图片）：WebView 里 <img> 直连 NAS 时是 tauri:// 源，**不带 fnOS 会话 Cookie**，
//! 而 /v/api/v1/sys/img 需要鉴权（浏览器里靠同源 Cookie）→ 图片全裂。
//! 另一个坑：fnOS 80 端口会把 /v/api/v1/* 302 到实际端口，**跨 host 重定向会剥鉴权头**，
//! 所以这里禁止自动重定向、手动带 Cookie 跟随；未鉴权时 NAS 会返回 200+JSON，识别后按 404 处理。
//!
//! 三级缓存（内存 → 磁盘 → 网络）在 crate::disk_cache：命中本地直接返回、**绝不重复请求**
//! （用户要求「拉了就本地缓存不要多拉」），落盘在应用数据目录（不是会被系统清理的 %TEMP%）；
//! 上限/占用/清理见设置面板（lib.rs 的 cache_* 命令）。

use std::sync::OnceLock;
use tauri::http::{Request, Response};
use tauri::{AppHandle, Manager};

use crate::disk_cache;
use crate::AppState;

/// TMDB 图片域（镜像反代 image.tmdb.org；**免签名**，见 tmdb-mirror worker.js 注释）
const TMDB_IMG_BASE: &str = "https://tmdb.690075.xyz";
/// Bangumi 图片域
const BGM_IMG_BASE: &str = "https://lain.bgm.tv";

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

/// 无鉴权的图片上游（TMDB 镜像 / lain.bgm.tv）：允许自动重定向（图片 CDN 常见 302）。
fn public_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("FntvPlus-Tauri2/0.1 (+https://github.com/YDMY007/Fntv-Plus)")
            .connect_timeout(std::time::Duration::from_secs(6))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("img public client build")
    })
}

/// [lc-114] 图片并发闸门：首页一次会打出**几百张**海报请求，全部经本代理转发上游。
/// 不限并发时 NAS/网关会被压出 200+HTML 的错误页（表现为图片全挂、变慢，还把并发中的
/// 媒体库 API 挤成"非 JSON"）。这里限制同时在飞的请求数，超出的排队。
fn img_gate() -> &'static tokio::sync::Semaphore {
    static G: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    G.get_or_init(|| tokio::sync::Semaphore::new(6))
}

/// 上游目标：解析路径前缀 → 缓存键 + 上游 URL（未登录也不需要知道 server/token 的走这里）
enum Target {
    /// NAS 相对路径（原样透传给 /v/api/v1/sys/img）
    Nas(String),
    /// 免鉴权图片上游（TMDB 镜像 / Bangumi）
    Public { key: String, url: String },
}

/// 相对路径安全闸：拒绝空段、`..`、反斜杠、控制字符（防穿透到上游别的路径）。
fn safe_rel(rel: &str) -> bool {
    if rel.is_empty() || rel.starts_with('/') {
        return false;
    }
    !rel.split('/').any(|seg| seg.is_empty() || seg == ".." || seg == "." || seg.contains('\\'))
        && !rel.chars().any(|c| c.is_control())
}

fn resolve_target(path: &str) -> Result<Target, (u16, String)> {
    if path.is_empty() || path == "/" {
        return Err((400, "missing path".into()));
    }
    if let Some(rest) = path.strip_prefix("/__tmdb/") {
        if !safe_rel(rest) {
            return Err((400, "bad tmdb path".into()));
        }
        return Ok(Target::Public {
            key: format!("img:tmdb:{rest}"),
            url: format!("{TMDB_IMG_BASE}/img/{rest}"),
        });
    }
    if let Some(rest) = path.strip_prefix("/__bgm/") {
        if !safe_rel(rest) {
            return Err((400, "bad bgm path".into()));
        }
        return Ok(Target::Public {
            key: format!("img:bgm:{rest}"),
            url: format!("{BGM_IMG_BASE}/{rest}"),
        });
    }
    Ok(Target::Nas(path.to_string()))
}

fn plain(status: u16, msg: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain")
        .body(msg.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::builder().status(500).body(Vec::new()).unwrap())
}

/// 命中缓存的响应：200 + 内容类型 + 长缓存 + 可观测的 `x-fntv-cache: HIT`
fn cached_response(ct: &str, bytes: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(200)
        .header("Content-Type", ct)
        .header("Cache-Control", "max-age=86400")
        .header("x-fntv-cache", "HIT")
        .body(bytes)
        .unwrap_or_else(|_| Response::builder().status(500).body(Vec::new()).unwrap())
}

/// 免鉴权图片上游（TMDB 镜像 / Bangumi）：内存 → 磁盘 → 网络，成功即落盘。
async fn proxy_public(key: &str, url: &str) -> Response<Vec<u8>> {
    if let Some((ct, bytes)) = disk_cache::get(key) {
        return cached_response(&ct, bytes);
    }
    let resp = match public_client().get(url).header("accept", "image/*,*/*").send().await {
        Ok(r) => r,
        Err(e) => return plain(502, &format!("img upstream: {e}")),
    };
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return plain(502, &format!("img upstream HTTP {status}"));
    }
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_lowercase();
    // 上游返回的 HTML/JSON 错误页（防盗链/网关页）不写盘、不缓存，直接按缺失处理
    if ct.contains("text/html") || ct.contains("application/json") {
        return plain(404, "upstream returned non-image");
    }
    let bytes = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
    if bytes.is_empty() {
        return plain(502, "upstream returned empty body");
    }
    disk_cache::put(key, &ct, &bytes);
    crate::libmpv::trace(&format!("[img] MISS {key} → {} 字节（已落盘）", bytes.len()));
    Response::builder()
        .status(200)
        .header("Content-Type", ct)
        .header("Cache-Control", "max-age=86400")
        .header("x-fntv-cache", "MISS")
        .body(bytes)
        .unwrap_or_else(|_| Response::builder().status(500).body(Vec::new()).unwrap())
}

pub async fn handle(app: AppHandle, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    // Windows 形态：http://fntvimg.localhost/<poster 路径> → path 即图片相对路径
    let path = request.uri().path().to_string();
    let target = match resolve_target(&path) {
        Ok(t) => t,
        Err((status, msg)) => return plain(status, &msg),
    };

    // [lc-114] 拿到闸门许可再发上游请求（限流，避免把上游压出错误页）
    let _permit = img_gate().acquire().await.ok();

    let (key, nas_path) = match target {
        Target::Public { key, url } => {
            // 免鉴权上游：不需要登录/配置，命中缓存直接返回（绝不重复请求）
            return proxy_public(&key, &url).await;
        }
        Target::Nas(p) => (format!("img:nas:{p}"), p),
    };

    let cfg = app.state::<AppState>().config.read().await.clone();
    if cfg.server.is_empty() {
        return plain(503, "not logged in");
    }
    // [lc-113] 命中本地缓存（内存 → 磁盘）直接返回，省掉一次 NAS 往返
    if let Some((ct, bytes)) = disk_cache::get(&key) {
        return cached_response(&ct, bytes);
    }
    let mut current = format!(
        "{}{}{}",
        cfg.server.trim_end_matches('/'),
        "/v/api/v1/sys/img",
        nas_path
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
                return plain(502, &format!("img proxy: {e}"));
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
                return plain(404, "unauthorized or missing");
            }
            // [lc-113] 成功取到图片后写入缓存（内存 + 磁盘；后续同图零网络、重启后仍走盘）
            disk_cache::put(&key, &ct, &bytes);
            Response::builder()
                .status(status)
                .header("Content-Type", ct)
                .header("Cache-Control", "max-age=86400")
                .header("x-fntv-cache", "MISS")
                .body(bytes)
                .unwrap_or_else(|_| Response::builder().status(500).body(Vec::new()).unwrap())
        }
        None => plain(502, "img proxy: redirect loop"),
    }
}

#[cfg(test)]
mod tests {
    use super::{public_client, BGM_IMG_BASE, TMDB_IMG_BASE};

    /// 真机冒烟（**默认忽略**）：
    ///   FNTV_TMDB_SECRET=<值> cargo test --lib imgproto::tests::live_public_image_upstreams_work -- --ignored --nocapture
    /// 为什么需要它：图片从「WebView 直连图片域」改成「Rust 侧代理 + 落盘」后，**请求方从浏览器换成了
    /// 我们自己的 UA/Accept** —— 一旦某个 CDN 按 UA/Referer 403（症状是整片图裂，而且只在日志里看得见），
    /// 静态断言是查不出来的。这条把两个上游各拉一张真图钉住（tmdb 图路径从详情响应现取，不写死）。
    #[tokio::test]
    #[ignore = "live network (+ FNTV_TMDB_SECRET for the TMDB leg)"]
    async fn live_public_image_upstreams_work() {
        // ── ① TMDB 图片域（镜像 /img/*，免签名）：先从详情响应里取一个真实 file_path ──
        let d = super::super::tmdb::tmdb_show_details(serde_json::json!({
            "mediaType": "tv", "trimId": "tm1399", "title": "权力的游戏"
        }))
        .await
        .expect("详情应成功（拿剧照路径用）");
        let path = d
            .get("backdrops")
            .and_then(|v| v.as_array())
            .and_then(|a| a.iter().find_map(|x| x.as_str()))
            .expect("应至少有一张剧照")
            .to_string();
        let url = format!("{TMDB_IMG_BASE}/img/t/p/w92{path}");
        let r = public_client().get(&url).send().await.expect("TMDB 图片请求失败");
        let st = r.status();
        let ct = r
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let n = r.bytes().await.map(|b| b.len()).unwrap_or(0);
        println!("tmdb img {url} → {st} {ct} {n}B");
        assert!(st.is_success(), "TMDB 图片域应 200（我们的 UA 不被拦）：{st}");
        assert!(ct.starts_with("image/"), "应是图片类型：{ct}");
        assert!(n > 100, "图片内容不应为空：{n}");

        // ── ② Bangumi 图片域（lain.bgm.tv，https）：从官方 API 取一张真图的相对路径 ──
        let sub: serde_json::Value = public_client()
            .get("https://api.bgm.tv/v0/subjects/8")
            .header("accept", "application/json")
            .send()
            .await
            .expect("Bangumi API 请求失败")
            .json()
            .await
            .expect("Bangumi API 响应不是 JSON");
        let rel = sub
            .pointer("/images/large")
            .or_else(|| sub.pointer("/images/common"))
            .and_then(|v| v.as_str())
            .expect("Bangumi 条目应有封面");
        let rel = rel.replace("http://lain.bgm.tv", "").replace("https://lain.bgm.tv", "");
        let url = format!("{BGM_IMG_BASE}{}", if rel.starts_with('/') { rel } else { format!("/{rel}") });
        let r = public_client().get(&url).send().await.expect("Bangumi 图片请求失败");
        let st = r.status();
        let n = r.bytes().await.map(|b| b.len()).unwrap_or(0);
        println!("bgm img {url} → {st} {n}B");
        assert!(st.is_success(), "lain.bgm.tv 应 200（https + 我们的 UA）：{st}");
        assert!(n > 100, "图片内容不应为空：{n}");
    }

    /// 路径闸门（不触网）：`__tmdb/__bgm` 前缀解析 + `..`/反斜杠/空段一律拒绝。
    #[test]
    fn target_resolution_and_path_guard() {
        use super::{resolve_target, Target};
        match resolve_target("/__tmdb/t/p/w500/a.jpg").expect("应解析") {
            Target::Public { key, url } => {
                assert_eq!(key, "img:tmdb:t/p/w500/a.jpg");
                assert_eq!(url, format!("{TMDB_IMG_BASE}/img/t/p/w500/a.jpg"));
            }
            _ => panic!("应是 Public 目标"),
        }
        match resolve_target("/__bgm/pic/user/l/1.jpg").expect("应解析") {
            Target::Public { key, url } => {
                assert_eq!(key, "img:bgm:pic/user/l/1.jpg");
                assert_eq!(url, format!("{BGM_IMG_BASE}/pic/user/l/1.jpg"));
            }
            _ => panic!("应是 Public 目标"),
        }
        // NAS 分支不受影响（原样透传相对路径）
        match resolve_target("/f5/04/abc.webp").expect("应解析") {
            Target::Nas(p) => assert_eq!(p, "/f5/04/abc.webp"),
            _ => panic!("应是 Nas 目标"),
        }
        assert!(resolve_target("/__tmdb/../secret").is_err(), "必须拒绝 ..");
        assert!(resolve_target("/__tmdb/a//b.jpg").is_err(), "必须拒绝空段");
        assert!(resolve_target("/").is_err());
    }
}
