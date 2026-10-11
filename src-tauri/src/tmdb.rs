//! TMDB 详情拉取 —— 原样照搬主仓库 src/main/handlers/plugins/tmdbSync.ts 的
//! fetchShowDetails + normalizeShow（一次主请求 + append_to_response 合并）。
//! 鉴权：v4 JWT（Bearer）或 v3 api_key（query）。Key 来源：主仓库 config.json 的 tmdbApiKey
//! （设置面板写 key 后在此处同源读取；也支持环境变量 FNTV_TMDB_KEY 覆盖）。

use serde_json::{json, Value};
use std::sync::OnceLock;

const TMDB_API: &str = "https://api.themoviedb.org/3";

fn api_key() -> Option<String> {
    static KEY: OnceLock<Option<String>> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Ok(k) = std::env::var("FNTV_TMDB_KEY") {
            if !k.trim().is_empty() {
                return Some(k.trim().to_string());
            }
        }
        // 主仓库 fn_config/config.json（Tauri2 PoC 复用主仓库配置，登录信息同源）
        let p = std::path::Path::new(&std::env::var("APPDATA").ok()?)
            .join("fntv/config.json");
        let s = std::fs::read_to_string(p).ok()?;
        let v: Value = serde_json::from_str(&s).ok()?;
        v.get("tmdbApiKey")
            .and_then(|k| k.as_str())
            .filter(|k| !k.trim().is_empty())
            .map(|k| k.trim().to_string())
    })
    .clone()
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) fntv-tauri2")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("tmdb http client")
}

fn year_of(s: &str) -> String {
    s.chars().take(4).collect()
}

/// 主仓库 authFor：v4 JWT → Bearer 头；v3 → query 参数
fn auth_params(key: &str) -> (Vec<(String, String)>, Vec<(String, String)>) {
    // (query 参数, header)。v4 JWT 走 Bearer 头；v3 走 api_key query
    if key.starts_with("eyJ") {
        (vec![], vec![("Authorization".into(), format!("Bearer {}", key))])
    } else {
        (vec![("api_key".into(), key.into())], vec![])
    }
}

fn names_of(v: Option<&Value>) -> Vec<String> {
    match v.and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|x| {
                x.get("name")
                    .and_then(|n| n.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
            })
            .collect(),
        None => vec![],
    }
}

