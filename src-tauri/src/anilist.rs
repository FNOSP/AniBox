//! [lc-1582] AniList **官方 GraphQL API**（`POST https://graphql.anilist.co`，免费、**无需 key**）
//! —— 只用来补 BGM 缺的「**具体放送时刻**」。
//!
//! 背景：BGM 的 /calendar 每条只有 `air_weekday`（星期）与 `air_date`（首播日），**没有"几点更新"**；
//! AniList 的 `Page.airingSchedules(airingAt_greater, airingAt_lesser, sort: TIME)` 一次就能拿到
//! **某一天**所有番的精确放送时刻（`airingAt` = UTC 秒）+ 集号。实测（2026-10-16）：
//! 药屋少女的呢喃 第3季 → 第 3 话 `airingAt=1792159200`（本地 22:00）。
//!
//! 为什么**直连、不走自建镜像**：实测本机（用户网络）可直连 `graphql.anilist.co`（不像 bgm/tmdb 被墙），
//! 且无需 key —— 走镜像只是多一跳。**失败时前端静默不显示时刻**（降级安全，不影响列表本身）。
//!
//! 条款（docs.anilist.co/guide/terms-of-use）：非商业免费；**禁止**把 API 当备份/存储服务、
//! **禁止大批量囤数据** ⇒ 我们只按"某一天"按需取 + 按天落盘缓存，绝不预抓整季/全站。
//! 限流：正常 90/分，**当前降级 30/分**（响应头 X-RateLimit-*）；超限 429 + Retry-After。
//! 所以：**一次拿一整天**（最多 4 页 × 50）、按天缓存。

use serde_json::{json, Value};
use std::time::Duration;

use crate::disk_cache;

const ENDPOINT: &str = "https://graphql.anilist.co";
const REQ_TIMEOUT_SECS: u64 = 10;
/// 每页 50（AniList 上限）× 最多 4 页 = 200 条/天（一天的放送表通常十几~一百多条）
const PER_PAGE: usize = 50;
const MAX_PAGES: usize = 4;
/// 落盘 TTL：**按天**缓存（某一天的放送表定了就不变；第二天是另一个缓存键）—— 24h 足够
const TTL_SECS: u64 = 24 * 3600;

const QUERY: &str = r#"
query ($a: Int, $b: Int, $p: Int) {
  Page(page: $p, perPage: 50) {
    pageInfo { hasNextPage }
    airingSchedules(airingAt_greater: $a, airingAt_lesser: $b, sort: TIME) {
      episode
      airingAt
      media { id title { native romaji english } }
    }
  }
}
"#;

/// [lc-1589] 往季番表查询：`Page.media(season, seasonYear, sort: POPULARITY_DESC)`。
/// 弹窗「总表查询」用（选年份 + 季度 → 该季番剧列表）。最多取 2 页（100 条，按人气排序足够）。
const SEASON_QUERY: &str = r#"
query ($y: Int, $s: MediaSeason, $p: Int) {
  Page(page: $p, perPage: 50) {
    pageInfo { hasNextPage }
    media(season: $s, seasonYear: $y, type: ANIME, sort: POPULARITY_DESC, isAdult: false) {
      id
      title { native romaji english }
      coverImage { medium }
      bannerImage
      episodes
      format
      status
      averageScore
      genres
    }
  }
}
"#;
const SEASON_MAX_PAGES: usize = 2;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(REQ_TIMEOUT_SECS))
        .build()
        .expect("anilist client build")
}

/// 某一天的放送表：`[start, end)` 为**本地时区**的 Unix 秒（**前端算好传进来** —— 不在 Rust 侧碰时区）。
/// 返回 `{ start, end, count, pages, items: [{anilistId, episode, airingAt, titles:[native,romaji,english]}] }`。
/// 前端按**标题**把 items 匹配到 BGM 日历行上，行上显示「第 N 话 · HH:MM」。
#[tauri::command]
pub async fn anilist_day_schedule(start: i64, end: i64) -> Result<Value, String> {
    if end <= start {
        return Err("时间范围非法".to_string());
    }
    let key = format!("json:anilist:day:v1|{start}|{end}");
    if let Some(v) = disk_cache::json_get(&key, TTL_SECS) {
        return Ok(v);
    }
    // ⚠ 查询窗口要在"本地那一天"上**前后各放宽 18 小时**：BGM 按**日本放送日**分组，而 airingAt 是
    // 绝对时刻 —— 深夜番（JST 24:53 这类）会落到本地隔天（实测东京复仇者 → 本地次日 00:53）。
    // 放宽后命中率 6/14 → 7/14；每周一集，3 天窗口内同一部不会出现两条，按标题匹配不会挑错集。
    // 缓存键仍按**本地那一天**（放宽量是常量，同一天永远同一把键）。
    const PAD_SECS: i64 = 18 * 3600;
    let (q_start, q_end) = (start - PAD_SECS, end + PAD_SECS);
    let http = client();
    let mut items: Vec<Value> = Vec::new();
    let mut pages = 0usize;
    for p in 1..=MAX_PAGES {
        let body = json!({ "query": QUERY, "variables": { "a": q_start, "b": q_end, "p": p } });
        let resp = http
            .post(ENDPOINT)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("AniList 连接失败：{e}"))?;
        let status = resp.status().as_u16();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| format!("AniList 响应不是合法 JSON：{e}"))?;
        if status != 200 {
            let msg = v.get("errors").map(|e| e.to_string()).unwrap_or_default();
            return Err(format!(
                "AniList HTTP {status}：{}",
                crate::mirror_sign::truncate(&msg, 160)
            ));
        }
        let page = v.pointer("/data/Page");
        let arr = page
            .and_then(|p| p.get("airingSchedules"))
            .and_then(|a| a.as_array());
        let Some(arr) = arr else { break };
        for s in arr {
            let Some(at) = s.get("airingAt").and_then(|x| x.as_i64()) else {
                continue;
            };
            let media = s.get("media").cloned().unwrap_or(Value::Null);
            let t = media.get("title").cloned().unwrap_or(Value::Null);
            // 三种标题都给前端（日文原名 / 罗马音 / 英文）——BGM 侧的名字可能是日文原名或中文，
            // 由前端用归一化标题（全等→前缀）去匹配，多给几种能显著提高命中率。
            let titles: Vec<String> = ["native", "romaji", "english"]
                .iter()
                .filter_map(|k| t.get(*k).and_then(|x| x.as_str()))
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            items.push(json!({
                "anilistId": media.get("id").and_then(|x| x.as_i64()).unwrap_or(0),
                "episode": s.get("episode").and_then(|x| x.as_i64()).unwrap_or(0),
                "airingAt": at,
                "titles": titles,
            }));
        }
        pages = p;
        let more = page
            .and_then(|p| p.pointer("/pageInfo/hasNextPage"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        if !more || arr.len() < PER_PAGE {
            break;
        }
    }
    let out = json!({
        "start": start, "end": end, "qStart": q_start, "qEnd": q_end,
        "count": items.len(), "pages": pages, "items": items,
    });
    disk_cache::json_put(&key, &out);
    crate::libmpv::trace(&format!(
        "[anilist] 当日放送表 {start}..{end}（查 {q_start}..{q_end}）→ {} 条（{pages} 页）",
        items.len()
    ));
    Ok(out)
}

