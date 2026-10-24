//! [nas-subtitle] NAS 外挂字幕自动挂载：mpv `path` 事件 → fnOS stream/list →
//! 选最佳外挂字幕 → 下载到本地 → IPC `sub-add <path> select <title>`。
//!
//! **为什么需要这个模块（用户报「MPV 无法加载外挂字幕」的主根因之一）**：
//! AniBox 的库内播放走 Go 代理（`http://127.0.0.1:22346/api/v1/playvideo/…`），
//! 媒体对 mpv 是一个 **URL** —— mpv 的 `sub-auto` 只会扫描**本地媒体同目录**，
//! 对 URL 永远不工作 ⇒ NAS 同目录的外挂字幕（`.srt`/`.ass`）**从不自动加载**。
//! 主仓库（Fntv-Plus mpv.ts `mountSubtitlesWithRetry`）一直由应用侧在 path 事件后
//! 显式挂载，portable_config/mpv.conf 的 slang 注释也写明了这一约定；Tauri2 迁移时
//! 这条链路没带过来 ⇒ 迁移后外挂字幕只剩 uosc「加载字幕」菜单一条手动路。
//! 本模块把它补齐（契约对齐主仓库 fn_api/api.ts `getSubtitle` + mpv.ts
//! `mountSubtitlesWithRetry`，语义见 `pick_best`）。
//!
//! 与既有链路的关系：
//!   · uosc「加载字幕」菜单（fntv_nas_sub.lua → shim 22347 /nas-subtitle*）是**手动**全量菜单，
//!     与本模块的**自动**挂载互补；下载缓存同一目录（`%TEMP%/anibox-subtitles`、
//!     同样的 `{safe}@{guid}.{format}` 命名），两边互不重复下载。
//!   · 弹幕走 `secondary-sid`（uosc_danmaku sub-add auto + 副字幕轨）/ OSD，与本模块
//!     挂主字幕轨（`sid`）互不冲突。
//!   · 在线源（webrs 直链）、直播、本地文件播放不经本模块（`parse_proxy_url` 只认代理 URL）。

use crate::fnapi;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 与 Node shim（danmaku-shim.cjs）共用的字幕缓存目录：手动菜单与自动挂载互享缓存。
fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("anibox-subtitles")
}

// ───────────────────────── 播放 URL 解析（纯函数，可测） ─────────────────────────

/// 从代理播放 URL 提取的 fnOS 连接上下文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NasCtx {
    pub item_guid: String,
    pub token: String,
    pub domain: String,
    /// [lc-1763] 当前会话 token（播放 URL 里的 token 失效时的重试票；探针/测试可空）。
    pub fallback_token: String,
}

/// 判断是否为本工程代理播放 URL（`http://127.0.0.1:2234[67]/…/playvideo/<guid>?token&domain`）。
///
/// 与 fntv_nas_sub.lua 的 `current_fn_ctx` 同一套判据：webrs 在线直链、直播、本地文件
/// 一律返回 `None`（不触发自动挂载）。参数取值做一次百分号解码（Lua 侧原样透传、
/// Node `searchParams.get` 解码，这里对齐解码语义）。
pub fn parse_proxy_url(path: &str) -> Option<NasCtx> {
    let rest = path.strip_prefix("http://127.0.0.1:2234")?;
    let (port_tail, rest) = rest.split_at(1.min(rest.len()));
    if port_tail != "6" && port_tail != "7" {
        return None;
    }
    let (p, query) = match rest.split_once('?') {
        Some((p, q)) => (p, q),
        None => (rest, ""),
    };
    let idx = p.find("/playvideo/")?;
    let guid = p[idx + "/playvideo/".len()..].split('/').next().unwrap_or("");
    if guid.is_empty() {
        return None;
    }
    let mut token = String::new();
    let mut domain = String::new();
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "token" => token = v.into_owned(),
            "domain" => domain = v.into_owned(),
            _ => {}
        }
    }
    if token.is_empty() || domain.is_empty() {
        return None;
    }
    Some(NasCtx {
        item_guid: guid.to_string(),
        token,
        domain,
        fallback_token: String::new(),
    })
}

// ───────────────────────── 选轨逻辑（纯函数，可测；对齐主仓库 getSubtitle） ─────────────────────────

