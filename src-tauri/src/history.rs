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
use std::collections::BTreeMap;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

/// [lc-1631] 容量上限：原为 500，但年度报告要能算「整年」——年初的记录会被挤掉，
/// 导致「1 月的观看」在 12 月时消失。这里两条一起上：
///   ① 上限抬到 [`MAX_ENTRIES`]（对齐原项目观看事件台账的 3000 上限）；
///   ② 另存**日级台账** `watch_days.json`（只记每天看了几部，永不裁剪）——
///      即便条目级记录滚动淘汰，报告仍能还原「哪些天在看 / 活跃天数 / 连续天数 / 月度节奏」。
/// 3000 条 JSON 约 1MB，读写仍是一次文件操作，可接受。
pub const MAX_ENTRIES: usize = 3000;

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
    // ── [lc-1631] 年度报告所需字段（全部 #[serde(default)]：旧 history.json 缺这些键也能读，向后兼容）──
    /// 条目**总时长**（ms；0 = 未知）。play/info 的 `duration`（秒）折算，落盘时即有。
    #[serde(default)]
    pub duration_ms: u64,
    /// 本次**实际观看时长**（ms；0 = 未知）。播放结束时由前端回填（见 [`backfill_session`]）——
    /// 起播那一刻拿不到，如实留 0，报告侧再按「进度 × 总时长」折算（原项目同口径）。
    #[serde(default)]
    pub watched_ms: u64,
    /// 观看进度 0..1（0 = 未知）。来自飞牛 play/list 的 ts/duration，或播放结束回填。
    #[serde(default)]
    pub progress: f64,
    /// 是否看完（进度达阈值，或飞牛 watched=1）。
    #[serde(default)]
    pub finished: bool,
}

/// [lc-1616] 「我的评分 + 评语」—— 观影记录详情浮层里用户自己的评价。
///
/// 为什么落在 Rust 侧（`app_data_dir/ratings.json`）而不是 localStorage：
///   ① 本项目是 Tauri 桌面端，localStorage 会随 WebView 清缓存/换端而丢；
///   ② 与 history.json 同目录，读写路径一致，后续做「评分随历史一起备份/同步」也顺。
/// 键口径与作品聚合一致：作品 guid（剧 guid / 电影自身 guid / 在线源 `webrs:<url>`）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct MyRating {
    /// 5 星评分（0 = 未评；写入时夹到 0..=5）
    #[serde(default)]
    pub score: u8,
    /// 评语（可为空）
    #[serde(default)]
    pub review: String,
    /// 最后修改时刻（ms；用于展示"已保存"排序/未来增量同步）
    #[serde(default)]
    pub updated_at: u64,
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

fn ratings_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("ratings.json"))
}

/// [lc-1631] 日级台账文件（与 history.json 同目录）。
fn days_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("watch_days.json"))
}

/// [lc-1631] 单日台账：只记「这天看了几部」，跨重启累积、**永不裁剪**。
/// 键 = `"Y-M-D"`（月份 **0 基**，与原项目 `fntv_wh_daycount_v1` 及前端热力图的
/// `getMonth()` 口径一致，避免 1 月/12 月差一位）。为什么不记时长：起播时拿不到真实
/// 观看时长，硬塞估算值会污染台账；时长在报告侧按记录/均值折算，台账只保「天数」这一
/// 永不丢失的事实。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DayStat {
    #[serde(default)]
    pub count: u32,
}

/// 本地日期 `"Y-M-D"`（月份 0 基）。record 恒在「此刻」调用，故直接用系统本地时间；
/// 非 Windows 退化为 UTC（本软件当前只发 Windows，对齐 feedback.rs 的做法）。
fn today_key() -> String {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut st: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut st) };
        // wMonth 是 1..12 → 减 1 得到 0 基，与前端一致
        format!("{}-{}-{}", st.wYear, st.wMonth as i32 - 1, st.wDay)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let secs = now_ms() / 1000;
        let days = (secs / 86400) as i64;
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let mo = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + if mo <= 2 { 1 } else { 0 };
        format!("{}-{}-{}", y, mo - 1, d)
    }
}

