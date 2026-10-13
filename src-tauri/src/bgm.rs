//! Bangumi —— 默认**全线走自建镜像站**（bgm.690075.xyz，兜底
//! fntv-bgm-mirror.122983191.workers.dev），镜像不可用时**降级直连官方域**（无凭证，见「降级」段）。
//! 镜像 worker（Fntv-Plus/bgm-mirror/worker.js）负责把 /v0/* 与 /p1/* 反代到上游两个官方域并在边缘缓存。
//! 上游域名只以**降级直连常量**形式出现（DIRECT_API / DIRECT_NEXT），默认端点列表里没有它们
//! （断言见本文件 tests：default_endpoints_are_mirror_not_upstream）。
//!
//! 准入三件套（与 worker.js verify() 逐字符对齐；**签名实现在 crate::mirror_sign，本文件不再自带**）：
//!   x-fntv-app : fntv-plus
//!   x-fntv-ts  : 当前 Unix 秒（服务端 ±300s 时间窗）
//!   x-fntv-sign: base64( sha256( "fntv-plus" + ts + METHOD + pathWithQuery + sha256hex(body) + secret ) )
//! ⚠️ 内层 body 摘要是 **hex**、外层整体才是 **base64** —— 写反会得到 bad_signature（实测踩过）。
//! pathWithQuery 含前导斜杠与 query，例：`/v0/subjects/8`、`/p1/subjects/8/comments?limit=2&offset=0`。
//!
//! 密钥来源（绝不写死进仓库）：
//!   1) 运行时 env FNTV_BGM_SECRET（dev.cmd 自动从 ../Fntv-Plus/bgm-mirror/.secret.local 读入，仅进程内）
//!   2) 编译期 option_env!("FNTV_BGM_SECRET")
//!   3) 都没有 → **不是错误**：打 WARN 后按「镜像不可用」降级直连官方域。Bangumi 的 API 本身
//!      不需要任何凭证，缺密钥只是少了镜像的缓存/防滥用层，功能不残——绝不能因缺密钥直接失败。
//!
//! 降级（与 tmdb.rs 同一套边界判据）：
//!   **可降级** = 缺密钥（配置错误）/ 连接错误 / 超时 / 5xx / 429 → 直连官方域（无凭证）
//!   **不降级** = 401（bad_signature 等签名错误）与其他 4xx —— 确定性拒绝，降级也改变不了结果
//!   直连路径克制重试：仅瞬时类（网络/超时/429/5xx）最多 2 次尝试、间隔 500ms、单次 8s 超时。
//!   ⚠ 直连的 /p1/* 必须走 **next.bgm.tv**：api.bgm.tv 对 /p1/* 返回 **HTTP 200 但 body 是
//!     `{"code":404,"error":"Not Found"}`** 的业务错误（实测），用错域名会静默拿到空评论列表。
//!
//! 多端点回退：FNTV_BGM_ENDPOINT（逗号分隔，可覆盖默认两址，对齐主仓库 usageStats.ts
//! 的 DEFAULT_ENDPOINT 模式）；连接错误/超时/5xx → 自动切下一个端点；全部不可用 → 降级直连。
//! 每请求 12s（镜像）/ 8s（直连）超时。
//!
//! 落盘缓存（%TEMP%/fntv-bgm-cache/）：条目 / 剧集 / 角色 / 制作人员按 subject_id、
//! 匹配结果按调用方 cache_key（fnOS 条目 guid）。**评论不落盘**（会持续增长，交给镜像边缘缓存）。
//! 排查慢/缓存问题看响应头 x-fntv-cache: HIT|MISS|STALE|BYPASS（这里会写进诊断日志）。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

use crate::mirror_sign;

/// 主址 + 兜底址（顺序即回退顺序）
const DEFAULT_ENDPOINTS: &str =
    "https://bgm.690075.xyz,https://fntv-bgm-mirror.122983191.workers.dev";
const REQ_TIMEOUT_SECS: u64 = 12;

// 降级直连上游（**仅降级路径使用**，默认流量一律走镜像）：
/// Bangumi 官方 API（v0/*）。**免凭证**：不需要 key，直连只是少了镜像的缓存层。
const DIRECT_API: &str = "https://api.bgm.tv";
/// /p1/* 的上游是 next.bgm.tv —— api.bgm.tv 对 /p1/* 会返回 200 + `{"code":404}` 业务错误，勿混。
const DIRECT_NEXT: &str = "https://next.bgm.tv";
/// 直连超时/重试（克制：不把整个请求拖死）
const DIRECT_TIMEOUT_SECS: u64 = 8;
const DIRECT_MAX_ATTEMPTS: usize = 2;
const DIRECT_RETRY_BACKOFF_MS: u64 = 500;

// 缓存 TTL（秒）：匹配结果最稳（30 天）；条目评分/排名会动（6h）；剧集（24h）；角色/制作人员（7 天）
const TTL_MATCH: u64 = 30 * 24 * 3600;
const TTL_SUBJECT: u64 = 6 * 3600;
const TTL_EPISODES: u64 = 24 * 3600;
const TTL_PERSONS: u64 = 7 * 24 * 3600;
/// 每日新番表（/calendar）：[lc-123-rev5] **TTL 3 小时**（原 1 小时）。
/// 依据（用户：「刷给我感觉不需要这么频繁，能不能搞个检测新增才刷新的逻辑」）：新番表是一张静态周表，
/// 一天最多变一两次；到期也**不无脑重拉** —— 带 ETag 条件请求，上游 304 就只顺延时间戳。
/// 另外**跨天必须重取**：缓存键按前端本机日期分桶，且读回来的 payload 还要比一次 `day`
/// （跨过本地零点 ⇒ 键变了 ⇒ 必然重取；万一键撞上也不会把昨天的表当今天用）。
const TTL_CALENDAR: u64 = 3 * 3600;

