//! [lc-1701] 演员页「入库状态」：把演员作品批量与飞牛媒体库比对。
//!
//! 匹配优先级（强标识优先，避免同名误判）：
//!   ① TMDB id（`tmdb_id` / `trim_id=tmNNN` / 纯数字 trim_id）+ 媒体类型
//!   ② Bangumi id（`trim_id=bgNNN`）
//!   ③ 归一化标题 + 年份（标题归一化口径对齐 `danmu-api.cjs`：
//!      全角→半角、繁简/异体字符映射、去标点/空白/装饰）
//!
//! 只在「标题 + 年份」都一致（或其中一侧缺年份）时才判定命中；同名不同年（翻拍）
//! 一律不匹配，宁可漏报也不误报。匹配逻辑是纯函数，便于 `cargo test --lib` 静态验证。

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// 媒体类型（只区分电影 / 剧集；未知 = None，表示两种都接受）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Movie,
    Tv,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Movie => "m",
            Kind::Tv => "t",
        }
    }
}

/// 前端传入的单个作品（`tmdbId` / `bgmId` / `title` / `originalTitle` / `year` / `mediaType`）
#[derive(Debug, Clone)]
pub struct WorkKey {
    pub tmdb_id: Option<i64>,
    pub bgm_id: Option<i64>,
    pub title: String,
    pub original_title: String,
    pub year: Option<u32>,
    pub kind: Option<Kind>,
}

impl WorkKey {
    pub fn from_json(v: &Value) -> Self {
        let id = |key: &str| -> Option<i64> {
            match v.get(key) {
                Some(Value::Number(n)) => n.as_i64(),
                Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
                _ => None,
            }
        };
        let text = |key: &str| -> String {
            v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
        };
        let year = match v.get("year") {
            Some(Value::Number(n)) => n.as_u64().map(|n| n as u32),
            Some(Value::String(s)) => year_of(s),
            _ => None,
        }
        .filter(|y| (1888..=2100).contains(y));
        let media = text("mediaType");
        let kind = match media.as_str() {
            "movie" => Some(Kind::Movie),
            "tv" | "anime" | "real" => Some(Kind::Tv),
            _ => None,
        };
        Self {
            tmdb_id: id("tmdbId"),
            bgm_id: id("bgmId"),
            title: text("title"),
            original_title: text("originalTitle"),
            year,
            kind,
        }
    }
}

struct TitleEntry {
    year: Option<u32>,
    kind: Option<Kind>,
    guid: String,
}

/// 媒体库索引（一次构建，批量匹配）
pub struct LibIndex {
    /// "m:123" / "t:123" → guid
    tmdb: HashMap<String, String>,
    /// "123" → guid
    bgm: HashMap<String, String>,
    /// 归一化标题 → 条目下标（同一标题可能有多季/多版本）
    by_title: HashMap<String, Vec<usize>>,
    entries: Vec<TitleEntry>,
}

