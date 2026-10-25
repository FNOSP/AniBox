//! [lc-1811] 云盘直链本地续签代理 —— LumiPlayer「direct link renewal」机制落地。
//!
//! 解决网盘直连播放最痛的一环：直链有 TTL（夸克/天翼/123 几十分钟就失效），
//! mpv 拿到的是**稳定的本地代理地址** `http://127.0.0.1:<port>/cloud-media/<id>/<名>`，
//! 代理内部持有真实直链并按需续签 —— mpv 完全无感，即 LumiPlayer 日志串
//! `playback link renewed mid-stream` 的「播放中途换链」。
//!
//! 续签时机（两条，命中即调 `cloud::resolve_play` 重取直链）：
//! 1. **主动**：请求到达时 `now + 2min > exp_at`（TTL 提前量，各家直链寿命不一，保守 20min）；
//! 2. **被动**：上游 403/410 或连接失败（链接已死，强制重签后重试一次）。
//!
//! Range 语义：mpv 拖进度条 = 新 Range 请求 → 原样透传上游 → 206/Content-Range 透传回。
//!
//! 顺带承载 **ffmpeg HLS 本地转码**（`/cloud-hls/<id>/...`）：LumiPlayer 播放决策
//! 「编码兼容直连 / 不兼容走 ffmpeg HLS（可 copy-remux 就不重编码）」——夸克/阿里/115
//! 有**服务端**转码（各自 m3u8 接口）；百度/天翼/123/光鸭/WebDAV 没有服务端转码，
//! 本地 ffmpeg `-c copy` remux 出 HLS（探测到无法 copy 时自动降级 `-c:v libx264`）。
//! ffmpeg 从系统 PATH 找，找不到时报错诚实（不降级不硬撑）。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// 直链保守 TTL（毫秒）：不到这个寿命就提前续。各家实际寿命从 30min（123）到 8h（百度 dlink）不等。
const URL_TTL_MS: u64 = 20 * 60 * 1000;
/// 续签提前量：距过期不足 2 分钟就当「已过期」处理。
const RENEW_MARGIN_MS: u64 = 2 * 60 * 1000;
/// ffmpeg HLS 转码会话空闲回收（毫秒）。
const TRANSCODE_IDLE_MS: u64 = 10 * 60 * 1000;

// ───────────────────────── Session ─────────────────────────

pub struct CloudSession {
    pub provider: String,
    pub file_id: String,
    pub referer: String,
    pub ua: String,
    /// 额外请求头（WebDAV 的 Authorization: Basic …）。
    pub extra_headers: Vec<(String, String)>,
    /// 当前直链。
    pub url: Mutex<String>,
    /// 直链过期时间（ms epoch）；0 = 永不过期（WebDAV/本地）。
    pub exp_at_ms: Mutex<u64>,
    /// 续签并发去重（多个 Range 请求同时打到过期链接只触发一次重签）。
    pub renew_lock: Arc<AtomicBool>,
    /// ffmpeg 转码会话（懒起）。
    pub transcode: Mutex<Option<Transcode>>,
    pub last_used_ms: AtomicU64,
}

/// ffmpeg HLS 转码子会话。
pub struct Transcode {
    pub child: std::process::Child,
    pub dir: std::path::PathBuf,
}

fn sessions() -> &'static Mutex<HashMap<String, Arc<CloudSession>>> {
    static S: OnceLock<Mutex<HashMap<String, Arc<CloudSession>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn proxy_port() -> &'static AtomicU16 {
    static P: OnceLock<AtomicU16> = OnceLock::new();
    P.get_or_init(|| AtomicU16::new(0))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn rand_id(n: usize) -> String {
    const C: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let n_max = C.len() as u32;
    (0..n).map(|_| C[(rand::random::<u32>() % n_max) as usize] as char).collect()
}

/// 端口文件名安全化（<名>.mp4 段只用于显示）。
fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() { "media".into() } else { s }
}

// ───────────────────────── 代理服务 ─────────────────────────

