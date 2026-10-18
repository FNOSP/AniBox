//! [lc-1500] 在线资源源（Kazumi 规则兼容）。
//!
//! 原理对齐 Predidit/Kazumi（GPL-3.0，本项目亦开源）：
//! 1. **规则 JSON**（社区仓库 KazumiRules 同格式）：`searchURL` 用 `@keyword` 占位符，
//!    `searchList/searchName/searchResult` 用 XPath 从搜索结果页抽「条目名 + 详情页链接」；
//!    `chapterRoads/chapterResult` 从详情页抽「线路 + 选集」。
//! 2. **直链解析**（Kazumi 的 WebView 嗅探思路）：
//!    - 先走纯 HTTP：多数站的播放器配置把 m3u8/mp4 直链明文写在页面 JS 里，正则直接命中；
//!    - 没命中再拉起**隐藏 WebView** 加载播放页，注入 Kazumi 同款 JS 钩子
//!      （hook fetch/XHR/Response.text + 扫 video/source 标签 + 定时扫页面源码），
//!      命中后跳 `anibox-sniff://found?u=<url>` 由 Rust 的 on_navigation 拦截回传。
//! 3. **播放**：直链交给独立 mpv，Referer/UA 通过 mpv 的 `referrer`/`user-agent`
//!    属性设置（HLS 分段请求会沿用）。
//!
//! 规则目录：app_data_dir/webrs-rules/*.json。首次运行写入两条内置默认规则
//! （AGE / mutefun，内容取自 Predidit/KazumiRules）；用户可自行丢社区规则进来。
//!
//! XPath 支持子集（Kazumi 社区规则只用得到这些）：
//! `//tag` / `.//tag` / `tag`、`tag[N]`、`tag[@attr='v']`、`tag[contains(@attr,'v')]`、
//! 结尾 `@attr` / `text()`。其余语法报「不支持的 XPath」。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

use url::Url;

// ───────────────────────── 规则结构 ─────────────────────────

/// [lc-1500] Kazumi 规则 JSON（字段名与 KazumiRules 仓库逐一对齐；老规则只写
/// 前几个字段，全部带 default 宽容解析）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Rule {
    pub api: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub version: String,
    #[serde(rename = "baseURL", alias = "baseUrl")]
    pub base_url: String,
    #[serde(rename = "searchURL")]
    pub search_url: String,
    #[serde(rename = "searchList")]
    pub search_list: String,
    #[serde(rename = "searchName")]
    pub search_name: String,
    #[serde(rename = "searchResult")]
    pub search_result: String,
    #[serde(rename = "chapterRoads")]
    pub chapter_roads: String,
    #[serde(rename = "chapterResult")]
    pub chapter_result: String,
    #[serde(rename = "userAgent")]
    pub user_agent: String,
    #[serde(default)]
    pub referer: String,
    #[serde(rename = "usePost")]
    pub use_post: bool,
    #[serde(rename = "useLegacyParser")]
    pub use_legacy_parser: bool,
    #[serde(rename = "searchMode")]
    pub search_mode: String, // "xpath"（默认）| "api"
    #[serde(rename = "chapterMode")]
    pub chapter_mode: String, // "xpath"（默认）| "api"
    #[serde(rename = "searchApiConfig")]
    pub search_api_config: Option<ApiSearchConfig>,
    #[serde(rename = "chapterApiConfig")]
    pub chapter_api_config: Option<ApiChapterConfig>,
    /// [lc-1606] 反爬验证配置（对齐 Kazumi AntiCrawlerConfig）。
    #[serde(rename = "antiCrawlerConfig")]
    pub anti_crawler_config: AntiCrawlerConfig,
}

/// [lc-1606] 反爬验证配置（逐字段对齐 Kazumi `lib/plugins/anti_crawler_config.dart`）。
///
/// 仅当 XPath 搜索响应被判为「验证挑战页」时才启用：用隐藏 WebView 加载搜索页，
/// 按 `captchaType` 完成验证并保存 cookie + UA，随后自动重试检索。三型：
/// - 1 imageCaptcha：抓验证码图 → 前端弹窗输入 → 回填提交（`captchaImage`/`captchaInput`/`captchaButton`）；
/// - 2 autoClickButton：定位 `captchaButton` 自动点击（如「我不是机器人」），无需用户交互；
/// - 3 customJavaScript：执行规则自带的 `captchaScript`（配 `window.KazumiCaptcha` 桥）。
///
/// `captchaDetectType`：1 xpath / 2 text / 3 regex，配合 `captchaDetectValue` 判定挑战页。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AntiCrawlerConfig {
    pub enabled: bool,
    #[serde(rename = "captchaType")]
    pub captcha_type: u32,
    #[serde(rename = "captchaImage")]
    pub captcha_image: String,
    #[serde(rename = "captchaInput")]
    pub captcha_input: String,
    #[serde(rename = "captchaButton")]
    pub captcha_button: String,
    #[serde(rename = "captchaDetectType")]
    pub captcha_detect_type: u32,
    #[serde(rename = "captchaDetectValue")]
    pub captcha_detect_value: String,
    #[serde(rename = "captchaScript")]
    pub captcha_script: String,
}

impl Default for AntiCrawlerConfig {
    fn default() -> Self {
        // 缺省对齐 Kazumi 的 fromJson 兜底：captchaType=1(imageCaptcha)、captchaDetectType=1(xpath)。
        // 容器级 #[serde(default)] 会用这里的值填充 JSON 里缺失的字段。
        Self {
            enabled: false,
            captcha_type: 1,
            captcha_image: String::new(),
            captcha_input: String::new(),
            captcha_button: String::new(),
            captcha_detect_type: 1,
            captcha_detect_value: String::new(),
            captcha_script: String::new(),
        }
    }
}

/// [lc-1501] API 搜索配置（如 mutefun：`/index.php/ajax/suggest`、xfdmnext：PostgREST RPC）。
/// JSONPath 只实现子集：`$.list[*]` / `$[*]` 这类「字段名 + [*] 通配」。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ApiSearchConfig {
    pub request: ApiRequest,
    #[serde(rename = "listPath")]
    pub list_path: String,
    #[serde(rename = "namePath")]
    pub name_path: String,
    #[serde(rename = "sourcePath")]
    pub source_path: String,
}

/// [lc-1501] API 选集配置（sorani / xfdmnext）：模板请求拿 JSON，roadsPath 逐线路、
/// episodesPath 逐集，episodePage.url 用 `@source` / `@episodeUrl` 拼出播放页。
/// 只实现 Kazumi 的 "nested" 格式（delimited 的 $$$ 打包形态社区已无人用）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ApiChapterConfig {
    pub request: ApiRequest,
    pub format: String, // "nested"
    #[serde(rename = "roadsPath")]
    pub roads_path: String,
    #[serde(rename = "roadNamePath")]
    pub road_name_path: String,
    #[serde(rename = "episodesPath")]
    pub episodes_path: String,
    #[serde(rename = "episodeNamePath")]
    pub episode_name_path: String,
    #[serde(rename = "episodeUrlPath")]
    pub episode_url_path: String,
    #[serde(rename = "episodePage")]
    pub episode_page: ApiEpisodePage,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ApiEpisodePage {
    pub url: String,
    pub query: std::collections::HashMap<String, Value>,
}

/// [lc-1501] 通用请求模板：query 值可为字符串/数字/布尔（"@keyword"/"@source" 只在
/// 字符串值里替换）；POST 支持 form 与 JSON 两种 body；headers 原样透传（规则自带
/// 的 apikey / Authorization，如 xfdmnext 的 PostgREST）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ApiRequest {
    pub method: String,
    pub url: String,
    pub headers: std::collections::HashMap<String, String>,
    #[serde(rename = "bodyType")]
    pub body_type: String, // "form"（默认）| "json"
    pub body: Value,
    pub query: std::collections::HashMap<String, Value>,
}

impl ApiRequest {
    /// query 的字符串值里替换占位符（非字符串原样保留）
    fn query_pairs(&self, ph: &[(&str, &str)]) -> Vec<(String, String)> {
        self.query
            .iter()
            .map(|(k, v)| {
                let val = match v {
                    Value::String(s) => {
                        let mut s = s.clone();
                        for (p, r) in ph {
                            s = s.replace(p, r);
                        }
                        s
                    }
                    other => other.to_string(),
                };
                (k.clone(), val)
            })
            .collect()
    }

    /// body（仅 JSON body 用）：递归替换字符串值里的占位符
    fn body_json(&self, ph: &[(&str, &str)]) -> Value {
        fn walk(v: &Value, ph: &[(&str, &str)]) -> Value {
            match v {
                Value::String(s) => {
                    let mut s = s.clone();
                    for (p, r) in ph {
                        s = s.replace(p, r);
                    }
                    Value::String(s)
                }
                Value::Array(a) => Value::Array(a.iter().map(|x| walk(x, ph)).collect()),
                Value::Object(o) => {
                    Value::Object(o.iter().map(|(k, x)| (k.clone(), walk(x, ph))).collect())
                }
                other => other.clone(),
            }
        }
        walk(&self.body, ph)
    }
}

// ───────────────────────── 内置规则（Predidit/KazumiRules 全量） ─────────────────────────

/// [lc-1501] 内置**全部 16 条**社区规则（7sefun/aafun/AGE/akianime/baimao/dalvdm/
/// DM84/ezdmw/giriGiriLove/mgnacg/moonci/mutefun/MXdm/sorani/xfdmneo/xfdmnext），
/// 打包成一个 JSON 数组随二进制分发；文件内容 = KazumiRules 仓库原文。
const BUILTIN_RULES_JSON: &str = include_str!("builtin_rules.json");

pub fn rules_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {e}"))?
        .join("webrs-rules");
    Ok(dir)
}

/// 把内置规则逐条补种进 webrs-rules/（**缺哪个补哪个**，已有同名文件不覆盖 ——
/// 尊重用户改动；用户删掉的内置源会在下次启动补回，行为等同"恢复默认"）。
async fn seed_rules(app: &AppHandle) -> Result<(), String> {
    let dir = rules_dir(app)?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("创建规则目录失败: {e}"))?;
    for v in serde_json::from_str::<Vec<Value>>(BUILTIN_RULES_JSON).unwrap_or_default() {
        let Some(name) = v.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let p = dir.join(format!("{name}.json"));
        if !p.exists() {
            // 原文落盘（Value 序列化），不经过 Rule 结构体 —— 免得丢掉未建模的字段
            let body = serde_json::to_string_pretty(&v).unwrap_or_default();
            let _ = tokio::fs::write(&p, body).await;
        }
    }
    Ok(())
}

/// 列出全部可用规则（宽容解析：坏文件跳过）。
pub async fn load_rules(app: &AppHandle) -> Result<Vec<Rule>, String> {
    seed_rules(app).await?;
    let dir = rules_dir(app)?;
    let mut out = Vec::new();
    let mut rd = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) => return Err(format!("读规则目录失败: {e}")),
    };
    let mut names: Vec<String> = Vec::new();
    while let Ok(Some(ent)) = rd.next_entry().await {
        let p = ent.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match tokio::fs::read_to_string(&p).await {
            Ok(s) => match serde_json::from_str::<Rule>(&s) {
                Ok(mut r) => {
                    if r.name.is_empty() {
                        r.name = p
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unnamed")
                            .to_string();
                    }
                    // mode 归一化（对齐 Kazumi RuleMode::normalize）
                    if r.search_mode.is_empty() {
                        r.search_mode = "xpath".into();
                    }
                    if r.chapter_mode.is_empty() {
                        r.chapter_mode = "xpath".into();
                    }
                    names.push(r.name.clone());
                    out.push(r);
                }
                Err(e) => crate::libmpv::trace(&format!(
                    "[webrs] 规则 {} 解析失败: {e}",
                    p.display()
                )),
            },
            Err(e) => crate::libmpv::trace(&format!("[webrs] 规则 {} 读取失败: {e}", p.display())),
        }
    }
    let _ = names; // 调试用
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn find_rule<'a>(rules: &'a [Rule], name: &str) -> Result<&'a Rule, String> {
    rules
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("找不到规则「{name}」"))
}

// ───────────────────────── HTTP 客户端 ─────────────────────────

const UA_POOL: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36 Edg/125.0.0.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
];

/// 随机 UA（Kazumi：规则没配 userAgent 时 getRandomUA()）
fn random_ua() -> String {
    let i = rand::random::<u32>() as usize % UA_POOL.len();
    UA_POOL[i].to_string()
}

fn rule_ua(rule: &Rule) -> String {
    if rule.user_agent.trim().is_empty() {
        random_ua()
    } else {
        rule.user_agent.trim().to_string()
    }
}

/// referer：规则显式配置优先，否则 baseUrl + '/'（对齐 Kazumi rule_engine）
fn rule_referer(rule: &Rule) -> String {
    if !rule.referer.trim().is_empty() {
        return rule.referer.trim().to_string();
    }
    if rule.base_url.trim().is_empty() {
        return String::new();
    }
    format!("{}/", rule.base_url.trim_end_matches('/'))
}

/// [lc-1624] HTTP 客户端构建**返回 Result**（原来是 `.expect("reqwest client build")`）。
/// release 是 `panic = "abort"` —— 客户端构建一旦失败（TLS 后端/系统代理等异常），
/// expect 就是**直接崩进程**，而这条路径在「每次点播放 / 每次搜索」上都会走到。
/// 失败改成可诊断错误上抛，最坏只是这一路请求失败，不会把整个软件带走。
fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("HTTP 客户端初始化失败: {e}"))
}

// ───────────────────────── [lc-1606] 每规则 cookie 收割 / 复用 ─────────────────────────

/// 某条规则验证成功后收获的 cookie + UA（对齐 Kazumi `plugin_cookie_manager.dart`）。
/// **仅内存**：进程重启即失效，需重新验证（与 Kazumi 一致，避免把站点的 clearance
/// cookie 落到磁盘上长期留存）。
#[derive(Debug, Clone, Default)]
struct RuleCookie {
    cookies: Vec<(String, String)>,
    user_agent: String,
}

static COOKIE_STORE: OnceLock<std::sync::Mutex<HashMap<String, RuleCookie>>> = OnceLock::new();

fn cookie_store() -> &'static std::sync::Mutex<HashMap<String, RuleCookie>> {
    COOKIE_STORE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 存 cookie + UA（按 name 覆盖同名，保留其它）。UA 非空才更新。
fn save_cookies(rule_name: &str, cookies: Vec<(String, String)>, user_agent: Option<String>) {
    let Ok(mut g) = cookie_store().lock() else { return };
    let entry = g.entry(rule_name.to_string()).or_default();
    if let Some(ua) = user_agent {
        let ua = ua.trim();
        if !ua.is_empty() {
            entry.user_agent = ua.to_string();
        }
    }
    for (k, v) in cookies {
        if k.is_empty() {
            continue;
        }
        match entry.cookies.iter_mut().find(|(n, _)| n == &k) {
            Some(slot) => slot.1 = v,
            None => entry.cookies.push((k, v)),
        }
    }
}