// ───────────────────────── 基础：密钥 / 端点 / 签名 / HTTP ─────────────────────────
// 签名本体（hex 内层 / base64 外层）与端点解析在 crate::mirror_sign —— 全 crate 唯一实现，
// bgm / tmdb 两个镜像共用，杜绝两份实现漂移。本文件只留「Bangumi 专属」的报错文案。

/// 密钥解析：运行时 env → 编译期 option_env → **None**。
/// None **不再是错误**（不再「缺密钥就直接失败」）：调用方按「镜像不可用」降级直连官方域。
fn secret() -> Option<String> {
    mirror_sign::resolve_secret("FNTV_BGM_SECRET", option_env!("FNTV_BGM_SECRET"))
}

/// 端点列表：FNTV_BGM_ENDPOINT 覆盖（逗号分隔）→ 默认主址+兜底址。
fn endpoints() -> Vec<String> {
    mirror_sign::resolve_endpoints("FNTV_BGM_ENDPOINT", DEFAULT_ENDPOINTS)
}

fn now_secs() -> u64 {
    mirror_sign::now_secs()
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("FntvPlus-Tauri2/0.1")
            .connect_timeout(Duration::from_secs(6))
            .timeout(Duration::from_secs(REQ_TIMEOUT_SECS))
            .build()
            .expect("bgm http client")
    })
}

/// 降级直连 client：8s 超时（弱网宁可报错也不拖着请求）。UA 带仓库地址
/// （Bangumi 要求可识别 UA，别用裸库名）。
fn direct_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("FntvPlus-Tauri2/0.1 (https://github.com/YDMY007/Fntv-Plus)")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(DIRECT_TIMEOUT_SECS))
            .build()
            .expect("bgm direct http client")
    })
}

fn truncate(s: &str, n: usize) -> String {
    mirror_sign::truncate(s, n)
}

/// 401 家族文案：共享映射（密钥为 Bangumi 镜像独立密钥）。
fn auth_error_hint(body_text: &str) -> String {
    mirror_sign::auth_error_hint(body_text, "FNTV_BGM_SECRET")
}

/// 镜像请求：多端点自动回退 + 签名 + 12s 超时。返回解析后的 JSON。
/// **可降级**（缺密钥 / 连接错误 / 超时 / 5xx / 429）→ 直连官方域（免凭证）；
/// **不降级**：401（bad_signature 等签名错误）与其余 4xx —— 确定性拒绝，原样报错。
async fn mirror_call(
    method: &str,
    path_with_query: &str,
    body: Option<&str>,
) -> Result<Value, String> {
    match mirror_call_full(method, path_with_query, body, None).await? {
        crate::tmdb::MirrorGet::Fresh(v, _) => Ok(v),
        crate::tmdb::MirrorGet::NotModified => {
            Err("未带 If-None-Match 却收到 304（镜像行为异常）".to_string())
        }
    }
}

/// [lc-123-rev5] 条件 GET（日历用）：带 `If-None-Match` 时镜像可能回 **304**（无 body）。
/// 结果与 TMDB 侧共用 `crate::tmdb::MirrorGet`（Fresh(内容, ETag) / NotModified）——
/// ETag 存进 disk_cache 的 meta，下次探测复用；304 就**什么都不改**，只顺延本地时间戳。
async fn get_json_cond(path_with_query: &str, if_none_match: Option<&str>) -> Result<crate::tmdb::MirrorGet, String> {
    mirror_call_full("GET", path_with_query, None, if_none_match).await
}

async fn mirror_call_full(
    method: &str,
    path_with_query: &str,
    body: Option<&str>,
    if_none_match: Option<&str>,
) -> Result<crate::tmdb::MirrorGet, String> {
    let body_text = body.unwrap_or("");
    // 缺密钥 = 配置错误 → 按「镜像不可用」降级（Bangumi 直连免凭证）。没有密钥就签不了名，
    // 端点一个都不用试。密钥**存在但错误**会拿到 401 bad_signature → 原样报错，不走这里。
    let Some(secret) = secret() else {
        crate::libmpv::trace(
            "[bgm] WARN 未配置 FNTV_BGM_SECRET（镜像密钥缺失）→ 按镜像不可用处理，直连官方域（免凭证）",
        );
        return direct_call(method, path_with_query, body_text)
            .await
            .map(|v| crate::tmdb::MirrorGet::Fresh(v, None));
    };
    let mut last_err = String::new();
    for ep in endpoints() {
        let url = format!("{ep}{path_with_query}");
        let mut rb = if method == "POST" {
            http().post(&url)
        } else {
            http().get(&url)
        };
        // 准入三件套（ts/签名由共享实现现算，与 URL 上实际发送的 path+query 对齐）
        rb = mirror_sign::apply_auth(rb, &secret, method, path_with_query, body_text)
            .header("accept", "application/json");
        if !body_text.is_empty() {
            rb = rb.header("content-type", "application/json").body(body_text.to_string());
        }
        // [lc-123-rev5] 条件请求：带上次存下的 ETag，没变就拿 304（几字节，省流量也省上游配额）
        if let Some(et) = if_none_match.filter(|e| !e.is_empty()) {
            rb = rb.header("if-none-match", et);
        }
        let resp = match rb.send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("{ep} 连接失败/超时: {e}");
                crate::libmpv::trace(&format!("[bgm] {method} {path_with_query} → {ep} 连接失败: {e}"));
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
            // 条件请求命中：上游说"没变"——连 body 都没有，NotModified 直接上抛给缓存层
            crate::libmpv::trace(&format!(
                "[bgm] {method} {path_with_query} → {ep} 304 Not Modified cache={cache_hdr}"
            ));
            return Ok(crate::tmdb::MirrorGet::NotModified);
        }
        if (200..300).contains(&status) {
            // ETag 必须在 body 被取走前读（json() 会消费 resp）
            let etag = resp
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            let v: Value = resp
                .json()
                .await
                .map_err(|e| format!("{ep} 响应不是合法 JSON: {e}"))?;
            crate::libmpv::trace(&format!(
                "[bgm] {method} {path_with_query} → {ep} {status} cache={cache_hdr} etag={}",
                etag.as_deref().unwrap_or("-")
            ));
            return Ok(crate::tmdb::MirrorGet::Fresh(v, etag));
        }
        let text = resp.text().await.unwrap_or_default();
        crate::libmpv::trace(&format!(
            "[bgm] {method} {path_with_query} → {ep} {status} cache={cache_hdr} body={}",
            truncate(&text, 160)
        ));
        if status == 429 {
            // 上游限流：主址与 workers.dev 是同一 worker，限流状态一致，换端点无意义 → 立即降级
            last_err = format!("{ep} HTTP 429: {}", truncate(&text, 160));
            break;
        }
        if status >= 500 {
            last_err = format!("{ep} HTTP {status}: {}", truncate(&text, 160));
            continue; // 5xx → 切下一个端点
        }
        // 4xx：确定性拒绝（签名/路由/上游 404）——换端点或直连结果相同，**不降级**，原样报错
        return Err(if status == 401 {
            auth_error_hint(&text)
        } else {
            format!("镜像请求失败（HTTP {status}）：{}", truncate(&text, 200))
        });
    }
    // 端点全部不可用（连接错误/超时/5xx）或 429 → 降级直连官方域（免凭证）
    crate::libmpv::trace(&format!("[bgm] 镜像不可用，降级直连（免凭证）：{last_err}"));
    direct_call(method, path_with_query, body_text)
        .await
        .map(|v| crate::tmdb::MirrorGet::Fresh(v, None)) // 降级直连不带条件头
        .map_err(|e| format!("所有镜像端点均失败（最后：{last_err}）；降级直连也失败：{e}"))
}

