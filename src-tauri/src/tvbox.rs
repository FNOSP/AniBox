//! [lc-2336] TVBox 配置导入（单链接）。
//!
//! 背景：用户希望像 TVBox 一样「填一个链接就自动出很多资源」。实测结论必须先说清：
//!   · TVBox 生态（CatVod / FongMi）的绝大多数源是 **`type:3` 的 `csp_xxx` 插件源 /
//!     `jar` 源 / `drpy` JS 源**，它们依赖 **Android Dalvik + DexClassLoader** 以及
//!     随包 native 库才能执行（实测 `饭太硬` 的 spider 包内是
//!     `classes.dex` + `libftyguard_v7/v8.so` + 1.2MB 加密 `ftyshinidie.guard`）。
//!     AniBox 是 Tauri + Rust，**没有 Java 运行时**，这类源无法接入，也不打算为此引入
//!     一个 Android 运行时。
//!   · 配置里真正能被 AniBox 直接消费的只有两类：
//!       1. **苹果CMS V10 采集源**（`type:1` 的 `api.php/provide/vod`）→ 我们的
//!          delimited 采集源体系（搜索/选集/直链全套现成）；
//!       2. **`lives` 段的 M3U 直播源**（`type:0` 的 url）→ 发现页 IPTV 源。
//!
//! 所以本模块只做**最简单的解析**：抓 → 解隐写 → 解析 JSON → 抽这两类 → 如实报告跳过的数量。
//!
//! 抓取契约（实测于 2026-11-24）：
//!   · 该站按 UA 分流：浏览器 UA 得到导航 HTML，`okhttp/*` 得到一张 JPEG；
//!   · 真正的配置**以 base64 拼在 JPEG 的 `FFD9` 之后**（CatVod 常见隐写手法）；
//!   · 配置 JSON 允许 `//` 行注释与尾逗号（CatVod 的宽容解析）。
//!
//! 本模块只做纯逻辑（抓取 + 解码 + 抽取），落库由 `tvbox_import` 命令按既有
//! `webrs_add_rule` / `webrs_maccms` 口径写入，不新增存储位置。

use base64::Engine as _;
use serde_json::{json, Value};

/// 抓取用的 UA：与 TVBox 客户端一致（该站按 UA 分流，浏览器 UA 只会拿到导航页）。
const TVBOX_UA: &str = "okhttp/3.12.0";
/// 配置体积上限（防止误填大文件地址把内存打满）。
const MAX_CONFIG_BYTES: usize = 8 * 1024 * 1024;

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .user_agent(TVBOX_UA)
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .map_err(|e| format!("HTTP 客户端初始化失败: {e}"))
}

/// 归一化用户填的链接：裸主机补 `http://`，只接受 http(s)。
pub fn normalize_url(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("地址为空".into());
    }
    let with_scheme = if s.contains("://") {
        s.to_string()
    } else {
        format!("http://{s}")
    };
    let u = url::Url::parse(&with_scheme).map_err(|e| format!("地址无效：{e}"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(format!("{}: 只支持 http/https", u.scheme()));
    }
    if u.host_str().unwrap_or("").is_empty() {
        return Err("缺少主机名".into());
    }
    Ok(u.to_string())
}

/// 从响应体里取出配置文本：明文 JSON 直接用；否则尝试「图片尾部 base64 隐写」。
///
/// 隐写判据：图片尾部跟一段 base64。**不依赖具体的图片格式前缀，也不假设偏移** ——
/// 把所有足够长的 base64 连续串从后往前试解，第一个「解出来是 JSON 对象」的即采纳
/// （隐写的在尾部，所以从后往前；阈值取 64 而不是几百，小配置也能认出来）。
pub fn extract_config_text(bytes: &[u8]) -> Result<String, String> {
    // 1) 明文 JSON（部分配置直接是文本）
    if let Ok(s) = std::str::from_utf8(bytes) {
        let t = s.trim_start_matches('\u{feff}').trim_start();
        if t.starts_with('{') {
            return Ok(s.to_string());
        }
    }
    // 2) 图片尾部的 base64 块
    let runs = base64_runs(bytes, 64);
    let mut last_err = String::from("返回内容既不是 JSON，也没有找到可解码的数据块");
    for run in runs.iter().rev() {
        let cleaned: Vec<u8> = run
            .iter()
            .copied()
            .filter(|b| !matches!(*b, b'\r' | b'\n' | b' ' | b'\t'))
            .collect();
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(pad_b64(&cleaned)) else {
            continue;
        };
        let Ok(text) = String::from_utf8(decoded) else {
            continue;
        };
        if text.trim_start().starts_with('{') {
            return Ok(text);
        }
        last_err = "解码结果不是 TVBox 配置 JSON".into();
    }
    Err(last_err)
}

