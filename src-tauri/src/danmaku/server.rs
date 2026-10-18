//! 极简 HTTP/1.1 服务（`std::net::TcpListener` + 每连接一个线程，零新增依赖）。
//!
//! Rust 引擎开启时监听 127.0.0.1:22347（Lua 硬编码端口），路由：
//!   GET /health                        → {ok:true,engine:"rust",...}
//!   GET /danmaku?...                   → 自建源三跳；未命中/低于阈值反代内部 Node 端口
//!   GET /danmaku-candidates?...        → 自建源候选置顶 + 反代 Node 候选并重排 index
//!   其余（含 /danmaku-pages、/danmaku-by-bvid、/danmaku-cookie-status、NAS 字幕）→ 原样反代

use crate::danmaku::client::SelfHostedClient;
use crate::danmaku::result_json;
use crate::danmaku::types::DanmuConfigFile;
use crate::danmaku::xml;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct ServerHandle {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

static SERVER: Mutex<Option<ServerHandle>> = Mutex::new(None);

#[allow(dead_code)]
pub fn is_running() -> bool {
    SERVER.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// 启动 Rust 弹幕服务（幂等）。`upstream` = 内部 Node shim 端口（B站/字幕反代目标）。
pub fn start(listen: u16, upstream: u16) -> Result<(), String> {
    let mut guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        return Ok(());
    }
    let listener = TcpListener::bind(("127.0.0.1", listen))
        .map_err(|e| format!("Rust 弹幕服务绑定 127.0.0.1:{listen} 失败: {e}"))?;
    let _ = listener.set_nonblocking(true);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let join = std::thread::spawn(move || {
        while !stop_thread.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    std::thread::spawn(move || {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(200)));
                        handle_conn(stream, upstream);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
    });
    *guard = Some(ServerHandle {
        stop,
        join: Some(join),
    });
    crate::libmpv::trace(&format!(
        "[danmaku] Rust 引擎已监听 127.0.0.1:{listen}（上游 Node {upstream}）"
    ));
    Ok(())
}

/// 停止 Rust 弹幕服务并释放 22347（切回 node 引擎时用）。
pub fn stop() {
    let handle = SERVER.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(mut h) = handle {
        h.stop.store(true, Ordering::SeqCst);
        if let Some(join) = h.join.take() {
            let _ = join.join();
        }
        crate::libmpv::trace("[danmaku] Rust 引擎已停止");
    }
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "Status",
    }
}

fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status_reason(status),
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

const MAX_HEADER_BYTES: usize = 64 * 1024;
/// 请求体上限（shim 端点都是 GET；留余量给可能的 POST，但拒绝按声明长度无限分配）。
const MAX_REQUEST_BODY: usize = 4 * 1024 * 1024;

