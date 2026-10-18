//! 标题归一化 / 季号解析 / 精确匹配 / 分集定位。
//!
//! 逐条对齐 `assets/danmu-api.cjs` 的 `normalizeTitle` / `splitSeason` /
//! `exactMatchTier` / `keywordCandidates` / `extractEpisodeTitle` / `pickEpisode` /
//! `pickVerifiedEpisode`。自建源只认「主名归一化后完全相等 + 季号一致」，
//! 不做模糊匹配（模糊匹配留给内置 B 站链路）。

use crate::danmaku::types::{Episode, PickedEpisode, SearchHit};
use regex::Regex;
use std::sync::OnceLock;

pub const SEASON_TIER_EXACT: u8 = 0;
pub const SEASON_TIER_IMPLIED_FIRST: u8 = 1;
pub const SEASON_TIER_UNMARKED: u8 = 2;
pub const SEASON_TIER_UNMARKED_VERIFY: u8 = 3;

fn re_bracket() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"【[^】]*】").unwrap())
}
fn re_year() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[\[(（](?:19|20)\d{2}[\])）]").unwrap())
}
fn re_from_suffix() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\s*from\s+[a-z0-9]+\s*$").unwrap())
}
fn re_punct() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#"[『』「」〔〕《》〈〉""''（）()·:：!！?？,，.。、\-_—~～△▲★☆♥・\s]"#)
            .unwrap()
    })
}
fn re_season_mark() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)第\s*(0*\d{1,2}|[一二三四五六七八九十]{1,3})\s*[季部]|season\s*0*(\d{1,2})|\bs\s*0*(\d{1,2})\b")
            .unwrap()
    })
}
fn re_keyword_split() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\s*[-–—]\s*|\s*[:：]\s*").unwrap())
}
fn re_ep_suffix() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)[.\s]*s\d{1,2}e\d{1,4}[.\s]*$").unwrap())
}
fn re_dash_split() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)^(.+?)\s*[-–—]\s*(?:s\d{1,2}e\d{1,4}\s*)?[:：]?\s*(.+)$").unwrap()
    })
}
fn re_colon_split() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^(.+?)\s*[:：]\s*(.+)$").unwrap())
}
fn re_ep_title_punct() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"[『』「」〔〕《》〈〉""''（）()]"#).unwrap())
}
fn re_ep_title_num() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"第\s*\d{1,4}\s*[话集期]").unwrap())
}
fn re_ep_title_trim() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[\s·:：!！?？,，.。、\-_—~～]+").unwrap())
}
fn re_ep_from_title() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"第\s*(\d{1,4})\s*[话集期]").unwrap())
}

/// 归一化番名：去年份/类型标签/来源后缀/包裹符号/装饰符（对齐 Node `normalizeTitle`）。
pub fn normalize_title(s: &str) -> String {
    let t = re_bracket().replace_all(s, "");
    let t = re_year().replace_all(&t, "");
    let t = re_from_suffix().replace_all(&t, "");
    let t = re_punct().replace_all(&t, "");
    t.to_lowercase()
}

fn cn_num(c: char) -> u32 {
    match c {
        '一' => 1,
        '二' => 2,
        '三' => 3,
        '四' => 4,
        '五' => 5,
        '六' => 6,
        '七' => 7,
        '八' => 8,
        '九' => 9,
        '十' => 10,
        _ => 0,
    }
}

/// 中文季号 → 数字（对齐 Node `seasonToInt`）。
pub fn season_to_int(s: &str) -> u32 {
    let t = s.trim();
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        return t.parse().unwrap_or(0);
    }
    let chars: Vec<char> = t.chars().collect();
    if chars.len() == 1 {
        return cn_num(chars[0]);
    }
    if chars.first() == Some(&'十') {
        return 10 + cn_num(chars[1]);
    }
    if chars.last() == Some(&'十') {
        return cn_num(chars[0]) * 10;
    }
    0
}

/// 拆「作品主名 + 季号」：先剥季号再归一化。
pub fn split_season(raw: &str) -> (String, u32) {
    let mut season = 0u32;
    let body = re_season_mark().replace_all(raw, |caps: &regex::Captures| {
        if season == 0 {
            let cn = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let en = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let sn = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            season = season_to_int(if !cn.is_empty() { cn } else if !en.is_empty() { en } else { sn });
        }
        " "
    });
    (normalize_title(&body), season)
}

