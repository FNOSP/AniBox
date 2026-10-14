//! TMDB —— **默认全线走自建镜像站**（tmdb.690075.xyz/3，带准入签名；镜像 worker：
//! Fntv-Plus/tmdb-mirror/worker.js）。镜像服务端注入自己的 TMDB v4 key，并**剥离客户端传来的
//! `api_key` / `Authorization`** —— 所以走镜像的请求**不带任何 key**，用户不再必须自己填 key。
//!
//! ⚠ **语言参数一律由客户端显式带上**（镜像已**不再**默认注入 `language=zh-CN`）：
//!   · 每个请求都要 `language=zh-CN`（不传 = 英文标题/简介）；
//!   · 要图片（`append_to_response=images`）必须 `include_image_language=zh-CN,en,null`
//!     —— 实测 `language=zh-CN` 会把 `images` 过滤成 **0 条**，靠 en/null 兜住有图可看；
//!   · 要视频（`append_to_response=videos`）必须 `include_video_language=zh-CN,en,ja,null`
//!     —— 预告片基本只有 en/ja（实测 `/3/tv/1399/videos?language=zh-CN` = 0 条、`en-US` = 7 条）。
//!
//! 降级（保留「镜像被限流/不可用时用用户自己 key」的能力）—— 每次降级都打诊断日志：
//!   · 镜像 **429 上游限流**（body 结构化 `{"error":"upstream_rate_limited"}`；镜像统计把它记成
//!     **RATE_LIMIT**）→ 降级直连用户 key（自己的配额可能没被限）
//!   · 所有镜像端点**网络不可达/超时**、或镜像 5xx
//!   · **缺 FNTV_TMDB_SECRET**（配置错误：没有密钥就无法签名 → 按「镜像不可用」处理，
//!     打 WARN 后直接降级。绝不能因缺密钥就让整个 TMDB 功能死掉——那比改造前还糟，
//!     改造前用户自己的 key 是能用的）
//!   → 用 config.json 的 `tmdbApiKey`（或 env FNTV_TMDB_KEY）**直连 api.themoviedb.org 重试**。
//!     直连路径克制重试：仅瞬时类错误（网络/429/5xx）最多 2 次尝试、单次 8s 超时。
//!   · 401（bad_signature，**密钥存在但错误**）与其余 4xx 是**确定性拒绝**，不降级——
//!     这类是真实业务/签名拒绝，降级改变不了结果，只会掩盖问题。
//!
//! ⚠ **两种 429 绝不能混**（本次镜像改造新加的语义）：
//!   · `{"error":"throttled"}` = **我们镜像自己的每 IP 短时上限** → **退避重试**（可带 Retry-After），
//!     不能当成上游限流立刻降级直连（那会让客户端多打自己的配额、也绕开了镜像的缓存）
//!   · `{"error":"upstream_rate_limited"}` = **上游 TMDB 429**（镜像统计里的 RATE_LIMIT）
//!     → 立刻走原有降级直连
//!   退避重试耗尽后仍会降级（有 key 则直连、无 key 则报错），只是**不再"一撞就降"**。
//!
//! 端点：FNTV_TMDB_ENDPOINT（逗号分隔）覆盖默认「主址+workers.dev 兜底址」；连接错误/超时/5xx
//! 自动切下一个端点（与 bgm.rs 同一套写法）。每请求 10s 超时。
//! 正文字形与主仓库 src/main/handlers/plugins/tmdbSync.ts 的 fetchShowDetails + normalizeShow
//! 对齐（一次搜索 + 一次详情 append_to_response 合并）；key 复用主仓库 config.json 的 tmdbApiKey。
//! 演员表（`tmdb_credits`）：**TMDB 主源** —— fnOS 条目的 `trim_id`（tm63578 = TMDB id /
//! tt325158 = IMDb id / bg456080 = Bangumi）能精确定位就直接定位，定位不到才回落标题搜索
//! （搜索**带最低相似度门槛**，不再是"取第一条"），命中方式写进 `matched_by` 与诊断日志，
//! 结果按 tmdb id 落盘缓存（disk_cache；[lc-1417] TTL 由 7 天提到 **120 天**，见 CREDITS_TTL_SECS），
//! 飞牛/Bangumi 侧只在前端做兜底展示。
//! 注：图片由前端经 fntvimg 自定义协议取（`/__tmdb/<t/p/size/...>`，三级缓存见 imgproto.rs）。

use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

use crate::disk_cache;
use crate::mirror_sign;

/// 上游官方 API —— **仅降级直连路径使用**；正常流量一律走镜像（默认端点里不含这个域名）。
const DIRECT_API: &str = "https://api.themoviedb.org/3";
/// 镜像主址 + 兜底址（同一 worker 的 workers.dev 入口；顺序即回退顺序）
const DEFAULT_ENDPOINTS: &str =
    "https://tmdb.690075.xyz,https://fntv-tmdb-mirror.122983191.workers.dev";
const MIRROR_TIMEOUT_SECS: u64 = 10;
/// 降级直连：克制的超时/重试（不把整个请求拖死）
const DIRECT_TIMEOUT_SECS: u64 = 8;
const DIRECT_MAX_ATTEMPTS: usize = 2;
const DIRECT_RETRY_BACKOFF_MS: u64 = 500;

/// 语言参数（**镜像不再注入**，客户端每个请求都显式带）
const LANG: &str = "zh-CN";
/// 图片语言：zh-CN + en + null（zh-CN 单独用会把 images 过滤成 0 条）
const INCLUDE_IMAGE_LANG: &str = "zh-CN,en,null";
/// 视频（预告片）语言：en/ja 是绝大多数预告片的语言，null 收住无语言视频
const INCLUDE_VIDEO_LANG: &str = "zh-CN,en,ja,null";

/// 镜像「每 IP 短时上限」的退避重试：最多 4 次尝试、指数退避 600ms 起
const THROTTLE_MAX_ATTEMPTS: usize = 4;
const THROTTLE_BASE_BACKOFF_MS: u64 = 600;
const THROTTLE_MAX_BACKOFF_MS: u64 = 8_000;

/// [lc-1417] 详情页 TMDB 数据的落盘 TTL：**120 天**（用户指定："数据拉了直接本地落盘
/// 不要老是拉取""落盘时间 120 天，用户也可自行清除"）。
/// 覆盖演员表（CREDITS）与条目详情（SHOW，剧照/相似/别名那批）。
/// 想立刻更新 → 设置面板「清理缓存」；那条命令会一并清掉这些。
const CREDITS_TTL_SECS: u64 = 120 * 24 * 3600;
/// 条目详情（tmdb_show_details）的 TTL，与上面同值、单独命名以便日后分别调整
const SHOW_TTL_SECS: u64 = 120 * 24 * 3600;

/// [lc-123-rev5] 发现页落盘缓存 TTL（用户反馈"刷得太频繁，想要"检测新增才刷新"）：
///   · 当季 12 小时 / 即将播出 24 小时 —— 这类榜单一天最多变一两次，镜像侧还有自己的边缘缓存；
///   · 到期也**不无脑重拉**：带 ETag 条件请求，上游 304 就只顺延时间戳（见 disk_cache::json_cached_conditional）；
///   · 手动刷新 force 只跳过本地 TTL 检查，条件头照带（"检测新增"才叫刷新）。
const DISCOVER_TTL_SEASON_SECS: u64 = 12 * 3600;
const DISCOVER_TTL_UPCOMING_SECS: u64 = 24 * 3600;


/// 用户自己的 key（可选）：env FNTV_TMDB_KEY → 主仓库 fn_config/config.json 的 tmdbApiKey。
/// 只在降级直连时用；镜像路径不需要 key。
fn api_key() -> Option<String> {
    static KEY: OnceLock<Option<String>> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Ok(k) = std::env::var("FNTV_TMDB_KEY") {
            if !k.trim().is_empty() {
                return Some(k.trim().to_string());
            }
        }
        // 主仓库 fn_config/config.json（Tauri2 PoC 复用主仓库配置，登录信息同源）
        let p = std::path::Path::new(&std::env::var("APPDATA").ok()?)
            .join("fntv/config.json");
        let s = std::fs::read_to_string(p).ok()?;
        let v: Value = serde_json::from_str(&s).ok()?;
        v.get("tmdbApiKey")
            .and_then(|k| k.as_str())
            .filter(|k| !k.trim().is_empty())
            .map(|k| k.trim().to_string())
    })
    .clone()
}

/// 镜像 client：10s 超时（镜像自带边缘缓存，正常响应很快）。
fn mirror_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("AniBox/0.1 (tmdb-mirror)")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(MIRROR_TIMEOUT_SECS))
            .build()
            .expect("tmdb mirror http client")
    })
}

/// 降级直连 client：更短的 8s 超时（弱网下宁可报错也不拖着请求）。
fn direct_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) anibox")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(DIRECT_TIMEOUT_SECS))
            .build()
            .expect("tmdb direct http client")
    })
}

/// 镜像取数的失败分类：
///   Degradable —— 不可达/镜像故障/上游限流（可降级直连）
///   Fatal      —— 确定性拒绝（签名/路由/配置错误；降级只会掩盖问题）
///   Throttled  —— **镜像自己的每 IP 短时上限**（429 `{"error":"throttled"}`）：
///                 先退避重试（见 get_json），**不**按"上游限流"直接降级。
enum FetchError {
    Degradable(String),
    Fatal(String),
    Throttled(String, u64),
}

/// 镜像 429 的两种语义（**绝不能混**，混了就会"被自己镜像限流 → 立刻改用用户 key 直连上游"）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Throttle429 {
    /// 我们镜像的每 IP 短时上限 → 退避重试
    MirrorThrottled,
    /// 上游 TMDB 429（镜像统计记 RATE_LIMIT）→ 立刻降级直连用户 key
    UpstreamRateLimited,
    /// 结构不认识：按"上游限流"的老行为处理（保守：不无限重试）
    Unknown,
}

/// 429 正文 → 语义分类。只认镜像 worker 的两个结构化 error。
pub(crate) fn classify_429(body: &str) -> Throttle429 {
    let err = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();
    match err.as_str() {
        "throttled" => Throttle429::MirrorThrottled,
        "upstream_rate_limited" => Throttle429::UpstreamRateLimited,
        _ => Throttle429::Unknown,
    }
}

/// Retry-After（秒）→ 退避毫秒；非法/过大 → None（走指数退避）
fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let s = headers.get("retry-after")?.to_str().ok()?.trim().to_string();
    let secs: u64 = s.parse().ok()?;
    Some((secs * 1000).min(THROTTLE_MAX_BACKOFF_MS))
}

fn year_of(s: &str) -> String {
    s.chars().take(4).collect()
}

/* ────────────── 日期算术（只为算「当季/下季」的日期窗口） ──────────────
   为什么不引 chrono/time：这里只需要「今天的日期」与「某季度的首末日期」，
   两个纯函数（Howard Hinnant 的 civil_from_days / days_from_civil 常数算法）就够，
   而且能**离线单测**（见本文件 tests）。TMDB 的 first_air_date 是**日期粒度**，
   用 UTC 日期（本机时区最多偏 8h，即中国凌晨 0-8 点会算作"昨天"）不会影响正确性：
   该窗口只决定"取哪些条目"，边界差一天在结果上只是多/少一条相邻条目。 */