/// 补 `=` padding 到 4 的倍数（base64 标准库要求严格长度）。
fn pad_b64(b: &[u8]) -> Vec<u8> {
    let mut out = b.to_vec();
    while out.len() % 4 != 0 {
        out.push(b'=');
    }
    out
}

/// 收集响应体里所有长度 ≥ `min` 的 base64 连续串（按出现顺序）。
fn base64_runs(bytes: &[u8], min: usize) -> Vec<Vec<u8>> {
    let is_b64 = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b'\r' | b'\n');
    let mut out = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    for &c in bytes {
        if is_b64(c) {
            cur.push(c);
        } else {
            push_run(&mut out, std::mem::take(&mut cur), min);
            cur.clear();
        }
    }
    push_run(&mut out, cur, min);
    out
}

fn push_run(out: &mut Vec<Vec<u8>>, run: Vec<u8>, min: usize) {
    let meaningful = run.iter().filter(|x| !matches!(**x, b'\r' | b'\n')).count();
    if meaningful >= min {
        out.push(run);
    }
}

/// 把 CatVod 风格的宽松 JSON 收严：去 `//` 行注释（不碰字符串内）、去尾逗号。
///
/// 只处理这两种实际出现的宽容语法；其余（`/* */`、单引号）宁可解析失败也不要猜。
pub fn strip_json_comments(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_str = false;
    let mut esc = false;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                // 跳过到行尾（换行保留，避免把两行粘成一个 token）
                while let Some(&n) = chars.peek() {
                    if n == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            _ => out.push(c),
        }
    }
    strip_trailing_commas(&out)
}

/// 去掉 `,` 后面只跟空白就遇到 `}` / `]` 的尾逗号（同样不碰字符串内）。
fn strip_trailing_commas(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut esc = false;
    for (i, &c) in b.iter().enumerate() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_whitespace() {
                j += 1;
            }
            if j < b.len() && matches!(b[j], '}' | ']') {
                continue; // 丢弃这个尾逗号
            }
        }
        out.push(c);
    }
    out
}

// ───────────────────────── 抽取：苹果CMS 采集源 ─────────────────────────

/// 抽出的苹果CMS 采集源（直接映射到 webrs 的 delimited 规则）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacSource {
    pub name: String,
    pub api: String,
    pub referer: String,
}

/// 判断是不是苹果CMS V10 标准的 `api.php/provide/vod` 接口。
///
/// 只看路径（允许 `?ac=…` 查询串与尾斜杠），且 `vod` 之后必须是边界 ——
/// `provide/vod2` 这类变体不是标准接口，收进来只会得到一个必然失败的源。
pub fn is_maccms_api(api: &str) -> bool {
    let low = api.to_ascii_lowercase();
    if !(low.starts_with("http://") || low.starts_with("https://")) {
        return false;
    }
    let path = low.split(['?', '#']).next().unwrap_or("");
    if !path.contains("api.php") {
        return false;
    }
    let Some(idx) = path.find("/provide/vod") else {
        return false;
    };
    let after = &path[idx + "/provide/vod".len()..];
    // 允许 '/', 空（结尾）; 不允许 '2' / 'x' 这类续写
    after.is_empty() || after.starts_with('/')
}

/// `type:1` 站点的 `api` 是接口地址本身；`ext` 可能是字符串（接口）或对象（带 `siteUrl`）。
/// 另外兼容 `api` 直接写接口、`ext` 写接口的两种社区写法。
fn maccms_api_of(site: &Value) -> Option<String> {
    let cand = |v: Option<&Value>| -> Option<String> {
        let s = v?.as_str()?.trim();
        if is_maccms_api(s) {
            Some(s.to_string())
        } else {
            None
        }
    };
    if let Some(a) = cand(site.get("api")) {
        return Some(a);
    }
    cand(site.get("ext"))
}