/// 降级直连 Bangumi 官方域（**免凭证**：Bangumi API 不需要任何 key，镜像只是防滥用+缓存层）。
/// /v0/* → api.bgm.tv；/p1/* → next.bgm.tv（api.bgm.tv 的 /p1/* 返回 200 + 业务 404，勿用）。
/// 克制重试：仅瞬时类（网络/超时/429/5xx）最多 DIRECT_MAX_ATTEMPTS 次、间隔 500ms；其余 4xx 直接报错。
async fn direct_call(method: &str, path_with_query: &str, body_text: &str) -> Result<Value, String> {
    let base = if path_with_query.starts_with("/p1/") {
        DIRECT_NEXT
    } else {
        DIRECT_API
    };
    let url = format!("{base}{path_with_query}");
    let mut last_err = String::new();
    for attempt in 1..=DIRECT_MAX_ATTEMPTS {
        if attempt > 1 {
            tokio::time::sleep(Duration::from_millis(DIRECT_RETRY_BACKOFF_MS)).await;
        }
        let mut rb = if method == "POST" {
            direct_client().post(&url)
        } else {
            direct_client().get(&url)
        };
        rb = rb.header("accept", "application/json");
        if !body_text.is_empty() {
            rb = rb.header("content-type", "application/json").body(body_text.to_string());
        }
        match rb.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if (200..300).contains(&status) {
                    let v: Value = resp
                        .json()
                        .await
                        .map_err(|e| format!("直连响应不是合法 JSON: {e}"))?;
                    crate::libmpv::trace(&format!(
                        "[bgm] 降级直连成功 {method} {path_with_query} → {base} {status} (attempt {attempt})"
                    ));
                    return Ok(v);
                }
                let text = resp.text().await.unwrap_or_default();
                crate::libmpv::trace(&format!(
                    "[bgm] 降级直连 {method} {path_with_query} → {base} {status} body={}",
                    truncate(&text, 160)
                ));
                if status == 429 || status >= 500 {
                    last_err = format!("直连 HTTP {status}: {}", truncate(&text, 160));
                    continue; // 瞬时类 → 再试一次（上限 DIRECT_MAX_ATTEMPTS）
                }
                return Err(format!("直连失败（HTTP {status}）：{}", truncate(&text, 200)));
            }
            Err(e) => {
                last_err = format!("直连网络错误/超时: {e}");
                continue;
            }
        }
    }
    Err(format!(
        "直连 Bangumi 失败（{DIRECT_MAX_ATTEMPTS} 次尝试后）：{last_err}"
    ))
}

async fn get_json(path_with_query: &str) -> Result<Value, String> {
    mirror_call("GET", path_with_query, None).await
}

async fn post_json(path_with_query: &str, body: &Value) -> Result<Value, String> {
    let s = serde_json::to_string(body).map_err(|e| e.to_string())?;
    mirror_call("POST", path_with_query, Some(&s)).await
}

// ───────────────────────── 落盘缓存（%TEMP%/fntv-bgm-cache/） ─────────────────────────

fn cache_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("fntv-bgm-cache")
}

/// 文件名保险：fnOS guid 一般是 uuid，但仍把非 [A-Za-z0-9_-] 字符换成 '_'。
fn safe_key(k: &str) -> String {
    k.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

async fn cache_get(kind: &str, key: &str, ttl: u64) -> Option<Value> {
    let p = cache_dir().join(format!("{kind}_{}.json", safe_key(key)));
    let s = tokio::fs::read_to_string(&p).await.ok()?;
    let v: Value = serde_json::from_str(&s).ok()?;
    let t = v.get("t").and_then(|x| x.as_u64())?;
    if now_secs().saturating_sub(t) > ttl {
        return None;
    }
    let val = v.get("v").cloned();
    if val.is_some() {
        crate::libmpv::trace(&format!("[bgm] cache HIT {kind} {key}"));
    }
    val
}

async fn cache_put(kind: &str, key: &str, val: &Value) {
    let dir = cache_dir();
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        return;
    }
    let p = dir.join(format!("{kind}_{}.json", safe_key(key)));
    let body = json!({ "t": now_secs(), "v": val });
    let _ = tokio::fs::write(&p, body.to_string()).await;
}