/// 从 item/list 的原始条目构建索引。只读取需要的字段，兼容 snake_case / camelCase。
pub fn build_index(items: &[Value]) -> LibIndex {
    let mut idx = LibIndex {
        tmdb: HashMap::new(),
        bgm: HashMap::new(),
        by_title: HashMap::new(),
        entries: Vec::new(),
    };
    for it in items {
        let guid = str_field(it, &["guid", "Guid"]).unwrap_or_default();
        if guid.is_empty() {
            continue;
        }
        let title = str_field(
            it,
            &["title", "name", "original_title", "original_name", "tv_title"],
        )
        .unwrap_or_default();
        let year = str_field(it, &["release_date", "air_date", "date"])
            .and_then(|s| year_of(&s))
            .or_else(|| {
                int_field(it, &["year"]).and_then(|y| {
                    let y = y as u32;
                    if (1888..=2100).contains(&y) { Some(y) } else { None }
                })
            });
        let kind = kind_of(str_field(it, &["type", "media_type"]).as_deref());

        // ① TMDB 强标识：显式 tmdb_id / tmdbId；trim_id=tmNNN；纯数字 trim_id 也按 TMDB id 处理
        let trim = str_field(it, &["trim_id", "trimId"]).unwrap_or_default();
        let mut tmdb_id = int_field(it, &["tmdb_id", "tmdbId"]);
        if tmdb_id.is_none() {
            tmdb_id = parse_tmdb_trim(&trim);
        }
        if let Some(t) = tmdb_id {
            match kind {
                Some(k) => {
                    idx.tmdb.insert(format!("{}:{}", k.key(), t), guid.clone());
                }
                None => {
                    // 类型未知：两种都登记，交给作品侧的 mediaType 收窄
                    idx.tmdb.insert(format!("m:{}", t), guid.clone());
                    idx.tmdb.insert(format!("t:{}", t), guid.clone());
                }
            }
        }

        // ② Bangumi 强标识：trim_id=bgNNN
        if let Some(b) = parse_bgm_trim(&trim) {
            idx.bgm.insert(b.to_string(), guid.clone());
        }

        // ③ 标题 + 年份
        let n = norm_title(&title);
        if n.chars().count() >= 2 {
            let i = idx.entries.len();
            idx.entries.push(TitleEntry {
                year,
                kind,
                guid: guid.clone(),
            });
            idx.by_title.entry(n).or_default().push(i);
        }
    }
    idx
}

/// 匹配单个作品，命中返回媒体库条目 guid。
pub fn match_work(idx: &LibIndex, w: &WorkKey) -> Option<String> {
    // ① TMDB id
    if let Some(t) = w.tmdb_id {
        match w.kind {
            Some(k) => {
                if let Some(g) = idx.tmdb.get(&format!("{}:{}", k.key(), t)) {
                    return Some(g.clone());
                }
            }
            None => {
                if let Some(g) = idx
                    .tmdb
                    .get(&format!("m:{}", t))
                    .or_else(|| idx.tmdb.get(&format!("t:{}", t)))
                {
                    return Some(g.clone());
                }
            }
        }
    }
    // ② Bangumi id
    if let Some(b) = w.bgm_id {
        if let Some(g) = idx.bgm.get(&b.to_string()) {
            return Some(g.clone());
        }
    }
    // ③ 标题（主标题 / 原名）
    for raw in [w.title.as_str(), w.original_title.as_str()] {
        if raw.trim().is_empty() {
            continue;
        }
        if let Some(g) = match_title(idx, raw, w.year, w.kind) {
            return Some(g);
        }
    }
    None
}

fn match_title(idx: &LibIndex, raw: &str, year: Option<u32>, kind: Option<Kind>) -> Option<String> {
    let n = norm_title(raw);
    let n_len = n.chars().count();
    if n_len < 2 {
        return None;
    }
    // 全等：允许「年份相等」或「任一侧缺年份」
    if let Some(list) = idx.by_title.get(&n) {
        if let Some(g) = pick(idx, list, year, kind, false) {
            return Some(g);
        }
    }
    // 前缀兜底（季/篇后缀差异，如「我推的孩子」↔「我推的孩子 第三季」）：
    // 只在标题足够长、且**年份严格相等**时采用，避免把同名前缀的不同作品算进来。
    if n_len >= 3 && year.is_some() {
        for (t, list) in &idx.by_title {
            let t_len = t.chars().count();
            if t_len < 3 || t == &n {
                continue;
            }
            if t.starts_with(&n) || n.starts_with(t) {
                if let Some(g) = pick(idx, list, year, kind, true) {
                    return Some(g);
                }
            }
        }
    }
    None
}

fn pick(
    idx: &LibIndex,
    list: &[usize],
    year: Option<u32>,
    kind: Option<Kind>,
    strict_year: bool,
) -> Option<String> {
    for &i in list {
        let e = &idx.entries[i];
        if !kind_compatible(kind, e.kind) {
            continue;
        }
        match (year, e.year) {
            (Some(y), Some(ey)) if y == ey => return Some(e.guid.clone()),
            (Some(_), Some(_)) => continue, // 同年份都给了但对不上 → 翻拍/不同作
            (Some(_), None) if !strict_year => return Some(e.guid.clone()),
            (None, _) if !strict_year => return Some(e.guid.clone()),
            _ => {}
        }
    }
    None
}