/// 从配置 JSON 抽苹果CMS 采集源（`type:1`，以及少数把接口写在 `ext` 的站点）。
///
/// 去重口径：同接口地址只留第一条（不同名字指向同一接口在 TVBox 里很常见）。
pub fn extract_maccms_sources(cfg: &Value) -> Vec<MacSource> {
    let mut out: Vec<MacSource> = Vec::new();
    let Some(sites) = cfg.get("sites").and_then(|v| v.as_array()) else {
        return out;
    };
    for s in sites {
        let Some(api) = maccms_api_of(s) else {
            continue;
        };
        let name = s
            .get("name")
            .and_then(|n| n.as_str())
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(clean_source_name)
            .unwrap_or_else(|| host_of(&api));
        if out.iter().any(|x| x.api == api) {
            continue;
        }
        // 防盗链：`ext` 对象里的 siteUrl 可作为 Referer（与源站同域时最稳）
        let referer = s
            .get("ext")
            .and_then(|e| e.get("siteUrl"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        out.push(MacSource { name, api, referer });
    }
    out
}

/// 站点名里常带 emoji 与竖线装饰（`🌞光影┃不卡`），去掉后才是能读的源名。
/// 规则源名会出现在源列表与搜索分组标题里，保持简短可读。
pub fn clean_source_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| {
            !matches!(*c,
                '\u{1F300}'..='\u{1FAFF}'
                | '\u{2600}'..='\u{27BF}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{1F000}'..='\u{1F2FF}'
                | '┃' | '｜' | '|'
            )
        })
        .collect();
    let trimmed = cleaned.trim().to_string();
    if trimmed.is_empty() {
        raw.trim().to_string()
    } else {
        trimmed
    }
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "TVBox 源".to_string())
}

// ───────────────────────── 抽取：直播源 ─────────────────────────

/// 抽 `lives` 段里**可直接当 M3U 订阅**的地址：
///   · `type:0` + 以 `.m3u`/`.m3u8` 结尾 → 原样；
///   · `type:0` + 其它（`.txt` / `.php` 网关）→ 也算候选，交由 IPTV 解析时判活。
/// `lives` 里 `type:1` 是 TVBox 的 txt 频道写法（`url#分组#频道`），不是订阅地址，跳过。
pub fn extract_live_sources(cfg: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(lives) = cfg.get("lives").and_then(|v| v.as_array()) else {
        return out;
    };
    for l in lives {
        let ty = l.get("type").and_then(|v| v.as_i64()).unwrap_or(0);
        if ty != 0 {
            continue;
        }
        let Some(u) = l.get("url").and_then(|v| v.as_str()).map(str::trim) else {
            continue;
        };
        // 相对路径（./list.txt）依赖 spider jar 的基址，我们没有那个上下文，跳过
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            continue;
        }
        if out.iter().any(|x| x == u) {
            continue;
        }
        out.push(u.to_string());
    }
    out
}

// ───────────────────────── 抽取：统计（如实报告跳过） ─────────────────────────

/// 源类型统计：告诉用户「为什么只有这么几个」而不是静默丢。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteStats {
    /// 苹果CMS 采集源（可导入）
    pub maccms: usize,
    /// CSP 插件源（`csp_*`，需要 Android 运行时）
    pub csp: usize,
    /// jar 源（spider dict 里的 key，需要 Android 运行时）
    pub jar: usize,
    /// drpy / js 源（需要 JS 引擎 + 站点的 drpy 运行时）
    pub js: usize,
    /// 其它（xml 站点、无法判定）
    pub other: usize,
}

