//! [danmaku-diagnose] 自建弹幕源手动诊断。
//!
//! 目标：定位「服务正常返回 JSON 但搜不到弹幕」这类问题。一次命令返回全部检查项，
//! 前端只负责展示。每项独立、互不阻塞；网络请求统一 5s 超时，失败只给可读原因，
//! **绝不打印 Cookie / 密钥**（地址只保留 host + `/api/...` 尾段，复用 `client::safe_url`）。
//!
//! 检查项：
//!   1. Node 运行时（是否找到 / 版本 / 路径）
//!   2. 自建源服务（base 是否可达 + `/api/v2/search/anime` 的 HTTP 状态/耗时/条目数/前几条标题）
//!   3. 业务匹配（复用 `matching` 的 normalize + 主名/季号多级匹配，判断是否会被拒收）
//!   4. shim（22347 是否监听 + `/health`）
//!   5. B 站 Cookie（复用 `bili_account_status`）
//!   6. 当前播放上下文（play-meta 里的标题/季/集）

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tauri::{AppHandle, State};

use crate::AppState;
use crate::danmaku::{client, effective_bases, matching};

/// 单项检查的网络超时上限（避免界面卡死）。
const ITEM_TIMEOUT: Duration = Duration::from_secs(5);
/// 取不到播放标题时使用的内置测试词。
const BUILTIN_KEYWORD: &str = "测试";

/// 播放上下文（由 `write_play_meta` 写入 `%TEMP%/anibox-play-meta.json`）。
#[derive(Default)]
struct PlayCtx {
    exists: bool,
    title: String,
    season: i64,
    episode: i64,
}

fn read_play_ctx() -> PlayCtx {
    let path = std::env::temp_dir().join("anibox-play-meta.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return PlayCtx::default();
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return PlayCtx::default();
    };
    PlayCtx {
        exists: true,
        title: v
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        season: v.get("season").and_then(|x| x.as_i64()).unwrap_or(0),
        episode: v.get("episode").and_then(|x| x.as_i64()).unwrap_or(0),
    }
}

/// 关键词来源：用户输入 > 当前播放标题 > 内置测试词。
fn resolve_keyword(input: Option<String>, ctx: &PlayCtx) -> (String, &'static str) {
    if let Some(k) = input {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return (k, "custom");
        }
    }
    if !ctx.title.is_empty() {
        return (ctx.title.clone(), "playing");
    }
    (BUILTIN_KEYWORD.to_string(), "builtin")
}

/// 组装单项结果（统一字段：id/label/status/ok/detail/timeMs）。
fn item(id: &str, label: &str, status: &str, detail: impl Into<String>, t: Instant) -> Value {
    json!({
        "id": id,
        "label": label,
        "status": status,
        "ok": status != "fail",
        "detail": detail.into(),
        "timeMs": t.elapsed().as_millis() as u64,
    })
}

fn truncate(s: &str, n: usize) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
    if cleaned.chars().count() <= n {
        cleaned
    } else {
        let mut out: String = cleaned.chars().take(n).collect();
        out.push('…');
        out
    }
}

fn describe_reqwest(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "请求超时".into()
    } else if e.is_connect() {
        "连接失败（服务未启动或端口不通）".into()
    } else {
        truncate(&e.to_string(), 120)
    }
}

/// 单个关键词的搜索探测结果。
struct Probe {
    keyword: String,
    /// 收到 HTTP 响应时的状态码；`None` = 所有地址传输层失败。
    http_status: Option<u16>,
    host: String,
    elapsed_ms: u64,
    animes: Vec<Value>,
    error: Option<String>,
}

impl Probe {
    fn unreachable(keyword: &str, err: String) -> Self {
        Probe {
            keyword: keyword.to_string(),
            http_status: None,
            host: String::new(),
            elapsed_ms: 0,
            animes: Vec::new(),
            error: Some(err),
        }
    }
}