/// 读取日级台账（文件缺失/损坏 → 空表，绝不报错）。
pub fn load_days(app: &AppHandle) -> BTreeMap<String, DayStat> {
    let Some(p) = days_path(app) else {
        return Default::default();
    };
    match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str::<BTreeMap<String, DayStat>>(&s).unwrap_or_default(),
        Err(_) => Default::default(),
    }
}

fn save_days(app: &AppHandle, map: &BTreeMap<String, DayStat>) {
    let Some(p) = days_path(app) else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string(map) {
        let _ = std::fs::write(&p, s);
    }
}

/// 台账 +1（纯逻辑，供单测）：脏键（年份 < 2000）丢弃，避免报告/热力图从脏年份起算。
fn bump_day(map: &mut BTreeMap<String, DayStat>, key: &str) {
    let y = key
        .split('-')
        .next()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    if y < 2000 {
        return;
    }
    map.entry(key.to_string()).or_default().count += 1;
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

/// 读取全部评分（文件不存在/损坏 → 空表，绝不报错）。
pub fn load_ratings(app: &AppHandle) -> std::collections::HashMap<String, MyRating> {
    let Some(p) = ratings_path(app) else {
        return Default::default();
    };
    match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Default::default(),
    }
}

fn save_ratings(app: &AppHandle, map: &std::collections::HashMap<String, MyRating>) {
    let Some(p) = ratings_path(app) else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(&p, s);
    }
}

/// 取某作品的评分（无则 None）。
pub fn get_rating(app: &AppHandle, key: &str) -> Option<MyRating> {
    if key.is_empty() {
        return None;
    }
    load_ratings(app).remove(key)
}

/// 写某作品的评分：score 夹到 0..=5；score=0 且评语为空 → 视为「清除该条」。
/// 与 [`record`] 共用同一把锁：都是「读-改-写」，串行化避免并发互相覆盖。
pub fn set_rating(app: &AppHandle, key: &str, score: u8, review: String) {
    if key.is_empty() {
        return;
    }
    let _guard = LOCK.lock();
    let mut map = load_ratings(app);
    apply_rating(&mut map, key, score, &review);
    save_ratings(app, &map);
}

/// [`set_rating`] 的纯逻辑（供单测）：夹分 + 「空分空评语 = 删除」。
fn apply_rating(map: &mut std::collections::HashMap<String, MyRating>, key: &str, score: u8, review: &str) {
    let score = score.min(5);
    let review = review.trim();
    if score == 0 && review.is_empty() {
        map.remove(key);
    } else {
        map.insert(
            key.to_string(),
            MyRating {
                score,
                review: review.to_string(),
                updated_at: now_ms(),
            },
        );
    }
}

/// 落一条播放记录：同 guid 覆盖更新（保留旧记录里更全的字段），超上限裁最旧。
/// [lc-1631] 同时：① 日级台账 +1；② 广播 `history-recorded`（前端据此把「播放结束回填的
/// 真实观看时长」对位到这条记录 —— 见前端 WatchHistoryView 模块级会话计时）。
pub fn record(app: &AppHandle, rec: WatchRecord) {
    if rec.guid.is_empty() {
        return;
    }
    let _guard = LOCK.lock();
    let mut list = load(app);
    upsert(&mut list, rec.clone());
    save(app, &list);
    // 日级台账：只记「今天看了几部」，永不裁剪（报告整年可算的关键）。
    let mut days = load_days(app);
    bump_day(&mut days, &today_key());
    save_days(app, &days);
    // 通知前端「刚起播的是哪一条」：mpv 事件流不带 guid，前端靠这条事件知道该把时长回填给谁。
    let _ = app.emit("history-recorded", json!({ "guid": rec.guid, "playedAt": rec.played_at }));
}

