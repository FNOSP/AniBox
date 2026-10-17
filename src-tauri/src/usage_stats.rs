//! [lc-1663] 匿名使用统计（Rust 侧，移植自旧项目 `src/main/handlers/plugins/usageStats.ts`）。
//!
//! 目标：知道「有多少台设备在用」，同时尽可能少地知道「是谁」。
//!
//! 隐私口径（与旧项目一致）：
//!   1. 匿名 ID：优先取操作系统机器标识（Windows `MachineGuid` / Linux `/etc/machine-id`）
//!      做 SHA-256 后取前 31 位 hex —— 不可逆、不含明文硬件信息；拿不到时回退 32 位随机 hex。
//!   2. 上报字段只有：匿名 ID、版本号、系统类型、CPU 架构、日期（+ 补报日期数组）。
//!      **不含账号 / IP / 媒体库 / 文件路径 / 播放内容**；服务端不存 IP。
//!   3. 每天最多一次；启动约 20s 后首报，之后每 6 小时检查一次（跨天补报）。
//!   4. 用户关闭 `usage_stats_enabled` 后，下一次检查立即停止上报（不再发任何请求）。
//!   5. 上报失败静默：只记日志、攒欠报日期，绝不影响启动与使用。
//!
//! 端点沿用旧项目（可用 `FNTV_STATS_ENDPOINT` 覆盖，逗号分隔多地址、主地址失败自动降级）。
//! 不硬编码任何密钥。

use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Manager, State};

use crate::{config, libmpv, AppState};

/// 旧项目端点（自有域名优先，workers.dev 兜底）。可用 FNTV_STATS_ENDPOINT 覆盖。
pub const DEFAULT_ENDPOINTS: &str =
    "https://stats.690075.xyz,https://fntv-stats.122983191.workers.dev";
const PING_PATH: &str = "/ping";
const TIMEOUT_SECS: u64 = 6;
/// 启动后延迟首报，避开启动高峰
const BOOT_DELAY_SECS: u64 = 20;
/// 之后每 6 小时检查一次（跨天补报）
const CHECK_INTERVAL_SECS: u64 = 6 * 60 * 60;
/// 欠报日期最多保留 7 天
const MAX_PENDING_DAYS: usize = 7;
/// 单次上报最多携带 8 个日期（今天 + 至多 7 天补报）
const MAX_DAYS_PER_PING: usize = 8;

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("AniBox/0.1")
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .build()
            .expect("usage stats http client")
    })
}

// ───────────────────────── 端点 ─────────────────────────

/// 解析端点列表（去空白、去尾部斜杠、过滤空项）。纯函数，便于单测。
pub fn parse_endpoints(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 实际使用的端点：`FNTV_STATS_ENDPOINT`（非空）优先，否则用旧项目默认值。
pub fn endpoints() -> Vec<String> {
    let raw = std::env::var("FNTV_STATS_ENDPOINT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ENDPOINTS.to_string());
    parse_endpoints(&raw)
}

// ───────────────────────── 匿名 ID ─────────────────────────

/// 机器标识 → 匿名 ID：SHA-256 前 31 位 hex（不可逆）。空输入返回空串。
pub fn derive_anon_id(raw_machine: &str) -> String {
    let raw = raw_machine.trim();
    if raw.is_empty() {
        return String::new();
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(raw.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex.chars().take(31).collect()
}

/// 是否是「机器派生」形态（31 位 hex）—— 命中则视为稳定身份，不再重派生。
pub fn is_machine_id(s: &str) -> bool {
    s.len() == 31 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 是否可作为兜底身份（8..=64 位 hex 或横杠；兼容旧 UUID 形态）。
pub fn is_acceptable_anon_id(s: &str) -> bool {
    (8..=64).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// 随机兜底 ID（32 位 hex；服务端 RE_AID 接受 hex/横杠）。
pub fn random_anon_id() -> String {
    format!(
        "{:016x}{:016x}",
        rand::random::<u64>(),
        rand::random::<u64>()
    )
}

#[cfg(target_os = "windows")]
fn machine_raw_id() -> String {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };
    let sub: Vec<u16> = "SOFTWARE\\Microsoft\\Cryptography\0".encode_utf16().collect();
    let val: Vec<u16> = "MachineGuid\0".encode_utf16().collect();
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * std::mem::size_of::<u16>()) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            &mut size,
        )
    };
    if rc != 0 {
        return String::new();
    }
    let chars = (size as usize / std::mem::size_of::<u16>()).saturating_sub(1);
    String::from_utf16_lossy(&buf[..chars.min(buf.len())])
        .trim()
        .to_string()
}

#[cfg(not(target_os = "windows"))]
fn machine_raw_id() -> String {
    for p in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(v) = std::fs::read_to_string(p) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    String::new()
}

/// 取匿名 ID：机器派生优先；拿不到机器标识时沿用旧值或随机生成。缺失时落盘。
async fn ensure_anon_id(app: &AppHandle, cur: &str) -> String {
    if is_machine_id(cur) {
        return cur.to_string();
    }
    let derived = derive_anon_id(&machine_raw_id());
    if !derived.is_empty() {
        if derived != cur {
            mutate_config(app, |c| c.usage_stats_anon_id = derived.clone()).await;
        }
        return derived;
    }
    if is_acceptable_anon_id(cur) {
        return cur.to_string();
    }
    let fresh = random_anon_id();
    mutate_config(app, |c| c.usage_stats_anon_id = fresh.clone()).await;
    fresh
}

// ───────────────────────── 日期 / 平台 ─────────────────────────

#[cfg(target_os = "windows")]
fn today_local() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut st: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay)
}