/// 依次尝试各 base，首个返回 HTTP 响应的地址即为结果（与客户端故障转移同口径）。
async fn probe_search(bases: &[String], keyword: &str, deadline: Instant) -> Probe {
    if bases.is_empty() {
        return Probe::unreachable(keyword, "未配置自建源地址".into());
    }
    let client = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => return Probe::unreachable(keyword, format!("构造请求失败: {e}")),
    };
    let path = format!(
        "/api/v2/search/anime?keyword={}",
        urlencoding::encode(keyword)
    );
    let mut errs: Vec<String> = Vec::new();
    for base in bases {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            errs.push("诊断总时长已用完".into());
            break;
        }
        let url = client::join_url(base, &path);
        let host = client::host_of(base);
        let started = Instant::now();
        match client
            .get(&url)
            .timeout(remain.min(ITEM_TIMEOUT))
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                let elapsed = started.elapsed().as_millis() as u64;
                if status >= 400 {
                    return Probe {
                        keyword: keyword.to_string(),
                        http_status: Some(status),
                        host,
                        elapsed_ms: elapsed,
                        animes: Vec::new(),
                        error: Some(format!("HTTP {status}")),
                    };
                }
                match serde_json::from_str::<Value>(&body) {
                    Ok(v) => {
                        let animes = v
                            .get("animes")
                            .and_then(|x| x.as_array())
                            .cloned()
                            .unwrap_or_default();
                        let success_false =
                            v.get("success").and_then(|x| x.as_bool()) == Some(false);
                        return Probe {
                            keyword: keyword.to_string(),
                            http_status: Some(status),
                            host,
                            elapsed_ms: elapsed,
                            animes,
                            error: if success_false {
                                Some("服务返回 success=false".into())
                            } else {
                                None
                            },
                        };
                    }
                    Err(e) => {
                        return Probe {
                            keyword: keyword.to_string(),
                            http_status: Some(status),
                            host,
                            elapsed_ms: elapsed,
                            animes: Vec::new(),
                            error: Some(format!(
                                "返回不是合法 JSON：{}",
                                truncate(&e.to_string(), 80)
                            )),
                        };
                    }
                }
            }
            Err(e) => {
                errs.push(format!("{host} {}", describe_reqwest(&e)));
            }
        }
    }
    Probe::unreachable(keyword, format!("地址全部不可达 —— {}", errs.join("；")))
}

