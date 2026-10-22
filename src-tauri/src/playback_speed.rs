//! [speed] 按**剧集（series）**持久化 mpv 播放速度。
//!
//! 需求：用户在 mpv 里调了播放速度（`speed` 属性），**同一部剧的所有集共享同一个倍速**：
//! 任意一集调到 1.5x，切到同剧其它集仍是 1.5x；换到另一部剧用它自己的记录
//! （没有记录就 1.0）。只作用于**内置 mpv**（PotPlayer 路径不走这里）。
//!
//! 存储沿用项目既有约定：`app_data_dir/playback-speed.json`，
//! 形状 `{ "<series_key>": 1.25 }`（与 `watched.json` / `favorites.json` 同思路）。
//! 这是**本地状态**，按仓库规则不入库（在 `app_data_dir`，本来就不在仓库里）。
//!
//! 写入策略：内存里维护一份 map，`record_speed` 只更新内存并起一个
//! [`FLUSH_DEBOUNCE`] 的防抖线程；期间再有改动会刷新 generation，旧线程自动作废。
//! 切集 / 退出 / 被强杀前由调用方显式 [`flush`]，保证最后一条落盘。
//!
//! key 规则（剧级键，**由调用方**在 `lib.rs::play_item` 算好再传进来）：
//!   · `series:<剧 guid>` —— play/info 带 `series_guid` 时最精确（跨季也同一部剧）；
//!   · 否则用 [`crate::history::series_key_of`] 同一套规则：
//!     `bgm:<id>` / `tmdb:<id>` → `parent:<季 guid>` → `title:<剧名>` → `guid:<条目 guid>`；
//!   · **空串**（直播、本地 m3u8、在线源 webrs 等拿不到剧级标识）→ **不持久化**，
//!     `record_speed` 直接忽略，播放时保持 1.0。用文件路径做 key 会因临时文件名
//!     （anibox-playlist.m3u）每次都变而失效，反而会串到别的片上。
//!
//! 旧格式（上一版按**单集 item guid** 写下的裸 guid 键）没有上述前缀，
//! [`ensure_loaded`] 读入时直接丢弃 —— 上线时间极短、迁移价值为零，见 [`is_series_key`]。

use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// 允许持久化的速度区间（与 mpv 的常见可用范围一致；异常值一律不写盘）。
pub const MIN_SPEED: f64 = 0.25;
pub const MAX_SPEED: f64 = 4.0;
/// 没有该剧记录时应用的速度。
pub const DEFAULT_SPEED: f64 = 1.0;
/// 写盘防抖窗口：连续调速（手柄连点 / 按住快捷键）只在停手后落一次盘。
const FLUSH_DEBOUNCE: Duration = Duration::from_millis(1500);

/// 剧级 key 的合法前缀（与 `history::series_key_of` 的前缀一一对应，外加 `series:`）。
const SERIES_KEY_PREFIXES: [&str; 6] = ["series:", "bgm:", "tmdb:", "parent:", "title:", "guid:"];

#[derive(Default)]
struct Store {
    map: HashMap<String, f64>,
    loaded: bool,
    dirty: bool,
    /// 每次 record_speed 递增；防抖线程据此判断自己是否已过期。
    generation: u64,
    /// 仅用于测试/诊断，生产路径不依赖。
    last_note: Option<Instant>,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

fn store_path(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("playback-speed.json"))
}

/// 速度是否落在允许区间内（NaN / Inf / 越界都不接受）。
pub fn valid_speed(speed: f64) -> bool {
    speed.is_finite() && (MIN_SPEED..=MAX_SPEED).contains(&speed)
}

/// 归一化：保留两位小数（mpv 的 speed 是 f64，1.2500000001 这种噪声不写盘）。
pub fn normalize_speed(speed: f64) -> Option<f64> {
    if !valid_speed(speed) {
        return None;
    }
    let s = (speed * 100.0).round() / 100.0;
    if valid_speed(s) {
        Some(s)
    } else {
        None
    }
}

/// 是不是**剧级** key（带 `history::series_key_of` 的前缀）。
///
/// 用途：旧版本按「单集 item guid」写过裸 guid 键，这些键没有前缀 → 读入时丢弃，
/// 相当于一次性忽略旧文件（不做迁移）。飞牛 guid 是纯十六进制串，不含冒号。
fn is_series_key(key: &str) -> bool {
    let k = key.trim();
    !k.is_empty() && SERIES_KEY_PREFIXES.iter().any(|p| k.starts_with(p))
}

