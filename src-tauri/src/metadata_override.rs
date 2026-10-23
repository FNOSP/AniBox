//! [lc-1720] 剧集「手动获取元数据」：本地覆盖层。
//!
//! 需求：海报卡「⋯ → 刷新元数据」改为**手动搜索**（TMDB / Bangumi 镜像两路），
//! 用户选定后**只写本地**（不调用飞牛任何写接口），并按「剧 guid（series guid）」
//! 对该剧的所有展示（首页行 / 媒体库 / 详情页 / 轮播）统一覆盖可用字段。
//!
//! 存储：`app_data_dir/metadata_overrides.json`，形状 `{ "<series guid>": MetadataChoice }`。
//! 选 `metadata_clear_override` 删除该键即恢复飞牛原始数据（原始响应未被修改）。
//!
//! 覆盖策略（只在字段有值时才覆盖，缺省保留飞牛原值）：
//!   · 剧/电影根条目：title / overview / vote_average / release_date / air_date / poster
//!   · 集/季条目：tv_title、series_poster、series_rating（前端可选用），
//!     并仅在原字段为空时补 overview / air_date —— **不覆盖集标题与集号**。
//!
//! [lc-1726] **季/集数不参与覆盖**：`number_of_seasons` / `number_of_episodes` /
//! `local_number_of_seasons` / `local_number_of_episodes` 一律保持飞牛原值，
//! 卡片与详情页展示的季/集数只反映**实际入库**数量，不随 TMDB/BGM 元数据变化。
//! `MetadataChoice.seasons/episodes` 仅用于搜索结果的展示，不再写回条目。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// 用户选定的元数据（前端 `metadata_search` 的统一结果原样回传）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetadataChoice {
    /// "tmdb" | "bgm"
    #[serde(default)]
    pub source: String,
    /// TMDB id / Bangumi subject id（统一转成字符串）
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub original_title: String,
    /// 年份（"2024"）
    #[serde(default)]
    pub year: String,
    /// 首播/上映日期（"YYYY-MM-DD"，可能为空）
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub overview: String,
    /// 0–10
    #[serde(default)]
    pub rating: f64,
    /// 已归一为前端可直接 imgUrl() 的完整 URL（TMDB 走 /__tmdb 代理、BGM 走 /__bgm 代理）
    #[serde(default)]
    pub poster: String,
    #[serde(default)]
    pub backdrop: String,
    /// [lc-1726] 搜索结果里的季数 —— **仅用于搜索/选择界面展示**，不写回条目
    /// （卡片/详情页的季数必须是飞牛实际入库数量）。
    #[serde(default)]
    pub seasons: i64,
    /// [lc-1726] 搜索结果里的集数 —— 同上，不写回条目。
    #[serde(default)]
    pub episodes: i64,
    /// 保存时刻（ms）
    #[serde(default)]
    pub updated_at: i64,
}

fn store_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("取应用数据目录失败: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("metadata_overrides.json"))
}

/// 读取全部覆盖（键 = 剧 guid）。文件不存在/损坏 → 空表（不阻塞任何展示）。
pub fn load(app: &AppHandle) -> HashMap<String, MetadataChoice> {
    let Ok(p) = store_path(app) else {
        return HashMap::new();
    };
    let Ok(text) = std::fs::read_to_string(&p) else {
        return HashMap::new();
    };
    serde_json::from_str::<HashMap<String, MetadataChoice>>(&text).unwrap_or_default()
}