/// 天数（自 1970-01-01，UTC）→ (年, 月, 日)
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (年, 月, 日) → 天数（自 1970-01-01，UTC）
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 今天（UTC）的 YYYY-MM-DD —— discover 的日期窗口与"是否已开播"判据都用它
fn today_ymd() -> String {
    let (y, m, d) = civil_from_days((mirror_sign::now_secs() / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 某季度的首末日期（`next=false` 当季、`true` 下季；季度按 1-3/4-6/7-9/10-12 划分）。
/// 末日期取该季度最后一天的**真实天数**（2 月 28/29、大小月 30/31 都由日历算，不写死）。
fn quarter_bounds(y: i64, m: u32, next: bool) -> (String, String) {
    let mut start_m = (m - 1) / 3 * 3 + 1; // 1 → 4 → 7 → 10
    let mut year = y;
    if next {
        start_m += 3;
        if start_m > 12 {
            start_m -= 12;
            year += 1;
        }
    }
    let end_m = start_m + 2;
    let (ny, nm) = if end_m == 12 { (year + 1, 1) } else { (year, end_m + 1) };
    let last_day = (days_from_civil(ny, nm, 1) - days_from_civil(year, end_m, 1)) as u32;
    (
        format!("{year:04}-{start_m:02}-01"),
        format!("{year:04}-{end_m:02}-{last_day:02}"),
    )
}

/// 主仓库 authFor：v4 JWT → Bearer 头；v3 → query 参数（**仅降级直连路径使用**）
fn auth_params(key: &str) -> (Vec<(String, String)>, Vec<(String, String)>) {
    // (query 参数, header)。v4 JWT 走 Bearer 头；v3 走 api_key query
    if key.starts_with("eyJ") {
        (vec![], vec![("Authorization".into(), format!("Bearer {}", key))])
    } else {
        (vec![("api_key".into(), key.into())], vec![])
    }
}

/// 镜像 GET 的两种结局（[lc-123-rev5] 条件请求）：
///   NotModified = 带 `If-None-Match` 时镜像回 **304**（没变化，连 body 都没有）；
///   Fresh = 200 的正文 + 本次响应的 `ETag`（有就存进 disk_cache 的 meta，供下次探测）。
pub(crate) enum MirrorGet {
    NotModified,
    Fresh(Value, Option<String>),
}

/// 镜像 GET：多端点回退 + 准入签名 + 10s 超时。`path_with_query` 以 `/3` 开头。
/// **429 分两种**（见 classify_429）：
///   · `throttled`（镜像自己的每 IP 上限）→ Throttled（调用方退避重试；不换端点：同一 worker 状态一致）
///   · `upstream_rate_limited`（上游 TMDB 429，镜像记 RATE_LIMIT）→ Degradable（原有的降级直连）
/// 5xx/连接错误 → 试下一个端点，全部失败后 Degradable；4xx（含 401）→ Fatal。
/// [lc-123-rev5] `if_none_match` 有值时透传 `If-None-Match`，镜像可能回 304 且**无 body**。
async fn mirror_get_full(
    path_with_query: &str,
    if_none_match: Option<&str>,
) -> Result<MirrorGet, FetchError> {
    // 缺密钥 = **配置错误 → 可降级**（按「镜像不可用」处理），不再直接失败：没有密钥就签不了名，
    // 连端点都不用试（省掉一次必然 401 的往返）。边界：只有「缺密钥」走这里；密钥**存在但错误**
    // 会拿到 401 bad_signature，那是确定性拒绝，仍按 Fatal 原样报错。
    let Some(secret) = mirror_sign::resolve_secret("FNTV_TMDB_SECRET", option_env!("FNTV_TMDB_SECRET"))
    else {
        crate::libmpv::trace(
            "[tmdb] WARN 未配置 FNTV_TMDB_SECRET（镜像密钥缺失）→ 按镜像不可用处理，降级直连用户 key",
        );
        return Err(FetchError::Degradable(
            "未配置 TMDB 镜像密钥（FNTV_TMDB_SECRET）→ 按镜像不可用降级。修复：用 dev.cmd 启动，\
             或设置该环境变量（值 = Fntv-Plus/tmdb-mirror/.secret.local 中 `FNTV_TMDB_SECRET=` 之后的部分）"
                .to_string(),
        ));
    };
    let mut last_err = String::new();
    for ep in mirror_sign::resolve_endpoints("FNTV_TMDB_ENDPOINT", DEFAULT_ENDPOINTS) {
        let url_str = format!("{ep}{path_with_query}");
        let url = match reqwest::Url::parse(&url_str) {
            Ok(u) => u,
            Err(e) => {
                last_err = format!("{ep} URL 非法: {e}");
                continue;
            }
        };
        // ⚠ 参与签名的 path+query 从**解析后的 URL** 取：与实际发出的请求逐字节一致
        // （避免 url crate 的百分号规范化让"签名的串"与"发出的串"漂移）。
        let signed_pwq = match url.query() {
            Some(q) => format!("{}?{}", url.path(), q),
            None => url.path().to_string(),
        };
        let mut rb = mirror_sign::apply_auth(mirror_client().get(url), &secret, "GET", &signed_pwq, "")
            .header("accept", "application/json");
        // [lc-123-rev5] 条件请求：带上次存下的 ETag，没变就拿 304（几字节，省流量也省上游配额）
        if let Some(et) = if_none_match.filter(|e| !e.is_empty()) {
            rb = rb.header("if-none-match", et);
        }
        let resp = match rb.send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("{ep} 连接失败/超时: {e}");
                crate::libmpv::trace(&format!("[tmdb] GET {path_with_query} → {ep} 连接失败: {e}"));
                continue; // 连接错误/超时 → 切下一个端点
            }
        };
        let status = resp.status().as_u16();
        let cache_hdr = resp
            .headers()
            .get("x-fntv-cache")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string();
        if status == 304 {
            // 条件请求命中：上游说"没变"——连 body 都没有，直接把 NotModified 交给缓存层
            crate::libmpv::trace(&format!(
                "[tmdb] GET {path_with_query} → {ep} 304 Not Modified cache={cache_hdr}"
            ));
            return Ok(MirrorGet::NotModified);
        }
        if (200..300).contains(&status) {
            // ETag 必须在 body 被取走前读（json() 会消费 resp）
            let etag = resp
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            let v: Value = resp.json().await.map_err(|e| {
                FetchError::Degradable(format!("{ep} 响应不是合法 JSON: {e}"))
            })?;
            crate::libmpv::trace(&format!(
                "[tmdb] GET {path_with_query} → {ep} {status} cache={cache_hdr} etag={}",
                etag.as_deref().unwrap_or("-")
            ));
            return Ok(MirrorGet::Fresh(v, etag));
        }
        // Retry-After 必须在把 body 取走之前读（resp.text() 会 move resp）
        let retry_after = retry_after_ms(resp.headers());
        let text = resp.text().await.unwrap_or_default();
        crate::libmpv::trace(&format!(
            "[tmdb] GET {path_with_query} → {ep} {status} cache={cache_hdr} body={}",
            mirror_sign::truncate(&text, 160)
        ));
        if status == 429 {
            match classify_429(&text) {
                // ★ 镜像自己的每 IP 短时上限：退避重试（不是上游限流，不该立刻降级直连）
                Throttle429::MirrorThrottled => {
                    return Err(FetchError::Throttled(
                        format!("镜像短时限流（429 throttled）：{}", mirror_sign::truncate(&text, 160)),
                        retry_after.unwrap_or(THROTTLE_BASE_BACKOFF_MS),
                    ));
                }
                // 上游 TMDB 429（镜像统计 = RATE_LIMIT）：换 key 可能就不限了 → 交调用方降级直连
                Throttle429::UpstreamRateLimited | Throttle429::Unknown => {
                    return Err(FetchError::Degradable(format!(
                        "镜像上游限流（HTTP 429 RATE_LIMIT）：{}",
                        mirror_sign::truncate(&text, 200)
                    )));
                }
            }
        }
        if status >= 500 {
            last_err = format!("{ep} HTTP {status}: {}", mirror_sign::truncate(&text, 160));
            continue; // 5xx → 切下一个端点
        }
        // 4xx：确定性拒绝（签名/路由问题），换端点结果相同，直接报错且不降级
        return Err(FetchError::Fatal(if status == 401 {
            mirror_sign::auth_error_hint(&text, "FNTV_TMDB_SECRET")
        } else {
            format!("镜像请求失败（HTTP {status}）：{}", mirror_sign::truncate(&text, 200))
        }));
    }
    Err(FetchError::Degradable(format!(
        "所有镜像端点均不可达（最后：{last_err}）"
    )))
}

/// 降级直连上游（用户自己的 key）：v4 JWT → Bearer；v3 → api_key query（保留原区分逻辑）。
/// 克制重试：仅网络错误/超时/429/5xx 重试，最多 DIRECT_MAX_ATTEMPTS 次，间隔 500ms；其余 4xx 直接报错。
async fn direct_get(path_with_query: &str, key: &str) -> Result<Value, String> {
    // /3/search/tv?... → 直连域名已是 .../3，去掉镜像路径里的 /3 前缀
    let api_path = path_with_query.strip_prefix("/3").unwrap_or(path_with_query);
    let (query_auth, header_auth) = auth_params(key);
    let mut url = format!("{DIRECT_API}{api_path}");
    // v3 的 api_key 走 query：分隔符要看 url 有没有已经是 query（写死 '&' 时，
    // 无 query 的路径如 /3/configuration 会拼成 ".../configuration&api_key=..." → 400 非法 URL）。
    // 现有调用点都带 query，所以这条只在将来新增无 query 路径时才会显形——先修掉。
    let mut sep = if url.contains('?') { '&' } else { '?' };
    for (k, v) in &query_auth {
        url.push(sep);
        url.push_str(&format!("{}={}", k, urlencoding::encode(v)));
        sep = '&';
    }
    let mut last_err = String::new();
    for attempt in 1..=DIRECT_MAX_ATTEMPTS {
        if attempt > 1 {
            tokio::time::sleep(Duration::from_millis(DIRECT_RETRY_BACKOFF_MS)).await;
        }
        let mut rb = direct_client().get(&url).header("accept", "application/json");
        for (k, v) in &header_auth {
            rb = rb.header(k, v);
        }
        match rb.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if (200..300).contains(&status) {
                    crate::libmpv::trace(&format!(
                        "[tmdb] 降级直连成功 {api_path} {status} (attempt {attempt})"
                    ));
                    return resp
                        .json()
                        .await
                        .map_err(|e| format!("直连响应不是合法 JSON: {e}"));
                }
                let text = resp.text().await.unwrap_or_default();
                if status == 429 || status >= 500 {
                    last_err = format!("直连 HTTP {status}: {}", mirror_sign::truncate(&text, 160));
                    continue; // 瞬时类 → 再试一次（上限 2 次），不做无限重试
                }
                return Err(format!(
                    "直连失败（HTTP {status}）：{}",
                    mirror_sign::truncate(&text, 200)
                ));
            }
            Err(e) => {
                last_err = format!("直连网络错误/超时: {e}");
                continue;
            }
        }
    }
    Err(format!("直连 TMDB 失败（{DIRECT_MAX_ATTEMPTS} 次尝试后）：{last_err}"))
}

/// 取数主入口：先镜像；
///   · **镜像短时限流（429 throttled）→ 指数退避重试**（最多 THROTTLE_MAX_ATTEMPTS 次；
///     这是*我们自己的*镜像闸门，不是上游限流，直接降级反而多打自己的配额）；
///   · 其余 Degradable（上游 429 RATE_LIMIT / 不可达 / 镜像故障）且有用户 key → 降级直连并打日志；
///   · 没有 key → 返回合并错误（提示可配自己的 key 或稍后重试）。
/// 退避重试耗尽后仍走降级（有 key 直连、无 key 报错）——只是不再"一撞就降"。
/// [lc-123-rev5] `if_none_match` 有值时走**条件请求**（镜像可能回 304，交给调用方决定"不动内容"）。
/// 降级直连那条路径不带条件头（官方域是否支持 ETag 不确定）→ 结果按 Fresh(_, None) 处理。
async fn get_json_cond(
    path_with_query: &str,
    if_none_match: Option<&str>,
) -> Result<MirrorGet, String> {
    let mut throttle_cause = String::new();
    for attempt in 1..=THROTTLE_MAX_ATTEMPTS {
        match mirror_get_full(path_with_query, if_none_match).await {
            Ok(v) => return Ok(v),
            Err(FetchError::Fatal(e)) => return Err(e),
            Err(FetchError::Throttled(msg, wait)) => {
                // 第一次响应带 Retry-After 就用它；之后（或没带）按 600/1200/2400ms 指数退避
                let backoff = if attempt == 1 {
                    wait
                } else {
                    (THROTTLE_BASE_BACKOFF_MS << (attempt - 1)).min(THROTTLE_MAX_BACKOFF_MS)
                };
                throttle_cause = msg.clone();
                if attempt == THROTTLE_MAX_ATTEMPTS {
                    break; // 退避用尽 → 落到下方的降级判定
                }
                crate::libmpv::trace(&format!(
                    "[tmdb] 镜像短时限流（throttled），{backoff}ms 后重试（第 {attempt}/{THROTTLE_MAX_ATTEMPTS} 次）"
                ));
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                continue;
            }
            // 降级直连不带条件头（官方域支持与否不确定）→ 结果按 Fresh(_, None) 收下
            Err(FetchError::Degradable(cause)) => {
                return degrade_or_err(path_with_query, cause)
                    .await
                    .map(|v| MirrorGet::Fresh(v, None))
            }
        }
    }
    // 重试到上限仍被我们自己限流：这时才降级（有 key 直连，无 key 报错并说明退避次数）
    degrade_or_err(
        path_with_query,
        format!("{throttle_cause}（已退避重试 {THROTTLE_MAX_ATTEMPTS} 次）"),
    )
    .await
    .map(|v| MirrorGet::Fresh(v, None))
}