/// 写入/更新一条记录；返回值 = 是否真的发生了变化（值相同视为未变）。
fn upsert(map: &mut HashMap<String, f64>, key: &str, speed: f64) -> bool {
    let key = key.trim();
    if key.is_empty() || !valid_speed(speed) {
        return false;
    }
    match map.get(key) {
        Some(old) if (*old - speed).abs() < 0.001 => false,
        _ => {
            map.insert(key.to_string(), speed);
            true
        }
    }
}

fn lookup(map: &HashMap<String, f64>, key: &str) -> Option<f64> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    map.get(key).copied().filter(|s| valid_speed(*s))
}

fn ensure_loaded(app: &AppHandle, st: &mut Store) {
    if st.loaded {
        return;
    }
    st.loaded = true;
    let Some(path) = store_path(app) else { return };
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    // 读失败/格式坏都当空表：坏文件下次 flush 时会被覆盖，不至于卡住播放。
    if let Ok(raw) = serde_json::from_str::<HashMap<String, f64>>(&text) {
        let before = raw.len();
        st.map = raw
            .into_iter()
            // 丢弃上一版「按单集 item guid」写下的裸 guid 键（无剧级前缀）→ 一次性忽略旧格式。
            .filter(|(k, v)| is_series_key(k) && valid_speed(*v))
            .map(|(k, v)| (k.trim().to_string(), normalize_speed(v).unwrap_or(v)))
            .collect();
        // 清理过旧键 → 标记脏，下次 flush 时把干净的表写回（顺手把旧文件覆盖掉）。
        if st.map.len() != before {
            st.dirty = true;
        }
    }
}

/// 查询某一部剧记录的速度；无记录 / 无剧级 key 返回 `None`。
pub fn speed_for(app: &AppHandle, series_key: &str) -> Option<f64> {
    if series_key.trim().is_empty() {
        return None;
    }
    let mut st = store().lock().ok()?;
    ensure_loaded(app, &mut st);
    lookup(&st.map, series_key)
}

/// 记录某一部剧当前速度（内存 + 防抖落盘）。剧级 key 为空或速度非法时忽略。
pub fn record_speed(app: &AppHandle, series_key: &str, speed: f64) {
    let Some(speed) = normalize_speed(speed) else { return };
    let key = series_key.trim();
    if key.is_empty() {
        return;
    }
    let generation = {
        let Ok(mut st) = store().lock() else { return };
        ensure_loaded(app, &mut st);
        // 1.0 = 默认值：没有记录时**不落盘**（否则每部剧都会写一条无意义的 1.0）。
        // 但如果该剧已有记录（用户从 2.0 调回 1.0），必须覆盖，否则下次仍恢复 2.0。
        if (speed - DEFAULT_SPEED).abs() < 0.001 && !st.map.contains_key(key) {
            return;
        }
        if !upsert(&mut st.map, key, speed) {
            return;
        }
        st.dirty = true;
        st.last_note = Some(Instant::now());
        st.generation = st.generation.wrapping_add(1);
        st.generation
    };
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(FLUSH_DEBOUNCE);
        let due = {
            let Ok(st) = store().lock() else { return };
            st.generation == generation && st.dirty
        };
        if due {
            flush(&app);
        }
    });
}

/// 立即落盘（切集 / 退出 / 强杀兜底前调用）。无脏数据时是空操作。
pub fn flush(app: &AppHandle) {
    let map = {
        let Ok(mut st) = store().lock() else { return };
        ensure_loaded(app, &mut st);
        if !st.dirty {
            return;
        }
        st.dirty = false;
        st.map.clone()
    };
    let Some(path) = store_path(app) else { return };
    let Ok(text) = serde_json::to_string_pretty(&map) else {
        mark_dirty();
        return;
    };
    // 原子写：先 .tmp 再 rename，避免强杀瞬间留下半截 JSON。
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_err() || std::fs::rename(&tmp, &path).is_err() {
        mark_dirty(); // 写失败 → 下次再试，别把内存里的记录丢掉
    }
}

fn mark_dirty() {
    if let Ok(mut st) = store().lock() {
        st.dirty = true;
    }
}

