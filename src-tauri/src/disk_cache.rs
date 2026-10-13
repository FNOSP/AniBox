//! 落盘缓存 —— 图片 / TMDB 演员表类 JSON 的「**内存 → 磁盘 → 网络**」三级缓存。
//!
//! 为什么不用 `%TEMP%`（bgm.rs 的条目缓存还在那儿，那是另一回事）：`%TEMP%` 会被系统清理，
//! 用户要求「拉了原图直接保存到**软件目录**里」「拉了就本地缓存不要多拉」。所以这里落在
//! **应用数据目录**（Tauri `app_data_dir()/cache`，与 config.json 同级）：
//!
//! ```text
//! <app_data_dir>/cache/
//!   img/<前2位>/<sha256(key)>.bin     ← 图片字节（NAS / TMDB / Bangumi 三种图共用一个池）
//!   img/<前2位>/<sha256(key)>.meta    ← {"key":..,"ct":..,"t":创建秒,"a":最后访问秒}
//!   json/<前2位>/<sha256(key)>.bin    ← JSON 缓存（如 TMDB 演员表）
//!   json/<前2位>/<sha256(key)>.meta
//! ```
//!
//! 三层行为：
//!   · 命中内存 → 直接返回（不碰盘、不碰网）；
//!   · 未命中内存但命中磁盘 → 读盘返回，并把「最后访问时间」按 ≤1 次/分钟写回 meta（LRU 用）；
//!   · 都没有 → 调用方去网络取，取到后 `put` 同时写内存 + 磁盘。
//!
//! LRU 淘汰：上限来自 config.json 的 `cache_limit_mb`（0 = 不限）。每次写入后若总占用超过上限，
//! 按 `a`（最后访问）升序删到上限的 90%（留余量，避免"写一张淘一张"）；**淘汰磁盘条目的同时
//! 把同名内存条目一起丢掉**（否则"清了盘内存还留着旧图"，用户看到的占用对不上）。
//!
//! 上限/占用/清理三个命令在 lib.rs（`cache_stats` / `cache_set_limit` / `cache_clear`），
//! 设置面板只认这三个数——这里不持有任何 UI/业务状态。

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// 内存层上限（沿用改造前 imgproto 的 400 张 / 192MB）
const MEM_MAX_ENTRIES: usize = 400;
const MEM_MAX_BYTES: usize = 192 << 20;
/// 磁盘命中后，至少间隔这么久才把「最后访问时间」写回 meta（否则每读一张图都要写一次盘）
const TOUCH_MIN_INTERVAL_SECS: u64 = 60;
/// 淘汰目标 = 上限 × 90%（留余量，避免每次 put 都触发一轮淘汰）
const EVICT_TARGET_PERCENT: u64 = 90;
/// 单条 JSON 缓存（演员表等）不设总大小硬限，但超上限时与图片一起参与 LRU
const META_SUFFIX: &str = ".meta";
const DATA_SUFFIX: &str = ".bin";

/// 内存层条目
struct MemEntry {
    ct: String,
    bytes: Vec<u8>,
    accessed: u64,
}

/// 磁盘层索引条目（启动时扫一遍目录建好，之后由 put/get/淘汰维护）
struct Entry {
    /// 实际落盘路径（`.bin`）
    path: PathBuf,
    /// 内容类型（从 meta 读回；磁盘命中时直接回给 WebView）
    ct: String,
    size: u64,
    /// 创建时间（秒）—— JSON 缓存的 TTL 判据
    created: u64,
    /// 最后访问时间（秒）—— LRU 判据
    accessed: u64,
}

