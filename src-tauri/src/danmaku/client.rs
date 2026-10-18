//! 自建弹幕源（danmu_api）客户端：三跳 + 多地址故障转移 + TOKEN 路径前缀 + 脱敏日志。
//!
//! 逐条对齐 `assets/danmu-api.cjs`：
//!   GET {base}/api/v2/search/anime?keyword=<urlencoded>
//!   GET {base}/api/v2/bangumi/{animeId}
//!   GET {base}/api/v2/comment/{episodeId}?format=xml
//! 其中 `base` 已含 TOKEN 路径前缀（`http://host:port/<TOKEN>`），无自定义 UA/Cookie/签名。
//!
//! 故障转移：只有**传输层失败**（连接不上/被掐断/超时）才切下一个地址；一旦服务端返回
//! 了 HTTP 响应（含 4xx/5xx）就停在该地址并把错误交给上层 —— 那说明地址是对的，问题在
//! 请求参数（例如 TOKEN 路径写错），切换只会掩盖真实错误。

use crate::danmaku::matching::{
    self, exact_match_tier, normalize_ep_title, normalize_title, sort_hits, SEASON_TIER_UNMARKED_VERIFY,
};
use crate::danmaku::memory;
use crate::danmaku::types::{
    Episode, PickedEpisode, SearchHit, SelfHostedOutcome,
};
use crate::danmaku::xml;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_TRIES: usize = 3;
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_BODY: u64 = 32 * 1024 * 1024;
pub const SOURCE_LABEL: &str = "自建源(danmu_api)";
pub const ID_PREFIX: &str = "dmapi:";

fn re_valid_base() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)^https?://[a-z0-9._\-]+(:\d{1,5})?(/[a-z0-9._\-/]*)?$").unwrap()
    })
}

pub fn is_valid_base(base: &str) -> bool {
    re_valid_base().is_match(base.trim())
}

/// 拼接 base（可能含 TOKEN 路径）+ `/api/...`。
pub fn join_url(base: &str, path_with_query: &str) -> String {
    format!(
        "{}{}",
        base.trim().trim_end_matches('/'),
        if path_with_query.starts_with('/') {
            path_with_query.to_string()
        } else {
            format!("/{path_with_query}")
        }
    )
}

/// 日志脱敏：只留 host + `/api/…` 尾段 + query；路径里没有 `/api/` 时一律隐藏前缀。
pub fn safe_url(u: &str) -> String {
    match url::Url::parse(u) {
        Ok(p) => {
            let path = p.path();
            let tail = match path.find("/api/") {
                Some(i) => &path[i..],
                None => "/<路径前缀·已隐藏>",
            };
            let query = if p.query().is_some() {
                format!("?{}", p.query().unwrap_or(""))
            } else {
                String::new()
            };
            let host = match p.port() {
                Some(port) => format!("{}:{port}", p.host_str().unwrap_or("(非法地址)")),
                None => p.host_str().unwrap_or("(非法地址)").to_string(),
            };
            format!("{}{}{}", host, tail, query)
        }
        Err(_) => "(非法地址)".into(),
    }
}

pub fn host_of(base: &str) -> String {
    url::Url::parse(base)
        .ok()
        .and_then(|u| u.host_str().map(|h| match u.port() {
            Some(p) => format!("{h}:{p}"),
            None => h.to_string(),
        }))
        .unwrap_or_else(|| "(非法地址)".into())
}

enum GetError {
    Transport(String),
    Http { status: u16, msg: String },
}

/// 故障转移结果。
#[allow(dead_code)] // Http 的字段在非测试构建里只用于展示/上层匹配
pub enum FailoverResult {
    Ok { body: String, host: String },
    Http { status: u16, msg: String, host: String },
    Transport(String),
}

fn extract_error_message(body: &str) -> String {
    if let Ok(j) = serde_json::from_str::<Value>(body) {
        for k in ["errorMessage", "error", "message"] {
            if let Some(s) = j.get(k).and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    return s.chars().take(60).collect();
                }
            }
        }
    }
    String::new()
}

