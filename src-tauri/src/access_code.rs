//! fnOS 网关「应用访问码」会话建立（移植自 Fntv-Plus
//! `src/main/common/accessCodeSession.ts`，按 AniBox 的 Tauri/Rust 架构适配）。
//!
//! 契约（与旧项目一致）：
//!   - 验证端点 `GET /access_code_verify`；
//!   - 请求头 `x-access-code: base64(访问码)`、`x-access-source: web`；
//!   - 手动跟随重定向（最多 5 次），仅允许同主机、禁止 https→http 降级；
//!   - 401/403/429 = 访问码被拒绝；404 = 网关未开启门禁（放行）；
//!   - 2xx = 通过，返回跟随重定向后的真实 origin（调用方回写 server）；
//!   - 网关授权 Cookie 由 `fnapi::capture_cookies` 记入进程内会话表，后续 API 自动携带；
//!   - 明文访问码只作为内存参数，不写日志、不拼进 URL。

use base64::Engine;
use std::time::Duration;

const MAX_REDIRECTS: usize = 5;
const VERIFY_TIMEOUT_MS: u64 = 10_000;
/// 访问码被网关明确拒绝的状态码（与旧项目一致）
const REJECTED_STATUS_CODES: [u16; 3] = [401, 403, 429];

#[derive(Debug)]
pub enum AccessCodeError {
    /// 访问码错误（401/403/429）
    Rejected,
    /// 网络 / 服务异常 / 不安全重定向
    Network(String),
}

impl std::fmt::Display for AccessCodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccessCodeError::Rejected => write!(f, "访问码错误"),
            AccessCodeError::Network(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for AccessCodeError {}

fn encode_access_code(code: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(code.as_bytes())
}

fn network(msg: impl Into<String>) -> AccessCodeError {
    AccessCodeError::Network(msg.into())
}

/// 建立 fnOS 访问码网关会话。
///
/// - 返回 `Ok(baseUrl)`：验证通过（或网关无门禁 404）后的真实 origin；
/// - 返回 `Err(Rejected)`：访问码被拒绝；
/// - 返回 `Err(Network(..))`：连不上 / 服务异常 / 不安全重定向。
pub async fn establish(base_url: &str, access_code: &str) -> Result<String, AccessCodeError> {
    let code = access_code.trim();
    if code.is_empty() {
        return Ok(base_url.to_string());
    }

    let initial = format!("{}/access_code_verify", base_url.trim_end_matches('/'));
    let mut current = url::Url::parse(&initial)
        .map_err(|_| network("访问码验证地址无效"))?;

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(VERIFY_TIMEOUT_MS))
        .build()
        .map_err(|e| network(format!("访问码验证客户端初始化失败: {e}")))?;

    let encoded = encode_access_code(code);

    for _ in 0..=MAX_REDIRECTS {
        let resp = client
            .get(current.as_str())
            .header("x-access-code", &encoded)
            .header("x-access-source", "web")
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    network("访问码验证请求超时")
                } else if e.is_connect() {
                    network("无法连接到访问码验证服务")
                } else {
                    network(format!("访问码验证请求失败: {e}"))
                }
            })?;

        // 网关授权 Cookie 落表（后续 API 请求自动携带）
        crate::fnapi::capture_cookies(current.as_str(), resp.headers());

        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            if location.is_empty() {
                return Err(network("访问码验证重定向缺少 Location"));
            }
            let next = current
                .join(&location)
                .map_err(|_| network("访问码验证重定向地址无效"))?;
            // 仅允许同主机；禁止 https→http 降级（防止把访问码带去别处）
            let same_host = next.host_str() == current.host_str();
            let secure_transition = !(current.scheme() == "https" && next.scheme() == "http");
            if !same_host || !secure_transition {
                return Err(network("访问码验证拒绝跨主机或不安全重定向"));
            }
            current = next;
            continue;
        }

        if REJECTED_STATUS_CODES.contains(&status) {
            return Err(AccessCodeError::Rejected);
        }
        // 404 = 网关没有验证端点（未开启访问码门禁 / 中继域名）：视为放行，
        // 原样返回输入地址（不得返回 origin 丢掉可能存在的路径前缀）。
        if status == 404 {
            return Ok(base_url.to_string());
        }
        if !(200..300).contains(&status) {
            return Err(network(format!("访问码验证服务返回 HTTP {status}")));
        }

        return Ok(current.origin().ascii_serialization());
    }

    Err(network("访问码验证重定向次数过多"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_access_code_as_base64() {
        assert_eq!(encode_access_code("1234"), "MTIzNA==");
    }

    #[tokio::test]
    async fn empty_code_is_passthrough() {
        // 空访问码不发起网络请求，原样返回
        let out = establish("http://127.0.0.1:1", "   ").await.unwrap();
        assert_eq!(out, "http://127.0.0.1:1");
    }
}
