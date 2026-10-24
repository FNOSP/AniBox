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
use std::collections::{BTreeMap, BTreeSet};
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
    /// 播放条目 guid（库内 = 集/电影 guid；在线源 = `webrs:<集页面 URL>`）。
    /// ⚠ 这不是去重键：仅用于「播放结束回填」对位与打开详情。去重见 [`series_key_of`]。
    pub guid: String,
    /// [lc-1710] **剧级去重键**：同一部剧/番剧/电影只保留一条记录。
    /// 旧 history.json 没有该键（空串）→ 读取时由 [`series_key_of`] 现算并归一化，
    /// 向后兼容、无需迁移脚本。
    #[serde(default)]
    pub series_key: String,
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

/// [lc-1710] **剧级稳定键**：同一部剧/番剧（跨集、跨季、重复观看）只算一次。
///
/// 优先级（前缀隔离，永不互撞）：
///   1. `bgm:<id>` / `tmdb:<id>` —— 作品级锚点，最稳定（跨季也同一部剧）；
///   2. `parent:<季 guid>` —— 剧集各集共用（play/info 的 parent_guid）；仅当它确实
///      不同于条目自身 guid 时才用（否则是「没有父级」的退化值，会变成按集去重）；
///   3. `title:<标题>` —— 番剧名兜底（在线源没有季 guid，只能按标题归并）；
///   4. `guid:<条目 guid>` —— 电影/个人视频等「自身即作品」的兜底。
///
/// 为什么不用条目 guid：它是**集**级，8 集就会出 8 条记录（旧实现的缺陷）。
pub fn series_key_of(r: &WatchRecord) -> String {
    if !r.series_key.trim().is_empty() {
        return r.series_key.trim().to_string();
    }
    series_key_from_parts(
        &r.guid,
        &r.parent_guid,
        &r.media_type,
        &r.tmdb_id,
        &r.bgm_id,
        &r.title,
    )
}

/// [`series_key_of`] 的字段级版本：**同一套规则**，供非历史记录场景复用
/// （例如 `lib.rs::play_item` 给 mpv 倍速持久化算剧级键，见 [`crate::playback_speed`]）。
pub fn series_key_from_parts(
    guid: &str,
    parent_guid: &str,
    media_type: &str,
    tmdb_id: &str,
    bgm_id: &str,
    title: &str,
) -> String {
    let bgm = bgm_id.trim();
    if !bgm.is_empty() {
        return format!("bgm:{bgm}");
    }
    let tmdb = tmdb_id.trim();
    if !tmdb.is_empty() {
        return format!("tmdb:{tmdb}");
    }
    let parent = parent_guid.trim();
    let guid = guid.trim();
    // 剧集类（Episode/TV/Season）与在线源：按「剧」聚合，而不是按集。
    let episodic = media_type.contains("Episode")
        || matches!(media_type, "TV" | "Season" | "webrs");
    if episodic {
        if !parent.is_empty() && parent != guid {
            return format!("parent:{parent}");
        }
        if !title.trim().is_empty() {
            return format!("title:{}", title.trim());
        }
    }
    if !guid.is_empty() {
        return format!("guid:{guid}");
    }
    if !parent.is_empty() {
        return format!("parent:{parent}");
    }
    if !title.trim().is_empty() {
        return format!("title:{}", title.trim());
    }
    String::new()
}