fn save(app: &AppHandle, map: &HashMap<String, MetadataChoice>) -> Result<(), String> {
    let p = store_path(app)?;
    let text = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    // 原子写：先写临时文件再 rename，避免中途崩溃留下半截 JSON。
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写元数据覆盖失败: {e}"))?;
    std::fs::rename(&tmp, &p).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("替换元数据覆盖失败: {e}")
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn str_of(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    String::new()
}

/// 从一条飞牛条目上取「剧 guid」。
///
/// 优先级：`series_guid/seriesGuid` → 类型为 TV/Movie 时自身 `guid`（剧/电影卡片）
/// → `parent_guid/parentGuid`（部分接口把剧 guid 直接放在 parent_guid）。
/// 注意：飞牛 `episode/list` 的 `parent_guid` 实测是**季 guid**，因此集条目不应
/// 依赖它做剧级匹配；详情页会显式传入剧 guid（`get_episodes(seriesGuid)`）。
pub fn series_key_of(item: &Value) -> Option<String> {
    let direct = str_of(item, &["series_guid", "seriesGuid"]);
    if !direct.is_empty() {
        return Some(direct);
    }
    let typ = str_of(item, &["type", "mediaType", "media_type"]).to_ascii_lowercase();
    let guid = str_of(item, &["guid"]);
    if !guid.is_empty() && (typ == "tv" || typ == "movie" || typ == "film") {
        return Some(guid);
    }
    let parent = str_of(item, &["parent_guid", "parentGuid"]);
    if !parent.is_empty() {
        return Some(parent);
    }
    if !guid.is_empty() {
        return Some(guid);
    }
    None
}

/// 把选定元数据叠加到一条条目上。`is_root` = 该条目本身就是剧/电影（而不是集/季）。
pub fn apply_choice(item: &mut Value, choice: &MetadataChoice, is_root: bool) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };

    if is_root {
        if !choice.title.is_empty() {
            obj.insert("title".into(), json!(choice.title));
            // 详情页标题读取顺序是 tvTitle → title（见 DetailView），剧集条目的 tvTitle
            // 往往还是飞牛旧剧名 —— 两个键一起覆盖，确保选中标题真正生效。
            obj.insert("tv_title".into(), json!(choice.title));
            obj.insert("tvTitle".into(), json!(choice.title));
        }
        if !choice.overview.is_empty() {
            obj.insert("overview".into(), json!(choice.overview));
        }
        if choice.rating > 0.0 {
            // 飞牛 vote_average 是字符串（前端 Number() 兼容数字），统一给字符串。
            obj.insert("vote_average".into(), json!(format!("{:.1}", choice.rating)));
        }
        if !choice.date.is_empty() {
            obj.insert("release_date".into(), json!(choice.date));
            obj.insert("air_date".into(), json!(choice.date));
        }
        if !choice.poster.is_empty() {
            obj.insert("poster".into(), json!(choice.poster));
        }
        // [lc-1726] 刻意**不覆盖** number_of_seasons / number_of_episodes：
        // 这两个字段（以及 local_*）是飞牛入库口径，卡片「N 季 · 共 M 集」必须反映
        // 实际入库数量，不能被 TMDB/BGM 的总季/总集数改写。choice.seasons/episodes
        // 只用于搜索结果展示，不落到条目上。
        // 供详情页识别「这是本地覆盖」并按覆盖值显示评分/年份（详情页的评分原本只读 BGM/TMDB）。
        obj.insert("_metadata_override".into(), json!(true));
        obj.insert("_override_rating".into(), json!(choice.rating));
        obj.insert("_override_date".into(), json!(choice.date));
        // 详情页标题/原名会读 tv_title / title；剧根条目保留原 tv_title 不动。
    } else {
        // 集/季：不覆盖集标题、集号与集封面；只补「剧级」信息。
        if !choice.title.is_empty() {
            obj.insert("tv_title".into(), json!(choice.title));
        }
        if !choice.poster.is_empty() {
            obj.insert("series_poster".into(), json!(choice.poster));
        }
        if choice.rating > 0.0 {
            obj.insert("series_rating".into(), json!(choice.rating));
        }
        let overview_empty = obj
            .get("overview")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().is_empty())
            .unwrap_or(true);
        if overview_empty && !choice.overview.is_empty() {
            obj.insert("overview".into(), json!(choice.overview));
        }
        let date_empty = obj
            .get("air_date")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().is_empty())
            .unwrap_or(true);
        if date_empty && !choice.date.is_empty() {
            obj.insert("air_date".into(), json!(choice.date));
        }
    }
}