// ───────────────────────── 匹配打分（标题相似度 + 年份 + 类型） ─────────────────────────

/// 归一化：全角→半角、转小写、去空白与中英标点（相似度比较只在归一化后做）。
/// `pub(crate)`：tmdb.rs 的标题匹配门槛**复用同一套**归一化（不在两个文件里各写一份、也不会漂移）。
pub(crate) fn norm(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            if ('\u{FF01}'..='\u{FF5E}').contains(&c) {
                // 全角 ASCII（！..～）→ 半角
                return char::from_u32(c as u32 - 0xFEE0).map(|x| x.to_ascii_lowercase());
            }
            if c.is_whitespace() || c.is_ascii_punctuation() {
                return None;
            }
            if matches!(
                c,
                '，' | '。'
                    | '、'
                    | '；'
                    | '：'
                    | '！'
                    | '？'
                    | '～'
                    | '·'
                    | '“'
                    | '”'
                    | '‘'
                    | '’'
                    | '（'
                    | '）'
                    | '【'
                    | '】'
                    | '《'
                    | '》'
                    | '〈'
                    | '〉'
                    | '「'
                    | '」'
                    | '─'
                    | '—'
                    | '−'
                    | '－'
                    | '｀'
                    | '\u{3000}'
            ) {
                return None;
            }
            Some(c.to_lowercase().next().unwrap_or(c))
        })
        .collect()
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// 字符二元组 Dice 系数（中文无空格，dice 比编辑距离更宽容语序）。
fn dice(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    if av.len() < 2 || bv.len() < 2 {
        return 0.0;
    }
    let mut m: HashMap<(char, char), i32> = HashMap::new();
    for w in av.windows(2) {
        *m.entry((w[0], w[1])).or_insert(0) += 1;
    }
    let mut hit = 0usize;
    for w in bv.windows(2) {
        if let Some(c) = m.get_mut(&(w[0], w[1])) {
            if *c > 0 {
                *c -= 1;
                hit += 1;
            }
        }
    }
    2.0 * hit as f64 / (av.len() - 1 + bv.len() - 1) as f64
}

/// 标题相似度：归一化后 全等=1 / 包含=0.72+0.28×短长比 / 其余=dice。
/// `pub(crate)`：tmdb.rs 复用（TMDB 搜索的"第一条即最佳"必须补门槛，用的是同一个相似度）。
pub(crate) fn title_sim(q: &str, cand: &str) -> f64 {
    if q.is_empty() || cand.is_empty() {
        return 0.0;
    }
    if q == cand {
        return 1.0;
    }
    if cand.contains(q) || q.contains(cand) {
        let (long, short) = if char_count(q) >= char_count(cand) {
            (q, cand)
        } else {
            (cand, q)
        };
        return 0.72 + 0.28 * (char_count(short) as f64 / char_count(long) as f64);
    }
    dice(q, cand)
}

/// 年份吻合度（0 / 0.45 / 1；任一侧缺年份给中性 0.5）。
/// `pub(crate)`：tmdb.rs 的相似度门槛复用（同一套"差 1 年给半分"的口径）。
pub(crate) fn year_score(want: Option<i32>, got: Option<i32>) -> f64 {
    match (want, got) {
        (Some(w), Some(g)) if w == g => 1.0,
        (Some(w), Some(g)) if (w - g).abs() == 1 => 0.45,
        (Some(_), Some(_)) => 0.0,
        _ => 0.5, // 任一侧缺年份 → 中性，不奖不罚
    }
}

fn type_score(media_type: Option<&str>, platform: &str) -> f64 {
    let Some(t) = media_type.map(|s| s.trim().to_ascii_lowercase()) else {
        return 0.5;
    };
    if t.is_empty() {
        return 0.5;
    }
    let p = platform.trim();
    if p.is_empty() {
        return 0.5;
    }
    let pl = p.to_ascii_lowercase();
    let movie_plat = p.contains("剧场") || p.contains("映画") || p.contains("电影") || pl.contains("movie");
    let tv_plat = pl == "tv" || pl.contains("web") || p.contains("电视");
    let is_movie = t == "movie";
    // 类型冲突（剧 ↔ 剧场版）**不给分**：同标题的剧场版与 TV 并存时，年份偶然对上也不该抢走
    // TV 条目（提取函数做过反例测试：冲突给 0.2 时 TV 项会被同年的剧场版反超）。
    if is_movie {
        if movie_plat {
            1.0
        } else if tv_plat {
            0.0
        } else {
            0.5
        }
    } else if tv_plat {
        1.0
    } else if movie_plat {
        0.0
    } else {
        0.5
    }
}

fn year_of(date: &str) -> Option<i32> {
    date.get(0..4).and_then(|y| y.parse::<i32>().ok())
}

/// 搜不到时的工作标题：去掉「第 N 季/部/期」之后的部分再搜一次（季标题长、条目名短是常态）。
fn strip_season_suffix(q: &str) -> Option<String> {
    let idx = q.find('第')?;
    let head = q[..idx].trim_end_matches([' ', '(', '（', '·', '-', '_', '　']).trim();
    if char_count(head) >= 2 && head != q {
        Some(head.to_string())
    } else {
        None
    }
}