/// 检查 1：Node 运行时。
async fn check_node(app: &AppHandle) -> Value {
    let t = Instant::now();
    match crate::danmaku::builtin::resolve_node(app) {
        Ok(p) => {
            let p2 = p.clone();
            let version = tokio::time::timeout(
                ITEM_TIMEOUT,
                tokio::task::spawn_blocking(move || crate::bili_account::node_version(&p2)),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();
            let path = crate::bili_account::display_node_path(&p);
            if version.is_empty() {
                item(
                    "node",
                    "Node 运行时",
                    "fail",
                    format!("找到 Node（{path}）但无法执行 --version"),
                    t,
                )
            } else {
                item(
                    "node",
                    "Node 运行时",
                    "ok",
                    format!("{version}（{path}）"),
                    t,
                )
            }
        }
        Err(e) => item("node", "Node 运行时", "fail", e, t),
    }
}

fn anime_title(v: &Value) -> String {
    v.get("animeTitle")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn anime_id(v: &Value) -> i64 {
    v.get("animeId").and_then(|x| x.as_i64()).unwrap_or(0)
}

fn tier_label(tier: u8) -> &'static str {
    match tier {
        matching::SEASON_TIER_EXACT => "主名+季号精确",
        matching::SEASON_TIER_IMPLIED_FIRST => "播放侧无季号，候选标第一季",
        matching::SEASON_TIER_UNMARKED => "候选未标季（默认第一季）",
        matching::SEASON_TIER_UNMARKED_VERIFY => "候选未标季，需分集标题核验",
        _ => "未知档位",
    }
}

/// 检查 2：服务可达性 + 原始搜索结果。返回 (检查项, 探测结果)。
fn check_service(probes: &[Probe], bases: &[String]) -> Value {
    let t = Instant::now();
    let Some(p) = probes
        .iter()
        // 优先展示「有结果」的候选词（播放标题的原始串通常带集名，0 条；干净首段才有结果）
        .find(|p| p.http_status.is_some() && p.error.is_none() && !p.animes.is_empty())
        .or_else(|| {
            probes
                .iter()
                .find(|p| p.http_status.is_some() && p.error.is_none())
        })
        .or_else(|| probes.iter().find(|p| p.http_status.is_some()))
        .or_else(|| probes.first())
    else {
        return item("service", "自建源服务", "fail", "没有可用的探测结果", t);
    };
    if let Some(err) = &p.error {
        let host = if p.host.is_empty() {
            String::new()
        } else {
            format!("（{}）", p.host)
        };
        return item("service", "自建源服务", "fail", format!("{}{host}", err), t);
    }
    let n = p.animes.len();
    let titles: Vec<String> = p
        .animes
        .iter()
        .take(3)
        .map(|a| truncate(&anime_title(a), 24))
        .filter(|s| !s.is_empty())
        .collect();
    let mut detail = format!(
        "HTTP {} · 关键词「{}」· {} 条结果 · {} · {}ms",
        p.http_status.unwrap_or(0),
        truncate(&p.keyword, 32),
        n,
        p.host,
        p.elapsed_ms
    );
    if !titles.is_empty() {
        detail.push_str(&format!("；前 {} 条：{}", titles.len(), titles.join(" / ")));
    }
    if bases.len() > 1 {
        detail.push_str(&format!("；已配置 {} 个地址", bases.len()));
    }
    let status = if n > 0 { "ok" } else { "warn" };
    if n == 0 {
        detail.push_str("；服务正常但该关键词 0 条结果");
    }
    item("service", "自建源服务", status, detail, t)
}

/// 检查 3：业务匹配（复用 Rust 侧 normalize / 主名+季号多级匹配）。
fn check_matching(probes: &[Probe], season: i64, ep_title: &str) -> Value {
    let t = Instant::now();
    let season_u32 = if season > 0 { season as u32 } else { 0 };
    let return_unverified = !ep_title.trim().is_empty();
    let mut best: Option<(u8, String, i64, String)> = None;
    let mut total = 0usize;
    for p in probes {
        for a in &p.animes {
            total += 1;
            let title = anime_title(a);
            if title.is_empty() {
                continue;
            }
            if let Some(tier) =
                matching::exact_match_tier(&p.keyword, season_u32, &title, return_unverified)
            {
                let better = best.as_ref().map(|(b, ..)| tier < *b).unwrap_or(true);
                if better {
                    best = Some((tier, title, anime_id(a), p.keyword.clone()));
                }
            }
        }
    }
    if let Some((tier, title, id, kw)) = best {
        return item(
            "matching",
            "业务匹配",
            "ok",
            format!(
                "命中《{}》（关键词「{}」，{}，animeId={}）",
                truncate(&title, 40),
                truncate(&kw, 32),
                tier_label(tier),
                id
            ),
            t,
        );
    }
    let tried: Vec<String> = probes
        .iter()
        .map(|p| format!("「{}」{} 条", truncate(&p.keyword, 20), p.animes.len()))
        .collect();
    if total == 0 {
        item(
            "matching",
            "业务匹配",
            "fail",
            format!(
                "所有关键词候选均 0 条结果（{}）—— 服务端未收录该番名，或地址/TOKEN 指向了别的库",
                tried.join("，")
            ),
            t,
        )
    } else {
        item(
            "matching",
            "业务匹配",
            "warn",
            format!(
                "共 {} 条近名条目（{}），但均未通过「主名归一化 + 季号」精确匹配，会被拒收以防挂错片；可改用更干净的番名或核对季号",
                total,
                tried.join("，")
            ),
            t,
        )
    }
}

/// 检查 4：shim（22347 + /health）。
async fn check_shim() -> Value {
    let t = Instant::now();
    let port = crate::danmaku::LISTEN_PORT;
    let listening = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false);
    if !listening {
        return item(
            "shim",
            "弹幕服务 shim",
            "fail",
            format!("{port} 未监听（弹幕服务未启动）"),
            t,
        );
    }
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return item(
                "shim",
                "弹幕服务 shim",
                "warn",
                format!("{port} 已监听，但构造请求失败：{e}"),
                t,
            );
        }
    };
    match client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await
    {
        Ok(r) => match r.json::<Value>().await {
            Ok(v) => {
                let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
                let engine = v
                    .get("engine")
                    .and_then(|x| x.as_str())
                    .unwrap_or("unknown");
                if ok {
                    item(
                        "shim",
                        "弹幕服务 shim",
                        "ok",
                        format!("{port} 监听正常 · /health ok · engine={engine}"),
                        t,
                    )
                } else {
                    item(
                        "shim",
                        "弹幕服务 shim",
                        "warn",
                        format!("{port} 已监听，但 /health 未返回 ok"),
                        t,
                    )
                }
            }
            Err(e) => item(
                "shim",
                "弹幕服务 shim",
                "warn",
                format!("{port} 已监听，但 /health 解析失败：{e}"),
                t,
            ),
        },
        Err(e) => item(
            "shim",
            "弹幕服务 shim",
            "warn",
            format!(
                "{port} 已监听，但 /health 请求失败：{}",
                describe_reqwest(&e)
            ),
            t,
        ),
    }
}