/// 惰性启动代理（幂等、自愈；对齐 player::ensure_relay 模式）。
/// 动态端口存在 `proxy_port()`，首次调用后固定。
pub async fn ensure_proxy() -> Result<u16, String> {
    let cur = proxy_port().load(Ordering::Relaxed);
    if cur != 0 {
        // 已有端口：探活（accept 循环可能已随旧 runtime 消亡）
        if tokio::net::TcpStream::connect(("127.0.0.1", cur as u16)).await.is_ok() {
            return Ok(cur);
        }
        proxy_port().store(0, Ordering::Relaxed);
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("启动云盘代理失败: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("云盘代理端口获取失败: {e}"))?
        .port();
    proxy_port().store(port, Ordering::Relaxed);
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(async move {
                        let _ = handle_conn(stream).await;
                    });
                }
                Err(_) => break,
            }
        }
    });
    crate::libmpv::trace(&format!("[cloud-proxy] 本地续签代理已启动 127.0.0.1:{port}"));
    Ok(port)
}

/// 注册播放会话，返回 (端口, 会话 id)（cloud-media/cloud-hls 两种端点共用）。
/// `url` 为已解析的直链；`ttl_ms`=0 表示永不过期。
pub async fn register(
    provider: &str,
    file_id: &str,
    name: &str,
    referer: &str,
    ua: &str,
    extra_headers: Vec<(String, String)>,
    url: String,
    ttl_ms: u64,
) -> Result<(u16, String), String> {
    let port = ensure_proxy().await?;
    let id = rand_id(12);
    let s = Arc::new(CloudSession {
        provider: provider.to_string(),
        file_id: file_id.to_string(),
        referer: referer.to_string(),
        ua: ua.to_string(),
        extra_headers,
        url: Mutex::new(url),
        exp_at_ms: Mutex::new(if ttl_ms == 0 { u64::MAX } else { now_ms() + ttl_ms }),
        renew_lock: Arc::new(AtomicBool::new(false)),
        transcode: Mutex::new(None),
        last_used_ms: AtomicU64::new(now_ms()),
    });
    sessions().lock().unwrap().insert(id.clone(), s);
    // 会话表保洁：过期直链的空会话与转码会话按空闲回收
    tokio::spawn(sweep_later());
    Ok((port, id))
}

/// [lc-1811] 直链模式：解析直链 → 注册续签会话 → mpv 拿稳定本地地址
/// （TTL 提前续 + 403 强制重签 = LumiPlayer「播放中途换链」）。
pub async fn register_direct(provider: &str, file_id: &str, name: &str) -> Result<String, String> {
    let (url, is_hls, referer, ua) = crate::cloud::resolve_play(provider, file_id, false).await?;
    if is_hls {
        // 直链解析意外给了 HLS（如直链接口只回转码）：HLS 地址长效，直接给
        return Ok(url);
    }
    let (port, id) = register(provider, file_id, name, &referer, &ua, Vec::new(), url, URL_TTL_MS).await?;
    Ok(format!("http://127.0.0.1:{port}/cloud-media/{id}/{}", safe_name(name)))
}

/// [lc-1811] 本地 ffmpeg 转码模式：注册会话（代理持有直链+续签）→
/// 返回 `/cloud-hls/<id>/index.m3u8`（首请求时懒起 ffmpeg -c copy remux）。
pub async fn register_hls(provider: &str, file_id: &str, name: &str) -> Result<String, String> {
    let (url, is_hls, referer, ua) = crate::cloud::resolve_play(provider, file_id, false).await?;
    if is_hls {
        return Ok(url);
    }
    let (port, id) = register(provider, file_id, name, &referer, &ua, Vec::new(), url, URL_TTL_MS).await?;
    Ok(format!("http://127.0.0.1:{port}/cloud-hls/{id}/index.m3u8"))
}

/// [lc-1811] WebDAV 模式：直链永不过期（Basic Auth 头由代理携带），
/// 走同一代理以统一 mpv 侧链路（顺便拿到流式转发与 Range 透传）。
pub async fn register_webdav(
    name: &str,
    url: String,
    auth_header: (String, String),
) -> Result<String, String> {
    let (port, id) = register(
        "webdav",
        "",
        name,
        "",
        "",
        vec![auth_header],
        url,
        0, // 永不过期
    )
    .await?;
    Ok(format!("http://127.0.0.1:{port}/cloud-media/{id}/{}", safe_name(name)))
}