/// [lc-1631] 播放结束回填：把前端统计到的真实观看时长/进度/看完状态补进对应记录。
/// 起播那一刻这些量都不存在（时长要播完才知道），所以只能事后回填；
/// 回填用 max/或 累积语义，避免重放/乱序把已有数据冲小。
pub fn backfill_session(app: &AppHandle, guid: &str, watched_ms: u64, progress: f64, finished: bool) {
    if guid.is_empty() {
        return;
    }
    let _guard = LOCK.lock();
    let mut list = load(app);
    if let Some(r) = list.iter_mut().find(|r| r.guid == guid) {
        r.watched_ms = r.watched_ms.max(watched_ms);
        r.progress = r.progress.max(progress.clamp(0.0, 1.0));
        r.finished = r.finished || finished;
        save(app, &list);
    }
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
/// [lc-1631] 年度报告字段：时长/进度/看完只增不减（新的那次可能还没播完 → 不能把
/// 上次看完整部的 watched_ms/finished 冲掉）。
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
        duration_ms: new.duration_ms.max(old.duration_ms),
        watched_ms: new.watched_ms.max(old.watched_ms),
        progress: new.progress.max(old.progress),
        finished: new.finished || old.finished,
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

/// [lc-1667] 观影记录可见口径（用户定稿）：
///   · **在线源（webrs）已搁置** → 列表 / 统计 / 年度报告一律不展示、不计算；
///   · **个人视频（type=Video）取不到封面（poster 为空）** → 默认不计算；
///     正规影视条目即便暂时缺海报仍保留，避免误伤刮削失败的内容。
pub fn visible_in_history(media_type: &str, source: &str, poster: &str) -> bool {
    if source == "webrs" || media_type == "webrs" {
        return false;
    }
    !(matches!(media_type, "Video" | "Directory") && poster.trim().is_empty())
}

/// [lc-1616] 作品级聚合：把「集」条目按作品归并成一张卡（纯函数，单测覆盖）。
///
/// 为什么：play/list 与本机历史都是**集**级 —— 同一部剧看 8 集就出 8 张卡；用户要「按部算」，
/// 一张卡 = 一部作品。
///
/// 作品键口径：优先用 [`enrich_series`] 补出的**剧 guid**（`seriesGuid`）；电影/在线源没有
/// `seriesGuid`，退化为条目自身 guid（在线源是 `webrs:<url>` 前缀键，天然不与飞牛 guid 相撞）。
/// ⚠ 富化失败（网络超限/无 token）的剧集条目 `seriesGuid` 为空 → 会退化成「每集一张卡」，
/// 这是**降级**而非错误（页面本身仍可用）；重新打开/有网后富化成功即自动并回一张。
///
/// 每部作品保留的字段：
///   · 海报/标题/锚点 id（tmdbId/bgmId）/来源名：取组内**首个非空**（海报已由富化换成竖版剧海报）；
///   · `playedAt` = 组内 max（最近观看时间）；
///   · `progress`/`episode` = **最近一次**那条（比 max 更准：看完第1集(1.0)再看第2集(0.2)
///     应显示"看到第2集 · 20%"而不是 1.0；据此判「已看完/在观看」也符合直觉）；
///   · `episodeCount` = 组内**不同集号标签**数（"看了几集"；电影/无集号为 0）；
///   · `sessions` = 组内全部条目按时间倒序（详情浮层的「播放记录」：时间 + 进度）。
pub fn aggregate_by_series(items: &[Value]) -> Vec<Value> {
    // 先按作品键分组（保持首次出现顺序，最后统一按时间倒序）
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<Value>> = std::collections::HashMap::new();
    for it in items {
        let series = it.get("seriesGuid").and_then(|x| x.as_str()).unwrap_or("");
        let guid = it.get("guid").and_then(|x| x.as_str()).unwrap_or("");
        let key = if !series.is_empty() { series } else { guid };
        if key.is_empty() {
            continue;
        }
        if !groups.contains_key(key) {
            order.push(key.to_string());
        }
        groups.entry(key.to_string()).or_default().push(it.clone());
    }

    let mut out: Vec<Value> = Vec::new();
    for key in order {
        let Some(mut g) = groups.remove(&key) else { continue };
        // 组内按时间倒序 → g[0] = 最近一次
        g.sort_by(|a, b| played_at_of(b).cmp(&played_at_of(a)));
        let first_nonempty = |k: &str| -> String {
            g.iter()
                .find_map(|v| v.get(k).and_then(|x| x.as_str()).filter(|s| !s.is_empty()))
                .unwrap_or("")
                .to_string()
        };
        // 类型：任一条目是 Movie 即电影（剧集条目 type 为 Episode/TV/Season）
        let is_movie = g
            .iter()
            .any(|v| v.get("type").and_then(|x| x.as_str()) == Some("Movie"));
        let media_type = if is_movie {
            "Movie".to_string()
        } else {
            first_nonempty("type")
        };
        let source = g
            .iter()
            .find_map(|v| v.get("source").and_then(|x| x.as_str()).filter(|s| !s.is_empty()))
            .unwrap_or("fnos")
            .to_string();
        // 不同集号标签数（"看了几集"）
        let mut eps: Vec<String> = g
            .iter()
            .filter_map(|v| {
                v.get("episode")
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
            })
            .collect();
        eps.sort();
        eps.dedup();
        let latest = &g[0];
        let sessions: Vec<Value> = g
            .iter()
            .map(|v| {
                json!({
                    "playedAt": played_at_of(v),
                    "episode": v.get("episode").and_then(|x| x.as_str()).unwrap_or(""),
                    "progress": progress_of(v),
                })
            })
            .collect();
        out.push(json!({
            "guid": key.clone(),
            "seriesGuid": key,
            "title": first_nonempty("title"),
            "episode": latest.get("episode").and_then(|x| x.as_str()).unwrap_or(""),
            "episodeCount": eps.len(),
            "poster": first_nonempty("poster"),
            "tmdbId": first_nonempty("tmdbId"),
            "bgmId": first_nonempty("bgmId"),
            "type": media_type,
            "playedAt": g.iter().map(played_at_of).max().unwrap_or(0),
            "source": source,
            "sourceName": first_nonempty("sourceName"),
            "progress": progress_of(latest),
            // [lc-1631] 作品级时长（组内最大，通常每集相同）/ 是否有任一条目看完
            "durationMs": g.iter()
                .map(|v| v.get("durationMs").and_then(|x| x.as_u64()).unwrap_or(0))
                .max()
                .unwrap_or(0),
            "finished": g.iter().any(|v| v.get("finished").and_then(|x| x.as_bool()).unwrap_or(false)),
            "sessions": sessions,
        }));
    }
    // 作品间也按最近观看时间倒序（无时间的沉底，稳定排序保持插入序）
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
        // [lc-1631] 本机记录也能带进度/时长/看完：播放结束回填后，卡片/统计/报告都用得到
        "progress": r.progress.clamp(0.0, 1.0),
        "durationMs": r.duration_ms,
        "watchedMs": r.watched_ms,
        "finished": r.finished,
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
        // [lc-1631] 飞牛条目自带总时长（duration 秒）与 watched 标记 —— 报告按
        // 「进度 × 总时长」折算观看量时要用，别丢。
        "durationMs": if duration > 0.0 { (duration * 1000.0) as u64 } else { 0 },
        "watchedMs": 0,
        "finished": watched,
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
    // [lc-1631] 总时长取较大（一方为 0 时用另一方的真实值）；看完状态取「或」
    let dur = |v: &Value| v.get("durationMs").and_then(|x| x.as_u64()).unwrap_or(0);
    keep["durationMs"] = json!(dur(&keep).max(dur(&other)));
    let fin = |v: &Value| v.get("finished").and_then(|x| x.as_bool()).unwrap_or(false);
    keep["finished"] = json!(fin(&keep) || fin(&other));
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

/// [lc-1631] 年度报告数据源：本机播放记录（事件级，带真实墙钟时间）+ 日级台账。
/// **不触网**（报告要秒开；飞牛侧的作品级事实由前端复用 `get_watch_history` 的 items）。
/// 前端据 `years` 渲染年份下拉；`events` 算月度节奏/观影时刻/连续天数/TOP；
/// `days` 在记录被裁掉时兜底「活跃天数/月度节奏」。
pub fn report_data(app: &AppHandle) -> Value {
    let recs = load(app);
    let days = load_days(app);
    let mut years: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    // 当前年份（本地）：即便今年还没看过，下拉里也要能选到
    if let Some(y) = today_key().split('-').next().and_then(|s| s.parse::<u32>().ok()) {
        years.insert(y);
    }
    let events: Vec<Value> = recs
        .iter()
        // [lc-1667] 与观影记录同口径：在线源已搁置、无封面个人视频不计算（年度报告也不含）
        .filter(|r| {
            r.played_at >= MIN_VALID_MS
                && visible_in_history(&r.media_type, &r.source, &r.poster)
        })
        .map(|r| {
            json!({
                "ts": r.played_at,
                "guid": r.guid,
                "title": r.title,
                "type": r.media_type,
                "source": r.source,
                "ms": r.watched_ms,
                "durationMs": r.duration_ms,
                "progress": r.progress.clamp(0.0, 1.0),
                "finished": r.finished,
            })
        })
        .collect();
    let mut day_list: Vec<Value> = Vec::new();
    for (k, st) in &days {
        let parts: Vec<&str> = k.split('-').collect();
        if parts.len() != 3 {
            continue;
        }
        let (Ok(y), Ok(m0), Ok(d)) = (
            parts[0].parse::<i32>(),
            parts[1].parse::<i32>(),
            parts[2].parse::<i32>(),
        ) else {
            continue;
        };
        if y < 2000 || st.count == 0 {
            continue;
        }
        years.insert(y as u32);
        day_list.push(json!({ "y": y, "m0": m0, "d": d, "count": st.count }));
    }
    json!({
        "years": years.into_iter().collect::<Vec<u32>>(),
        "events": events,
        "days": day_list,
    })
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
    fn history_visibility_policy() {
        // 在线源已搁置：无论有没有海报都不展示
        assert!(!visible_in_history("webrs", "webrs", ""));
        assert!(!visible_in_history("Movie", "webrs", "/p.webp"));
        // 个人视频：取不到封面 → 不计算；有封面则保留
        assert!(!visible_in_history("Video", "fnos", ""));
        assert!(!visible_in_history("Directory", "fnos", "  "));
        assert!(visible_in_history("Video", "fnos", "/p.webp"));
        // 正规影视条目缺海报（刮削失败）仍保留，避免误伤
        assert!(visible_in_history("Movie", "fnos", ""));
        assert!(visible_in_history("Episode", "fnos", ""));
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

    #[test]
    fn aggregate_groups_episodes_by_series() {
        // 同一部剧的 3 集 → 1 张卡；电影各自成卡
        let items = vec![
            json!({"guid":"e3","seriesGuid":"S","title":"剧名","episode":"S1E3","poster":"/s.webp",
                   "type":"Episode","playedAt":300,"progress":0.2,"source":"fnos","tmdbId":"9"}),
            json!({"guid":"e1","seriesGuid":"S","title":"剧名","episode":"S1E1","poster":"",
                   "type":"Episode","playedAt":100,"progress":1.0,"source":"fnos"}),
            json!({"guid":"e2","seriesGuid":"S","title":"剧名","episode":"S1E2",
                   "type":"Episode","playedAt":200,"progress":1.0,"source":"fnos"}),
            json!({"guid":"m1","title":"电影","type":"Movie","playedAt":50,"progress":0.5,"source":"fnos"}),
        ];
        let out = aggregate_by_series(&items);
        assert_eq!(out.len(), 2); // 剧(并 3 集) + 电影
        // 最近观看的剧排在前
        assert_eq!(out[0]["guid"], json!("S"));
        assert_eq!(out[0]["episode"], json!("S1E3")); // 最近一集
        assert_eq!(out[0]["episodeCount"], json!(3)); // 看了 3 集
        assert!((out[0]["progress"].as_f64().unwrap() - 0.2).abs() < 1e-9); // 最近一次进度（不是 max）
        assert_eq!(out[0]["playedAt"], json!(300)); // 取 max
        assert_eq!(out[0]["poster"], json!("/s.webp")); // 首个非空海报
        assert_eq!(out[0]["sessions"].as_array().unwrap().len(), 3); // 播放记录 = 全部集
        // 电影：自身成卡、无集号
        assert_eq!(out[1]["guid"], json!("m1"));
        assert_eq!(out[1]["type"], json!("Movie"));
        assert_eq!(out[1]["episodeCount"], json!(0));
    }

    #[test]
    fn aggregate_keeps_untagged_items_separate() {
        // 无 seriesGuid 的条目（在线源集页 URL 各不相同）→ 各自成卡，且不与飞牛 guid 相撞
        let items = vec![
            json!({"guid":"webrs:https://x/ep1","title":"在线","type":"webrs","source":"webrs","playedAt":10}),
            json!({"guid":"webrs:https://x/ep2","title":"在线","type":"webrs","source":"webrs","playedAt":20}),
        ];
        let out = aggregate_by_series(&items);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["source"], json!("webrs"));
        assert_eq!(out[1]["source"], json!("webrs"));
    }

    #[test]
    fn rating_apply_clamps_and_clears() {
        let mut map: std::collections::HashMap<String, MyRating> = Default::default();
        // 夹到 5
        apply_rating(&mut map, "k", 9, "很棒");
        assert_eq!(map["k"].score, 5);
        assert_eq!(map["k"].review, "很棒");
        // 评语去首尾空白
        apply_rating(&mut map, "k", 3, "  还行  ");
        assert_eq!(map["k"].score, 3);
        assert_eq!(map["k"].review, "还行");
        // 空分 + 空评语 → 删除
        apply_rating(&mut map, "k", 0, "   ");
        assert!(!map.contains_key("k"));
    }

    // ── [lc-1631] 年度报告数据完整性 ──

    #[test]
    fn old_history_json_without_report_fields_still_parses() {
        // 旧 history.json（没有 duration_ms/watched_ms/progress/finished）必须能读，缺字段给默认值
        let old = r#"[{"guid":"g1","title":"旧记录","played_at":1700000000000,"source":"fnos"}]"#;
        let list: Vec<WatchRecord> = serde_json::from_str(old).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "旧记录");
        assert_eq!(list[0].duration_ms, 0);
        assert_eq!(list[0].watched_ms, 0);
        assert_eq!(list[0].progress, 0.0);
        assert!(!list[0].finished);
        // 再序列化回 JSON 不丢新键（未来可继续演进）
        let round: Vec<WatchRecord> =
            serde_json::from_str(&serde_json::to_string(&list).unwrap()).unwrap();
        assert_eq!(round[0].guid, "g1");
    }

    #[test]
    fn merge_record_keeps_report_fields_monotonic() {
        // 新记录（重放、还没播完）不能把旧记录已有的真实时长/看完状态冲小
        let old = WatchRecord {
            guid: "g".into(),
            watched_ms: 3_600_000,
            duration_ms: 3_600_000,
            progress: 1.0,
            finished: true,
            played_at: 100,
            ..Default::default()
        };
        let new = WatchRecord {
            guid: "g".into(),
            played_at: 200,
            ..Default::default()
        };
        let m = merge_record(old, new);
        assert_eq!(m.watched_ms, 3_600_000);
        assert_eq!(m.duration_ms, 3_600_000);
        assert_eq!(m.progress, 1.0);
        assert!(m.finished);
        assert_eq!(m.played_at, 200);
    }

    #[test]
    fn day_ledger_accumulates_and_drops_dirty() {
        let mut m: BTreeMap<String, DayStat> = Default::default();
        bump_day(&mut m, "2026-0-5"); // 2026 年 1 月 5 日（0 基月）
        bump_day(&mut m, "2026-0-5");
        bump_day(&mut m, "2026-11-31");
        bump_day(&mut m, "1970-0-1"); // 脏键：年份 < 2000 → 丢弃
        assert_eq!(m.get("2026-0-5").unwrap().count, 2);
        assert_eq!(m.get("2026-11-31").unwrap().count, 1);
        assert!(!m.contains_key("1970-0-1"));
    }

    #[test]
    fn fnos_duration_and_watched_map_to_report_fields() {
        // 飞牛条目：duration(秒) → durationMs；watched=1 → finished
        let it = json!({ "guid":"g1","title":"片","type":"Movie","ts":600,"duration":1200,"watched":1 });
        let j = fnos_to_json(&it);
        assert_eq!(j["durationMs"], json!(1_200_000u64));
        assert_eq!(j["finished"], json!(true));
        assert!((j["progress"].as_f64().unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn max_entries_covers_a_full_year() {
        // 上限必须足够容纳一整年（原 500 会把年初挤掉 → 报告算不出整年）
        assert!(MAX_ENTRIES >= 3000);
    }
}