/// [lc-1589] **往季番表**：`Page.media(season, seasonYear, sort: POPULARITY_DESC)` ——
/// 弹窗「总表查询」用（选年份 + 季度 → 看该季番剧列表，数据即 AniList 的季度表）。
/// 缓存 `json:anilist:season:v2|{year}|{season}`，TTL 24h（往季基本定型；当季评分/状态还会动）。
/// [lc-1600] v1→v2：条目新增 `banner` 字段（bannerImage，游客模式首页横图用）。键升版强制重取 ——
/// 否则旧缓存里没有 banner，游客 hero 只能退化成糊封面。
#[tauri::command]
pub async fn anilist_season(year: i64, season: String) -> Result<Value, String> {
    let s = season.trim().to_ascii_uppercase();
    if !matches!(s.as_str(), "WINTER" | "SPRING" | "SUMMER" | "FALL") {
        return Err(format!("season 只支持 WINTER/SPRING/SUMMER/FALL：{season}"));
    }
    if !(1940..=2100).contains(&year) {
        return Err(format!("year 非法：{year}"));
    }
    let key = format!("json:anilist:season:v2|{year}|{s}");
    if let Some(v) = disk_cache::json_get(&key, TTL_SECS) {
        return Ok(v);
    }
    let http = client();
    let mut items: Vec<Value> = Vec::new();
    for p in 1..=SEASON_MAX_PAGES {
        let body = json!({ "query": SEASON_QUERY, "variables": { "y": year, "s": s, "p": p } });
        let resp = http
            .post(ENDPOINT)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("AniList 连接失败：{e}"))?;
        let status = resp.status().as_u16();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| format!("AniList 响应不是合法 JSON：{e}"))?;
        if status != 200 {
            let msg = v.get("errors").map(|e| e.to_string()).unwrap_or_default();
            return Err(format!(
                "AniList HTTP {status}：{}",
                crate::mirror_sign::truncate(&msg, 160)
            ));
        }
        let page = v.pointer("/data/Page");
        let arr = page.and_then(|p| p.get("media")).and_then(|a| a.as_array());
        let Some(arr) = arr else { break };
        for m in arr {
            let t = m.get("title").cloned().unwrap_or(Value::Null);
            let titles: Vec<String> = ["native", "romaji", "english"]
                .iter()
                .filter_map(|k| t.get(*k).and_then(|x| x.as_str()))
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            let genres: Vec<String> = m
                .get("genres")
                .and_then(|g| g.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| s.to_string()).collect())
                .unwrap_or_default();
            items.push(json!({
                "anilistId": m.get("id").and_then(|x| x.as_i64()).unwrap_or(0),
                "titles": titles,
                "cover": m.pointer("/coverImage/medium").and_then(|x| x.as_str()).unwrap_or(""),
                // [lc-1600] 横图（bannerImage，可能为空）：游客模式首页轮播用它当 hero 横图
                "banner": m.get("bannerImage").and_then(|x| x.as_str()).unwrap_or(""),
                "episodes": m.get("episodes").and_then(|x| x.as_i64()).unwrap_or(0),
                "format": m.get("format").and_then(|x| x.as_str()).unwrap_or(""),
                "status": m.get("status").and_then(|x| x.as_str()).unwrap_or(""),
                "score": m.get("averageScore").and_then(|x| x.as_i64()).unwrap_or(0),
                "genres": genres,
            }));
        }
        let more = page
            .and_then(|p| p.pointer("/pageInfo/hasNextPage"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        if !more || arr.len() < PER_PAGE {
            break;
        }
    }
    let out = json!({ "year": year, "season": s, "count": items.len(), "items": items });
    disk_cache::json_put(&key, &out);
    crate::libmpv::trace(&format!("[anilist] 往季番表 {year} {s} → {} 条", items.len()));
    Ok(out)
}
