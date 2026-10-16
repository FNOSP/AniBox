//! [lc-1615] 本地播放历史 —— 「观影记录」页的两大数据源之一（另一来源 = 飞牛 play/list）。
//!
//! 为什么记录在 Rust 侧：本软件的两条实际播放链路（库内 `play_item`、在线源 `webrs_play`）
//! 都在 Rust 层发起，在「成功起播」那一刻落盘最可靠（前端刷新/切页/崩溃都不会丢）。
//! 存储：`app_data_dir/history.json`（与 config.json 同级），容量上限 [`MAX_ENTRIES`]，
//! 超出按播放时间裁掉最旧的。
//!
//! 合并口径（`get_watch_history` 使用，见 [`merge`]，纯函数、单测覆盖）：
//!   · 同 guid 取更新的那条 —— 本机记录带真实播放时间；飞牛 play/list **没有时间戳字段**
//!     （2026-10-16 实测 59 条样本，无 lastPlayed/updateTime 一类字段），所以同 guid 时
//!     本机记录胜出（它一定有真实时间），飞牛侧的进度/海报等空字段用于补齐。
//!   · 排序：played_at 倒序；无时间戳的（飞牛独有）保持 play/list 的原始顺序沉底。
//!   · 去重键：库内 = 条目 guid；在线源 = `webrs:<集页面 URL>`（前缀隔离，永不与飞牛 guid 相撞）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// 容量上限：观影记录页只关心近期历史；无限增长只会拖慢每次读写。
/// 500 条 ≈ 一年重度使用量，超出裁掉最旧的（按 played_at）。
pub const MAX_ENTRIES: usize = 500;

/// 富化阶段（季→剧）最多允许的**网络**请求数；缓存命中不计。
/// 兜底闸门：万一 play/list 异常大，不至于把「打开观影记录页」拖成批量拉取。
const ENRICH_MAX_FETCHES: usize = 100;
/// 富化并发（对齐 disk_cache 的图片闸门思路：别把 NAS 压出错误页）
const ENRICH_CONCURRENCY: usize = 6;
/// 2000-01-01 前的时间戳一律视为脏数据（epoch 0 / 进度值误当时间），对齐主仓库 MIN_VALID_TS
const MIN_VALID_MS: u64 = 946_684_800_000;

/// 一条本机播放记录（history.json 的数组元素）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct WatchRecord {
    /// 去重键：库内 = 播放条目 guid；在线源 = `webrs:<集页面 URL>`
    pub guid: String,
    /// 显示标题（剧名/电影名/番剧名）
    pub title: String,
    /// 集号标签（"S2E1"；电影为空）
    #[serde(default)]
    pub episode: String,
    /// 海报（NAS 相对路径；前端统一走 imgUrl 归一；在线源为空）
    #[serde(default)]
    pub poster: String,
    #[serde(default)]
    pub tmdb_id: String,
    #[serde(default)]
    pub bgm_id: String,
    /// 类型：Movie / Episode / ...（库内取 play/info 的 type；在线源固定 "webrs"）
    #[serde(default, rename = "type")]
    pub media_type: String,
    /// 播放时刻（毫秒时间戳）
    pub played_at: u64,
    /// 来源标记："fnos"（库内）| "webrs"（在线源）
    pub source: String,
    /// 来源名（在线源 = 规则名；库内为空）
    #[serde(default)]
    pub source_name: String,
    /// 所属季 guid（剧集；由 play/info 的 parent_guid 得到。剧 guid 在富化时补全）
    #[serde(default)]
    pub parent_guid: String,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn file_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("history.json"))
}