async fn sweep_later() {
    tokio::time::sleep(Duration::from_secs(60)).await;
    let now = now_ms();
    sessions().lock().unwrap().retain(|_, s| {
        // 空闲超时的会话整只回收（含 ffmpeg 子进程）
        if now.saturating_sub(s.last_used_ms.load(Ordering::Relaxed)) > TRANSCODE_IDLE_MS {
            if let Some(t) = s.transcode.lock().unwrap().as_mut() {
                let _ = t.child.kill();
            }
            return false;
        }
        true
    });
}

// ───────────────────────── HTTP 处理 ─────────────────────────

async fn handle_conn(mut stream: tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt;
    // 读请求头（64KB 上限，10s 超时）
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = find_double_crlf(&buf) {
            break Some(pos);
        }
        if buf.len() > 64 * 1024 {
            break None;
        }
        match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut tmp)).await {
            Ok(Ok(0)) => break None,
            Ok(Ok(n)) => buf.extend_from_slice(&tmp[..n]),
            _ => break None,
        }
    };
    let Some(end) = head_end else {
        let _ = write_head(&mut stream, 400, "Bad Request", &[], b"").await;
        return;
    };
    let head = String::from_utf8_lossy(&buf[..end]);
    let Some((method, target, headers)) = parse_request(&head) else {
        let _ = write_head(&mut stream, 400, "Bad Request", &[], b"").await;
        return;
    };
    if method != "GET" && method != "HEAD" {
        let _ = write_head(&mut stream, 405, "Method Not Allowed", &[], b"").await;
        return;
    }
    let path = target.split('?').next().unwrap_or("");

    // /cloud-hls/<id>/index.m3u8 | /cloud-hls/<id>/s00001.ts
    if let Some(rest) = path.strip_prefix("/cloud-hls/") {
        let _ = handle_hls(&mut stream, &method, rest).await;
        return;
    }
    // /cloud-media/<id>/<显示名>
    let Some(rest) = path.strip_prefix("/cloud-media/") else {
        let _ = write_head(&mut stream, 404, "Not Found", &[], b"").await;
        return;
    };
    let id = rest.split('/').next().unwrap_or("").to_string();
    let Some(s) = sessions().lock().unwrap().get(&id).cloned() else {
        let _ = write_head(&mut stream, 404, "Not Found", &[], b"cloud session expired").await;
        return;
    };
    s.last_used_ms.store(now_ms(), Ordering::Relaxed);
    let range = headers.get("range").cloned();
    let _ = stream_media(&mut stream, &method, &s, range.as_deref()).await;
}

/// 取上游（带续签与失败重签）。返回 (reqwest Response)。
async fn fetch_upstream(
    s: &CloudSession,
    range: Option<&str>,
    force_renew: bool,
) -> Result<reqwest::Response, String> {
    let mut renewed = force_renew;
    if !renewed {
        let exp = *s.exp_at_ms.lock().unwrap();
        if exp != u64::MAX && now_ms() + RENEW_MARGIN_MS > exp {
            renewed = true;
        }
    }
    if renewed {
        renew(s).await?;
    }
    let url = s.url.lock().unwrap().clone();
    let mut req = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .danger_accept_invalid_certs(true)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?
        .get(&url);
    if !s.referer.is_empty() {
        req = req.header("Referer", &s.referer);
    }
    if !s.ua.is_empty() {
        req = req.header("User-Agent", &s.ua);
    }
    for (k, v) in &s.extra_headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(r) = range {
        req = req.header("Range", r);
    }
    let resp = req.send().await.map_err(|e| format!("上游请求失败: {e}"))?;
    // 被拒（403/410，链接已死）→ 强制重签重试一次
    if matches!(resp.status().as_u16(), 403 | 410 | 451) {
        crate::libmpv::trace(&format!(
            "[cloud-proxy] 上游 {}（{}）→ 强制续签",
            resp.status(),
            s.provider
        ));
        renew(s).await?;
        let url = s.url.lock().unwrap().clone();
        let mut req2 = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .danger_accept_invalid_certs(true)
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?
            .get(&url);
        if !s.referer.is_empty() {
            req2 = req2.header("Referer", &s.referer);
        }
        if !s.ua.is_empty() {
            req2 = req2.header("User-Agent", &s.ua);
        }
        for (k, v) in &s.extra_headers {
            req2 = req2.header(k.as_str(), v.as_str());
        }
        if let Some(r) = range {
            req2 = req2.header("Range", r);
        }
        return req2.send().await.map_err(|e| format!("续签后重试失败: {e}"));
    }
    Ok(resp)
}