/// fnOS `stream/list` 里的一条字幕流（只取选轨需要的字段）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubStream {
    pub guid: String,
    pub title: String,
    pub format: String,
    pub language: String,
    pub is_default: i64,
    /// `extra_file=1`（与视频同目录的独立字幕文件；fnOS 现版 `is_external` 恒 0，
    /// 保留兼容旧版 —— 主仓库实测注释：只按 is_external 过滤会一条都挂不上）。
    pub external: bool,
}

/// 解析 stream/list 的 `data.subtitle_streams`（snake_case/camelCase 字段名都认）。
pub fn parse_streams(v: &Value) -> Vec<SubStream> {
    let Some(arr) = v.get("data").and_then(|d| d.get("subtitle_streams")).and_then(|s| s.as_array())
    else {
        return Vec::new();
    };
    let str_of = |o: &Value, keys: &[&str]| -> String {
        keys.iter()
            .find_map(|k| o.get(*k).and_then(|x| x.as_str()))
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let num_of = |o: &Value, keys: &[&str]| -> i64 {
        keys.iter()
            .find_map(|k| {
                o.get(*k).and_then(|x| {
                    x.as_i64().or_else(|| x.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
                })
            })
            .unwrap_or(0)
    };
    arr.iter()
        .filter_map(|o| {
            let guid = str_of(o, &["guid"]);
            if guid.is_empty() {
                return None;
            }
            let extra = num_of(o, &["extra_file"]) == 1 || num_of(o, &["is_external", "isExternal"]) == 1;
            Some(SubStream {
                guid,
                title: str_of(o, &["title"]),
                format: str_of(o, &["format"]),
                language: str_of(o, &["language"]),
                is_default: num_of(o, &["is_default", "isDefault"]),
                external: extra,
            })
        })
        .collect()
}

/// 中文轨道判定（对齐主仓库 isChineseSubtitle）：lang 前缀/关键词，或标题含中文/中字标记。
pub fn is_chinese(s: &SubStream) -> bool {
    let lang = s.language.to_lowercase();
    let starts = ["zh", "chi", "zho", "cmn", "yue"];
    if starts.iter().any(|p| lang.starts_with(p)) {
        return true;
    }
    if ["chinese", "中文", "国语", "普通话", "简体", "繁体"].iter().any(|k| lang.contains(k)) {
        return true;
    }
    let t = s.title.as_str();
    if t.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)) {
        return true; // 标题含中文字符（主仓库用 [一-鿿]，同一区间）
    }
    let lower = t.to_lowercase();
    ["中字", "简体", "繁体", "国语", "中文", "字幕", "chs", "cht", "gb", "big5"]
        .iter()
        .any(|k| lower.contains(k))
}

/// 标题是否只是语言标签/空/序号（不含片名信息）。
/// 这类字幕的归属由服务端 media_guid 绑定，标题匹配对它必然判 0，
/// 不能拿「标题不匹配」一票否决它（对齐主仓库 isLanguageLabelOnly）。
pub fn is_language_label_only(title: &str) -> bool {
    use std::sync::OnceLock;
    let re: &'static Option<regex::Regex> = {
        static RE: OnceLock<Option<regex::Regex>> = OnceLock::new();
        RE.get_or_init(|| {
            let lang = "(chinese|english|japanese|korean|french|german|spanish|portuguese|russian|italian\
                    |arabic|thai|vietnamese|indonesian|malay|simplified|traditional|mandarin|cantonese\
                    |zh|chi|zho|chs|cht|sc|tc|eng|jpn|kor|ja|en\
                    |简体|繁体|简中|繁中|中文|中字|国语|粤语|日语|英语|韩语|双语)";
            let pat = format!("^{lang}([\\s._\\-/|+&]*{lang})*[\\s._\\-()#\\d]*$");
            regex::RegexBuilder::new(&pat)
                .case_insensitive(true)
                .build()
                .ok()
        })
    };
    let t = title.trim();
    if t.is_empty() {
        return true;
    }
    if t.chars().all(|c| c.is_ascii_digit() || c.is_whitespace() || "._-()#".contains(c)) {
        return true;
    }
    re.as_ref().map(|r| r.is_match(&t.to_lowercase())).unwrap_or(false)
}