struct Store {
    root: PathBuf,
    index: HashMap<String, Entry>,
    mem: HashMap<String, MemEntry>,
    mem_bytes: u64,
    total: u64,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
/// 上限（字节；0 = 不限）。单独放原子量：淘汰判据要在锁内读，但设置变更来自别的线程。
static LIMIT_BYTES: AtomicU64 = AtomicU64::new(2 << 30);

fn now_secs() -> u64 {
    crate::mirror_sign::now_secs()
}

/// 缓存键 → 落盘文件名（sha256 十六进制；键里含 `:`、`/`、中文都能安全落盘）。
fn hash_of(key: &str) -> String {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    let d = h.finalize();
    let mut s = String::with_capacity(64);
    for b in d.iter() {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 缓存键前缀决定落在 img/ 还是 json/（两类共用一个上限与一套 LRU）
fn sub_dir(key: &str) -> &'static str {
    if key.starts_with("json:") {
        "json"
    } else {
        "img"
    }
}

fn entry_paths(root: &std::path::Path, key: &str) -> (PathBuf, PathBuf) {
    let h = hash_of(key);
    let dir = root.join(sub_dir(key)).join(&h[..2]);
    (dir.join(format!("{h}{DATA_SUFFIX}")), dir.join(format!("{h}{META_SUFFIX}")))
}

fn ensure_dir(p: &std::path::Path) {
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
}

/// 启动初始化：建目录 + 扫盘建索引 + 先按上限淘汰一次（上次退出后配置可能被改小了）。
pub fn init(root: PathBuf, limit_mb: u64) {
    LIMIT_BYTES.store(limit_mb.saturating_mul(1024 * 1024), Ordering::Relaxed);
    let mut index = HashMap::new();
    let mut total = 0u64;
    for kind in ["img", "json"] {
        let Ok(buckets) = std::fs::read_dir(root.join(kind)) else { continue };
        for bucket in buckets.flatten() {
            let Ok(files) = std::fs::read_dir(bucket.path()) else { continue };
            for f in files.flatten() {
                let p = f.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                if !name.ends_with(META_SUFFIX) {
                    continue;
                }
                let data = p.with_file_name(format!("{}{DATA_SUFFIX}", name.trim_end_matches(META_SUFFIX)));
                let Ok(meta_txt) = std::fs::read_to_string(&p) else { continue };
                let Ok(meta) = serde_json::from_str::<Value>(&meta_txt) else {
                    let _ = std::fs::remove_file(&p);
                    continue;
                };
                let key = meta.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if key.is_empty() {
                    let _ = std::fs::remove_file(&p);
                    continue;
                }
                // 只有 meta 没有数据文件 = 写盘途中被打断 → 视为垃圾清掉
                let Ok(md) = std::fs::metadata(&data) else {
                    let _ = std::fs::remove_file(&p);
                    continue;
                };
                let size = md.len();
                index.insert(
                    key,
                    Entry {
                        path: data,
                        ct: meta.get("ct").and_then(|v| v.as_str()).unwrap_or("image/jpeg").to_string(),
                        size,
                        created: meta.get("t").and_then(|v| v.as_u64()).unwrap_or(0),
                        accessed: meta.get("a").and_then(|v| v.as_u64()).unwrap_or(0),
                    },
                );
                total += size;
            }
        }
    }
    let _ = STORE.set(Mutex::new(Store {
        root,
        index,
        mem: HashMap::new(),
        mem_bytes: 0,
        total,
    }));
    let entries = with_store(|s| s.index.len()).unwrap_or(0);
    crate::libmpv::trace(&format!(
        "[cache] 初始化 root={} 已有 {} 条 / {:.1}MB 上限={}MB",
        root_display(),
        entries,
        total as f64 / 1048576.0,
        limit_mb
    ));
    enforce_limit(None);
}

pub fn root_display() -> String {
    with_store(|s| s.root.display().to_string()).unwrap_or_default()
}

fn with_store<T>(f: impl FnOnce(&mut Store) -> T) -> Option<T> {
    let mut g = STORE.get()?.lock().ok()?;
    Some(f(&mut g))
}

/// 改上限（config 保存后调用）：立即按新上限淘汰一次。
pub fn set_limit_mb(limit_mb: u64) {
    LIMIT_BYTES.store(limit_mb.saturating_mul(1024 * 1024), Ordering::Relaxed);
    crate::libmpv::trace(&format!("[cache] 上限改为 {limit_mb}MB"));
    enforce_limit(None);
}

pub fn limit_mb() -> u64 {
    let b = LIMIT_BYTES.load(Ordering::Relaxed);
    if b == 0 {
        0
    } else {
        b / (1024 * 1024)
    }
}

/// (占用字节, 条目数) —— 设置面板显示用
pub fn stats() -> (u64, u64) {
    with_store(|s| (s.total, s.index.len() as u64)).unwrap_or((0, 0))
}

/// 清空：**内存层与磁盘层一起清**（清了盘内存还留着旧图 = 占用对不上、还看得到旧图，
/// 正是这次要避免的那半个 bug）。删目录后重建，索引/计数归零。
pub fn clear() -> Result<(), String> {
    let root = with_store(|s| {
        s.mem.clear();
        s.mem_bytes = 0;
        s.index.clear();
        s.total = 0;
        s.root.clone()
    })
    .ok_or_else(|| "缓存未初始化".to_string())?;
    if root.exists() {
        std::fs::remove_dir_all(&root).map_err(|e| format!("清理缓存目录失败: {e}"))?;
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("重建缓存目录失败: {e}"))?;
    crate::libmpv::trace("[cache] 已清空（内存 + 磁盘）");
    Ok(())
}

/// 三级读：内存 → 磁盘。返回 (content-type, bytes)。
pub fn get(key: &str) -> Option<(String, Vec<u8>)> {
    // ① 内存（命中即刷新 LRU 序号，不碰盘）
    if let Some((ct, bytes)) = with_store(|s| {
        let now = now_secs();
        let e = s.mem.get_mut(key)?;
        e.accessed = now;
        Some((e.ct.clone(), e.bytes.clone()))
    })
    .flatten()
    {
        return Some((ct, bytes));
    }
    // ② 磁盘：先在锁内取路径/ct/元信息，出锁读文件（大图几 MB，别把别的请求堵在锁上）
    let (path, ct, accessed) = with_store(|s| {
        let e = s.index.get(key)?;
        Some((e.path.clone(), e.ct.clone(), e.accessed))
    })
    .flatten()?;
    let bytes = std::fs::read(&path).ok()?;
    // 回填内存层（后续同图零盘 IO），并把最后访问时间按节流写回磁盘 meta
    put_mem(key.to_string(), ct.clone(), bytes.clone());
    let now = now_secs();
    if now.saturating_sub(accessed) >= TOUCH_MIN_INTERVAL_SECS {
        with_store(|s| {
            if let Some(e) = s.index.get_mut(key) {
                e.accessed = now;
                let (_, meta) = entry_paths(&s.root, key);
                let body = json!({ "key": key, "ct": e.ct, "t": e.created, "a": e.accessed });
                let _ = std::fs::write(meta, body.to_string());
            }
        });
    }
    Some((ct, bytes))
}

/// 写：内存 + 磁盘（磁盘写失败只记日志——内存层仍然有效，不影响本次返回）。
pub fn put(key: &str, ct: &str, bytes: &[u8]) {
    put_mem(key.to_string(), ct.to_string(), bytes.to_vec());
    let Some((path, meta_path)) = with_store(|s| {
        let (p, m) = entry_paths(&s.root, key);
        ensure_dir(&p);
        (p, m)
    }) else {
        return;
    };
    if std::fs::write(&path, bytes).is_err() {
        return;
    }
    let now = now_secs();
    let created = now;
    let size = bytes.len() as u64;
    let meta = json!({ "key": key, "ct": ct, "t": created, "a": now });
    if std::fs::write(&meta_path, meta.to_string()).is_err() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    with_store(|s| {
        let old = s.index.insert(
            key.to_string(),
            Entry {
                path,
                ct: ct.to_string(),
                size,
                created,
                accessed: now,
            },
        );
        s.total = s.total.saturating_sub(old.map(|o| o.size).unwrap_or(0)) + size;
    });
    // ③ 写入后按上限淘汰（超限才动，正常路径这里是一次原子读 + 比较）
    enforce_limit(Some(key));
}

fn put_mem(key: String, ct: String, bytes: Vec<u8>) {
    with_store(|s| {
        if let Some(old) = s.mem.remove(&key) {
            s.mem_bytes = s.mem_bytes.saturating_sub(old.bytes.len() as u64);
        }
        s.mem_bytes += bytes.len() as u64;
        s.mem.insert(key, MemEntry { ct, bytes, accessed: now_secs() });
        // 内存层独立 LRU（与磁盘层互不影响：内存只丢副本，磁盘还在）
        while s.mem.len() > MEM_MAX_ENTRIES || s.mem_bytes > MEM_MAX_BYTES as u64 {
            let Some(victim) = s
                .mem
                .iter()
                .min_by_key(|(_, e)| e.accessed)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(v) = s.mem.remove(&victim) {
                s.mem_bytes = s.mem_bytes.saturating_sub(v.bytes.len() as u64);
            }
        }
    });
}

/// LRU 淘汰：超上限 → 按最后访问时间升序删到上限的 90%；**磁盘条目被删时内存副本同步删掉**。
/// `just_written` = 刚写入的键（优先保护它，避免极端情况下"刚拉回的图立刻被淘汰"）。
fn enforce_limit(just_written: Option<&str>) {
    let limit = LIMIT_BYTES.load(Ordering::Relaxed);
    if limit == 0 {
        return;
    }
    let mut removed = 0usize;
    with_store(|s| {
        if s.total <= limit {
            return;
        }
        let target = limit * EVICT_TARGET_PERCENT / 100;
        let mut keys: Vec<(String, u64)> = s.index.iter().map(|(k, e)| (k.clone(), e.accessed)).collect();
        keys.sort_by_key(|(_, a)| *a);
        for (k, _) in keys {
            if s.total <= target {
                break;
            }
            if Some(k.as_str()) == just_written {
                continue;
            }
            if let Some(e) = s.index.remove(&k) {
                let _ = std::fs::remove_file(&e.path);
                let (_, meta) = entry_paths(&s.root, &k);
                let _ = std::fs::remove_file(meta);
                s.total = s.total.saturating_sub(e.size);
                // 内存层同步失效：清了盘还留着旧内存副本 = 看得到已删的图
                if let Some(m) = s.mem.remove(&k) {
                    s.mem_bytes = s.mem_bytes.saturating_sub(m.bytes.len() as u64);
                }
                removed += 1;
            }
        }
    });
    if removed > 0 {
        crate::libmpv::trace(&format!("[cache] LRU 淘汰 {removed} 条（超上限 {}MB）", limit / (1024 * 1024)));
    }
}

/// JSON 缓存读（TTL 秒；0 = 不过期）。命中返回原 JSON。
pub fn json_get(key: &str, ttl_secs: u64) -> Option<Value> {
    let created = with_store(|s| s.index.get(key).map(|e| e.created)).flatten()?;
    if ttl_secs > 0 && now_secs().saturating_sub(created) > ttl_secs {
        return None;
    }
    let (ct, bytes) = get(key)?;
    if !ct.contains("json") {
        return None;
    }
    serde_json::from_slice::<Value>(&bytes).ok()
}

/// JSON 缓存写（与图片共用同一个上限与 LRU）。
pub fn json_put(key: &str, val: &Value) {
    let Ok(text) = serde_json::to_string(val) else { return };
    put(key, "application/json", text.as_bytes());
}
