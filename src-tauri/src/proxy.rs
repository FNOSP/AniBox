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
        return Err(format!(
            "Go 代理不存在: {}（可在登录页高级设置里改为 Fntv-Plus 安装目录下的 third_party/proxy/proxy.exe）",
            proxy_path
        ));
    }
    use std::os::windows::process::CommandExt as _;
    let child = std::process::Command::new(proxy_path)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
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

/// 构造播放直链 URL，逐段对齐 media.ts:831-842 getProxyUrl
pub fn play_url(cfg: &crate::config::Config, item_guid: &str) -> String {
    let domain = urlencoding::encode(cfg.server.trim_end_matches('/')).to_string();
    let cookie = format!("Trim-MC-token={}", cfg.token);
    format!(
        "http://127.0.0.1:{}/api/v1/playvideo/{}?token={}&skipVerify=1&account={}&domain={}&useNasLocal={}&sourceIndex=0&cookie={}",
        PROXY_PORT,
        item_guid,
        urlencoding::encode(&cfg.token),
        urlencoding::encode(&cfg.username),
        domain,
        if cfg.nas_proxy_enabled { 1 } else { 0 },
        urlencoding::encode(&cookie)
    )
}

#[allow(dead_code)]
fn _unused(_v: &Value) {}