/// 检查 5：B 站 Cookie 登录态（复用现有体检，不打印 Cookie）。
async fn check_bili(state: State<'_, AppState>) -> Value {
    let t = Instant::now();
    match tokio::time::timeout(
        ITEM_TIMEOUT,
        crate::bili_account::bili_account_status(state),
    )
    .await
    {
        Ok(Ok(v)) => {
            let logged = v.get("loggedIn").and_then(|x| x.as_bool()).unwrap_or(false);
            let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("");
            let uname = v.get("uname").and_then(|x| x.as_str()).unwrap_or("");
            let reason = v.get("reason").and_then(|x| x.as_str()).unwrap_or("");
            if logged {
                let who = if uname.is_empty() {
                    String::new()
                } else {
                    format!(" · {uname}")
                };
                item("bili", "B 站 Cookie", "ok", format!("已登录{who}"), t)
            } else if status == "missing" {
                item(
                    "bili",
                    "B 站 Cookie",
                    "warn",
                    "未登录（未找到 Cookie；匿名仍可获取弹幕，但成功率较低）",
                    t,
                )
            } else {
                let extra = if reason.is_empty() {
                    status.to_string()
                } else {
                    format!("{status}：{reason}")
                };
                item(
                    "bili",
                    "B 站 Cookie",
                    "warn",
                    format!("未登录（{extra}）"),
                    t,
                )
            }
        }
        Ok(Err(e)) => item("bili", "B 站 Cookie", "warn", format!("体检失败：{e}"), t),
        Err(_) => item(
            "bili",
            "B 站 Cookie",
            "warn",
            "体检超时（>5s），可能网络较慢",
            t,
        ),
    }
}

/// 检查 6：当前播放上下文。
fn check_play(ctx: &PlayCtx) -> Value {
    let t = Instant::now();
    if !ctx.exists {
        return item(
            "play",
            "当前播放上下文",
            "warn",
            "未检测到播放上下文（尚未开始播放）",
            t,
        );
    }
    if ctx.title.is_empty() {
        return item(
            "play",
            "当前播放上下文",
            "warn",
            "播放上下文存在但缺少标题（旧版本写入，重新播放一次即可）",
            t,
        );
    }
    let se = match (ctx.season > 0, ctx.episode > 0) {
        (true, true) => format!(" · S{}E{}", ctx.season, ctx.episode),
        (false, true) => format!(" · 第 {} 集", ctx.episode),
        (true, false) => format!(" · 第 {} 季", ctx.season),
        _ => String::new(),
    };
    item(
        "play",
        "当前播放上下文",
        "ok",
        format!("《{}》{se}", truncate(&ctx.title, 60)),
        t,
    )
}