fn describe_reqwest(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "请求超时".into()
    } else if e.is_connect() {
        "连接失败".into()
    } else {
        e.to_string()
    }
}

pub struct SelfHostedClient {
    bases: Vec<String>,
    active: Mutex<usize>,
    search_cache: Mutex<HashMap<String, (Instant, Vec<SearchHit>)>>,
    bangumi_cache: Mutex<HashMap<String, (Instant, Vec<Episode>)>>,
    last_fail: Mutex<String>,
    last_raw_anime_count: AtomicI64,
    http: reqwest::Client,
}

impl SelfHostedClient {
    pub fn new(bases: Vec<String>) -> Self {
        let bases: Vec<String> = bases
            .into_iter()
            .map(|b| b.trim().trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty())
            .collect();
        SelfHostedClient {
            bases,
            active: Mutex::new(0),
            search_cache: Mutex::new(HashMap::new()),
            bangumi_cache: Mutex::new(HashMap::new()),
            last_fail: Mutex::new(String::new()),
            last_raw_anime_count: AtomicI64::new(0),
            http: reqwest::Client::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        !self.bases.is_empty() && self.bases.iter().any(|b| is_valid_base(b))
    }

    pub fn last_fail_reason(&self) -> String {
        self.last_fail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set_last_fail(&self, s: String) {
        *self.last_fail.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    async fn get_once(&self, url: &str, timeout_ms: u64) -> Result<String, GetError> {
        let mut resp = self
            .http
            .get(url)
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await
            .map_err(|e| GetError::Transport(describe_reqwest(&e)))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GetError::Http {
                status: status.as_u16(),
                msg: extract_error_message(&body),
            });
        }
        if let Some(len) = resp.content_length() {
            if len > MAX_BODY {
                return Err(GetError::Transport(format!(
                    "响应超过 {} 字节上限",
                    MAX_BODY
                )));
            }
        }
        let bytes = {
            let mut buf: Vec<u8> = Vec::new();
            loop {
                match resp.chunk().await {
                    Ok(Some(chunk)) => {
                        if buf.len() as u64 + chunk.len() as u64 > MAX_BODY {
                            return Err(GetError::Transport(format!(
                                "响应超过 {} 字节上限，已中断",
                                MAX_BODY
                            )));
                        }
                        buf.extend_from_slice(&chunk);
                    }
                    Ok(None) => break,
                    Err(e) => return Err(GetError::Transport(describe_reqwest(&e))),
                }
            }
            buf
        };
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// 文本请求的 failover：从上次连通的地址开始按序尝试。
    pub(crate) async fn text_failover(
        &self,
        path_with_query: &str,
        timeout_ms: u64,
    ) -> FailoverResult {
        let n = self.bases.len();
        if n == 0 {
            return FailoverResult::Transport("未配置自建源地址".into());
        }
        let start = *self.active.lock().unwrap_or_else(|e| e.into_inner());
        let mut errs: Vec<String> = Vec::new();
        for step in 0..n {
            let idx = (start + step) % n;
            let base = &self.bases[idx];
            let url = join_url(base, path_with_query);
            match self.get_once(&url, timeout_ms).await {
                Ok(body) => {
                    *self.active.lock().unwrap_or_else(|e| e.into_inner()) = idx;
                    return FailoverResult::Ok {
                        body,
                        host: host_of(base),
                    };
                }
                Err(GetError::Http { status, msg }) => {
                    let detail = if msg.is_empty() {
                        format!("HTTP {status}")
                    } else {
                        format!("HTTP {status} {msg}")
                    };
                    self.set_last_fail(format!(
                        "服务有响应但请求失败（{detail}）| {}",
                        safe_url(&url)
                    ));
                    return FailoverResult::Http {
                        status,
                        msg,
                        host: host_of(base),
                    };
                }
                Err(GetError::Transport(e)) => {
                    errs.push(format!("#{} {} {e}", idx + 1, host_of(base)));
                }
            }
        }
        let joined = errs.join("；");
        self.set_last_fail(format!(
            "服务地址全部不可达 —— {joined}（请检查 danmu_api 服务、网络/节点是否在线）"
        ));
        FailoverResult::Transport(joined)
    }

    async fn get_json_failover(&self, path_with_query: &str, timeout_ms: u64) -> Option<Value> {
        match self.text_failover(path_with_query, timeout_ms).await {
            FailoverResult::Ok { body, host } => match serde_json::from_str::<Value>(&body) {
                Ok(v) => Some(v),
                Err(e) => {
                    self.set_last_fail(format!(
                        "服务有响应但返回的不是合法 JSON（{host}）：{e}"
                    ));
                    None
                }
            },
            _ => None,
        }
    }

    async fn search_with_keyword(
        &self,
        keyword: &str,
        season: u32,
        ep_title: &str,
        timeout_ms: u64,
        force_fresh: bool,
    ) -> Vec<SearchHit> {
        let key = normalize_title(keyword);
        let mut animes: Option<Vec<Value>> = None;
        {
            let cache = self.search_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, val)) = cache.get(&key) {
                if !force_fresh && at.elapsed() < CACHE_TTL {
                    animes = Some(
                        val.iter()
                            .map(|h| {
                                json!({
                                    "animeId": h.anime_id,
                                    "animeTitle": h.anime_title,
                                    "episodeCount": h.episode_count,
                                    "source": h.source,
                                })
                            })
                            .collect(),
                    );
                }
            }
        }
        let raw_animes: Vec<Value> = match animes {
            Some(a) => a,
            None => {
                let path = format!(
                    "/api/v2/search/anime?keyword={}",
                    urlencoding::encode(keyword)
                );
                let j = self.get_json_failover(&path, timeout_ms).await;
                let j = match j {
                    Some(j) => j,
                    None => return vec![],
                };
                if j.get("success").and_then(|v| v.as_bool()) == Some(false) {
                    return vec![];
                }
                let list = match j.get("animes").and_then(|v| v.as_array()) {
                    Some(a) => a.clone(),
                    None => return vec![],
                };
                // 缓存原始条目（用于同关键词连播复用）
                let hits_for_cache: Vec<SearchHit> = list
                    .iter()
                    .filter_map(|a| {
                        let id = a.get("animeId").and_then(|v| v.as_i64()).unwrap_or(0);
                        if id == 0 {
                            return None;
                        }
                        Some(SearchHit {
                            anime_id: id,
                            anime_title: a
                                .get("animeTitle")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            episode_count: a
                                .get("episodeCount")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0),
                            source: a
                                .get("source")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            season_tier: 0,
                        })
                    })
                    .collect();
                self.search_cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key.clone(), (Instant::now(), hits_for_cache));
                list
            }
        };

        let raw_len = raw_animes.len() as i64;
        if raw_len > self.last_raw_anime_count.load(Ordering::Relaxed) {
            self.last_raw_anime_count.store(raw_len, Ordering::Relaxed);
        }

        let mut hits: Vec<SearchHit> = Vec::new();
        for a in &raw_animes {
            let id = a.get("animeId").and_then(|v| v.as_i64()).unwrap_or(0);
            if id == 0 {
                continue;
            }
            let anime_title = a
                .get("animeTitle")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(tier) = exact_match_tier(keyword, season, &anime_title, !ep_title.is_empty()) {
                hits.push(SearchHit {
                    anime_id: id,
                    anime_title,
                    episode_count: a.get("episodeCount").and_then(|v| v.as_i64()).unwrap_or(0),
                    source: a
                        .get("source")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    season_tier: tier,
                });
            }
        }
        sort_hits(&mut hits);
        hits
    }

    async fn search_animes(&self, title: &str, season: u32, ep_title: &str) -> Vec<SearchHit> {
        for cand in matching::keyword_candidates(title) {
            let hits = self
                .search_with_keyword(&cand, season, ep_title, 12_000, false)
                .await;
            if !hits.is_empty() {
                return hits;
            }
        }
        vec![]
    }

    async fn episodes_of(&self, anime_id: i64) -> Vec<Episode> {
        let key = anime_id.to_string();
        {
            let cache = self.bangumi_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, val)) = cache.get(&key) {
                if at.elapsed() < CACHE_TTL {
                    return val.clone();
                }
            }
        }
        let path = format!("/api/v2/bangumi/{anime_id}");
        let eps: Vec<Episode> = match self.get_json_failover(&path, 12_000).await {
            Some(j) => j
                .get("bangumi")
                .and_then(|b| b.get("episodes"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| {
                            let id = e.get("episodeId").and_then(|v| v.as_i64()).unwrap_or(0);
                            if id == 0 {
                                return None;
                            }
                            Some(Episode {
                                episode_id: id,
                                episode_title: e
                                    .get("episodeTitle")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                episode_number: e
                                    .get("episodeNumber")
                                    .and_then(|v| v.as_f64())
                                    .or_else(|| {
                                        e.get("episodeNumber")
                                            .and_then(|v| v.as_str())
                                            .and_then(|s| s.parse::<f64>().ok())
                                    }),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            None => vec![],
        };
        self.bangumi_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, (Instant::now(), eps.clone()));
        eps
    }

    /// 拉弹幕 XML 并落盘；返回条目数（0 = 未命中/写盘失败）。
    async fn fetch_xml(&self, episode_id: i64, out: &str) -> i64 {
        let path = format!("/api/v2/comment/{episode_id}?format=xml");
        let body = match self.text_failover(&path, 30_000).await {
            FailoverResult::Ok { body, .. } => body,
            FailoverResult::Http { .. } => return 0,
            FailoverResult::Transport(e) => {
                self.set_last_fail(format!("弹幕库拉取失败，服务地址全部不可达 —— {e}"));
                return 0;
            }
        };
        let count = xml::count_entries(&body);
        if count == 0 {
            return 0;
        }
        if xml::write_raw(out, &body).is_err() {
            return 0;
        }
        count
    }

    async fn pick_for_hit(&self, hit: &SearchHit, ep: i64, verify_title: &str) -> Option<PickedEpisode> {
        let eps = self.episodes_of(hit.anime_id).await;
        if hit.season_tier == SEASON_TIER_UNMARKED_VERIFY {
            matching::pick_verified_episode(&eps, ep, verify_title)
        } else {
            matching::pick_episode(&eps, hit.episode_count, ep)
        }
    }

    /// 自建源主流程（对齐 Node `autoFetch`）。命中返回 `Some`，未命中返回 `None`。
    pub async fn auto_fetch(
        &self,
        title: &str,
        ep: i64,
        out: &str,
        season: u32,
        ep_title: &str,
        series_key: &str,
        alt_title: &str,
    ) -> Option<SelfHostedOutcome> {
        if !self.is_active() {
            return None;
        }
        self.set_last_fail(String::new());
        self.last_raw_anime_count.store(0, Ordering::Relaxed);

        // 播放侧集标题：调用方没给时从整串 title 尾段提取
        let mut verify_title = ep_title.to_string();
        if verify_title.is_empty() {
            let ex = matching::extract_episode_title(title);
            let head = matching::keyword_candidates(title)
                .last()
                .cloned()
                .unwrap_or_default();
            if !ex.is_empty() && normalize_ep_title(&ex) != normalize_ep_title(&head) {
                verify_title = ex;
            }
        }

        // 系列记忆命中：直接复用条目定位分集，跳过标题搜索
        if let Some(mem) = memory::recall(series_key) {
            if let Some(e) = self.pick_for_hit(&mem, ep, &verify_title).await {
                let count = self.fetch_xml(e.episode_id, out).await;
                if count > 0 {
                    return Some(SelfHostedOutcome::hit(
                        count,
                        title.to_string(),
                        mem.anime_title.clone(),
                    ));
                }
            }
        }

        let mut hits = self.search_animes(title, season, &verify_title).await;
        if hits.is_empty() && !alt_title.is_empty() {
            let alt_hits = self.search_animes(alt_title, season, "").await;
            if !alt_hits.is_empty() {
                hits.extend(alt_hits);
            }
        }
        if hits.is_empty() {
            // 干净词强刷重试（绕过本地缓存，8s 短超时）
            let mut clean: Vec<String> = matching::keyword_candidates(title)
                .into_iter()
                .skip(1)
                .collect();
            if !alt_title.is_empty() {
                clean.push(alt_title.to_string());
            }
            let mut seen = std::collections::HashSet::new();
            clean.retain(|k| {
                let t = k.trim().to_string();
                !t.is_empty() && seen.insert(t)
            });
            for kw in clean {
                hits.extend(
                    self.search_with_keyword(&kw, season, &verify_title, 8_000, true)
                        .await,
                );
            }
        }
        if hits.is_empty() {
            let raw = self.last_raw_anime_count.load(Ordering::Relaxed);
            if self.last_fail_reason().is_empty() {
                let reason = if raw > 0 {
                    format!(
                        "自建源有 {raw} 条近名条目，但均非「主名+季号」精确匹配（已拒收，防挂错片）"
                    )
                } else {
                    "自建源未收录该条目（服务端 0 条结果）".to_string()
                };
                self.set_last_fail(reason);
            }
            return None;
        }

        for hit in hits.iter().take(MAX_TRIES) {
            let Some(e) = self.pick_for_hit(hit, ep, &verify_title).await else {
                continue;
            };
            let count = self.fetch_xml(e.episode_id, out).await;
            if count > 0 {
                memory::remember(series_key, hit);
                return Some(SelfHostedOutcome::hit(
                    count,
                    title.to_string(),
                    hit.anime_title.clone(),
                ));
            }
        }
        if self.last_fail_reason().is_empty() {
            self.set_last_fail(format!(
                "已精确匹配 {} 个条目，但都取不到有效弹幕（服务端这些条目的弹幕库为空）",
                hits.len().min(MAX_TRIES)
            ));
        }
        None
    }

    /// 手动搜索候选（对齐 Node `candidates`），bvid 位填 `dmapi:<animeId>:<episodeId>`。
    pub async fn candidates(&self, title: &str, ep: i64, season: u32) -> Vec<Value> {
        if !self.is_active() {
            return vec![];
        }
        let ep_title = matching::extract_episode_title(title);
        let hits = self.search_animes(title, season, &ep_title).await;
        let mut out = Vec::new();
        for hit in hits.iter().take(MAX_TRIES + 2) {
            let mut e = self.pick_for_hit(hit, ep, &ep_title).await;
            if e.is_none() && hit.season_tier == SEASON_TIER_UNMARKED_VERIFY {
                let eps = self.episodes_of(hit.anime_id).await;
                e = matching::pick_episode(&eps, hit.episode_count, ep);
            }
            let Some(e) = e else { continue };
            let main_name = main_name_of(&hit.anime_title);
            let plat = platform_of(&hit.anime_title);
            let title = if e.episode_title.is_empty() {
                main_name
            } else {
                format!("{} · {}", main_name, e.episode_title)
            };
            out.push(json!({
                "index": out.len(),
                "cid": Value::Null,
                "bvid": format!("{ID_PREFIX}{}:{}", hit.anime_id, e.episode_id),
                "title": title,
                "source": "自建源",
                "season": season,
                "is_compilation": false,
                "sim": 1,
                "platform": plat,
            }));
        }
        out
    }
}

/// 剥「(年份)【类型】from 平台」尾缀，得到干净主名（候选 UI 用）。
pub fn main_name_of(anime_title: &str) -> String {
    static RE_YEAR: OnceLock<Regex> = OnceLock::new();
    static RE_TYPE: OnceLock<Regex> = OnceLock::new();
    static RE_FROM: OnceLock<Regex> = OnceLock::new();
    let re_year = RE_YEAR.get_or_init(|| Regex::new(r"\s*[\(（](?:19|20)\d{2}[\)）]\s*").unwrap());
    let re_type = RE_TYPE.get_or_init(|| Regex::new(r"【[^】]*】").unwrap());
    let re_from = RE_FROM.get_or_init(|| Regex::new(r"(?i)\s*from\s+[a-z0-9]+\s*$").unwrap());
    let t = re_year.replace_all(anime_title, "");
    let t = re_type.replace_all(&t, "");
    re_from.replace_all(&t, "").trim().to_string()
}

pub fn platform_of(anime_title: &str) -> String {
    static RE_FROM: OnceLock<Regex> = OnceLock::new();
    let re = RE_FROM.get_or_init(|| Regex::new(r"(?i)from\s+([a-z0-9]+)").unwrap());
    re.captures(anime_title)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    fn spawn_server(
        status: &'static str,
        body: &'static str,
        hits: Arc<AtomicUsize>,
    ) -> (u16, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for conn in listener.incoming() {
                let mut sock = match conn {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf);
                hits.fetch_add(1, Ordering::SeqCst);
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        (port, handle)
    }

    fn tokio_rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn token_prefix_join_and_redaction() {
        assert_eq!(
            join_url("http://h:9321/TOKEN/", "/api/v2/x?q=1"),
            "http://h:9321/TOKEN/api/v2/x?q=1"
        );
        let red = safe_url("http://h:9321/TOKEN/api/v2/search/anime?keyword=a");
        assert_eq!(red, "h:9321/api/v2/search/anime?keyword=a");
        assert!(!red.contains("TOKEN"));
        // 无 /api/ 时隐藏整段路径
        let red2 = safe_url("http://h:9321/TOKEN/root");
        assert_eq!(red2, "h:9321/<路径前缀·已隐藏>");
        assert!(!red2.contains("TOKEN"));
        assert!(is_valid_base("http://192.168.1.2:9321/abc"));
        assert!(!is_valid_base("ftp://x"));
        assert!(!is_valid_base("http://h:9321/TO\nKEN"));
    }

    #[test]
    fn failover_switches_on_transport_error() {
        let hits_b = Arc::new(AtomicUsize::new(0));
        let (port_b, _h) = spawn_server("200 OK", r#"{"ok":true}"#, hits_b.clone());
        // 先占一个端口再释放，制造「连接被拒绝」
        let dead_port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let client = SelfHostedClient::new(vec![
            format!("http://127.0.0.1:{dead_port}"),
            format!("http://127.0.0.1:{port_b}"),
        ]);
        let rt = tokio_rt();
        let res = rt.block_on(client.text_failover("/api/v2/search/anime?keyword=x", 2000));
        match res {
            FailoverResult::Ok { body, .. } => assert!(body.contains("true")),
            _ => panic!("应切到第二个地址成功"),
        }
        assert_eq!(hits_b.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failover_does_not_switch_on_4xx() {
        let hits_a = Arc::new(AtomicUsize::new(0));
        let hits_b = Arc::new(AtomicUsize::new(0));
        let (port_a, _ha) = spawn_server(
            "401 Unauthorized",
            r#"{"errorMessage":"Unauthorized"}"#,
            hits_a.clone(),
        );
        let (port_b, _hb) = spawn_server("200 OK", r#"{"ok":true}"#, hits_b.clone());
        let client = SelfHostedClient::new(vec![
            format!("http://127.0.0.1:{port_a}"),
            format!("http://127.0.0.1:{port_b}"),
        ]);
        let rt = tokio_rt();
        let res = rt.block_on(client.text_failover("/api/v2/search/anime?keyword=x", 2000));
        match res {
            FailoverResult::Http { status, msg, .. } => {
                assert_eq!(status, 401);
                assert_eq!(msg, "Unauthorized");
            }
            _ => panic!("4xx 不应切换地址"),
        }
        assert_eq!(hits_a.load(Ordering::SeqCst), 1);
        assert_eq!(hits_b.load(Ordering::SeqCst), 0, "第二个地址不应被访问");
    }
}
