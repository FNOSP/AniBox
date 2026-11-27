//! 自建镜像准入签名 —— **bgm.rs 与 tmdb.rs 共用的唯一实现**（不许再各写一份）。
//!
//! 两个镜像（bgm.690075.xyz / tmdb.690075.xyz）用同一套签名协议、**各自独立的密钥**，
//! 协议逐字符对齐 worker.js verify()：
//!   x-fntv-app : fntv-plus
//!   x-fntv-ts  : 当前 Unix 秒（服务端 ±300s 时间窗）
//!   x-fntv-sign: base64( sha256( "fntv-plus" + ts + METHOD + pathWithQuery + sha256hex(body) + secret ) )
//! ⚠️ 内层 body 摘要是 **hex**、外层整体才是 **base64** —— 写反会得到 bad_signature（实测踩过）。
//! pathWithQuery 含前导斜杠与 query 原文，例：`/v0/subjects/8`、`/3/search/tv?query=x&language=zh-CN`。
//! 调用方必须保证「参与签名的字符串 == 实际发出的 URL 的 path+query」：tmdb.rs 是从
//! 解析后的 Url 里取 path/query 再签，避免编码差异（百分号转义等）造成签名漂移。
//!
//! 密钥来源（绝不写死进仓库）：运行时 env → 编译期 option_env → None（由调用方决定报错文案）。
//! 为什么 option_env 由调用方传入：编译期宏只认字面量，本模块拿不到变量名。

use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// 两个镜像共用的应用标识（worker verify 的第一道闸）
pub const APP_ID: &str = "fntv-plus";

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sha256_bytes(s: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().into()
}

fn hex32(b: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for x in b {
        out.push_str(&format!("{:02x}", x));
    }
    out
}

/// 签名（唯一实现）：
///   bodyDigest = **hex**(sha256(body))        ← 内层 hex
///   sign       = base64(sha256(appId+ts+method+pathWithQuery+bodyDigest+secret))  ← 外层 base64
pub fn sign(secret: &str, ts: u64, method: &str, path_with_query: &str, body: &str) -> String {
    let body_digest_hex = hex32(&sha256_bytes(body));
    let payload = format!("{APP_ID}{ts}{method}{path_with_query}{body_digest_hex}{secret}");
    base64::engine::general_purpose::STANDARD.encode(sha256_bytes(&payload))
}

/// 给请求挂上准入三件套（ts/签名现算，ts 与签名内用值必然一致）。
/// `path_with_query` 必须与最终请求 URL 的 path+query 完全一致（含顺序）。
pub fn apply_auth(
    rb: reqwest::RequestBuilder,
    secret: &str,
    method: &str,
    path_with_query: &str,
    body: &str,
) -> reqwest::RequestBuilder {
    let ts = now_secs();
    let sig = sign(secret, ts, method, path_with_query, body);
    rb.header("x-fntv-app", APP_ID)
        .header("x-fntv-ts", ts.to_string())
        .header("x-fntv-sign", sig)
}