#[cfg(not(target_os = "windows"))]
fn today_local() -> String {
    // 民用历算法（Howard Hinnant days_from_civil 的逆变换），UTC
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if mo <= 2 { 1 } else { 0 };
    format!("{y:04}-{mo:02}-{d:02}")
}

fn os_label() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

fn arch_label() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

// ───────────────────────── 调度 / 上报 ─────────────────────────

/// 本次要上报的日期：今天 + 欠报（去重、排除今天、最多 8 个）。纯函数。
pub fn days_to_report(today: &str, pending: &[String]) -> Vec<String> {
    let mut out = vec![today.to_string()];
    for d in pending {
        if d != today && !out.iter().any(|x| x == d) {
            out.push(d.clone());
        }
    }
    out.truncate(MAX_DAYS_PER_PING);
    out
}

/// 上报失败后保留的欠报日期（最多 7 天）。纯函数。
pub fn pending_after_failure(days: &[String]) -> Vec<String> {
    days.iter().take(MAX_PENDING_DAYS).cloned().collect()
}

/// 上报体（唯一会被送出本机的内容）。纯函数，便于单测隐私字段集合。
pub fn build_payload(aid: &str, today: &str, days: &[String]) -> Value {
    json!({
        "aid": aid,
        "v": env!("CARGO_PKG_VERSION"),
        "os": os_label(),
        "arch": arch_label(),
        "d": today,
        "days": days,
    })
}

async fn mutate_config(app: &AppHandle, f: impl FnOnce(&mut config::Config)) -> config::Config {
    let state = app.state::<AppState>();
    let mut cfg = state.config.read().await.clone();
    f(&mut cfg);
    if let Err(e) = config::save(app, &cfg).await {
        libmpv::trace(&format!("[stats] 保存统计状态失败（忽略）: {e}"));
    }
    *state.config.write().await = cfg.clone();
    cfg
}

/// 发一次「今日在线」心跳。`force=true` 忽略「今日已报」限速与开发模式限制（面板手动测试用）。
/// 返回是否真正上报成功；关闭 / 开发模式 / 无端点 / 今日已报均返回 Ok(false)（静默跳过）。
pub async fn ping(app: &AppHandle, force: bool) -> Result<bool, String> {
    let cfg = app.state::<AppState>().config.read().await.clone();
    if !cfg.usage_stats_enabled {
        return Ok(false);
    }
    let forced = std::env::var("FNTV_STATS_FORCE").ok().as_deref() == Some("1");
    if !force && cfg!(debug_assertions) && !forced {
        libmpv::trace("[stats] 开发模式默认不上报（FNTV_STATS_FORCE=1 可强制）");
        return Ok(false);
    }
    let eps = endpoints();
    if eps.is_empty() {
        return Ok(false);
    }

    let aid = ensure_anon_id(app, &cfg.usage_stats_anon_id).await;
    let today = today_local();
    if !force && cfg.usage_stats_last_ping_day == today {
        return Ok(false);
    }
    let days = days_to_report(&today, &cfg.usage_stats_pending_days);

    // ⚠ 这里是全部会被送出本机的内容；新增字段前请先确认隐私口径。
    let payload = build_payload(&aid, &today, &days);

    let mut last_err = String::new();
    for base in &eps {
        let url = format!("{base}{PING_PATH}");
        match http()
            .post(&url)
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let reported = days.len();
                mutate_config(app, |c| {
                    c.usage_stats_last_ping_day = today.clone();
                    c.usage_stats_last_ping_ok = true;
                    c.usage_stats_pending_days.clear();
                })
                .await;
                libmpv::trace(&format!(
                    "[stats] 匿名心跳上报成功（{base}，含补报 {} 天）",
                    reported.saturating_sub(1)
                ));
                return Ok(true);
            }
            Ok(resp) => last_err = format!("HTTP {}", resp.status()),
            Err(e) => last_err = e.to_string(),
        }
    }

    let pending = pending_after_failure(&days);
    mutate_config(app, |c| {
        c.usage_stats_last_ping_day = today.clone();
        c.usage_stats_last_ping_ok = false;
        c.usage_stats_pending_days = pending.clone();
    })
    .await;
    libmpv::trace(&format!(
        "[stats] 匿名心跳上报失败（静默忽略，已记 {} 天待补报）: {last_err}",
        days.len()
    ));
    Ok(false)
}