/// 标题归一化：去括号/空格/分隔符、去集数标记、去占位符噪声（对齐主仓库 norm：
/// `s\d+e?\d+` 与 `第?\d+[集话話]` 都剥掉再做互含比较）。
fn norm_title(s: &str) -> String {
    use std::sync::OnceLock;
    let sxxe: &'static Option<regex::Regex> = {
        static R: OnceLock<Option<regex::Regex>> = OnceLock::new();
        R.get_or_init(|| regex::Regex::new(r"s\d+e\d+").ok())
    };
    let ep: &'static Option<regex::Regex> = {
        static R: OnceLock<Option<regex::Regex>> = OnceLock::new();
        R.get_or_init(|| regex::Regex::new(r"第?\d+[集话話]").ok())
    };
    let lowered = s.to_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut prev_sep = false;
    for ch in lowered.chars() {
        if "[]()（）【】 ._-".contains(ch) {
            prev_sep = true;
            continue;
        }
        if prev_sep && !out.is_empty() {
            out.push(' ');
        }
        prev_sep = false;
        out.push(ch);
    }
    // 去 notitle/notvtitle 占位符与集数标记
    let cleaned: String = out
        .split_whitespace()
        .filter(|w| *w != "notitle" && *w != "notvtitle")
        .collect::<Vec<_>>()
        .join(" ");
    let mut cleaned = cleaned;
    if let Some(r) = sxxe {
        cleaned = r.replace_all(&cleaned, "").to_string();
    }
    if let Some(r) = ep {
        cleaned = r.replace_all(&cleaned, "").to_string();
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 抽标题里的集数标识（SxxExx / 第N集/话/話）并**归一化数字**（去前导零），
/// 用于「同集信号」判定：应用侧标题是 `S1E3`（无补零）、社区字幕常是 `S01E03`，
/// 不归一化的话同集会被判不同（比主仓库更宽一档，语义不变）。
fn episode_tokens(s: &str) -> Vec<String> {
    use std::sync::OnceLock;
    let sxxe: &'static Option<regex::Regex> = {
        static R: OnceLock<Option<regex::Regex>> = OnceLock::new();
        R.get_or_init(|| regex::Regex::new(r"s(\d+)e(\d+)").ok())
    };
    let ep: &'static Option<regex::Regex> = {
        static R: OnceLock<Option<regex::Regex>> = OnceLock::new();
        R.get_or_init(|| regex::Regex::new(r"第?(\d+)[集话話]").ok())
    };
    let mut out = Vec::new();
    if let Some(r) = sxxe {
        for cap in r.captures_iter(s) {
            let se: u64 = cap[1].parse().unwrap_or(0);
            let ep: u64 = cap[2].parse().unwrap_or(0);
            out.push(format!("s{se}e{ep}"));
        }
    }
    if let Some(r) = ep {
        for cap in r.captures_iter(s) {
            let n: u64 = cap[1].parse().unwrap_or(0);
            out.push(format!("ep{n}"));
        }
    }
    out
}

/// 标题是否含剧集集标识（SxxExx），用于区分电影/剧集（对齐主仓库 isEpisode）。
fn looks_like_episode(title: &str) -> bool {
    use std::sync::OnceLock;
    let re: &'static Option<regex::Regex> = {
        static R: OnceLock<Option<regex::Regex>> = OnceLock::new();
        R.get_or_init(|| regex::RegexBuilder::new(r"s\d+e\d+").case_insensitive(true).build().ok())
    };
    re.as_ref().map(|r| r.is_match(title)).unwrap_or(false)
}

/// 字幕标题与影片标题的匹配度：2=完全一致，1=互含或共享集数标识，0=无关。
/// 「共享 SxxExx」兼容季名写法差异（视频『无职转生Ⅲ S03E06』vs 字幕『无职转生 S03E06』）。
pub fn score_title(sub_title: &str, video_title: &str) -> i32 {
    if video_title.is_empty() {
        return 0;
    }
    let a = norm_title(sub_title);
    let b = norm_title(video_title);
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    if a == b {
        return 2;
    }
    if a.contains(&b) || b.contains(&a) {
        return 1;
    }
    let ea = episode_tokens(sub_title);
    let eb = episode_tokens(video_title);
    if !ea.is_empty() && ea.iter().any(|x| eb.contains(x)) {
        return 1;
    }
    0
}