fn kind_compatible(a: Option<Kind>, b: Option<Kind>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

fn kind_of(raw: Option<&str>) -> Option<Kind> {
    let t = raw?.to_lowercase();
    if t.contains("movie") || t.contains("电影") || t.contains("film") {
        Some(Kind::Movie)
    } else if t.contains("tv")
        || t.contains("series")
        || t.contains("show")
        || t.contains("season")
        || t.contains("剧")
        || t.contains("集")
    {
        Some(Kind::Tv)
    } else {
        None
    }
}

fn str_field(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            if !s.trim().is_empty() {
                return Some(s.trim().to_string());
            }
        }
    }
    None
}

fn int_field(v: &Value, keys: &[&str]) -> Option<i64> {
    for k in keys {
        match v.get(*k) {
            Some(Value::Number(n)) => {
                if let Some(i) = n.as_i64() {
                    return Some(i);
                }
            }
            Some(Value::String(s)) => {
                if let Ok(i) = s.trim().parse::<i64>() {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_tmdb_trim(trim: &str) -> Option<i64> {
    let t = trim.trim();
    if let Some(rest) = t.strip_prefix("tm") {
        return rest.parse::<i64>().ok();
    }
    if t.bytes().all(|b| b.is_ascii_digit()) && !t.is_empty() {
        return t.parse::<i64>().ok();
    }
    None
}

fn parse_bgm_trim(trim: &str) -> Option<i64> {
    trim.trim().strip_prefix("bg")?.parse::<i64>().ok()
}

/// 年份：取字符串里第一个 4 位数字（1888..=2100），兼容 "2021" / "2021-05-01"。
pub fn year_of(raw: &str) -> Option<u32> {
    let chars: Vec<char> = raw.chars().collect();
    for i in 0..chars.len().saturating_sub(3) {
        if chars[i..i + 4].iter().all(|c| c.is_ascii_digit()) {
            let s: String = chars[i..i + 4].iter().collect();
            if let Ok(y) = s.parse::<u32>() {
                if (1888..=2100).contains(&y) {
                    return Some(y);
                }
            }
        }
    }
    None
}

fn year_bracket_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\[(（]\s*(?:19|20)\d{2}\s*[\])）]").expect("year bracket regex"))
}

fn from_suffix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\s*from\s+[a-z0-9]+\s*$").expect("from suffix regex"))
}

/// 繁简/异体字归一（与 `danmu-api.cjs` 的 TITLE_VARIANT_MAP 同口径，只做确定等价替换）。
fn variant_char(c: char) -> char {
    match c {
        '份' => '分',
        '裏' | '裡' => '里',
        '臺' | '檯' => '台',
        '為' => '为',
        '與' => '与',
        '這' => '这',
        '說' => '说',
        '們' => '们',
        '個' => '个',
        '隻' => '只',
        '著' => '着',
        '麼' => '么',
        '處' => '处',
        '衛' => '卫',
        '髮' => '发',
        '鬥' => '斗',
        '啟' => '启',
        '歷' => '历',
        _ => c,
    }
}

/// 去掉【…】包裹装饰（与 danmu-api 一致；库标题偶尔带【完结】等标签）。
fn strip_decor(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for ch in s.chars() {
        match ch {
            '【' => depth += 1,
            '】' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            _ => out.push(ch),
        }
    }
    out
}

/// 归一化标题：去装饰/年份括号/来源后缀 → 全角转半角 → 繁简异体映射 → 小写 → 只留字母数字与汉字。
pub fn norm_title(raw: &str) -> String {
    let s = strip_decor(raw);
    let s = year_bracket_re().replace_all(&s, "");
    let s = from_suffix_re().replace(&s, "");
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let c = match ch {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(ch as u32 - 0xFEE0).unwrap_or(ch),
            _ => ch,
        };
        let c = variant_char(c);
        for lc in c.to_lowercase() {
            if lc.is_alphanumeric() {
                out.push(lc);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn work(v: Value) -> WorkKey {
        WorkKey::from_json(&v)
    }

    #[test]
    fn norm_title_handles_fullwidth_punct_and_variants() {
        assert_eq!(norm_title("进击的巨人：最终季"), "进击的巨人最终季");
        assert_eq!(norm_title("Ｆａｔｅ／Ｚｅｒｏ"), "fatezero");
        assert_eq!(norm_title("裏臺"), "里台");
        assert_eq!(norm_title("【完结】某番 (2019)"), "某番");
        assert_eq!(norm_title("某番 from bilibili"), "某番");
    }

    #[test]
    fn year_of_parses_dates_and_years() {
        assert_eq!(year_of("2021"), Some(2021));
        assert_eq!(year_of("2021-05-01"), Some(2021));
        assert_eq!(year_of("S01E02"), None);
        assert_eq!(year_of(""), None);
    }

    #[test]
    fn matches_by_tmdb_id_with_kind() {
        let items = vec![
            json!({"guid":"g1","title":"进击的巨人","type":"TV","release_date":"2013-04-07","tmdb_id":1429}),
            json!({"guid":"g2","title":"某电影","type":"Movie","release_date":"2020-01-01"}),
        ];
        let idx = build_index(&items);
        let w = work(json!({"tmdbId":1429,"mediaType":"tv","title":"进击的巨人"}));
        assert_eq!(match_work(&idx, &w).as_deref(), Some("g1"));
        // 类型不符不应命中（同一 tmdb id 但作品是电影）
        let w2 = work(json!({"tmdbId":1429,"mediaType":"movie","title":"进击的巨人"}));
        assert_eq!(match_work(&idx, &w2), None);
    }

    #[test]
    fn matches_by_title_and_year_only() {
        let items = vec![json!({
            "guid":"g2","title":"某电影","type":"Movie","release_date":"2020-01-01"
        })];
        let idx = build_index(&items);
        assert_eq!(
            match_work(&idx, &work(json!({"title":"某电影","year":"2020","mediaType":"movie"}))).as_deref(),
            Some("g2")
        );
        // 同年份但标题不同 → 不命中
        assert_eq!(
            match_work(&idx, &work(json!({"title":"别的电影","year":"2020","mediaType":"movie"}))),
            None
        );
    }

    #[test]
    fn title_remake_with_different_year_does_not_match() {
        let items = vec![json!({
            "guid":"g3","title":"同名电影","type":"Movie","release_date":"1999-01-01"
        })];
        let idx = build_index(&items);
        assert_eq!(
            match_work(&idx, &work(json!({"title":"同名电影","year":"2021","mediaType":"movie"}))),
            None
        );
        // 作品缺年份时允许命中（无法排除，只能接受）
        assert_eq!(
            match_work(&idx, &work(json!({"title":"同名电影","mediaType":"movie"}))).as_deref(),
            Some("g3")
        );
    }

    #[test]
    fn matches_by_trim_id_bgm() {
        let items = vec![json!({
            "guid":"g4","title":"某番","type":"TV","trim_id":"bg456080"
        })];
        let idx = build_index(&items);
        let w = work(json!({"bgmId":456080,"mediaType":"anime","title":"某番"}));
        assert_eq!(match_work(&idx, &w).as_deref(), Some("g4"));
    }

    #[test]
    fn prefix_fallback_requires_same_year() {
        let items = vec![json!({
            "guid":"g5","title":"我推的孩子 第二季","type":"TV","release_date":"2024-07-03"
        })];
        let idx = build_index(&items);
        assert_eq!(
            match_work(&idx, &work(json!({"title":"我推的孩子","year":"2024","mediaType":"tv"}))).as_deref(),
            Some("g5")
        );
        // 年份不同 → 前缀兜底不生效
        assert_eq!(
            match_work(&idx, &work(json!({"title":"我推的孩子","year":"2023","mediaType":"tv"}))),
            None
        );
    }
}