fn read_request(
    stream: &mut TcpStream,
) -> Result<(String, String, Vec<(String, String)>, Vec<u8>), (u16, &'static str)> {
    let mut data: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end;
    loop {
        let n = stream
            .read(&mut tmp)
            .map_err(|_| (400u16, "读取请求失败"))?;
        if n == 0 {
            return Err((400, "连接在请求头完成前关闭"));
        }
        data.extend_from_slice(&tmp[..n]);
        // 先做上限检查再找终止符：否则「超大缓冲区恰好带 \r\n\r\n」会绕过限制
        if data.len() > MAX_HEADER_BYTES {
            return Err((400, "请求头过大"));
        }
        if let Some(pos) = find_subsequence(&data, b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
    }
    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let req_line = lines.next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_lowercase();
            let v = v.trim().to_string();
            if k == "content-length" {
                content_length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    if content_length > MAX_REQUEST_BODY {
        return Err((413, "请求体过大"));
    }
    let mut body = data[header_end..].to_vec();
    if body.len() > MAX_REQUEST_BODY {
        return Err((413, "请求体过大"));
    }
    while body.len() < content_length {
        let n = stream
            .read(&mut tmp)
            .map_err(|_| (400u16, "读取请求体失败"))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
        if body.len() > MAX_REQUEST_BODY {
            return Err((413, "请求体过大"));
        }
    }
    body.truncate(content_length);
    Ok((method, target, headers, body))
}

fn handle_conn(mut stream: TcpStream, upstream: u16) {
    let (method, target, headers, body) = match read_request(&mut stream) {
        Ok(r) => r,
        Err((status, msg)) => {
            let value = json!({"ok": false, "error": msg});
            let bytes = serde_json::to_vec(&value).unwrap_or_default();
            write_response(
                &mut stream,
                status,
                "application/json; charset=utf-8",
                &bytes,
            );
            return;
        }
    };
    let (status, ctype, resp) =
        tauri::async_runtime::block_on(route(&method, &target, &headers, &body, upstream));
    write_response(&mut stream, status, &ctype, &resp);
}

fn json_body(status: u16, value: &Value) -> (u16, String, Vec<u8>) {
    (
        status,
        "application/json; charset=utf-8".into(),
        serde_json::to_vec(value).unwrap_or_else(|_| b"{\"ok\":false}".to_vec()),
    )
}

fn target_path(target: &str) -> &str {
    target.split('?').next().unwrap_or(target)
}

fn query_params(target: &str) -> HashMap<String, String> {
    let q = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    url::form_urlencoded::parse(q.as_bytes())
        .into_owned()
        .collect()
}

fn read_danmu_config() -> DanmuConfigFile {
    let path = std::env::temp_dir().join("anibox-danmu-config.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<DanmuConfigFile>(&s).ok())
        .unwrap_or_default()
}

fn read_series_key() -> String {
    let path = std::env::temp_dir().join("anibox-play-meta.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return String::new();
    };
    for k in ["parentGuid", "episodeGuid"] {
        if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn parse_i64(q: &HashMap<String, String>, key: &str) -> Option<i64> {
    q.get(key).and_then(|s| s.trim().parse::<i64>().ok())
}

fn parse_u32(q: &HashMap<String, String>, key: &str) -> u32 {
    q.get(key)
        .and_then(|s| s.trim().parse::<i64>().ok())
        .map(|v| if v > 0 { v as u32 } else { 0 })
        .unwrap_or(0)
}

async fn route(
    method: &str,
    target: &str,
    headers: &[(String, String)],
    body: &[u8],
    upstream: u16,
) -> (u16, String, Vec<u8>) {
    let path = target_path(target);
    match path {
        "/health" => json_body(
            200,
            &json!({
                "ok": true,
                "ready": true,
                "engine": "rust",
                "upstream_port": upstream,
            }),
        ),
        "/danmaku" => handle_danmaku(target, upstream).await,
        "/danmaku-candidates" => handle_candidates(target, upstream).await,
        _ => proxy(upstream, method, target, headers, body).await,
    }
}

async fn handle_danmaku(target: &str, upstream: u16) -> (u16, String, Vec<u8>) {
    let q = query_params(target);
    let title = q.get("title").cloned().unwrap_or_default();
    let out = q.get("out").cloned().unwrap_or_default();
    if title.trim().is_empty() || out.trim().is_empty() {
        return json_body(400, &json!({"ok": false, "error": "缺少 title 或 out 参数"}));
    }
    if !xml::is_safe_path(&out) {
        return json_body(
            403,
            &json!({"ok": false, "error": "out 路径不在允许的弹幕缓存目录内"}),
        );
    }

    let cfg = read_danmu_config();
    let bases = cfg.effective_bases();
    if !cfg.enabled || bases.is_empty() {
        return proxy(upstream, "GET", target, &[], &[]).await;
    }

    let client = SelfHostedClient::new(bases);
    let ep = parse_i64(&q, "ep").unwrap_or(0);
    let season = parse_u32(&q, "season");
    let threshold = parse_i64(&q, "threshold").unwrap_or(0);
    let alt = q.get("alt").cloned().unwrap_or_default();
    let mut series = q.get("guid").cloned().unwrap_or_default();
    if series.is_empty() {
        series = read_series_key();
    }

    let outcome = client
        .auto_fetch(&title, ep, &out, season, "", &series, &alt)
        .await;

    match outcome {
        Some(o) if o.ok => {
            // 自建源命中但低于聚合阈值 → 交给内部 Node 走「自建源 + 内置B站」合并语义
            if threshold > 0 && o.danmaku_count < threshold {
                let (st, ct, b) = proxy(upstream, "GET", target, &[], &[]).await;
                if (200..300).contains(&st) {
                    return (st, ct, b);
                }
                crate::libmpv::trace(
                    "[danmaku] 低于阈值反代 Node 失败，保留自建源结果",
                );
            }
            json_body(200, &result_json(&o))
        }
        _ => proxy(upstream, "GET", target, &[], &[]).await,
    }
}

fn is_self_candidate(c: &Value) -> bool {
    c.get("source").and_then(|v| v.as_str()) == Some("自建源")
        || c.get("bvid")
            .and_then(|v| v.as_str())
            .map(|s| s.starts_with("dmapi:"))
            .unwrap_or(false)
}

async fn handle_candidates(target: &str, upstream: u16) -> (u16, String, Vec<u8>) {
    let q = query_params(target);
    let title = q.get("title").cloned().unwrap_or_default();
    if title.trim().is_empty() {
        return json_body(400, &json!({"ok": false, "error": "缺少 title 参数"}));
    }
    let ep = parse_i64(&q, "ep").unwrap_or(0);
    let season = parse_u32(&q, "season");

    let cfg = read_danmu_config();
    let bases = cfg.effective_bases();
    if !cfg.enabled || bases.is_empty() {
        return proxy(upstream, "GET", target, &[], &[]).await;
    }

    let client = SelfHostedClient::new(bases);
    let self_cands = client.candidates(&title, ep, season).await;
    let self_count = self_cands.len();
    let self_empty = self_cands.is_empty();

    let (ust, _uct, ub) = proxy(upstream, "GET", target, &[], &[]).await;
    let upstream_json: Value = if (200..300).contains(&ust) {
        serde_json::from_slice(&ub).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let upstream_cands: Vec<Value> = upstream_json
        .get("candidates")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut merged: Vec<Value> = Vec::new();
    if self_empty {
        merged.extend(upstream_cands);
    } else {
        merged.extend(self_cands);
        for c in upstream_cands {
            if is_self_candidate(&c) {
                continue;
            }
            merged.push(c);
        }
    }
    for (i, c) in merged.iter_mut().enumerate() {
        if let Some(obj) = c.as_object_mut() {
            obj.insert("index".into(), json!(i));
        }
    }

    let self_status = json!({
        "active": true,
        "matched": !self_empty,
        "count": self_count,
    });

    if merged.is_empty() {
        let err = upstream_json
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("未找到候选");
        return json_body(
            200,
            &json!({"ok": false, "error": err, "selfHosted": self_status}),
        );
    }
    json_body(
        200,
        &json!({"ok": true, "candidates": merged, "selfHosted": self_status}),
    )
}

async fn proxy(
    upstream: u16,
    method: &str,
    target: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> (u16, String, Vec<u8>) {
    let url = format!("http://127.0.0.1:{upstream}{target}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(200))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut rb = client.request(m, &url);
    for (k, v) in headers {
        if matches!(k.as_str(), "content-type" | "authorization" | "accept") {
            rb = rb.header(k, v);
        }
    }
    if !body.is_empty() {
        rb = rb.body(body.to_vec());
    }
    match rb.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let ctype = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json; charset=utf-8")
                .to_string();
            let bytes = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            (status, ctype, bytes)
        }
        Err(e) => json_body(
            502,
            &json!({"ok": false, "error": format!("上游弹幕服务不可达: {e}")}),
        ),
    }
}