/// 手动诊断自建弹幕源（设置面板「诊断」按钮）。
///
/// `keyword` 可选：不传/空时优先用当前播放标题，取不到再用内置测试词。
#[tauri::command]
pub async fn danmaku_diagnose(
    app: AppHandle,
    state: State<'_, AppState>,
    keyword: Option<String>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let play = read_play_ctx();
    let (kw, keyword_source) = resolve_keyword(keyword, &play);
    let bases = effective_bases(&cfg);

    // 关键词候选（原始词优先，必要时回退到砍掉集标题后的首段）。
    let mut candidates = matching::keyword_candidates(&kw);
    if candidates.is_empty() {
        candidates.push(kw.clone());
    }
    candidates.truncate(3);

    // 搜索探测共用 5s 预算（每项检查不额外叠加）。
    let deadline = Instant::now() + ITEM_TIMEOUT;
    let mut probes: Vec<Probe> = Vec::new();
    for cand in &candidates {
        if Instant::now() >= deadline {
            break;
        }
        probes.push(probe_search(&bases, cand, deadline).await);
    }
    if probes.is_empty() {
        probes.push(Probe::unreachable(&kw, "诊断超时（5s）".into()));
    }

    let mut checks = Vec::new();
    checks.push(check_node(&app).await);
    checks.push(check_service(&probes, &bases));
    checks.push(check_matching(&probes, play.season, &play.title));
    checks.push(check_shim().await);
    checks.push(check_bili(state).await);
    checks.push(check_play(&play));

    Ok(json!({
        "keyword": kw,
        "keywordSource": keyword_source,
        "enabled": cfg.danmu_api_enabled,
        "checks": checks,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(kw: &str, titles: &[&str]) -> Probe {
        Probe {
            keyword: kw.to_string(),
            http_status: Some(200),
            host: "127.0.0.1:22352".into(),
            elapsed_ms: 12,
            animes: titles
                .iter()
                .enumerate()
                .map(|(i, t)| json!({"animeId": i as i64 + 1, "animeTitle": t}))
                .collect(),
            error: None,
        }
    }

    #[test]
    fn keyword_prefers_input_then_play_then_builtin() {
        let mut ctx = PlayCtx::default();
        assert_eq!(
            resolve_keyword(None, &ctx),
            (BUILTIN_KEYWORD.into(), "builtin")
        );
        ctx.title = "葬送的芙莉莲 - S01E03: 魔法".into();
        assert_eq!(resolve_keyword(None, &ctx).1, "playing");
        assert_eq!(
            resolve_keyword(Some("  自定义 ".into()), &ctx),
            ("自定义".into(), "custom")
        );
        // 空白输入视为未提供
        assert_eq!(resolve_keyword(Some("   ".into()), &ctx).1, "playing");
    }

    #[test]
    fn matching_reports_hit_for_exact_normalized_title() {
        let p = probe("沧元图", &["沧元图(2023)【国漫】from youku"]);
        let v = check_matching(&[p], 0, "");
        assert_eq!(v["status"], json!("ok"));
        assert!(v["detail"].as_str().unwrap().contains("命中"));
    }

    #[test]
    fn matching_warns_when_only_near_names() {
        let p = probe("沧元图", &["沧元图外传", "另一个番"]);
        let v = check_matching(&[p], 0, "");
        assert_eq!(v["status"], json!("warn"));
        assert!(v["detail"].as_str().unwrap().contains("近名条目"));
    }

    #[test]
    fn matching_fails_when_no_results() {
        let p = probe("不存在", &[]);
        let v = check_matching(&[p], 0, "");
        assert_eq!(v["status"], json!("fail"));
    }
}