/// 从搜索结果里挑选最佳条目：标题相似度 + 年份 + 类型加权。
/// 权重：有年份 0.65/0.25/0.10（标题/年份/类型）；无年份 0.85/0.15（标题/类型）。
/// 门槛：标题相似度 ≥ 0.45 且总分 ≥ 0.50，否则视为「匹配失败」（宁可不匹配，不给错条目）。
fn pick_best(
    items: &[Value],
    query: &str,
    want_year: Option<i32>,
    media_type: Option<&str>,
) -> Option<(f64, Value, String)> {
    const MIN_TITLE_SIM: f64 = 0.45;
    const MIN_TOTAL: f64 = 0.50;
    let qn = norm(query);
    let mut best: Option<(f64, Value, String)> = None;
    for it in items {
        let name = it.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let name_cn = it.get("name_cn").and_then(|v| v.as_str()).unwrap_or("");
        let sim_cn = title_sim(&qn, &norm(name_cn));
        let sim_ja = title_sim(&qn, &norm(name));
        let (sim, which) = if sim_cn >= sim_ja {
            (sim_cn, "name_cn")
        } else {
            (sim_ja, "name")
        };
        if sim < MIN_TITLE_SIM {
            continue;
        }
        let date = it.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let got_year = year_of(date);
        let ys = year_score(want_year, got_year);
        let platform = it.get("platform").and_then(|v| v.as_str()).unwrap_or("");
        let ts = type_score(media_type, platform);
        let total = if want_year.is_some() {
            0.65 * sim + 0.25 * ys + 0.10 * ts
        } else {
            0.85 * sim + 0.15 * ts
        };
        if total < MIN_TOTAL {
            continue;
        }
        let matched_by = format!(
            "title={sim:.2}({which}) year={}/{} platform={} total={total:.2}",
            want_year.map(|y| y.to_string()).unwrap_or_else(|| "—".into()),
            got_year.map(|y| y.to_string()).unwrap_or_else(|| "—".into()),
            if platform.is_empty() { "—" } else { platform },
        );
        if best.as_ref().map(|(b, _, _)| total > *b).unwrap_or(true) {
            best = Some((total, it.clone(), matched_by));
        }
    }
    best
}

// ───────────────────────── Tauri 命令 ─────────────────────────

/// 匹配：POST /v0/search/subjects（filter.type=[2] 动画），打分选最佳。
/// 结果按 cache_key（调用方传 fnOS 条目 guid）落盘缓存 30 天，避免每次进详情页都搜。
/// 返回 { subject_id, name, name_cn, date, score, rank, images, matched_by }。
#[tauri::command]
pub async fn bgm_match(
    query: String,
    year: Option<String>,
    media_type: Option<String>,
    cache_key: Option<String>,
) -> Result<Value, String> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Err("匹配标题为空".into());
    }
    let ck = cache_key.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(ck) = ck {
        if let Some(v) = cache_get("match", ck, TTL_MATCH).await {
            return Ok(v);
        }
    }

    let mut resp = post_json("/v0/search/subjects", &json!({ "keyword": q, "filter": { "type": [2] } })).await?;
    let mut items: Vec<Value> = resp
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        if let Some(alt) = strip_season_suffix(&q) {
            crate::libmpv::trace(&format!("[bgm] 搜索无结果，去季标题重试: {alt}"));
            resp = post_json(
                "/v0/search/subjects",
                &json!({ "keyword": alt, "filter": { "type": [2] } }),
            )
            .await?;
            items = resp
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
        }
    }

    let want_year = year.as_deref().and_then(|y| y.trim().get(0..4)).and_then(|y| y.parse::<i32>().ok());
    let mt = media_type.as_deref();
    let picked = pick_best(&items, &q, want_year, mt).ok_or_else(|| {
        format!("Bangumi 未找到与「{q}」足够相似的条目（候选 {} 条）", items.len())
    })?;
    let (_, item, matched_by) = picked;
    crate::libmpv::trace(&format!("[bgm] match 「{q}」 → {} ({matched_by})", item.get("id").and_then(|v| v.as_i64()).unwrap_or(0)));

    let out = json!({
        "subject_id": item.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
        "name": item.get("name").and_then(|v| v.as_str()).unwrap_or(""),
        "name_cn": item.get("name_cn").and_then(|v| v.as_str()).unwrap_or(""),
        "date": item.get("date").and_then(|v| v.as_str()).unwrap_or(""),
        "score": item.get("score").cloned().unwrap_or(Value::Null),
        "rank": item.get("rank").cloned().unwrap_or(Value::Null),
        "images": item.get("images").cloned().unwrap_or(Value::Null),
        "matched_by": matched_by,
    });
    if let Some(ck) = ck {
        cache_put("match", ck, &out).await;
    }
    Ok(out)
}

/// 条目详情：GET /v0/subjects/{id}（按 subject_id 落盘，TTL 6h）。
#[tauri::command]
pub async fn bgm_subject(id: i64) -> Result<Value, String> {
    let key = id.to_string();
    if let Some(v) = cache_get("subject", &key, TTL_SUBJECT).await {
        return Ok(v);
    }
    let v = get_json(&format!("/v0/subjects/{id}")).await?;
    cache_put("subject", &key, &v).await;
    Ok(v)
}

/// 剧集列表：GET /v0/episodes?subject_id={id}&limit={n}（响应自带 name_cn/desc/duration）。
/// limit 默认 100、夹在 1..=200；缓存键含 limit（不同 limit 的响应不同）。
#[tauri::command]
pub async fn bgm_episodes(id: i64, limit: Option<i64>) -> Result<Value, String> {
    let n = limit.unwrap_or(100).clamp(1, 200);
    let key = format!("{id}_{n}");
    if let Some(v) = cache_get("episodes", &key, TTL_EPISODES).await {
        return Ok(v);
    }
    let v = get_json(&format!("/v0/episodes?subject_id={id}&limit={n}")).await?;
    cache_put("episodes", &key, &v).await;
    Ok(v)
}

/// 角色：GET /v0/subjects/{id}/characters（TTL 7 天）。
#[tauri::command]
pub async fn bgm_characters(id: i64) -> Result<Value, String> {
    let key = id.to_string();
    if let Some(v) = cache_get("chars", &key, TTL_PERSONS).await {
        return Ok(v);
    }
    let v = get_json(&format!("/v0/subjects/{id}/characters")).await?;
    cache_put("chars", &key, &v).await;
    Ok(v)
}

