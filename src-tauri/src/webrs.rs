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

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::OnceLock;
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

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .expect("reqwest client build")
}

async fn fetch_text(url: &str, ua: &str, referer: &str) -> Result<String, String> {
    let client = http();
    let mut req = client.get(url).header("user-agent", ua);
    if !referer.is_empty() {
        req = req.header("referer", referer);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求失败 {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("请求返回 {status}: {url}"));
    }
    // 编码：这些站点多是 UTF-8，GBK 站点先用 lossless 兜底（latin1 丢字）——
    // reqwest 的 text() 默认按 header/UTF-8 解，乱码页面的搜索基本也匹配不上，先不管。
    resp.text().await.map_err(|e| format!("读响应失败: {e}"))
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
    } else {
        (default_axis, raw)
    };
    if let Some(attr) = raw.strip_prefix('@') {
        return Ok(Step {
            axis,
            test: Test::Attr(attr.trim().to_string()),
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
    branches.iter().map(|b| parse_single_xpath(b)).collect()
}

/// 解析单条分支（`.` 开头 = 相对上下文求值）。
fn parse_single_xpath(expr: &str) -> Result<Vec<Step>, String> {
    let mut s = expr.trim();
    let mut relative = false;
    if let Some(rest) = s.strip_prefix('.') {
        relative = true;
        s = rest;
    }
    if s.is_empty() {
        return Err("空 XPath".into());
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
            return Err(format!("XPath 语法错误（缺少起始 /）: {expr}"));
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
        steps.push(parse_step(&s[start..i], axis)?);
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
    let ua = rule_ua(rule);
    let referer = rule_referer(rule);
    let mut url = req.url.clone();
    for (p, r) in ph {
        url = url.replace(p, r);
    }
    if !url.starts_with("http") {
        url = join_url(&rule.base_url, &url).ok_or_else(|| format!("模板 URL 无法拼接: {url}"))?;
    }
    let client = http();
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

/// [lc-1501] 规则搜索核心（单源）：返回 [{name, src}]。api 模式走 JSONPath 子集，
/// xpath 模式走 HTML 解析。webrs_search / webrs_search_multi 共用。
async fn rule_search(rule: &Rule, keyword: &str) -> Result<Vec<(String, String)>, String> {
    let ua = rule_ua(rule);
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
        let html = fetch_text(&url, &ua, &referer).await?;
        let doc = Html::parse_document(&html);
        let root = vec![doc.tree.root().id()];
        let list = eval_nodes(&doc, &root, &rule.search_list)?;
        let mut out = Vec::new();
        for node in list.into_iter().take(60) {
            let name = match eval_xpath(&doc, &[node], &parse_xpath(&rule.search_name)?)? {
                EvalOut::Str(v) => v.first().cloned().unwrap_or_default(),
                EvalOut::Nodes(v) => v.first().map(|n| text_of(&doc, *n)).unwrap_or_default(),
            };
            let href = match eval_xpath(&doc, &[node], &parse_xpath(&rule.search_result)?)? {
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
    let items = rule_search(&rule, &keyword).await?;
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
                Err(e) => json!({ "name": name, "status": "error", "error": e, "items": [] }),
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
    let ua = rule_ua(&rule);
    let referer = rule_referer(&rule);
    let page_url = join_url(&rule.base_url, &url).ok_or("详情页 URL 无效")?;
    let html = fetch_text(&page_url, &ua, &referer).await?;
    let doc = Html::parse_document(&html);
    let root = vec![doc.tree.root().id()];
    let road_nodes = eval_nodes(&doc, &root, &rule.chapter_roads)?;
    if road_nodes.is_empty() {
        return Err(
            "没解析到播放线路 —— 该站选集很可能由脚本动态加载（如 baimao），HTTP 拿不到，换个源试试".into(),
        );
    }
    let mut roads: Vec<Value> = Vec::new();
    for (ri, road) in road_nodes.into_iter().enumerate() {
        let eps_ctx = vec![road];
        let Ok(ep_steps) = parse_xpath(&rule.chapter_result) else {
            continue;
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
    let ua = rule_ua(&rule);
    let referer = rule_referer(&rule);
    let page_url = join_url(&rule.base_url, &url).ok_or("播放页 URL 无效")?;

    crate::libmpv::trace(&format!("[webrs] resolve: {page_url}"));

    // ── 1) HTTP 直解 ──
    match fetch_text(&page_url, &ua, &referer).await {
        Ok(html) => {
            if let Some(hit) = extract_from_html(&html, &page_url) {
                crate::libmpv::trace(&format!("[webrs] HTTP 命中: {hit}"));
                return Ok(json!({ "url": hit, "referer": referer, "ua": ua, "via": "http" }));
            }
        }
        Err(e) => crate::libmpv::trace(&format!("[webrs] 页面拉取失败（转 WebView 嗅探）: {e}")),
    }

    // ── 2) WebView 嗅探 ──
    let hit = webview_sniff(&app, &page_url, &ua, &referer).await?;
    crate::libmpv::trace(&format!("[webrs] WebView 命中: {hit}"));
    Ok(json!({ "url": hit, "referer": referer, "ua": ua, "via": "webview" }))
}

// ── WebView 嗅探（Kazumi video_webview 的 JS 钩子思路）──

const SNIFF_LABEL: &str = "webrs-sniffer";
const SNIFF_SCHEME: &str = "anibox-sniff";

fn sniff_script(beacon_port: u16) -> String {
    r#"(function () {
  if (window.__aniboxSniff) return;
  window.__aniboxSniff = 1;
  var done = false;
  var seen = {};
  var BEACON = 'http://127.0.0.1:PORT/sniff?u=';
  function ok(u) {
    if (!u || typeof u !== 'string' || done) return false;
    if (!/^https?:\/\//i.test(u)) return false;
    var l = u.toLowerCase();
    if (/googleads|googlesyndication|adtrafficquality|doubleclick|prestrain\.html|prestrain%2ehtml/.test(l)) return false;
    if (seen[u]) return false;
    return /\.(m3u8|mp4|flv)(\?|#|$)/i.test(l);
  }
  function emit(u) {
    if (!ok(u)) return;
    seen[u] = 1;
    done = true;
    var q = encodeURIComponent(u);
    // 回传四通道：本地 beacon（image/fetch/sendBeacon，页面无 CSP 时必达）+ 伪导航兜底
    try { new Image().src = BEACON + q; } catch (e) {}
    try { fetch(BEACON + q, { mode: 'no-cors' }).catch(function () {}); } catch (e) {}
    try { navigator.sendBeacon && navigator.sendBeacon(BEACON + q); } catch (e) {}
    var nav = 'anibox-sniff://found?u=' + q;
    try { window.top.location.href = nav; return; } catch (e) {}
    try { location.href = nav; } catch (e) {}
  }
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
}

/// 本地嗅探回传 beacon：嗅探 JS 命中直链后向 `http://127.0.0.1:<port>/sniff?u=<url>`
/// 发请求（image/fetch/sendBeacon 三连），这里收下转交等待者。比伪导航可靠 ——
/// WebView2 对未知 scheme 的 location.href 未必触发顶层 NavigationStarting。
static BEACON_PORT: OnceLock<u16> = OnceLock::new();
static SNIFF_TX: OnceLock<std::sync::Mutex<Option<std::sync::mpsc::Sender<String>>>> = OnceLock::new();

fn ensure_beacon() -> Result<u16, String> {
    if let Some(p) = BEACON_PORT.get() {
        return Ok(*p);
    }
    for port in [22347u16, 22348, 22349] {
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

/// 隐藏 WebView 加载播放页，等待直链回传。通道：① 本地 beacon（嗅探 JS 的
/// image/fetch/sendBeacon 三连，页面/iframe 无 CSP 时必达）② 伪导航
/// anibox-sniff://（on_navigation 拦截，兜底）。JS hook 注入全 frame
/// （WebView2 AddScriptToExecuteOnDocumentCreated 覆盖 iframe）。
async fn webview_sniff(app: &AppHandle, page_url: &str, ua: &str, _referer: &str) -> Result<String, String> {
    use std::sync::mpsc;

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

    let app2 = app.clone();
    let page = page_url.to_string();
    let ua_owned = ua.to_string();
    let script = sniff_script(beacon_port);
    let tx_nav = tx.clone();

    let dispatch = app.run_on_main_thread(move || {
        // 上一个残留的嗅探窗先销毁
        if let Some(w) = app2.get_webview_window(SNIFF_LABEL) {
            let _ = w.destroy();
        }
        let Ok(target) = Url::parse(&page) else {
            let _ = tx_nav.send(Err(format!("播放页 URL 无效: {page}")));
            return;
        };
        let tx_build_err = tx_nav.clone();
        let b = tauri::webview::WebviewWindowBuilder::new(
            &app2,
            SNIFF_LABEL,
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
        if let Err(e) = b.build() {
            let _ = tx_build_err.send(Err(format!("创建嗅探 WebView 失败: {e}")));
        }
    });

    if let Err(e) = dispatch {
        if let Some(m) = SNIFF_TX.get() {
            if let Ok(mut g) = m.lock() {
                *g = None;
            }
        }
        return Err(format!("嗅探窗口调度失败: {e}"));
    }

    // 等结果（最长 40s）
    let wait = tokio::task::spawn_blocking(move || rx.recv_timeout(std::time::Duration::from_secs(40)))
        .await
        .map_err(|e| format!("嗅探等待失败: {e}"));

    // 清理：撤销等待者 + 销毁嗅探窗
    if let Some(m) = SNIFF_TX.get() {
        if let Ok(mut g) = m.lock() {
            *g = None;
        }
    }
    let app3 = app.clone();
    let app4 = app.clone();
    let _ = app3.run_on_main_thread(move || {
        if let Some(w) = app4.get_webview_window(SNIFF_LABEL) {
            let _ = w.destroy();
        }
    });

    match wait {
        Ok(Ok(Ok(url))) => Ok(url),
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(_)) => Err("嗅探超时（40s 内没截到视频流）——该站点可能需要验证或用了不支持的加密方案，换一个源试试".into()),
        Err(e) => Err(e),
    }
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
) -> Result<(), String> {
    let cfg = state.config.read().await.clone();
    if cfg.mpv_path.is_empty() {
        return Err("mpv 路径未配置".into());
    }

    // 直链预检（带 Referer/UA；只看状态码，不读 body）
    {
        let client = http();
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
            .map_err(|e| format!("直链不可达: {e}"))?;
        let status = resp.status();
        if !(status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT) {
            return Err(format!("直链返回 {status}，源可能失效"));
        }
        crate::libmpv::trace(&format!("[webrs] 直链预检 ok: {status} {url}"));
    }

    let _ = app.emit("mpv-load", json!({ "phase": "start" }));

    let mut slot = state.mpv.lock().await;
    let session = match slot.as_ref() {
        Some(m) if !m.is_dead().await => m.clone(),
        Some(m) => {
            m.shutdown().await;
            crate::mpv::MpvSession::launch(app.clone(), &cfg.mpv_path).await?
        }
        None => crate::mpv::MpvSession::launch(app.clone(), &cfg.mpv_path).await?,
    };
    *slot = Some(session.clone());
    drop(slot);

    // Referer/UA 先写属性再 loadfile（对本地 NAS 播放无影响；play_item 播本地前会清）
    if let Some(r) = referer.as_deref().filter(|s| !s.is_empty()) {
        let _ = session.command(json!(["set", "referrer", r])).await;
    } else {
        let _ = session.command(json!(["set", "referrer", ""])).await;
    }
    if let Some(u) = ua.as_deref().filter(|s| !s.is_empty()) {
        let _ = session.command(json!(["set", "user-agent", u])).await;
    }
    // [lc-1506] Referer/UA 再写一份 http-header-fields：确保 ffmpeg(hls) 分段请求
    // 也带上（mpv 的 referrer 属性只覆盖部分加载路径）
    {
        let mut hf: Vec<String> = Vec::new();
        if let Some(r) = referer.as_deref().filter(|s| !s.is_empty()) {
            hf.push(format!("Referer: {r}"));
        }
        if let Some(u) = ua.as_deref().filter(|s| !s.is_empty()) {
            hf.push(format!("User-Agent: {u}"));
        }
        if !hf.is_empty() {
            let _ = session
                .command(json!(["set", "http-header-fields", hf.join(", ")]))
                .await;
        }
    }
    session.command(json!(["loadfile", url, "replace"])).await?;
    Ok(())
}
