//! [lc-2330] 弹幕共享映射客户端。
//!
//! 解决的问题：一季动画的弹幕库集号经常与本地文件集数错位（季合并、特别篇、先行
//! 放映）。目前用户每集都要手动选一遍正确的剧集，而且只在本机记住（`episode_offset`
//! 是纯本地 JSON）。这里接上 `inbox.690075.xyz` 的共享映射：**一个人校准一次，同组
//! 设备以后自动命中**。
//!
//! 协议（见 `inbox-mirror/worker.js`）：
//!   POST /v1/map/resolve  { title, season, episode }         → 命中返回 selection，否则 404
//!   POST /v1/map/save     { title, season, episode, selection } → 需令牌
//!
//! 设计取舍：
//!   · **读不阻塞播放**。解析失败/超时一律当作「没有共享映射」，走原有的本地识别链
//!     （哈希匹配 → 自动搜索 → 历史）。共享映射只是**加速层**，绝不能让它成为新的
//!     起播延迟来源（这是 lc-2296「连播等 8s」的血训）。
//!   · **只在用户明确手动选择时写入**。自动命中/哈希匹配/历史复用都不能冒充手动选择，
//!     否则会把错误对应关系污染到所有人（协议文档里特别强调过）。
//!   · **身份用设备码**（用户定的「一机一账号」）：sha256(机器标识)，不传原始标识，
//!     也不做账号体系。写入失败不打扰用户（只是这次校准没共享出去）。

use serde_json::{json, Value};
use std::time::Duration;

/// 共享映射服务根地址。
///
/// 与 `bgm.690075.xyz` / `tmdb.690075.xyz` 同一套自托管模式。留空则该功能**整体关闭**
/// （不发起任何请求），便于自建用户完全离线使用。
pub const DEFAULT_BASE: &str = "https://inbox.690075.xyz";

/// 解析超时。短是刻意的：这是加速层，慢就不该等。
const RESOLVE_TIMEOUT_MS: u64 = 2500;
/// 写入超时。用户已经选完了，多等一会儿无所谓，但也不能挂住 UI。
const SAVE_TIMEOUT_MS: u64 = 6000;

/// 一次解析的结果。
#[derive(Debug, Clone)]
pub struct SharedMapping {
    pub episode_id: String,
    pub bangumi_id: String,
    pub episode_title: Option<String>,
}

