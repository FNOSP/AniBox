//! 网盘媒体库：扫描 + 文件名解析 + 本地索引（lc-1890，设计见 docs/cloud-first-design.md §二）。
//!
//! 定位：把「网盘文件树」变成「可刮削的资源库」的第一步——先落**库索引**：
//!   · 只扫用户指定的媒体根目录（每盘可多个），深度受限 + 每目录页数受限，绝不全量递归（API 限额）；
//!   · 识别（零请求）：文件名解析 标题/年份/季集/集名，兼容中文主流命名（SxxEyy / 第x季第x集 / EPxx / `剧名 - S1E28: 集名`）；
//!   · TMDB 刮削、详情页接入、进度聚合在后续 lc 接到本索引上（cloud_metadata.json 与索引分离）。
//!
//! 存储：`app_data_dir/cloud-library.json`，凭据不落这里（凭据在各自既有文件）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::AppHandle;

/// 扫描预算（防 API 限额）：单次扫描最多发起的列目录请求次数
const MAX_LIST_CALLS: usize = 60;
/// 单个目录最多拉的分页数（cursor 翻页上限）
const MAX_PAGES_PER_DIR: usize = 2;
/// 单个目录最多收的条目数（防异常超大目录拖死扫描）
const MAX_ENTRIES_PER_DIR: usize = 300;
/// 扫描深度：root(0) → 子目录(1) → 孙目录(2)，共 3 层；第 3 层只收文件不再下钻
const MAX_DEPTH: usize = 2;

fn lib_path() -> Option<PathBuf> {
    crate::cloud::app_data_dir().map(|d| d.join("cloud-library.json"))
}

/// 用户为某个网盘指定的媒体根目录
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CloudRoot {
    pub fid: String,
    pub name: String,
}