#[tauri::command]
pub async fn tmdb_show_details(arg: Value) -> Result<Value, String> {
    let key = api_key().ok_or("未配置 TMDB API Key")?;
    let mt = if arg.get("mediaType").and_then(|v| v.as_str()) == Some("movie") {
        "movie"
    } else {
        "tv"
    };
    let title = arg.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let year = arg.get("year").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (query_auth, header_auth) = auth_params(&key);
    let client = http();

    // 1) 搜索 best match（对齐 tmdbSearchBest：先精确名+年，退纯名）
    let mut search_url = format!("{}/search/{}?query={}", TMDB_API, mt, urlencoding::encode(&title));
    for (k, v) in &query_auth {
        search_url.push_str(&format!("&{}={}", k, v));
    }
    search_url.push_str(&format!("&language=zh-CN"));
    if !year.is_empty() {
        let yk = if mt == "movie" { "year" } else { "first_air_date_year" };
        search_url.push_str(&format!("&{}={}", yk, year));
    }
    let mut req = client.get(&search_url);
    for (k, v) in &header_auth {
        req = req.header(k, v);
    }
    let sr: Value = req.send().await.map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
    let id = sr
        .get("results")
        .and_then(|r| r.as_array())
        .and_then(|arr| arr.first())
        .and_then(|r| r.get("id"))
        .and_then(|i| i.as_i64())
        .ok_or_else(|| format!("TMDB 未找到匹配条目：{}", title))?;

    // 2) 详情 + append_to_response（对齐主仓库字段清单）
    let append = if mt == "tv" {
        "aggregate_credits,credits,external_ids,keywords,content_ratings,alternative_titles,images,videos,watch/providers,recommendations"
    } else {
        "credits,external_ids,keywords,alternative_titles,images,videos,release_dates,watch/providers,recommendations"
    };
    let mut detail_url = format!(
        "{}/{}/{}?language=zh-CN&append_to_response={}&include_image_language=zh-CN,en,null",
        TMDB_API, mt, id, append
    );
    for (k, v) in &query_auth {
        detail_url.push_str(&format!("&{}={}", k, v));
    }
    let mut req = client.get(&detail_url);
    for (k, v) in &header_auth {
        req = req.header(k, v);
    }
    let d: Value = req.send().await.map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;

    // 3) 归一化（字段与主仓库 normalizeShow 对齐，只取渲染需要的）
    let s = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cast = d
        .pointer("/aggregate_credits/cast")
        .or_else(|| d.pointer("/credits/cast"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .take(20)
                .filter_map(|c| {
                    let name = c.get("name")?.as_str()?.to_string();
                    let character = c
                        .get("character")
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            c.get("roles")
                                .and_then(|r| r.as_array())
                                .and_then(|a| a.first())
                                .and_then(|r| r.get("character"))
                                .and_then(|v| v.as_str())
                        })
                        .unwrap_or("")
                        .to_string();
                    Some(json!({
                        "name": name,
                        "character": character,
                        "profile": c.get("profile_path").and_then(|v| v.as_str()).unwrap_or(""),
                        "order": c.get("order").and_then(|v| v.as_i64()).unwrap_or(999),
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut cast = cast;
    cast.sort_by_key(|c| c.get("order").and_then(|o| o.as_i64()).unwrap_or(999));

    let backdrops: Vec<String> = d
        .pointer("/images/backdrops")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|b| {
                    let p = b.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                    !p.is_empty()
                        && p != s("backdrop_path")
                        && b.get("width").and_then(|w| w.as_i64()).map(|w| w >= 400).unwrap_or(true)
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .iter()
        .cloned()
        .collect::<Vec<Value>>()
        .into_iter()
        .map(|b| b.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string())
        .collect();

    // 相似剧集（recommendations）
    let recommendations: Vec<Value> = d
        .pointer("/recommendations/results")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    let tmdb_id = r.get("id")?.as_i64()?;
                    let t = if mt == "movie" {
                        r.get("title").or_else(|| r.get("original_title"))
                    } else {
                        r.get("name").or_else(|| r.get("original_name"))
                    }
                    .and_then(|v| v.as_str())?
                    .to_string();
                    let date = if mt == "movie" {
                        r.get("release_date")
                    } else {
                        r.get("first_air_date")
                    }
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                    Some(json!({
                        "tmdbId": tmdb_id,
                        "title": t,
                        "year": year_of(date),
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let recommendations: Vec<Value> =
        recommendations.into_iter().take(8).collect();

    // 别名（tv: results / movie: titles）
    let alt = if mt == "tv" {
        d.pointer("/alternative_titles/results")
    } else {
        d.pointer("/alternative_titles/titles")
    };
    let title_v = s("name");
    let title_v = if title_v.is_empty() { s("title") } else { title_v };
    let original = if mt == "movie" {
        s("original_title")
    } else {
        s("original_name")
    };
    let aliases: Vec<String> = alt
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.get("title").and_then(|t| t.as_str()))
                .filter(|t| !t.is_empty() && t != &title_v && t != &original)
                .take(8)
                .map(|t| t.to_string())
                .collect()
        })
        .unwrap_or_default();

    // 关键词
    let kw = if mt == "tv" {
        d.pointer("/keywords/results")
    } else {
        d.pointer("/keywords/keywords")
    };
    let keywords = names_of(kw).into_iter().take(16).collect::<Vec<_>>();

    // 在线看（watch/providers flatrate 六区合并去重，滤 with Ads）
    let providers: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for region in ["CN", "HK", "TW", "JP", "KR", "US"] {
            let path = format!("/watch~1providers/results/{}/flatrate", region);
            if let Some(arr) = d.pointer(&path).and_then(|v| v.as_array()) {
                for n in arr.iter().filter_map(|p| p.get("provider_name").and_then(|v| v.as_str())) {
                    if n.ends_with(" with Ads") || n.ends_with("with Ads") {
                        continue;
                    }
                    if seen.insert(n.to_string()) {
                        out.push(n.to_string());
                    }
                }
            }
        }
        out.into_iter().take(8).collect()
    };


    let air = if mt == "movie" { s("release_date") } else { s("first_air_date") };

    Ok(json!({
        "tmdbId": id,
        "mediaType": mt,
        "title": title_v,
        "originalTitle": original,
        "overview": s("overview"),
        "year": year_of(&air),
        "airDate": air,
        "status": s("status"),
        "seasons": d.get("number_of_seasons").and_then(|v| v.as_i64()).unwrap_or(0),
        "episodes": d.get("number_of_episodes").and_then(|v| v.as_i64()).unwrap_or(0),
        "rating": d.get("vote_average").and_then(|v| v.as_f64()).unwrap_or(0.0),
        "votes": d.get("vote_count").and_then(|v| v.as_i64()).unwrap_or(0),
        "popularity": d.get("popularity").and_then(|v| v.as_f64()).unwrap_or(0.0),
        "genres": names_of(d.get("genres")),
        "networks": names_of(d.get("networks")),
        "cast": cast,
        // 剧照排序：vote_average 降序（主仓库 normalizeShow 同款）
        "backdrops": backdrops,
        "keywords": keywords,
        "aliases": aliases,
        "providers": providers,
        "recommendations": recommendations,
        "externalIds": {
            "imdb": d.pointer("/external_ids/imdb_id").and_then(|v| v.as_str())
                .or_else(|| d.get("imdb_id").and_then(|v| v.as_str())).unwrap_or(""),
            "tmdb": id.to_string(),
            "wikidata": d.pointer("/external_ids/wikidata_id").and_then(|v| v.as_str()).unwrap_or(""),
            "instagram": d.pointer("/external_ids/instagram_id").and_then(|v| v.as_str()).unwrap_or(""),
            "twitter": d.pointer("/external_ids/twitter_id").and_then(|v| v.as_str()).unwrap_or(""),
        },
    }))
}
