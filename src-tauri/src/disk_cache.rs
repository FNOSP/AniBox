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
    /// [lc-123-rev5] 上游响应的 ETag（有则下次刷新带 `If-None-Match`，304 就不动内容）
    etag: Option<String>,
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
                        // [lc-123-rev5] 重启后 ETag 仍在（条件请求要跨进程复用）
                        etag: meta.get("etag").and_then(|v| v.as_str()).map(str::to_string),
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
                // 顺带把 etag 一起写回（否则一次「访问时间写回」会把条件请求要用的 ETag 抹掉）
                let body = match &e.etag {
                    Some(et) => json!({ "key": key, "ct": e.ct, "t": e.created, "a": e.accessed, "etag": et }),
                    None => json!({ "key": key, "ct": e.ct, "t": e.created, "a": e.accessed }),
                };
                let _ = std::fs::write(meta, body.to_string());
            }
        });
    }
    Some((ct, bytes))
}

/// 写：内存 + 磁盘（磁盘写失败只记日志——内存层仍然有效，不影响本次返回）。
pub fn put(key: &str, ct: &str, bytes: &[u8]) {
    put_full(key, ct, bytes, None);
}

/// [lc-123-rev5] 写入并**记下上游 ETag**（key 不变，meta 里多一个 `etag` 字段）。
pub fn put_full(key: &str, ct: &str, bytes: &[u8], etag: Option<&str>) {
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
    let meta = match etag {
        Some(et) if !et.is_empty() => {
            json!({ "key": key, "ct": ct, "t": created, "a": now, "etag": et })
        }
        _ => json!({ "key": key, "ct": ct, "t": created, "a": now }),
    };
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
                etag: etag.filter(|e| !e.is_empty()).map(str::to_string),
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

/* ─────────── [lc-123-rev5] 条件请求（ETag / If-None-Match）支持 ───────────
   需求（用户）：「刷给我感觉不需要这么频繁，能不能搞个检测新增才刷新的逻辑」——
   所以"到期"不等于重拉全量：带上次存下的 ETag 发一个几字节的探测，上游 304 就**什么都不动**
   （界面零变化、零闪烁），只把本地时间戳顺延（= TTL 重新计时）。

   为什么放在 disk_cache：TMDB discover 与 Bangumi calendar 都要这条流程，**只此一份实现**
   （别在两个业务模块里各抄一遍状态机）。`json_cached_conditional` 的参数化 fetcher 也是
   离线小测的注入点（注入计数假 fetcher → 证明命中缓存时网络次数为 0）。 */

/// JSON 缓存读（含元信息）：(payload, etag, created)。**不判 TTL** ——
/// 调用方按 `created` 自己判（条件请求需要"就算过期也要拿到旧内容与 ETag"）。
pub fn json_get_full(key: &str) -> Option<(Value, Option<String>, u64)> {
    let (etag, created) = with_store(|s| s.index.get(key).map(|e| (e.etag.clone(), e.created))).flatten()?;
    let (ct, bytes) = get(key)?;
    if !ct.contains("json") {
        return None;
    }
    let v = serde_json::from_slice::<Value>(&bytes).ok()?;
    Some((v, etag, created))
}

/// JSON 缓存写（带 ETag；key 不变，meta 里多一个 `etag` 字段）。
pub fn json_put_full(key: &str, val: &Value, etag: Option<&str>) {
    let Ok(text) = serde_json::to_string(val) else { return };
    put_full(key, "application/json", text.as_bytes(), etag);
}

/// **只顺延时间戳**（304 走这里）：内容与 ETag 都不动，TTL 重新计时。
/// 返回 false = 键不在（没有可顺延的东西，调用方按"本地无缓存"处理）。
pub fn json_touch(key: &str) -> bool {
    with_store(|s| {
        let root = s.root.clone();
        let Some(e) = s.index.get_mut(key) else {
            return false;
        };
        let now = now_secs();
        e.created = now;
        e.accessed = now;
        let (_, meta) = entry_paths(&root, key);
        let body = match &e.etag {
            Some(et) => json!({ "key": key, "ct": e.ct, "t": e.created, "a": e.accessed, "etag": et }),
            None => json!({ "key": key, "ct": e.ct, "t": e.created, "a": e.accessed }),
        };
        let _ = std::fs::write(meta, body.to_string());
        true
    })
    .unwrap_or(false)
}

/// 条件请求的两种结局（镜像 GET 的结果）
pub enum Cond {
    /// 304 Not Modified：上游说"没变"
    NotModified,
    /// 200：新内容 + 上游给的 ETag（可能没有；`/img/*`/`/dl/*` 这类**永远没有**，不是异常）
    Fresh(Value, Option<String>),
}

/// RFC 7232 **弱比较**（忽略 `W/` 前缀）：
/// 实测坑（镜像侧 17/17 自检给的结论）——Cloudflare 出口压缩会把 200 响应上的强 ETag 改写成
/// `W/"<hash>"`，而 304（无 body、不走压缩）回的是 `"<hash>"`。所以**比较**时两侧都要容忍 `W/`。
/// 但**回传 `If-None-Match` 时把服务端上次给的值原样带回**（不做任何加工，否则会出现
/// "送回 W/… 永远匹配不上 → 永远 200"的假失效）。
pub fn etag_weak_eq(a: Option<&str>, b: Option<&str>) -> bool {
    fn norm(s: Option<&str>) -> &str {
        let x = s.unwrap_or("").trim();
        x.strip_prefix("W/").unwrap_or(x).trim()
    }
    let (x, y) = (norm(a), norm(b));
    !x.is_empty() && x == y
}

/// 「落盘缓存 + 条件请求」的**唯一实现**（TMDB discover / Bangumi calendar 共用）：
///   ① 本地**新鲜**（未超 TTL）且非 force → 直接返回缓存：**零网络**、changed=false；
///      注：`ttl_secs = 0` 与 `json_get` 同义 = 不过期（调用方一律传正数，这里保持一致）；
///   ② 超 TTL / force → 带 `If-None-Match: <存下的 ETag>` 请求：
///        304 → **payload 不变**，只 `json_touch` 顺延时间戳 → changed=false；
///        200 → 新内容替换 + 存新 ETag → changed=true；
///   ③ 本地没有（首次/被清）→ 全量拉（没有 ETag 可带）→ changed=true。
/// `no_etag`：失败重试专用（连条件头都不带，强制全量）。
/// `valid`：缓存内容的额外校验（如"日历 payload 必须属于今天"）；不通过就当作本地没有 → 全量重取。
/// 返回 `(payload, changed)`：**changed=false 时调用方不得碰 UI**（304 不许 re-render/重置滚动）。
pub async fn json_cached_conditional<F, Fut>(
    key: &str,
    ttl_secs: u64,
    force: bool,
    no_etag: bool,
    valid: impl Fn(&Value) -> bool,
    fetch: F,
) -> Result<(Value, bool), String>
where
    F: FnOnce(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<Cond, String>>,
{
    if let Some((val, etag, created)) = json_get_full(key) {
        if valid(&val) {
            if !force && (ttl_secs == 0 || now_secs().saturating_sub(created) <= ttl_secs) {
                crate::libmpv::trace(&format!("[cache] JSON 命中（TTL 未到）→ 不打网络: {key}"));
                return Ok((val, false));
            }
            let want = if no_etag { None } else { etag.clone() }; // clone：下面弱比较还要用本地那份
            return match fetch(want).await? {
                Cond::NotModified => {
                    json_touch(key); // 304：内容不变，只把时间戳顺延（TTL 重新计时）
                    crate::libmpv::trace(&format!(
                        "[cache] 304 Not Modified → 内容不变，仅顺延时间戳: {key}"
                    ));
                    Ok((val, false))
                }
                Cond::Fresh(v, et) => {
                    // 200 但**内容其实没变**也算"没变"（镜像刚升过缓存键版本时，第一次带
                    // If-None-Match 的请求大概率仍回 200；老副本回源后内容一致）：
                    //   · ETag 弱比较相等（忽略 W/ 前缀）→ 内容同一份；
                    //   · 或 payload 逐字节相同 → 同一份（上游没给 ETag 时的兜底）。
                    // 这一支只顺延时间戳、**不换内容、不碰 UI** —— 才是用户要的「检测新增才刷新」。
                    if etag_weak_eq(etag.as_deref(), et.as_deref()) || v == val {
                        json_touch(key);
                        crate::libmpv::trace(&format!(
                            "[cache] 200 但内容未变（ETag 弱相等/载荷相同）→ 仅顺延时间戳: {key}"
                        ));
                        return Ok((val, false));
                    }
                    json_put_full(key, &v, et.as_deref());
                    Ok((v, true))
                }
            };
        }
        // 键对不上（典型：跨天）→ 旧 payload 与旧 ETag 都不可信 → 走全量重取
        crate::libmpv::trace(&format!("[cache] JSON 内容校验不通过（如跨天）→ 全量重取: {key}"));
    }
    match fetch(None).await? {
        Cond::Fresh(v, et) => {
            json_put_full(key, &v, et.as_deref());
            Ok((v, true))
        }
        Cond::NotModified => Err("本地无缓存却收到 304（上游/镜像状态不一致）".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// 离线小测（**不打网络**）：注入计数假 fetcher，验证三种场景的网络次数与 changed 语义：
    ///   (a) 缓存新鲜 → 网络 0 次、内容/时间戳不变；
    ///   (b) TTL 过期 + 假 304 → 网络 1 次（探测）、**payload 不变**、时间戳顺延；
    ///   (c) TTL 过期 + 假 200 → 网络 1 次、payload 被替换、ETag 更新；
    ///   (d) 手动 force → 即使新鲜也发一次**带条件头**的请求；304 则内容不变。
    #[tokio::test]
    async fn json_cached_conditional_offline_paths() {
        let dir = std::env::temp_dir().join(format!("fntv-dc-cond-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        init(dir, 64);

        let key = "json:test:cond:v1";
        let calls = Rc::new(Cell::new(0usize));
        let ttl = 12 * 3600u64;

        // (a1) 首次：本地没有 → 网络 1 次、落盘、changed=true
        let c = calls.clone();
        let (v1, ch1) = json_cached_conditional(key, ttl, false, false, |_| true, move |_et| {
            let c = c.clone();
            async move {
                c.set(c.get() + 1);
                Ok(Cond::Fresh(json!({"n": 1}), Some("\"etag-1\"".to_string())))
            }
        })
        .await
        .expect("首次不该失败");
        assert_eq!(calls.get(), 1, "(a1) 首次应打 1 次网络");
        assert!(ch1, "(a1) 首次 changed 必须为 true（调用方要渲染）");
        assert_eq!(v1["n"], 1);
        let (_, etag1, created1) = json_get_full(key).expect("应已落盘");
        assert_eq!(etag1.as_deref(), Some("\"etag-1\""), "(a1) ETag 必须存进 meta");

        // (a2) 缓存新鲜（TTL 未到）→ **网络 0 次**、changed=false、内容与时间戳都不动
        let c = calls.clone();
        let (v2, ch2) = json_cached_conditional(key, ttl, false, false, |_| true, move |_et| {
            let c = c.clone();
            async move {
                c.set(c.get() + 1);
                Ok(Cond::Fresh(json!({"n": 999}), None))
            }
        })
        .await
        .expect("命中缓存不该失败");
        assert_eq!(calls.get(), 1, "(a2) 新鲜缓存不许打网络");
        assert!(!ch2, "(a2) 命中缓存 changed=false（不许碰 UI）");
        assert_eq!(v2, v1, "(a2) 内容不该变");
        assert_eq!(json_get_full(key).unwrap().2, created1, "(a2) 命中缓存不该改时间戳");

        // (b) TTL 过期（ttl=1s）+ 假 304 → 网络 1 次、payload 不变、时间戳顺延
        //     sleep 2s：created/now 都是**整数秒**，1.1s 的差会被算成 1（不大于 1）→ 仍算新鲜
        std::thread::sleep(std::time::Duration::from_millis(2100));
        let c = calls.clone();
        let saw_etag = Rc::new(Cell::new(false));
        let se = saw_etag.clone();
        let (v3, ch3) = json_cached_conditional(key, 1, false, false, |_| true, move |et| {
            let c = c.clone();
            let se = se.clone();
            async move {
                c.set(c.get() + 1);
                se.set(et.as_deref() == Some("\"etag-1\""));
                Ok(Cond::NotModified)
            }
        })
        .await
        .expect("304 不该失败");
        assert_eq!(calls.get(), 2, "(b) 过期只探测 1 次");
        assert!(!ch3, "(b) 304 必须 changed=false（界面零变化）");
        assert_eq!(v3, v1, "(b) 304 不许换内容");
        assert!(saw_etag.get(), "(b) 条件请求必须带上存下的 ETag");
        assert!(
            json_get_full(key).unwrap().2 > created1,
            "(b) 304 后时间戳要顺延（TTL 重新计时）"
        );

        // (c) TTL 过期 + 假 200 → 网络 1 次、payload 替换、ETag 更新
        //     注意：(b) 的 304 刚把时间戳顺延过 → 这里必须再等过一个 TTL 周期才算"过期"
        std::thread::sleep(std::time::Duration::from_millis(2100));
        let c = calls.clone();
        let (v4, ch4) = json_cached_conditional(key, 1, false, false, |_| true, move |_et| {
            let c = c.clone();
            async move {
                c.set(c.get() + 1);
                Ok(Cond::Fresh(json!({"n": 2}), Some("\"etag-2\"".to_string())))
            }
        })
        .await
        .expect("200 不该失败");
        assert_eq!(calls.get(), 3, "(c) 过期 200 也是一次请求");
        assert!(ch4, "(c) 新内容 changed=true（调用方要替换 + 淡入）");
        assert_eq!(v4["n"], 2);
        assert_eq!(json_get_full(key).unwrap().1.as_deref(), Some("\"etag-2\""), "(c) ETag 要更新");

        // (d) 手动 force（缓存新鲜也去请求）仍带条件头；304 → 内容不变
        let c = calls.clone();
        let se2 = Rc::new(Cell::new(false));
        let se2c = se2.clone();
        let (v5, ch5) = json_cached_conditional(key, ttl, true, false, |_| true, move |et| {
            let c = c.clone();
            let se2c = se2c.clone();
            async move {
                c.set(c.get() + 1);
                se2c.set(et.as_deref() == Some("\"etag-2\""));
                Ok(Cond::NotModified)
            }
        })
        .await
        .expect("force + 304 不该失败");
        assert_eq!(calls.get(), 4, "(d) force 必须真发一次请求");
        assert!(se2.get(), "(d) force 也要带 If-None-Match");
        assert!(!ch5, "(d) force 收到 304 同样不许碰 UI");
        assert_eq!(v5["n"], 2, "(d) 304 内容不变");

        // (e) 失败重试 no_etag：不带条件头（请求里 etag=None）
        let c = calls.clone();
        let got_none = Rc::new(Cell::new(false));
        let gn = got_none.clone();
        let (v6, ch6) = json_cached_conditional(key, ttl, true, true, |_| true, move |et| {
            let c = c.clone();
            let gn = gn.clone();
            async move {
                c.set(c.get() + 1);
                gn.set(et.is_none());
                Ok(Cond::Fresh(json!({"n": 3}), None))
            }
        })
        .await
        .expect("no_etag 不该失败");
        assert!(got_none.get(), "(e) no_etag 分支必须不带 ETag（强制全量）");
        assert!(ch6 && v6["n"] == 3);
        assert_eq!(json_get_full(key).unwrap().1, None, "(e) 上游没给 ETag 时 meta 里就不该有");

        // (f) 200 但**内容没变**（ETag 弱相等：200 给 W/"x"，304 给 "x"）→ 只顺延时间戳、不换内容
        std::thread::sleep(std::time::Duration::from_millis(2100)); // 先让它过期
        json_put_full(key, &json!({"n": 4}), Some("\"x1\""));      // 本地存的是强 ETag
        std::thread::sleep(std::time::Duration::from_millis(2100));
        let created_before = json_get_full(key).unwrap().2;
        let c = calls.clone();
        let (v7, ch7) = json_cached_conditional(key, 1, false, false, |_| true, move |_et| {
            let c = c.clone();
            async move {
                c.set(c.get() + 1);
                // Cloudflare 压缩把 200 的强 ETag 改写成 W/"…"（同一个哈希）
                Ok(Cond::Fresh(json!({"n": 999}), Some("W/\"x1\"".to_string())))
            }
        })
        .await
        .expect("内容未变不该失败");
        assert!(!ch7, "(f) ETag 弱相等 = 内容没变 → changed 必须是 false（不许碰 UI）");
        assert_eq!(v7["n"], 4, "(f) 内容保持旧值，不被 999 覆盖");
        assert!(json_get_full(key).unwrap().2 > created_before, "(f) 时间戳要顺延");

        // ETag 弱比较的纯函数断言（两侧都可带 W/）
        assert!(etag_weak_eq(Some("W/\"a\""), Some("\"a\"")));
        assert!(etag_weak_eq(Some("\"a\""), Some("W/\"a\"")));
        assert!(etag_weak_eq(Some("W/\"a\""), Some("W/\"a\"")));
        assert!(!etag_weak_eq(Some("\"a\""), Some("\"b\"")));
        assert!(!etag_weak_eq(None, Some("\"a\"")));
        assert!(!etag_weak_eq(Some(""), Some("")));
    }
}