/// 自建源准入：主名完全相等 + 季号一致。命中返回档位，否则 None。
pub fn exact_match_tier(
    query: &str,
    query_season: u32,
    cand_title: &str,
    return_unverified: bool,
) -> Option<u8> {
    let (q_name, q_season) = split_season(query);
    let (c_name, c_season) = split_season(cand_title);
    if q_name.is_empty() || q_name != c_name {
        return None;
    }
    let want = if query_season > 0 { query_season } else { q_season };
    if c_season > 0 {
        if c_season == want {
            return Some(SEASON_TIER_EXACT);
        }
        if want == 0 && c_season == 1 {
            return Some(SEASON_TIER_IMPLIED_FIRST);
        }
        return None;
    }
    if want <= 1 {
        return Some(SEASON_TIER_UNMARKED);
    }
    if return_unverified {
        Some(SEASON_TIER_UNMARKED_VERIFY)
    } else {
        None
    }
}

/// 搜索关键词候选：原始 title 优先，失败时回退到砍掉集标题后缀的首段。
pub fn keyword_candidates(title: &str) -> Vec<String> {
    let raw = title.trim();
    if raw.is_empty() {
        return vec![];
    }
    let segs: Vec<String> = re_keyword_split()
        .split(raw)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let head = segs.first().cloned().unwrap_or_default();
    if !head.is_empty() && head != raw {
        vec![raw.to_string(), head]
    } else {
        vec![raw.to_string()]
    }
}

/// 归一化集标题（保留集号语义，剥装饰）。
pub fn normalize_ep_title(s: &str) -> String {
    let t = re_bracket().replace_all(s, "");
    let t = re_ep_title_punct().replace_all(&t, "");
    let t = re_ep_title_num().replace_all(&t, " ");
    let t = re_ep_title_trim().replace_all(&t, "");
    t.to_lowercase()
}

/// 从播放侧标题里提取「本集标题」；提取不出返回空串。
pub fn extract_episode_title(title: &str) -> String {
    let raw = title.trim();
    if raw.is_empty() {
        return String::new();
    }
    let s = re_ep_suffix().replace(raw, "").trim().to_string();
    let mut head = String::new();
    let mut tail = s.clone();
    if let Some(caps) = re_dash_split().captures(&s) {
        head = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        tail = caps.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
    } else if let Some(caps) = re_colon_split().captures(&s) {
        head = caps.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        tail = caps.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
    }
    if tail.is_empty() || tail.eq_ignore_ascii_case("noTitle") {
        return String::new();
    }
    let cmp_head = if head.is_empty() { raw } else { &head };
    if normalize_ep_title(&tail) == normalize_ep_title(cmp_head) {
        return String::new();
    }
    tail
}

/// 分集标题里兜底提集数。
pub fn ep_from_title(t: &str) -> i64 {
    re_ep_from_title()
        .captures(t)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}