/// 无条件 GET（原有调用点用这个）：把 get_json_cond 的 Fresh 拆出来。
async fn get_json(path_with_query: &str) -> Result<Value, String> {
    match get_json_cond(path_with_query, None).await? {
        MirrorGet::Fresh(v, _) => Ok(v),
        MirrorGet::NotModified => Err("未带条件头却收到 304（不应发生）".to_string()),
    }
}

/// Degradable → 有用户 key 就直连重试；没有 key 就返回「已尝试降级」的合并错误。
async fn degrade_or_err(path_with_query: &str, cause: String) -> Result<Value, String> {
    let Some(key) = api_key() else {
        return Err(format!(
            "{cause}；未配置个人 TMDB API Key（config.json 的 tmdbApiKey 或环境变量 FNTV_TMDB_KEY），无法降级直连，请稍后重试"
        ));
    };
    crate::libmpv::trace(&format!("[tmdb] 镜像不可用，降级直连（用户 key）：{cause}"));
    direct_get(path_with_query, &key)
        .await
        .map_err(|e| format!("{cause}；降级直连也失败：{e}"))
}

fn names_of(v: Option<&Value>) -> Vec<String> {
    match v.and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|x| {
                x.get("name")
                    .and_then(|n| n.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
            })
            .collect(),
        None => vec![],
    }
}

/* ══════════ 请求路径构造（纯函数）—— **显式语言参数只在这里拼**，单测逐条钉住 ══════════
   为什么集中：镜像是可替换的（worker 有兜底址、还能整个换成官方域），语言参数一旦散落在各
   调用点，就会在"镜像不再注入 zh-CN"之后静默退回英文——集中 + 单测能保证新增端点不会再漏。 */

/// 搜索路径：先精确名+年（TV 是 first_air_date_year），再退纯名（与主仓库 tmdbSearchBest 同序）
fn search_path(mt: &str, title: &str, year: &str) -> String {
    let mut p = format!("/3/search/{mt}?query={}&language={LANG}", urlencoding::encode(title));
    if !year.is_empty() {
        let yk = if mt == "movie" { "year" } else { "first_air_date_year" };
        p.push_str(&format!("&{}={}", yk, urlencoding::encode(year)));
    }
    p
}

/// IMDb → TMDB 的 find 端点（`external_source=imdb_id`；结果按 movie_results/tv_results 分组）
fn find_path(imdb_id: &str) -> String {
    format!(
        "/3/find/{}?external_source=imdb_id&language={LANG}",
        urlencoding::encode(imdb_id)
    )
}

/// 详情：`language` + `include_image_language` + `include_video_language` **全部显式**
/// （镜像已不再注入 language；图片/视频语言靠 include_* 收住 en/null，否则 zh-CN 会把它们滤空）
fn detail_path(mt: &str, id: i64, append: &str) -> String {
    format!(
        "/3/{mt}/{id}?language={LANG}&append_to_response={append}\
         &include_image_language={INCLUDE_IMAGE_LANG}&include_video_language={INCLUDE_VIDEO_LANG}"
    )
}

/// 演员表路径：剧集要 `aggregate_credits`（roles[].episode_count 才有的"演了几集"），电影只需 credits
fn credits_path(mt: &str, id: i64) -> String {
    let append = if mt == "tv" { "aggregate_credits,credits" } else { "credits" };
    format!(
        "/3/{mt}/{id}?language={LANG}&append_to_response={append}&include_image_language={INCLUDE_IMAGE_LANG}"
    )
}

/// 发现页：`language` 同样显式；`vote_min` 只有"当季"能带（未开播条目 vote_count=0 会被滤空）
fn discover_path(
    media_type: &str,
    gte_key: &str,
    gte: &str,
    lte_key: &str,
    lte: &str,
    vote_min: bool,
    page: i64,
) -> String {
    let mut p = format!("/3/discover/{media_type}?sort_by=popularity.desc&{gte_key}={gte}&language={LANG}");
    if !lte.is_empty() {
        p.push_str(&format!("&{lte_key}={lte}"));
    }
    if vote_min {
        p.push_str("&vote_count.gte=5");
    }
    p.push_str(&format!("&page={page}"));
    p
}

/* ══════════ fnOS 条目锚点（trim_id）→ TMDB id ══════════
   飞牛条目上的 `trim_id` 形如 `tm63578`（TMDB id）/ `tt325158`（IMDb id）/ `bg456080`（Bangumi）。
   能精确锚定就直接锚（省一次搜索，也不会像标题搜索那样认错片子），锚不到才回落标题搜索。 */

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Anchor {
    Tmdb(i64),
    Imdb(String),
    /// Bangumi 源（飞牛对中国动画常用）—— **没有 TMDB id**，只能回落标题搜索
    Bangumi(i64),
    None,
}

/// trim_id 解析（大小写不敏感；也兼容纯数字——老口径把纯数字当 TMDB id）
pub(crate) fn parse_trim_id(raw: &str) -> Anchor {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Anchor::None;
    }
    let digits = |d: &str| -> Option<i64> {
        if !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()) && d.len() <= 12 {
            d.parse::<i64>().ok()
        } else {
            None
        }
    };
    if let Some(d) = s.strip_prefix("tm") {
        if let Some(n) = digits(d) {
            return Anchor::Tmdb(n);
        }
    }
    if let Some(d) = s.strip_prefix("bg") {
        if let Some(n) = digits(d) {
            return Anchor::Bangumi(n);
        }
    }
    if let Some(d) = s.strip_prefix("tt") {
        if digits(d).is_some() {
            return Anchor::Imdb(format!("tt{d}"));
        }
    }
    if let Some(n) = digits(&s) {
        return Anchor::Tmdb(n);
    }
    Anchor::None
}

/// 锚点解析结果
struct Resolved {
    id: i64,
    matched_by: String,
}

fn year_num(date: &str) -> Option<i32> {
    date.get(0..4).and_then(|y| y.parse::<i32>().ok())
}

/// TMDB 搜索结果选最佳 —— **补上"取第一条、无任何校验"缺的门槛**：
/// 标题相似度（多语言标题取最高）+ 年份加权；标题相似度 < 0.5 直接淘汰（宁可匹配失败，
/// 也不把「某部同名烂片」的演员表贴到用户条目上）。相似度/年份口径与 bgm.rs 共用。
fn pick_best_search(items: &[Value], query: &str, want_year: Option<i32>, mt: &str) -> Option<(i64, String)> {
    const MIN_TITLE_SIM: f64 = 0.50;
    const MIN_TOTAL: f64 = 0.50;
    let qn = crate::bgm::norm(query);
    if qn.is_empty() {
        return None;
    }
    let mut best: Option<(f64, i64, String)> = None;
    for it in items {
        let (zh_key, orig_key) = if mt == "movie" {
            ("title", "original_title")
        } else {
            ("name", "original_name")
        };
        let names = [it.get(zh_key), it.get(orig_key)]
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if names.is_empty() {
            continue;
        }
        let mut sim = 0.0f64;
        let mut which = zh_key;
        for cand in &names {
            let s = crate::bgm::title_sim(&qn, &crate::bgm::norm(cand));
            if s > sim {
                sim = s;
                which = if cand == &names[0] { zh_key } else { orig_key };
            }
        }
        if sim < MIN_TITLE_SIM {
            continue;
        }
        let date = it
            .get(if mt == "movie" { "release_date" } else { "first_air_date" })
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let got_year = year_num(date);
        let ys = crate::bgm::year_score(want_year, got_year);
        let total = if want_year.is_some() {
            0.72 * sim + 0.28 * ys
        } else {
            sim
        };
        if total < MIN_TOTAL {
            continue;
        }
        let Some(id) = it.get("id").and_then(|v| v.as_i64()) else { continue };
        let votes = it.get("vote_count").and_then(|v| v.as_i64()).unwrap_or(0);
        let matched_by = format!(
            "title={sim:.2}({which}) year={}/{} votes={votes} total={total:.2}",
            want_year.map(|y| y.to_string()).unwrap_or_else(|| "—".into()),
            got_year.map(|y| y.to_string()).unwrap_or_else(|| "—".into()),
        );
        // 同分取票数高的（同名条目里"没人看过"的那个大概率不是用户要的）
        let better = match &best {
            None => true,
            Some((b, _, _)) => total > b + 1e-9 || ((total - b).abs() < 1e-9 && votes > 0),
        };
        if better {
            best = Some((total, id, matched_by));
        }
    }
    best.map(|(_, id, mb)| (id, mb))
}