/// 从 stream/list 全量流里选**最佳外挂字幕**（对齐主仓库 getSubtitle 的完整语义）：
///   1. 只留外挂文件（extra_file=1 / is_external=1）；内封轨由 mpv 按 slang 自选；
///   2. 优先中文；无中文外挂但容器已内封中文 → 不挂（select 会顶掉已选中的内封中文）；
///   3. 带「实义片名」的字幕参与标题匹配（剔除飞牛字幕元数据错配，如本集返回『佐罗』），
///      语言标签类标题不参与一票否决；剧集（标题含 SxxExx）无一匹配 → 宁缺毋滥；
///      电影无匹配仍退化为挂最佳一条；
///   4. 排序：is_default 优先 → 标题匹配度 → 标题字典序；取第一。
pub fn pick_best<'a>(streams: &'a [SubStream], video_title: Option<&str>) -> Option<&'a SubStream> {
    let external: Vec<&SubStream> = streams.iter().filter(|s| s.external).collect();
    if external.is_empty() {
        return None;
    }
    let embedded: Vec<&SubStream> = streams.iter().filter(|s| !s.external).collect();

    let chinese: Vec<&&SubStream> = external.iter().filter(|s| is_chinese(s)).collect();
    let pool: Vec<&&SubStream> = if chinese.is_empty() {
        if embedded.iter().any(|s| is_chinese(s)) {
            // 无中文外挂、但已有内封中文字幕轨：跳过挂载以免 select 顶掉内封中文
            return None;
        }
        external.iter().collect()
    } else {
        chinese
    };

    let vt = video_title.unwrap_or("");
    let matched: Vec<&&SubStream> = pool
        .iter()
        .filter(|s| is_language_label_only(&s.title) || score_title(&s.title, vt) > 0)
        .copied()
        .collect();

    let use_pool: Vec<&&SubStream> = if !matched.is_empty() {
        matched
    } else if !looks_like_episode(vt) {
        pool.clone() // 电影/单集（含标题未知）：退化为挂最佳一条，避免无字幕可用
    } else {
        return None; // 剧集且无一匹配：疑似飞牛字幕元数据错配，不挂
    };

    use_pool
        .iter()
        .max_by(|x, y| {
            // is_default 降序 → 标题匹配度降序 → 标题字典序升序
            (x.is_default, score_title(&x.title, vt))
                .cmp(&(y.is_default, score_title(&y.title, vt)))
                .then_with(|| y.title.to_lowercase().cmp(&x.title.to_lowercase()))
        })
        .map(|s| **s)
}

// ───────────────────────── fnOS 请求（stream/list + 字幕下载） ─────────────────────────

/// GET `/v/api/v1/stream/list/{guid}` → 全量字幕流（失败返回 Err）。
///
/// [lc-1763] 诊断加固：失败时把响应**原文**（前 300 字符）带进错误与日志 —— 此前只打
/// msg 字段，服务端返回无 msg 的错误码（如 -6 Not Found / 会话失效形态）时只剩「未知错误」，
/// 真机无法定位（实测 Node/Rust 探针同 guid 同凭据可成功，需原文比对 App 现场）。
/// `fallback_token` 非空且与 ctx.token 不同时，失败后用它重试一次：播放 URL 里的 token
/// 是拼 URL 那一刻的会话票，长会话/重登录后可能滞后于当前有效 token。
pub async fn fetch_streams(ctx: &NasCtx) -> Result<Vec<SubStream>, String> {
    match fetch_streams_with_token(ctx, &ctx.token).await {
        Ok(v) => Ok(v),
        Err(primary_err) => {
            if !ctx.fallback_token.is_empty() && ctx.fallback_token != ctx.token {
                crate::libmpv::trace(&format!(
                    "[nas-subtitle] stream/list 主 token 失败（{primary_err}），用当前会话 token 重试"
                ));
                match fetch_streams_with_token(ctx, &ctx.fallback_token).await {
                    Ok(v) => Ok(v),
                    Err(retry_err) => {
                        crate::libmpv::trace(&format!(
                            "[nas-subtitle] stream/list 重试仍失败：{retry_err}"
                        ));
                        Err(retry_err)
                    }
                }
            } else {
                Err(primary_err)
            }
        }
    }
}

async fn fetch_streams_with_token(ctx: &NasCtx, token: &str) -> Result<Vec<SubStream>, String> {
    let client = fnapi::new_client();
    let v = client
        .call(
            &ctx.domain,
            &format!("/v/api/v1/stream/list/{}", urlencoding::encode(&ctx.item_guid)),
            "GET",
            token,
            None,
        )
        .await?;
    if v.get("code").and_then(|c| c.as_i64()) != Some(0) {
        // 兼容 `success` 形态（主仓库 ApiResponse）：code!=0 或 success=false 都算失败
        if v.get("success").and_then(|s| s.as_bool()) == Some(false) {
            return Err(v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("stream/list 返回失败")
                .to_string());
        }
        let body = serde_json::to_string(&v).unwrap_or_default();
        let head: String = body.chars().take(300).collect();
        return Err(format!("stream/list 返回非 0（响应原文: {head}）"));
    }
    Ok(parse_streams(&v))
}