/// 解析后的单条媒体（文件）
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CloudItem {
    pub provider: String,
    pub file_id: String,
    pub parent: String,
    pub name: String,
    #[serde(default)]
    pub size: u64,
    /// "movie" | "episode"
    pub kind: String,
    pub title: String,
    /// 剧集的集名（`剧名 - S1E28: 集名` 的冒号后段）；电影为空
    #[serde(default)]
    pub ep_title: String,
    #[serde(default)]
    pub year: u32,
    #[serde(default)]
    pub season: u32,
    #[serde(default)]
    pub episode: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CloudLib {
    pub provider: String,
    #[serde(default)]
    pub roots: Vec<CloudRoot>,
    #[serde(default)]
    pub scanned_at_ms: u64,
    #[serde(default)]
    pub items: Vec<CloudItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct CloudLibraryStore {
    #[serde(default)]
    libs: Vec<CloudLib>,
}

fn load_store() -> CloudLibraryStore {
    let Some(p) = lib_path() else { return CloudLibraryStore::default() };
    let Ok(s) = std::fs::read_to_string(p) else { return CloudLibraryStore::default() };
    serde_json::from_str(&s).unwrap_or_default()
}

fn save_store(st: &CloudLibraryStore) -> Result<(), String> {
    let Some(p) = lib_path() else { return Err("app_data_dir 未初始化".into()) };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let s = serde_json::to_string_pretty(st).map_err(|e| format!("序列化库索引失败: {e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写库索引失败: {e}"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ───────────────────────── 文件名解析（识别，零请求） ─────────────────────────

#[derive(Debug, PartialEq)]
pub struct Parsed {
    pub title: String,
    pub ep_title: String,
    pub year: u32,
    pub season: u32,
    pub episode: u32,
    pub is_episode: bool,
}

/// 分辨率/来源组之类的技术标签（集名兜底时不把它误当集名）
fn is_tag_ish(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    s.chars().count() <= 24
        && [
            "1080", "720", "2160", "4k", "8k", "hevc", "h26", "x26", "avc", "web", "bluray", "blu-ray", "hdr", "remux", "dv",
        ]
        .iter()
        .any(|k| l.contains(k))
}

/// 清洗标题：去首尾与连续的分隔符（. _ 空格 - ～）
fn clean_title(s: &str) -> String {
    let t = s.trim();
    let t = t.trim_matches(|c| matches!(c, '.' | '_' | '-' | ' ' | '～' | '~' | '·'));
    let mut out = String::new();
    let mut last_space = false;
    for ch in t.chars() {
        let is_sep = matches!(ch, '.' | '_');
        if is_sep {
            if !last_space && !out.is_empty() {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(ch);
            last_space = ch == ' ';
        }
    }
    out.trim().to_string()
}

/// 从文件名解析 标题/年份/季集/集名。规则从宽、宁缺毋滥：
/// 认不出的字段留空，由后续刮削/手动匹配兜底。
///
/// 兼容形态：
///   · `剧名 - S1E28: 集名.ext`（mpv 容器标题同款）→ title=剧名，ep_title=集名
///   · `剧名 S01E24 1080p HEVC.ext` / `[组] 剧名 EP05 [1080p].ext`
///   · `剧名 第24集 4K.ext` / `剧名 第2季 游郭篇 - S02E03.ext`
///   · 电影：`你的名字.(2016).1080p...ext` / `千与千寻.ext`
pub fn parse_name(raw: &str) -> Parsed {
    let stem = raw.rsplit_once('.').map_or(raw.to_string(), |(a, b)| {
        // 扩展名只剥一次且必须是常见视频后缀，避免「乱马1/2」这种名字被误剥
        if matches!(
            b.to_ascii_lowercase().as_str(),
            "mkv" | "mp4" | "avi" | "rmvb" | "flv" | "ts" | "m2ts" | "webm" | "wmv" | "mov" | "mpg" | "iso"
        ) {
            a.to_string()
        } else {
            raw.to_string()
        }
    });

    // 注：Rust regex 不支持 lookahead —— 边界字符改用「前缀捕获 + 非捕获尾巴」，
    // 尾巴消费的分隔符只影响 m.end() 之后的集名提取（已 trim），不影响标题截断位置。
    let ep_marker = regex::Regex::new(
        r"(?i)[Ss](\d{1,2})\s*[Ee×xX](\d{1,3})|(?:^|[\s._\-\[])[Ee][Pp]?\s*(\d{1,4})(?:[\s._\-\]]|$)|第\s*(\d{1,4})\s*[集话話]",
    )
    .expect("ep regex");
    let season_re = regex::Regex::new(
        r"(?i)第\s*(\d{1,2})\s*季|(?:^|[\s._\-\[])S(\d{1,2})(?:[\s._\-\]Ee]|$)|season\s*(\d{1,2})",
    )
    .expect("season regex");
    let year_re = regex::Regex::new(r"(?:^|[^\d])((?:19|20)\d{2})(?:[^\d]|$)").expect("year regex");

    // ① `主标题 - 剧集尾段`：尾段含季集标记时，主标题取左侧、季集/集名解析右侧
    let mut title_src = stem.clone();
    let mut src = stem.clone(); // 季集解析的目标串
    if let Some(pos) = stem.find(" - ") {
        let right = &stem[pos + 3..];
        if ep_marker.is_match(right) {
            src = right.to_string();
            title_src = stem[..pos].to_string();
        }
    }

    // ② 季集
    let mut is_episode = false;
    let mut season: u32 = 0;
    let mut episode: u32 = 0;
    let mut ep_title = String::new();
    if let Some(m) = ep_marker.find(&src) {
        if let Some(caps) = ep_marker.captures_at(&src, m.start()) {
            let s1 = caps.get(1).and_then(|g| g.as_str().parse::<u32>().ok());
            if let Some(e) = caps
                .get(2)
                .or_else(|| caps.get(3))
                .or_else(|| caps.get(4))
                .and_then(|g| g.as_str().parse::<u32>().ok())
            {
                is_episode = true;
                episode = e;
                season = s1.unwrap_or_else(|| {
                    season_re
                        .captures(&title_src)
                        .and_then(|sc| {
                            sc.get(1)
                                .or_else(|| sc.get(2))
                                .or_else(|| sc.get(3))
                                .and_then(|g| g.as_str().parse::<u32>().ok())
                        })
                        .unwrap_or(1)
                });
                // 非拆分形态：标题在标记左侧
                if src == title_src {
                    title_src.truncate(m.start());
                }
                // ③ 集名：标记后的冒号段（`S1E28: 格斗茶道`）
                let after = src[m.end()..]
                    .trim_start_matches([':', '：', ' ', '-', '.']);
                if !after.is_empty() && !is_tag_ish(after) {
                    ep_title = clean_title(after.split(['(', '[']).next().unwrap_or(after));
                }
            }
        }
    } else if let Some(sc) = season_re.captures(&src) {
        if let Some(g) = sc.get(1).or_else(|| sc.get(2)).or_else(|| sc.get(3)) {
            if let Ok(s) = g.as_str().parse::<u32>() {
                season = s;
                if src == title_src {
                    let st_ = sc.get(0).unwrap().start();
                    title_src.truncate(st_);
                }
            }
        }
    }

    // ④ 年份（在标题串上找，截断其后的技术尾巴）
    let mut year: u32 = 0;
    if let Some(yc) = year_re.captures(&title_src) {
        if let Ok(y) = yc.get(1).unwrap().as_str().parse::<u32>() {
            year = y;
            let mut cut = yc.get(1).unwrap().start();
            if title_src[..cut].ends_with('(') {
                cut -= 1;
            }
            title_src.truncate(cut);
        }
    }

    // ⑤ 标题清洗 + 剥前缀字幕组标签（[SubGroup] 剧名 / (Group) 剧名）
    let mut title = clean_title(&title_src);
    loop {
        let t = title.trim();
        if t.starts_with('[') {
            if let Some(end) = t.find(']') {
                title = clean_title(&t[end + 1..]);
                continue;
            }
        }
        if t.starts_with('(') {
            if let Some(end) = t.find(')') {
                title = clean_title(&t[end + 1..]);
                continue;
            }
        }
        break;
    }
    if title.is_empty() {
        title = clean_title(&stem);
    }
    Parsed {
        title,
        ep_title,
        year,
        season,
        episode,
        is_episode,
    }
}

fn is_video_name(name: &str) -> bool {
    let Some(ext) = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()) else {
        return false;
    };
    matches!(
        ext.as_str(),
        "mkv" | "mp4" | "avi" | "rmvb" | "flv" | "ts" | "m2ts" | "webm" | "wmv" | "mov" | "mpg"
    )
}

// ───────────────────────── 扫描 ─────────────────────────

/// 列一个目录（自动翻页，页数受限）。返回 (items, next_fid)。
async fn list_dir_all(
    app: &AppHandle,
    provider: &str,
    fid: &str,
    budget: &mut usize,
) -> Result<(Vec<Value>, String), String> {
    let mut out: Vec<Value> = Vec::new();
    let mut cursor = String::new();
    loop {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let v = crate::cloud::cloud_list(
            app.clone(),
            provider.to_string(),
            Some(fid.to_string()),
            Some(cursor.clone()),
        )
        .await?;
        let items = v.get("items").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        for it in items {
            if out.len() >= MAX_ENTRIES_PER_DIR {
                break;
            }
            out.push(it);
        }
        cursor = v
            .get("nextCursor")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if cursor.is_empty() || out.len() >= MAX_ENTRIES_PER_DIR {
            break;
        }
    }
    Ok((out, cursor))
}

async fn scan_provider(app: &AppHandle, lib: &mut CloudLib) -> (usize, usize, usize) {
    let mut items: Vec<CloudItem> = Vec::new();
    let mut budget = MAX_LIST_CALLS;
    // BFS：queue 里放 (fid, name, depth)
    let mut queue: std::collections::VecDeque<(String, String, usize)> = std::collections::VecDeque::new();
    for r in &lib.roots {
        queue.push_back((r.fid.clone(), r.name.clone(), 0));
    }
    while let Some((fid, parent, depth)) = queue.pop_front() {
        if budget == 0 || items.len() >= MAX_ENTRIES_PER_DIR * MAX_LIST_CALLS {
            break;
        }
        let Ok((entries, _)) = list_dir_all(app, &lib.provider, &fid, &mut budget).await else {
            continue;
        };
        for e in entries {
            let name = e.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let file_id = e
                .get("fileId")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .or_else(|| e.get("file_id").and_then(|x| x.as_str()).map(|s| s.to_string()))
                .unwrap_or_default();
            if file_id.is_empty() || name.is_empty() {
                continue;
            }
            let is_dir = e.get("isDir").and_then(|x| x.as_bool()).unwrap_or(false);
            if is_dir {
                // 深度受限下钻：跳过明显不是媒体目录的（样本/固件等不看扩展名，全下钻由预算兜底）
                if depth < MAX_DEPTH {
                    queue.push_back((file_id, name, depth + 1));
                }
                continue;
            }
            if !is_video_name(&name) {
                continue;
            }
            if items.iter().any(|it| it.file_id == file_id) {
                continue;
            }
            let p = parse_name(&name);
            items.push(CloudItem {
                provider: lib.provider.clone(),
                file_id,
                parent: parent.clone(),
                name,
                size: e.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                kind: if p.is_episode { "episode" } else { "movie" }.into(),
                title: p.title,
                ep_title: p.ep_title,
                year: p.year,
                season: p.season,
                episode: p.episode,
            });
        }
    }
    let movies = items.iter().filter(|i| i.kind == "movie").count();
    let episodes = items.iter().filter(|i| i.kind == "episode").count();
    lib.items = items;
    (lib.items.len(), movies, episodes)
}

fn upsert_lib<'a>(st: &'a mut CloudLibraryStore, provider: &str) -> &'a mut CloudLib {
    if let Some(i) = st.libs.iter().position(|l| l.provider == provider) {
        return &mut st.libs[i];
    }
    st.libs.push(CloudLib {
        provider: provider.to_string(),
        ..Default::default()
    });
    st.libs.last_mut().unwrap()
}

// ───────────────────────── 命令 ─────────────────────────

/// 库总览（每盘：根目录、条目数、上次扫描时间）
#[tauri::command]
pub async fn cloud_library_all() -> Result<Value, String> {
    let st = load_store();
    let libs: Vec<Value> = st
        .libs
        .iter()
        .map(|l| {
            json!({
                "provider": l.provider,
                "roots": l.roots,
                "scannedAtMs": l.scanned_at_ms,
                "count": l.items.len(),
            })
        })
        .collect();
    Ok(json!({ "libs": libs }))
}

/// 某盘的条目索引（给后续详情/刮削 UI 用）
#[tauri::command]
pub async fn cloud_library_items(provider: String) -> Result<Value, String> {
    let st = load_store();
    let lib = st.libs.iter().find(|l| l.provider == provider);
    Ok(json!({
        "provider": provider,
        "roots": lib.map(|l| l.roots.clone()).unwrap_or_default(),
        "scannedAtMs": lib.map(|l| l.scanned_at_ms).unwrap_or(0),
        "items": lib.map(|l| l.items.clone()).unwrap_or_default(),
        // 全局元数据表（key=media|归一化标题，跨盘共享同名匹配）
        "meta": load_meta(),
    }))
}

/// [lc-1890] 追加一个媒体根目录（去重；不触发扫描）。给「网盘接入」页的「加入媒体库」按钮用。
#[tauri::command]
pub async fn cloud_library_add_root(provider: String, fid: String, name: String) -> Result<Value, String> {
    let fid = fid.trim().to_string();
    if fid.is_empty() {
        return Err("目录为空，请先进入要入库的媒体目录".into());
    }
    let mut st = load_store();
    let dup = {
        let lib = upsert_lib(&mut st, &provider);
        let dup = lib.roots.iter().any(|r| r.fid == fid);
        if !dup {
            lib.roots.push(CloudRoot {
                fid,
                name: if name.trim().is_empty() { "根目录".into() } else { name },
            });
        }
        dup
    };
    save_store(&st)?;
    Ok(json!({ "ok": true, "duplicate": dup }))
}

/// 设置某盘的媒体根目录（覆盖式；不触发扫描）
#[tauri::command]
pub async fn cloud_library_set_roots(provider: String, roots: Vec<Value>) -> Result<Value, String> {
    let mut st = load_store();
    let count = {
        let lib = upsert_lib(&mut st, &provider);
        lib.roots = roots
            .into_iter()
            .map(|r| CloudRoot {
                fid: r.get("fid").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                name: r.get("name").and_then(|x| x.as_str()).unwrap_or("根目录").to_string(),
            })
            .filter(|r| !r.fid.is_empty())
            .collect();
        lib.roots.len()
    };
    save_store(&st)?;
    Ok(json!({ "ok": true, "count": count }))
}

/// 扫描某盘全部根目录 → 重建索引（懒扫描预算内，超预算的部分下次扫描补）
#[tauri::command]
pub async fn cloud_library_scan(app: AppHandle, provider: String) -> Result<Value, String> {
    let mut st = load_store();
    let roots = {
        let lib = upsert_lib(&mut st, &provider);
        lib.roots.clone()
    };
    if roots.is_empty() {
        return Err("请先选择媒体目录（在网盘接入页进入目录后点「加入媒体库」）".into());
    }
    let mut lib = CloudLib {
        provider: provider.clone(),
        roots,
        scanned_at_ms: 0,
        items: Vec::new(),
    };
    let (count, movies, episodes) = scan_provider(&app, &mut lib).await;
    lib.scanned_at_ms = now_ms();
    st.libs.retain(|l| l.provider != provider);
    st.libs.push(lib);
    save_store(&st)?;
    Ok(json!({ "ok": true, "count": count, "movies": movies, "episodes": episodes }))
}

/// 移除某盘的库（含索引；不动网盘里的文件）
#[tauri::command]
pub async fn cloud_library_remove(provider: String) -> Result<Value, String> {
    let mut st = load_store();
    st.libs.retain(|l| l.provider != provider);
    save_store(&st)?;
    Ok(json!({ "ok": true }))
}

// ───────────────────────── TMDB 刮削（按标题去重，结果与索引分离） ─────────────────────────

/// 单标题的刮削结果。key = `媒体类型|归一化标题`（与具体 file_id 无关——同剧多集共享一条）。
/// 落 `cloud-metadata.json`：重新扫描不丢刮削结果（[lc-1889] 设计 §二.3）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CloudMeta {
    pub tmdb_id: i64,
    pub media_type: String,
    pub title: String,
    #[serde(default)]
    pub poster: String,
    #[serde(default)]
    pub year: u32,
}

fn meta_path() -> Option<PathBuf> {
    crate::cloud::app_data_dir().map(|d| d.join("cloud-metadata.json"))
}

type MetaMap = std::collections::BTreeMap<String, CloudMeta>;

fn load_meta() -> MetaMap {
    let Some(p) = meta_path() else { return MetaMap::new() };
    let Ok(s) = std::fs::read_to_string(p) else { return MetaMap::new() };
    serde_json::from_str(&s).unwrap_or_default()
}

fn save_meta(m: &MetaMap) -> Result<(), String> {
    let Some(p) = meta_path() else { return Err("app_data_dir 未初始化".into()) };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let s = serde_json::to_string_pretty(m).map_err(|e| format!("序列化元数据失败: {e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写元数据失败: {e}"))
}

fn normalize_title_key(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, ' ' | '.' | '_' | '-' | '～' | '~' | '·' | '　'))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn meta_key(title: &str, media: &str) -> String {
    format!("{}|{}", media, normalize_title_key(title))
}

/// 对某盘索引做 TMDB 刮削：按 (媒体类型, 归一化标题) 去重逐个搜；
/// 已有结果的默认跳过（force=true 重刮全部）；先带年份搜，0 结果退无年重试。
#[tauri::command]
pub async fn cloud_library_scrape(provider: String, force: Option<bool>) -> Result<Value, String> {
    let st = load_store();
    let Some(lib) = st.libs.iter().find(|l| l.provider == provider) else {
        return Err("该网盘还没有库索引，请先「加入媒体库」扫描".into());
    };
    // 分组：key -> (media, 展示标题, 年份)
    let mut groups: std::collections::BTreeMap<String, (String, String, u32)> = Default::default();
    for it in &lib.items {
        if it.title.trim().is_empty() {
            continue;
        }
        let media = if it.kind == "movie" { "movie" } else { "tv" }.to_string();
        let key = meta_key(&it.title, &media);
        groups.entry(key).or_insert_with(|| (media, it.title.clone(), it.year));
    }
    let force = force.unwrap_or(false);
    let mut meta = load_meta();
    let mut matched = 0usize;
    let mut skipped = 0usize;
    let mut unmatched: Vec<String> = Vec::new();
    for (key, (media, title, year)) in &groups {
        if !force && meta.contains_key(key) {
            skipped += 1;
            continue;
        }
        let mut hit: Option<CloudMeta> = None;
        for with_year in [true, false] {
            if with_year && *year == 0 {
                continue; // 没解析到年份就只搜一次（无年）
            }
            let mut arg = json!({ "query": title, "mediaType": media });
            if with_year {
                arg["year"] = json!(year);
            }
            if let Ok(res) = crate::tmdb::tmdb_search(arg).await {
                if let Some(first) = res.get("items").and_then(|x| x.as_array()).and_then(|a| a.first()) {
                    let id = first.get("tmdbId").and_then(|v| v.as_i64()).unwrap_or(0);
                    if id != 0 {
                        hit = Some(CloudMeta {
                            tmdb_id: id,
                            media_type: media.clone(),
                            title: first.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            poster: first.get("poster").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            year: first.get("year").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                        });
                        break;
                    }
                }
            }
        }
        if let Some(m) = hit {
            meta.insert(key.clone(), m);
            matched += 1;
        } else {
            unmatched.push(title.clone());
        }
    }
    save_meta(&meta)?;
    Ok(json!({ "matched": matched, "skipped": skipped, "unmatched": unmatched }))
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_show_dash_episode_title() {
        let p = parse_name("乱马1/2 - S1E28: 格斗茶道.mp4");
        assert_eq!(p.title, "乱马1/2");
        assert_eq!(p.ep_title, "格斗茶道");
        assert_eq!((p.season, p.episode), (1, 28));
        assert!(p.is_episode);
    }

    #[test]
    fn parse_plain_sxxeyy() {
        let p = parse_name("葬送的芙莉莲 S01E24 1080p HEVC.mkv");
        assert_eq!(p.title, "葬送的芙莉莲");
        assert_eq!((p.season, p.episode), (1, 24));
        assert!(p.is_episode);
        assert!(!p.title.contains("1080"));
    }

    #[test]
    fn parse_cn_ji() {
        let p = parse_name("铃芽之旅 第24集 4K.mp4");
        assert!(p.is_episode);
        assert_eq!(p.episode, 24);
        assert_eq!(p.title, "铃芽之旅");
    }

    #[test]
    fn parse_movie_with_year() {
        let p = parse_name("你的名字.(2016).1080p.BluRay.x264.mkv");
        assert!(!p.is_episode);
        assert_eq!(p.year, 2016);
        assert_eq!(p.title, "你的名字");
    }

    #[test]
    fn parse_movie_no_year() {
        let p = parse_name("千与千寻.mkv");
        assert!(!p.is_episode);
        assert_eq!(p.title, "千与千寻");
        assert_eq!(p.year, 0);
    }

    #[test]
    fn parse_epxx_form() {
        let p = parse_name("[SubGroup] 孤独摇滚 EP05 [1080p].mp4");
        assert!(p.is_episode);
        assert_eq!(p.episode, 5);
        assert!(p.title.contains("孤独摇滚"), "title={}", p.title);
    }

    #[test]
    fn parse_season_cn() {
        let p = parse_name("鬼灭之刃 第2季 游郭篇 - S02E03.mkv");
        assert!(p.is_episode);
        assert_eq!(p.season, 2);
        assert_eq!(p.episode, 3);
        assert_eq!(p.title, "鬼灭之刃 第2季 游郭篇");
    }

    #[test]
    fn ext_strip_only_known_video_ext() {
        let p = parse_name("乱马1/2 - S1E28: 格斗茶道");
        // 没有扩展名 → 不剥，但解析仍应工作
        assert!(p.is_episode);
        assert_eq!(p.title, "乱马1/2");
    }
}