/// 拼 `name=value; name2=value2` 的 Cookie 头；无则空串。
fn cookie_header_for(rule_name: &str) -> String {
    cookie_store()
        .lock()
        .ok()
        .and_then(|g| {
            g.get(rule_name).map(|c| {
                c.cookies
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
        })
        .unwrap_or_default()
}

fn verified_ua(rule_name: &str) -> Option<String> {
    cookie_store().lock().ok().and_then(|g| {
        g.get(rule_name).and_then(|c| {
            if c.user_agent.is_empty() {
                None
            } else {
                Some(c.user_agent.clone())
            }
        })
    })
}

/// 有效 UA：**验证过的 UA 覆盖规则 UA**（clearance cookie 与签发它的 UA 绑定，
/// 换一个 UA 会让站点重新发挑战页）。无验证记录时退回规则 UA。
fn effective_ua(rule: &Rule) -> String {
    if !cookie_header_for(&rule.name).is_empty() {
        if let Some(ua) = verified_ua(&rule.name) {
            return ua;
        }
    }
    rule_ua(rule)
}

/// 一次 HTTP 取回的原始响应：状态码 + 正文 + `cf-mitigated: challenge` 标记。
/// 非 2xx 也**保留 body**（对齐 Kazumi `_isCaptchaChallengeResponse`：挑战页常以
/// 403/429/503 返回，需要正文才能跑检测）。
struct HttpResp {
    status: reqwest::StatusCode,
    body: String,
    cf_mitigated: bool,
}

async fn fetch_text(url: &str, ua: &str, referer: &str, rule_name: &str) -> Result<HttpResp, String> {
    let client = http()?;
    let mut req = client.get(url).header("user-agent", ua);
    if !referer.is_empty() {
        req = req.header("referer", referer);
    }
    let cookie = cookie_header_for(rule_name);
    if !cookie.is_empty() {
        req = req.header("cookie", cookie);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求失败 {url}: {e}"))?;
    let status = resp.status();
    // Cloudflare 对 managed challenge 响应打此头（正文可能为空/压缩，头最可靠）
    let cf_mitigated = resp
        .headers()
        .get("cf-mitigated")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("challenge"))
        .unwrap_or(false);
    // 编码：这些站点多是 UTF-8，GBK 站点先用 lossless 兜底（latin1 丢字）——
    // reqwest 的 text() 默认按 header/UTF-8 解，乱码页面的搜索基本也匹配不上，先不管。
    let body = resp.text().await.map_err(|e| format!("读响应失败: {e}"))?;
    Ok(HttpResp {
        status,
        body,
        cf_mitigated,
    })
}

/// [lc-1606] 判定一次响应是否为「反爬验证挑战页」（对齐 Kazumi
/// `xpath_rule_strategy.dart:detectsCaptchaChallenge` + `rule_engine.dart:_isCaptchaChallengeResponse`）：
/// 1) `cf-mitigated: challenge` 头直接判真；
/// 2) 配了 `captchaDetectValue` 时按 captchaDetectType：1 xpath / 2 text(包含) / 3 regex；
/// 3) 未配 detectValue 时回退：看 captchaImage / captchaButton 选择器是否命中。
/// 未启用反爬（`enabled=false`）一律为假。
fn detects_captcha_challenge(resp: &HttpResp, cfg: &AntiCrawlerConfig) -> bool {
    if !cfg.enabled {
        return false;
    }
    if resp.cf_mitigated {
        return true;
    }
    let raw = resp.body.as_str();
    if raw.trim().is_empty() {
        return false;
    }
    let detect = cfg.captcha_detect_value.trim();
    if !detect.is_empty() {
        match cfg.captcha_detect_type {
            2 => return raw.contains(detect),
            3 => {
                if let Ok(re) = regex::RegexBuilder::new(detect)
                    .case_insensitive(true)
                    .dot_matches_new_line(true)
                    .build()
                {
                    return re.is_match(raw);
                }
                return false;
            }
            // 1 或未知 → xpath
            _ => return xpath_hits(raw, detect),
        }
    }
    // 回退：没有 detectValue 时看验证码图/按钮选择器是否命中
    for sel in [cfg.captcha_image.trim(), cfg.captcha_button.trim()] {
        if !sel.is_empty() && xpath_hits(raw, sel) {
            return true;
        }
    }
    false
}

/// 用本项目的手写 XPath 子集在 HTML 上求值，命中（非空节点）即真；解析失败视为未命中。
fn xpath_hits(html: &str, expr: &str) -> bool {
    let doc = scraper::Html::parse_document(html);
    let root = vec![doc.tree.root().id()];
    eval_nodes(&doc, &root, expr).map(|v| !v.is_empty()).unwrap_or(false)
}

/// 相对 URL 拼接（对齐 Kazumi normalizeEpisodeUrl 的常见形态）：
/// 绝对链接原样；`//host/...` 补协议；相对路径对 base 做 RFC3986 join。
/// 兼容 api 规则的 `名字$http://...` 打包格式（取 $ 后半段）。
fn join_url(base: &str, href: &str) -> Option<String> {
    let mut h = href.trim();
    if h.is_empty() {
        return None;
    }
    if let Some(pos) = h.rfind('$') {
        let tail = h[pos + 1..].trim();
        if tail.starts_with("http://") || tail.starts_with("https://") {
            h = tail;
        }
    }
    if h.starts_with("http://") || h.starts_with("https://") {
        return Some(h.to_string());
    }
    if h.starts_with("javascript:") || h.starts_with('#') {
        return None;
    }
    let base = Url::parse(base.trim()).ok()?;
    if let Some(rest) = h.strip_prefix("//") {
        return Url::parse(&format!("{}://{}", base.scheme(), rest))
            .ok()
            .map(|u| u.to_string());
    }
    base.join(h).ok().map(|u| u.to_string())
}

// ───────────────────────── XPath 子集引擎 ─────────────────────────

#[derive(Debug, Clone)]
enum Pred {
    Pos(usize),
    Eq(String, String),
    Contains(String, String),
    StartsWith(String, String),
}

#[derive(Debug, Clone)]
enum Test {
    Tag(String),
    Any,
    Attr(String),
    Text,
}

/// XPath 轴（子集）：`//`=Descendant（经 descendant-or-self::node() 展开）、
/// `/`=Child、`self::`、`following-sibling::`（ezdmw 的选集规则用到后两个）。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Axis {
    Child,
    Desc,
    SelfAxis,
    FollowingSibling,
}

#[derive(Debug, Clone)]
struct Step {
    axis: Axis,
    test: Test,
    preds: Vec<Pred>,
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2
        && ((t.starts_with('\'') && t.ends_with('\'')) || (t.starts_with('"') && t.ends_with('"')))
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn parse_pred(raw: &str) -> Result<Pred, String> {
    let p = raw.trim();
    if p.is_empty() {
        return Err("空谓词".into());
    }
    if p.chars().all(|c| c.is_ascii_digit()) {
        return Ok(Pred::Pos(p.parse().map_err(|_| "谓词下标解析失败")?));
    }
    if let Some(rest) = p.strip_prefix("starts-with(") {
        let inner = rest.strip_suffix(')').ok_or("starts-with() 缺右括号")?;
        let (left, right) = inner
            .split_once(',')
            .ok_or("starts-with() 需要 @attr 与字符串两个参数")?;
        let attr = left
            .trim()
            .strip_prefix('@')
            .ok_or("starts-with() 第一个参数必须是 @attr")?
            .trim()
            .to_string();
        return Ok(Pred::StartsWith(attr, unquote(right)));
    }
    if let Some(rest) = p.strip_prefix("contains(") {
        let inner = rest.strip_suffix(')').ok_or("contains() 缺右括号")?;
        let (left, right) = inner
            .split_once(',')
            .ok_or("contains() 需要 @attr 与字符串两个参数")?;
        let attr = left
            .trim()
            .strip_prefix('@')
            .ok_or("contains() 第一个参数必须是 @attr")?
            .trim()
            .to_string();
        return Ok(Pred::Contains(attr, unquote(right)));
    }
    if let Some((left, right)) = p.split_once('=') {
        let attr = left
            .trim()
            .strip_prefix('@')
            .ok_or("等值谓词左边必须是 @attr")?
            .trim()
            .to_string();
        return Ok(Pred::Eq(attr, unquote(right)));
    }
    Err(format!("不支持的谓词: {p}"))
}

fn parse_step(raw: &str, default_axis: Axis) -> Result<Step, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("空 XPath 步进".into());
    }
    // 显式轴前缀（ezdmw 的选集规则用 self:: / following-sibling::）
    let (axis, raw) = if let Some(rest) = raw.strip_prefix("self::") {
        (Axis::SelfAxis, rest.trim())
    } else if let Some(rest) = raw.strip_prefix("following-sibling::") {
        (Axis::FollowingSibling, rest.trim())
    } else if raw == ".." || raw.starts_with("../") {
        // [lc-1624] 父节点轴：社区规则里没有，但写成 `..` 时给**可诊断**错误，
        // 而不是被当成一个名为 ".." 的标签静默匹配不到（表现为"没结果"，更难查）。
        return Err("暂不支持父节点轴 `..`（如需请改写为绝对路径）".into());
    } else if let Some(name) = [
        "parent::",
        "ancestor::",
        "ancestor-or-self::",
        "preceding-sibling::",
        "preceding::",
        "descendant::",
        "descendant-or-self::",
        "child::",
        "attribute::",
    ]
    .iter()
    .find_map(|a| raw.strip_prefix(*a))
    {
        return Err(format!(
            "暂不支持的轴 `{}`（仅支持 //、/、self::、following-sibling::）",
            &raw[..raw.len() - name.len()]
        ));
    } else {
        (default_axis, raw)
    };
    if let Some(attr) = raw.strip_prefix('@') {
        let name = attr.trim();
        // [lc-1624] `@attr` 混写谓词/路径（如 @href[1]、@href/a）不是合法属性步进，
        // 原实现会把整串当成属性名 → 静默匹配不到。这里给可诊断错误。
        if name.is_empty() || name.contains('[') || name.contains('/') {
            return Err(format!(
                "属性步进写法不支持: @{name}（属性名里不应含 [ 或 /）"
            ));
        }
        return Ok(Step {
            axis,
            test: Test::Attr(name.to_string()),
            preds: Vec::new(),
        });
    }
    if raw == "text()" {
        return Ok(Step {
            axis,
            test: Test::Text,
            preds: Vec::new(),
        });
    }
    let (tag, rest) = match raw.find('[') {
        Some(p) => (&raw[..p], &raw[p..]),
        None => (raw, ""),
    };
    let test = if tag.trim() == "*" {
        Test::Any
    } else {
        Test::Tag(tag.trim().to_string())
    };
    // 逐个抠 [ ... ]（谓词内可含引号）
    let mut preds = Vec::new();
    let mut rest = rest;
    while let Some(st) = rest.find('[') {
        let mut depth = 0usize;
        let mut q: Option<char> = None;
        let mut end = st;
        for (i, c) in rest[st..].char_indices() {
            if let Some(qc) = q {
                if c == qc {
                    q = None;
                }
            } else {
                match c {
                    '\'' | '"' => q = Some(c),
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            end = st + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        if q.is_some() || depth != 0 {
            return Err(format!("谓词括号不闭合: {raw}"));
        }
        preds.push(parse_pred(&rest[st + 1..end])?);
        rest = &rest[end + 1..];
    }
    Ok(Step { axis, test, preds })
}

/// 解析整条 XPath 为**联合分支**：顶层 `|`（引号/括号外）把表达式切成多条
/// 供选路径（ezdmw chapterResult 用 4 个分支取不同线路的选集链接）。
/// 每条分支内部按 `.` 开头 = 相对上下文求值。
///
/// [lc-1624] 空分支（如末尾多写 `|`）直接忽略 —— "能解析就解析"；只有整条都没有
/// 可解析分支时才报错。
fn parse_xpath(expr: &str) -> Result<Vec<Vec<Step>>, String> {
    let mut branches: Vec<String> = Vec::new();
    let mut depth = 0i32;
    let mut q: Option<char> = None;
    let mut start = 0usize;
    for (i, c) in expr.char_indices() {
        if let Some(qc) = q {
            if c == qc {
                q = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => q = Some(c),
            '[' => depth += 1,
            ']' => depth -= 1,
            '|' if depth == 0 => {
                branches.push(expr[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    if q.is_some() || depth != 0 {
        return Err(format!("XPath 括号/引号不闭合: {expr}"));
    }
    branches.push(expr[start..].trim().to_string());
    let mut out: Vec<Vec<Step>> = Vec::new();
    for b in &branches {
        if b.is_empty() {
            continue; // 空联合分支忽略（末尾 / 多余的 |）
        }
        out.push(parse_single_xpath(b)?);
    }
    if out.is_empty() {
        return Err(format!("XPath「{expr}」为空（没有任何可解析的分支）"));
    }
    Ok(out)
}

/// 解析单条分支（`.` 开头 = 相对上下文求值）。
///
/// [lc-1624] **空步进容错**（这是「空 XPath 步进」报错的根因）：
/// 社区规则里出现过 `chapterRoads = "//"`（内置 aafun 就是），以及末尾 / 相邻多写 `/`
/// 的笔误（`//div//`、`//a///b`）。原实现把空段丢给 `parse_step` 直接报
/// 「空 XPath 步进」，于是**整个源**的选集/搜索链路全挂。现在：
///   · 相邻或末尾的空段直接忽略（等价于少写一个 `/`）；
///   · 若最终一个步进都没有（`//` / `/` / `.`），表示「上下文节点本身」，返回空步进列表
///     （求值即原样返回上下文 —— aafun 的 `//` 正是这个语义：以根为唯一"线路"）。
fn parse_single_xpath(expr: &str) -> Result<Vec<Step>, String> {
    let mut s = expr.trim();
    let mut relative = false;
    if let Some(rest) = s.strip_prefix('.') {
        relative = true;
        s = rest;
    }
    let mut steps = Vec::new();
    let mut i = 0usize;
    while i < s.len() {
        let axis = if s[i..].starts_with("//") {
            i += 2;
            Axis::Desc
        } else if s[i..].starts_with('/') {
            i += 1;
            Axis::Child
        } else if steps.is_empty() && relative {
            // ".tag" 形态不合法，但 ".//a" 已在上面被剥成 "//a" / "/a"，这里兜住
            Axis::Child
        } else {
            return Err(format!(
                "XPath「{expr}」语法错误：第 {} 个字符处缺少起始 /（步进之间只能由 / 或 // 分隔）",
                i + 1
            ));
        };
        // 抠一个步进（遇到深度 0 且不在引号内的 '/' 停）
        let start = i;
        let mut depth = 0i32;
        let mut q: Option<char> = None;
        while i < s.len() {
            let c = s[i..].chars().next().unwrap();
            if let Some(qc) = q {
                if c == qc {
                    q = None;
                }
            } else {
                match c {
                    '\'' | '"' => q = Some(c),
                    '[' => depth += 1,
                    ']' => depth -= 1,
                    '/' if depth == 0 => break,
                    _ => {}
                }
            }
            i += c.len_utf8();
        }
        if q.is_some() || depth != 0 {
            return Err(format!("XPath「{expr}」谓词括号/引号不闭合"));
        }
        let seg = s[start..i].trim();
        if seg.is_empty() {
            // 相邻 / 末尾多写的分隔符：忽略这一段（能解析就解析）
            continue;
        }
        steps.push(parse_step(seg, axis)?);
    }
    Ok(steps)
}

// ── 求值（基于 scraper 的 html5ever DOM）──

use ego_tree::NodeId;
use scraper::Html;

enum EvalOut {
    Nodes(Vec<NodeId>),
    Str(Vec<String>),
}

fn name_of(doc: &Html, id: NodeId) -> String {
    match doc.tree.get(id).map(|n| n.value()) {
        Some(scraper::node::Node::Element(el)) => el.name().to_string(),
        _ => String::new(),
    }
}

fn attr_of(doc: &Html, id: NodeId, name: &str) -> Option<String> {
    match doc.tree.get(id).map(|n| n.value()) {
        Some(scraper::node::Node::Element(el)) => el.attr(name).map(|s| s.to_string()),
        _ => None,
    }
}

fn text_of(doc: &Html, id: NodeId) -> String {
    let mut s = String::new();
    if let Some(node) = doc.tree.get(id) {
        for n in node.descendants() {
            if let scraper::node::Node::Text(t) = n.value() {
                s.push_str(t);
            }
        }
    }
    s
}

fn apply_preds(doc: &Html, list: &[NodeId], preds: &[Pred]) -> Vec<NodeId> {
    list.iter()
        .enumerate()
        .filter(|(i, id)| {
            let id = **id;
            preds.iter().all(|p| match p {
                Pred::Pos(n) => i + 1 == *n,
                Pred::Eq(a, v) => attr_of(doc, id, a).as_deref() == Some(v.as_str()),
                Pred::Contains(a, v) => attr_of(doc, id, a)
                    .map(|s| s.contains(v.as_str()))
                    .unwrap_or(false),
                Pred::StartsWith(a, v) => attr_of(doc, id, a)
                    .map(|s| s.starts_with(v.as_str()))
                    .unwrap_or(false),
            })
        })
        .map(|(_, id)| *id)
        .collect()
}

fn eval_xpath(doc: &Html, ctx: &[NodeId], branches: &[Vec<Step>]) -> Result<EvalOut, String> {
    let mut out_nodes: Vec<NodeId> = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out_str: Vec<String> = Vec::new();
    let mut any_str = false;
    for steps in branches {
        match eval_single_xpath(doc, ctx, steps)? {
            EvalOut::Nodes(v) => {
                for n in v {
                    if seen.insert(n) {
                        out_nodes.push(n);
                    }
                }
            }
            EvalOut::Str(v) => {
                any_str = true;
                for s in v {
                    if !out_str.contains(&s) {
                        out_str.push(s);
                    }
                }
            }
        }
    }
    Ok(if any_str {
        EvalOut::Str(out_str)
    } else {
        EvalOut::Nodes(out_nodes)
    })
}

fn eval_single_xpath(doc: &Html, ctx: &[NodeId], steps: &[Step]) -> Result<EvalOut, String> {
    let mut cur: Vec<NodeId> = ctx.to_vec();
    for (si, st) in steps.iter().enumerate() {
        match &st.test {
            Test::Attr(a) => {
                if si + 1 != steps.len() {
                    return Err("@attr 只能作为最后一步".into());
                }
                let vals = cur
                    .iter()
                    .filter_map(|id| attr_of(doc, *id, a))
                    .collect::<Vec<_>>();
                return Ok(EvalOut::Str(vals));
            }
            Test::Text => {
                if si + 1 != steps.len() {
                    return Err("text() 只能作为最后一步".into());
                }
                let vals = cur.iter().map(|id| text_of(doc, *id)).collect::<Vec<_>>();
                return Ok(EvalOut::Str(vals));
            }
            Test::Any | Test::Tag(_) => {
                let mut out: Vec<NodeId> = Vec::new();
                let mut seen: HashSet<NodeId> = HashSet::new();
                let tag_of = |cid: NodeId| -> bool {
                    match &st.test {
                        Test::Any => true,
                        Test::Tag(t) => name_of(doc, cid).eq_ignore_ascii_case(t),
                        _ => false,
                    }
                };
                // ⚠ XPath 规范语义：`//div[2]` = /descendant-or-self::node()/child::div[2]
                // —— 谓词按**每个父节点各自的子列表**求值（"每家的第 2 个"），
                // 不是合并成一个大列表后取第 2 个。child 轴天然按上下文分开，这里
                // desc 轴必须同样展开成「descendant-or-self 逐节点走 child 轴」。
                for id in &cur {
                    let Some(node) = doc.tree.get(*id) else { continue };
                    match st.axis {
                        Axis::Desc => {
                            for d in std::iter::once(*id).chain(node.descendants().map(|n| n.id())) {
                                let Some(dn) = doc.tree.get(d) else { continue };
                                let children: Vec<NodeId> = dn
                                    .children()
                                    .filter(|n| n.value().is_element())
                                    .map(|n| n.id())
                                    .filter(|cid| tag_of(*cid))
                                    .collect();
                                for k in apply_preds(doc, &children, &st.preds) {
                                    if seen.insert(k) {
                                        out.push(k);
                                    }
                                }
                            }
                        }
                        Axis::Child => {
                            let children: Vec<NodeId> = node
                                .children()
                                .filter(|n| n.value().is_element())
                                .map(|n| n.id())
                                .filter(|cid| tag_of(*cid))
                                .collect();
                            for k in apply_preds(doc, &children, &st.preds) {
                                if seen.insert(k) {
                                    out.push(k);
                                }
                            }
                        }
                        Axis::SelfAxis => {
                            // 上下文节点本身（要求是元素），谓词按单元素列表求值
                            if node.value().is_element() && tag_of(*id) {
                                let one = [*id];
                                for k in apply_preds(doc, &one, &st.preds) {
                                    if seen.insert(k) {
                                        out.push(k);
                                    }
                                }
                            }
                        }
                        Axis::FollowingSibling => {
                            let sibs: Vec<NodeId> = std::iter::successors(node.next_sibling(), |n| {
                                n.next_sibling()
                            })
                            .filter(|n| n.value().is_element())
                            .map(|n| n.id())
                            .filter(|cid| tag_of(*cid))
                            .collect();
                            for k in apply_preds(doc, &sibs, &st.preds) {
                                if seen.insert(k) {
                                    out.push(k);
                                }
                            }
                        }
                    }
                }
                cur = out;
            }
        }
    }
    Ok(EvalOut::Nodes(cur))
}

fn eval_nodes(doc: &Html, ctx: &[NodeId], expr: &str) -> Result<Vec<NodeId>, String> {
    match eval_xpath(doc, ctx, &parse_xpath(expr)?)? {
        EvalOut::Nodes(v) => Ok(v),
        EvalOut::Str(_) => Err(format!("XPath「{expr}」应定位到节点而不是属性/文本")),
    }
}

// ───────────────────────── 搜索 ─────────────────────────

#[tauri::command]
pub async fn webrs_rules(app: AppHandle) -> Result<Vec<Value>, String> {
    let rules = load_rules(&app).await?;
    Ok(rules
        .iter()
        .map(|r| json!({ "name": r.name, "version": r.version, "mode": r.search_mode }))
        .collect())
}

/// [lc-1501] api 模板请求执行器：url / query / headers / body 字符串值里的占位符
/// 统一替换；GET 把 query 追加进 URL，POST 按 bodyType 走 form / JSON。
async fn api_request(rule: &Rule, req: &ApiRequest, ph: &[(&str, &str)]) -> Result<String, String> {
    let ua = effective_ua(rule);
    let referer = rule_referer(rule);
    let mut url = req.url.clone();
    for (p, r) in ph {
        url = url.replace(p, r);
    }
    if !url.starts_with("http") {
        url = join_url(&rule.base_url, &url).ok_or_else(|| format!("模板 URL 无法拼接: {url}"))?;
    }
    let client = http()?;
    let is_post = req.method.eq_ignore_ascii_case("post");
    let mut builder = if is_post {
        client.post(&url)
    } else {
        let mut u = Url::parse(&url).map_err(|e| format!("URL 解析失败: {e}"))?;
        for (k, v) in req.query_pairs(ph) {
            u.query_pairs_mut().append_pair(&k, &v);
        }
        client.get(u.as_str())
    };
    builder = builder.header("user-agent", &ua).header("referer", &referer);
    // [lc-1606] 带上该规则验证后收获的 cookie（若已通过反爬验证）
    let cookie = cookie_header_for(&rule.name);
    if !cookie.is_empty() {
        builder = builder.header("cookie", cookie);
    }
    for (k, v) in &req.headers {
        if let Ok(hv) = reqwest::header::HeaderValue::from_str(v) {
            builder = builder.header(k.as_str(), hv);
        }
    }
    if is_post {
        if req.body_type.eq_ignore_ascii_case("json") {
            builder = builder.header("content-type", "application/json");
            builder = builder.body(serde_json::to_string(&req.body_json(ph)).unwrap_or_default());
        } else {
            builder = builder.form(&req.query_pairs(ph));
        }
    }
    let resp = builder.send().await.map_err(|e| format!("请求失败 {url}: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("读响应失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("请求返回 {status}: {url}"));
    }
    Ok(text)
}

/// [lc-1606] 检索错误：区分「需要人机验证」与普通错误，供上层触发/回退验证流程。
#[derive(Debug)]
enum SearchError {
    Captcha,
    Other(String),
}

impl From<String> for SearchError {
    fn from(s: String) -> Self {
        SearchError::Other(s)
    }
}

impl From<&str> for SearchError {
    fn from(s: &str) -> Self {
        SearchError::Other(s.to_string())
    }
}

/// [lc-1501] 规则搜索核心（单源）：返回 [{name, src}]。api 模式走 JSONPath 子集，
/// xpath 模式走 HTML 解析。webrs_search / webrs_search_multi 共用。
async fn rule_search(rule: &Rule, keyword: &str) -> Result<Vec<(String, String)>, SearchError> {
    let ua = effective_ua(rule);
    let referer = rule_referer(rule);
    let encoded = urlencoding::encode(keyword).to_string();

    let items: Vec<(String, String)> = if rule.search_mode == "api" {
        let cfg = rule
            .search_api_config
            .clone()
            .ok_or("规则缺少 searchApiConfig")?;
        let body = api_request(rule, &cfg.request, &[("@keyword", keyword)]).await?;
        let v: Value = serde_json::from_str(body.trim())
            .map_err(|e| format!("API 搜索返回的不是 JSON: {e}"))?;
        let list = json_path(&v, &cfg.list_path).ok_or("listPath 解析失败")?;
        let mut out = Vec::new();
        for item in list {
            let name = json_path_str(item, &cfg.name_path).unwrap_or_default();
            let src = json_path_str(item, &cfg.source_path).unwrap_or_default();
            if name.is_empty() || src.is_empty() {
                continue;
            }
            out.push((name, src));
        }
        out
    } else {
        if rule.search_url.is_empty() || rule.search_list.is_empty() {
            return Err("规则缺少 searchURL/searchList".into());
        }
        let url = rule.search_url.replace("@keyword", &encoded);
        let resp = fetch_text(&url, &ua, &referer, &rule.name).await?;
        if !resp.status.is_success() {
            // [lc-1606] 非 2xx 也要看正文：命中反爬挑战 → 上报「需要验证」（对齐 Kazumi）
            if detects_captcha_challenge(&resp, &rule.anti_crawler_config) {
                return Err(SearchError::Captcha);
            }
            return Err(SearchError::Other(format!("请求返回 {}: {url}", resp.status)));
        }
        // [lc-1606] 实测部分站点（如 dalvdm）挑战页也以 HTTP 200 返回 —— 对启用了反爬的
        // 规则，成功响应同样跑一次检测（detectValue/选择器是验证码专用，误判风险低）。
        if detects_captcha_challenge(&resp, &rule.anti_crawler_config) {
            return Err(SearchError::Captcha);
        }
        let doc = Html::parse_document(&resp.body);
        let root = vec![doc.tree.root().id()];
        // [lc-1624] 给 XPath 解析/求值错误带上**字段名**，坏源一眼可查；
        // 错误只影响这一个源（多源聚合里各源独立 catch），不会带崩整条链路。
        let list = eval_nodes(&doc, &root, &rule.search_list)
            .map_err(|e| format!("searchList 选择器失败：{e}"))?;
        let mut out = Vec::new();
        for node in list.into_iter().take(60) {
            let name = match eval_xpath(&doc, &[node], &parse_xpath(&rule.search_name)
                .map_err(|e| format!("searchName 选择器失败：{e}"))?)? {
                EvalOut::Str(v) => v.first().cloned().unwrap_or_default(),
                EvalOut::Nodes(v) => v.first().map(|n| text_of(&doc, *n)).unwrap_or_default(),
            };
            let href = match eval_xpath(&doc, &[node], &parse_xpath(&rule.search_result)
                .map_err(|e| format!("searchResult 选择器失败：{e}"))?)? {
                EvalOut::Str(v) => v.first().cloned().unwrap_or_default(),
                EvalOut::Nodes(v) => v.first().and_then(|n| attr_of(&doc, *n, "href")).unwrap_or_default(),
            };
            let name = name.trim().to_string();
            if name.is_empty() {
                continue;
            }
            let Some(full) = join_url(&rule.base_url, &href) else {
                continue;
            };
            out.push((name, full));
        }
        out
    };
    Ok(items)
}

/// [lc-1500] 单源搜索（保留给单源场景 / 调试）。
#[tauri::command]
pub async fn webrs_search(app: AppHandle, rule_name: String, keyword: String) -> Result<Value, String> {
    let rules = load_rules(&app).await?;
    let rule = find_rule(&rules, &rule_name)?.clone();
    let keyword = keyword.trim().to_string();
    if keyword.is_empty() {
        return Err("搜索词为空".into());
    }
    let items = match rule_search(&rule, &keyword).await {
        Ok(v) => v,
        Err(SearchError::Captcha) => {
            return Err("该源需要人机验证，请在前端完成验证后重试".into())
        }
        Err(SearchError::Other(e)) => return Err(e),
    };
    if items.is_empty() {
        return Err("没有搜到结果，换个源或换关键词试试".into());
    }
    Ok(json!(items
        .into_iter()
        .map(|(name, url)| json!({ "name": name, "url": url }))
        .collect::<Vec<_>>()))
}

/// [lc-1501] 多源聚合搜索（对齐 Kazumi「播放来源」）：并发跑**全部**规则，
/// 按源分组回传 status：ok（有结果）/ empty（无结果）/ error（检索失败，带 error 文案）。
#[tauri::command]
pub async fn webrs_search_multi(
    app: AppHandle,
    keyword: String,
    rules: Option<Vec<String>>,
) -> Result<Value, String> {
    let keyword = keyword.trim().to_string();
    if keyword.is_empty() {
        return Err("搜索词为空".into());
    }
    // [lc-1502] 只搜用户勾选的源；不传 = 全部（兜底）
    let all = load_rules(&app).await?;
    let picked: Vec<Rule> = match &rules {
        Some(names) if !names.is_empty() => all
            .into_iter()
            .filter(|r| names.iter().any(|n| n.eq_ignore_ascii_case(&r.name)))
            .collect(),
        _ => all,
    };
    let rules = picked;
    if rules.is_empty() {
        return Err("没有已启用的源 —— 点 ⚙ 勾选要用的源".into());
    }
    let mut set: tokio::task::JoinSet<Value> = tokio::task::JoinSet::new();
    for rule in rules {
        let keyword = keyword.clone();
        set.spawn(async move {
            let name = rule.name.clone();
            match rule_search(&rule, &keyword).await {
                Ok(items) if items.is_empty() => json!({ "name": name, "status": "empty", "error": "", "items": [] }),
                Ok(items) => json!({
                    "name": name,
                    "status": "ok",
                    "error": "",
                    "items": items
                        .into_iter()
                        .take(20)
                        .map(|(n, u)| json!({ "name": n, "url": u }))
                        .collect::<Vec<_>>()
                }),
                // [lc-1606] 需要人机验证：单独 status，前端据此弹出验证流程
                Err(SearchError::Captcha) => json!({ "name": name, "status": "captcha", "error": "需要人机验证", "items": [] }),
                Err(SearchError::Other(e)) => json!({ "name": name, "status": "error", "error": e, "items": [] }),
            }
        });
    }
    let mut groups = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(v) = res {
            groups.push(v);
        }
    }
    groups.sort_by(|a, b| {
        let sa = a.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let sb = b.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let ka = if sa == "ok" { 0 } else { 1 };
        let kb = if sb == "ok" { 0 } else { 1 };
        ka.cmp(&kb).then_with(|| {
            a.get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .cmp(b.get("name").and_then(|n| n.as_str()).unwrap_or(""))
        })
    });
    Ok(json!(groups))
}

/// [lc-1501] 用户加源：粘贴 Kazumi 规则 JSON → 校验 → 落盘 webrs-rules/{name}.json。
/// 返回规则名。同名已存在则报错（先删再加）。
#[tauri::command]
pub async fn webrs_add_rule(app: AppHandle, json_text: String) -> Result<String, String> {
    let v: Value = serde_json::from_str(json_text.trim())
        .map_err(|e| format!("不是合法 JSON: {e}"))?;
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("规则缺少 name 字段".into());
    }
    serde_json::from_value::<Rule>(v.clone())
        .map_err(|e| format!("规则字段不合法: {e}"))?;
    let dir = rules_dir(&app)?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("创建规则目录失败: {e}"))?;
    let p = dir.join(format!("{name}.json"));
    if p.exists() {
        return Err(format!("规则「{name}」已存在，先删掉再加"));
    }
    tokio::fs::write(&p, serde_json::to_string_pretty(&v).unwrap_or_default())
        .await
        .map_err(|e| format!("写入规则失败: {e}"))?;
    crate::libmpv::trace(&format!("[webrs] 新增规则: {name}"));
    Ok(name)
}

/// [lc-1501] 用户删源。内置源删掉后下次启动会被补种（等同"恢复默认"）。
#[tauri::command]
pub async fn webrs_remove_rule(app: AppHandle, rule_name: String) -> Result<(), String> {
    let name = rule_name.trim().to_string();
    if name.is_empty() || name.contains(['/', '\\', ':']) || name.contains("..") {
        return Err("规则名不合法".into());
    }
    let p = rules_dir(&app)?.join(format!("{name}.json"));
    match tokio::fs::remove_file(&p).await {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("删除失败: {e}")),
    }
}

// ───────────────────────── [lc-1608] 在线规则目录 + 一键更新 ─────────────────────────
//
// 对齐 Kazumi 的 plugin catalog（`plugin_catalog_api.dart` / `plugins_controller.dart`）：
//   · `index.json` 是目录（每条 = name + version + …），`{name}.json` 是规则全文；
//   · 上游仓库 = Predidit/KazumiRules，端点 raw.githubusercontent.com；
//   · 版本比较走「按 `.` 切段逐段比整数」，远端更大才算「可更新」（Kazumi `needUpdate`）。
// 与 Kazumi 的差异（为什么这么选）：
//   · Kazumi 另有镜像 `raw.gitcode.com`（`pluginShopMirror`）。国内直连 raw 常不通，
//     所以这里**直连失败自动回退镜像**——不引入任何密钥，也不依赖我们自建的 gh 镜像
//     （那个 `/raw/*` 走签名轨，需要 FNTV_GH_SECRET，本项目没有也不该为此新增密钥）。
//   · 目录带 6 小时缓存（内存 + 落盘），`force=true` 跳过 TTL —— 面板「检查更新」默认用缓存。

/// 规则目录上游（Kazumi 同款）。raw 直连优先，失败回退 gitcode 镜像。
const CATALOG_BASE: &str = "https://raw.githubusercontent.com/Predidit/KazumiRules/main/";
const CATALOG_BASE_MIRROR: &str = "https://raw.gitcode.com/gh_mirrors/ka/KazumiRules/raw/main/";
/// 目录缓存 TTL（内存 / 落盘共用）。6 小时 —— 规则更新以「天」为节奏，够新。
const CATALOG_TTL_SECS: u64 = 6 * 3600;

/// [lc-1608] 目录条目（逐字段对齐 Kazumi `PluginHTTPItem`；缺字段宽容、多余字段忽略）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogItem {
    pub name: String,
    pub version: String,
    #[serde(rename = "useNativePlayer")]
    pub use_native_player: bool,
    pub author: String,
    #[serde(rename = "lastUpdate")]
    pub last_update: i64,
    #[serde(rename = "antiCrawlerEnabled")]
    pub anti_crawler_enabled: bool,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// [lc-1608] 版本比较：远端是否比本地**更新**（对齐 Kazumi `utils/version.dart` 的 needUpdate）。
/// 按 `.` 切段，逐段转整数比较，缺段补 0；某段不是整数（如 `1.0-beta`）→ 退回「整体字符串不等」。
/// 返回 true 仅当 remote 严格大于 local。
fn remote_is_newer(local: &str, remote: &str) -> bool {
    let l = local.trim();
    let r = remote.trim();
    let ls: Vec<&str> = l.split('.').collect();
    let rs: Vec<&str> = r.split('.').collect();
    let n = ls.len().max(rs.len());
    for i in 0..n {
        // 缺段补 0；本段存在但不是整数 → 无法按段比，回退字符串判等
        let a = if i < ls.len() {
            match ls[i].trim().parse::<i64>() {
                Ok(v) => v,
                Err(_) => return l != r,
            }
        } else {
            0
        };
        let b = if i < rs.len() {
            match rs[i].trim().parse::<i64>() {
                Ok(v) => v,
                Err(_) => return l != r,
            }
        } else {
            0
        };
        if b > a {
            return true;
        }
        if b < a {
            return false;
        }
    }
    false
}

/// 内存层缓存：`(拉取时刻秒, 条目)`。进程内复用，避免反复打网络。
static CATALOG_MEM: OnceLock<std::sync::Mutex<Option<(u64, Vec<CatalogItem>)>>> = OnceLock::new();

fn catalog_mem() -> &'static std::sync::Mutex<Option<(u64, Vec<CatalogItem>)>> {
    CATALOG_MEM.get_or_init(|| std::sync::Mutex::new(None))
}

/// [lc-1624] 取目录缓存锁，**容忍中毒**：原写法 `.lock().unwrap()` 在别的线程持锁
/// panic 过一次后（毒锁）会连锁 panic —— 在 `panic = "abort"` 下即整进程崩溃。
/// 目录缓存只是加速用的旁路数据，毒锁时直接取回内部值继续用，绝不 panic。
fn catalog_mem_guard() -> std::sync::MutexGuard<'static, Option<(u64, Vec<CatalogItem>)>> {
    catalog_mem().lock().unwrap_or_else(|e| e.into_inner())
}

/// 落盘缓存路径：app_data_dir/webrs-catalog.json（与 config.json 同级，非仓库内文件）。
fn catalog_cache_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {e}"))?
        .join("webrs-catalog.json"))
}

async fn read_disk_catalog(app: &AppHandle) -> Option<(u64, Vec<CatalogItem>)> {
    let p = catalog_cache_path(app).ok()?;
    let s = tokio::fs::read_to_string(&p).await.ok()?;
    let v: Value = serde_json::from_str(&s).ok()?;
    let at = v.get("fetchedAt").and_then(|x| x.as_u64()).unwrap_or(0);
    let items: Vec<CatalogItem> = serde_json::from_value(v.get("items")?.clone()).ok()?;
    Some((at, items))
}

async fn write_disk_catalog(app: &AppHandle, items: &[CatalogItem]) {
    let Ok(p) = catalog_cache_path(app) else {
        return;
    };
    let body = json!({ "fetchedAt": now_secs(), "items": items });
    let _ = tokio::fs::write(&p, serde_json::to_string(&body).unwrap_or_default()).await;
}

/// 拉目录里的一个文件（`index.json` 或 `{name}.json`）：raw 直连优先，失败回退镜像。
/// 两个端点都不需要密钥；任一成功即返回，全失败带最后一次错误。
async fn fetch_catalog_file(path: &str) -> Result<String, String> {
    let client = http()?;
    let mut last = String::from("未知错误");
    for base in [CATALOG_BASE, CATALOG_BASE_MIRROR] {
        let url = format!("{base}{path}");
        match client.get(&url).send().await {
            Ok(r) => {
                let st = r.status();
                match r.text().await {
                    Ok(t) if st.is_success() => return Ok(t),
                    Ok(_) => last = format!("HTTP {st}（{url}）"),
                    Err(e) => last = format!("读响应失败: {e}（{url}）"),
                }
            }
            Err(e) => last = format!("请求失败: {e}（{url}）"),
        }
    }
    Err(format!("规则目录拉取失败（直连与镜像均不可用）：{last}"))
}

/// 解析目录 JSON（对齐 Kazumi `parsePluginList`：坏条目跳过；整体为空则报错）。
fn parse_catalog(text: &str) -> Result<Vec<CatalogItem>, String> {
    let arr: Vec<Value> =
        serde_json::from_str(text.trim()).map_err(|e| format!("目录不是合法 JSON 数组: {e}"))?;
    let mut items = Vec::new();
    for v in &arr {
        match serde_json::from_value::<CatalogItem>(v.clone()) {
            Ok(it) if !it.name.trim().is_empty() => items.push(it),
            _ => {} // 缺 name 等坏条目跳过（对应 Kazumi 的 skippedItems）
        }
    }
    if !arr.is_empty() && items.is_empty() {
        return Err("规则目录里没有可用条目".into());
    }
    Ok(items)
}

/// 目录取数（三级：内存缓存 → 落盘缓存 → 网络），`force` 跳过前两级。
async fn catalog_items(app: &AppHandle, force: bool) -> Result<Vec<CatalogItem>, String> {
    if !force {
        if let Some((at, items)) = catalog_mem_guard().as_ref() {
            if now_secs().saturating_sub(*at) < CATALOG_TTL_SECS {
                return Ok(items.clone());
            }
        }
        if let Some((at, items)) = read_disk_catalog(app).await {
            if now_secs().saturating_sub(at) < CATALOG_TTL_SECS {
                *catalog_mem_guard() = Some((at, items.clone()));
                return Ok(items);
            }
        }
    }
    let text = fetch_catalog_file("index.json").await?;
    let items = parse_catalog(&text)?;
    *catalog_mem_guard() = Some((now_secs(), items.clone()));
    write_disk_catalog(app, &items).await;
    Ok(items)
}

/// [lc-1608] 拉在线规则目录（`index.json`），返回 `{ items: [...] }`。带 6 小时缓存。
#[tauri::command]
pub async fn webrs_catalog(app: AppHandle, force: Option<bool>) -> Result<Value, String> {
    let items = catalog_items(&app, force.unwrap_or(false)).await?;
    Ok(json!({ "items": items }))
}

/// 规则名合法性（防路径穿越）——与 `webrs_remove_rule` 同口径。
fn safe_rule_name(name: &str) -> Result<String, String> {
    let n = name.trim().to_string();
    if n.is_empty() || n.contains(['/', '\\', ':']) || n.contains("..") {
        return Err("规则名不合法".into());
    }
    Ok(n)
}

/// 把一份规则 JSON 写进规则目录。`overwrite=false` 时已存在即报错（安装语义）；
/// `overwrite=true` 直接覆盖（更新语义）。抽成独立函数便于单测（用临时目录、不触网）。
async fn write_rule_file(
    dir: &std::path::Path,
    name: &str,
    v: &Value,
    overwrite: bool,
) -> Result<Value, String> {
    let remote_name = v
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if remote_name.is_empty() {
        return Err("规则缺少 name 字段".into());
    }
    // 名称一致性（对齐 Kazumi `_preparePluginUpdate`）：防目录条目与下载内容对不上
    if !remote_name.eq_ignore_ascii_case(name) {
        return Err(format!("下载的规则名「{remote_name}」与请求的「{name}」不一致"));
    }
    let rule: Rule =
        serde_json::from_value(v.clone()).map_err(|e| format!("规则字段不合法: {e}"))?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("创建规则目录失败: {e}"))?;
    let p = dir.join(format!("{name}.json"));
    if p.exists() && !overwrite {
        return Err(format!("规则「{name}」已存在，请改用更新"));
    }
    // 原文落盘（Value 序列化），不经过 Rule 结构体 —— 免得丢掉未建模的字段
    tokio::fs::write(&p, serde_json::to_string_pretty(v).unwrap_or_default())
        .await
        .map_err(|e| format!("写入规则失败: {e}"))?;
    crate::libmpv::trace(&format!(
        "[webrs] {}规则: {name} v{}",
        if overwrite { "更新" } else { "安装" },
        rule.version
    ));
    Ok(json!({ "name": name, "version": rule.version }))
}

