//! Authx 签名 —— 与 Fntv-Plus `src/modules/fn_api/request.ts:85-102` 的 genFnAuthx 逐字节对齐。
//! sign 源串 = apiKey_url_nonce_时间戳_dataMd5_apiSecret（"_" 连接后整体 MD5），
//! dataMd5 必须与实际发送的 body 字节完全一致（发送端用同一序列化字符串）。
//!
//! # 签名材料来源（不再硬编码）
//!
//! 这对 key/secret 是飞牛 fnOS 官方 Web 影视客户端内置的**公开客户端凭据**，
//! 不硬编码进源码/产物（对齐 `fpk/src/go/internal/bridge/bridge.go` 的
//! `FNTV_AUTHX_KEY` / `FNTV_AUTHX_SECRET` 语义）。取值优先级：
//!   1. 运行时环境变量 `ANIBOX_FN_AUTHX_KEY` / `ANIBOX_FN_AUTHX_SECRET`（本地 `cargo run` 可直接给）；
//!   2. 构建期注入（`build.rs` 从 `secrets/fn-authx.local` 读取后 `cargo:rustc-env`）。
//! 两者都缺失 → 视为「未注入」，`authx()` 返回 `None`，调用方**不发 `Authx` 头**，
//! 请求照常发出，由飞牛网关按未签名请求处理（与 bridge.go 的 `genAuthx` 一致）。
//!
//! 配置说明见 `docs/SECURITY-notes.md`。

/// 读取运行时环境变量（优先），回退到构建期注入值；空串视为未设置。
fn read_material(env_name: &str, compiled: Option<&'static str>) -> Option<String> {
    if let Ok(v) = std::env::var(env_name) {
        let v = v.trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    compiled
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn authx_key() -> Option<String> {
    read_material("ANIBOX_FN_AUTHX_KEY", option_env!("ANIBOX_FN_AUTHX_KEY"))
}

fn authx_secret() -> Option<String> {
    read_material(
        "ANIBOX_FN_AUTHX_SECRET",
        option_env!("ANIBOX_FN_AUTHX_SECRET"),
    )
}

pub fn md5hex(s: &str) -> String {
    format!("{:x}", md5::compute(s.as_bytes()))
}

/// 6 位随机数字（对齐 request.ts generateRandomDigits(100000, 1000000)）
pub fn rand6() -> String {
    use rand::Rng;
    rand::rng().random_range(100000..1000000).to_string()
}

/// 生成 Authx 头。body 为实际发送的 JSON 字符串；GET 无 body 传 None（md5("")）。
///
/// 未注入签名材料时返回 `None`（调用方据此不加 `Authx` 头）。
pub fn authx(url_path: &str, body_json: Option<&str>) -> Option<String> {
    sign_with(
        authx_key().as_deref(),
        authx_secret().as_deref(),
        url_path,
        body_json,
    )
}

/// 纯函数版签名（便于测试，不读环境）：缺任一材料 → `None`。
fn sign_with(
    key: Option<&str>,
    secret: Option<&str>,
    url_path: &str,
    body_json: Option<&str>,
) -> Option<String> {
    let (key, secret) = (key?, secret?);
    if key.is_empty() || secret.is_empty() {
        return None;
    }
    let nonce = rand6();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let data_md5 = md5hex(body_json.unwrap_or(""));
    let sign = md5hex(&format!(
        "{}_{}_{}_{}_{}_{}",
        key, url_path, nonce, timestamp, data_md5, secret
    ));
    Some(format!("nonce={}&timestamp={}&sign={}", nonce, timestamp, sign))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 未注入材料 → 返回 None（双向验证的「未注入」一侧）。
    #[test]
    fn no_material_returns_none() {
        assert!(sign_with(None, None, "/v/api/v1/x", None).is_none());
        assert!(sign_with(Some("k"), None, "/p", None).is_none());
        assert!(sign_with(None, Some("s"), "/p", None).is_none());
        assert!(sign_with(Some(""), Some("s"), "/p", None).is_none());
    }

    /// 注入假值 → 产出非空 Authx 头，且签名可独立复算校验（用假值，绝不用真实密钥）。
    #[test]
    fn fake_material_signs_and_verifies() {
        let key = "test-key";
        let secret = "test-secret";
        let path = "/v/api/v1/subtitle/search";
        let body = r#"{"nonce":"123456"}"#;

        let header = sign_with(Some(key), Some(secret), path, Some(body))
            .expect("注入假值时应产出 Authx 头");
        assert!(!header.is_empty());

        // 拆解并独立复算 sign
        let mut nonce = "";
        let mut ts = "";
        let mut sign = "";
        for kv in header.split('&') {
            let (k, v) = kv.split_once('=').unwrap();
            match k {
                "nonce" => nonce = v,
                "timestamp" => ts = v,
                "sign" => sign = v,
                _ => {}
            }
        }
        assert_eq!(nonce.len(), 6);
        assert!(!ts.is_empty());

        let expected = md5hex(&format!(
            "{}_{}_{}_{}_{}_{}",
            key,
            path,
            nonce,
            ts,
            md5hex(body),
            secret
        ));
        assert_eq!(sign, expected);
    }
}