/// 在条目里定位目标集；ep<=0 时只认单集条目。
pub fn pick_episode(episodes: &[Episode], episode_count: i64, ep: i64) -> Option<PickedEpisode> {
    if episodes.is_empty() {
        return None;
    }
    if ep <= 0 {
        if episodes.len() == 1 || episode_count == 1 {
            let e = &episodes[0];
            if e.episode_id != 0 {
                return Some(PickedEpisode {
                    episode_id: e.episode_id,
                    episode_title: e.episode_title.clone(),
                });
            }
        }
        return None;
    }
    for e in episodes {
        if e.episode_id == 0 {
            continue;
        }
        match e.episode_number {
            Some(num) if num == ep as f64 => {
                return Some(PickedEpisode {
                    episode_id: e.episode_id,
                    episode_title: e.episode_title.clone(),
                })
            }
            None => {
                if ep_from_title(&e.episode_title) == ep {
                    return Some(PickedEpisode {
                        episode_id: e.episode_id,
                        episode_title: e.episode_title.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    None
}

/// 未标季条目：用播放侧集标题核对服务端分集标题后再放行。
pub fn pick_verified_episode(episodes: &[Episode], ep: i64, ep_title: &str) -> Option<PickedEpisode> {
    let want = normalize_ep_title(ep_title);
    if want.is_empty() {
        return None;
    }
    let mut exact: Vec<PickedEpisode> = Vec::new();
    let mut contains: Vec<PickedEpisode> = Vec::new();
    for e in episodes {
        if e.episode_id == 0 {
            continue;
        }
        let norm = normalize_ep_title(&e.episode_title);
        if norm.is_empty() {
            continue;
        }
        let item = PickedEpisode {
            episode_id: e.episode_id,
            episode_title: e.episode_title.clone(),
        };
        if norm == want {
            exact.push(item);
        } else if norm.contains(&want) && want.chars().count() >= 2 {
            contains.push(item);
        }
    }
    let decide = |list: &[PickedEpisode]| -> Option<PickedEpisode> {
        if list.is_empty() {
            return None;
        }
        if list.len() > 1 {
            let by_num: Vec<&PickedEpisode> = list
                .iter()
                .filter(|x| ep_from_title(&x.episode_title) == ep)
                .collect();
            if by_num.len() == 1 {
                return Some(by_num[0].clone());
            }
            return None;
        }
        Some(list[0].clone())
    };
    decide(&exact).or_else(|| decide(&contains))
}

/// 把候选条目按季号档位排序（同档位优先 bilibili 源）。
pub fn sort_hits(hits: &mut [SearchHit]) {
    hits.sort_by(|x, y| {
        x.season_tier
            .cmp(&y.season_tier)
            .then_with(|| {
                let bx = if x.source == "bilibili" { 1 } else { 0 };
                let by = if y.source == "bilibili" { 1 } else { 0 };
                by.cmp(&bx)
            })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_year_type_source() {
        assert_eq!(normalize_title("沧元图(2023)【国漫】from youku"), "沧元图");
        assert_eq!(normalize_title("流浪地球2from 360"), "流浪地球2");
        assert_eq!(normalize_title("  《葬送的芙莉莲》 "), "葬送的芙莉莲");
        // 片名含 From 不被误伤
        assert_eq!(normalize_title("From 破晓"), "from破晓");
    }

    #[test]
    fn season_mark_parsing() {
        assert_eq!(split_season("第一季"), ("".into(), 1));
        assert_eq!(split_season("某番 第二季"), ("某番".into(), 2));
        assert_eq!(split_season("某番 Season 3"), ("某番".into(), 3));
        assert_eq!(split_season("某番 S02"), ("某番".into(), 2));
        assert_eq!(season_to_int("十一"), 11);
        assert_eq!(season_to_int("二十"), 20);
        assert_eq!(season_to_int("3"), 3);
        // 「最终季」不当季号
        assert_eq!(split_season("最终季").1, 0);
    }

    #[test]
    fn exact_match_tiers() {
        assert_eq!(exact_match_tier("某番", 2, "某番 第二季", false), Some(0));
        assert_eq!(exact_match_tier("某番", 0, "某番 第一季", false), Some(1));
        assert_eq!(exact_match_tier("某番", 0, "某番", false), Some(2));
        assert_eq!(exact_match_tier("某番", 2, "某番", false), None);
        assert_eq!(exact_match_tier("某番", 2, "某番", true), Some(3));
        // 主名不等一律拒收
        assert_eq!(exact_match_tier("悬案", 0, "李小龙悬案", false), None);
    }

    #[test]
    fn keyword_candidates_fallback() {
        assert_eq!(keyword_candidates("悬案 - : 矢量"), vec!["悬案 - : 矢量", "悬案"]);
        assert_eq!(keyword_candidates("命运石之门"), vec!["命运石之门"]);
        assert!(keyword_candidates("   ").is_empty());
    }

    #[test]
    fn episode_title_extraction() {
        assert_eq!(extract_episode_title("某番 - S02E03: 夏天"), "夏天");
        assert_eq!(extract_episode_title("某番: 夏天"), "夏天");
        assert_eq!(extract_episode_title("某番"), "");
        assert_eq!(extract_episode_title("某番 - : noTitle"), "");
        assert_eq!(extract_episode_title("The Demon Hunter.S02E27"), "The Demon Hunter");
    }

    #[test]
    fn episode_pick_and_verify() {
        let eps = vec![
            Episode { episode_id: 11, episode_title: "第1话 开始".into(), episode_number: Some(1.0) },
            Episode { episode_id: 12, episode_title: "第2话 继续".into(), episode_number: Some(2.0) },
        ];
        assert_eq!(pick_episode(&eps, 2, 2).unwrap().episode_id, 12);
        assert!(pick_episode(&eps, 2, 9).is_none());
        assert!(pick_episode(&eps, 2, 0).is_none());

        let compilation = vec![
            Episode { episode_id: 21, episode_title: "【youku】 第69集 前传：东宁府的夏天 叁".into(), episode_number: Some(69.0) },
            Episode { episode_id: 22, episode_title: "别的".into(), episode_number: Some(70.0) },
        ];
        let picked = pick_verified_episode(&compilation, 69, "东宁府的夏天：叁").unwrap();
        assert_eq!(picked.episode_id, 21);
        assert!(pick_verified_episode(&compilation, 69, "完全无关").is_none());
    }
}