/// 下载 `{name}.json` 并落盘（`overwrite` 决定安装 / 更新语义）。
async fn fetch_and_write_rule(app: &AppHandle, name: &str, overwrite: bool) -> Result<Value, String> {
    let name = safe_rule_name(name)?;
    let text = fetch_catalog_file(&format!("{name}.json")).await?;
    let v: Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("规则不是合法 JSON: {e}"))?;
    let dir = rules_dir(app)?;
    write_rule_file(&dir, &name, &v, overwrite).await
}

/// [lc-1608] 安装一条在线规则（写进规则目录，已存在则报错）。
#[tauri::command]
pub async fn webrs_install_rule(app: AppHandle, name: String) -> Result<Value, String> {
    fetch_and_write_rule(&app, &name, false).await
}

/// [lc-1608] 更新一条在线规则（下载并**覆盖**本地同名规则）。
#[tauri::command]
pub async fn webrs_update_rule(app: AppHandle, name: String) -> Result<Value, String> {
    fetch_and_write_rule(&app, &name, true).await
}

/// [lc-1608] 本地规则（含内置种入）vs 目录版本 → 可更新列表 `{ items: [{name, localVersion, remoteVersion}] }`。
/// 判据 = Kazumi `_remoteIsNewer`（远端严格更新才算）。
#[tauri::command]
pub async fn webrs_check_updates(app: AppHandle) -> Result<Value, String> {
    let items = catalog_items(&app, false).await?;
    let local = load_rules(&app).await?;
    let mut local_map: HashMap<String, String> = HashMap::new();
    for r in &local {
        local_map.insert(r.name.to_lowercase(), r.version.clone());
    }
    let mut out = Vec::new();
    for it in &items {
        if let Some(lv) = local_map.get(&it.name.to_lowercase()) {
            if remote_is_newer(lv, &it.version) {
                out.push(json!({
                    "name": it.name,
                    "localVersion": lv,
                    "remoteVersion": it.version,
                }));
            }
        }
    }
    Ok(json!({ "items": out }))
}