/// GET `/v/api/v1/subtitle/dl/{guid}` 下载字幕二进制到本地缓存（已存在即复用）。
///
/// 头部构造对齐 fnapi::FnClient::call_inner（Authx 签名 + Cookie + 网关授权 Cookie），
/// 但返回**字节**而不是 JSON —— 字幕本体是文本/二进制流，服务端错误才是 JSON。
async fn download_subtitle(ctx: &NasCtx, sub: &SubStream) -> Result<PathBuf, String> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建字幕缓存目录失败: {e}"))?;
    let safe: String = sub
        .title
        .chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string();
    let safe = if safe.is_empty() { sub.guid.clone() } else { safe };
    let fmt = if sub.format.is_empty() { "srt".to_string() } else { sub.format.clone() };
    let file = dir.join(format!("{safe}@{}.{}", sub.guid, fmt));
    if let Ok(md) = std::fs::metadata(&file) {
        if md.len() > 0 {
            return Ok(file); // 已缓存（手动菜单/上一集已下载过）
        }
    }

    let path = format!("/v/api/v1/subtitle/dl/{}", urlencoding::encode(&sub.guid));
    let client = fnapi::new_client();
    let mut base = fnapi::normalize_server(&ctx.domain);
    let mut cur_path = path.clone();
    let bytes;
    loop {
        let url = format!("{base}{cur_path}");
        let authx = crate::authx::authx(&cur_path, None);
        let mut cookie = format!("mode=relay; Trim-MC-token={}", ctx.token);
        let gw = fnapi::gateway_cookies(&url);
        if !gw.is_empty() {
            cookie.push_str("; ");
            cookie.push_str(&gw);
        }
        let mut req = client
            .http
            .get(&url)
            .header("Content-Type", "application/json")
            .header("Authorization", &ctx.token)
            .header("Cookie", cookie)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) anibox");
        if let Some(a) = authx {
            req = req.header("Authx", a);
        }
        let resp = req.send().await.map_err(|e| format!("字幕下载请求失败: {e}"))?;
        fnapi::capture_cookies(&url, resp.headers());
        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 307 | 308) {
            let loc = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            if loc.is_empty() {
                return Err(format!("字幕下载返回 {status} 且无 Location"));
            }
            let (nb, np) = split_redirect_owned(&base, &loc);
            base = nb;
            cur_path = np;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(format!("字幕下载返回 HTTP {status}"));
        }
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let body = resp.bytes().await.map_err(|e| format!("读取字幕内容失败: {e}"))?;
        if ct.contains("application/json") {
            // 服务端错误体（JSON）：提取 msg
            let msg = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("msg")
                        .or_else(|| v.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "NAS 上不存在或无权限".to_string());
            return Err(format!("字幕下载失败: {msg}"));
        }
        if body.is_empty() {
            return Err("字幕下载失败(NAS 上不存在或无权限)".to_string());
        }
        bytes = body;
        break;
    }
    std::fs::write(&file, &bytes).map_err(|e| format!("写字幕缓存失败: {e}"))?;
    Ok(file)
}

fn split_redirect_owned(base: &str, location: &str) -> (String, String) {
    if let Some(idx) = location.find("://") {
        let after = &location[idx + 3..];
        return match after.find('/') {
            Some(slash) => (
                format!("{}//{}", &location[..idx + 3], &after[..slash]),
                after[slash..].to_string(),
            ),
            None => (location.to_string(), "/".to_string()),
        };
    }
    (base.to_string(), location.to_string())
}

// ───────────────────────── 自动挂载（path 事件驱动，带代次号防陈旧） ─────────────────────────

/// 代次号：每次 path 事件递增；异步链路（取列表→下载→挂载→重试）全程绑定本次代次，
/// 过期立即静默退出 —— 快速切集时上一集的慢链路不能把字幕挂到当前集上
/// （对齐主仓库 subLoadSeq 的防陈旧竞态设计）。
static MOUNT_GEN: AtomicU64 = AtomicU64::new(0);

/// IPC 写端闭包：mpv.rs 传入（往命名管道写一行 JSON，fire-and-forget）。
pub type IpcWriter = Arc<dyn Fn(&str) + Send + Sync>;