/// 启动统计调度：20s 后首报，之后每 6 小时检查一次。失败静默，不阻塞启动。
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(BOOT_DELAY_SECS)).await;
        loop {
            if let Err(e) = ping(&app, false).await {
                libmpv::trace(&format!("[stats] 心跳异常（忽略）: {e}"));
            }
            tokio::time::sleep(Duration::from_secs(CHECK_INTERVAL_SECS)).await;
        }
    });
}

// ───────────────────────── 命令 ─────────────────────────

/// 读匿名统计开关。
#[tauri::command]
pub async fn usage_stats_get(state: State<'_, AppState>) -> Result<Value, String> {
    Ok(json!({ "enabled": state.config.read().await.usage_stats_enabled }))
}

/// 写匿名统计开关（持久化到 config.json；关闭后下一次检查即停止上报）。
#[tauri::command]
pub async fn usage_stats_set(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<Value, String> {
    let mut cfg = state.config.read().await.clone();
    cfg.usage_stats_enabled = enabled;
    config::save(&app, &cfg).await?;
    *state.config.write().await = cfg;
    libmpv::trace(&format!("[stats] 匿名使用统计开关: {enabled}"));
    Ok(json!({ "enabled": enabled }))
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_parse_and_fallback() {
        assert_eq!(
            parse_endpoints(" https://a.example , https://b.example/ ,, "),
            vec!["https://a.example", "https://b.example"]
        );
        assert!(parse_endpoints("   ").is_empty());
        // 默认端点包含旧项目两个地址
        let d = parse_endpoints(DEFAULT_ENDPOINTS);
        assert_eq!(d.len(), 2);
        assert!(d[0].contains("stats.690075.xyz"));
        assert!(d[1].contains("fntv-stats.122983191.workers.dev"));
    }

    #[test]
    fn anon_id_is_stable_31_hex() {
        let a = derive_anon_id("machine-guid-abc");
        let b = derive_anon_id("machine-guid-abc");
        assert_eq!(a, b, "同一机器标识必须派生同一 ID");
        assert_eq!(a.len(), 31);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, derive_anon_id("machine-guid-xyz"));
        assert_eq!(derive_anon_id("  "), "");
        assert!(is_machine_id(&a));
        assert!(!is_machine_id("abc"));
    }

    #[test]
    fn fallback_anon_id_shape() {
        let r = random_anon_id();
        assert_eq!(r.len(), 32);
        assert!(is_acceptable_anon_id(&r));
        // 旧 UUID 形态也接受
        assert!(is_acceptable_anon_id("123e4567-e89b-12d3-a456-426614174000"));
        assert!(!is_acceptable_anon_id(""));
        assert!(!is_acceptable_anon_id("not-an-id!!"));
    }

    #[test]
    fn days_to_report_dedup_and_cap() {
        let today = "2026-10-17";
        let pending: Vec<String> = (1..=10)
            .map(|i| format!("2026-10-{:02}", i))
            .chain(std::iter::once(today.to_string()))
            .collect();
        let days = days_to_report(today, &pending);
        assert_eq!(days[0], today);
        assert!(days.len() <= MAX_DAYS_PER_PING);
        // 今天不重复
        assert_eq!(days.iter().filter(|d| *d == today).count(), 1);
        // 无重复项
        let mut sorted = days.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), days.len());
    }

    #[test]
    fn pending_after_failure_caps_at_seven() {
        let days: Vec<String> = (1..=10).map(|i| format!("2026-10-{i:02}")).collect();
        let p = pending_after_failure(&days);
        assert_eq!(p.len(), 7);
        assert_eq!(p[0], days[0]);
    }

    #[test]
    fn payload_contains_only_privacy_safe_fields() {
        let days = vec!["2026-10-17".to_string()];
        let p = build_payload("abc123", "2026-10-17", &days);
        let obj = p.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
        keys.sort();
        assert_eq!(keys, vec!["aid", "arch", "d", "days", "os", "v"]);
        // 明确断言不含账号 / 路径 / 内容类字段
        for forbidden in ["token", "user", "username", "path", "server", "message", "log"] {
            assert!(obj.get(forbidden).is_none(), "不应上报 {forbidden}");
        }
    }

    #[test]
    fn today_local_shape() {
        let t = today_local();
        assert_eq!(t.len(), 10, "YYYY-MM-DD: {t}");
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[7..8], "-");
        assert!(t[..4].bytes().all(|b| b.is_ascii_digit()));
    }
}
