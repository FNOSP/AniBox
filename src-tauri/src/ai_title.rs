//! [lc-1778] AI 标题兜底：TMDB/Bangumi 都搜不到时，用 LLM 从「脏标题」
//! （文件名 / 文件夹名 / 字幕组前缀 / 分辨率集数垃圾）里提取正式作品名再重搜。
//!
//! 设计口径（借鉴 LumiPlayer 的刮削 AI 兜底思路，落到 AniBox 的手动元数据搜索链路）：
//!   · 只在**两路镜像都成功且都 0 结果**时触发——正常标题一次命中，永远不会多花一次 LLM；
//!   · 纯可选：未配置端点/密钥时本模块整体短路（`Ok(None)`），行为与之前完全一致；
//!   · LLM 输出按「不可信外部输入」处理：只提取 JSON、白名单字段、长度/字符校验，
//!     永不因为兜底失败让整体搜索报错；
//!   · 密钥只存本地（`%APPDATA%\fntv\config.json`，与 tmdbApiKey 同源）或环境变量，
//!     绝不入库、绝不写日志（连前缀都不打）。
//!
//! 配置（三样都非空才启用）：
//!   · `config.json` 的 `aiTitleBaseUrl` / `aiTitleApiKey` / `aiTitleModel`
//!   · 或环境变量 `FNTV_AI_BASE_URL` / `FNTV_AI_API_KEY` / `FNTV_AI_MODEL`（dev 优先级更高）
//!
//! 兼容任何 OpenAI Chat Completions 协议端点（官方 / 中转 / Ollama 的 /v1 兼容层 /
//! LM Studio 等）。`aiTitleBaseUrl` 填到不含 `/chat/completions` 的根（如
//! `https://api.openai.com/v1`），本模块自动拼路径并容忍尾部斜杠。

use serde_json::{json, Value};
use std::time::Duration;

const AI_TIMEOUT_SECS: u64 = 20;
/// 输入侧防御：太短不像作品名、太长是路径/整段文本的，直接不花钱。
const MIN_QUERY_LEN: usize = 4;
const MAX_QUERY_LEN: usize = 300;
/// 提取出的标题长度上限（任何正常作品名都远小于这个数）。
const MAX_TITLE_LEN: usize = 120;

pub struct RefinedTitle {
    /// 主搜索词（官方中文名优先，其次原名）
    pub title: String,
    /// 备选搜索词（原名/别名）；与 title 相同时为 None
    pub alt_title: Option<String>,
    pub year: Option<u16>,
    /// "tv" | "movie" | None
    pub kind: Option<String>,
}

struct AiConfig {
    base_url: String,
    api_key: String,
    model: String,
}

impl AiConfig {
    fn load() -> Option<AiConfig> {
        let env_or_cfg = |env: &str, cfg_key: &str| -> Option<String> {
            if let Ok(v) = std::env::var(env) {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    return Some(v);
                }
            }
            let p = std::path::Path::new(&std::env::var("APPDATA").ok()?)
                .join("fntv/config.json");
            let s = std::fs::read_to_string(p).ok()?;
            let v: Value = serde_json::from_str(&s).ok()?;
            v.get(cfg_key)
                .and_then(|k| k.as_str())
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
        };
        Some(AiConfig {
            base_url: env_or_cfg("FNTV_AI_BASE_URL", "aiTitleBaseUrl")?,
            api_key: env_or_cfg("FNTV_AI_API_KEY", "aiTitleApiKey")?,
            model: env_or_cfg("FNTV_AI_MODEL", "aiTitleModel")?,
        })
    }
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .user_agent("AniBox/0.1 (ai-title-fallback)")
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(AI_TIMEOUT_SECS))
                .build()
                .expect("ai title http client")
        })
}

/// 入口：脏标题 → 正式标题。未配置 / 输入不合要求 / LLM 失败 / 提取无效 → `Ok(None)`。
pub async fn refine_title(raw_query: &str) -> Result<Option<RefinedTitle>, String> {
    let q = raw_query.trim();
    if q.len() < MIN_QUERY_LEN || q.len() > MAX_QUERY_LEN {
        return Ok(None);
    }
    let Some(cfg) = AiConfig::load() else {
        return Ok(None);
    };

    let system = "你是影视/动画作品名清洗器。用户给一个搜索失败的原始字符串（可能是文件名、文件夹名，"
        .to_string()
        + "带字幕组前缀、分辨率、编码、集数、年份等垃圾）。提取真实作品名，只输出 JSON，"
        + "不要任何解释或代码块围栏。格式："
        + r#"{"title":"最佳搜索词（官方中文名优先，没有则原名）","altTitle":"原名或常见别名，无则 null","year":数字或 null,"kind":"tv"或"movie"或 null}"#
        + "。判断不了作品名就输出 {\"title\":null}。";
    let body = json!({
        "model": cfg.model,
        "temperature": 0,
        "max_tokens": 200,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": q },
        ],
    });

    let url = format!(
        "{}/chat/completions",
        cfg.base_url.trim_end_matches('/')
    );
    let resp = http_client()
        .post(&url)
        .bearer_auth(&cfg.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("ai_title 请求失败: {e}"))?;
    let status = resp.status();
    let payload: Value = resp
        .json()
        .await
        .map_err(|e| format!("ai_title 响应解析失败: {e}"))?;
    if !status.is_success() {
        // 只回传状态码，不回传响应体（中转服务可能把密钥回显在错误里）
        return Err(format!("ai_title 上游 {}", status.as_u16()));
    }

    let content = payload
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let refined = parse_llm_title(&content, q);
    Ok(refined)
}