/// [lc-1710] 读取期归一化：补齐 `series_key` 并**按剧折叠**旧数据。
///
/// 旧 `history.json` 是「一条 guid（集）= 一条记录」，同一部剧看多集就有多条。
/// 这里按 [`series_key_of`] 分组、用 [`merge_record`] 合并（新者胜、旧字段互补），
/// 再按播放时间倒序 + 容量上限裁剪。纯函数、幂等，可在每次 `load` 安全调用；
/// 真正的落盘发生在下一次 `record`（读-改-写）。
pub fn normalize_records(list: Vec<WatchRecord>) -> Vec<WatchRecord> {
    // 先按时间升序处理，保证「后处理的更晚」→ merge_record 的 new 侧是更新的那条。
    let mut sorted: Vec<WatchRecord> = list
        .into_iter()
        .map(|mut r| {
            if r.series_key.trim().is_empty() {
                r.series_key = series_key_of(&r);
            }
            r
        })
        .collect();
    sorted.sort_by_key(|r| r.played_at);
    let mut out: Vec<WatchRecord> = Vec::new();
    let mut idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in sorted {
        let key = r.series_key.clone();
        match idx.get(&key).copied() {
            Some(i) => {
                let old = std::mem::take(&mut out[i]);
                out[i] = merge_record(old, r);
            }
            None => {
                idx.insert(key, out.len());
                out.push(r);
            }
        }
    }
    out.sort_by(|a, b| b.played_at.cmp(&a.played_at));
    if out.len() > MAX_ENTRIES {
        out.truncate(MAX_ENTRIES);
    }
    out
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
    /// [lc-1668] 热力图口径：当天播放过的**作品去重数**（同一部剧一天只算一次）。
    #[serde(default)]
    pub heat: u32,
    /// [lc-1668] 是否已按新口径记录过（旧 watch_days.json 没有该字段 → false，
    /// 热力图对旧数据回退用 count，避免历史天直接变空）。
    #[serde(default)]
    pub tracked: bool,
    /// [lc-1668] 当天已计过的作品键（标题），用于同一天去重；只存标题，不存路径。
    #[serde(default)]
    pub series: BTreeSet<String>,
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
/// [lc-1668] 同时维护热力图口径：
///   · `count` = 当天**去重作品数**（[lc-1710] 与 heat 同口径：同一部剧一天重复观看不重复计数）；
///   · `heat` = 当天**去重作品数**（同一部剧一天只算一次，剧级键作为作品键）；
///   · `count_for_heat=false`（在线源已搁置）不计入热力图，但仍保留一条计数痕迹。
fn bump_day(map: &mut BTreeMap<String, DayStat>, key: &str, series_key: &str, count_for_heat: bool) {
    let y = key
        .split('-')
        .next()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    if y < 2000 {
        return;
    }
    let st = map.entry(key.to_string()).or_default();
    // 旧台账迁移：没有 tracked 标记时，先把已有 count 作为热力图基数（历史无法回溯去重）
    if !st.tracked {
        st.heat = st.heat.max(st.count);
    }
    st.tracked = true;
    if count_for_heat {
        let k = series_key.trim();
        if k.is_empty() {
            st.count += 1;
        } else if st.series.insert(k.to_string()) {
            // 同一部剧当天首次出现 → count/heat 各 +1；重复观看不再计数。
            st.count += 1;
            st.heat += 1;
        }
    } else {
        st.count += 1;
    }
}

/// [lc-1668] 热力图数据：日级台账 → `[{y, m0, d, count}]`。
/// `count` = 当天**去重作品数**（同一部剧一天只算一次）；旧台账没有 `tracked` 标记时
/// 回退用原始播放次数，避免历史天直接变空（无法回溯去重，但至少保留活动痕迹）。
pub fn heat_days(map: &BTreeMap<String, DayStat>) -> Vec<Value> {
    let mut out = Vec::new();
    for (k, st) in map {
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
        if y < 2000 {
            continue;
        }
        let count = if st.tracked { st.heat } else { st.count };
        if count == 0 {
            continue;
        }
        out.push(json!({ "y": y, "m0": m0, "d": d, "count": count }));
    }
    out
}

/// [lc-1669] 毫秒时间戳 → 本地日期 `"Y-M-D"`（月份 0 基）。
/// 用于把旧 `history.json` 回填到日级台账；Windows 走系统本地时区（含夏令时），
/// 其他平台退化为 UTC（本软件当前只发 Windows）。
#[cfg(target_os = "windows")]
fn local_day_key(ms: u64) -> Option<String> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME = 100ns 间隔，起点 1601-01-01；先加 Unix epoch 差值再换算。
    let ft_val = (ms as u128 + 11_644_473_600_000u128) * 10_000u128;
    let filetime = FILETIME {
        dwLowDateTime: (ft_val & 0xFFFF_FFFF) as u32,
        dwHighDateTime: (ft_val >> 32) as u32,
    };
    let mut utc_st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    if unsafe { FileTimeToSystemTime(&filetime, &mut utc_st) } == 0 {
        return None;
    }
    let mut local_st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    if unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc_st, &mut local_st) } == 0 {
        return None;
    }
    Some(format!(
        "{}-{}-{}",
        local_st.wYear,
        local_st.wMonth as i32 - 1,
        local_st.wDay
    ))
}

#[cfg(not(target_os = "windows"))]
fn local_day_key(ms: u64) -> Option<String> {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if mo <= 2 { 1 } else { 0 };
    Some(format!("{}-{}-{}", y, mo - 1, d))
}