pub fn site_stats(cfg: &Value) -> SiteStats {
    let mut st = SiteStats::default();
    let Some(sites) = cfg.get("sites").and_then(|v| v.as_array()) else {
        return st;
    };
    for s in sites {
        let api = s.get("api").and_then(|v| v.as_str()).unwrap_or("").trim();
        if maccms_api_of(s).is_some() {
            st.maccms += 1;
        } else if api.starts_with("csp_") {
            st.csp += 1;
        } else if api.contains(".js") || api.to_ascii_lowercase().contains("drpy") {
            st.js += 1;
        } else if !api.is_empty() && !api.starts_with("http") {
            // 形如 "douban" / "magnet" 的 key = 引用配置自身 spider dict 的 jar 源
            st.jar += 1;
        } else {
            st.other += 1;
        }
    }
    st
}

// ───────────────────────── 抓取 ─────────────────────────

/// 抓一个 TVBox 配置链接，返回（配置 JSON, 原始字节长度, 是否走了隐写解码）。
pub async fn fetch_config(url: &str) -> Result<(Value, usize, bool), String> {
    let url = normalize_url(url)?;
    let resp = client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("请求失败：{e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取响应失败：{e}"))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "配置过大（{}MB），已拒绝解析",
            bytes.len() / 1024 / 1024
        ));
    }
    let raw_len = bytes.len();
    let text = extract_config_text(&bytes)?;
    let stego = !text.trim_start().starts_with('{') || raw_len > text.len();
    let cleaned = strip_json_comments(&text);
    let cfg: Value = serde_json::from_str(&cleaned)
        .map_err(|e| format!("配置 JSON 解析失败：{e}（可能是私有加密格式）"))?;
    if !cfg.is_object() {
        return Err("配置不是 JSON 对象".into());
    }
    Ok((cfg, raw_len, stego))
}

// ───────────────────────── [lc-2336] 导入命令 ─────────────────────────

/// 预览结果里每条候选的动作字典（前端直接渲染，不二次判断）。
fn mac_rule_json(src: &MacSource) -> Value {
    let base = url::Url::parse(&src.api)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_default();
    json!({
        "name": src.name,
        "version": "1.0.0",
        "baseURL": base,
        "referer": src.referer,
        "searchMode": "api",
        "searchApiConfig": {
            "request": { "method": "GET", "url": src.api, "query": { "ac": "videolist", "wd": "@keyword" } },
            "listPath": "$.list[*]",
            "namePath": "$.vod_name",
            "sourcePath": "$.vod_id"
        },
        "chapterMode": "api",
        "chapterApiConfig": {
            "request": { "method": "GET", "url": src.api, "query": { "ac": "detail", "ids": "@source" } },
            "format": "delimited",
            "roadNamesPath": "$.list[*].vod_play_from",
            "roadEpisodesPath": "$.list[*].vod_play_url"
        }
    })
}

