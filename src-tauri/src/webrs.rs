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
    pub chapter_mode: String, // 本实现只支持 "xpath"
    #[serde(rename = "searchApiConfig")]
    pub search_api_config: Option<ApiSearchConfig>,
}

/// [lc-1500] API 搜索配置（如 mutefun：`/index.php/ajax/suggest`）。
/// JSONPath 只实现子集：`$.list[*]` 这类「字段名 + [*] 通配」。
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

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ApiRequest {
    pub method: String,
    pub url: String,
    pub query: std::collections::HashMap<String, String>,
}

// ───────────────────────── 内置默认规则（Predidit/KazumiRules 原文） ─────────────────────────

const DEFAULT_RULES: &[&str] = &[
    r#"{
    "api": "1",
    "type": "anime",
    "name": "AGE",
    "version": "1.5",
    "muliSources": true,
    "useWebview": true,
    "useNativePlayer": true,
    "userAgent": "",
    "baseURL": "https://www.agedm.io/",
    "searchURL": "https://www.agedm.io/search?query=@keyword",
    "searchList": "//div[2]/div/section/div/div/div/div",
    "searchName": "//div/div[2]/h5/a",
    "searchResult": "//div/div[2]/h5/a",
    "chapterRoads": "//div[2]/div/section/div/div[2]/div[2]/div[2]/div",
    "chapterResult": "//ul/li/a"
}"#,
    r#"{
    "api": "8",
    "type": "anime",
    "name": "mutefun",
    "version": "1.3",
    "muliSources": true,
    "useWebview": true,
    "useNativePlayer": true,
    "usePost": false,
    "useLegacyParser": false,
    "adBlocker": false,
    "userAgent": "",
    "baseURL": "https://www.2kdm.com/voddetail/",
    "searchMode": "api",
    "chapterMode": "xpath",
    "searchApiConfig": {
        "request": {
            "method": "GET",
            "url": "https://www.2kdm.com/index.php/ajax/suggest",
            "query": {
                "mid": "1",
                "wd": "@keyword"
            }
        },
        "listPath": "$.list[*]",
        "namePath": "$.name",
        "sourcePath": "$.id"
    },
    "searchURL": "",
    "searchList": "",
    "searchName": "",
    "searchResult": "",
    "chapterRoads": "//div[contains(@class,'module-play-list-content')]",
    "chapterResult": ".//a[contains(@class,'module-play-list-link')]",
    "referer": ""
}"#,
];

pub fn rules_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {e}"))?
        .join("webrs-rules");
    Ok(dir)
}

/// 首次运行时把内置规则写进 webrs-rules/（已有同名文件不覆盖，尊重用户改动）。
async fn seed_rules(app: &AppHandle) -> Result<(), String> {
    let dir = rules_dir(app)?;
    if dir.exists() {
        return Ok(());
    }
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("创建规则目录失败: {e}"))?;
    for body in DEFAULT_RULES {
        let name = serde_json::from_str::<Rule>(body)
            .ok()
            .map(|r| r.name)
            .unwrap_or_else(|| "rule".into());
        let p = dir.join(format!("{name}.json"));
        if !p.exists() {
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
}

#[derive(Debug, Clone)]
enum Test {
    Tag(String),
    Any,
    Attr(String),
    Text,
}

#[derive(Debug, Clone)]
struct Step {
    desc: bool, // true = `//`（后代轴），false = `/`（子轴）
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

fn parse_step(raw: &str, desc: bool) -> Result<Step, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("空 XPath 步进".into());
    }
    if let Some(attr) = raw.strip_prefix('@') {
        return Ok(Step {
            desc,
            test: Test::Attr(attr.trim().to_string()),
            preds: Vec::new(),
        });
    }
    if raw == "text()" {
        return Ok(Step {
            desc,
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
    Ok(Step { desc, test, preds })
}

/// 解析整条 XPath（`.` 开头 = 相对上下文求值）。
fn parse_xpath(expr: &str) -> Result<Vec<Step>, String> {
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
        let desc = if s[i..].starts_with("//") {
            i += 2;
            true
        } else if s[i..].starts_with('/') {
            i += 1;
            false
        } else if steps.is_empty() && relative {
            // ".tag" 形态不合法，但 ".//a" 已在上面被剥成 "//a" / "/a"，这里兜住
            false
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
        steps.push(parse_step(&s[start..i], desc)?);
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
            })
        })
        .map(|(_, id)| *id)
        .collect()
}