/// 精确锚点 → TMDB id：`arg.tmdbId` > trim_id(tm/纯数字) > trim_id(tt)/`arg.imdbId` 走 find >
/// 标题搜索（带门槛）。**命中方式（matched_by）随返回值一起给前端并写诊断日志**——
/// 用户/排查时能一眼看出这条数据是"精确锚定"还是"猜的"。
async fn resolve_tmdb_id(arg: &Value, mt: &str, title: &str, year: &str) -> Result<Resolved, String> {
    // ① 前端显式给的 tmdbId（数字或纯数字串）
    let explicit_tmdb = arg
        .get("tmdbId")
        .map(|v| {
            if let Some(n) = v.as_i64() {
                Some(n)
            } else {
                v.as_str().and_then(|s| s.trim().parse::<i64>().ok())
            }
        })
        .flatten()
        .filter(|n| *n > 0);
    if let Some(id) = explicit_tmdb {
        return Ok(Resolved { id, matched_by: format!("arg.tmdbId={id}") });
    }

    // ② trim_id 精确锚（tm*/纯数字 = TMDB id；tt* = IMDb → find；bg* = Bangumi → 只能搜索）
    let trim_raw = arg
        .get("trimId")
        .or_else(|| arg.get("trim_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let mut implicit_imdb = String::new();
    match parse_trim_id(&trim_raw) {
        Anchor::Tmdb(id) => {
            return Ok(Resolved {
                id,
                matched_by: format!("trim_id={trim_raw}"),
            });
        }
        Anchor::Imdb(imdb) => implicit_imdb = imdb,
        Anchor::Bangumi(bg) => {
            crate::libmpv::trace(&format!(
                "[tmdb] trim_id={trim_raw} 是 Bangumi 源（#{bg}，无 TMDB id）→ 回落标题搜索"
            ));
        }
        Anchor::None => {}
    }
    // ③ 前端显式给的 imdbId / trim_id 推出的 IMDb id → /3/find
    let imdb = arg
        .get("imdbId")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(implicit_imdb);
    if !imdb.is_empty() {
        let f = get_json(&find_path(&imdb)).await?;
        let bucket = if mt == "movie" { "/movie_results" } else { "/tv_results" };
        if let Some(id) = f
            .pointer(bucket)
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_i64())
        {
            return Ok(Resolved {
                id,
                matched_by: format!("imdb={imdb}"),
            });
        }
        crate::libmpv::trace(&format!(
            "[tmdb] find {imdb} 无 {mt} 结果 → 回落标题搜索"
        ));
    }

    // ④ 标题搜索（带最低相似度门槛）
    if title.trim().is_empty() {
        return Err("缺少标题且无可用 tmdb/imdb 锚点，无法匹配 TMDB 条目".to_string());
    }
    let want_year = year_num(year);
    let sr = get_json(&search_path(mt, title, year)).await?;
    let empty = Vec::new();
    let items = sr.get("results").and_then(|r| r.as_array()).unwrap_or(&empty);
    if let Some((id, matched_by)) = pick_best_search(items, title, want_year, mt) {
        return Ok(Resolved { id, matched_by });
    }
    // 带年份搜不到就退纯名再搜一次（主仓库 tmdbSearchBest 的"先精确名+年，退纯名"；年份错标很常见）
    if !year.is_empty() {
        crate::libmpv::trace(&format!(
            "[tmdb] 搜索「{title}」({year}) 无达标候选（{} 条）→ 退纯名重搜",
            items.len()
        ));
        let sr2 = get_json(&search_path(mt, title, "")).await?;
        let items2 = sr2.get("results").and_then(|r| r.as_array()).unwrap_or(&empty);
        if let Some((id, matched_by)) = pick_best_search(items2, title, want_year, mt) {
            return Ok(Resolved {
                id,
                matched_by: format!("{matched_by} (no-year-retry)"),
            });
        }
    }
    Err(format!(
        "TMDB 未找到与「{title}」足够相似的条目（相似度门槛 0.50）"
    ))
}

/* ══════════ 详情字段归一化（演员表 / 地区分组 / 预告片） ══════════ */

/// 演员表：剧集用 `aggregate_credits`（跨季聚合，roles[].episode_count 能给出"演了几集"），
/// 电影用 `credits`。返回 (cast, crew)：cast ≤ 24 人、按 order 升序；crew 取导演/编剧/制片 ≤ 16 人。
fn normalize_credits(d: &Value) -> (Vec<Value>, Vec<Value>) {
    let cast_src = d
        .pointer("/aggregate_credits/cast")
        .or_else(|| d.pointer("/credits/cast"))
        .and_then(|v| v.as_array());
    let mut cast = Vec::new();
    if let Some(arr) = cast_src {
        for c in arr.iter().take(40) {
            let Some(name) = c.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
                continue;
            };
            let mut roles: Vec<Value> = Vec::new();
            let mut episodes = 0i64;
            if let Some(rs) = c.get("roles").and_then(|v| v.as_array()) {
                let mut sorted: Vec<&Value> = rs.iter().collect();
                sorted.sort_by_key(|r| {
                    -r.get("episode_count").and_then(|v| v.as_i64()).unwrap_or(0)
                });
                for r in sorted.iter().take(3) {
                    let ch = r.get("character").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let ec = r.get("episode_count").and_then(|v| v.as_i64()).unwrap_or(0);
                    roles.push(json!({ "character": ch, "episodes": ec }));
                }
                episodes = rs
                    .iter()
                    .map(|r| r.get("episode_count").and_then(|v| v.as_i64()).unwrap_or(0))
                    .sum();
            }
            let character = c
                .get("character")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    roles
                        .first()
                        .and_then(|r| r.get("character"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                });
            if character.is_empty() && roles.is_empty() {
                roles.push(json!({ "character": "", "episodes": 0 }));
            }
            cast.push(json!({
                "id": c.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                "name": name,
                "character": character,
                "roles": roles,
                "episodes": episodes,
                "profile": c.get("profile_path").and_then(|v| v.as_str()).unwrap_or(""),
                "order": c.get("order").and_then(|v| v.as_i64()).unwrap_or(999),
            }));
        }
    }
    cast.sort_by_key(|c| c.get("order").and_then(|o| o.as_i64()).unwrap_or(999));
    cast.truncate(24);

    let crew_src = d
        .pointer("/aggregate_credits/crew")
        .or_else(|| d.pointer("/credits/crew"))
        .and_then(|v| v.as_array());
    let mut crew: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(arr) = crew_src {
        for c in arr.iter() {
            let dept = c.get("department").and_then(|v| v.as_str()).unwrap_or("");
            if !matches!(dept, "Directing" | "Writing" | "Production") {
                continue;
            }
            let Some(name) = c.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
                continue;
            };
            // 剧集是 jobs[]（多个职务），电影是单个 job
            let mut jobs: Vec<(String, i64)> = Vec::new();
            if let Some(js) = c.get("jobs").and_then(|v| v.as_array()) {
                for j in js {
                    let job = j.get("job").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let ec = j.get("episode_count").and_then(|v| v.as_i64()).unwrap_or(0);
                    if !job.is_empty() {
                        jobs.push((job, ec));
                    }
                }
            } else if let Some(job) = c.get("job").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                jobs.push((job.to_string(), 0));
            }
            for (job, ec) in jobs.into_iter().take(2) {
                if !seen.insert((name.to_string(), job.clone())) {
                    continue;
                }
                crew.push(json!({
                    "id": c.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                    "name": name,
                    "job": job,
                    "jobLabel": job_label(&job),
                    "department": dept,
                    "episodes": ec,
                    "profile": c.get("profile_path").and_then(|v| v.as_str()).unwrap_or(""),
                }));
                if crew.len() >= 16 {
                    break;
                }
            }
            if crew.len() >= 16 {
                break;
            }
        }
    }
    (cast, crew)
}

/// 职务中文化（认不出就用原文，绝不硬编）
fn job_label(job: &str) -> &'static str {
    match job {
        "Director" => "导演",
        "Writer" | "Screenplay" | "Story" => "编剧",
        "Producer" => "制片人",
        "Executive Producer" => "执行制片",
        "Creator" => "创作者",
        "Novel" | "Original Story" => "原作",
        "Director of Photography" => "摄影",
        "Original Music Composer" => "音乐",
        _ => "",
    }
}