/// [lc-1669] 把旧 `history.json` 回填到日级台账（幂等，页面每次加载都可安全调用）。
/// 背景：热力图在 lc-1668 之前用「作品清单」倒推，老用户只有 history.json / 旧 watch_days；
/// 升级后把已有播放记录补成「按天 + 按作品去重」的台账，历史天才有数据。
pub fn migrate_days_from_history(app: &AppHandle, records: &[WatchRecord]) {
    if records.is_empty() {
        return;
    }
    // 按天聚合：count = 记录条数（history.json 一条 guid 只留最近一次，无法还原真实次数），
    // heat = 去重作品数；在线源已搁置，不参与。
    let mut rebuilt: BTreeMap<String, (u32, BTreeSet<String>)> = BTreeMap::new();
    for r in records {
        if r.played_at < MIN_VALID_MS || r.source == "webrs" {
            continue;
        }
        let Some(key) = local_day_key(r.played_at) else {
            continue;
        };
        let series = if r.title.trim().is_empty() {
            r.guid.clone()
        } else {
            r.title.clone()
        };
        let e = rebuilt.entry(key).or_default();
        e.0 += 1;
        e.1.insert(series);
    }
    if rebuilt.is_empty() {
        return;
    }
    let _guard = LOCK.lock();
    let mut days = load_days(app);
    if merge_rebuilt_days(rebuilt, &mut days) {
        save_days(app, &days);
    }
}

/// 纯逻辑：把回填数据并入台账。已有 `tracked` 的天保留（新口径数据更准），
/// 旧格式天用回填值补上（count 取较大者，heat/series 来自历史记录）。
fn merge_rebuilt_days(
    rebuilt: BTreeMap<String, (u32, BTreeSet<String>)>,
    days: &mut BTreeMap<String, DayStat>,
) -> bool {
    let mut changed = false;
    for (key, (count, series)) in rebuilt {
        let st = days.entry(key).or_default();
        if st.tracked {
            continue;
        }
        st.count = st.count.max(count);
        st.heat = series.len() as u32;
        st.series = series;
        st.tracked = true;
        changed = true;
    }
    changed
}