/// 对 `{ list: [...] }` 形态的响应就地应用覆盖（get_items / get_items_global /
/// get_carousel_items / get_favorites 共用）。
pub fn apply_response_list(app: &AppHandle, resp: &mut Value) {
    let map = load(app);
    if map.is_empty() {
        return;
    }
    let Some(list) = resp.get_mut("list").and_then(|v| v.as_array_mut()) else {
        return;
    };
    for it in list.iter_mut() {
        let Some(key) = series_key_of(it) else {
            continue;
        };
        let Some(choice) = map.get(&key) else {
            continue;
        };
        let is_root = str_of(it, &["guid"]) == key
            && {
                let typ = str_of(it, &["type", "mediaType", "media_type"]).to_ascii_lowercase();
                typ == "tv" || typ == "movie" || typ == "film"
            };
        apply_choice(it, choice, is_root);
    }
}

/// 对单个条目（get_item_detail）就地应用覆盖。`series_guid` 可由调用方显式指定
/// （集详情场景）；缺省从条目自身字段推断。
pub fn apply_to_item(app: &AppHandle, item: &mut Value, series_guid: Option<&str>) {
    let map = load(app);
    if map.is_empty() {
        return;
    }
    let key = series_guid
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| series_key_of(item));
    let Some(key) = key else {
        return;
    };
    let Some(choice) = map.get(&key) else {
        return;
    };
    let guid = str_of(item, &["guid"]);
    let typ = str_of(item, &["type", "mediaType", "media_type"]).to_ascii_lowercase();
    let is_root = guid == key || typ == "tv" || typ == "movie" || typ == "film";
    apply_choice(item, choice, is_root);
}

/// [lc-1728] 对一组条目就地应用覆盖（每条自选键；整表只读一次盘）。
/// 用于「作品聚合卡」类响应（如 get_watch_history 的观影记录：guid = 作品/剧 guid，
/// 字段形状与 item/list 不同，走 [`series_key_of`] 自选键 + guid==key 判根）。
pub fn apply_to_items(app: &AppHandle, items: &mut [Value]) {
    let map = load(app);
    if map.is_empty() {
        return;
    }
    for item in items.iter_mut() {
        apply_with_map(item, &map);
    }
}

/// [`apply_to_items`] 的内层纯逻辑（不触盘，便于单测）：自选键命中覆盖表后按根/集应用。
fn apply_with_map(item: &mut Value, map: &HashMap<String, MetadataChoice>) {
    let Some(key) = series_key_of(item) else {
        return;
    };
    let Some(choice) = map.get(&key) else {
        return;
    };
    let guid = str_of(item, &["guid"]);
    let typ = str_of(item, &["type", "mediaType", "media_type"]).to_ascii_lowercase();
    let is_root = guid == key || typ == "tv" || typ == "movie" || typ == "film";
    apply_choice(item, choice, is_root);
}

/// 对一组条目应用覆盖（get_episodes 的集列表）。`series_guid` 为剧 guid。
pub fn apply_to_list(app: &AppHandle, list: &mut Vec<Value>, series_guid: &str) {
    let map = load(app);
    let Some(choice) = map.get(series_guid) else {
        return;
    };
    for it in list.iter_mut() {
        let had_overview = it
            .get("overview")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        apply_choice(it, choice, false);
        // [多季] 集列表里的 `overview` 是**单集简介**，绝不能把「剧/第一季的整部简介」灌进去，
        //   否则多季条目每一集都会显示同一段（第一季）简介。原为空就保持为空，交给前端按集回填。
        if !had_overview {
            if let Some(obj) = it.as_object_mut() {
                obj.remove("overview");
            }
        }
    }
}

// ───────────────────────── 镜像图片 URL 归一 ─────────────────────────

/// TMDB `file_path` → fntvimg 代理 URL（与前端 `tmdbImg()` 完全一致）。
pub fn tmdb_img(path: &str, size: &str) -> String {
    let p = path.trim();
    if p.is_empty() {
        return String::new();
    }
    if p.starts_with("http://") || p.starts_with("https://") {
        return p.to_string();
    }
    let p = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    };
    format!("http://fntvimg.localhost/__tmdb/t/p/{size}{p}")
}