/// 在线观看（watch/providers）：**按地区并列**返回，地区名由前端渲染成中文。
/// 地区顺序：常见区优先（PREFERRED_REGIONS 的顺序），其余按字母序兜底；只保留有数据的地区，
/// 最多 8 个。**不对 CN 做任何特判**——CN 没数据就自然不出现（不报错、不渲染空区块）。
fn providers_by_region(d: &Value) -> Vec<Value> {
    const PREFERRED_REGIONS: [&str; 16] = [
        "HK", "TW", "JP", "KR", "SG", "MY", "TH", "US", "GB", "CA", "AU", "DE", "FR", "IN", "RU", "BR",
    ];
    let Some(results) = d.pointer("/watch~1providers/results").and_then(|v| v.as_object()) else {
        return vec![];
    };
    let mut regions: Vec<&String> = results.keys().collect();
    let rank = |r: &str| {
        PREFERRED_REGIONS
            .iter()
            .position(|p| *p == r)
            .unwrap_or(PREFERRED_REGIONS.len())
    };
    regions.sort_by_key(|r| (rank(r), (*r).clone()));
    let mut out = Vec::new();
    for region in regions {
        let Some(entry) = results.get(region) else { continue };
        let Some(arr) = entry.get("flatrate").and_then(|v| v.as_array()) else {
            continue; // 该区只有 rent/buy（付费点播）→ 与"在线观看"语义不符，跳过
        };
        let mut items: Vec<Value> = arr
            .iter()
            .filter_map(|p| {
                let name = p.get("provider_name").and_then(|v| v.as_str())?;
                if name.ends_with("with Ads") || name.ends_with(" with Ads") {
                    return None;
                }
                Some(json!({
                    "name": name,
                    "logo": p.get("logo_path").and_then(|v| v.as_str()).unwrap_or(""),
                    "priority": p.get("display_priority").and_then(|v| v.as_i64()).unwrap_or(999),
                }))
            })
            .collect();
        if items.is_empty() {
            continue;
        }
        items.sort_by_key(|i| i.get("priority").and_then(|v| v.as_i64()).unwrap_or(999));
        let mut seen = std::collections::HashSet::new();
        items.retain(|i| seen.insert(i.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string()));
        out.push(json!({ "region": region, "items": items }));
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// 分级：tv 走 content_ratings（results[].rating），movie 走 release_dates
/// （release_dates[].certification，影院 version 优先）。**按地区并列**，空的地区不出现。
fn ratings_by_region(d: &Value, mt: &str) -> Vec<Value> {
    let mut out = Vec::new();
    if mt == "tv" {
        let Some(arr) = d.pointer("/content_ratings/results").and_then(|v| v.as_array()) else {
            return vec![];
        };
        for r in arr {
            let region = r.get("iso_3166_1").and_then(|v| v.as_str()).unwrap_or("");
            let rating = r.get("rating").and_then(|v| v.as_str()).unwrap_or("");
            if region.is_empty() || rating.is_empty() {
                continue;
            }
            out.push(json!({ "region": region, "rating": rating }));
        }
    } else {
        let Some(arr) = d.pointer("/release_dates/results").and_then(|v| v.as_array()) else {
            return vec![];
        };
        for r in arr {
            let region = r.get("iso_3166_1").and_then(|v| v.as_str()).unwrap_or("");
            let Some(dates) = r.get("release_dates").and_then(|v| v.as_array()) else {
                continue;
            };
            // type 3 = 影院上映：优先取它；否则取第一条非空认证
            let mut cert = String::new();
            let mut theatrical = String::new();
            for rd in dates {
                let c = rd.get("certification").and_then(|v| v.as_str()).unwrap_or("");
                if c.is_empty() {
                    continue;
                }
                if cert.is_empty() {
                    cert = c.to_string();
                }
                if rd.get("type").and_then(|v| v.as_i64()) == Some(3) {
                    theatrical = c.to_string();
                    break;
                }
            }
            let rating = if theatrical.is_empty() { cert } else { theatrical };
            if region.is_empty() || rating.is_empty() {
                continue;
            }
            out.push(json!({ "region": region, "rating": rating }));
        }
    }
    // 常见区在前，其余保持上游顺序（上游已按"有数据的国家/地区"给全，这里不做地区特判）
    const PREFERRED: [&str; 16] = [
        "HK", "TW", "JP", "KR", "SG", "MY", "TH", "US", "GB", "CA", "AU", "DE", "FR", "IN", "RU", "BR",
    ];
    let rank = |r: &str| PREFERRED.iter().position(|p| *p == r).unwrap_or(PREFERRED.len());
    out.sort_by_key(|v| {
        let r = v.get("region").and_then(|x| x.as_str()).unwrap_or("");
        rank(r)
    });
    out.truncate(10);
    out
}

/// 预告片：YouTube 的 Trailer/Teaser，最多 4 条（顺序即 TMDB 给的先后）
fn videos_of(d: &Value) -> Vec<Value> {
    let Some(arr) = d.pointer("/videos/results").and_then(|v| v.as_array()) else {
        return vec![];
    };
    arr.iter()
        .filter(|v| v.get("site").and_then(|s| s.as_str()) == Some("YouTube"))
        .filter(|v| matches!(v.get("type").and_then(|s| s.as_str()), Some("Trailer") | Some("Teaser")))
        .filter_map(|v| {
            let key = v.get("key").and_then(|s| s.as_str())?;
            if key.is_empty() {
                return None;
            }
            Some(json!({
                "key": key,
                "name": v.get("name").and_then(|s| s.as_str()).unwrap_or(""),
                "type": v.get("type").and_then(|s| s.as_str()).unwrap_or(""),
                "lang": v.get("iso_639_1").and_then(|s| s.as_str()).unwrap_or(""),
            }))
        })
        .take(4)
        .collect()
}

/// [lc-1433] 从 TMDB 详情响应里挑**面积最大**的一张图（横图/竖图通用）。
/// 取值顺序：`images.<list>` 里 w*h 最大 → 主图字段 `images.<list>` 之外的单值字段 → 空串。
///
/// 为什么要挑最大而不是取首位（实测数据，样本 `json:show:tv:316694`）：
///   · `images.backdrops` 首位 /3pEV… 是 3840×2160，但**主图 `backdrop_path` 只有 1920×1080**；
///   · `images.posters` 最大是 2000×3000，而 `poster_path` 压根不在这个列表里；
///   · 列表顺序由 TMDB 的 vote_average 决定，**与尺寸无关** —— 取首位纯属运气。
/// 前端把它拼成 `t/p/original{path}`（走 fntvimg 代理落盘），所以挑错一张就是整屏糊。
///
/// 抽成独立函数是为了能**离线单测**：这条规则错了只会表现为"图糊"（不报错、不崩），
/// 而真机验证要联网 + 密钥，所以必须有确定性测试钉住它。
fn pick_hero_path(d: &Value, list_key: &str, single_key: &str) -> String {
    let s = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    d.pointer(&format!("/images/{list_key}"))
        .and_then(|b| b.as_array())
        .and_then(|arr| {
            arr.iter()
                .filter_map(|b| {
                    let p = b.get("file_path").and_then(|v| v.as_str()).filter(|p| !p.is_empty())?;
                    let w = b.get("width").and_then(|v| v.as_i64()).unwrap_or(0);
                    let h = b.get("height").and_then(|v| v.as_i64()).unwrap_or(0);
                    Some((w * h, p.to_string()))
                })
                // max_by_key 在并列时返回**最后一个**最大项；并列（如同为 3840×2160）取哪张都行
                .max_by_key(|(area, _)| *area)
                .map(|(_, p)| p)
        })
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| s(single_key))
}

#[tauri::command]
pub async fn tmdb_show_details(arg: Value) -> Result<Value, String> {
    let mt = if arg.get("mediaType").and_then(|v| v.as_str()) == Some("movie") {
        "movie"
    } else {
        "tv"
    };
    let title = arg.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let year = arg.get("year").and_then(|v| v.as_str()).unwrap_or("").to_string();

    // 1) 定位条目：**trim_id/tmdbId/imdbId 精确锚优先**，锚不到才标题搜索（带相似度门槛）
    let Resolved { id, matched_by } = resolve_tmdb_id(&arg, mt, &title, &year).await?;
    crate::libmpv::trace(&format!("[tmdb] show_details mt={mt} title=「{title}」 → {id} ({matched_by})"));

    // 2) 详情 + append_to_response（对齐主仓库字段清单；语言/图片/视频参数全在 detail_path 里显式带）
    let append = if mt == "tv" {
        "aggregate_credits,credits,external_ids,keywords,content_ratings,alternative_titles,images,videos,watch/providers,recommendations"
    } else {
        "credits,external_ids,keywords,alternative_titles,images,videos,release_dates,watch/providers,recommendations"
    };
    // [lc-1417] **落盘缓存 120 天**：键按 mt + tmdb id，同一部剧任何入口进来都命中同一份。
    //   缓存的是**上游原始响应**（不是归一化结果）—— 归一化逻辑改了不必等缓存过期。
    let detail_key = format!("json:show:{mt}:{id}");
    let d = if let Some(v) = disk_cache::json_get(&detail_key, SHOW_TTL_SECS) {
        crate::libmpv::trace(&format!("[tmdb] show_details 命中缓存 {detail_key}"));
        v
    } else {
        let fresh = get_json(&detail_path(mt, id, append)).await?;
        disk_cache::json_put(&detail_key, &fresh);
        fresh
    };

    // 3) 归一化（字段与主仓库 normalizeShow 对齐，只取渲染需要的）
    let s = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    // 演员表：TMDB 主源（剧集 aggregate_credits 带"演了几集"）。前端另有 tmdb_credits（落盘缓存）优先用，
    // 这里保留一份是为了 credits 命令失败时仍有兜底数据（同一次 append，零额外请求）。
    let (cast, crew) = normalize_credits(&d);

    let backdrops: Vec<String> = d
        .pointer("/images/backdrops")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|b| {
                    let p = b.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                    !p.is_empty()
                        && p != s("backdrop_path")
                        && b.get("width").and_then(|w| w.as_i64()).map(|w| w >= 400).unwrap_or(true)
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .iter()
        .cloned()
        .collect::<Vec<Value>>()
        .into_iter()
        .map(|b| b.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string())
        .collect();

    // [lc-1433] 高清横图（轮播 hero 用）：**按面积挑最大的**，优先 3840×2160 这类原图。
    //   为什么单列一个字段：前端轮播要的是"一张尽量大的横图"，而 `backdrops` 是**有序列表**
    //   （主仓库按 vote_average 排），首位常常不是最大的那张；飞牛自带的 backdrops 更是**单个
    //   字符串**（实测缓存里大量 507×338 的小 webp）⇒ 整屏拉伸必糊。
    //   取值顺序：images.backdrops 里面积最大的 → 主图 backdrop_path → 空串（前端回退飞牛）。
    let hero_backdrop = pick_hero_path(&d, "backdrops", "backdrop_path");

    // [lc-1433] 高清竖版海报（详情页封面用）：同一套"按面积挑最大"逻辑，取值顺序
    //   images.posters 面积最大 → 主图 poster_path → 空串（前端回退 Bangumi 封面 / 飞牛）。
    let hero_poster = pick_hero_path(&d, "posters", "poster_path");

    // 相似剧集（recommendations）
    let recommendations: Vec<Value> = d
        .pointer("/recommendations/results")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    let tmdb_id = r.get("id")?.as_i64()?;
                    let t = if mt == "movie" {
                        r.get("title").or_else(|| r.get("original_title"))
                    } else {
                        r.get("name").or_else(|| r.get("original_name"))
                    }
                    .and_then(|v| v.as_str())?
                    .to_string();
                    let date = if mt == "movie" {
                        r.get("release_date")
                    } else {
                        r.get("first_air_date")
                    }
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                    Some(json!({
                        "tmdbId": tmdb_id,
                        "title": t,
                        "year": year_of(date),
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let recommendations: Vec<Value> =
        recommendations.into_iter().take(8).collect();

    // 别名（tv: results / movie: titles）
    let alt = if mt == "tv" {
        d.pointer("/alternative_titles/results")
    } else {
        d.pointer("/alternative_titles/titles")
    };
    let title_v = s("name");
    let title_v = if title_v.is_empty() { s("title") } else { title_v };
    let original = if mt == "movie" {
        s("original_title")
    } else {
        s("original_name")
    };
    let aliases: Vec<String> = alt
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.get("title").and_then(|t| t.as_str()))
                .filter(|t| !t.is_empty() && t != &title_v && t != &original)
                .take(8)
                .map(|t| t.to_string())
                .collect()
        })
        .unwrap_or_default();

    // 关键词
    let kw = if mt == "tv" {
        d.pointer("/keywords/results")
    } else {
        d.pointer("/keywords/keywords")
    };
    let keywords = names_of(kw).into_iter().take(16).collect::<Vec<_>>();

    // 在线看 / 分级：**按地区并列**（不再合并成一锅、也不给 CN 任何特判——没数据的地区直接不出现）
    let providers_by_region = providers_by_region(&d);
    let ratings_by_region = ratings_by_region(&d, mt);
    let videos = videos_of(&d);

    let air = if mt == "movie" { s("release_date") } else { s("first_air_date") };

    Ok(json!({
        "tmdbId": id,
        "matchedBy": matched_by,
        "mediaType": mt,
        "title": title_v,
        "originalTitle": original,
        "overview": s("overview"),
        "year": year_of(&air),
        "airDate": air,
        "status": s("status"),
        "seasons": d.get("number_of_seasons").and_then(|v| v.as_i64()).unwrap_or(0),
        "episodes": d.get("number_of_episodes").and_then(|v| v.as_i64()).unwrap_or(0),
        "rating": d.get("vote_average").and_then(|v| v.as_f64()).unwrap_or(0.0),
        "votes": d.get("vote_count").and_then(|v| v.as_i64()).unwrap_or(0),
        "popularity": d.get("popularity").and_then(|v| v.as_f64()).unwrap_or(0.0),
        "genres": names_of(d.get("genres")),
        "networks": names_of(d.get("networks")),
        "cast": cast,
        "crew": crew,
        // 剧照排序：vote_average 降序（主仓库 normalizeShow 同款）
        "backdrops": backdrops,
        // [lc-1433] 高清横图（面积最大的那张；轮播 hero 主用）——前端拼 `t/p/original{path}`，
        //   走 fntvimg 代理**拉了即落盘**；空串 = TMDB 没图，前端回退飞牛自带图。
        "heroBackdrop": hero_backdrop,
        // [lc-1433] 高清竖版海报（面积最大；详情页封面主用），同样拼 `t/p/original`。
        "heroPoster": hero_poster,
        "keywords": keywords,
        "aliases": aliases,
        // 预告片（include_video_language=zh-CN,en,ja,null 才拿得到）
        "videos": videos,
        // 按地区并列的在线观看与分级（前端逐地区渲染并标地区名）
        "providersByRegion": providers_by_region,
        "ratingsByRegion": ratings_by_region,
        "recommendations": recommendations,
        "externalIds": {
            "imdb": d.pointer("/external_ids/imdb_id").and_then(|v| v.as_str())
                .or_else(|| d.get("imdb_id").and_then(|v| v.as_str())).unwrap_or(""),
            "tmdb": id.to_string(),
            "wikidata": d.pointer("/external_ids/wikidata_id").and_then(|v| v.as_str()).unwrap_or(""),
            "instagram": d.pointer("/external_ids/instagram_id").and_then(|v| v.as_str()).unwrap_or(""),
            "twitter": d.pointer("/external_ids/twitter_id").and_then(|v| v.as_str()).unwrap_or(""),
        },
    }))
}

/// 演员表（**TMDB 主源**，前端详情页「演职人员」用它）：精确锚点（trim_id tm*/tt*、tmdbId、imdbId）
/// 优先 → 标题搜索（相似度门槛）兜底；结果按 `cacheKey`（fnOS 条目 guid）与 `tmdb id` **两层落盘缓存**，
/// 7 天 TTL —— 命中本地就**绝不重复请求**（用户明确要求"拉了就本地缓存不要多拉"）。
///
/// arg:
///   title/year/mediaType —— 搜索兜底与展示用
///   trimId/tmdbId/imdbId —— 精确锚点（能定位就不搜索）
///   cacheKey            —— fnOS 条目 guid（同一部剧不同季/集共用一条缓存时传同一个键）
/// 返回 { tmdbId, mediaType, matchedBy, cast[], crew[] }（字段形状见 normalize_credits）。
#[tauri::command]
pub async fn tmdb_credits(arg: Value) -> Result<Value, String> {
    let mt = if arg.get("mediaType").and_then(|v| v.as_str()) == Some("movie") {
        "movie"
    } else {
        "tv"
    };
    let title = arg.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let year = arg.get("year").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cache_key = arg
        .get("cacheKey")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // ① 条目级缓存（guid → 结果）：命中就完全不发请求，连"定位"都省了
    let guid_key = cache_key.as_ref().map(|k| format!("json:credits:guid:{k}"));
    if let Some(k) = &guid_key {
        if let Some(v) = disk_cache::json_get(k, CREDITS_TTL_SECS) {
            crate::libmpv::trace(&format!("[tmdb] credits 缓存命中（guid {k}）"));
            return Ok(v);
        }
    }

    // ② 定位 TMDB id（精确锚优先）
    let Resolved { id, matched_by } = resolve_tmdb_id(&arg, mt, &title, &year).await?;
    let data_key = format!("json:credits:{mt}:{id}");

    // ③ 条目共享缓存（tmdb id → 演员表）：同一部剧的季/集条目共用
    let payload = if let Some(mut v) = disk_cache::json_get(&data_key, CREDITS_TTL_SECS) {
        crate::libmpv::trace(&format!("[tmdb] credits 命中 id 缓存 {data_key}"));
        // 缓存体里的 matchedBy 是"当时怎么定位到的"；本次定位方式可能不同（guid 已变/trim 新增）
        if let Some(o) = v.as_object_mut() {
            o.insert("matchedBy".into(), json!(matched_by));
            o.insert("cached".into(), json!(true));
        }
        v
    } else {
        let d = get_json(&credits_path(mt, id)).await?;
        let (cast, crew) = normalize_credits(&d);
        let v = json!({
            "tmdbId": id,
            "mediaType": mt,
            "matchedBy": matched_by,
            "cached": false,
            "cast": cast,
            "crew": crew,
        });
        disk_cache::json_put(&data_key, &v);
        v
    };

    if let Some(k) = &guid_key {
        disk_cache::json_put(k, &payload);
    }
    crate::libmpv::trace(&format!(
        "[tmdb] credits mt={mt} id={id} 演职员 {} 条 ({matched_by})",
        payload.get("cast").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0)
    ));
    Ok(payload)
}

/// [lc-123] 发现页左栏取数：`/3/discover/{tv|movie}` 一条命令带三种口味（**参数**决定，不做三条命令：
/// 三者只差 query，拆开只会把"vote_count 例外"这类边界抄三遍）。
///   arg.mode      : "season"（默认）= 当季热门网格；"upcoming" = 即将播出
///   arg.mediaType : "tv"（默认）| "movie"（决定 discover 路径与日期字段名）
///   arg.season    : "current"（默认）| "next"（只对 mode=season 有意义）
///   arg.page      : 1..25（默认 1，夹紧；镜像/上游都有缓存，翻页成本低）
///                   [lc-123-rev] 3 → 25：「即将播出」的「加载更多」要能翻到几百条
///                   （20 条/页 × 25 = 500；上游 total_pages 由前端再夹一次）
/// 参数取值（**全部经真机实测**，见 tests::live_* 与报告）：
///   当季：sort_by=popularity.desc + first_air_date.gte/lte=<当季首末> + vote_count.gte=5
///         （5 是"滤掉刚建条目/零票垃圾"的下限；取 10 会把季初的网格削到十几条）
///   即将：sort_by=popularity.desc + first_air_date.gte=<今天> —— **绝不能带 vote_count.gte**：
///         未开播条目 vote_count=0，同一过滤器会把整个列表滤空（实测 total_results=0）。
/// language **由客户端显式带上**（`discover_path` 里统一拼 `language=zh-CN`）——镜像已不再注入，
/// 依赖镜像默认值会在换端点/换 worker 版本时静默退回英文标题。
/// 每条 item 返回 `poster` 与 **`backdropPath`**（16:9 横图，前端主推卡/横向卡优先用它）。
/// [lc-123-rev5] 落盘缓存 + 条件请求（TTL 见 DISCOVER_TTL_*）：
///   arg.force  : true = **跳过本地 TTL 检查**去探测（手动刷新用；**仍然带 If-None-Match**）
///   arg.noEtag : true = 连条件头都不带（**失败重试**用：强制全量重拉）
///   返回里带 `changed`：false = 缓存命中或 304（**内容与上次一致，前端不许碰 UI**）。
#[tauri::command]
pub async fn tmdb_discover(arg: Value) -> Result<Value, String> {
    let is_movie = arg.get("mediaType").and_then(|v| v.as_str()) == Some("movie");
    let mode = if arg.get("mode").and_then(|v| v.as_str()) == Some("upcoming") {
        "upcoming"
    } else {
        "season"
    };
    let next = arg.get("season").and_then(|v| v.as_str()) == Some("next");
    let page = arg.get("page").and_then(|v| v.as_i64()).unwrap_or(1).clamp(1, 25); // [lc-123-rev] 3→25：见上方 arg.page 注释
    let force = arg.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let no_etag = arg.get("noEtag").and_then(|v| v.as_bool()).unwrap_or(false);
    let media_type = if is_movie { "movie" } else { "tv" };
    // tv 的日期字段是 first_air_date，电影是 primary_release_date —— 同一条路径按类型换名
    let (gte_key, lte_key) = if is_movie {
        ("primary_release_date.gte", "primary_release_date.lte")
    } else {
        ("first_air_date.gte", "first_air_date.lte")
    };
    let today = today_ymd();
    let (gte, lte) = if mode == "upcoming" {
        (today.clone(), String::new())
    } else {
        // 当季/下季：由**今天**推季度首末（日期分解就地完成，不做"字符串→元组"的双表示）
        let (ty, tm, _) = civil_from_days((mirror_sign::now_secs() / 86_400) as i64);
        quarter_bounds(ty, tm, next)
    };

    // 路径统一由 discover_path 拼（含显式 language=zh-CN 与 vote_count 例外）
    let path = discover_path(
        media_type,
        gte_key,
        &gte,
        lte_key,
        &lte,
        mode == "season",
        page,
    );

    // [lc-123-rev5] 「先读缓存 → 判断 TTL → 过期才打网络」的唯一入口：
    //   · 缓存新鲜且非 force → 命中即返回（**零网络**）； · 过期/force → 带 ETag 条件请求（304 就不动内容）。
    let season_key = if mode == "upcoming" {
        "upcoming"
    } else if next {
        "next"
    } else {
        "current"
    };
    // 缓存键：**稳定量**（媒体类型 + 模式 + 季度 + 页码 + 日期窗口）——
    // 绝不用飞牛 guid 这类"重新刮削就变"的东西（那会让缓存永远命中不了）。
    let cache_key =
        format!("json:tmdb:discover:v1|{media_type}|{mode}|{season_key}|{page}|{gte}|{lte}");
    let ttl = if mode == "upcoming" {
        DISCOVER_TTL_UPCOMING_SECS
    } else {
        DISCOVER_TTL_SEASON_SECS
    };
    let (payload, changed) = disk_cache::json_cached_conditional(
        &cache_key,
        ttl,
        force,
        no_etag,
        |_| true,
        |etag| {
            let path = path.clone();
            let today = today.clone();
            let gte = gte.clone();
            let lte = lte.clone();
            let mode = mode;
            let media_type = media_type;
            async move {
                match get_json_cond(&path, etag.as_deref()).await? {
                    MirrorGet::NotModified => Ok(disk_cache::Cond::NotModified),
                    MirrorGet::Fresh(raw, et) => Ok(disk_cache::Cond::Fresh(
                        discover_payload(&raw, is_movie, &today, mode, media_type, next, &gte, &lte, page),
                        et,
                    )),
                }
            }
        },
    )
    .await?;

    let mut out = payload;
    if let Value::Object(ref mut m) = out {
        // changed=false（缓存命中 / 304）时前端**不得**碰 UI：不加这个标志，前端只能"每次都重画"
        m.insert("changed".to_string(), json!(changed));
    }
    Ok(out)
}

/// discover 原始响应 → 前端 payload（items[] + 窗口/分页元信息）。
/// **纯函数**：同一份原始响应永远得到同一份 payload —— 条件请求的 fetch 闭包与单测都复用这一份。
fn discover_payload(
    d: &Value,
    is_movie: bool,
    today: &str,
    mode: &str,
    media_type: &str,
    next: bool,
    gte: &str,
    lte: &str,
    page: i64,
) -> Value {
    let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let items: Vec<Value> = d
        .get("results")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    let id = r.get("id").and_then(|v| v.as_i64())?;
                    let title = if is_movie {
                        let t = s(r, "title");
                        if t.is_empty() { s(r, "original_title") } else { t }
                    } else {
                        let t = s(r, "name");
                        if t.is_empty() { s(r, "original_name") } else { t }
                    };
                    let original = if is_movie { s(r, "original_title") } else { s(r, "original_name") };
                    let date = if is_movie { s(r, "release_date") } else { s(r, "first_air_date") };
                    // 状态：discover **不返回 status**（那是详情端点才有）。这里只给一个**可辩护的**二态：
                    // 首播日期还没到 = 未开播；否则 = 已开播。**不编「连载中/已完结」**（discover 无从得知）。
                    let state = if date.is_empty() {
                        "unknown"
                    } else if date.as_str() > today {
                        "upcoming"
                    } else {
                        "airing"
                    };
                    Some(json!({
                        "tmdbId": id,
                        "title": title,
                        "originalTitle": original,
                        "year": year_of(&date),
                        "date": date,
                        "state": state,
                        "rating": r.get("vote_average").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        "votes": r.get("vote_count").and_then(|v| v.as_i64()).unwrap_or(0),
                        "popularity": r.get("popularity").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        "poster": s(r, "poster_path"),
                        // [lc-123-rev3] **横版大图**：TMDB 的标准结果对象本来就带 backdrop_path，
                        // 之前这一格漏映射 ⇒ 前端只能拿 2:3 海报硬裁进 16:9（用户截图里的"主体被切"）。
                        // 实测（带签名直打镜像，2026-10-13）：当季剧集 18/18、当季电影 20/20、
                        // 即将播出 18/20 —— 合计 56/58 = 96.6% 非空；缺失的 2 条连 poster 也没有。
                        "backdropPath": s(r, "backdrop_path"),
                        "overview": s(r, "overview"),
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    crate::libmpv::trace(&format!(
        "[tmdb] discover mode={mode} media={media_type} season={} {gte}..{lte} page={page} → {} 条",
        if mode == "upcoming" { "upcoming" } else if next { "next" } else { "current" },
        items.len()
    ));

    json!({
        "mode": mode,
        "mediaType": media_type,
        "season": if mode == "upcoming" { "upcoming" } else if next { "next" } else { "current" },
        "gte": gte,
        "lte": lte,
        "page": page,
        "total": d.get("total_results").and_then(|v| v.as_i64()).unwrap_or(0),
        "totalPages": d.get("total_pages").and_then(|v| v.as_i64()).unwrap_or(0),
        "items": items,
    })
}

/// "YYYY-MM-DD" → (年, 月, 日)。日期算术的输入输出都用字符串（today_ymd 产出的形态），
/// 这里只作为**自测断言**的解析器（生产路径不经过它）。
#[cfg_attr(not(test), allow(dead_code))]
fn parse_ymd(s: &str) -> Option<(i64, u32, u32)> {
    let b = s.as_bytes();
    if b.len() < 10 {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let m: u32 = s.get(5..7)?.parse().ok()?;
    let d: u32 = s.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

#[cfg(test)]
mod tests {
    use super::{auth_params, get_json, pick_hero_path, quarter_bounds, today_ymd, DEFAULT_ENDPOINTS};
    use serde_json::json;

    /// [lc-1433] 高清图挑选规则（**离线、零网络**）：必须按**面积**挑最大，而不是取首位。
    /// 数据是**真机抓下来的 TMDB 响应**（`json:show:tv:316694`，落盘缓存里那份）：
    ///   images.backdrops = [3840×2160, 3840×2160, 1920×1080]，而 backdrop_path = 1920×1080
    ///   images.posters   = [2000×3000, 1920×2711, 1920×2700]，而 poster_path **不在列表里**
    /// 这条规则错了只会表现为"图糊"（不报错、不崩），真机验证又要联网+密钥，
    /// 所以必须在这里钉死：主图字段（backdrop_path/poster_path）恰恰**常常不是最大的那张**。
    #[test]
    fn hero_path_picks_largest_by_area_not_first() {
        // ⚠ 夹具刻意让**最大的那张不在首位**：TMDB 的列表顺序由 vote_average 决定、
        //   与尺寸无关，所以"取首位"是真实存在过的错误实现（lc-1430 前就是那样）。
        //   夹具若把最大项放在首位，这条测试对"取首位"的错误实现**无感**（会假通过）。
        let d = json!({
            "backdrop_path": "/small-1920x1080.jpg",
            "poster_path": "/not-in-list.jpg",
            "images": {
                "backdrops": [
                    {"file_path": "/mid-1920x1080.jpg", "width": 1920, "height": 1080},
                    {"file_path": "/tiny-1280x720.jpg", "width": 1280, "height": 720},
                    {"file_path": "/big-3840x2160.jpg", "width": 3840, "height": 2160}
                ],
                "posters": [
                    {"file_path": "/p-1000x1500.jpg", "width": 1000, "height": 1500},
                    {"file_path": "/p-2000x3000.jpg", "width": 2000, "height": 3000}
                ]
            }
        });
        assert_eq!(pick_hero_path(&d, "backdrops", "backdrop_path"), "/big-3840x2160.jpg");
        assert_eq!(pick_hero_path(&d, "posters", "poster_path"), "/p-2000x3000.jpg");

        // 横图必须用 backdrops 那份（不能串到 posters）
        assert_ne!(pick_hero_path(&d, "backdrops", "backdrop_path"), "/p-2000x3000.jpg");
    }

    /// 回退链：images 列表缺失/空/全是空 file_path 时 → 用主图字段；主图也空 → 空串
    /// （空串是**约定**：前端据此回退飞牛自带图，绝不能给个假路径）。
    #[test]
    fn hero_path_falls_back_to_single_then_empty() {
        // ① 没有 images → 用主图字段
        let d = json!({"backdrop_path": "/only.jpg"});
        assert_eq!(pick_hero_path(&d, "backdrops", "backdrop_path"), "/only.jpg");

        // ② images 是空数组 → 用主图字段
        let d = json!({"backdrop_path": "/only.jpg", "images": {"backdrops": []}});
        assert_eq!(pick_hero_path(&d, "backdrops", "backdrop_path"), "/only.jpg");

        // ③ 列表里全是空 file_path → 用主图字段（不能选中空串）
        let d = json!({
            "backdrop_path": "/only.jpg",
            "images": {"backdrops": [{"file_path": "", "width": 9999, "height": 9999}]}
        });
        assert_eq!(pick_hero_path(&d, "backdrops", "backdrop_path"), "/only.jpg");

        // ④ 两边都没有 → 空串（前端回退飞牛）
        let d = json!({});
        assert_eq!(pick_hero_path(&d, "backdrops", "backdrop_path"), "");
        let d = json!({"images": {"backdrops": []}});
        assert_eq!(pick_hero_path(&d, "backdrops", "backdrop_path"), "");

        // ⑤ 没有 width/height 的项按面积 0 算，输给有尺寸的（同样把"有尺寸的"放在后面，
        //    这样"取首位"的错误实现也会在这条上失败）
        let d = json!({
            "images": {"backdrops": [
                {"file_path": "/no-dims.jpg"},
                {"file_path": "/has-dims.jpg", "width": 10, "height": 10}
            ]}
        });
        assert_eq!(pick_hero_path(&d, "backdrops", "backdrop_path"), "/has-dims.jpg");
    }

    /// 静态断言（不触网）：默认路径必须是**镜像**，绝不能是上游官方域名
    /// （api.themoviedb.org 只允许出现在降级直连常量 DIRECT_API 里）。
    #[test]
    fn default_endpoint_is_mirror_not_upstream() {
        let eps =
            crate::mirror_sign::resolve_endpoints("FNTV_TMDB_ENDPOINT_UNSET_XYZ", DEFAULT_ENDPOINTS);
        assert_eq!(eps[0], "https://tmdb.690075.xyz");
        assert!(eps.iter().all(|e| !e.contains("api.themoviedb.org")));
        assert!(!DEFAULT_ENDPOINTS.contains("api.themoviedb.org"));
    }

    /// 降级直连的 key 形态分派（不触网）：v4 JWT → Bearer 头；v3 → api_key query。
    /// v3 分支在本机没有可用的 v3 key，**没法真机覆盖**，所以由这条单元测试钉住
    /// （缺了它就只剩 v4 一条被真机验过）。
    #[test]
    fn auth_params_splits_v4_jwt_from_v3_key() {
        let (q, h) = auth_params("eyJhbGciOiJIUzI1NiJ9.eyJzY29wZSI6ImFwaSJ9.sig");
        assert!(q.is_empty(), "v4 不应带 query 参数");
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].0, "Authorization");
        assert!(h[0].1.starts_with("Bearer "), "v4 必须走 Bearer 头");

        let (q, h) = auth_params("0123456789abcdef0123456789abcdef");
        assert!(h.is_empty(), "v3 不应带 Authorization 头");
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].0, "api_key");
        assert_eq!(q[0].1, "0123456789abcdef0123456789abcdef");
    }

    /// 日期算术（**不触网**）：往返自洽 + 已知锚点 + 季度边界。
    /// 钉住的是"当季/下季窗口"这唯一一处日期逻辑：跨年（Q4→次 Q1）、季度分档（1/4/7/10）、
    /// 以及大月 31 天与闰年 2 月的真实长度（30/31 写死在这里会静默多／少一天）。
    #[test]
    fn date_math_and_quarter_bounds() {
        // 锚点：1970-01-01 = 第 0 天；往返在含闰日的一段上自洽
        assert_eq!(super::civil_from_days(0), (1970, 1, 1));
        let a = super::days_from_civil(2024, 1, 1);
        let b = super::days_from_civil(2024, 3, 1);
        assert_eq!(b - a, 60, "2024 是闰年，1/1→3/1 应为 60 天");
        for z in a..b {
            let (y, m, d) = super::civil_from_days(z);
            assert_eq!(super::days_from_civil(y, m, d), z, "往返应自洽：{z}");
        }

        assert_eq!(quarter_bounds(2026, 10, false), ("2026-10-01".into(), "2026-12-31".into()));
        assert_eq!(quarter_bounds(2026, 10, true), ("2027-01-01".into(), "2027-03-31".into()));
        assert_eq!(quarter_bounds(2026, 12, true), ("2027-01-01".into(), "2027-03-31".into()));
        assert_eq!(quarter_bounds(2026, 1, false), ("2026-01-01".into(), "2026-03-31".into()));
        assert_eq!(quarter_bounds(2024, 1, false), ("2024-01-01".into(), "2024-03-31".into()));
        assert_eq!(quarter_bounds(2026, 7, false), ("2026-07-01".into(), "2026-09-30".into()));
        // 4 月/11 月是 30 天（写死 31 会多取/少取一天）
        assert_eq!(quarter_bounds(2026, 4, false), ("2026-04-01".into(), "2026-06-30".into()));

        // today_ymd 必须是可解析的 YYYY-MM-DD（discover 的日期窗口直接吃这个字符串）
        let t = today_ymd();
        assert_eq!(t.len(), 10);
        assert!(super::parse_ymd(&t).is_some(), "today_ymd 应可解析：{t}");
    }

    /// 真机冒烟（**默认忽略**）：发现页两条取数链打真镜像。跑法：
    ///   FNTV_TMDB_SECRET=<值> cargo test --lib tmdb::tests::live_mirror_discover -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "live network + FNTV_TMDB_SECRET required"]
    async fn live_mirror_discover_season_and_upcoming() {
        // 当季：窗口形态必须成立（gte ≤ lte、都是 YYYY-MM-DD）
        let s = super::tmdb_discover(serde_json::json!({"mode":"season","mediaType":"tv","season":"current"}))
            .await
            .expect("当季热门应成功");
        let gte = s["gte"].as_str().unwrap_or("").to_string();
        let lte = s["lte"].as_str().unwrap_or("").to_string();
        assert_eq!(gte.len(), 10, "gte 应是日期：{gte}");
        assert!(gte <= lte, "gte 不应晚于 lte：{gte} .. {lte}");
        println!("season items={} total={}", s["items"].as_array().map(|a| a.len()).unwrap_or(0), s["total"]);

        // 即将播出：**这条断言就是钉住那个坑** —— 一旦有人把 vote_count.gte 也加上，未开播条目
        // （vote_count=0）会被整片滤掉，这里立刻变空。
        let u = super::tmdb_discover(serde_json::json!({"mode":"upcoming"}))
            .await
            .expect("即将播出应成功");
        let n = u["items"].as_array().map(|a| a.len()).unwrap_or(0);
        assert!(n > 0, "即将播出不应为空（带 vote_count.gte 会滤空）: {u}");

        // 电影口味同一条命令
        let m = super::tmdb_discover(serde_json::json!({"mode":"season","mediaType":"movie"}))
            .await
            .expect("电影当季应成功");
        assert_eq!(m["mediaType"].as_str(), Some("movie"));
    }

    /// 真机冒烟（**默认忽略**，不进 CI）：带 FNTV_TMDB_SECRET 直打 tmdb 镜像，
    /// 验证「共享签名 + 请求构造 + JSON 解析」整链在真实链路上成立。
    /// 手动跑：FNTV_TMDB_SECRET=<值> cargo test --lib tmdb -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "live network + FNTV_TMDB_SECRET required"]
    async fn live_mirror_get_configuration() {
        let v = get_json("/3/configuration?language=zh-CN")
            .await
            .expect("live mirror GET /3/configuration 应成功");
        assert!(v.get("images").is_some(), "configuration 应含 images 段");
    }

    /// 真机冒烟（**默认忽略**）：走**真实生产路径**——中文标题经 urlencoding 编码进搜索 query、
    /// 详情的 append_to_response / include_image_language 都含**逗号**——验证「签名串取自解析后的
    /// Url」在含中文百分号转义与逗号的 query 上不漂移（这是签名路径最容易翻车的编码差异）。
    /// 手动跑：FNTV_TMDB_SECRET=<值> cargo test --lib tmdb -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "live network + FNTV_TMDB_SECRET required"]
    async fn live_mirror_detail_with_cjk_and_comma_query() {
        let search = format!(
            "/3/search/tv?query={}&language=zh-CN&first_air_date_year=2014",
            urlencoding::encode("画江湖之不良人")
        );
        let sr = get_json(&search).await.expect("搜索应成功");
        let id = sr
            .pointer("/results/0/id")
            .and_then(|v| v.as_i64())
            .expect("搜索结果应含 id");
        let detail = format!(
            "/3/tv/{id}?language=zh-CN&append_to_response=credits,external_ids,images\
             &include_image_language=zh-CN,en,null"
        );
        let d = get_json(&detail).await.expect("详情应成功");
        assert_eq!(d.get("id").and_then(|v| v.as_i64()), Some(id));
    }

    /// 降级链真机检查（**默认忽略**）：先把 FNTV_TMDB_ENDPOINT 指向一个不可达地址模拟镜像故障，
    /// 预期日志出现「[tmdb] 镜像不可用，降级直连（用户 key）」，随后按直连结果成败（断网/被墙时
    /// 报错也必须点明已尝试降级）。跑法：
    ///   FNTV_TMDB_ENDPOINT=http://127.0.0.1:9 cargo test --lib tmdb -- --ignored live_fallback --nocapture
    #[tokio::test]
    #[ignore = "needs FNTV_TMDB_ENDPOINT pointed at an unreachable host"]
    async fn live_fallback_attempts_direct() {
        match get_json("/3/configuration?language=zh-CN").await {
            Ok(v) => assert!(v.get("images").is_some(), "降级直连应拿到 configuration"),
            Err(e) => assert!(e.contains("降级直连"), "错误应点明已尝试降级：{e}"),
        }
    }

    /// 缺密钥降级（**默认忽略**）：**必须先把 FNTV_TMDB_SECRET 置空**再跑，验证「缺密钥」不再直接失败
    /// （旧行为：Fatal → 整个 TMDB 功能死掉，比改造前还糟），而是按「镜像不可用」降级直连。
    /// 断言对有无个人 key 都成立：有 key 且直连通 → Ok；直连不通/无 key → 错误里必须点明已尝试降级。
    /// 跑法（Git Bash）：
    ///   FNTV_TMDB_SECRET=" " cargo test --lib tmdb::tests::live_missing_secret_falls_back -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "live network; run with FNTV_TMDB_SECRET blanked"]
    async fn live_missing_secret_falls_back() {
        match get_json("/3/configuration?language=zh-CN").await {
            Ok(v) => assert!(v.get("images").is_some(), "降级直连应拿到 configuration"),
            Err(e) => assert!(e.contains("降级直连"), "错误应点明已尝试降级：{e}"),
        }
    }

    /* ────────── [lc-128] 静态断言（不触网）：语言参数 / 429 分类 / trim_id 锚点 ────────── */

    /// 所有请求路径都必须**显式**带语言参数（镜像已不再注入 zh-CN）。
    /// 这条是"以后新增端点又漏了 language"的哨兵：漏了就在这里红。
    #[test]
    fn every_request_path_carries_explicit_language() {
        let sp = super::search_path("tv", "画江湖之不良人", "2014");
        assert!(sp.starts_with("/3/search/tv?query="), "{sp}");
        assert!(sp.contains("language=zh-CN"), "搜索必须显式带 language：{sp}");

        let dp = super::detail_path("tv", 1399, "credits,images,videos");
        assert!(dp.starts_with("/3/tv/1399?"), "{dp}");
        assert!(dp.contains("language=zh-CN"), "详情必须显式带 language：{dp}");
        assert!(dp.contains("include_image_language=zh-CN,en,null"), "图片语言：{dp}");
        assert!(
            dp.contains("include_video_language=zh-CN,en,ja,null"),
            "视频语言（不然预告片会被 zh-CN 滤空）：{dp}"
        );

        let cp = super::credits_path("movie", 550);
        assert!(cp.contains("language=zh-CN"), "演员表必须显式带 language：{cp}");
        assert!(cp.contains("append_to_response=credits"), "{cp}");
        // 剧集要 aggregate_credits（带 roles[].episode_count）
        assert!(super::credits_path("tv", 1399).contains("aggregate_credits"));

        let fp = super::find_path("tt325158");
        assert!(fp.contains("external_source=imdb_id"), "{fp}");
        assert!(fp.contains("language=zh-CN"), "find 也要带 language：{fp}");

        let dpc = super::discover_path("tv", "first_air_date.gte", "2026-10-01", "first_air_date.lte", "2026-12-31", true, 1);
        assert!(dpc.contains("language=zh-CN"), "发现页必须显式带 language：{dpc}");
        assert!(dpc.contains("vote_count.gte=5"), "{dpc}");
        // 即将播出**不能**带 vote_count.gte（未开播条目 vote_count=0 会被滤空）
        let up = super::discover_path("tv", "first_air_date.gte", "2026-10-12", "", "", false, 1);
        assert!(!up.contains("vote_count.gte"), "即将播出不得带 vote_count：{up}");
    }

    /// 两种 429 **必须是两种处理**：
    ///   throttled（我们镜像每 IP 限）→ 退避重试；upstream_rate_limited（上游 = RATE_LIMIT）→ 降级直连。
    #[test]
    fn classify_429_separates_mirror_throttle_from_upstream_rate_limit() {
        assert_eq!(
            super::classify_429(r#"{"error":"throttled"}"#),
            super::Throttle429::MirrorThrottled,
            "镜像自己的短时限流必须与上游限流分开"
        );
        assert_eq!(
            super::classify_429(r#"{"error":"upstream_rate_limited","hint":"上游限流，请稍后重试"}"#),
            super::Throttle429::UpstreamRateLimited,
            "上游 429（镜像统计 RATE_LIMIT）走原有降级路径"
        );
        // 结构不认识（老镜像/网关页）→ 保守：按上游限流处理，不无限重试
        assert_eq!(super::classify_429("<html>429</html>"), super::Throttle429::Unknown);
    }

    /// trim_id 锚点解析：tm* = TMDB id、tt* = IMDb、bg* = Bangumi（无 TMDB id）、纯数字按老口径当 TMDB id。
    #[test]
    fn trim_id_anchor_parsing() {
        use super::{parse_trim_id, Anchor};
        assert_eq!(parse_trim_id("tm63578"), Anchor::Tmdb(63578));
        assert_eq!(parse_trim_id("TM63578"), Anchor::Tmdb(63578));
        assert_eq!(parse_trim_id("tt325158"), Anchor::Imdb("tt325158".into()));
        assert_eq!(parse_trim_id("bg456080"), Anchor::Bangumi(456080));
        assert_eq!(parse_trim_id("1399"), Anchor::Tmdb(1399));
        assert_eq!(parse_trim_id("  tt0137523 "), Anchor::Imdb("tt0137523".into()));
        assert_eq!(parse_trim_id(""), Anchor::None);
        assert_eq!(parse_trim_id("mystery-token"), Anchor::None);
    }

    /// 搜索门槛：相似的能选中；不相似的**必须拒绝**（旧行为是"取第一条"，会把同名烂片贴上去）。
    #[test]
    fn search_gate_rejects_dissimilar_first_result() {
        let items = serde_json::json!([
            {"id": 1, "name": "完全不相干的另一部剧", "original_name": "Totally Unrelated", "first_air_date": "2019-01-01", "vote_count": 9000},
            {"id": 2, "name": "画江湖之不良人", "original_name": "Hua Jianghu", "first_air_date": "2014-07-31", "vote_count": 40}
        ]);
        let hit = super::pick_best_search(items.as_array().unwrap(), "画江湖之不良人", Some(2014), "tv")
            .expect("同名条目应当选中");
        assert_eq!(hit.0, 2, "应当选相似度高的那一条，而不是搜索结果第一条");
        assert!(hit.1.contains("total="), "matched_by 应可读：{}", hit.1);

        // 全部候选都不像（比如标题是长描述）→ 拒绝，绝不给"最像的烂片"
        let junk = serde_json::json!([
            {"id": 7, "name": "zzz", "first_air_date": "2020-01-01", "vote_count": 5}
        ]);
        assert!(
            super::pick_best_search(junk.as_array().unwrap(), "画江湖之不良人", Some(2014), "tv").is_none(),
            "相似度不达标必须拒绝"
        );
    }

    /// 演员表归一化：剧集走 aggregate_credits（roles[].episode_count 汇总成"演了几集"），
    /// 电影走 credits；crew 只留导演/编剧/制片并给中文职务。
    #[test]
    fn credits_normalization_tv_and_movie() {
        let tv = serde_json::json!({
            "aggregate_credits": {
                "cast": [
                    {"id": 1, "name": "演员甲", "profile_path": "/a.jpg", "order": 1,
                     "roles": [{"character": "角色一", "episode_count": 6}, {"character": "角色二", "episode_count": 2}]},
                    {"id": 2, "name": "演员乙", "order": 0, "roles": [{"character": "主角", "episode_count": 8}]}
                ],
                "crew": [
                    {"id": 9, "name": "导演甲", "department": "Directing", "jobs": [{"job": "Director", "episode_count": 8}]},
                    {"id": 10, "name": "灯光师", "department": "Lighting", "jobs": [{"job": "Gaffer"}]}
                ]
            }
        });
        let (cast, crew) = super::normalize_credits(&tv);
        assert_eq!(cast.len(), 2);
        assert_eq!(cast[0]["name"], "演员乙", "order 小的排前面");
        assert_eq!(cast[1]["character"], "角色一", "取集数最多的角色");
        assert_eq!(cast[1]["episodes"], 8, "roles 的 episode_count 汇总");
        assert_eq!(crew.len(), 1, "只保留导演/编剧/制片（灯光不在内）");
        assert_eq!(crew[0]["jobLabel"], "导演");

        let movie = serde_json::json!({
            "credits": {
                "cast": [{"id": 3, "name": "演员丙", "character": "主角", "order": 0}],
                "crew": [{"id": 4, "name": "编剧甲", "department": "Writing", "job": "Screenplay"}]
            }
        });
        let (mcast, mcrew) = super::normalize_credits(&movie);
        assert_eq!(mcast.len(), 1);
        assert_eq!(mcast[0]["character"], "主角");
        assert_eq!(mcrew[0]["jobLabel"], "编剧");
    }

    /// 地区分组：多个国家/地区并列返回；没数据的地区（如 CN）自然不出现，不做特判。
    #[test]
    fn providers_and_ratings_are_grouped_by_region_without_cn_special_case() {
        let d = serde_json::json!({
            "watch/providers": { "results": {
                "JP": {"flatrate": [{"provider_name": "Netflix", "display_priority": 1}]},
                "US": {"flatrate": [{"provider_name": "Hulu", "display_priority": 2},
                                    {"provider_name": "Hulu with Ads", "display_priority": 3}]},
                "HK": {"flatrate": [{"provider_name": "Disney+", "display_priority": 1}]},
                "DE": {"rent": [{"provider_name": "Apple TV"}]}
            }},
            "content_ratings": { "results": [
                {"iso_3166_1": "US", "rating": "TV-MA"},
                {"iso_3166_1": "JP", "rating": "TV-14"},
                {"iso_3166_1": "HK", "rating": ""}
            ]}
        });
        let regions = super::providers_by_region(&d);
        let codes: Vec<String> = regions
            .iter()
            .map(|r| r.get("region").and_then(|v| v.as_str()).unwrap_or("").to_string())
            .collect();
        assert_eq!(
            codes,
            vec!["HK".to_string(), "JP".to_string(), "US".to_string()],
            "常见区优先 + 只留有数据的区（DE 只有租买、不出现）"
        );
        assert!(codes.iter().all(|c| c != "CN"), "CN 无数据 → 自然缺省，不特判也不报错");
        let us = regions.iter().find(|r| r["region"] == "US").unwrap();
        assert_eq!(us["items"].as_array().unwrap().len(), 1, "带 Ads 的促销条目滤掉");

        let ratings = super::ratings_by_region(&d, "tv");
        let rcodes: Vec<String> = ratings
            .iter()
            .map(|r| r["region"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(rcodes, vec!["JP".to_string(), "US".to_string()], "HK 的空分级不出现");
        assert_eq!(ratings[0]["rating"], "TV-14");
    }

    /// 边界哨兵（**默认忽略**）：密钥**存在但错误** → 镜像 401 bad_signature = 确定性拒绝，
    /// 必须**原样报错、不降级**（降级改变不了结果，只会掩盖问题）。与
    /// `live_missing_secret_falls_back` 构成一对：缺密钥 → 降级；密钥错 → 报错。
    /// 跑法（Git Bash）：
    ///   FNTV_TMDB_SECRET=definitely-wrong cargo test --lib tmdb::tests::live_wrong_secret_is_fatal -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "live network; run with FNTV_TMDB_SECRET set to a WRONG value"]
    async fn live_wrong_secret_is_fatal() {
        let e = get_json("/3/configuration?language=zh-CN")
            .await
            .expect_err("错误密钥必须原样报错（确定性拒绝，不降级）");
        assert!(
            e.contains("bad_signature") || e.contains("401"),
            "应报签名被拒（401/bad_signature）：{e}"
        );
        assert!(
            !e.contains("降级直连"),
            "错误密钥属于确定性拒绝，不得降级直连（会掩盖问题）：{e}"
        );
    }

    /* ────────── 真机冒烟（**默认忽略**）：语言参数改造 + 演员表主源（锚点定位 + 落盘缓存） ──────────
       跑法（Git Bash）：
         FNTV_TMDB_SECRET=<值> cargo test --lib tmdb::tests::live_language_and_credits_by_anchor -- --ignored --nocapture
       它把本轮改造的**真实链路**一次走通：签名请求（含逗号/中文 query）→ 显式 language=zh-CN 生效
       → include_image_language / include_video_language 让剧照与预告片不为空 → trim_id=tm1399 精确锚点
       → 演员表（aggregate_credits 的 roles/episodes）→ 结果**落盘**且第二次调用命中缓存（不重复请求）。 */
    #[tokio::test]
    #[ignore = "live network + FNTV_TMDB_SECRET required"]
    async fn live_language_and_credits_by_anchor() {
        // 落盘缓存：用**独立临时目录**初始化（别污染真实 app_data），顺手验证「第二次不重复请求」
        let dir = std::env::temp_dir().join("anibox-test-cache");
        let _ = std::fs::remove_dir_all(&dir);
        crate::disk_cache::init(dir.clone(), 64);

        let d = super::get_json(&super::detail_path(
            "tv",
            1399,
            "aggregate_credits,credits,videos,watch/providers,content_ratings,images",
        ))
        .await
        .expect("详情应成功");
        assert_eq!(d.get("id").and_then(|v| v.as_i64()), Some(1399));

        // ① language=zh-CN 生效：中文标题（不传 language 会是英文 "Game of Thrones"）
        let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("");
        println!("detail name={name}");
        assert!(
            name.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "language=zh-CN 应给中文标题：{name}"
        );

        // ② include_video_language=zh-CN,en,ja,null：实测单独用 zh-CN 时 /videos 是 0 条
        let videos = super::videos_of(&d);
        println!("videos={}", videos.len());
        assert!(!videos.is_empty(), "带 include_video_language 后预告片不应为空：{videos:?}");

        // ③ include_image_language=zh-CN,en,null：单独用 zh-CN 会把 images 过滤成 0 条
        let backdrops = d
            .pointer("/images/backdrops")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        println!("backdrops={backdrops}");
        assert!(backdrops > 0, "带 include_image_language 后剧照不应为空");

        let regions = super::providers_by_region(&d);
        let ratings = super::ratings_by_region(&d, "tv");
        println!(
            "providers regions={:?} ratings regions={:?}",
            regions.iter().map(|r| r["region"].clone()).collect::<Vec<_>>(),
            ratings.iter().map(|r| r["region"].clone()).collect::<Vec<_>>()
        );
        assert!(
            !regions.is_empty() || !ratings.is_empty(),
            "权力的游戏应有分级或在线观看数据（按地区并列）"
        );

        // ④ 演员表主源：trim_id=tm1399 精确锚点（不搜索）
        let first = super::tmdb_credits(serde_json::json!({
            "mediaType": "tv", "trimId": "tm1399", "title": "权力的游戏"
        }))
        .await
        .expect("credits 应成功");
        assert_eq!(first["tmdbId"].as_i64(), Some(1399));
        assert_eq!(first["matchedBy"].as_str(), Some("trim_id=tm1399"), "精确锚点不应回落搜索");
        assert_eq!(first["cached"].as_bool(), Some(false), "首拉不是缓存");
        let cast = first["cast"].as_array().cloned().unwrap_or_default();
        println!("cast={} crew={}", cast.len(), first["crew"].as_array().map(|a| a.len()).unwrap_or(0));
        assert!(cast.len() >= 5, "权力的游戏应有多名演员");
        assert!(
            cast[0]["roles"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
            "剧集演员应带 roles[].episode_count：{}",
            cast[0]
        );

        // ⑤ 落盘 + 命中本地不再请求：磁盘上有文件，第二次调用直接吃缓存
        let mut files = 0;
        for e in walkdir(&dir) {
            files += 1;
            let _ = e;
        }
        assert!(files >= 2, "演员表应落盘（.bin + .meta）：{dir:?} 文件数={files}");
        let second = super::tmdb_credits(serde_json::json!({
            "mediaType": "tv", "trimId": "tm1399", "title": "权力的游戏"
        }))
        .await
        .expect("credits 第二次应成功");
        assert_eq!(second["cached"].as_bool(), Some(true), "第二次应命中本地缓存（不重复请求）");
        assert_eq!(second["cast"].as_array().map(|a| a.len()), Some(cast.len()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 测试用：递归列目录（只看文件数，不引 walkdir 依赖）
    fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else { return out };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walkdir(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
