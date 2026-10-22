//! Go 代理守护：对齐 Fntv-Plus `src/main/common/proxy.ts` 的关键行为 ——
//! 先探测 127.0.0.1:22346 是否已在监听（复用已运行实例），否则 spawn proxy.exe 并轮询端口就绪。

use serde_json::Value;

pub const PROXY_PORT: u16 = 22346;

fn port_ready() -> bool {
    std::net::TcpStream::connect(("127.0.0.1", PROXY_PORT)).is_ok()
}

/// 就绪则直接返回；否则拉起 proxy.exe 并等待端口监听（最长 8s）。
pub async fn ensure(
    proxy_path: &str,
    holder: &tokio::sync::Mutex<Option<std::process::Child>>,
) -> Result<(), String> {
    if port_ready() {
        return Ok(());
    }
    if !std::path::Path::new(proxy_path).exists() {
        #[cfg(target_os = "windows")]
        let hint = "third_party/proxy/proxy.exe";
        #[cfg(not(target_os = "windows"))]
        let hint = "third_party/proxy/proxy";
        return Err(format!(
            "Go 代理不存在: {}（可在登录页高级设置里改为安装目录下的 {}）",
            proxy_path, hint
        ));
    }
    // [macos] 打包进 .app 的 proxy 可能丢失可执行位（dmg → 拖进 /Applications 的复制链路
    // 不保证保留 mode）。spawn 前主动补一次 0o755；失败不致命（真没权限时 spawn 会报清晰错误）。
    #[cfg(not(target_os = "windows"))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Ok(meta) = std::fs::metadata(proxy_path) {
            if meta.permissions().mode() & 0o111 == 0 {
                let _ = std::fs::set_permissions(
                    proxy_path,
                    std::fs::Permissions::from_mode(0o755),
                );
            }
        }
    }
    let mut cmd = std::process::Command::new(proxy_path);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // CREATE_NO_WINDOW 仅 Windows 有；其它平台无需处理。
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("启动 Go 代理失败: {}", e))?;
    let _ = holder.lock().await.insert(child);
    for _ in 0..40 {
        if port_ready() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err("Go 代理启动超时（127.0.0.1:22346 未监听）".into())
}

/// [lc-101] 预检直链：Go 代理解析失败时返回的是 **HTTP 500 + JSON 错误体**
/// （实测日志：`[e][curl] HTTP error 500` + `type=application/json`），而 mpv 侧只会给
/// "Failed to open" 这种笼统错误 —— 用户看到的就是"半天没反应、错误不显示"。
/// 这里先用 reqwest 探一次：非 2xx 直接把代理给的 msg/正文原文抛出来当错误文案；
/// 2xx 则只是读了响应头就丢弃连接（顺带把代理/连接预热了，代价一个网络缓冲）。
pub async fn probe(url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("构建探测客户端失败: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("直链不可达（Go 代理 127.0.0.1:22346 在跑吗？）: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    let body_trim: String = body.chars().take(400).collect();
    let msg = serde_json::from_str::<Value>(&body_trim)
        .ok()
        .and_then(|v| {
            ["msg", "message", "error", "err", "data"]
                .iter()
                .find_map(|k| v.get(*k).and_then(|m| m.as_str()).map(|s| s.to_string()))
        })
        .unwrap_or_else(|| body_trim.clone());
    Err(format!("直链返回 {status}：{msg}"))
}

/// JS `escape()` 的等价实现。主仓库 `getProxyUrl` 用的是 `escape(cfg.domain)`（不是
/// `encodeURIComponent`）：不转义 `A-Za-z0-9@*_+-./`，其余转 `%XX` ⇒ domain 里的 `:` `/` 保持原样。
/// 我们原来用 urlencoding 会把 `/` 编成 `%2F`，虽然代理解码后等价，但既然要抄就抄一致。
fn js_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '@' | '*' | '_' | '+' | '-' | '.' | '/') {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// 构造播放直链 URL，逐段对齐 `media.ts:831-842 getProxyUrl`：
/// `token`/`account` 原样插入；`domain` 走 `escape()`；`cookie` 走 `encodeURIComponent` 且仅在非空时追加；
/// `skipVerify` 在 Electron 侧按"可信/私有主机"判定，本 PoC 的 NAS 都是内网地址 ⇒ 恒为 1。
pub fn play_url(cfg: &crate::config::Config, item_guid: &str) -> String {
    let domain = js_escape(cfg.server.trim_end_matches('/'));
    let cookie_raw = format!("Trim-MC-token={}", cfg.token);
    let cookie_param = if cookie_raw.trim().is_empty() {
        String::new()
    } else {
        format!("&cookie={}", urlencoding::encode(&cookie_raw))
    };
    format!(
        "http://127.0.0.1:{}/api/v1/playvideo/{}?token={}&skipVerify=1&account={}&domain={}&useNasLocal={}&sourceIndex=0{}",
        PROXY_PORT,
        item_guid,
        cfg.token,
        cfg.username,
        domain,
        if cfg.nas_proxy_enabled { 1 } else { 0 },
        cookie_param
    )
}

#[allow(dead_code)]
fn _unused(_v: &Value) {}