/// 制作人员：GET /v0/subjects/{id}/persons（TTL 7 天）。
#[tauri::command]
pub async fn bgm_persons(id: i64) -> Result<Value, String> {
    let key = id.to_string();
    if let Some(v) = cache_get("persons", &key, TTL_PERSONS).await {
        return Ok(v);
    }
    let v = get_json(&format!("/v0/subjects/{id}/persons")).await?;
    cache_put("persons", &key, &v).await;
    Ok(v)
}

/// 关联条目：GET /v0/subjects/{id}/subjects（**裸数组**，TTL 7 天）。
/// 上游每条 = `{id, type, name, name_cn, relation, images}`；`relation` 是中文关系词
/// （实测 55 条 / 16 种：片头曲 4、片尾曲 4、原声集 2、游戏 14、画集 7、广播剧 7、番外篇 2 …），
/// 顺序即上游原序（热度序），**这里不分组不截断** —— 分组交给前端（改版式不必重发 Rust）。
/// 注意上游 `images` 可能是**空对象**（实测有），前端按空串兜底。
#[tauri::command]
pub async fn bgm_related(id: i64) -> Result<Value, String> {
    let key = id.to_string();
    if let Some(v) = cache_get("related", &key, TTL_PERSONS).await {
        return Ok(v);
    }
    let v = get_json(&format!("/v0/subjects/{id}/subjects")).await?;
    cache_put("related", &key, &v).await;
    Ok(v)
}

/// 吐槽（评论）：GET /p1/subjects/{id}/comments?limit={l}&offset={o}。
/// **不落盘缓存**：评论会持续增长，交给镜像边缘缓存（x-fntv-cache 头可观察）。
/// 分页交给前端：offset += **实际返回条数**（不假定 limit 生效）。
#[tauri::command]
pub async fn bgm_comments(id: i64, offset: Option<i64>, limit: Option<i64>) -> Result<Value, String> {
    let l = limit.unwrap_or(20).clamp(1, 50);
    let o = offset.unwrap_or(0).max(0);
    get_json(&format!("/p1/subjects/{id}/comments?limit={l}&offset={o}")).await
}

/// [lc-123] 每日新番表（发现页右栏）：GET /calendar —— **裸数组**，7 组
/// `{ weekday: {en,cn,ja,id}, items: [...] }`（不是 /v0 的 {data,total} 包装，别按 /v0 解析）。
/// 上游条目字段（**实测** 7 组 / 105 条，2026-10-12 走镜像核对）：id/url/type/name/name_cn/summary/
/// air_date/air_weekday/rating{total,count,score}/images{large,common,medium,small,grid}/
/// collection{doing}/rank —— 归一化只留渲染需要的（`type` 留着：以后要区分动画/三次元不必再改 Rust）。
/// ⚠ 实测的三条空字段（都已在归一化里兜住，前端也有回落）：
///   · **没有集号、没有播出时刻**（`ep`/`time` 这类字段根本不存在）⇒ 前端**不显示**「第 N 话 / 今晚几点」，
///     那两列无数据可依（要显示只能对每条再打一次 /v0/subjects/{id}，105 条全打不值得）。
///   · `summary` **105/105 全空** ⇒ 简介只能靠 bgm_subject（详情弹层就是这么做的）。
///   · `name_cn` 19/105 为空、`rating.score` 17 条为 null ⇒ 标题回落 `name`、评分容忍 null。
/// [lc-123-rev5] 落盘缓存改用 **disk_cache 的 json 层**（与 TMDB 同一套：TTL + ETag + LRU）：
///   · TTL_CALENDAR（3 小时）；**跨天必重取**（缓存键按 `arg.day` 分桶 + 读回来还要比 `payload.day`）；
///   · 到期不无脑重拉：带 `If-None-Match` 条件请求，304 → 只顺延时间戳（前端零变化）；
///   · `force`（手动 ⟳）跳过本地 TTL 检查但**仍带条件头**；`no_etag`（失败重试）才强制全量。
///   · 返回带 `changed`：false = 命中缓存或 304（**前端不许碰 UI**）。
/// arg.day = 前端本机日期（YYYY-MM-DD）：本地零点一过，键就变了 ⇒ 必然重取（不会把昨天的表当今天用）。
#[tauri::command]
pub async fn bgm_calendar(
    force: Option<bool>,
    no_etag: Option<bool>,
    day: Option<String>,
) -> Result<Value, String> {
    let force = force == Some(true);
    let no_etag = no_etag == Some(true);
    let day = day.unwrap_or_default();
    let key = format!("json:bgm:calendar:v1|{day}");
    let day_check = day.clone();
    let (payload, changed) = crate::disk_cache::json_cached_conditional(
        &key,
        TTL_CALENDAR,
        force,
        no_etag,
        // 跨天保险：payload 记着自己属于哪一天；对不上就当本地没有（键撞车也不会用错天的表）
        move |v| v.get("day").and_then(|d| d.as_str()) == Some(day_check.as_str()),
        |etag| async move {
            match get_json_cond("/calendar", etag.as_deref()).await? {
                crate::tmdb::MirrorGet::NotModified => Ok(crate::disk_cache::Cond::NotModified),
                crate::tmdb::MirrorGet::Fresh(raw, et) => {
                    let days = normalize_calendar(&raw);
                    if days.iter().all(|d| d.get("count").and_then(|c| c.as_u64()).unwrap_or(0) == 0) {
                        // 空表当作失败：上游改结构时宁可报错（上一份可用缓存留着），
                        // 也不给前端一个"7 天全空"的假象
                        return Err(format!(
                            "Bangumi 每日新番表为空或结构不符（顶层应为 7 组数组）：{}",
                            truncate(&raw.to_string(), 160)
                        ));
                    }
                    let total: u64 = days
                        .iter()
                        .map(|g| g.get("count").and_then(|c| c.as_u64()).unwrap_or(0))
                        .sum();
                    crate::libmpv::trace(&format!(
                        "[bgm] calendar → {} 组 / {total} 部（day={day}）",
                        days.len()
                    ));
                    Ok(crate::disk_cache::Cond::Fresh(
                        json!({ "day": day, "cachedAt": now_secs(), "days": days }),
                        et,
                    ))
                }
            }
        },
    )
    .await?;

    let mut out = payload;
    if let Value::Object(ref mut m) = out {
        m.insert("changed".to_string(), json!(changed));
    }
    Ok(out)
}