/// Bangumi 图片 URL → fntvimg 代理 URL（与前端 `bgmImg()` 等价）。
pub fn bgm_img(url: &str) -> String {
    let u = url.trim();
    if u.is_empty() {
        return String::new();
    }
    if let Some(rest) = u
        .strip_prefix("https://lain.bgm.tv")
        .or_else(|| u.strip_prefix("http://lain.bgm.tv"))
    {
        return format!("http://fntvimg.localhost/__bgm{rest}");
    }
    if u.starts_with("//") {
        return format!("http://fntvimg.localhost/__bgm{u}");
    }
    u.to_string()
}

// ───────────────────────── Tauri 命令 ─────────────────────────

/// 手动搜索元数据：并行走 TMDB 与 Bangumi 镜像，返回两路统一结构。
/// 单路失败不整体报错（errors 里回传原因），两路都失败才 Err。
#[tauri::command]
pub async fn metadata_search(query: String, media_type: Option<String>) -> Result<Value, String> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Err("搜索关键词为空".into());
    }
    let mt = media_type.unwrap_or_default().trim().to_ascii_lowercase();
    let want_movie = mt == "movie";
    let want_tv = mt == "tv";

    // TMDB：按提示类型搜索；未指定时 tv/movie 两路并发合并。
    let tmdb_fut = async {
        if want_movie {
            crate::tmdb::tmdb_search(json!({ "query": q.as_str(), "mediaType": "movie", "page": 1 })).await
        } else if want_tv {
            crate::tmdb::tmdb_search(json!({ "query": q.as_str(), "mediaType": "tv", "page": 1 })).await
        } else {
            let (tv, mv) = tokio::join!(
                crate::tmdb::tmdb_search(json!({ "query": q.as_str(), "mediaType": "tv", "page": 1 })),
                crate::tmdb::tmdb_search(json!({ "query": q.as_str(), "mediaType": "movie", "page": 1 })),
            );
            match (tv, mv) {
                (Ok(mut a), Ok(b)) => {
                    if let (Some(x), Some(y)) =
                        (a.get_mut("items").and_then(|v| v.as_array_mut()), b.get("items").and_then(|v| v.as_array()))
                    {
                        x.extend(y.iter().cloned());
                    }
                    Ok(a)
                }
                (Ok(a), Err(_)) => Ok(a),
                (Err(_), Ok(b)) => Ok(b),
                (Err(e), Err(_)) => Err(e),
            }
        }
    };
    let bgm_fut = crate::bgm::search_subjects(&q, 20);

    let (tmdb_res, bgm_res) = tokio::join!(tmdb_fut, bgm_fut);

    let mut errors = serde_json::Map::new();
    let tmdb_items = match tmdb_res {
        Ok(v) => tmdb_search_items(&v),
        Err(e) => {
            errors.insert("tmdb".into(), json!(e));
            Vec::new()
        }
    };
    let bgm_items = match bgm_res {
        Ok(items) => bgm_search_items(&items),
        Err(e) => {
            errors.insert("bgm".into(), json!(e));
            Vec::new()
        }
    };
    if tmdb_items.is_empty() && bgm_items.is_empty() {
        if errors.is_empty() {
            return Ok(json!({ "query": q, "tmdb": [], "bgm": [], "errors": {} }));
        }
        return Err(format!(
            "镜像搜索失败：{}",
            errors
                .iter()
                .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    Ok(json!({ "query": q, "tmdb": tmdb_items, "bgm": bgm_items, "errors": errors }))
}

fn tmdb_search_items(resp: &Value) -> Vec<Value> {
    let media = resp
        .get("mediaType")
        .and_then(|v| v.as_str())
        .unwrap_or("tv");
    let subtitle = if media == "movie" { "电影" } else { "剧集" };
    resp.get("items")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|it| {
                    let id = it.get("tmdbId").and_then(|v| v.as_i64())?;
                    let title = it.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    if title.is_empty() {
                        return None;
                    }
                    let poster = it.get("poster").and_then(|v| v.as_str()).unwrap_or("");
                    let backdrop = it.get("backdropPath").and_then(|v| v.as_str()).unwrap_or("");
                    Some(json!({
                        "source": "tmdb",
                        "id": id.to_string(),
                        "title": title,
                        "originalTitle": it.get("originalTitle").and_then(|v| v.as_str()).unwrap_or(""),
                        "year": it.get("year").and_then(|v| v.as_str()).unwrap_or(""),
                        "date": it.get("date").and_then(|v| v.as_str()).unwrap_or(""),
                        "overview": it.get("overview").and_then(|v| v.as_str()).unwrap_or(""),
                        "rating": it.get("rating").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        "poster": tmdb_img(poster, "w500"),
                        "backdrop": tmdb_img(backdrop, "w780"),
                        "seasons": 0,
                        "episodes": 0,
                        "subtitle": subtitle,
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn bgm_search_items(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .filter_map(|it| {
            let id = it.get("id").and_then(|v| v.as_i64())?;
            let name = it.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let name_cn = it.get("name_cn").and_then(|v| v.as_str()).unwrap_or("").trim();
            let title = if name_cn.is_empty() { name } else { name_cn };
            if title.is_empty() {
                return None;
            }
            let date = it.get("date").and_then(|v| v.as_str()).unwrap_or("");
            let year = date.get(0..4).unwrap_or("");
            let summary = it.get("summary").and_then(|v| v.as_str()).unwrap_or("");
            let poster = it
                .get("images")
                .and_then(|i| i.get("large").or_else(|| i.get("common")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let eps = it.get("eps").and_then(|v| v.as_i64()).unwrap_or(0);
            let platform = it.get("platform").and_then(|v| v.as_str()).unwrap_or("");
            Some(json!({
                "source": "bgm",
                "id": id.to_string(),
                "title": title,
                "originalTitle": name,
                "year": year,
                "date": date,
                "overview": summary,
                "rating": it.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0),
                "poster": bgm_img(poster),
                "backdrop": "",
                "seasons": 0,
                "episodes": eps,
                "subtitle": if platform.is_empty() { "Bangumi".to_string() } else { platform.to_string() },
            }))
        })
        .collect()
}

/// 读取某部剧当前选定的本地元数据（无覆盖 → null）。
#[tauri::command]
pub async fn metadata_get_override(
    app: AppHandle,
    series_guid: String,
) -> Result<Option<MetadataChoice>, String> {
    let key = series_guid.trim();
    if key.is_empty() {
        return Ok(None);
    }
    Ok(load(&app).remove(key))
}

/// 保存用户选定的元数据（仅本地文件，不触碰飞牛）。
#[tauri::command]
pub async fn metadata_set_override(
    app: AppHandle,
    series_guid: String,
    mut choice: MetadataChoice,
) -> Result<MetadataChoice, String> {
    let key = series_guid.trim().to_string();
    if key.is_empty() {
        return Err("缺少剧 guid".into());
    }
    if choice.source.trim().is_empty() || choice.id.trim().is_empty() {
        return Err("缺少来源或条目 id".into());
    }
    if choice.title.trim().is_empty() {
        return Err("标题不能为空".into());
    }
    choice.updated_at = now_ms();
    let mut map = load(&app);
    map.insert(key, choice.clone());
    save(&app, &map)?;
    Ok(choice)
}

/// 清除本地覆盖（恢复飞牛原始元数据）。
#[tauri::command]
pub async fn metadata_clear_override(app: AppHandle, series_guid: String) -> Result<bool, String> {
    let key = series_guid.trim();
    if key.is_empty() {
        return Err("缺少剧 guid".into());
    }
    let mut map = load(&app);
    let removed = map.remove(key).is_some();
    if removed {
        save(&app, &map)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice() -> MetadataChoice {
        MetadataChoice {
            source: "tmdb".into(),
            id: "123".into(),
            title: "新标题".into(),
            original_title: "New Title".into(),
            year: "2024".into(),
            date: "2024-04-01".into(),
            overview: "新简介".into(),
            rating: 8.6,
            poster: "http://fntvimg.localhost/__tmdb/t/p/w500/a.jpg".into(),
            seasons: 2,
            episodes: 24,
            ..Default::default()
        }
    }

    #[test]
    fn series_key_prefers_explicit_series_guid() {
        let it = json!({ "guid": "ep1", "type": "Episode", "series_guid": "series1", "parent_guid": "season1" });
        assert_eq!(series_key_of(&it).as_deref(), Some("series1"));
    }

    #[test]
    fn series_key_uses_tv_guid() {
        let it = json!({ "guid": "tv1", "type": "TV" });
        assert_eq!(series_key_of(&it).as_deref(), Some("tv1"));
    }

    /// [lc-1728] 观影记录「作品聚合卡」形状：guid = 作品/剧 guid、无 type 字段 ——
    /// apply_to_items 的内层逻辑应能按 guid 命中并按根条目覆盖标题/海报，未命中的条目不动。
    #[test]
    fn apply_with_map_hits_aggregated_work_card() {
        let mut items = vec![
            json!({ "guid": "tv1", "title": "旧标题", "poster": "/old.jpg" }),
            json!({ "guid": "other", "title": "无关条目" }),
        ];
        let mut map = HashMap::new();
        map.insert("tv1".to_string(), choice());
        for it in items.iter_mut() {
            apply_with_map(it, &map);
        }
        assert_eq!(items[0]["title"], "新标题");
        assert_eq!(items[0]["tv_title"], "新标题");
        assert!(items[0]["poster"].as_str().unwrap().contains("/__tmdb/"));
        assert_eq!(items[1]["title"], "无关条目", "未命中的条目不得被改写");
    }

    #[test]
    fn apply_root_overlays_core_fields() {
        let mut item = json!({
            "guid": "tv1", "type": "TV", "title": "旧标题",
            "overview": "旧简介", "vote_average": "5.0", "release_date": "2020-01-01",
            "poster": "/old.jpg", "number_of_seasons": 1,
            "number_of_episodes": 13, "local_number_of_seasons": 1,
            "local_number_of_episodes": 12
        });
        apply_choice(&mut item, &choice(), true);
        assert_eq!(item["title"], "新标题");
        assert_eq!(item["overview"], "新简介");
        assert_eq!(item["vote_average"], "8.6");
        assert_eq!(item["release_date"], "2024-04-01");
        assert!(item["poster"].as_str().unwrap().contains("/__tmdb/"));
    }

    /// [lc-1726] 季/集数必须保持飞牛入库口径，不被 TMDB/BGM 覆盖。
    #[test]
    fn apply_root_keeps_library_season_and_episode_counts() {
        let mut item = json!({
            "guid": "tv1", "type": "TV",
            "number_of_seasons": 1, "number_of_episodes": 13,
            "local_number_of_seasons": 1, "local_number_of_episodes": 12
        });
        apply_choice(&mut item, &choice(), true); // choice: seasons=2, episodes=24
        assert_eq!(item["number_of_seasons"], 1);
        assert_eq!(item["number_of_episodes"], 13);
        assert_eq!(item["local_number_of_seasons"], 1);
        assert_eq!(item["local_number_of_episodes"], 12);
    }

    /// 条目原本没有季/集字段时，覆盖也不得凭空写入元数据的季/集总数。
    #[test]
    fn apply_root_does_not_invent_counts() {
        let mut item = json!({ "guid": "tv1", "type": "TV", "title": "旧标题" });
        apply_choice(&mut item, &choice(), true);
        assert!(item.get("number_of_seasons").is_none());
        assert!(item.get("number_of_episodes").is_none());
    }

    #[test]
    fn apply_episode_keeps_episode_title_and_number() {
        let mut ep = json!({
            "guid": "ep1", "type": "Episode", "title": "第 1 集",
            "episode_number": 1, "overview": ""
        });
        apply_choice(&mut ep, &choice(), false);
        assert_eq!(ep["title"], "第 1 集", "集标题不得被剧标题覆盖");
        assert_eq!(ep["episode_number"], 1);
        assert_eq!(ep["tv_title"], "新标题");
        assert_eq!(ep["overview"], "新简介");
        assert!(ep.get("series_poster").is_some());
    }

    #[test]
    fn image_urls_are_proxied() {
        assert_eq!(
            tmdb_img("/a.jpg", "w500"),
            "http://fntvimg.localhost/__tmdb/t/p/w500/a.jpg"
        );
        assert_eq!(
            bgm_img("https://lain.bgm.tv/pic/cover/l/ab/cd/123.jpg"),
            "http://fntvimg.localhost/__bgm/pic/cover/l/ab/cd/123.jpg"
        );
    }
}