/// [lc-1670] 旧 Electron 版热力图台账迁移。
/// 旧版把 `fntv_wh_daycount_v1`（形如 `{"2026-7-31":6}`，月份 0 基）存在
/// `%APPDATA%/fntv/Partitions/*/Local Storage/leveldb`；新版 localStorage 不同源读不到，
/// 这里直接扫描旧 LevelDB 的 .log/.ldb 原始字节提取该键（一次/进程，幂等）。
#[cfg(target_os = "windows")]
pub fn migrate_legacy_electron_days(app: &AppHandle) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let Some(appdata) = std::env::var_os("APPDATA") else {
            return;
        };
        // 旧 Electron 版的 userData 目录名可能是 fntv / Fntv-Plus 等：
        // 扫描 %APPDATA% 下所有名字含 fntv 的目录，取其 Local Storage / Partitions。
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(PathBuf::from(appdata)) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if !name.contains("fntv") || !e.path().is_dir() {
                    continue;
                }
                let base = e.path();
                dirs.push(base.join("Local Storage").join("leveldb"));
                if let Ok(parts) = std::fs::read_dir(base.join("Partitions")) {
                    for p in parts.flatten() {
                        let p = p.path().join("Local Storage").join("leveldb");
                        if p.is_dir() {
                            dirs.push(p);
                        }
                    }
                }
            }
        }
        let mut legacy: BTreeMap<String, u32> = BTreeMap::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if ext != "log" && ext != "ldb" {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&path) {
                    extract_legacy_daycounts(&bytes, &mut legacy);
                }
            }
        }
        if legacy.is_empty() {
            return;
        }
        let _guard = LOCK.lock();
        let mut days = load_days(app);
        let mut changed = false;
        for (key, count) in legacy {
            if !valid_day_key(&key) {
                continue;
            }
            let st = days.entry(key).or_default();
            // 新版已按「作品去重」记过的天不覆盖（旧数据只有次数，粒度更粗）
            if st.tracked {
                continue;
            }
            if count > st.count {
                st.count = count;
                changed = true;
            }
        }
        if changed {
            save_days(app, &days);
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub fn migrate_legacy_electron_days(_app: &AppHandle) {}

/// 从 LevelDB 原始字节里提取旧键 `fntv_wh_daycount_v1` 的 JSON 值（UTF-16LE / UTF-8 都试）。
fn extract_legacy_daycounts(bytes: &[u8], out: &mut BTreeMap<String, u32>) {
    const KEY: &str = "fntv_wh_daycount_v1";
    // UTF-16LE 解码（Chromium localStorage 字符串按 UTF-16 存）
    let utf16: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let text16 = String::from_utf16_lossy(&utf16);
    extract_json_after_key(&text16, KEY, out);
    // 兜底：部分旧版本/工具写入的是 UTF-8
    if let Ok(text8) = std::str::from_utf8(bytes) {
        extract_json_after_key(text8, KEY, out);
    }
}

/// 在文本里找 `key`，取其后第一个平衡的 `{...}` 解析成 `日期 → 次数`，同键取较大值。
fn extract_json_after_key(text: &str, key: &str, out: &mut BTreeMap<String, u32>) {
    let mut from = 0usize;
    while let Some(pos) = text[from..].find(key) {
        let after = from + pos + key.len();
        let window = &text[after..];
        if let Some(rel) = window.find('{') {
            // 键和值之间可能有少量二进制字符；只在前 200 个字符内找 JSON 起点
            if rel <= 200 {
                if let Some(json) = balanced_json(&window[rel..]) {
                    if let Ok(map) = serde_json::from_str::<BTreeMap<String, u32>>(json) {
                        for (k, v) in map {
                            out.entry(k).and_modify(|x| *x = (*x).max(v)).or_insert(v);
                        }
                    }
                }
            }
        }
        from = after;
    }
}

/// 取字符串开头第一个平衡的 `{...}`（正确处理 JSON 字符串内的花括号/转义）。
fn balanced_json(s: &str) -> Option<&str> {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&s[..=i]);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// 合法日期键（`Y-M-D`，月份 0 基）：挡掉 localStorage 里的脏数据。
fn valid_day_key(k: &str) -> bool {
    let parts: Vec<&str> = k.split('-').collect();
    if parts.len() != 3 {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (
        parts[0].parse::<i32>(),
        parts[1].parse::<i32>(),
        parts[2].parse::<i32>(),
    ) else {
        return false;
    };
    y >= 2000 && (0..=11).contains(&m) && (1..=31).contains(&d)
}

/// 读写串行化：record 是「读-改-写」，两条播放命令几乎同时触发时不能互相覆盖。
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 读取本机历史（文件不存在/损坏 → 空列表，绝不报错——观影记录页可以只显示飞牛部分）。
pub fn load(app: &AppHandle) -> Vec<WatchRecord> {
    let Some(p) = file_path(app) else {
        return Vec::new();
    };
    match std::fs::read_to_string(&p) {
        Ok(s) => normalize_records(serde_json::from_str::<Vec<WatchRecord>>(&s).unwrap_or_default()),
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
    // [lc-1710] 去重粒度 = 剧：同一部剧重复起播只更新、不新增（见 upsert / series_key_of）。
    let series_key = series_key_of(&rec);
    upsert(&mut list, rec.clone());
    save(app, &list);
    // 日级台账：只记「今天看了几部」，永不裁剪（报告整年可算的关键）。
    // [lc-1668] 同时记热力图去重键：作品键 = 剧级键（同一部剧各集/重看共用）。
    let mut days = load_days(app);
    bump_day(&mut days, &today_key(), &series_key, rec.source != "webrs");
    save_days(app, &days);
    // 通知前端「刚起播的是哪一条」：mpv 事件流不带 guid，前端靠这条事件知道该把时长回填给谁。
    // [progress] 一并带上 source / seriesKey：前端据此决定要不要回传飞牛（webrs 不回传）。
    let _ = app.emit(
        "history-recorded",
        json!({
            "guid": rec.guid,
            "playedAt": rec.played_at,
            "source": rec.source,
            "seriesKey": series_key,
        }),
    );
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

/// [progress] 进度上报纯逻辑（供单测）：把「当前位置 / 总时长」合并进一条记录。
///
/// 语义（与 [`backfill_session`] 一致，只增不减，避免 seek/乱序把数据冲小）：
///   · `duration_ms`：仅在拿到更大值时补齐（起播时可能未知）；
///   · `watched_ms`：取 `max(旧, position)` —— 位置是「实际看到哪」的下界；
///   · `progress`：`position / duration` 夹到 0..1，取较大值；
///   · `finished`：进度 ≥ 98% 时置真（与前端 92% 阈值口径独立，后端只做兜底）。
///
/// 返回是否有字段变化（调用方据此决定要不要落盘）。
fn apply_progress(r: &mut WatchRecord, position_ms: u64, duration_ms: u64) -> bool {
    let mut changed = false;
    if duration_ms > r.duration_ms {
        r.duration_ms = duration_ms;
        changed = true;
    }
    if position_ms > r.watched_ms {
        r.watched_ms = position_ms;
        changed = true;
    }
    let dur = if duration_ms > 0 {
        duration_ms
    } else {
        r.duration_ms
    };
    if dur > 0 {
        let p = (position_ms as f64 / dur as f64).clamp(0.0, 1.0);
        if p > r.progress {
            r.progress = p;
            changed = true;
        }
        if p >= 0.98 && !r.finished {
            r.finished = true;
            changed = true;
        }
    }
    changed
}

/// [progress] 播放中定期 / 暂停 / 切集 / 退出时回传进度：**只更新已有记录，绝不新增**。
///
/// 为什么按 `guid` 找：`guid` 来自 `history-recorded`（起播时后端刚写的那条），
/// 而 `history::record` 已按剧级 `series_key` 去重 —— 因此这里命中的就是「这部剧」的唯一
/// 记录，同一部剧重复观看不会多出记录。找不到（记录已被裁剪/竞态）时静默忽略。
pub fn update_progress(app: &AppHandle, guid: &str, position_ms: u64, duration_ms: u64) -> bool {
    if guid.is_empty() {
        return false;
    }
    let _guard = LOCK.lock();
    let mut list = load(app);
    let Some(r) = list.iter_mut().find(|r| r.guid == guid) else {
        return false;
    };
    let changed = apply_progress(r, position_ms, duration_ms);
    let series_key = series_key_of(r);
    if changed {
        save(app, &list);
    }
    // [heat] 兜底：只要「有进度回传」就确保这部剧今天在日级台账里（按剧去重，重复不计数）。
    // 起播时 history::record 已经写过一次；这里只补「当时没写成/被裁掉」的缺口，
    // 正常路径下 `series` 已包含该键 → 不重复计数、也不额外写盘。
    if !series_key.trim().is_empty() {
        let today = today_key();
        let mut days = load_days(app);
        let already = days
            .get(&today)
            .map(|d| d.series.contains(series_key.trim()))
            .unwrap_or(false);
        if !already {
            bump_day(&mut days, &today, &series_key, true);
            save_days(app, &days);
        }
    }
    changed
}

/// 插入/更新（纯函数，供单测）：**同一部剧（series_key）**用新记录覆盖、空字段从旧记录补齐；
/// 之后按 played_at 倒序 + 裁到 [`MAX_ENTRIES`]。
///
/// [lc-1710] 去重键从「条目 guid（集）」改为「剧级 series_key」：
/// 同一部剧看第 2 集时更新原记录（刷新最近观看时间/集号/进度），而不是再插一条。
fn upsert(list: &mut Vec<WatchRecord>, mut rec: WatchRecord) {
    if rec.series_key.trim().is_empty() {
        rec.series_key = series_key_of(&rec);
    }
    if let Some(pos) = list
        .iter()
        .position(|r| series_key_of(r) == rec.series_key)
    {
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
        series_key: keep(new.series_key, old.series_key),
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

/// 观影记录可见口径：
///   · [lc-1667] **个人视频（type=Video/Directory）取不到封面（poster 为空）** → 默认不计算；
///     正规影视条目即便暂时缺海报仍保留，避免误伤刮削失败的内容。
///   · [lc-1747] 在线源（webrs）功能恢复：不再按 source 整条隐藏，列表 / 年度报告同口径可见。
pub fn visible_in_history(media_type: &str, poster: &str) -> bool {
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
                // [lc-1774] 缓存存的是完整响应外壳（{code,data}），字段在 data 里 ——
                // 此前直接把外壳喂给 apply_enrich，parent_guid 永远读不到 → 富化从没生效
                let inner = v.get("data").unwrap_or(&v);
                apply_enrich(&mut items[*i], inner);
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
                let inner = v.get("data").unwrap_or(&v); // [lc-1774] 同上：剥响应外壳
                apply_enrich(&mut items[i], inner);
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
        // [lc-1667] 与观影记录同口径：无封面个人视频不计算；[lc-1747] 在线源恢复计入（年度报告同口径）
        .filter(|r| r.played_at >= MIN_VALID_MS && visible_in_history(&r.media_type, &r.poster))
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
    fn apply_progress_merges_monotonically() {
        let mut r = rec("g1", 1);
        // 起播：时长未知，位置 30s → 进度按已知时长算（0 时不写 progress）
        assert!(apply_progress(&mut r, 30_000, 0));
        assert_eq!(r.watched_ms, 30_000);
        assert_eq!(r.progress, 0.0);
        // 拿到总时长（20 分钟）→ 进度 2.5%
        assert!(apply_progress(&mut r, 30_000, 1_200_000));
        assert_eq!(r.duration_ms, 1_200_000);
        assert!((r.progress - 0.025).abs() < 1e-9);
        // 往回 seek 不应把已有进度/观看时长冲小
        assert!(!apply_progress(&mut r, 10_000, 1_200_000));
        assert_eq!(r.watched_ms, 30_000);
        assert!((r.progress - 0.025).abs() < 1e-9);
        // 播到 98% 以上 → finished
        assert!(apply_progress(&mut r, 1_190_000, 1_200_000));
        assert!(r.finished);
        assert!(r.progress >= 0.98);
        // 已 finished 后重复上报同一位置 → 无变化（避免频繁写盘）
        assert!(!apply_progress(&mut r, 1_190_000, 1_200_000));
    }

    #[test]
    fn series_upsert_keeps_single_record() {
        // 纯逻辑层面：apply_progress 只改传入记录；这里断言「同一部剧再次起播不新增记录」
        // （update_progress 只 find 不 insert 由实现保证，单测不依赖 AppHandle）。
        let mut list = vec![rec("g1", 1)];
        let before = list.len();
        let series = series_key_of(&list[0]);
        upsert(
            &mut list,
            WatchRecord {
                guid: "g2".into(),
                title: "T-g1".into(),
                played_at: 2,
                source: "fnos".into(),
                series_key: series,
                ..Default::default()
            },
        );
        // 同一部剧第二次起播仍是同一条记录（剧级去重），不是新增
        assert_eq!(list.len(), before);
    }

    #[test]
    fn episode_label_variants() {
        assert_eq!(episode_label(2, 1), "S2E1");
        assert_eq!(episode_label(0, 5), "第5集");
        assert_eq!(episode_label(0, 0), "");
    }

    #[test]
    fn history_visibility_policy() {
        // [lc-1747] 在线源恢复：不再按 source 隐藏，与库内条目同口径可见
        assert!(visible_in_history("webrs", ""));
        assert!(visible_in_history("Movie", "/p.webp"));
        // 个人视频：取不到封面 → 不计算；有封面则保留（[lc-1667]）
        assert!(!visible_in_history("Video", ""));
        assert!(!visible_in_history("Directory", "  "));
        assert!(visible_in_history("Video", "/p.webp"));
        // 正规影视条目缺海报（刮削失败）仍保留，避免误伤
        assert!(visible_in_history("Movie", ""));
        assert!(visible_in_history("Episode", ""));
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
        bump_day(&mut m, "2026-0-5", "剧A", true); // 2026 年 1 月 5 日（0 基月）
        bump_day(&mut m, "2026-0-5", "剧A", true); // 同一部剧同一天：count+1，heat 不增
        bump_day(&mut m, "2026-0-5", "剧B", true);
        bump_day(&mut m, "2026-11-31", "电影C", true);
        bump_day(&mut m, "2026-11-31", "在线源", false); // 在线源不计热力图
        bump_day(&mut m, "1970-0-1", "脏", true); // 脏键：年份 < 2000 → 丢弃
        let d = m.get("2026-0-5").unwrap();
        // [lc-1710] 同一部剧当天重复观看不重复计数：剧A + 剧B = 2
        assert_eq!(d.count, 2);
        assert_eq!(d.heat, 2);
        let e = m.get("2026-11-31").unwrap();
        assert_eq!(e.count, 2);
        assert_eq!(e.heat, 1);
        assert!(!m.contains_key("1970-0-1"));
    }

    #[test]
    fn heat_days_uses_dedup_count_and_legacy_fallback() {
        let mut m: BTreeMap<String, DayStat> = Default::default();
        bump_day(&mut m, "2026-9-17", "剧A", true);
        bump_day(&mut m, "2026-9-17", "剧B", true);
        bump_day(&mut m, "2026-9-17", "剧A", true);
        let h = heat_days(&m);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0]["count"], json!(2));
        assert_eq!(h[0]["y"], json!(2026));
        assert_eq!(h[0]["m0"], json!(9));
        assert_eq!(h[0]["d"], json!(17));

        // 旧台账没有 tracked 标记 → 回退用原始 count，历史天不会变空
        let mut legacy: BTreeMap<String, DayStat> = Default::default();
        legacy.insert(
            "2026-9-18".into(),
            DayStat { count: 1, ..Default::default() },
        );
        assert_eq!(heat_days(&legacy)[0]["count"], json!(1));
    }

    #[test]
    fn local_day_key_matches_today_key() {
        // 同一时刻的本地日期转换应与 record 用的 today_key 口径一致
        assert_eq!(local_day_key(now_ms()).unwrap(), today_key());
    }

    #[test]
    fn migration_rebuilds_legacy_days_without_overwriting_new() {
        let mut days: BTreeMap<String, DayStat> = Default::default();
        // 旧格式天：只有 count，没有 tracked/series
        days.insert(
            "2026-9-17".into(),
            DayStat { count: 1, ..Default::default() },
        );
        // 新格式天：已有去重数据，迁移不能覆盖
        days.insert(
            "2026-9-18".into(),
            DayStat {
                count: 5,
                heat: 2,
                tracked: true,
                series: ["剧X".to_string(), "剧Y".to_string()].into_iter().collect(),
            },
        );
        let mut rebuilt: BTreeMap<String, (u32, BTreeSet<String>)> = Default::default();
        rebuilt.insert(
            "2026-9-17".into(),
            (2, ["剧A".to_string(), "剧B".to_string()].into_iter().collect()),
        );
        rebuilt.insert(
            "2026-9-18".into(),
            (1, ["新剧".to_string()].into_iter().collect()),
        );
        assert!(merge_rebuilt_days(rebuilt, &mut days));
        let d = days.get("2026-9-17").unwrap();
        assert_eq!(d.count, 2);
        assert_eq!(d.heat, 2);
        assert!(d.tracked);
        assert_eq!(d.series.len(), 2);
        // 新格式天保持原样
        let e = days.get("2026-9-18").unwrap();
        assert_eq!(e.count, 5);
        assert_eq!(e.heat, 2);
        assert_eq!(e.series.len(), 2);
    }

    #[test]
    fn legacy_electron_daycount_extraction() {
        // 模拟 Chromium localStorage 的 UTF-16LE 原始字节（键后跟 JSON 值，取最新较大值）
        let payload = "fntv_wh_daycount_v1\u{1}{\"2026-7-31\":3}\u{0}fntv_wh_daycount_v1\u{1}{\"2026-7-31\":6}";
        let bytes: Vec<u8> = payload.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let mut out: BTreeMap<String, u32> = Default::default();
        extract_legacy_daycounts(&bytes, &mut out);
        assert_eq!(out.get("2026-7-31"), Some(&6));
        assert!(valid_day_key("2026-7-31"));
        assert!(!valid_day_key("2026-13-1"));
        assert!(!valid_day_key("垃圾"));
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

    // ── [lc-1710] 剧级去重（新规则：起播即记录；同一部剧只算一次）──

    fn ep(guid: &str, parent: &str, title: &str, episode: &str, played_at: u64) -> WatchRecord {
        WatchRecord {
            guid: guid.into(),
            title: title.into(),
            episode: episode.into(),
            media_type: "Episode".into(),
            source: "fnos".into(),
            parent_guid: parent.into(),
            played_at,
            ..Default::default()
        }
    }

    #[test]
    fn series_key_prefers_stable_show_anchors() {
        // BGM/TMDB 作品级锚点最优先（跨季仍是同一部剧）
        let mut r = ep("e1", "season1", "剧A", "S1E1", 1);
        r.bgm_id = "123".into();
        r.tmdb_id = "456".into();
        assert_eq!(series_key_of(&r), "bgm:123");
        r.bgm_id.clear();
        assert_eq!(series_key_of(&r), "tmdb:456");
        // 无锚点 → 季 guid（同剧各集共用）
        r.tmdb_id.clear();
        assert_eq!(series_key_of(&r), "parent:season1");
        // 没有父级（parent == guid）→ 退到番剧名
        let r2 = ep("e1", "e1", "剧A", "S1E1", 1);
        assert_eq!(series_key_of(&r2), "title:剧A");
        // 电影/个人视频：自身即作品
        let movie = WatchRecord {
            guid: "m1".into(),
            title: "电影".into(),
            media_type: "Movie".into(),
            ..Default::default()
        };
        assert_eq!(series_key_of(&movie), "guid:m1");
        // 在线源没有季 guid → 按番剧名归并
        let web = WatchRecord {
            guid: "webrs:https://x/ep1".into(),
            title: "在线番".into(),
            media_type: "webrs".into(),
            source: "webrs".into(),
            ..Default::default()
        };
        assert_eq!(series_key_of(&web), "title:在线番");
    }

    /// [speed] 倍速持久化复用同一套剧级规则：同剧不同集得到**同一个**键，不同剧不同键。
    #[test]
    fn series_key_from_parts_matches_record_rule() {
        // 同一部剧的第 1 集 / 第 2 集（不同条目 guid、同一季 guid）→ 同键
        let k1 = series_key_from_parts("e1", "season1", "Episode", "", "", "剧A");
        let k2 = series_key_from_parts("e2", "season1", "Episode", "", "", "剧A");
        assert_eq!(k1, "parent:season1");
        assert_eq!(k1, k2);
        // 另一部剧 → 不同键
        assert_eq!(
            series_key_from_parts("e9", "season9", "Episode", "", "", "剧B"),
            "parent:season9"
        );
        // 与 series_key_of 完全一致
        let r = ep("e1", "season1", "剧A", "S1E1", 1);
        assert_eq!(
            series_key_from_parts(&r.guid, &r.parent_guid, &r.media_type, &r.tmdb_id, &r.bgm_id, &r.title),
            series_key_of(&r)
        );
        // 电影：自身即作品
        assert_eq!(
            series_key_from_parts("m1", "", "Movie", "", "", "电影"),
            "guid:m1"
        );
        // 全空 → 空串（调用方据此决定不持久化倍速）
        assert_eq!(series_key_from_parts("", "", "", "", "", ""), "");
    }

    #[test]
    fn first_play_creates_one_record() {
        let mut list = Vec::new();
        upsert(&mut list, ep("e1", "s1", "剧A", "S1E1", 100));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].guid, "e1");
        assert_eq!(list[0].series_key, "parent:s1");
    }

    #[test]
    fn replay_same_show_does_not_add_record() {
        let mut list = Vec::new();
        upsert(&mut list, ep("e1", "s1", "剧A", "S1E1", 100));
        // 同一部剧的第二集：应更新原记录（不新增），并刷新最近观看时间/集号
        upsert(&mut list, ep("e2", "s1", "剧A", "S1E2", 200));
        assert_eq!(list.len(), 1, "同一部剧只能有一条记录");
        assert_eq!(list[0].guid, "e2");
        assert_eq!(list[0].episode, "S1E2");
        assert_eq!(list[0].played_at, 200);
        // 重复观看同一集：仍只有一条，时间刷新
        upsert(&mut list, ep("e2", "s1", "剧A", "S1E2", 300));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].played_at, 300);
    }

    #[test]
    fn different_shows_create_separate_records() {
        let mut list = Vec::new();
        upsert(&mut list, ep("e1", "s1", "剧A", "S1E1", 100));
        upsert(&mut list, ep("e9", "s2", "剧B", "S1E1", 200));
        assert_eq!(list.len(), 2);
        // 倒序：最近的在前
        assert_eq!(list[0].title, "剧B");
        assert_eq!(list[1].title, "剧A");
    }

    #[test]
    fn normalize_merges_legacy_episode_records() {
        // 旧 history.json：同一部剧 3 集 = 3 条（没有 series_key 字段）
        let legacy = vec![
            ep("e1", "s1", "剧A", "S1E1", 100),
            ep("e3", "s1", "剧A", "S1E3", 300),
            ep("e2", "s1", "剧A", "S1E2", 200),
            ep("x1", "s2", "剧B", "S1E1", 50),
        ];
        let out = normalize_records(legacy);
        assert_eq!(out.len(), 2, "旧数据按剧折叠");
        let a = out.iter().find(|r| r.title == "剧A").unwrap();
        assert_eq!(a.series_key, "parent:s1");
        assert_eq!(a.played_at, 300); // 最近一次
        assert_eq!(a.episode, "S1E3");
        assert_eq!(a.guid, "e3");
    }

    #[test]
    fn normalize_is_idempotent() {
        let once = normalize_records(vec![
            ep("e1", "s1", "剧A", "S1E1", 100),
            ep("e2", "s1", "剧A", "S1E2", 200),
        ]);
        let twice = normalize_records(once.clone());
        assert_eq!(once.len(), twice.len());
        assert_eq!(once[0].series_key, twice[0].series_key);
        assert_eq!(once[0].played_at, twice[0].played_at);
    }
}