fn json_path<'a>(root: &'a Value, path: &str) -> Option<Vec<&'a Value>> {
    // 支持 `$.a.b[*]`、`$[*]`（根数组，xfdmnext 用）与 `$`（原样）
    let mut p = path.trim().strip_prefix('$')?;
    if let Some(rest) = p.strip_prefix('.') {
        p = rest;
    }
    let mut cur: Vec<&Value> = vec![root];
    if p.is_empty() {
        return Some(cur);
    }
    for seg in p.split('.') {
        let (key, wildcard) = match seg.strip_suffix("[*]") {
            Some(k) => (k, true),
            None => (seg, false),
        };
        let mut next = Vec::new();
        for v in &cur {
            // key 为空 = 通配作用在当前值本身（`$[*]` 首段）
            let targets: Vec<&Value> = if key.is_empty() {
                vec![*v]
            } else {
                v.get(key).into_iter().collect()
            };
            for hit in targets {
                if wildcard {
                    match hit.as_array() {
                        Some(arr) => next.extend(arr.iter()),
                        // 通配作用在非数组上：部分 API 单条返回对象，容错当单元素
                        None => next.push(hit),
                    }
                } else {
                    next.push(hit);
                }
            }
        }
        cur = next;
    }
    Some(cur)
}

fn json_path_str(v: &Value, path: &str) -> Option<String> {
    let vals = json_path(v, path)?;
    match vals.first()? {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => {
            // sorani 的 episodeOrder 是 1.0 这类浮点：整数值渲染成 "1"（拼 episodePage
            // 模板时 "/episode/1.0" 会 404；对齐 Dart int/dart 的直觉输出）
            if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f.abs() < 9e15 {
                    return Some(format!("{}", f as i64));
                }
            }
            Some(n.to_string())
        }
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

// ───────────────────────── 线路 / 选集 ─────────────────────────

/// [lc-1500] 详情页 → 线路 + 选集（对齐 Kazumi parseChapters：
/// chapterRoads 每个节点是一条线路，chapterResult 相对求值出每集的 href + 文本）。
#[tauri::command]
pub async fn webrs_chapters(app: AppHandle, rule_name: String, url: String) -> Result<Value, String> {
    let rules = load_rules(&app).await?;
    let rule = find_rule(&rules, &rule_name)?.clone();

    // ── api 选集（sorani / xfdmnext）：模板请求拿 JSON，roadsPath/episodesPath 逐层抽取 ──
    if rule.chapter_mode == "api" {
        let cfg = rule
            .chapter_api_config
            .clone()
            .ok_or("规则缺少 chapterApiConfig")?;
        if cfg.format.eq_ignore_ascii_case("delimited") {
            return Err("该规则使用 delimited 打包格式，暂不支持".into());
        }
        let src_id = url.trim().to_string();
        if src_id.is_empty() {
            return Err("缺少资源 id".into());
        }
        let body = api_request(&rule, &cfg.request, &[("@source", &src_id)]).await?;
        let v: Value = serde_json::from_str(body.trim())
            .map_err(|e| format!("API 选集返回的不是 JSON: {e}"))?;
        let roads_raw = json_path(&v, &cfg.roads_path).ok_or("roadsPath 解析失败")?;
        let mut roads: Vec<Value> = Vec::new();
        for road in roads_raw {
            let rname = if cfg.road_name_path.trim().is_empty() {
                String::new()
            } else {
                json_path_str(road, &cfg.road_name_path).unwrap_or_default()
            };
            let eps_raw = json_path(road, &cfg.episodes_path).unwrap_or_default();
            let mut eps: Vec<Value> = Vec::new();
            for ep in eps_raw {
                let ename = json_path_str(ep, &cfg.episode_name_path).unwrap_or_default();
                let eurl = json_path_str(ep, &cfg.episode_url_path).unwrap_or_default();
                if eurl.is_empty() {
                    continue;
                }
                // episodePage 模板：@source / @episodeUrl → 播放页（后面 resolve 阶段嗅探直链）
                let page = cfg
                    .episode_page
                    .url
                    .replace("@source", &src_id)
                    .replace("@episodeUrl", &eurl);
                if page.is_empty() {
                    continue;
                }
                let name = if ename.is_empty() {
                    format!("第{}集", eps.len() + 1)
                } else {
                    ename
                };
                eps.push(json!({ "name": name, "url": page }));
            }
            if eps.is_empty() {
                continue;
            }
            let rname = if rname.is_empty() {
                format!("线路 {}", roads.len() + 1)
            } else {
                rname
            };
            roads.push(json!({ "name": rname, "index": roads.len(), "episodes": eps }));
        }
        if roads.is_empty() {
            return Err("没解析到播放线路（api 选集）".into());
        }
        return Ok(json!({ "roads": roads, "pageUrl": url }));
    }

    if rule.chapter_mode != "xpath" {
        return Err(format!(
            "规则「{}」的选集模式是 {}，本版本只支持 xpath/api",
            rule.name, rule.chapter_mode
        ));
    }
    if rule.chapter_roads.is_empty() || rule.chapter_result.is_empty() {
        return Err("规则缺少 chapterRoads/chapterResult".into());
    }
    let ua = effective_ua(&rule);
    let referer = rule_referer(&rule);
    let page_url = join_url(&rule.base_url, &url).ok_or("详情页 URL 无效")?;
    // [lc-1606] 带上验证 cookie；非 2xx 直接报错（选集页一般不落挑战页）
    let resp = fetch_text(&page_url, &ua, &referer, &rule.name).await?;
    if !resp.status.is_success() {
        return Err(format!("请求返回 {}: {page_url}", resp.status));
    }
    let doc = Html::parse_document(&resp.body);
    let root = vec![doc.tree.root().id()];
    // [lc-1624] 字段名进错误文案；坏源只影响自己
    let road_nodes = eval_nodes(&doc, &root, &rule.chapter_roads)
        .map_err(|e| format!("chapterRoads 选择器失败：{e}"))?;
    if road_nodes.is_empty() {
        return Err(
            "没解析到播放线路 —— 该站选集很可能由脚本动态加载（如 baimao），HTTP 拿不到，换个源试试".into(),
        );
    }
    let mut roads: Vec<Value> = Vec::new();
    for (ri, road) in road_nodes.into_iter().enumerate() {
        let eps_ctx = vec![road];
        let ep_steps = match parse_xpath(&rule.chapter_result) {
            Ok(s) => s,
            Err(e) => {
                crate::libmpv::trace(&format!("[webrs] chapterResult 选择器失败: {e}"));
                continue;
            }
        };
        let ep_nodes = match eval_xpath(&doc, &eps_ctx, &ep_steps)? {
            EvalOut::Nodes(v) => v,
            EvalOut::Str(_) => continue,
        };
        let mut eps: Vec<Value> = Vec::new();
        for (ei, ep) in ep_nodes.into_iter().enumerate().take(500) {
            let Some(href) = attr_of(&doc, ep, "href") else {
                continue;
            };
            let Some(full) = join_url(&page_url, &href) else {
                continue;
            };
            let mut name = text_of(&doc, ep);
            name = name.split_whitespace().collect::<Vec<_>>().join("");
            if name.is_empty() {
                name = format!("第{}集", ei + 1);
            }
            eps.push(json!({ "name": name, "url": full }));
        }
        if eps.is_empty() {
            continue;
        }
        roads.push(json!({
            "name": format!("线路 {}", roads.len() + 1),
            "index": ri,
            "episodes": eps,
        }));
    }
    if roads.is_empty() {
        return Err("线路里没有可用选集（chapterResult 不匹配？）".into());
    }
    Ok(json!({ "roads": roads, "pageUrl": page_url }))
}

// ───────────────────────── 直链解析 ─────────────────────────

static RE_MEDIA: OnceLock<regex::Regex> = OnceLock::new();
static RE_REL: OnceLock<regex::Regex> = OnceLock::new();

fn media_regex() -> &'static regex::Regex {
    RE_MEDIA.get_or_init(|| {
        regex::Regex::new(r#"https?://[^\s"'<>\\]+?\.(?:m3u8|mp4|flv)(?:\?[^\s"'<>\\]*)?"#).unwrap()
    })
}

fn rel_regex() -> &'static regex::Regex {
    RE_REL.get_or_init(|| {
        regex::Regex::new(r#"["'](/[^\s"'<>\\]{4,200}?\.m3u8(?:\?[^\s"'<>\\]{0,200})?)["']"#).unwrap()
    })
}

/// 页面源码正则抽直链（多数站的播放器配置明文写在 JS 里）。m3u8 优先，其次 mp4/flv。
fn extract_from_html(html: &str, page_url: &str) -> Option<String> {
    // 常见转义（\/ → /）先还原
    let unescaped = html.replace("\\/", "/");
    let mut m3u8: Option<String> = None;
    let mut other: Option<String> = None;
    for m in media_regex().find_iter(&unescaped) {
        let u = m.as_str().to_string();
        if u.contains(".m3u8") && m3u8.is_none() {
            m3u8 = Some(u);
        } else if m3u8.is_none() && other.is_none() {
            other = Some(u);
        }
    }
    if m3u8.is_none() {
        // 站内相对路径的 m3u8
        if let Some(m) = rel_regex().captures(&unescaped) {
            if let Some(full) = join_url(page_url, m.get(1).unwrap().as_str()) {
                m3u8 = Some(full);
            }
        }
    }
    m3u8.or(other)
}

/// [lc-1500] 播放页 → 直链。先 HTTP 正则，没命中走隐藏 WebView 嗅探（Kazumi 同款 JS 钩子）。
/// 返回 {url, referer, ua, via}。
#[tauri::command]
pub async fn webrs_resolve(
    app: AppHandle,
    rule_name: String,
    url: String,
) -> Result<Value, String> {
    let rules = load_rules(&app).await?;
    let rule = find_rule(&rules, &rule_name)?.clone();
    let ua = effective_ua(&rule);
    let referer = rule_referer(&rule);
    let page_url = join_url(&rule.base_url, &url).ok_or("播放页 URL 无效")?;
    // [lc-1606] useLegacyParser：该站的播放器藏在 iframe 里，外层页拿不到直链
    let legacy = rule.use_legacy_parser;

    crate::libmpv::trace(&format!("[webrs] resolve: {page_url}"));

    // ── 1) HTTP 直解 ──
    match fetch_text(&page_url, &ua, &referer, &rule.name).await {
        Ok(resp) if resp.status.is_success() => {
            if let Some(hit) = extract_from_html(&resp.body, &page_url) {
                crate::libmpv::trace(&format!("[webrs] HTTP 命中: {hit}"));
                return Ok(json!({ "url": hit, "referer": referer, "ua": ua, "via": "http" }));
            }
            // [lc-1606] legacy：外层页没有直链时，取第一个 iframe[src] 再解一次
            if legacy {
                if let Some(inner) = first_iframe_src(&resp.body, &page_url) {
                    crate::libmpv::trace(&format!("[webrs] legacy iframe: {inner}"));
                    if let Ok(r2) = fetch_text(&inner, &ua, &referer, &rule.name).await {
                        if r2.status.is_success() {
                            if let Some(hit) = extract_from_html(&r2.body, &inner) {
                                return Ok(json!({ "url": hit, "referer": referer, "ua": ua, "via": "legacy" }));
                            }
                        }
                    }
                }
            }
        }
        Ok(resp) => crate::libmpv::trace(&format!(
            "[webrs] 页面返回 {}（转 WebView 嗅探）",
            resp.status
        )),
        Err(e) => crate::libmpv::trace(&format!("[webrs] 页面拉取失败（转 WebView 嗅探）: {e}")),
    }

    // ── 2) WebView 嗅探 ──
    let mut hit = webview_sniff(&app, &page_url, &ua, &referer, legacy).await?;
    // legacy 的嗅探回传可能是 iframe 播放页 URL（非直链）—— 再 HTTP 解一次
    if legacy && !is_media_url(&hit) {
        if let Ok(r) = fetch_text(&hit, &ua, &referer, &rule.name).await {
            if r.status.is_success() {
                if let Some(m) = extract_from_html(&r.body, &hit) {
                    hit = m;
                }
            }
        }
    }
    crate::libmpv::trace(&format!("[webrs] WebView 命中: {hit}"));
    Ok(json!({ "url": hit, "referer": referer, "ua": ua, "via": "webview" }))
}

/// [lc-1606] 是否为直链媒体（m3u8/mp4/flv）。
fn is_media_url(u: &str) -> bool {
    media_regex().is_match(u)
}

/// [lc-1606] 抽页面里第一个 iframe 的 src（useLegacyParser 用，xfdmneo 这类）。
fn first_iframe_src(html: &str, base: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let sel = scraper::Selector::parse("iframe").ok()?;
    for el in doc.select(&sel) {
        if let Some(src) = el.value().attr("src") {
            if let Some(full) = join_url(base, src) {
                if full.starts_with("http") {
                    return Some(full);
                }
            }
        }
    }
    None
}

// ── WebView 嗅探（Kazumi video_webview 的 JS 钩子思路）──

const SNIFF_LABEL: &str = "webrs-sniffer";
const SNIFF_SCHEME: &str = "anibox-sniff";

// ── [lc-1622] 嗅探回传：主通道改「JS 写全局 + Rust 轮询」 ──
//
// 背景（真凶）：注入脚本命中直链后走 `<img src="http://127.0.0.1:<beacon>/sniff?u=…">`
// 回传，但**页面 CSP 只管得住页面自己的资源加载**：强 CSP 站点（如 hhplayer 系）
// 只允许 `img-src 'self' data: https:`，`http://127.0.0.1` 的图片被 CSP + mixed-content
// 双重拦死 → 应用收不到 → 解析失败 → 播不了。fetch/sendBeacon 同样吃 connect-src。
//
// 解法：让注入脚本把结果写进**页面全局** `window.__aniboxSniff = {url,ts}`，Rust 侧用
// 宿主的 `eval_with_callback`（见 eval_json，lc-1606 为验证码做的同一套）**定时读取**。
// `ExecuteScript`/`eval` 是宿主行为，**不受页面 CSP 限制** ✓。
//
// 跨 frame：initialization_script 全 frame 注入。每个 frame 命中后 ① 写本 frame 全局；
// ② 尝试 `window.top.postMessage({__aniboxSniff:u}, '*')` 冒泡给主 frame（跨域 postMessage
// 到 top 是允许的，只要求 top 侧有监听）；③ 主 frame 的 message 监听把值落到自己的
// `window.__aniboxSniff`。Rust 只轮询**主 frame** 即可拿到任意 frame 的命中。
//
// 三通道（取先到）：
//   ① 全局变量 + Rust 轮询 —— 主通道，**不受页面 CSP 限制**（跨域 frame 靠 postMessage 冒泡）；
//   ② 本地 beacon（image/fetch/sendBeacon）—— 页面/iframe 无 CSP 时必达，强 CSP 站点会被拦；
//   ③ anibox-sniff:// 伪导航（on_navigation 拦截）—— 兜底。
fn sniff_script(beacon_port: u16, legacy: bool) -> String {
    r#"(function () {
  // __aniboxSniffInit 防同 frame 重复注入；__aniboxSniff 是给 Rust 轮询读的结果对象
  // （不再是布尔哨兵）。
  if (window.__aniboxSniffInit) return;
  window.__aniboxSniffInit = true;
  var done = false;
  var seen = {};
  var LEGACY = __LEGACY__;
  var BEACON = 'http://127.0.0.1:PORT/sniff?u=';
  // [lc-1622] 主 frame 监听子 frame 冒泡：跨域 iframe 里命中的直链经 postMessage 冒泡上来。
  // （本监听在每个 frame 都会装，但只有主 frame 会真正收到 top 定向的 message。）
  try {
    window.addEventListener('message', function (ev) {
      try {
        var d = ev && ev.data;
        if (d && typeof d.__aniboxSniff === 'string' && d.__aniboxSniff) {
          if (!(window.__aniboxSniff && window.__aniboxSniff.url)) {
            window.__aniboxSniff = { url: d.__aniboxSniff, ts: Date.now() };
          }
        }
      } catch (e) {}
    });
  } catch (e) {}
  // [lc-1622] 统一回传：isRaw=true 时跳过「媒体扩展名 / prestrain」校验（legacy 回传 iframe 页 URL 用）。
  function deliver(u, isRaw) {
    if (!u || typeof u !== 'string' || done) return false;
    if (!/^https?:\/\//i.test(u)) return false;
    var l = u.toLowerCase();
    if (/googleads|googlesyndication|adtrafficquality|doubleclick/.test(l)) return false;
    if (!isRaw && /prestrain\.html|prestrain%2ehtml/.test(l)) return false;
    if (!isRaw && !/\.(m3u8|mp4|flv)(\?|#|$)/i.test(l)) return false;
    if (seen[u]) return false;
    seen[u] = 1;
    done = true;
    // ① 主通道：写本 frame 全局 —— Rust 用宿主 eval 轮询读，**不受页面 CSP 限制**
    try { window.__aniboxSniff = { url: u, ts: Date.now() }; } catch (e) {}
    // ② 冒泡到主 frame（跨域 postMessage 到 top 合法）—— 子 frame 命中靠这条到 top
    try { if (window.top && window.top !== window) window.top.postMessage({ __aniboxSniff: u }, '*'); } catch (e) {}
    var q = encodeURIComponent(u);
    // ③ 次要通道：本地 beacon（页面/iframe 无 CSP 时必达；强 CSP 站点会被拦）
    try { new Image().src = BEACON + q; } catch (e) {}
    try { fetch(BEACON + q, { mode: 'no-cors' }).catch(function () {}); } catch (e) {}
    try { navigator.sendBeacon && navigator.sendBeacon(BEACON + q); } catch (e) {}
    // ④ 兜底：伪导航（on_navigation 拦截 anibox-sniff://）
    var nav = 'anibox-sniff://found?u=' + q;
    try { window.top.location.href = nav; return true; } catch (e) {}
    try { location.href = nav; } catch (e) {}
    return true;
  }
  function emit(u) { return deliver(u, false); }
  // [lc-1606] useLegacyParser：不回传直链，而是回传 iframe 播放页 URL（外层页没有
  // 直链、播放器被塞进 iframe 的站，如 xfdmneo）。Rust 侧拿到后再 HTTP 解一次。
  function emitRaw(u) { return deliver(u, true); }
  // XHR（Kazumi 同款）：open 时按 URL 命中（hls.js 拉 m3u8 清单必经之路），
  // load 后再查响应体是否 #EXTM3U 开头（覆盖无扩展名的动态清单接口）
  try {
    var oo = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (m, u) {
      var self = this, u0 = u;
      try { emit(u0); } catch (e) {}
      this.addEventListener('load', function () {
        try {
          if (self.responseText && self.responseText.trim().indexOf('#EXTM3U') === 0) {
            emit(self.responseURL || u0);
          }
        } catch (e) {}
      });
      return oo.apply(this, arguments);
    };
  } catch (e) {}
  // fetch（Kazumi 同款）：Response.text 响应体 #EXTM3U 开头 → this.url
  try {
    if (window.Response) {
      var ot = Response.prototype.text;
      Response.prototype.text = function () {
        var p = ot.apply(this, arguments);
        var self = this;
        try {
          p.then(function (t) {
            try { if (t && t.trim().indexOf('#EXTM3U') === 0) emit(self.url || ''); } catch (e) {}
          });
        } catch (e) {}
        return p;
      };
    }
  } catch (e) {}
  // video/source 标签扫描 + 轮询兜底（Kazumi 的 timer scan 同款）
  function scan() {
    if (done) return;
    try {
      var els = document.querySelectorAll('video,source');
      for (var i = 0; i < els.length; i++) {
        var s1 = els[i].getAttribute('src') || '';
        if (s1.indexOf('blob:') === 0) continue;
        emit(s1);
        emit(els[i].src || '');
      }
    } catch (e) {}
    try {
      // 页面 JS 里的直链常被写成 https:\/\/…（\/ 转义），先归一化再匹配
      var flat = document.documentElement.innerHTML.split('\\/').join('/');
      var m = flat.match(/https?:\/\/[^"'\\\s<>]{8,300}?\.m3u8(?:\?[^"'\\\s<>]{0,200})?/i);
      if (m) emit(m[0]);
      var m2 = flat.match(/["'](\/[^"'\\\s<>]{4,200}?\.m3u8[^"'\\\s<>]{0,100})["']/i);
      if (m2) emit(location.origin + m2[1]);
    } catch (e) {}
    // [lc-1606] legacy：没有直链时回传 iframe 播放页 URL（Kazumi legacy 分支）
    if (LEGACY) {
      try {
        var ifs = document.querySelectorAll('iframe');
        for (var j = 0; j < ifs.length; j++) {
          var s = ifs[j].getAttribute('src') || '';
          if (s && !/\.(m3u8|mp4|flv)(\?|#|$)/i.test(s)) emitRaw(s);
        }
      } catch (e) {}
    }
  }
  scan();
  var iv = setInterval(function () {
    if (done) { clearInterval(iv); return; }
    scan();
  }, 900);
  setTimeout(function () { clearInterval(iv); }, 60000);
})();
"#
    .replace("PORT", &beacon_port.to_string())
    .replace("__LEGACY__", if legacy { "1" } else { "0" })
}

/// 本地嗅探回传 beacon：嗅探 JS 命中直链后向 `http://127.0.0.1:<port>/sniff?u=<url>`
/// 发请求（image/fetch/sendBeacon 三连），这里收下转交等待者。比伪导航可靠 ——
/// WebView2 对未知 scheme 的 location.href 未必触发顶层 NavigationStarting。
static BEACON_PORT: OnceLock<u16> = OnceLock::new();
static SNIFF_TX: OnceLock<std::sync::Mutex<Option<std::sync::mpsc::Sender<String>>>> = OnceLock::new();

/// [lc-1624] 同一时刻只允许一次嗅探。
///
/// 两次并发解析会：① 抢同一个 `SNIFF_LABEL`，后一次建窗会把前一次的窗 `destroy` 掉；
/// ② 覆盖 `SNIFF_TX` 里的等待者（谁后注册谁生效），先注册的那次从此收不到回传；
/// ③ 先结束的那次 `reset_sniff_waiters()` 又会把后一次的登记清空。结果是两边都拿不到直链、
/// 还各自留一个隐藏窗。用一个进程内闸门把「解析」串行化，第二次直接给可诊断错误。
static SNIFF_ACTIVE: AtomicBool = AtomicBool::new(false);

struct SniffGuard;

impl SniffGuard {
    fn acquire() -> Option<Self> {
        SNIFF_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| SniffGuard)
    }
}

impl Drop for SniffGuard {
    // 无论正常返回还是中途 `?` 早退 / panic 展开，都会复位，不会把闸门永久锁死
    fn drop(&mut self) {
        SNIFF_ACTIVE.store(false, Ordering::SeqCst);
    }
}

fn ensure_beacon() -> Result<u16, String> {
    if let Some(p) = BEACON_PORT.get() {
        return Ok(*p);
    }
    // [lc-1560] 22347 让给弹幕 shim（uosc_danmaku 的 Lua 硬编码该端口，
// 见 main.lua / menu.lua / extra.lua 的 127.0.0.1:22347 调用）——嗅探 beacon
// 从 22348 起找，避免两个本地服务抢同一端口。
    for port in [22348u16, 22349, 22350] {
        let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", port)) else {
            continue;
        };
        let _ = BEACON_PORT.set(port);
        let _ = SNIFF_TX.set(std::sync::Mutex::new(None));
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut sock) = conn else { continue };
                let mut buf = [0u8; 4096];
                let n = std::io::Read::read(&mut sock, &mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                // 请求行：GET /sniff?u=<encoded>&… HTTP/1.1
                if let Some(line) = req.lines().next() {
                    if let Some(path) = line.split_whitespace().nth(1) {
                        if let Some(qs) = path.split_once('?').map(|(_, q)| q) {
                            for kv in qs.split('&') {
                                if let Some(v) = kv.strip_prefix("u=") {
                                    let url = urlencoding::decode(v)
                                        .map(|s| s.to_string())
                                        .unwrap_or_default();
                                    if !url.is_empty() {
                                        if let Some(tx) =
                                            SNIFF_TX.get().and_then(|m| m.lock().ok().and_then(|g| g.clone()))
                                        {
                                            let _ = tx.send(url);
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
                let _ = std::io::Write::write_all(
                    &mut sock,
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        crate::libmpv::trace(&format!("[webrs] 嗅探 beacon 监听 127.0.0.1:{port}"));
        return Ok(port);
    }
    Err("嗅探回传端口（22347-22349）全部被占用".into())
}

// ─────────────── [lc-1613] 隐藏 WebView 创建：对齐主窗口参数 + 失败重试 ───────────────
//
// 根因：主窗口（tauri.conf.json `app.windows[0].additionalBrowserArgs`）设了
// `--autoplay-policy=no-user-gesture-required --disable-features=msWebOOUI,msPdfOOUI`，
// 而本文件追加创建的隐藏窗口（嗅探 / 验证）没设该项，落到 wry 的默认值
// `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`。
//
// WebView2 规则（见 wry `web_context.rs` 注释 + MS 文档）：**同一 user data folder 下
// 若已存在运行中的实例，且两次 Environment 的 CoreWebView2EnvironmentOptions 不一致，
// 则新的 CreateCoreWebView2EnvironmentWithOptions 直接失败**，返回
// ERROR_INVALID_STATE(0x8007139F)「组或资源的状态不是执行请求操作的正确状态」。
// 主窗口是进程里第一个 environment（所以能建），追加窗口 options 不同 ⇒ 每次必挂。
//
// 修法：追加窗口**逐字对齐主窗口的 additionalBrowserArgs**（运行期从配置读，避免硬编码，
// 改了 tauri.conf.json 也不会再错位）。兜底：建窗失败自动换新 label 重试一次；仍失败给出
// 可诊断文案。

/// 主窗口（配置里首个设了 additionalBrowserArgs 的窗口）的浏览器参数。
/// 返回 None 表示主窗口也没设 ⇒ 追加窗口同样保持 None（两边都用 wry 默认值，天然一致）。
fn main_window_browser_args(app: &AppHandle) -> Option<String> {
    app.config()
        .app
        .windows
        .iter()
        .find_map(|w| w.additional_browser_args.clone())
}

/// [lc-1613] 把建窗失败翻译成带排查建议的文案（识别 ERROR_INVALID_STATE 0x8007139F）。
fn describe_webview_error(err: &tauri::Error) -> String {
    let raw = err.to_string();
    let invalid_state = raw.contains("8007139F")
        || raw.contains("8007139f")
        || raw.contains("-2147019873") // 0x8007139F 的有符号 i32 形式
        || raw.contains("正确状态")
        || raw.contains("not in the correct state");
    if invalid_state {
        format!(
            "{raw}\n（WebView2 ERROR_INVALID_STATE：常见于「同一用户数据目录下 Environment \
             选项不一致」或「上次退出残留 WebView2 进程占用用户数据目录」。排查：① 任务管理器\
             结束所有 msedgewebview2.exe 后重开应用；② 到「设置→应用」修复/更新 Microsoft \
             Edge WebView2 Runtime；③ 若装了会拦截进程的安全软件，请放行本应用。）"
        )
    } else {
        raw
    }
}

/// [lc-1613] 在主线程序列上创建隐藏 WebView。
/// `make(app, label)` 负责按给定 label 组装并 build 窗口，最多被调用 2 次；
/// 第 2 次换新 label（`{base}-r1`）以规避上一次残留 / 瞬时状态。
/// 成功返回**实际使用的 label**（调用方据此销毁窗口）；失败返回可诊断错误。
///
/// [lc-1624] 防「孤儿隐藏窗」（这是「点几次播放就崩」的关键成因之一）：
/// 等待方一旦超时（20s）就给 `aborted` 置位；主线程闭包**建成后**把 label 交回等待方，
/// 交不回（等待方已放弃）或已 aborted，就**立刻把刚建的窗销毁**。
/// 否则会出现「等待方已经返回失败、闭包稍后才把窗建出来」——那个窗没有任何人负责销毁，
/// 每次失败都漏一个完整 WebView2 实例，点几次就耗尽资源把进程拖崩。
///
/// 同时去掉原来在主线程里的 `std::thread::sleep(300ms)` 重试退避：主线程就是事件循环，
/// 占住它反而会拖慢其它已排队的建窗 / 销毁，把 20s 超时逼出来、制造孤儿窗；
/// 换 label 本身已经能绕开 0x8007139F 的 EnvironmentOptions 冲突，不需要阻塞主线程。
async fn create_hidden_webview<F>(
    app: &AppHandle,
    base_label: &str,
    make: F,
) -> Result<String, String>
where
    F: Fn(&AppHandle, &str) -> Result<(), tauri::Error> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
    let app2 = app.clone();
    let base = base_label.to_string();
    let aborted = Arc::new(AtomicBool::new(false));
    let aborted2 = aborted.clone();
    app.run_on_main_thread(move || {
        let mut last_err = String::from("创建 WebView 失败");
        for attempt in 0..2u32 {
            let label = if attempt == 0 {
                base.clone()
            } else {
                format!("{base}-r{attempt}")
            };
            // 清掉可能残留的同名窗口（只有此前创建成功过才会有）
            if let Some(w) = app2.get_webview_window(&label) {
                let _ = w.destroy();
            }
            match make(&app2, &label) {
                Ok(()) => {
                    // [lc-1618] 埋点：建窗成功也留痕。
                    // [lc-1624] 但**先**看等待方还要不要这个窗：交不回 / 已放弃 → 立即自毁。
                    let delivered = tx.send(Ok(label.clone())).is_ok();
                    if delivered && !aborted2.load(Ordering::Relaxed) {
                        crate::libmpv::trace(&format!(
                            "[webrs] 隐藏窗创建成功 label={label}（第 {} 次尝试）",
                            attempt + 1
                        ));
                    } else {
                        if let Some(w) = app2.get_webview_window(&label) {
                            let _ = w.destroy();
                        }
                        crate::libmpv::trace(&format!(
                            "[webrs] 隐藏窗创建成功但等待方已放弃 → 立即销毁 label={label}"
                        ));
                    }
                    return;
                }
                Err(e) => {
                    last_err = describe_webview_error(&e);
                    crate::libmpv::trace(&format!(
                        "[webrs] 隐藏窗创建失败 label={label}（第 {} 次尝试）: {last_err}",
                        attempt + 1
                    ));
                }
            }
        }
        let _ = tx.send(Err(last_err));
    })
    .map_err(|e| format!("窗口调度失败: {e}"))?;

    let aborted3 = aborted.clone();
    let res = tokio::task::spawn_blocking(move || {
        match rx.recv_timeout(std::time::Duration::from_secs(20)) {
            Ok(v) => v,
            Err(_) => {
                // 等待方放弃：置位，让主线程闭包若稍后建成就自毁，别留孤儿窗
                aborted3.store(true, Ordering::Relaxed);
                Err("创建 WebView 超时（20s）".to_string())
            }
        }
    })
    .await
    .map_err(|e| format!("等待窗口失败: {e}"))?;

    if res.is_err() {
        // 兜底：两个可能用过的 label 都再 best-effort 清一次（闭包可能刚建好、置位晚一拍）
        for l in [base_label.to_string(), format!("{base_label}-r1")] {
            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Some(w) = app2.get_webview_window(&l) {
                    let _ = w.destroy();
                }
            });
        }
    }
    res
}

/// [lc-1613] 撤销嗅探等待者登记（早退 / 收尾时调用）。
fn reset_sniff_waiters() {
    if let Some(m) = SNIFF_TX.get() {
        if let Ok(mut g) = m.lock() {
            *g = None;
        }
    }
}

/// 隐藏 WebView 加载播放页，等待直链回传。三通道（取先到）：
/// ① [lc-1622] 注入脚本写页面全局 `window.__aniboxSniff`，本函数用宿主 eval 轮询
///    **主 frame** 读取 —— **不受页面 CSP 限制**，强 CSP 站点靠它；跨域 frame 命中的值
///    由脚本 postMessage 冒泡到 top 后落进主 frame 全局。
/// ② 本地 beacon（嗅探 JS 的 image/fetch/sendBeacon 三连，页面/iframe 无 CSP 时必达）
/// ③ 伪导航 anibox-sniff://（on_navigation 拦截，兜底）
/// JS hook 注入全 frame（WebView2 AddScriptToExecuteOnDocumentCreated 覆盖 iframe）。
async fn webview_sniff(
    app: &AppHandle,
    page_url: &str,
    ua: &str,
    _referer: &str,
    legacy: bool,
) -> Result<String, String> {
    use std::sync::mpsc;

    // [lc-1624] 串行化：闸门在函数返回/早退时随 SniffGuard::drop 自动复位
    let _sniff_guard = SniffGuard::acquire()
        .ok_or_else(|| "已有一次直链解析在进行中，请等它结束再试".to_string())?;

    let beacon_port = ensure_beacon()?;
    let (tx, rx) = mpsc::channel::<Result<String, String>>();

    // 等待者登记：beacon（发纯 URL）经转发线程并入统一回传通道
    let (btx, brx) = mpsc::channel::<String>();
    {
        let slot = SNIFF_TX.get().ok_or("beacon 未初始化")?;
        *slot.lock().map_err(|e| e.to_string())? = Some(btx);
    }
    {
        let tx_fwd = tx.clone();
        std::thread::spawn(move || {
            // rx 在等待结束后被 drop → brx 断开 → 本线程自然退出
            while let Ok(u) = brx.recv() {
                if tx_fwd.send(Ok(u)).is_err() {
                    break;
                }
            }
        });
    }

    let page = page_url.to_string();
    let ua_owned = ua.to_string();
    let script = sniff_script(beacon_port, legacy);
    let tx_nav = tx.clone();

    // 目标 URL 无效则直接失败，不必起窗口
    let target = match Url::parse(&page) {
        Ok(u) => u,
        Err(_) => {
            reset_sniff_waiters();
            return Err(format!("播放页 URL 无效: {page}"));
        }
    };

    // 组装建窗闭包（最多调用 2 次，故用 Fn 且每次 clone 捕获物）：
    // ① on_navigation 拦截 anibox-sniff:// 伪导航回传直链；
    // ② [lc-1613] 必须与主窗口传**完全相同**的 additionalBrowserArgs，否则同一
    //    user data folder 下 EnvironmentOptions 不一致 → 0x8007139F。
    let make = move |app: &AppHandle, label: &str| {
        let tx_nav = tx_nav.clone();
        let target = target.clone();
        let mut b = tauri::webview::WebviewWindowBuilder::new(
            app,
            label,
            tauri::WebviewUrl::External(target),
        )
        .title("AniBox 资源解析")
        .visible(false)
        .initialization_script(&script)
        .user_agent(&ua_owned)
        .on_navigation(move |u| {
            if u.scheme() == SNIFF_SCHEME {
                for (_, v) in u.query_pairs() {
                    if !v.is_empty() {
                        let _ = tx_nav.send(Ok(v.to_string()));
                    }
                }
                return false; // 取消这次伪导航，页面保持原状
            }
            true
        });
        if let Some(args) = main_window_browser_args(app) {
            b = b.additional_browser_args(&args);
        }
        b.build().map(|_| ())
    };

    let used_label = match create_hidden_webview(app, SNIFF_LABEL, make).await {
        Ok(l) => l,
        Err(e) => {
            reset_sniff_waiters();
            return Err(format!("创建嗅探 WebView 失败: {e}"));
        }
    };

    // [lc-1622] 主通道：宿主侧定时 eval 读**主 frame** 的 window.__aniboxSniff。
    // ExecuteScript/eval 是宿主行为，**不受页面 CSP（img-src / connect-src）限制** ——
    // 这正是「强 CSP 源站把本地 beacon 图片拦死」的正解。子 frame 命中后经 postMessage
    // 冒泡到 top（见 sniff_script），故只需轮询主 frame。
    {
        let app_poll = app.clone();
        let label_poll = used_label.clone();
        let tx_poll = tx.clone();
        tokio::spawn(async move {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
            loop {
                if std::time::Instant::now() >= deadline {
                    break;
                }
                // 与验证流程同一节奏（350ms 档），首次略等一下让页面开始跑
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let Some(w) = app_poll.get_webview_window(&label_poll) else {
                    break; // 窗口已销毁（等待结束后的清理）→ 退出
                };
                let Some(raw) = eval_json(
                    &w,
                    "window.__aniboxSniff && window.__aniboxSniff.url || ''",
                )
                .await
                else {
                    continue;
                };
                let Ok(url) = serde_json::from_str::<String>(&raw) else {
                    continue;
                };
                if !url.is_empty() {
                    crate::libmpv::trace(&format!("[webrs] 轮询命中（{label_poll}）: {url}"));
                    let _ = tx_poll.send(Ok(url));
                    break;
                }
            }
        });
    }

    // 等结果（最长 40s）
    let wait = tokio::task::spawn_blocking(move || rx.recv_timeout(std::time::Duration::from_secs(40)))
        .await
        .map_err(|e| format!("嗅探等待失败: {e}"));

    // 清理：撤销等待者 + 销毁嗅探窗（用实际 label，重试过就是 `-r1`）
    reset_sniff_waiters();
    let app3 = app.clone();
    let app4 = app.clone();
    let used_label2 = used_label.clone();
    let _ = app3.run_on_main_thread(move || {
        if let Some(w) = app4.get_webview_window(&used_label2) {
            let _ = w.destroy();
        }
    });

    match wait {
        Ok(Ok(Ok(url))) => {
            crate::libmpv::trace(&format!("[webrs] 嗅探命中（{used_label}）: {url}"));
            Ok(url)
        }
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(_)) => {
            // [lc-1618] 埋点：嗅探超时是最常见的「在线源播不了」成因（页面没截到直链 /
            // 需验证 / 加密方案不支持），过去只把文案冒给前端、日志里没有对应记录。
            crate::libmpv::trace(&format!(
                "[webrs] 嗅探超时（{used_label}）40s 内未截到视频流 —— 页面可能需验证 / 加密方案不支持"
            ));
            Err("嗅探超时（40s 内没截到视频流）——该站点可能需要验证或用了不支持的加密方案，换一个源试试".into())
        }
        Err(e) => Err(e),
    }
}
// ───────────────────────── [lc-1606] 反爬验证流程 ─────────────────────────
//
// 对齐 Kazumi `captcha_verification_service.dart` + `captcha_webview_windows_impl.dart`：
// 用隐藏 WebView 加载搜索页，按 captchaType 完成验证，再统一收尾（收割 cookie + UA +
// HTML、卸载窗口），随后前端自动重试检索（此时 cookie 已在内存 jar 里）。
// 三型：1 图片验证码（抓图 → 前端输入 → 回填提交）；2 自动点击按钮；3 自定义 JS。
//
// 与本项目既有 webview_sniff 一样走隐藏 WebviewWindow + initialization_script；
// 但验证需要「双向」通信，这里改用 `eval_with_callback` 轮询页面里由注入脚本维护的
// `window.__aniboxCaptcha` 状态对象（避开 CSP 对本地 beacon 的限制）。

/// 取消原因码（[lc-1624]）：区分「用户主动取消 / 被新会话顶掉」——两者都**不是验证失败**，
/// 不该用「人机验证失败」的文案糊到用户脸上（这正是用户报的「失败 · dalvdm：已取消」）。
const CAPTCHA_CANCEL_NONE: u8 = 0;
const CAPTCHA_CANCEL_USER: u8 = 1;
const CAPTCHA_CANCEL_SUPERSEDED: u8 = 2;

/// 当前验证会话（同一时刻只允许一个）。
struct CaptchaHandle {
    rule: String,
    label: String,
    /// 0=进行中；1=用户取消；2=被新会话顶掉（见上面的常量）
    cancel: Arc<AtomicU8>,
}

static CAPTCHA_SESSION: OnceLock<std::sync::Mutex<Option<CaptchaHandle>>> = OnceLock::new();
static CAPTCHA_SEQ: AtomicU64 = AtomicU64::new(0);
/// [lc-1624] 串行化「发起验证」：两次并发发起会各自建窗、互相顶掉，且后注册的覆盖会话，
/// 先注册的那个 flow 会一直跑到超时（窗还泄漏）。整段持锁，保证同一时刻只有一个会话在建立。
static CAPTCHA_START_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn captcha_start_lock() -> &'static tokio::sync::Mutex<()> {
    CAPTCHA_START_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn captcha_session() -> &'static std::sync::Mutex<Option<CaptchaHandle>> {
    CAPTCHA_SESSION.get_or_init(|| std::sync::Mutex::new(None))
}

/// 在隐藏窗里 eval 一段 JS 表达式并取回 JSON 序列化结果（超时 5s 返回 None）。
/// 表达式异常时返回 null（Windows 下 eval 的异常会被吞掉，故 JS 里自带 try/catch）。
async fn eval_json(w: &tauri::WebviewWindow, expr: &str) -> Option<String> {
    let js = format!("(function(){{ try {{ return {expr}; }} catch (e) {{ return null; }} }})()");
    // 用 std mpsc（Sender::send 取 &self，闭包可 Fn 重复调用；oneshot 会 move self 不适用）
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if w.eval_with_callback(js, move |s| {
        let _ = tx.send(s);
    })
    .is_err()
    {
        return None;
    }
    tokio::task::spawn_blocking(move || rx.recv_timeout(std::time::Duration::from_secs(5)).ok())
        .await
        .ok()
        .flatten()
}

/// 注入到验证页的脚本：维护 `window.__aniboxCaptcha = {status,image,log,url}`。
/// status: idle/found/clicked/submitted/done/fail。按 captchaType 走不同分支。
fn captcha_script(cfg: &AntiCrawlerConfig) -> String {
    let img = serde_json::to_string(&cfg.captcha_image).unwrap_or_else(|_| "\"\"".into());
    let input = serde_json::to_string(&cfg.captcha_input).unwrap_or_else(|_| "\"\"".into());
    let btn = serde_json::to_string(&cfg.captcha_button).unwrap_or_else(|_| "\"\"".into());
    let head = r#"(function () {
  try {
    if (window.__aniboxCaptcha) return;
    var st = { status: 'idle', image: '', log: '', url: location.href, submitted: false };
    window.__aniboxCaptcha = st;
    var IMG = __IMG__, INPUT = __INPUT__, BTN = __BTN__, TYPE = __TYPE__;
    function evalX(xp) {
      try { var r = document.evaluate(xp, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null); return r.singleNodeValue; }
      catch (e) { return null; }
    }
    function setStatus(s) { st.status = s; }
    function startDisappear(xp) {
      if (st._obs) return;
      try {
        st._obs = new MutationObserver(function () {
          if (!evalX(xp)) { try { st._obs.disconnect(); } catch (e) {} st._obs = null; setStatus('done'); }
        });
        st._obs.observe(document.documentElement, { childList: true, subtree: true, attributes: true });
      } catch (e) {}
    }
    function capture(img, cb) {
      function go() {
        try {
          var c = document.createElement('canvas');
          c.width = img.naturalWidth || img.width || 120;
          c.height = img.naturalHeight || img.height || 40;
          c.getContext('2d').drawImage(img, 0, 0);
          cb(c.toDataURL('image/png'));
        } catch (e) { cb(null); }
      }
      if (img.complete && img.naturalWidth > 0) go();
      else { img.addEventListener('load', go); img.addEventListener('error', function () { cb(null); }); }
    }
    function tick() {
      st.url = location.href;
      if (TYPE === 1) {
        var im = evalX(IMG);
        if (im) {
          startDisappear(IMG);
          if (!st.image) capture(im, function (d) { if (d) { st.image = d; setStatus('found'); } });
        }
      } else if (TYPE === 2) {
        var b = evalX(BTN);
        if (b && st.status !== 'clicked' && st.status !== 'done') {
          try { b.click(); } catch (e) {}
          setStatus('clicked');
          startDisappear(BTN);
        }
      }
    }
"#;
    let tail = if cfg.captcha_type == 3 {
        // type3：注入规则脚本，暴露 window.KazumiCaptcha 桥；脚本返回 true 或调 done() 即通过
        r#"    try {
      window.KazumiCaptcha = {
        log: function (m) { st.log = 'JS:' + String(m); },
        clicked: function () { setStatus('clicked'); },
        done: function () { setStatus('done'); },
        fail: function (m) { st.log = 'fail:' + String(m); setStatus('fail'); }
      };
      var __r = (function () {
__SCRIPT__
      })();
      if (__r === true) setStatus('done');
    } catch (e) { st.log = 'err:' + (e && e.message ? e.message : e); setStatus('fail'); }
"#
        .replace("__SCRIPT__", cfg.captcha_script.trim())
    } else {
        r#"    tick();
    st._iv = setInterval(function () {
      try { if (st.status === 'done') { clearInterval(st._iv); return; } tick(); } catch (e) {}
    }, 500);
"#
        .to_string()
    };
    let end = "  } catch (e) {}\n})();\n";
    format!("{head}{tail}{end}")
        .replace("__IMG__", &img)
        .replace("__INPUT__", &input)
        .replace("__BTN__", &btn)
        .replace("__TYPE__", &cfg.captcha_type.to_string())
}

/// type1 提交脚本：native value setter 写值 + 派发 input/change + 点提交按钮
/// （对齐 Kazumi `submitCaptchaInteract`）。
fn captcha_submit_script(cfg: &AntiCrawlerConfig, code: &str) -> String {
    let input = serde_json::to_string(&cfg.captcha_input).unwrap_or_else(|_| "\"\"".into());
    let btn = serde_json::to_string(&cfg.captcha_button).unwrap_or_else(|_| "\"\"".into());
    let code = serde_json::to_string(code).unwrap_or_else(|_| "\"\"".into());
    r#"(function () {
  function evalX(xp) {
    try { var r = document.evaluate(xp, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null); return r.singleNodeValue; }
    catch (e) { return null; }
  }
  var input = evalX(__INPUT__);
  if (input) {
    try {
      input.focus();
      var d = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value');
      d.set.call(input, __CODE__);
      input.dispatchEvent(new Event('input', { bubbles: true }));
      input.dispatchEvent(new Event('change', { bubbles: true }));
    } catch (e) {}
  }
  var b = evalX(__BTN__);
  if (b) { try { b.click(); } catch (e) {} }
  if (window.__aniboxCaptcha) {
    window.__aniboxCaptcha.image = '';
    window.__aniboxCaptcha.status = 'submitted';
    window.__aniboxCaptcha.submitted = true;
  }
  return true;
})();"#
        .replace("__INPUT__", &input)
        .replace("__BTN__", &btn)
        .replace("__CODE__", &code)
}

fn destroy_captcha_window(app: &AppHandle, label: &str) {
    let app2 = app.clone();
    let label = label.to_string();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = app2.get_webview_window(&label) {
            let _ = w.destroy();
        }
    });
}

/// [lc-1624] 撤销当前验证会话并销毁窗口。`reason` 用上面的 CANCEL_* 常量，
/// 让正在跑的 flow 能区分「用户取消」与「被新会话顶掉」，从而给出正确文案。
fn cancel_captcha(app: &AppHandle, reason: u8) {
    let label = {
        let Ok(mut g) = captcha_session().lock() else {
            return;
        };
        match g.take() {
            Some(h) => {
                h.cancel.store(reason, Ordering::Relaxed);
                h.label
            }
            None => return,
        }
    };
    destroy_captcha_window(app, &label);
}

/// 收尾：收割 HTML + UA + cookie 存入内存 jar，卸载窗口（对齐 Kazumi `_saveCookiesAndUnload`）。
async fn harvest_captcha(app: &AppHandle, rule: &Rule, label: &str, page_url: &str) {
    let Some(w) = app.get_webview_window(label) else {
        return;
    };
    let html = eval_json(
        &w,
        "document.documentElement ? document.documentElement.outerHTML : ''",
    )
    .await
    .and_then(|s| serde_json::from_str::<String>(&s).ok())
    .unwrap_or_default();
    let ua = eval_json(&w, "navigator.userAgent || ''")
        .await
        .and_then(|s| serde_json::from_str::<String>(&s).ok())
        .unwrap_or_default();
    let mut cookies: Vec<(String, String)> = Vec::new();
    if let Ok(u) = Url::parse(page_url) {
        if let Ok(cs) = w.cookies_for_url(u) {
            for c in cs {
                cookies.push((c.name().to_string(), c.value().to_string()));
            }
        }
    }
    crate::libmpv::trace(&format!(
        "[webrs] 验证收尾: html {} 字节, cookie {} 条",
        html.len(),
        cookies.len()
    ));
    save_cookies(&rule.name, cookies, Some(ua));
}

/// 轮询验证页状态直到通过/失败/超时，然后统一收尾。
async fn run_captcha_flow(app: AppHandle, rule: Rule, label: String, initial_url: String, cancel: Arc<AtomicU8>) {
    let cfg = rule.anti_crawler_config.clone();
    let name = rule.name.clone();
    let _ = app.emit(
        "webrs-captcha-status",
        json!({ "rule": name, "status": "loading", "error": "" }),
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let mut last_image = String::new();
    let mut saw_action = false;
    let outcome: Result<(), String> = loop {
        // [lc-1624] 取消原因分型：被新会话顶掉 ≠ 用户取消 ≠ 失败
        let cancel_reason = cancel.load(Ordering::Relaxed);
        if cancel_reason != CAPTCHA_CANCEL_NONE {
            break Err(if cancel_reason == CAPTCHA_CANCEL_SUPERSEDED {
                "已被新的验证会话取代".to_string()
            } else {
                "已取消".to_string()
            });
        }
        if std::time::Instant::now() > deadline {
            break Err("验证超时，请重试".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        let Some(w) = app.get_webview_window(&label) else {
            break Err("验证窗口已关闭".into());
        };
        let Some(raw) = eval_json(&w, "window.__aniboxCaptcha || null").await else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
        let image = v.get("image").and_then(|s| s.as_str()).unwrap_or("");
        let url = v.get("url").and_then(|s| s.as_str()).unwrap_or("");
        // type1：新验证码图 → 推给前端弹窗
        if cfg.captcha_type == 1 && !image.is_empty() && image != last_image {
            last_image = image.to_string();
            let _ = app.emit("webrs-captcha", json!({ "rule": name, "image": image }));
        }
        if status == "clicked" || status == "submitted" {
            saw_action = true;
        }
        if status == "done" {
            break Ok(());
        }
        if status == "fail" {
            let log = v
                .get("log")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("验证脚本执行失败");
            break Err(log.to_string());
        }
        // 导航视为成功：仅当已发生交互（点击/提交）后，避免 http→https 之类的自动跳转误判
        if saw_action && !url.is_empty() && url != initial_url {
            break Ok(());
        }
    };

    if outcome.is_ok() {
        harvest_captcha(&app, &rule, &label, &initial_url).await;
    }
    destroy_captcha_window(&app, &label);
    // 清会话（仅当仍是本会话，避免误清新会话）
    if let Ok(mut g) = captcha_session().lock() {
        if g.as_ref().map(|h| h.label.as_str()) == Some(label.as_str()) {
            *g = None;
        }
    }
    match outcome {
        Ok(()) => {
            crate::libmpv::trace(&format!("[webrs] 反爬验证通过: {name}"));
            let _ = app.emit(
                "webrs-captcha-status",
                json!({ "rule": name, "status": "verified", "error": "" }),
            );
        }
        Err(e) => {
            // [lc-1624] 取消 / 被顶掉 **不是**验证失败：发 cancelled 状态，
            // 前端据此清态（不弹"验证失败"），避免误报「失败 · dalvdm：已取消」。
            let cancel_reason = cancel.load(Ordering::Relaxed);
            if cancel_reason != CAPTCHA_CANCEL_NONE {
                crate::libmpv::trace(&format!("[webrs] 反爬验证中止: {name} · {e}"));
                let _ = app.emit(
                    "webrs-captcha-status",
                    json!({ "rule": name, "status": "cancelled", "error": e }),
                );
            } else {
                crate::libmpv::trace(&format!("[webrs] 反爬验证失败: {name} · {e}"));
                let _ = app.emit(
                    "webrs-captcha-status",
                    json!({ "rule": name, "status": "failed", "error": e }),
                );
            }
        }
    }
}

/// [lc-1606] 发起反爬验证：建隐藏窗加载搜索页并按 captchaType 注入脚本。
/// 立即返回；结果经 `webrs-captcha`（type1 图）/`webrs-captcha-status` 事件回传。
#[tauri::command]
pub async fn webrs_captcha_start(
    app: AppHandle,
    rule_name: String,
    keyword: String,
) -> Result<(), String> {
    let rules = load_rules(&app).await?;
    let rule = find_rule(&rules, &rule_name)?.clone();
    if !rule.anti_crawler_config.enabled {
        return Err(format!("规则「{}」未启用反爬验证", rule.name));
    }
    if rule.search_url.is_empty() {
        return Err("规则缺少 searchURL".into());
    }
    let encoded = urlencoding::encode(keyword.trim()).to_string();
    let url = rule.search_url.replace("@keyword", &encoded);

    // [lc-1624] 串行化整个「发起验证」：保证同一时刻只有一个会话在建立。
    // 否则两次并发发起会各自建窗、互相顶掉；后注册的覆盖会话，先注册的 flow 会一直跑到
    // 90s 超时（窗口还泄漏）。持锁到会话登记完成为止。
    let _start_guard = captcha_start_lock().lock().await;

    // 撤掉可能存在的旧会话 —— 用 SUPERSEDED 原因，让它给"被取代"文案而不是"验证失败"
    cancel_captcha(&app, CAPTCHA_CANCEL_SUPERSEDED);
    let cancel = Arc::new(AtomicU8::new(CAPTCHA_CANCEL_NONE));
    let label_base = format!(
        "webrs-captcha-{}",
        CAPTCHA_SEQ.fetch_add(1, Ordering::Relaxed)
    );

    let ua = effective_ua(&rule);
    let script = captcha_script(&rule.anti_crawler_config);
    // 验证页 URL 无效则直接失败，不必起窗口
    let target = Url::parse(&url).map_err(|_| format!("验证页 URL 无效: {url}"))?;

    // 组装建窗闭包（最多调用 2 次）：[lc-1613] 必须与主窗口传**完全相同**的
    // additionalBrowserArgs，否则同一 user data folder 下 EnvironmentOptions
    // 不一致 → CreateCoreWebView2EnvironmentWithOptions 返回 0x8007139F。
    let make = move |app: &AppHandle, label: &str| {
        let mut b = tauri::webview::WebviewWindowBuilder::new(
            app,
            label,
            tauri::WebviewUrl::External(target.clone()),
        )
        .title("AniBox 人机验证")
        .visible(false)
        .initialization_script(&script)
        .user_agent(&ua);
        if let Some(args) = main_window_browser_args(app) {
            b = b.additional_browser_args(&args);
        }
        b.build().map(|_| ())
    };

    let label = match create_hidden_webview(&app, &label_base, make).await {
        Ok(l) => l,
        Err(e) => return Err(format!("创建验证窗口失败: {e}")),
    };

    // 建窗成功后才登记会话（label 可能因重试变成 `-r1`，提交/取消都按它找窗）
    {
        let mut g = captcha_session().lock().map_err(|e| e.to_string())?;
        *g = Some(CaptchaHandle {
            rule: rule.name.clone(),
            label: label.clone(),
            cancel: cancel.clone(),
        });
    }

    let app3 = app.clone();
    tauri::async_runtime::spawn(async move {
        run_captcha_flow(app3, rule, label, url, cancel).await;
    });
    Ok(())
}

/// [lc-1606] type1：前端把用户输入的验证码交回来，在窗里回填并点提交。
#[tauri::command]
pub async fn webrs_captcha_submit(
    app: AppHandle,
    rule_name: String,
    code: String,
) -> Result<(), String> {
    let rules = load_rules(&app).await?;
    let rule = find_rule(&rules, &rule_name)?.clone();
    let code = code.trim().to_string();
    if code.is_empty() {
        return Err("验证码为空".into());
    }
    let label = {
        let g = captcha_session().lock().map_err(|e| e.to_string())?;
        match g.as_ref() {
            Some(h) if h.rule.eq_ignore_ascii_case(&rule_name) => h.label.clone(),
            _ => return Err("没有进行中的验证会话，请重新发起".into()),
        }
    };
    let w = app
        .get_webview_window(&label)
        .ok_or("验证窗口已关闭，请重新发起验证")?;
    let script = captcha_submit_script(&rule.anti_crawler_config, &code);
    w.eval(script)
        .map_err(|e| format!("提交验证码失败: {e}"))?;
    Ok(())
}

/// [lc-1606] 用户取消验证：撤销会话并销毁窗口。
/// [lc-1624] 用 USER 原因 —— 正在跑的 flow 会发 `cancelled` 状态（而非 `failed`），
/// 前端据此清态，不再误报「人机验证失败 · …：已取消」。
#[tauri::command]
pub async fn webrs_captcha_cancel(app: AppHandle) -> Result<(), String> {
    cancel_captcha(&app, CAPTCHA_CANCEL_USER);
    Ok(())
}

// ───────────────────────── mpv 播放 ─────────────────────────

/// [lc-1500] 在线资源直链 → 独立 mpv。m3u8 直链普遍校验 Referer/UA，
/// 通过 mpv 的 `referrer` / `user-agent` 属性带上（HLS 分段请求沿用）。
#[tauri::command]
pub async fn webrs_play(
    state: State<'_, crate::AppState>,
    app: AppHandle,
    url: String,
    referer: Option<String>,
    ua: Option<String>,
    // [lc-1615] 观影记录元数据（可选；老调用方不传时仅按直链 URL 记一笔，不影响播放）。
    // title = 番剧名、episode = 集名、rule_name = 规则源名、episode_url = 集页面 URL（去重键）。
    title: Option<String>,
    episode: Option<String>,
    rule_name: Option<String>,
    episode_url: Option<String>,
) -> Result<(), String> {
    let cfg = state.config.read().await.clone();
    // 播放器可用性前置校验（mpv 路径非空 / PotPlayer 路径有效）
    let _ = crate::player::resolve_player(&cfg)?;
    // [lc-1618] 埋点：入口留痕（含直链与是否带 Referer/UA），便于把「播不了」定位到
    // 「预检没过 / 会话没起 / loadfile 失败 / 加载后源站拒流」中的哪一步。
    crate::libmpv::trace(&format!(
        "[webrs] play: 收到播放请求 url={url} referer={:?} ua={:?}",
        referer.as_deref().filter(|s| !s.is_empty()),
        ua.as_deref().filter(|s| !s.is_empty())
    ));

    // 直链预检（带 Referer/UA；只看状态码，不读 body）
    {
        let client = http()?;
        let mut req = client
            .get(&url)
            .header("user-agent", ua.clone().unwrap_or_else(random_ua));
        if let Some(r) = &referer {
            if !r.is_empty() {
                req = req.header("referer", r);
            }
        }
        let resp = req
            .send()
            .await
            .map_err(|e| {
                crate::libmpv::trace(&format!("[webrs] play: 直链预检请求失败: {e}"));
                format!("直链不可达: {e}")
            })?;
        let status = resp.status();
        if !(status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT) {
            crate::libmpv::trace(&format!("[webrs] play: 直链预检返回 {status}（源可能失效）"));
            return Err(format!("直链返回 {status}，源可能失效"));
        }
        crate::libmpv::trace(&format!("[webrs] 直链预检 ok: {status} {url}"));
    }

    // [player] 统一入口：
    //   · mpv：新建会话时 URL+Referer/UA 全部进命令行（零启动期 IPC）；复用会话走 IPC。
    //   · PotPlayer：命令行不支持 Referer/UA → player 内部注册到本地 relay（22349）再播放；
    //     relay URL 路径附带「番名 - 集名」，PotPlayer 标题/播放列表项显示可读中文名。
    // [lc-1615] 拼一个可读标题供播放器显示（历史记录仍用 title/episode 原字段）。
    let display_title = {
        let t = title.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let e = episode.as_deref().map(str::trim).filter(|s| !s.is_empty());
        match (t, e) {
            (Some(t), Some(e)) => format!("{t} - {e}"),
            (Some(t), None) => t.to_string(),
            (None, Some(e)) => e.to_string(),
            (None, None) => String::new(),
        }
    };
    let headers = crate::player::HttpHeaders::new(referer.clone(), ua.clone());
    let header_ref = if headers.needs_relay() { Some(&headers) } else { None };
    let launch_res = crate::player::launch(
        &app,
        &state,
        &cfg,
        &url,
        &display_title,
        0.0,
        0.0,
        header_ref,
        &[],
    )
    .await;
    crate::libmpv::trace("[webrs] play: player::launch 完成");

    // [lc-1615] 起播成功 → 落一笔本机播放历史（观影记录页数据源）。
    // [lc-1710] 先记录、后传播播放器错误：只要已把可播地址交给播放器就算一次观看，
    // 不能因 IPC/启动失败丢掉记录。去重由 history 内部按剧级 series_key（在线源 = 番剧名）。
    let key = episode_url.filter(|s| !s.is_empty()).unwrap_or_else(|| url.clone());
    crate::history::record(
        &app,
        crate::history::WatchRecord {
            guid: format!("webrs:{}", key),
            title: title.unwrap_or_default(),
            episode: episode.unwrap_or_default(),
            poster: String::new(),
            tmdb_id: String::new(),
            bgm_id: String::new(),
            media_type: "webrs".into(),
            played_at: crate::history::now_ms(),
            source: "webrs".into(),
            source_name: rule_name.unwrap_or_default(),
            parent_guid: String::new(),
            // [lc-1631] 在线源起播时拿不到总时长/进度 → 如实留空；
            // 实际观看时长由前端会话计时在播放结束回填（record_watch_session）。
            duration_ms: 0,
            watched_ms: 0,
            progress: 0.0,
            finished: false,
            ..Default::default()
        },
    );
    launch_res?;
    Ok(())
}

// ───────────────────────── [lc-1606] 单测（不触网） ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anticrawler_defaults_when_fields_missing() {
        // 只写 enabled：其余字段应走 Default（captchaType=1 / captchaDetectType=1），对齐 Kazumi
        let cfg: AntiCrawlerConfig = serde_json::from_str("{\"enabled\":true}").unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.captcha_type, 1);
        assert_eq!(cfg.captcha_detect_type, 1);
        assert!(cfg.captcha_image.is_empty());
        assert!(cfg.captcha_script.is_empty());
    }

    #[test]
    fn anticrawler_real_shape() {
        // dalvdm 的真实 antiCrawlerConfig
        let j = r#"{"enabled":true,"captchaType":1,
          "captchaImage":"//div[contains(@class, 'mac_msg_jump')]//img[contains(@class, 'mac_verify_img')]",
          "captchaInput":"//div[contains(@class, 'mac_msg_jump')]//input[@name='verify']",
          "captchaButton":"//div[contains(@class, 'mac_msg_jump')]//input[contains(@class, 'verify_submit')]",
          "captchaDetectType":1,
          "captchaDetectValue":"//div[contains(@class, 'mac_msg_jump')]//input[@name='verify']",
          "captchaScript":""}"#;
        let cfg: AntiCrawlerConfig = serde_json::from_str(j).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.captcha_type, 1);
        assert_eq!(cfg.captcha_detect_type, 1);
        assert!(cfg.captcha_detect_value.contains("mac_msg_jump"));
        assert!(cfg.captcha_image.contains("mac_verify_img"));
    }

    #[test]
    fn rule_parses_anticrawler_config() {
        let j = r#"{"name":"x","searchURL":"http://a/@keyword",
          "antiCrawlerConfig":{"enabled":true,"captchaType":2,"captchaButton":"//button"}}"#;
        let r: Rule = serde_json::from_str(j).unwrap();
        assert!(r.anti_crawler_config.enabled);
        assert_eq!(r.anti_crawler_config.captcha_type, 2);
        assert_eq!(r.anti_crawler_config.captcha_button, "//button");
    }

    #[test]
    fn rule_without_anticrawler_defaults_disabled() {
        let r: Rule = serde_json::from_str(r#"{"name":"y"}"#).unwrap();
        assert!(!r.anti_crawler_config.enabled);
        assert_eq!(r.anti_crawler_config.captcha_type, 1);
    }

    fn resp(status: u16, body: &str, cf: bool) -> HttpResp {
        HttpResp {
            status: reqwest::StatusCode::from_u16(status).unwrap(),
            body: body.to_string(),
            cf_mitigated: cf,
        }
    }

    #[test]
    fn challenge_cf_mitigated_header() {
        let cfg = AntiCrawlerConfig {
            enabled: true,
            ..Default::default()
        };
        // 头命中即判真，即使 body 为空
        assert!(detects_captcha_challenge(&resp(403, "", true), &cfg));
    }

    #[test]
    fn challenge_disabled_is_never_true() {
        let cfg = AntiCrawlerConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!detects_captcha_challenge(&resp(403, "challenge", true), &cfg));
    }

    #[test]
    fn challenge_detect_text() {
        let cfg = AntiCrawlerConfig {
            enabled: true,
            captcha_detect_type: 2,
            captcha_detect_value: "Just a moment".into(),
            ..Default::default()
        };
        assert!(detects_captcha_challenge(
            &resp(503, "<title>Just a moment...</title>", false),
            &cfg
        ));
        assert!(!detects_captcha_challenge(&resp(200, "normal page", false), &cfg));
    }

    #[test]
    fn challenge_detect_regex() {
        let cfg = AntiCrawlerConfig {
            enabled: true,
            captcha_detect_type: 3,
            captcha_detect_value: r"cf-\w+".into(),
            ..Default::default()
        };
        assert!(detects_captcha_challenge(
            &resp(403, "<div class=\"cf-challenge\">", false),
            &cfg
        ));
        assert!(!detects_captcha_challenge(&resp(403, "<div>nope</div>", false), &cfg));
    }

    #[test]
    fn challenge_detect_xpath() {
        let cfg = AntiCrawlerConfig {
            enabled: true,
            captcha_detect_type: 1,
            captcha_detect_value: "//input[@name='verify']".into(),
            ..Default::default()
        };
        assert!(detects_captcha_challenge(
            &resp(403, "<div><input name='verify'></div>", false),
            &cfg
        ));
        assert!(!detects_captcha_challenge(
            &resp(403, "<div><input name='q'></div>", false),
            &cfg
        ));
    }

    #[test]
    fn challenge_xpath_fallback_selectors() {
        // 未配 detectValue 时回退看 captchaImage / captchaButton 是否命中
        let cfg = AntiCrawlerConfig {
            enabled: true,
            captcha_detect_type: 1,
            captcha_detect_value: String::new(),
            captcha_image: "//img[@class='verify']".into(),
            ..Default::default()
        };
        assert!(detects_captcha_challenge(
            &resp(403, "<img class='verify'>", false),
            &cfg
        ));
        assert!(!detects_captcha_challenge(
            &resp(403, "<img class='x'>", false),
            &cfg
        ));
    }

    #[test]
    fn cookie_header_join_and_overwrite() {
        let name = "unit-test-rule-cookie";
        save_cookies(
            name,
            vec![("a".into(), "1".into()), ("b".into(), "2".into())],
            Some("UA/1".into()),
        );
        assert_eq!(cookie_header_for(name), "a=1; b=2");
        // 同名覆盖，其余保留
        save_cookies(name, vec![("a".into(), "9".into())], None);
        assert_eq!(cookie_header_for(name), "a=9; b=2");
        assert_eq!(verified_ua(name).as_deref(), Some("UA/1"));
    }

    #[test]
    fn effective_ua_overrides_after_verify() {
        let name = "unit-test-rule-ua";
        let rule = Rule {
            name: name.into(),
            user_agent: "RuleUA".into(),
            ..Default::default()
        };
        assert_eq!(effective_ua(&rule), "RuleUA");
        // 有 cookie + 验证 UA → 覆盖规则 UA（clearance cookie 与 UA 绑定）
        save_cookies(name, vec![("c".into(), "1".into())], Some("VerifiedUA".into()));
        assert_eq!(effective_ua(&rule), "VerifiedUA");
    }

    #[test]
    fn challenge_detect_dalvdm_real_body() {
        // 用真机探针抓到的 dalvdm 挑战页形态（HTTP 200 + mac_msg_jump 结构）验证检测
        let cfg = AntiCrawlerConfig {
            enabled: true,
            captcha_type: 1,
            captcha_image: "//div[contains(@class, 'mac_msg_jump')]//img[contains(@class, 'mac_verify_img')]".into(),
            captcha_input: "//div[contains(@class, 'mac_msg_jump')]//input[@name='verify']".into(),
            captcha_button: "//div[contains(@class, 'mac_msg_jump')]//input[contains(@class, 'verify_submit')]".into(),
            captcha_detect_type: 1,
            captcha_detect_value: "//div[contains(@class, 'mac_msg_jump')]//input[@name='verify']".into(),
            captcha_script: String::new(),
        };
        let challenge = r#"<html><head><title>系统安全验证</title></head><body>
          <div class="mac_msg_jump"><img class="mac_verify_img" src="/v">
          <input name="verify" type="text"><input class="verify_submit" type="submit"></div></body></html>"#;
        // 200 也要能识别（dalvdm 挑战页就是 200）
        assert!(detects_captcha_challenge(&resp(200, challenge, false), &cfg));
        // 正常结果页不含验证码选择器 → 不误判
        let normal = r#"<html><body><div class="myui-vodbox-content"><div class="card-info">
          <div class="title">少女</div></div></div></body></html>"#;
        assert!(!detects_captcha_challenge(&resp(200, normal, false), &cfg));
    }

    // ───────────────────── [lc-1624] XPath 引擎：空步进容错 + 诊断 ─────────────────────

    fn parse_doc(html: &str) -> Html {
        Html::parse_document(html)
    }

    #[test]
    fn xpath_bare_double_slash_is_context_root() {
        // 内置 aafun 的 chapterRoads 就是 `//` —— 原实现报「空 XPath 步进」，整个源挂掉。
        // 现在应解析为「零步进 = 上下文本身」，即根节点为唯一"线路"。
        let d = parse_doc("<html><body><div class='x'><a href='/1'>ep</a></div></body></html>");
        let root = vec![d.tree.root().id()];
        let steps = parse_xpath("//").expect("`//` 应可解析");
        assert_eq!(steps.len(), 1);
        assert!(steps[0].is_empty(), "`//` 应解析为零步进（上下文本身）");
        let nodes = eval_nodes(&d, &root, "//").expect("`//` 应求值为节点");
        assert_eq!(nodes, root, "`//` 应返回上下文节点本身");
        // 以根为上下文时，aafun 的 chapterResult 仍能取到选集链接
        let links = eval_nodes(&d, &root, "//div[@class='x']/a").unwrap();
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn xpath_trailing_and_duplicate_separators_are_lenient() {
        let d = parse_doc("<html><body><div><span>x</span></div></body></html>");
        let root = vec![d.tree.root().id()];
        let base = eval_nodes(&d, &root, "//div").unwrap();
        assert_eq!(eval_nodes(&d, &root, "//div/").unwrap(), base, "末尾多写 / 应忽略");
        assert_eq!(eval_nodes(&d, &root, "//div//").unwrap(), base, "末尾多写 // 应忽略");
        // 相邻分隔符 //a///b 折叠成 //a/b（能解析就解析）
        assert_eq!(eval_nodes(&d, &root, "//div///span").unwrap().len(), 1);
    }

    #[test]
    fn xpath_empty_union_branch_ignored_but_all_empty_errors() {
        let d = parse_doc("<html><body><p>a</p></body></html>");
        let root = vec![d.tree.root().id()];
        // 末尾多写 | → 空分支忽略
        assert_eq!(eval_nodes(&d, &root, "//p|").unwrap().len(), 1);
        // 全是空分支 / 空表达式 → 可诊断错误
        assert!(parse_xpath("|").is_err());
        assert!(parse_xpath("").is_err());
        assert!(parse_xpath("  |  | ").is_err());
    }

    #[test]
    fn xpath_parent_axis_reports_diagnostic() {
        // 不支持的轴给**点名**的可诊断错误，而不是静默匹配不到
        let e = parse_xpath("//div/..").unwrap_err();
        assert!(e.contains(".."), "错误应点名 `..`：{e}");
        let e2 = parse_xpath("//div/ancestor::body").unwrap_err();
        assert!(e2.contains("ancestor::"), "错误应点名不支持的轴：{e2}");
    }

    #[test]
    fn xpath_attr_step_rejects_garbage() {
        // `@attr` 混写谓词 / 路径不再被当成属性名静默失配
        assert!(parse_xpath("//a/@href[1]").is_err());
        assert!(parse_xpath("//a/@").is_err());
        // 正常属性步进仍可用
        assert!(parse_xpath("//a/@href").is_ok());
    }

    // ───────────────────── [lc-1608] 在线规则目录：单测（不触网） ─────────────────────

    #[test]
    fn catalog_parses_items_and_skips_bad_ones() {
        // 混入两条坏数据（非对象 / 缺 name），应被跳过，其余正常解析
        let text = r#"[
          {"name":"7sefun","version":"1.3","useNativePlayer":true,"author":"","lastUpdate":1786362131000},
          "not-an-object",
          {"version":"9.9"},
          {"name":"aafun","version":"1.2"}
        ]"#;
        let items = parse_catalog(text).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "7sefun");
        assert_eq!(items[0].version, "1.3");
        assert!(items[0].use_native_player);
        assert_eq!(items[0].last_update, 1786362131000);
        assert_eq!(items[1].name, "aafun");
        assert_eq!(items[1].version, "1.2");
    }

    #[test]
    fn catalog_root_must_be_array() {
        assert!(parse_catalog(r#"{"name":"x"}"#).is_err());
        assert!(parse_catalog("not json").is_err());
    }

    #[test]
    fn version_compare_three_cases() {
        // 可更新：远端更大
        assert!(remote_is_newer("1.2", "1.3"));
        assert!(remote_is_newer("1.2", "1.2.1")); // 缺段补 0 → 1.2.0 < 1.2.1
        assert!(remote_is_newer("1.9", "1.10")); // 逐段整数比较，非字典序
        // 不可更新：相等
        assert!(!remote_is_newer("1.2", "1.2"));
        assert!(!remote_is_newer("1.2", "1.2.0")); // 补齐后相等
        // 本地更高：远端更小
        assert!(!remote_is_newer("2.0", "1.9"));
        assert!(!remote_is_newer("1.2.1", "1.2"));
    }

    #[test]
    fn version_compare_non_numeric_falls_back_to_inequality() {
        // 某段不是整数 → 无法按段比，退回「整体字符串不等」
        assert!(remote_is_newer("1.0", "1.0-beta"));
        assert!(!remote_is_newer("1.0-beta", "1.0-beta"));
    }

    /// 唯一的临时目录（无 tempfile 依赖）：用进程 id + 纳秒后缀，测试完清掉。
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("webrs-cat-test-{tag}-{}-{nanos}", std::process::id()))
    }

    #[tokio::test]
    async fn install_then_update_writes_and_overwrites() {
        let dir = temp_dir("install");
        let v1 = json!({"name":"demo","version":"1.0","searchURL":"http://a/@keyword"});
        let v2 = json!({"name":"demo","version":"2.0","searchURL":"http://a/@keyword"});

        // 安装：写入并回传版本
        let r = write_rule_file(&dir, "demo", &v1, false).await.unwrap();
        assert_eq!(r["version"], "1.0");
        let p = dir.join("demo.json");
        assert!(p.exists());
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(saved["version"], "1.0");

        // 已存在时再安装（overwrite=false）→ 报错，且不覆盖
        assert!(write_rule_file(&dir, "demo", &v2, false).await.is_err());
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(saved["version"], "1.0");

        // 更新（overwrite=true）→ 覆盖成功
        let r = write_rule_file(&dir, "demo", &v2, true).await.unwrap();
        assert_eq!(r["version"], "2.0");
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(saved["version"], "2.0");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn write_rule_rejects_name_mismatch_and_missing_name() {
        let dir = temp_dir("mismatch");
        // 内容里的 name 与请求名不一致 → 拒绝（对齐 Kazumi）
        let bad = json!({"name":"other","version":"1.0"});
        assert!(write_rule_file(&dir, "demo", &bad, true).await.is_err());
        // 缺 name → 拒绝
        let noname = json!({"version":"1.0"});
        assert!(write_rule_file(&dir, "demo", &noname, true).await.is_err());
        // 目录里不应留下任何文件
        assert!(!dir.join("demo.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