/// 从 mpv `speed` 事件数据取 f64（mpv 可能给整数 1 / 小数 1.5）。
pub fn speed_from_value(v: &Value) -> Option<f64> {
    v.as_f64().filter(|s| valid_speed(*s))
}

#[cfg(test)]
mod tests {
    use super::{
        is_series_key, lookup, normalize_speed, speed_from_value, upsert, valid_speed, MAX_SPEED,
        MIN_SPEED,
    };
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn speed_range_validation() {
        assert!(valid_speed(1.0));
        assert!(valid_speed(MIN_SPEED));
        assert!(valid_speed(MAX_SPEED));
        assert!(!valid_speed(0.1));
        assert!(!valid_speed(8.0));
        assert!(!valid_speed(f64::NAN));
        assert!(!valid_speed(f64::INFINITY));
        assert!(!valid_speed(0.0));
        assert!(!valid_speed(-1.0));
    }

    #[test]
    fn normalize_rounds_and_rejects() {
        assert_eq!(normalize_speed(1.23456), Some(1.23));
        assert_eq!(normalize_speed(1.5), Some(1.5));
        assert_eq!(normalize_speed(10.0), None);
        assert_eq!(normalize_speed(f64::NAN), None);
    }

    /// 剧级键：同剧不同集用**同一个** key → 共享速度；不同剧 key 不同 → 互不影响。
    #[test]
    fn same_series_shares_speed_other_series_isolated() {
        let mut map: HashMap<String, f64> = HashMap::new();
        // 第 1 集调到 1.5 → 记在剧级键上
        assert!(upsert(&mut map, "parent:season1", 1.5));
        // 切到同剧第 2 集：查同一个剧级键 → 仍是 1.5（不再回 1.0）
        assert_eq!(lookup(&map, "parent:season1"), Some(1.5));
        // 另一部剧：无记录 → None（播放时回落 1.0）
        assert_eq!(lookup(&map, "parent:season2"), None);
        // 另一部剧调到 0.75，不影响第一部
        assert!(upsert(&mut map, "parent:season2", 0.75));
        assert_eq!(lookup(&map, "parent:season1"), Some(1.5));
        assert_eq!(lookup(&map, "parent:season2"), Some(0.75));
        // 同剧再调 → 覆盖同一键（不产生第二条记录）
        assert!(upsert(&mut map, "parent:season1", 2.0));
        assert_eq!(map.len(), 2);
        assert_eq!(lookup(&map, "parent:season1"), Some(2.0));
        // 空 key / 非法值不落
        assert!(!upsert(&mut map, "", 1.5));
        assert!(!upsert(&mut map, "guid:x", 99.0));
        assert_eq!(lookup(&map, ""), None);
    }

    /// 迁移/忽略旧格式：上一版按单集 item guid 写的裸 guid 键必须被识别为「非剧级」。
    #[test]
    fn legacy_item_guid_keys_are_not_series_keys() {
        assert!(!is_series_key("abc123def456"));
        assert!(!is_series_key(""));
        assert!(is_series_key("series:s0"));
        assert!(is_series_key("parent:abc123"));
        assert!(is_series_key("tmdb:12345"));
        assert!(is_series_key("bgm:678"));
        assert!(is_series_key("title:某部番"));
        assert!(is_series_key("guid:abc123"));
    }

    #[test]
    fn map_serialization_roundtrip() {
        let mut map: HashMap<String, f64> = HashMap::new();
        map.insert("parent:season-a".into(), 1.5);
        map.insert("guid:movie-b".into(), 0.75);
        let text = serde_json::to_string(&map).unwrap();
        let back: HashMap<String, f64> = serde_json::from_str(&text).unwrap();
        assert_eq!(back.get("parent:season-a").copied(), Some(1.5));
        assert_eq!(back.get("guid:movie-b").copied(), Some(0.75));
    }

    #[test]
    fn speed_value_extraction() {
        assert_eq!(speed_from_value(&json!(1.5)), Some(1.5));
        assert_eq!(speed_from_value(&json!(2)), Some(2.0));
        assert_eq!(speed_from_value(&json!("1.5")), None);
        assert_eq!(speed_from_value(&json!(null)), None);
        assert_eq!(speed_from_value(&json!(20)), None);
    }
}