/// 当前集标题读取闭包：mpv.rs 传入（读 force-media-title 状态槽）。
/// 切集瞬间标题可能还没写回 —— 每次尝试时**重读**，重试窗口内会补上。
pub type TitleGetter = Arc<dyn Fn() -> String + Send + Sync>;

/// path 事件入口：解析出 NAS 上下文后启动后台挂载任务（非阻塞，失败只留痕）。
///
/// 首次尝试留 ~1.5s 宽限：逐集标题（#EXTINF）由 mpv.rs 的标题状态机在
/// start-file 后经两跳 IPC 取回，path 事件可能先到；标题空着会退化成
/// 「电影模式」（挂最佳一条），剧集防错配判据就失效了 —— 等一下更稳。
pub fn spawn_mount(url: &str, title_get: TitleGetter, writer: IpcWriter, session_token: String) {
    // 代次号**无条件**先递增：切到非代理 URL（在线源/直播/本地文件）也要让上一集
    // 还在重试的挂载任务立即过期 —— 否则「NAS → 在线源」快速切换时，旧任务可能
    // 把 NAS 字幕 sub-add 到在线播放上。
    let mount_gen = MOUNT_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(ctx) = parse_proxy_url(url) else {
        return;
    };
    // [lc-1763] 注入当前会话票：URL 里的 token 是拼 URL 那刻的，长会话后可能滞后
    let mut ctx = ctx;
    ctx.fallback_token = session_token;
    let ctx = Arc::new(ctx);
    tauri::async_runtime::spawn(async move {
        // 空结果退避重试：fnOS 刚开播时流元数据可能未就绪（主仓库实测，最多 3 次）
        let delays = [1500u64, 3000, 8000];
        for (attempt, delay) in delays.iter().enumerate() {
            tokio::time::sleep(Duration::from_millis(*delay)).await;
            if MOUNT_GEN.load(Ordering::SeqCst) != mount_gen {
                return; // 已切下一集，本代次过期
            }
            let video_title = title_get();
            match fetch_streams(&ctx).await {
                Ok(streams) => {
                    if MOUNT_GEN.load(Ordering::SeqCst) != mount_gen {
                        return;
                    }
                    let best = pick_best(&streams, Some(video_title.as_str()));
                    let Some(best) = best else {
                        if attempt == 0 {
                            crate::libmpv::trace(&format!(
                                "[nas-subtitle] {} 无可自动挂载的外挂字幕（流共 {} 条，标题={}）",
                                ctx.item_guid,
                                streams.len(),
                                video_title
                            ));
                        }
                        return; // 有明确结论（无外挂/被判错配）→ 不重试
                    };
                    match download_subtitle(&ctx, best).await {
                        Ok(path) => {
                            if MOUNT_GEN.load(Ordering::SeqCst) != mount_gen {
                                return;
                            }
                            let title = if best.title.is_empty() {
                                format!("字幕-{}", best.guid.chars().take(8).collect::<String>())
                            } else {
                                best.title.clone()
                            };
                            // sub-add <path> select <title>：挂主字幕轨并立即生效。
                            // 弹幕在副字幕轨（secondary-sid），不受影响。
                            let line = serde_json::json!({
                                "command": ["sub-add", path.to_string_lossy(), "select", title],
                                "request_id": 900_000_000u64 + mount_gen,
                            })
                            .to_string();
                            writer(&format!("{line}\n"));
                            crate::libmpv::trace(&format!(
                                "[nas-subtitle] 已挂载外挂字幕: {}（{}，第 {} 次尝试）",
                                title,
                                path.display(),
                                attempt + 1
                            ));
                            return;
                        }
                        Err(e) => {
                            crate::libmpv::trace(&format!(
                                "[nas-subtitle] 字幕下载失败(第 {}/3 次): {e}",
                                attempt + 1
                            ));
                        }
                    }
                }
                Err(e) => {
                    crate::libmpv::trace(&format!(
                        "[nas-subtitle] stream/list 失败(第 {}/3 次): {e}",
                        attempt + 1
                    ));
                }
            }
        }
        crate::libmpv::trace(&format!(
            "[nas-subtitle] {} 重试 3 次后仍未挂载（媒体可能确实没有外挂字幕）",
            ctx.item_guid
        ));
    });
}

// ───────────────────────── 单测（纯逻辑，不触网） ─────────────────────────