/// 解析 LLM 输出并校验（纯函数，便于单测）。返回 None = 无效/无增益，调用方静默放弃。
fn parse_llm_title(content: &str, orig: &str) -> Option<RefinedTitle> {
    let obj = extract_json_object(content)?;
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| {
            s.len() >= 2
                && s.chars().count() <= MAX_TITLE_LEN
                && !s.eq_ignore_ascii_case(orig.trim())
                && !"null".eq_ignore_ascii_case(s)
        })?;
    let clean = |v: Option<&str>| -> Option<String> {
        v.map(|s| s.trim().to_string())
            .filter(|s| {
                s.len() >= 2
                    && s.chars().count() <= MAX_TITLE_LEN
                    && !s.eq_ignore_ascii_case(&title)
                    && !"null".eq_ignore_ascii_case(s)
            })
    };
    let alt_title = clean(obj.get("altTitle").and_then(|v| v.as_str()));
    let year = obj
        .get("year")
        .and_then(|v| v.as_u64())
        .and_then(|y| u16::try_from(y).ok())
        .filter(|&y| (1900..=2100).contains(&y));
    let kind = obj
        .get("kind")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s == "tv" || s == "movie");
    Some(RefinedTitle { title, alt_title, year, kind })
}

/// 从 LLM 回复里抠出第一个平衡的 JSON 对象（容忍代码围栏 / 前后 prose）。
fn extract_json_object(content: &str) -> Option<Value> {
    let s = content.trim();
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        let c = b as char;
        if in_str {
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
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_slice(&bytes[start..=i]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_json() {
        let r = parse_llm_title(
            r#"{"title":"葬送的芙莉莲","altTitle":"Sousou no Frieren","year":2023,"kind":"tv"}"#,
            "[SubsPlease] Sousou no Frieren - 17 [1080p].mkv",
        )
        .unwrap();
        assert_eq!(r.title, "葬送的芙莉莲");
        assert_eq!(r.alt_title.as_deref(), Some("Sousou no Frieren"));
        assert_eq!(r.year, Some(2023));
        assert_eq!(r.kind.as_deref(), Some("tv"));
    }

    #[test]
    fn parse_fenced_json_with_prose() {
        let r = parse_llm_title(
            "好的，结果如下：\n```json\n{\"title\": \"孤独摇滚!\", \"altTitle\": null, \"year\": 2022, \"kind\": \"tv\"}\n```\n以上。",
            "Bocchi the Rock! S01 1080p B-Global",
        )
        .unwrap();
        assert_eq!(r.title, "孤独摇滚!");
        assert_eq!(r.alt_title, None);
    }

    #[test]
    fn parse_null_title_gives_none() {
        assert!(parse_llm_title(r#"{"title":null}"#, "random junk string").is_none());
    }

    #[test]
    fn parse_title_same_as_original_gives_none() {
        // 清洗结果和原词一样 = 无增益，放弃（避免无效重搜）
        assert!(parse_llm_title(
            r#"{"title":"Sousou no Frieren","altTitle":null,"year":null,"kind":null}"#,
            "Sousou no Frieren"
        )
        .is_none());
    }

    #[test]
    fn parse_bad_year_and_kind_dropped() {
        let r = parse_llm_title(
            r#"{"title":"进击的巨人","altTitle":null,"year":99999,"kind":"anime"}"#,
            "Shingeki no Kyojin Final Season EP74",
        )
        .unwrap();
        assert_eq!(r.year, None);
        assert_eq!(r.kind, None);
    }

    #[test]
    fn parse_garbage_gives_none() {
        assert!(parse_llm_title("抱歉我不知道这是什么", "whatever").is_none());
        assert!(parse_llm_title("", "whatever").is_none());
    }

    #[test]
    fn extract_json_handles_nested_braces_in_strings() {
        let v = extract_json_object(r#"前缀 {"a":"包含 } 花括号","b":{"c":1}} 后缀"#).unwrap();
        assert_eq!(v["b"]["c"], 1);
        assert_eq!(v["a"], "包含 } 花括号");
    }
}