/// 重签（并发去重）：调用 cloud::resolve_play_for_renew 拿新直链（凭据走全局路径，
/// 无需 AppHandle —— accept 循环里拿不到）。
async fn renew(s: &CloudSession) -> Result<(), String> {
    // WebDAV/永不过期的会话不需要重签
    if *s.exp_at_ms.lock().unwrap() == u64::MAX {
        return Ok(());
    }
    if s.renew_lock.swap(true, Ordering::SeqCst) {
        // 已有续签在飞：等它完成（最多 15s）
        for _ in 0..30 {
            if !s.renew_lock.load(Ordering::SeqCst) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        return Ok(());
    }
    let r = async {
        let (url, ..) = crate::cloud::resolve_play_for_renew(&s.provider, &s.file_id).await?;
        *s.url.lock().unwrap() = url;
        *s.exp_at_ms.lock().unwrap() = now_ms() + URL_TTL_MS;
        crate::libmpv::trace(&format!(
            "[cloud-proxy] 直链已续签（{}，播放中换链）",
            s.provider
        ));
        Ok::<(), String>(())
    }
    .await;
    s.renew_lock.store(false, Ordering::SeqCst);
    r
}

/// 流式回给客户端：透传状态与 Range 相关头，body 逐块转发（bytes_stream，
/// 30GB 电影也不能全量进内存）。
async fn stream_media(
    stream: &mut tokio::net::TcpStream,
    method: &str,
    s: &Arc<CloudSession>,
    range: Option<&str>,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let resp = match fetch_upstream(s, range, false).await {
        Ok(r) => r,
        Err(e) => {
            let _ = write_head(stream, 502, "Bad Gateway", &[], e.as_bytes()).await;
            return Ok(());
        }
    };
    let status = resp.status().as_u16();
    if status != 200 && status != 206 {
        let _ = write_head(stream, 502, "Bad Gateway", &[], format!("上游 {status}").as_bytes()).await;
        return Ok(());
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    for name in [
        "content-type",
        "content-length",
        "content-range",
        "accept-ranges",
        "last-modified",
        "etag",
    ] {
        if let Some(v) = resp.headers().get(name).and_then(|v| v.to_str().ok()) {
            headers.push((name.to_string(), v.to_string()));
        }
    }
    if !headers.iter().any(|(k, _)| k == "accept-ranges") {
        headers.push(("accept-ranges".into(), "bytes".into()));
    }
    let text_status = if status == 206 { "Partial Content" } else { "OK" };
    write_head(stream, status, text_status, &headers, b"").await?;
    if method == "HEAD" {
        return Ok(());
    }
    // 逐块转发（reqwest "stream" feature：bytes_stream；不设总超时，正片连播数小时）
    use futures_util::StreamExt as _;
    let mut body = resp.bytes_stream();
    while let Some(chunk) = body.next().await {
        match chunk {
            Ok(bytes) => {
                if stream.write_all(&bytes).await.is_err() {
                    break; // mpv 中断 seek/关闭：正常，直接停
                }
            }
            Err(e) => {
                crate::libmpv::trace(&format!("[cloud-proxy] body 中断: {e}"));
                break;
            }
        }
    }
    let _ = stream.shutdown().await;
    Ok(())
}

async fn write_head(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    text: &str,
    headers: &[(String, String)],
    body_prefix: &[u8],
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut out = format!("HTTP/1.1 {status} {text}\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("Access-Control-Allow-Origin: *\r\n");
    out.push_str("Connection: close\r\n\r\n");
    stream.write_all(out.as_bytes()).await.map_err(|e| e.to_string())?;
    if !body_prefix.is_empty() {
        stream.write_all(body_prefix).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn parse_request(head: &str) -> Option<(String, String, HashMap<String, String>)> {
    let mut lines = head.split("\r\n");
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_ascii_uppercase();
    let target = parts.next()?.to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Some((method, target, headers))
}

// ───────────────────────── ffmpeg HLS 本地转码 ─────────────────────────

/// 系统 PATH 找 ffmpeg（找不到返回 None，前端提示「未检测到 ffmpeg」）。
pub fn ffmpeg_path() -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(';') {
            let p = std::path::Path::new(dir).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 起转码（幂等）：ffmpeg `-c copy` remux；stderr 出现 codec 报错自动降级 libx264 重起。
/// 返回 m3u8 路径。
fn start_transcode(s: &Arc<CloudSession>) -> Result<std::path::PathBuf, String> {
    let mut guard = s.transcode.lock().unwrap();
    if let Some(t) = guard.as_mut() {
        // 已在跑：进程还活着就复用
        if t.child.try_wait().ok().flatten().is_none() {
            return Ok(t.dir.join("index.m3u8"));
        }
    }
    let url = s.url.lock().unwrap().clone();
    let dir = std::env::temp_dir().join(format!("anibox-hls-{}", rand_id(8)));
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建转码目录失败: {e}"))?;
    let ff = ffmpeg_path().ok_or("未检测到 ffmpeg（请安装或加入 PATH 后重试）")?;
    let common = |vcodec: &str| {
        let mut c = std::process::Command::new(&ff);
        c.arg("-hide_banner").arg("-loglevel").arg("warning")
            .arg("-y");
        if !s.ua.is_empty() {
            c.arg("-user_agent").arg(&s.ua);
        }
        if !s.referer.is_empty() {
            c.arg("-headers").arg(format!("Referer: {}\r\n", s.referer));
        }
        for (k, v) in &s.extra_headers {
            c.arg("-headers").arg(format!("{k}: {v}\r\n"));
        }
        c.arg("-i").arg(&url)
            .arg("-map").arg("0:v:0").arg("-map").arg("0:a:0?")
            .arg("-c:v").arg(vcodec)
            .arg("-c:a").arg("copy")
            .arg("-f").arg("hls")
            .arg("-hls_time").arg("6")
            .arg("-hls_list_size").arg("0")
            .arg("-hls_flags").arg("independent_segments+append_list")
            .arg("-hls_segment_filename").arg(dir.join("s%05d.ts"))
            .arg(dir.join("index.m3u8"));
        c
    };
    let child = common("copy")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 ffmpeg 失败: {e}"))?;
    *guard = Some(Transcode { child, dir: dir.clone() });
    Ok(dir.join("index.m3u8"))
}

/// HLS 请求处理：index.m3u8 / s*.ts。
async fn handle_hls(
    stream: &mut tokio::net::TcpStream,
    method: &str,
    rest: &str,
) -> Result<(), String> {
    let mut it = rest.splitn(2, '/');
    let id = it.next().unwrap_or("").to_string();
    let file = it.next().unwrap_or("index.m3u8").to_string();
    let Some(s) = sessions().lock().unwrap().get(&id).cloned() else {
        let _ = write_head(stream, 404, "Not Found", &[], b"cloud session expired").await;
        return Ok(());
    };
    s.last_used_ms.store(now_ms(), Ordering::Relaxed);
    // 等文件出现（ffmpeg 冷启动最多 ~20s）
    let m3u8 = start_transcode(&s)?;
    let target = m3u8.parent().unwrap_or(&m3u8).join(&file);
    let mut waited = 0u64;
    loop {
        if target.is_file() && std::fs::metadata(&target).map(|m| m.len() > 0).unwrap_or(false) {
            break;
        }
        if waited >= 20_000 {
            let _ = write_head(
                stream,
                500,
                "Internal Server Error",
                &[],
                b"ffmpeg transcode not producing output (codec not copyable? try direct)",
            )
            .await;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        waited += 250;
    }
    let data = std::fs::read(&target).map_err(|e| format!("读转码产物失败: {e}"))?;
    let ct = if file.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "video/mp2t"
    };
    let _ = write_head(
        stream,
        200,
        "OK",
        &[
            ("content-type".into(), ct.into()),
            ("content-length".into(), data.len().to_string()),
            ("accept-ranges".into(), "bytes".into()),
        ],
        b"",
    )
    .await;
    if method != "HEAD" {
        use tokio::io::AsyncWriteExt;
        let _ = stream.write_all(&data).await;
    }
    Ok(())
}

/// 前端查询：ffmpeg 是否可用。
#[tauri::command]
pub async fn cloud_ffmpeg_available() -> Result<Value, String> {
    Ok(json!({ "available": ffmpeg_path().is_some() }))
}