/// 归一化标题：与 Worker 端 `normTitle` **必须逐条一致**，否则两边键对不上。
///
/// 顺序也是契约的一部分（顺序不同会得出不同的键）：
///   ① 全角 ASCII（U+FF01..U+FF5E）→ 半角；全角空格 → 普通空格
///   ② 分隔标点 → 空格（含半角 `;` 与全角 `；`，前者是后者 NFKC 后的形态）
///   ③ 连续空白折叠成一个、去首尾
///   ④ 小写
///
/// 不做「去括号」「去季后缀」这类语义归一化 —— 那会把不同作品混成一条。
pub fn norm_title(raw: &str) -> String {
    // ① 全角 → 半角
    let mut s = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let c = ch as u32;
        if (0xFF01..=0xFF5E).contains(&c) {
            s.push(char::from_u32(c - 0xFEE0).unwrap_or(ch));
        } else if c == 0x3000 {
            s.push(' ');
        } else {
            s.push(ch);
        }
    }
    // ② 标点 → 空格
    let mapped: String = s
        .chars()
        .map(|ch| match ch {
            '·' | '・' | ':' | '：' | '-' | '—' | '–' | '_' | '/' | '\\' | '|' | ';' | '；' => ' ',
            other => other,
        })
        .collect();
    // ③ 空白折叠 + 去首尾；④ 小写
    mapped
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// 读取共享映射。**任何失败都返回 None**（超时、网络错、404、格式不对）。
///
/// 这是刻意的：调用方把它当「可选的加速结果」，不是必须品。
pub async fn resolve(
    http: &reqwest::Client,
    base: &str,
    title: &str,
    season: u32,
    episode: i64,
) -> Option<SharedMapping> {
    if base.trim().is_empty() || title.trim().is_empty() || season == 0 || episode <= 0 {
        return None;
    }
    let url = format!("{}/v1/map/resolve", base.trim().trim_end_matches('/'));
    let resp = http
        .post(&url)
        .json(&json!({
            "title": title.trim(),
            "season": season,
            "episode": episode,
        }))
        .timeout(Duration::from_millis(RESOLVE_TIMEOUT_MS))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None; // 404 = 没有映射，是正常路径
    }
    let body: Value = resp.json().await.ok()?;
    let sel = body.pointer("/data/selection")?;
    let episode_id = sel.get("episodeId")?.as_str()?.to_string();
    let bangumi_id = sel
        .get("bangumiId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // episodeId 必须是十进制（弹弹play 的约定）；不是就当没命中，别把脏值塞进请求
    if episode_id.is_empty() || !episode_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(SharedMapping {
        episode_id,
        bangumi_id,
        episode_title: sel
            .get("episodeTitle")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

/// 写入共享映射。只在**用户手动选择**后调用。
///
/// 返回值说明：`Ok(true)` = 已共享；`Ok(false)` = 服务端未配置（功能关闭）；
/// `Err` = 试过了但失败（调用方只记日志，不打扰用户）。
pub async fn save(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    device: &str,
    title: &str,
    season: u32,
    episode: i64,
    bangumi_id: &str,
    episode_id: &str,
    episode_title: Option<&str>,
) -> Result<bool, String> {
    if base.trim().is_empty() {
        return Ok(false);
    }
    if token.trim().is_empty() {
        // 没有写入令牌 → 只能读不能写（协议允许无令牌客户端读取共享结果）
        return Ok(false);
    }
    if title.trim().is_empty() || season == 0 || episode <= 0 {
        return Err("标题/季/集不合法".into());
    }
    let url = format!("{}/v1/map/save", base.trim().trim_end_matches('/'));
    let mut sel = json!({ "bangumiId": bangumi_id, "episodeId": episode_id });
    if let Some(t) = episode_title.filter(|t| !t.trim().is_empty()) {
        sel["episodeTitle"] = json!(t.trim());
    }
    let resp = http
        .post(&url)
        .header("X-Anibox-Token", token.trim())
        .header("X-Anibox-Device", device)
        .json(&json!({
            "title": title.trim(),
            "season": season,
            "episode": episode,
            "selection": sel,
        }))
        .timeout(Duration::from_millis(SAVE_TIMEOUT_MS))
        .send()
        .await
        .map_err(|e| format!("连接共享映射服务失败: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(true);
    }
    let body = resp.text().await.unwrap_or_default();
    Err(format!("共享映射写入失败 HTTP {status}: {}", truncate(&body, 200)))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_title_matches_worker_rules() {
        // 与 worker.js 的 normTitle 对齐的四个关键行为
        assert_eq!(norm_title("葬送的芙莉莲"), "葬送的芙莉莲");
        assert_eq!(norm_title("  葬送的芙莉莲  "), "葬送的芙莉莲");
        assert_eq!(norm_title("葬送的芙莉莲：黄金乡"), "葬送的芙莉莲 黄金乡");
        // 全角字母数字 → 半角
        assert_eq!(norm_title("ＳＴＥＩＮＳ；ＧＡＴＥ"), "steins gate");
        // 全角空格
        assert_eq!(norm_title("葬送\u{3000}的芙莉莲"), "葬送 的芙莉莲");
        // 连续空白折叠
        assert_eq!(norm_title("a   b"), "a b");
        // 小写
        assert_eq!(norm_title("Frieren"), "frieren");
    }

    #[test]
    fn norm_title_does_not_merge_different_works() {
        // 不能做语义归一化：这两部是不同的作品
        assert_ne!(norm_title("葬送的芙莉莲"), norm_title("葬送的芙莉莲 第二季"));
        assert_ne!(norm_title("进击的巨人"), norm_title("进击的巨人 最终季"));
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        // 中文按字截断，不能切出半个 UTF-8
        let s = "一二三四五";
        assert_eq!(truncate(s, 3), "一二三…");
        assert_eq!(truncate(s, 10), "一二三四五");
    }
}