/// 密钥解析：运行时 env `runtime_var` → 编译期 `compile_time` → None。
/// 空白视为未配置（`set /p` / 环境变量里常见尾随空白）。
pub fn resolve_secret(runtime_var: &str, compile_time: Option<&'static str>) -> Option<String> {
    if let Ok(s) = std::env::var(runtime_var) {
        let t = s.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    if let Some(s) = compile_time {
        let t = s.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    None
}

/// 端点列表：环境变量 `var`（逗号分隔）覆盖 → 默认值；去空白/去尾斜杠/丢空项。
pub fn resolve_endpoints(var: &str, default: &str) -> Vec<String> {
    let raw = std::env::var(var).unwrap_or_default();
    let raw = if raw.trim().is_empty() {
        default.to_string()
    } else {
        raw
    };
    raw.split(',')
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn truncate(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if t.len() < s.len() { format!("{t}…") } else { t }
}

/// [lc-2415] 端点熔断：某端点**连接失败/超时**后，接下来一段时间内直接跳过它。
///
/// 起因（实测日志）：默认端点列表里第二位 `fntv-bgm-mirror.122983191.workers.dev`
/// 在家用网络上 DNS 被污染（解析到 2a03:2880:f10d:183:face:b00c:0:25de，Facebook 段），
/// 每次请求都要白等满 12 秒超时才切到下一个端点。一次「打开吐槽」要跨 2 个端点，
/// 实测单次请求耗时 6 秒，其中一半以上纯粹浪费在一个**永远不可能成功**的地址上。
///
/// 只对**连接层失败**（DNS/连接/超时）计数，不对 HTTP 状态码计数：
/// 502/429 说明端点可达、只是上游有问题，换端点依然值得试，不能熔断。
/// 恢复方式：冷却期到点后自动重新尝试，成功即清除记录（无需重启）。
pub fn mark_endpoint_down(endpoint: &str) {
    down_until().lock().unwrap_or_else(|e| e.into_inner()).insert(endpoint.to_string(), now_secs() + ENDPOINT_COOLDOWN_SECS);
}

/// 端点是否处于熔断冷却期。
pub fn endpoint_is_down(endpoint: &str) -> bool {
    let mut m = down_until().lock().unwrap_or_else(|e| e.into_inner());
    match m.get(endpoint) {
        Some(&t) if t > now_secs() => true,
        Some(_) => {
            m.remove(endpoint); // 冷却结束，顺手清理
            false
        }
        None => false,
    }
}

/// 端点恢复正常 → 清掉熔断记录。
pub fn mark_endpoint_up(endpoint: &str) {
    down_until().lock().unwrap_or_else(|e| e.into_inner()).remove(endpoint);
}

fn down_until() -> &'static std::sync::Mutex<std::collections::HashMap<String, u64>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
        std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 熔断冷却时长：短到能自愈（上游/网络恢复后 1 分钟内重新试），
/// 长到能覆盖"一次打开详情页连续发好几个请求"的场景。
const ENDPOINT_COOLDOWN_SECS: u64 = 60;

/// [lc-2415] 可用端点（已剔除熔断中的）。全被熔断时**兜底返回原列表**：
/// 宁可白等一次，也不能因为熔断记录导致一个端点都不试（那就真的什么都发不出去了）。
pub fn healthy_endpoints(all: Vec<String>) -> Vec<String> {
    let alive: Vec<String> = all.iter().filter(|e| !endpoint_is_down(e)).cloned().collect();
    if alive.is_empty() {
        all
    } else {
        alive
    }
}

/// 401 家族的错误文案：把「本应用的密钥问题」与「镜像/上游问题」区分开。
/// `secret_env` 是调用方对应的环境变量名（bgm/tmdb 密钥独立，报错要指对人）。
/// 判断镜像返回的 401 是不是**签名侧**错误（密钥四兄弟）。
/// 用于把「镜像签名错」与「用户自带 Bearer token 被上游拒了」区分开 —— 后者
/// （如 Bangumi /v0/me 的假 token）不能套密钥文案，会误导排查方向。
pub fn is_mirror_auth_error(body_text: &str) -> bool {
    let err = serde_json::from_str::<serde_json::Value>(body_text)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
        .unwrap_or_default();
    matches!(
        err.as_str(),
        "bad_signature" | "stale_signature" | "bad_app" | "mirror_misconfigured"
    )
}

pub fn auth_error_hint(body_text: &str, secret_env: &str) -> String {
    let err = serde_json::from_str::<serde_json::Value>(body_text)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
        .unwrap_or_default();
    match err.as_str() {
        "bad_signature" => format!(
            "镜像拒绝签名（bad_signature）：检查 {secret_env} 是否为镜像 .secret.local 的完整值（勿加引号/换行）。"
        ),
        "stale_signature" => {
            "镜像时间窗校验失败（stale_signature）：检查系统时间，或升级 AniBox（密钥已轮换）。"
                .to_string()
        }
        "bad_app" => "镜像拒绝应用标识（bad_app）：x-fntv-app 必须是 fntv-plus。".to_string(),
        "mirror_misconfigured" => format!(
            "镜像未配置密钥（mirror_misconfigured）：服务端缺 {secret_env} 绑定。"
        ),
        _ => format!("镜像鉴权失败：{}", truncate(body_text, 200)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 交叉验证向量：由 Node（crypto）算出，且该 Node 实现已实测通过真实镜像 verify
    // （2026-10-13 对 tmdb.690075.xyz 签名请求返回 200；错误签名返回 401 bad_signature）。
    // 若谁改动了签名算法，这里必须同步核对——防止 hex/base64 内外层再次写反。
    #[test]
    fn sign_matches_node_reference_vectors() {
        assert_eq!(
            sign("test-secret", 1700000000, "GET", "/3/search/tv?query=x&language=zh-CN", ""),
            "JU0zyJ4IKLotzoGl4gx8GduBWyks1+ER+Q5OgnCxugA="
        );
        assert_eq!(
            sign("test-secret", 1700000000, "POST", "/v0/search/subjects", "{\"keyword\":\"x\"}"),
            "EWaDaUVoEtZYsV5qhE6rSV2oFQu3hHneRvR7/idZv6w="
        );
        assert_eq!(
            sign("another", 1791866052, "GET", "/img/t/p/w500/a.jpg", ""),
            "exjT335uPOrrzb3wumi3KPU65vFBnaS6C1LYDH3vAgM="
        );
    }

    #[test]
    fn resolve_endpoints_trims_and_filters() {
        // 不依赖真实环境变量（避免测试间串扰）：只验证默认值分支的解析行为
        let eps = resolve_endpoints("FNTV_TEST_ENDPOINT_UNSET_XYZ", "https://a.example/, https://b.example/");
        assert_eq!(eps, vec!["https://a.example", "https://b.example"]);
    }
}