fn eval_xpath(doc: &Html, ctx: &[NodeId], steps: &[Step]) -> Result<EvalOut, String> {
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
                    if st.desc {
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
                    } else {
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

/// [lc-1500] 规则搜索：返回 [{name, url}]。api 模式走 JSONPath 子集（sourcePath
/// 若不是 URL， Chapters 阶段不支持 —— 这类规则直接提示）。
#[tauri::command]
pub async fn webrs_search(app: AppHandle, rule_name: String, keyword: String) -> Result<Value, String> {
    let rules = load_rules(&app).await?;
    let rule = find_rule(&rules, &rule_name)?.clone();
    let ua = rule_ua(&rule);
    let referer = rule_referer(&rule);
    let keyword = keyword.trim().to_string();
    if keyword.is_empty() {
        return Err("搜索词为空".into());
    }
    let encoded = urlencoding::encode(&keyword).to_string();

    let items: Vec<(String, String)> = if rule.search_mode == "api" {
        let cfg = rule.search_api_config.clone().ok_or("规则缺少 searchApiConfig")?;
        let mut url = cfg.request.url.replace("@keyword", &encoded);
        if !url.starts_with("http") {
            url = join_url(&rule.base_url, &url).ok_or("searchApiConfig.url 无法拼接")?;
        }
        let req = crate::libmpv::trace(&format!("[webrs] api 搜索: {url}"));
        let _ = req;
        let body = if cfg.request.method.eq_ignore_ascii_case("post") {
            // POST 模式：query → form body（对齐 Kazumi PreparedRuleRequest）。
            // ⚠ 传**原始关键词**：form 序列化会自己编码，先 encode 过会双重编码。
            let form: Vec<(String, String)> = cfg
                .request
                .query
                .iter()
                .map(|(k, v)| (k.clone(), v.replace("@keyword", &keyword)))
                .collect();
            let resp = http()
                .post(&url)
                .header("user-agent", &ua)
                .header("referer", &referer)
                .form(&form)
                .send()
                .await
                .map_err(|e| format!("API 搜索请求失败: {e}"))?;
            resp.text().await.map_err(|e| format!("读响应失败: {e}"))?
        } else {
            let mut q: Vec<(String, String)> = cfg
                .request
                .query
                .iter()
                .map(|(k, v)| (k.clone(), v.replace("@keyword", &keyword)))
                .collect();
            if q.is_empty() {
                fetch_text(&url, &ua, &referer).await?
            } else {
                // url 可能自带 query，用 Url 合并
                let mut u = Url::parse(&url).map_err(|e| format!("URL 解析失败: {e}"))?;
                for (k, v) in q.drain(..) {
                    u.query_pairs_mut().append_pair(&k, &v);
                }
                fetch_text(u.as_str(), &ua, &referer).await?
            }
        };
        // JSONPath 子集：$.a.b[*] 取列表；namePath/sourcePath 相对每项取字段
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

    if items.is_empty() {
        return Err("没有搜到结果，换个源或换关键词试试".into());
    }
    Ok(json!(items
        .into_iter()
        .map(|(name, url)| json!({ "name": name, "url": url }))
        .collect::<Vec<_>>()))
}

/// JSONPath 极简子集：`$.a.b[*]`。返回路径终点数组（支持中间任意层 [*]）。
fn json_path<'a>(root: &'a Value, path: &str) -> Option<Vec<&'a Value>> {
    let p = path.trim().strip_prefix("$.")?;
    let mut cur: Vec<&Value> = vec![root];
    for seg in p.split('.') {
        let (key, wildcard) = match seg.strip_suffix("[*]") {
            Some(k) => (k, true),
            None => (seg, false),
        };
        let mut next = Vec::new();
        for v in &cur {
            let hit = v.get(key);
            if let Some(hit) = hit {
                if wildcard {
                    if let Some(arr) = hit.as_array() {
                        for it in arr {
                            next.push(it);
                        }
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
        Value::Number(n) => Some(n.to_string()), // `$.id` 这类数字字段（对齐 Dart toString）
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
    if rule.chapter_mode != "xpath" {
        return Err(format!(
            "规则「{}」的选集模式是 {}，本版本只支持 xpath",
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
        return Err("没解析到播放线路（规则 chapterRoads 不匹配？站点改版了？）".into());
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

fn sniff_script() -> String {
    r#"(function () {
  if (window.__aniboxSniff) return;
  window.__aniboxSniff = 1;
  var done = false;
  var seen = {};
  function ok(u) {
    if (!u || typeof u !== 'string' || done) return false;
    if (!/^https?:\/\//i.test(u)) return false;
    if (seen[u]) return false;
    return /\.(m3u8|mp4|flv)(\?|#|$)/i.test(u);
  }
  function emit(u) {
    if (!ok(u)) return;
    seen[u] = 1;
    done = true;
    var target = 'SCHEME://found?u=' + encodeURIComponent(u);
    // 播放器常嵌在 iframe 里：顶窗导航（framebusting，跨域 iframe 也被允许）
    // 才能被宿主的 on_navigation 拦到；失败再退回当前 frame。
    try { window.top.location.href = target; return; } catch (e) {}
    try { location.href = target; } catch (e) {}
  }
  try {
    var of = window.fetch;
    if (of) window.fetch = function (input) {
      try { emit(typeof input === 'string' ? input : (input && input.url) || ''); } catch (e) {}
      return of.apply(this, arguments);
    };
  } catch (e) {}
  try {
    var oo = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (m, u) {
      try { emit(u); } catch (e) {}
      return oo.apply(this, arguments);
    };
  } catch (e) {}
  try {
    if (window.Response) {
      var ot = Response.prototype.text;
      Response.prototype.text = function () {
        var p = ot.apply(this, arguments);
        var self = this;
        try {
          p.then(function (t) {
            try { if (t && t.indexOf('#EXTM3U') === 0) emit((self && self.url) || ''); } catch (e) {}
          });
        } catch (e) {}
        return p;
      };
    }
  } catch (e) {}
  function scan() {
    if (done) return;
    try {
      var els = document.querySelectorAll('video,source');
      for (var i = 0; i < els.length; i++) {
        emit(els[i].getAttribute('src') || '');
        emit(els[i].src || '');
      }
    } catch (e) {}
    try {
      // 页面 JS 里的直链常被写成 https:\/\/…（\/ 转义），先归一化再匹配
      var flat = document.documentElement.innerHTML.split('\\/').join('/');
      var m = flat.match(/https?:\/\/[^"'\\\s<>]{8,300}?\.m3u8(?:\?[^"'\\\s<>]{0,200})?/i);
      if (m) emit(m[0]);
      var m2 = flat.match(/["'](\/[^"'\\\s<>]{4,200}?\.m3u8(?:\?[^"'\\\s<>]{0,200})?)["']/i);
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
    .replace("SCHEME", SNIFF_SCHEME)
}

/// 隐藏 WebView 加载播放页，on_navigation 拦 `anibox-sniff://found?u=` 回传直链。
async fn webview_sniff(app: &AppHandle, page_url: &str, ua: &str, _referer: &str) -> Result<String, String> {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let app2 = app.clone();
    let page = page_url.to_string();
    let ua_owned = ua.to_string();
    let script = sniff_script();

    app.run_on_main_thread(move || {
        let tx2 = tx.clone();
        // 上一个残留的嗅探窗先销毁
        if let Some(w) = app2.get_webview_window(SNIFF_LABEL) {
            let _ = w.destroy();
        }
        let Ok(target) = Url::parse(&page) else {
            let _ = tx.send(Err(format!("播放页 URL 无效: {page}")));
            return;
        };
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
                for (k, v) in u.query_pairs() {
                    if k == "u" && !v.is_empty() {
                        let _ = tx2.send(Ok(v.to_string()));
                    }
                }
                return false; // 取消这次伪导航，页面保持原状
            }
            true
        });
        if let Err(e) = b.build() {
            let _ = tx.send(Err(format!("创建嗅探 WebView 失败: {e}")));
        }
    })
    .map_err(|e| format!("嗅探窗口调度失败: {e}"))?;

    // 等结果（最长 40s；window.close 后由 finally 清理）
    let wait = tokio::task::spawn_blocking(move || rx.recv_timeout(std::time::Duration::from_secs(40)))
        .await
        .map_err(|e| format!("嗅探等待失败: {e}"))?;

    // 清理嗅探窗
    let app3 = app.clone();
    let app4 = app.clone();
    let _ = app3.run_on_main_thread(move || {
        if let Some(w) = app4.get_webview_window(SNIFF_LABEL) {
            let _ = w.destroy();
        }
    });

    match wait {
        Ok(Ok(url)) => Ok(url),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("嗅探超时（40s 内没截到视频流）——该站点可能用了不支持的加密/验证方案，换一个源试试".into()),
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
    session.command(json!(["loadfile", url, "replace"])).await?;
    Ok(())
}