/// 读写串行化：record 是「读-改-写」，两条播放命令几乎同时触发时不能互相覆盖。
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 读取本机历史（文件不存在/损坏 → 空列表，绝不报错——观影记录页可以只显示飞牛部分）。
pub fn load(app: &AppHandle) -> Vec<WatchRecord> {
    let Some(p) = file_path(app) else {
        return Vec::new();
    };
    match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str::<Vec<WatchRecord>>(&s).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn save(app: &AppHandle, list: &[WatchRecord]) {
    let Some(p) = file_path(app) else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(list) {
        let _ = std::fs::write(&p, s);
    }
}

/// 落一条播放记录：同 guid 覆盖更新（保留旧记录里更全的字段），超上限裁最旧。
pub fn record(app: &AppHandle, rec: WatchRecord) {
    if rec.guid.is_empty() {
        return;
    }
    let _guard = LOCK.lock();
    let mut list = load(app);
    upsert(&mut list, rec);
    save(app, &list);
}

/// 插入/更新（纯函数，供单测）：同 guid 用新记录覆盖、空字段从旧记录补齐；
/// 之后按 played_at 倒序 + 裁到 [`MAX_ENTRIES`]。
fn upsert(list: &mut Vec<WatchRecord>, rec: WatchRecord) {
    if let Some(pos) = list.iter().position(|r| r.guid == rec.guid) {
        let old = list.remove(pos);
        list.push(merge_record(old, rec));
    } else {
        list.push(rec);
    }
    list.sort_by(|a, b| b.played_at.cmp(&a.played_at));
    if list.len() > MAX_ENTRIES {
        list.truncate(MAX_ENTRIES);
    }
}

/// 同一 guid 的两条记录合并：played_at 取新的；旧记录有而新记录缺的字段保留
/// （重放时拿到的元数据可能更少，比如起播时详情页尚未刮削完成）。
fn merge_record(old: WatchRecord, new: WatchRecord) -> WatchRecord {
    let keep = |n: String, o: String| if n.is_empty() { o } else { n };
    WatchRecord {
        guid: new.guid,
        title: keep(new.title, old.title),
        episode: keep(new.episode, old.episode),
        poster: keep(new.poster, old.poster),
        tmdb_id: keep(new.tmdb_id, old.tmdb_id),
        bgm_id: keep(new.bgm_id, old.bgm_id),
        media_type: keep(new.media_type, old.media_type),
        played_at: new.played_at.max(old.played_at),
        source: keep(new.source, old.source),
        source_name: keep(new.source_name, old.source_name),
        parent_guid: keep(new.parent_guid, old.parent_guid),
    }
}

/// 飞牛 play/list ∪ 本机记录 → 统一结构（camelCase，前端直接消费）。
/// 纯函数（不触网），单测覆盖去重/排序口径；「季→剧」富化见 [`enrich_series`]。
pub fn merge(fnos: Option<&Value>, local: Vec<WatchRecord>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // ① 本机记录先入（都带真实时间戳）
    for r in local {
        if r.guid.is_empty() {
            continue;
        }
        idx.insert(r.guid.clone(), out.len());
        out.push(record_to_json(&r));
    }

    // ② 飞牛条目并入（同 guid 按「更新的那条」合并）
    if let Some(v) = fnos {
        for it in fnos_items(v) {
            let m = fnos_to_json(it);
            let guid = m.get("guid").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if guid.is_empty() {
                continue;
            }
            match idx.get(&guid).copied() {
                Some(i) => merge_same_guid(&mut out[i], &m),
                None => {
                    idx.insert(guid, out.len());
                    out.push(m);
                }
            }
        }
    }

    // ③ 时间倒序；无时间戳的（played_at=0）保持插入序沉底（sort_by 是稳定排序）
    out.sort_by(|a, b| played_at_of(b).cmp(&played_at_of(a)));
    out
}

/// 兼容两种形态：data 直接是数组（2026-10-16 实测如此）；防御 {list:[...]} 包装。
fn fnos_items(v: &Value) -> &[Value] {
    if let Some(arr) = v.as_array() {
        return arr;
    }
    v.get("list")
        .and_then(|x| x.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

fn played_at_of(v: &Value) -> u64 {
    v.get("playedAt").and_then(|x| x.as_u64()).unwrap_or(0)
}

fn progress_of(v: &Value) -> f64 {
    v.get("progress").and_then(|x| x.as_f64()).unwrap_or(0.0)
}

fn record_to_json(r: &WatchRecord) -> Value {
    json!({
        "guid": r.guid.clone(),
        "title": r.title.clone(),
        "episode": r.episode.clone(),
        "poster": r.poster.clone(),
        "tmdbId": r.tmdb_id.clone(),
        "bgmId": r.bgm_id.clone(),
        "type": r.media_type.clone(),
        "playedAt": r.played_at,
        "source": r.source.clone(),
        "sourceName": r.source_name.clone(),
        "parentGuid": r.parent_guid.clone(),
        "seriesGuid": "",
        "progress": 0.0,
    })
}

/// 飞牛 play/list 条目 → 统一结构。字段实测为 snake_case（guid/tv_title/title/type/poster/
/// season_number/episode_number/ts/duration/watched/parent_guid…），读取时做 camelCase 兼容。
fn fnos_to_json(it: &Value) -> Value {
    let tv_title = str_of(it, &["tv_title", "tvTitle"]);
    let title = if tv_title.is_empty() {
        str_of(it, &["title"])
    } else {
        tv_title
    };
    let season = num_of(it, &["season_number", "seasonNumber"]) as i64;
    let episode = num_of(it, &["episode_number", "episodeNumber"]) as i64;
    let ts = num_of(it, &["ts"]);
    let duration = num_of(it, &["duration"]);
    let watched = num_of(it, &["watched"]) == 1.0;
    let mut progress = if duration > 0.0 {
        (ts / duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if watched {
        progress = 1.0;
    }
    let (tmdb_id, bgm_id) = parse_trim_ids(&str_of(it, &["trim_id", "trimId"]));
    json!({
        "guid": str_of(it, &["guid"]),
        "title": title,
        "episode": episode_label(season, episode),
        "poster": pick_img(it.get("poster")),
        "tmdbId": tmdb_id,
        "bgmId": bgm_id,
        "type": str_of(it, &["type"]),
        "playedAt": parse_played_at(it),
        "source": "fnos",
        "sourceName": "",
        "parentGuid": str_of(it, &["parent_guid", "parentGuid"]),
        "seriesGuid": "",
        "progress": progress,
    })
}

/// 同 guid 合并（「取更新的那条」+ 空字段互补）：
/// 时间戳大的胜出；本机记录一定带时间戳、飞牛条目通常为 0 → 本机胜，飞牛补进度/海报等。
fn merge_same_guid(existing: &mut Value, incoming: &Value) {
    let keep_incoming = played_at_of(incoming) > played_at_of(existing);
    let (mut keep, other) = if keep_incoming {
        (incoming.clone(), existing.clone())
    } else {
        (existing.clone(), incoming.clone())
    };
    for k in [
        "title", "episode", "poster", "tmdbId", "bgmId", "type", "sourceName", "parentGuid",
        "seriesGuid",
    ] {
        let cur = keep.get(k).and_then(|x| x.as_str()).unwrap_or("");
        if cur.is_empty() {
            if let Some(v) = other.get(k) {
                if v.as_str().map(|s| !s.is_empty()).unwrap_or(false) {
                    keep[k] = v.clone();
                }
            }
        }
    }
    // 进度取两者较大（飞牛侧为准；本机记录不记进度）
    keep["progress"] = json!(progress_of(&keep).max(progress_of(&other)));
    *existing = keep;
}

/// 剧集条目富化：用「季 guid」（parentGuid）换出「剧 guid」（seriesGuid）+ 竖版海报。
///
/// 为什么必须富化：play/info 与 play/list 给的都是**集**条目 —— 集海报是横版剧照
/// （实测 1000×563，当 2:3 卡片封面会被裁得很难看）、guid 是集 guid（点开详情需剧 guid
/// 才能出季/集列表）。季条目的 parent_guid 就是剧 guid、posters 是竖版（实测 2000×2825）。
///
/// 缓存优先（复用详情页同款 `detail:item:{guid}` 落盘缓存 120 天）：首次打开约 0.7s 拉完，
/// 之后零网络；网络失败/超限的条目保持原样（前端点击退化为集详情，不影响页面本身）。
pub async fn enrich_series(items: &mut [Value], server: &str, token: &str) {
    if token.is_empty() {
        return;
    }
    // 收集待富化项：剧集类 + 有季 guid + 尚未拿到剧 guid
    let mut targets: Vec<(usize, String)> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let series = it.get("seriesGuid").and_then(|x| x.as_str()).unwrap_or("");
        let parent = it.get("parentGuid").and_then(|x| x.as_str()).unwrap_or("");
        if !series.is_empty() || parent.is_empty() {
            continue;
        }
        let t = it.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if !(t.contains("Episode") || t == "TV" || t == "Season") {
            continue;
        }
        targets.push((i, parent.to_string()));
    }

    let mut budget = ENRICH_MAX_FETCHES;
    for chunk in targets.chunks(ENRICH_CONCURRENCY) {
        // 缓存命中先直接应用（零网络、零并发占用）
        let mut pending: Vec<(usize, String)> = Vec::new();
        for (i, sg) in chunk {
            let key = format!("detail:item:{}", sg);
            if let Some(v) = crate::disk_cache::json_get(&key, crate::DETAIL_TTL_SECS) {
                apply_enrich(&mut items[*i], &v);
            } else if budget > 0 {
                budget -= 1;
                pending.push((*i, sg.clone()));
            }
        }
        if pending.is_empty() {
            continue;
        }
        // 未命中的并发拉取（每块最多 ENRICH_CONCURRENCY 个）
        let mut set = tokio::task::JoinSet::new();
        for (i, sg) in pending {
            let server = server.to_string();
            let token = token.to_string();
            set.spawn(async move {
                let client = crate::fnapi::new_client();
                let r = client
                    .call(&server, &format!("/v/api/v1/item/{}", sg), "GET", &token, None)
                    .await;
                (i, sg, r)
            });
        }
        while let Some(joined) = set.join_next().await {
            if let Ok((i, sg, Ok(v))) = joined {
                crate::disk_cache::json_put(&format!("detail:item:{}", sg), &v);
                apply_enrich(&mut items[i], &v);
            }
        }
    }
}

/// 季条目 → 补到集条目上：剧 guid + 竖版海报（有才覆盖）+ 锚点 id 兜底。
fn apply_enrich(item: &mut Value, season: &Value) {
    let series = str_of(season, &["parent_guid", "parentGuid"]);
    if !series.is_empty() {
        item["seriesGuid"] = json!(series);
    }
    let mut poster = pick_img(season.get("posters"));
    if poster.is_empty() {
        poster = pick_img(season.get("poster"));
    }
    if !poster.is_empty() {
        item["poster"] = json!(poster);
    }
    let (tm, bg) = parse_trim_ids(&str_of(season, &["trim_id", "trimId"]));
    if !tm.is_empty() && item.get("tmdbId").and_then(|x| x.as_str()).unwrap_or("").is_empty() {
        item["tmdbId"] = json!(tm);
    }
    if !bg.is_empty() && item.get("bgmId").and_then(|x| x.as_str()).unwrap_or("").is_empty() {
        item["bgmId"] = json!(bg);
    }
}

/// 季集号 → 展示标签（"S2E1"；只有集号时"第5集"；都没有为空）。
pub fn episode_label(season: i64, episode: i64) -> String {
    if season > 0 && episode > 0 {
        format!("S{season}E{episode}")
    } else if episode > 0 {
        format!("第{episode}集")
    } else {
        String::new()
    }
}

/// 飞牛锚点 trim_id → (tmdb_id, bgm_id)：`tm12345` → TMDB、`bg12345` → Bangumi；
/// `tt*`（IMDb）与其它形态不落（本软件用 TMDB/BGM 定位，IMDb 由详情页链路自行解析）；
/// 纯数字按 TMDB 兜底（对齐前端 App.tsx 的同款口径）。
pub fn parse_trim_ids(trim: &str) -> (String, String) {
    let t = trim.trim();
    if t.is_empty() {
        return (String::new(), String::new());
    }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = t.strip_prefix("tm") {
        if digits(rest) {
            return (rest.to_string(), String::new());
        }
    }
    if let Some(rest) = t.strip_prefix("bg") {
        if digits(rest) {
            return (String::new(), rest.to_string());
        }
    }
    if digits(t) {
        return (t.to_string(), String::new());
    }
    (String::new(), String::new())
}

/// 飞牛条目 → 播放时间（ms）。实测 play/list 无时间戳字段（全部为 0），保留多键探测以兼容
/// 接口演进；**绝不含 watched_ts/ts**（那是播放进度秒数、不是时间戳 —— 主仓库 lc-1230 已定性）。
fn parse_played_at(item: &Value) -> u64 {
    for k in [
        "lastPlayed", "last_played", "lastPlayedAt", "last_played_at", "watchedAt", "watched_at",
        "updateTime", "update_time", "updatedAt", "updated_at",
    ] {
        let Some(v) = item.get(k) else { continue };
        let n = if let Some(n) = v.as_u64() {
            Some(n)
        } else if let Some(n) = v.as_f64() {
            Some(n as u64)
        } else {
            v.as_str().and_then(|s| s.parse::<u64>().ok())
        };
        let Some(n) = n else { continue };
        let ms = if n < 1_000_000_000_000 { n * 1000 } else { n };
        if ms >= MIN_VALID_MS {
            return ms;
        }
    }
    0
}

fn str_of(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            if !s.trim().is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn num_of(v: &Value, keys: &[&str]) -> f64 {
    for k in keys {
        if let Some(n) = v.get(*k).and_then(|x| x.as_f64()) {
            return n;
        }
    }
    0.0
}

/// 图片字段归一：字符串直接用；数组取第一个；对象取 file_path/url/path/image/src。
/// 口径对齐主仓库 pickImg —— 只取「相对路径或完整 URL」的原始字符串，不做拼 host（前端 imgUrl 归一）。
pub fn pick_img(v: Option<&Value>) -> String {
    let Some(v) = v else {
        return String::new();
    };
    let extract = |x: &Value| -> String {
        if let Some(s) = x.as_str() {
            return s.to_string();
        }
        for k in ["file_path", "url", "path", "image", "src"] {
            if let Some(s) = x.get(k).and_then(|y| y.as_str()) {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
        String::new()
    };
    if let Some(arr) = v.as_array() {
        return arr.first().map(extract).unwrap_or_default();
    }
    extract(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(guid: &str, played_at: u64) -> WatchRecord {
        WatchRecord {
            guid: guid.into(),
            title: format!("T-{guid}"),
            played_at,
            source: "fnos".into(),
            ..Default::default()
        }
    }

    #[test]
    fn episode_label_variants() {
        assert_eq!(episode_label(2, 1), "S2E1");
        assert_eq!(episode_label(0, 5), "第5集");
        assert_eq!(episode_label(0, 0), "");
    }

    #[test]
    fn trim_id_parse() {
        assert_eq!(parse_trim_ids("tm1315772"), ("1315772".to_string(), String::new()));
        assert_eq!(parse_trim_ids("bg123"), (String::new(), "123".to_string()));
        // tt*（IMDb）不落：本软件用 TMDB/BGM 定位
        assert_eq!(parse_trim_ids("tt206630"), (String::new(), String::new()));
        // 纯数字按 TMDB 兜底（对齐前端口径）
        assert_eq!(parse_trim_ids("12345"), ("12345".to_string(), String::new()));
        assert_eq!(parse_trim_ids(""), (String::new(), String::new()));
    }

    #[test]
    fn merge_dedupes_same_guid_keeps_timestamped_local() {
        // 飞牛条目无时间戳（实测）；本机记录有时间 → 保留本机，进度/集号从飞牛补齐
        let fnos = json!([{
            "guid": "g1", "tv_title": "剧名", "title": "集名", "type": "Episode",
            "ts": 50, "duration": 100, "season_number": 1, "episode_number": 2
        }]);
        let local = vec![WatchRecord {
            guid: "g1".into(),
            title: "剧名".into(),
            episode: "S1E2".into(),
            played_at: 1000,
            source: "fnos".into(),
            ..Default::default()
        }];
        let out = merge(Some(&fnos), local);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["playedAt"], json!(1000));
        assert!((out[0]["progress"].as_f64().unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(out[0]["episode"], json!("S1E2"));
    }

    #[test]
    fn merge_prefers_newer_fnos_when_timestamped() {
        // 飞牛条目带真实时间戳且更新 → 飞牛胜（兼容接口未来补时间戳的情况）
        let fnos = json!([{
            "guid": "g1", "tv_title": "飞牛标题", "type": "Movie", "lastPlayed": 1700000000u64
        }]);
        let local = vec![WatchRecord {
            guid: "g1".into(),
            title: "本机标题".into(),
            played_at: 1600000000000,
            source: "fnos".into(),
            ..Default::default()
        }];
        let out = merge(Some(&fnos), local);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["title"], json!("飞牛标题"));
        assert_eq!(out[0]["playedAt"], json!(1700000000000u64));
    }

    #[test]
    fn merge_sorts_desc_and_untimed_last() {
        // 有时间的倒序在前；无时间戳的保持 play/list 原始顺序沉底
        let fnos = json!([
            { "guid": "a", "title": "飞牛A", "type": "Movie" },
            { "guid": "b", "title": "飞牛B", "type": "Movie" }
        ]);
        let local = vec![rec("c", 300), rec("d", 200)];
        let out = merge(Some(&fnos), local);
        let guids: Vec<&str> = out.iter().map(|v| v["guid"].as_str().unwrap()).collect();
        assert_eq!(guids, vec!["c", "d", "a", "b"]);
    }

    #[test]
    fn merge_webrs_and_fnos_never_collide() {
        let fnos = json!([{ "guid": "webrs-shared", "title": "飞牛同名", "type": "Movie" }]);
        let local = vec![WatchRecord {
            guid: "webrs:https://example.com/ep1".into(),
            title: "在线源".into(),
            played_at: 500,
            source: "webrs".into(),
            ..Default::default()
        }];
        let out = merge(Some(&fnos), local);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["source"], json!("webrs"));
        assert_eq!(out[1]["source"], json!("fnos"));
    }

    #[test]
    fn upsert_updates_and_caps() {
        let mut list = Vec::new();
        for i in 0..(MAX_ENTRIES + 10) {
            upsert(&mut list, rec(&format!("g{i}"), i as u64 + 1));
        }
        assert_eq!(list.len(), MAX_ENTRIES);
        // 最旧的被裁掉
        assert!(!list.iter().any(|r| r.guid == "g0"));
        // 同 guid 更新：不新增、时间刷新、排到最前
        upsert(&mut list, rec("g500", 9_999_999));
        assert_eq!(list.iter().filter(|r| r.guid == "g500").count(), 1);
        assert_eq!(list[0].guid, "g500");
        // 重放时元数据变少（如海报为空）→ 旧值保留
        let mut w = WatchRecord {
            guid: "keep".into(),
            title: "标题".into(),
            poster: "/p1.webp".into(),
            played_at: 10,
            ..Default::default()
        };
        w.source = "fnos".into();
        let mut l2 = vec![w];
        upsert(
            &mut l2,
            WatchRecord {
                guid: "keep".into(),
                title: "标题".into(),
                played_at: 20,
                source: "fnos".into(),
                ..Default::default()
            },
        );
        assert_eq!(l2.len(), 1);
        assert_eq!(l2[0].poster, "/p1.webp");
        assert_eq!(l2[0].played_at, 20);
    }
}