#[cfg(test)]
mod tests {
    /// [lc-1763 真机探针] 手动跑（--ignored）：复现「stream/list 返回非 0」——用本机
    /// config.json 的 NAS 凭据 + 日志里失败的 guid 走与 fetch_streams 完全相同的链路，
    /// dump 响应原文定位与 Node 探针的差异（Node 同 guid 能拿到字幕）。
    #[test]
    #[ignore = "真机网络探针：需本机 config.json 与 NAS 可达"]
    fn probe_stream_list_on_real_nas() {
        let cfg_path = std::env::var("APPDATA")
            .map(|d| std::path::PathBuf::from(d).join("com.anibox.app/config.json"))
            .unwrap();
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).expect("读 config.json"))
                .expect("config.json 解析");
        let ctx = NasCtx {
            item_guid: "2cc6b30e1a994a69bd66943daea11cff".into(),
            token: cfg["token"].as_str().expect("token").to_string(),
            domain: cfg["server"].as_str().expect("server").to_string(),
            fallback_token: String::new(),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = crate::fnapi::new_client();
        let path = format!(
            "/v/api/v1/stream/list/{}",
            urlencoding::encode(&ctx.item_guid)
        );
        let v: serde_json::Value = rt
            .block_on(client.call(&ctx.domain, &path, "GET", &ctx.token, None))
            .expect("call 本身失败（网络层）");
        panic!("响应原文: {}", serde_json::to_string_pretty(&v).unwrap());
    }
    use super::*;
    use serde_json::json;

    fn stream(guid: &str, title: &str, lang: &str, extra: i64, default: i64) -> SubStream {
        SubStream {
            guid: guid.into(),
            title: title.into(),
            format: "srt".into(),
            language: lang.into(),
            is_default: default,
            external: extra == 1,
        }
    }

    #[test]
    fn parse_proxy_url_accepts_nas_and_rejects_others() {
        let c = parse_proxy_url(
            "http://127.0.0.1:22346/api/v1/playvideo/abc123?token=tok&skipVerify=1&account=u&domain=http://192.168.1.5:5666&useNasLocal=0&sourceIndex=0",
        )
        .unwrap();
        assert_eq!(c.item_guid, "abc123");
        assert_eq!(c.token, "tok");
        assert_eq!(c.domain, "http://192.168.1.5:5666");

        // 域名带百分号编码也能还原
        let c2 = parse_proxy_url("http://127.0.0.1:22346/api/v1/playvideo/g?token=t&domain=http%3A%2F%2Fnas%3A80")
            .unwrap();
        assert_eq!(c2.domain, "http://nas:80");

        // 非 22346/22347、非 playvideo、缺 token/domain → None
        assert!(parse_proxy_url("https://cdn.example.com/v/a.m3u8?token=1&domain=x").is_none());
        assert!(parse_proxy_url("http://127.0.0.1:22348/api/v1/playvideo/g?token=t&domain=d").is_none());
        assert!(parse_proxy_url("http://127.0.0.1:22346/api/v1/other/g?token=t&domain=d").is_none());
        assert!(parse_proxy_url("http://127.0.0.1:22346/api/v1/playvideo/g?domain=d").is_none());
        assert!(parse_proxy_url("http://127.0.0.1:22346/api/v1/playvideo/g?token=t").is_none());
    }

    #[test]
    fn parse_streams_reads_snake_and_camel() {
        let v = json!({
            "data": { "subtitle_streams": [
                { "guid": "a", "extra_file": 1, "title": "简中", "language": "zh", "is_default": 1 },
                { "guid": "b", "isExternal": 1, "title": "", "format": "ass" },
                { "guid": "", "extra_file": 1 },
                { "guid": "c", "extra_file": 0 },
            ]}
        });
        let s = parse_streams(&v);
        assert_eq!(s.len(), 3);
        assert!(s[0].external && s[0].is_default == 1);
        assert!(s[1].external && s[1].format == "ass");
        assert!(!s[2].external);
    }

    #[test]
    fn pick_best_requires_external() {
        let streams = vec![stream("emb", "中文", "zh", 0, 1)];
        assert!(pick_best(&streams, Some("剧名")).is_none());
    }

    #[test]
    fn pick_best_prefers_chinese() {
        let streams = vec![
            stream("en", "English", "en", 1, 1),
            stream("zh", "chs", "zh", 1, 0),
        ];
        let best = pick_best(&streams, Some("剧名")).unwrap();
        assert_eq!(best.guid, "zh");
    }

    #[test]
    fn pick_best_skips_when_only_embedded_chinese() {
        // 无中文外挂 + 已有内封中文 → 不挂（select 会顶掉内封中文）
        let streams = vec![
            stream("emb", "中文", "chi", 0, 1),
            stream("en", "English", "eng", 1, 0),
        ];
        assert!(pick_best(&streams, Some("剧名")).is_none());
    }

    #[test]
    fn pick_best_falls_back_to_all_external_without_chinese_anywhere() {
        let streams = vec![
            stream("en1", "English", "eng", 1, 0),
            stream("jp", "日本語", "jpn", 1, 1),
        ];
        // 无任何中文**语言标签** → 回退全部外挂。注意「日本語」标题含 CJK 字符，
        // 按主仓库 isChineseSubtitle 的「标题含中文字符」启发式算「中文优先」
        //（CJK 一体的已知边界，与主仓库行为一致），故选 jp 且 is_default 优先。
        let best = pick_best(&streams, Some("Movie")).unwrap();
        assert_eq!(best.guid, "jp");
    }

    #[test]
    fn pick_best_episode_mismatch_guard() {
        // 剧集（标题含 SxxExx）且字幕标题是「实义片名」但都不匹配 → 宁缺毋滥
        let streams = vec![stream("zorro", "佐罗", "zh", 1, 0)];
        assert!(pick_best(&streams, Some("药屋少女 - S1E3: 试试")).is_none());

        // 但同集信号（共享 SxxExx）即使剧名写法不同也放行
        let streams2 = vec![stream("ok", "无职转生 S01E06", "zh", 1, 0)];
        let best = pick_best(&streams2, Some("无职转生Ⅲ - S1E6: 集名")).unwrap();
        assert_eq!(best.guid, "ok");
    }

    #[test]
    fn pick_best_movie_fallback_without_match() {
        // 电影（无 SxxExx）：标题不匹配也退化为挂最佳一条
        let streams = vec![stream("m1", "某个片名字幕", "zh", 1, 0)];
        assert!(pick_best(&streams, Some("另一部电影")).is_some());
    }

    #[test]
    fn pick_best_keeps_language_label_only() {
        // 语言标签类标题（无片名信息）不参与标题否决
        let streams = vec![stream("lbl", "chs", "und", 1, 0)];
        let best = pick_best(&streams, Some("随便什么剧 - S1E1: x")).unwrap();
        assert_eq!(best.guid, "lbl");
    }

    #[test]
    fn pick_best_sorts_default_then_score() {
        let streams = vec![
            stream("plain", "药屋少女 S01E01", "zh", 1, 0),
            stream("defaulted", "chs", "zh", 1, 1),
        ];
        // is_default 优先于标题匹配
        let best = pick_best(&streams, Some("药屋少女 - S1E1: x")).unwrap();
        assert_eq!(best.guid, "defaulted");
    }

    #[test]
    fn language_label_detection() {
        assert!(is_language_label_only(""));
        assert!(is_language_label_only("chs"));
        assert!(is_language_label_only("简体中文"));
        assert!(is_language_label_only("chs & eng"));
        assert!(is_language_label_only("1"));
        assert!(!is_language_label_only("药屋少女"));
        assert!(!is_language_label_only("Kusuriya S01E03"));
    }

    #[test]
    fn score_title_levels() {
        assert_eq!(score_title("药屋少女的呢喃", "药屋少女的呢喃"), 2);
        // 集数标记归一化时被剥掉（主仓库 norm 同款）→ 剥完完全一致也算 2
        assert_eq!(score_title("药屋少女的呢喃 S01E03", "药屋少女的呢喃"), 2);
        // 剧名写法差异（Ⅲ vs 空）：剥掉集标识后剧名仍是互含关系 → 1
        assert_eq!(score_title("药屋少女 S01E03", "药屋少女Ⅲ - S1E3: 某话"), 1);
        // 完全无关的片名 → 0
        assert_eq!(score_title("佐罗", "药屋少女 - S1E3: x"), 0);
        assert_eq!(score_title("任意", ""), 0);
    }

    #[test]
    fn chinese_detection_variants() {
        assert!(is_chinese(&stream("a", "", "zh-Hans", 1, 0)));
        assert!(is_chinese(&stream("a", "", "CHI", 1, 0)));
        assert!(is_chinese(&stream("a", "简体", "und", 1, 0)));
        assert!(is_chinese(&stream("a", "某标题.中字.ass", "", 1, 0)));
        assert!(!is_chinese(&stream("a", "English", "en", 1, 0)));
    }
}