/// [lc-2336] TVBox 配置导入（单链接）。`apply=false` 只预览，`apply=true` 落库。
///
/// 落库口径（**不新增存储**）：
///   · 苹果CMS 采集源 → 复用 `webrs` 的 delimited 规则模板，写进 webrs-maccms/；
///   · 直播源 → 追加进 `config.iptv_sources`（受 IPTV 的 4 条上限约束，超出的如实报告）。
///
/// 返回结构给前端直接渲染：
/// `{ url, rawBytes, stego, stats, maccms:[{name,api,installed}], lives:[...], iptvAdded, skipped }`
#[tauri::command]
pub async fn tvbox_import(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    url: String,
    apply: Option<bool>,
) -> Result<Value, String> {
    let (cfg, raw_len, stego) = fetch_config(&url).await?;
    let stats = site_stats(&cfg);
    let macs = extract_maccms_sources(&cfg);
    let lives = extract_live_sources(&cfg);
    let apply = apply.unwrap_or(false);

    // 已装判定：按源名（与 webrs 的落盘文件名同一口径）
    let existing = crate::webrs::load_rules(&app).await.unwrap_or_default();
    let mut installed: Vec<String> = Vec::new();
    if apply {
        for src in &macs {
            let rule = mac_rule_json(src);
            // 复用现有入库路径（含重名检查）；已存在则跳过，不覆盖用户改动
            match crate::webrs::webrs_add_rule(app.clone(), rule.to_string()).await {
                Ok(n) => installed.push(n),
                Err(e) => {
                    if e.contains("已存在") {
                        installed.push(src.name.clone());
                    } else {
                        return Err(format!("导入「{}」失败：{e}", src.name));
                    }
                }
            }
        }
    } else {
        installed = macs
            .iter()
            .filter(|s| existing.iter().any(|r| r.name == s.name))
            .map(|s| s.name.clone())
            .collect();
    }

    // 直播源：合并进现有 IPTV 源（去重、上限由 iptv::normalize_sources 统一裁）
    let mut iptv_added = 0usize;
    let mut iptv_overflow = 0usize;
    if apply && !lives.is_empty() {
        let mut cfg_now = state.config.read().await.clone();
        let (current, _custom) = crate::iptv::resolve_sources(&cfg_now);
        // 逐条归一化后再比对：配置里的地址和我们落盘形态可能差一个 /interface.m3u
        let norm_lives: Vec<String> = lives
            .iter()
            .filter_map(|u| crate::iptv::normalize_source(u).ok())
            .collect();
        let mut merged = current.clone();
        for u in &norm_lives {
            if !merged.iter().any(|x| x == u) {
                merged.push(u.clone());
            }
        }
        let normalized = crate::iptv::normalize_sources(&merged)?;
        iptv_added = norm_lives
            .iter()
            .filter(|u| normalized.contains(u) && !current.contains(u))
            .count();
        iptv_overflow = merged.len().saturating_sub(normalized.len());
        cfg_now.iptv_sources = normalized;
        crate::config::save(&app, &cfg_now).await?;
        *state.config.write().await = cfg_now;
    }

    let mac_out: Vec<Value> = macs
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "api": s.api,
                "referer": s.referer,
                "installed": installed.iter().any(|n| n == &s.name),
            })
        })
        .collect();

    // 跳过说明：让用户看到「为什么只有这么几个」，而不是以为解析失败
    let mut skipped: Vec<String> = Vec::new();
    if stats.csp > 0 {
        skipped.push(format!("{} 个 csp 插件源（需 Android 运行时，无法接入）", stats.csp));
    }
    if stats.jar > 0 {
        skipped.push(format!("{} 个 jar 源（需 Android 运行时，无法接入）", stats.jar));
    }
    if stats.js > 0 {
        skipped.push(format!("{} 个 drpy/JS 源（需 TVBox 的 JS 运行时，无法接入）", stats.js));
    }
    if stats.other > 0 {
        skipped.push(format!("{} 个其它类型源（非苹果CMS 接口）", stats.other));
    }

    Ok(json!({
        "url": url,
        "rawBytes": raw_len,
        "stego": stego,
        "applied": apply,
        "stats": {
            "maccms": stats.maccms, "csp": stats.csp, "jar": stats.jar,
            "js": stats.js, "other": stats.other,
        },
        "maccms": mac_out,
        "lives": lives,
        "iptvAdded": iptv_added,
        "iptvOverflow": iptv_overflow,
        "skipped": skipped,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 图片尾部 base64 隐写：从 JPEG 结束标记后重新解出配置。
    #[test]
    fn extracts_config_from_trailing_base64() {
        let cfg = r#"{"sites":[{"name":"测试源","type":1,"api":"https://a.com/api.php/provide/vod"}]}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(cfg);
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        bytes.extend_from_slice(b"JFIF-only-binary-not-base64-long-enough");
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        bytes.extend_from_slice(b"**");
        bytes.extend_from_slice(b64.as_bytes());
        let text = extract_config_text(&bytes).expect("应解出配置");
        assert!(text.contains("api.php/provide/vod"));
    }

    /// 明文 JSON（含 BOM）原样返回，不走隐写分支。
    #[test]
    fn accepts_plain_json_with_bom() {
        let mut bytes = "\u{feff}".as_bytes().to_vec();
        bytes.extend_from_slice(br#"{"sites":[]}"#);
        let text = extract_config_text(&bytes).unwrap();
        assert!(text.contains("\"sites\""));
    }

    /// 既不是 JSON 也没有 base64 块 → 明确报错，不返回空配置。
    #[test]
    fn rejects_content_without_config() {
        assert!(extract_config_text(b"<html><body>not found</body></html>").is_err());
    }

    /// CatVod 宽容语法：`//` 注释（含行内注释里的引号）与尾逗号都能收严。
    #[test]
    fn strips_comments_and_trailing_commas() {
        let raw = "{\n // 这是注释, 带\"引号\"\n \"a\": 1, // 尾注释\n \"b\": [1, 2,],\n}";
        let cleaned = strip_json_comments(raw);
        let v: Value = serde_json::from_str(&cleaned).expect("应能解析");
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"].as_array().unwrap().len(), 2);
    }

    /// 字符串里的 `//`（URL）不能被当注释吃掉。
    #[test]
    fn keeps_slashes_inside_strings() {
        let raw = r#"{"api":"https://a.com/api.php/provide/vod"}"#;
        assert_eq!(strip_json_comments(raw), raw);
    }

    /// 只抽苹果CMS 采集源，csp / jar / js 一律跳过并计数。
    #[test]
    fn extracts_only_maccms_sources() {
        let cfg: Value = serde_json::from_str(
            r#"{"sites":[
                {"name":"🌞光影┃不卡","type":3,"api":"csp_T4Guard","ext":"AAAA"},
                {"name":"量子资源","type":1,"api":"https://cj.lziapi.com/api.php/provide/vod/"},
                {"name":"重复同接口","type":1,"api":"https://cj.lziapi.com/api.php/provide/vod/"},
                {"name":"虎牙js","type":3,"api":"./lib/drpy2.min.js","ext":"./js/huya.js"},
                {"name":"内置key","type":3,"api":"douban"}
            ]}"#,
        )
        .unwrap();
        let srcs = extract_maccms_sources(&cfg);
        assert_eq!(srcs.len(), 1, "同接口应去重：{srcs:?}");
        assert_eq!(srcs[0].name, "量子资源");
        assert_eq!(srcs[0].api, "https://cj.lziapi.com/api.php/provide/vod/");

        let st = site_stats(&cfg);
        assert_eq!(st.maccms, 2); // 含被去重的那条（统计口径按站点数）
        assert_eq!(st.csp, 1);
        assert_eq!(st.js, 1);
        assert_eq!(st.jar, 1);
    }

    /// 苹果CMS 接口识别：只看路径，查询串不影响；非 http(s) 一律不算。
    #[test]
    fn detects_maccms_api_shapes() {
        assert!(is_maccms_api("https://a.com/api.php/provide/vod"));
        assert!(is_maccms_api("https://a.com/api.php/provide/vod/?ac=videolist"));
        assert!(is_maccms_api("HTTP://A.COM/API.PHP/PROVIDE/VOD"));
        assert!(!is_maccms_api("https://a.com/api.php/provide/vod2"));
        assert!(!is_maccms_api("csp_T4Guard"));
        assert!(!is_maccms_api("/api.php/provide/vod"));
    }

    /// 直播源：只收 type 0 的绝对地址，相对路径与 txt 频道写法跳过。
    #[test]
    fn extracts_live_sources_only_absolute_type0() {
        let cfg: Value = serde_json::from_str(
            r#"{"lives":[
                {"name":"A","type":0,"url":"https://sub.ottiptv.cc/a.m3u"},
                {"name":"B","type":0,"url":"https://sub.ottiptv.cc/a.m3u"},
                {"name":"C","type":0,"url":"./list.txt"},
                {"name":"D","type":1,"url":"http://h/x#组#频道"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            extract_live_sources(&cfg),
            vec!["https://sub.ottiptv.cc/a.m3u".to_string()]
        );
    }

    /// 源名清洗：去 emoji 与竖线装饰，保留可读中文。
    #[test]
    fn cleans_source_names() {
        assert_eq!(clean_source_name("🌞光影┃不卡"), "光影不卡");
        assert_eq!(clean_source_name("量子资源"), "量子资源");
        assert_eq!(clean_source_name("🎮"), "🎮"); // 全是装饰 → 回落原文
    }

    /// 裸主机补协议；非 http(s) 报错。
    #[test]
    fn normalizes_urls() {
        assert_eq!(normalize_url("example.com/tv").unwrap(), "http://example.com/tv");
        assert_eq!(
            normalize_url(" https://a.com/tv ").unwrap(),
            "https://a.com/tv"
        );
        assert!(normalize_url("ftp://a.com/x").is_err());
        assert!(normalize_url("   ").is_err());
    }
}