/// /calendar 归一化：7 组原样保留 `weekday`（en/cn/ja/id），items 只留渲染字段。
/// `cover` 取 medium（列表用 46×62 小图，medium 足够）→ 逐级回退；上游给的是 **http://lain.bgm.tv**
/// （日历端点是 http 明链），前端 bgmImg() 会把 lain.bgm.tv 升到 https（混合内容会整片裂图）。
fn normalize_calendar(raw: &Value) -> Vec<Value> {
    raw.as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|g| {
                    let wd = g.get("weekday")?;
                    let txt = |k: &str| wd.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let items: Vec<Value> = g
                        .get("items")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|it| {
                                    let id = it.get("id").and_then(|v| v.as_i64())?;
                                    let s = |k: &str| {
                                        it.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
                                    };
                                    let cover = ["medium", "common", "large", "grid", "small"]
                                        .iter()
                                        .find_map(|k| {
                                            it.pointer(&format!("/images/{k}"))
                                                .and_then(|v| v.as_str())
                                                .filter(|s| !s.is_empty())
                                        })
                                        .unwrap_or("")
                                        .to_string();
                                    Some(json!({
                                        "id": id,
                                        "type": it.get("type").and_then(|v| v.as_i64()).unwrap_or(2),
                                        "name": s("name"),
                                        "name_cn": s("name_cn"),
                                        "air_date": s("air_date"),
                                        "air_weekday": it.get("air_weekday").and_then(|v| v.as_i64()),
                                        "score": it.pointer("/rating/score").cloned().unwrap_or(Value::Null),
                                        "votes": it.pointer("/rating/total").and_then(|v| v.as_i64()).unwrap_or(0),
                                        "doing": it.pointer("/collection/doing").and_then(|v| v.as_i64()).unwrap_or(0),
                                        "cover": cover,
                                    }))
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    Some(json!({
                        "id": wd.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
                        "cn": txt("cn"),
                        "en": txt("en"),
                        "ja": txt("ja"),
                        "count": items.len(),
                        "items": items,
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{direct_call, DEFAULT_ENDPOINTS, DIRECT_API, DIRECT_NEXT};

    /// 静态断言（不触网）：默认路径必须是**镜像**（上游域名只许出现在降级直连常量里）。
    #[test]
    fn default_endpoints_are_mirror_not_upstream() {
        let eps =
            crate::mirror_sign::resolve_endpoints("FNTV_BGM_ENDPOINT_UNSET_XYZ", DEFAULT_ENDPOINTS);
        assert_eq!(eps[0], "https://bgm.690075.xyz");
        assert!(eps.iter().all(|e| !e.contains("bgm.tv")));
        assert!(!DEFAULT_ENDPOINTS.contains(DIRECT_API));
        assert!(!DEFAULT_ENDPOINTS.contains(DIRECT_NEXT));
    }

    /// 镜像**主路径**冒烟（**默认忽略**，不进 CI）：带真实 FNTV_BGM_SECRET 直打 bgm 镜像，
    /// 验证「共享签名 + 请求构造（GET，以及**带 body 的 POST**）+ JSON 解析」整链成立。
    /// 与 live_direct_call_is_credential_free 互补：那条证明降级可用，这条确保主路径没被改坏
    /// （缺密钥降级改动最容易的回归就是「把主路径也顺手改坏」）。跑法：
    ///   FNTV_BGM_SECRET=<值> cargo test --lib bgm::tests::live_mirror_get_and_post_signed -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "live network + FNTV_BGM_SECRET required"]
    async fn live_mirror_get_and_post_signed() {
        let v = super::get_json("/v0/subjects/8").await.expect("镜像 GET 应成功");
        assert_eq!(v.get("id").and_then(|x| x.as_i64()), Some(8));

        // POST 会签 body 摘要（内层 hex）——GET 通了不代表 POST 通，必须单独覆盖
        let s = super::post_json(
            "/v0/search/subjects",
            &serde_json::json!({"keyword": "命运石之门", "filter": {"type": [2]}}),
        )
        .await
        .expect("镜像 POST 应成功");
        assert!(
            s.get("data")
                .and_then(|d| d.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "镜像 POST 搜索结果应非空"
        );
    }

    /// 降级直连冒烟（**默认忽略**，不进 CI）：证明「直连免凭证」+ **/p1/* 必须走 next.bgm.tv**。
    /// 手动跑：cargo test --lib bgm::tests::live_direct_call -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "live network"]
    async fn live_direct_call_is_credential_free() {
        // /v0/* → api.bgm.tv
        let v = direct_call("GET", "/v0/subjects/8", "")
            .await
            .expect("直连 GET /v0/subjects/8 应成功（免凭证）");
        assert_eq!(v.get("id").and_then(|x| x.as_i64()), Some(8));

        // /p1/* → next.bgm.tv。⚠ 这条是防「打到 api.bgm.tv 拿到 HTTP 200 + {"code":404} 空壳」的哨兵。
        let c = direct_call("GET", "/p1/subjects/8/comments?limit=2&offset=0", "")
            .await
            .expect("直连 GET /p1/... 应成功");
        assert!(
            c.get("data")
                .and_then(|d| d.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "评论应非空；若为空且含 code=404，说明 /p1/* 错打到了 api.bgm.tv"
        );

        // POST 搜索同样免凭证
        let s = direct_call(
            "POST",
            "/v0/search/subjects",
            "{\"keyword\":\"命运石之门\",\"filter\":{\"type\":[2]}}",
        )
        .await
        .expect("直连 POST /v0/search/subjects 应成功");
        assert!(
            s.get("data")
                .and_then(|d| d.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false)
        );
    }

    /// 缺密钥降级（**默认忽略**）：**必须先把 FNTV_BGM_SECRET 置空**再跑，验证不再「缺密钥就直接失败」，
    /// 而是按「镜像不可用」直连官方域（Bangumi 免凭证）。跑法（Git Bash）：
    ///   FNTV_BGM_SECRET=" " cargo test --lib bgm::tests::live_missing_secret_uses_direct -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "live network; run with FNTV_BGM_SECRET blanked"]
    async fn live_missing_secret_uses_direct() {
        let v = super::get_json("/v0/subjects/8")
            .await
            .expect("缺密钥时应降级直连（而不是 Fatal）");
        assert_eq!(v.get("id").and_then(|x| x.as_i64()), Some(8));
    }

    /// 边界哨兵（**默认忽略**）：密钥**存在但错误** → 镜像 401 bad_signature = 确定性拒绝，
    /// 必须**原样报错、不降级**。与 `live_missing_secret_uses_direct` 构成一对。跑法（Git Bash）：
    ///   FNTV_BGM_SECRET=definitely-wrong cargo test --lib bgm::tests::live_wrong_secret_is_fatal -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "live network; run with FNTV_BGM_SECRET set to a WRONG value"]
    async fn live_wrong_secret_is_fatal() {
        let e = super::get_json("/v0/subjects/8")
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

    /// /calendar 归一化（**不触网**）：喂一份上游真实形状的样本（裸数组 7 组 + 真实字段名），
    /// 钉住三件事：① 顶层是**数组**（按 /v0 的 {data} 解析会得到 0 组）② cover 逐级回退
    /// （medium 缺 → common）③ rating/collection 缺字段时给 null/0 而不是 panic。
    /// 真实返回结构见报告：`[{weekday:{en,cn,ja,id}, items:[{id,name,name_cn,air_date,air_weekday,
    /// rating:{total,count,score}, images:{...}, collection:{doing}}]}]`
    #[test]
    fn normalize_calendar_handles_upstream_shape() {
        let raw = serde_json::json!([
            { "weekday": { "en": "Mon", "cn": "星期一", "ja": "月耀日", "id": 1 },
              "items": [
                { "id": 390200, "type": 2, "name": "列女战纪", "name_cn": "列女战纪：女娲石记",
                  "air_date": "2026-10-05", "air_weekday": 1,
                  "rating": { "total": 6, "count": { "1": 3 }, "score": 3.3 },
                  "images": { "medium": "http://lain.bgm.tv/pic/cover/m/1c/b4/390200.jpg",
                              "common": "http://lain.bgm.tv/pic/cover/c/1c/b4/390200.jpg" },
                  "collection": { "doing": 5 } },
                // 缺 images.medium / rating / collection → 只能靠回退与默认值活下来
                { "id": 418229, "type": 2, "name": "魔法少女养成计划 restart",
                  "images": { "common": "http://lain.bgm.tv/pic/cover/c/x.jpg" } }
              ] },
            { "weekday": { "en": "Tue", "cn": "星期二", "ja": "火耀日", "id": 2 }, "items": [] }
        ]);
        let days = super::normalize_calendar(&raw);
        assert_eq!(days.len(), 2, "应保留 2 组（组数由上游给，不要补齐成 7 组）");
        assert_eq!(days[0]["cn"].as_str(), Some("星期一"));
        assert_eq!(days[0]["id"].as_i64(), Some(1));
        assert_eq!(days[0]["count"].as_u64(), Some(2), "count = 该组条目数");
        assert_eq!(days[1]["count"].as_u64(), Some(0));
        assert_eq!(days[0]["items"][0]["id"].as_i64(), Some(390200));
        assert_eq!(days[0]["items"][0]["score"].as_f64(), Some(3.3));
        assert_eq!(days[0]["items"][0]["votes"].as_i64(), Some(6));
        assert_eq!(days[0]["items"][0]["doing"].as_i64(), Some(5));
        assert!(days[0]["items"][0]["cover"].as_str().unwrap().contains("/m/"), "优先 medium");
        assert_eq!(days[0]["items"][1]["cover"].as_str().unwrap().contains("/c/"), true, "medium 缺 → 回退 common");
        assert_eq!(days[0]["items"][1]["score"], serde_json::Value::Null, "缺 rating → null（不是 0.0）");
        assert_eq!(days[0]["items"][1]["votes"].as_i64(), Some(0));
        assert_eq!(days[0]["items"][1]["air_date"].as_str(), Some(""), "缺日期 → 空串");
        // 顶层不是数组（比如误按 /v0 的 {data:[...]} 解析）→ 0 组，由调用方当失败处理
        assert!(super::normalize_calendar(&serde_json::json!({ "data": [] })).is_empty());
    }

    /// 真机冒烟（**默认忽略**）：走真镜像拉 /calendar（本条同时验证第 0 步新加的路由与签名）。
    /// 跑法：FNTV_BGM_SECRET=<值> cargo test --lib bgm::tests::live_calendar -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "live network + FNTV_BGM_SECRET required"]
    async fn live_calendar_seven_groups() {
        let v = super::bgm_calendar(Some(true), None, Some("2026-10-13".to_string()))
            .await
            .expect("镜像 /calendar 应成功");
        let days = v.get("days").and_then(|d| d.as_array()).expect("应含 days 数组");
        assert_eq!(days.len(), 7, "应恰好 7 组（周一..周日）");
        let total: u64 = days
            .iter()
            .map(|g| g.get("count").and_then(|c| c.as_u64()).unwrap_or(0))
            .sum();
        println!("calendar: 7 组 / {total} 部");
        assert!(total > 0, "总条目数应为正");
        assert!(days[0].get("items").and_then(|i| i.as_array()).is_some(), "每组应带 items");
    }
}
