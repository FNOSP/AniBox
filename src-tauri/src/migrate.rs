//! 旧版本数据目录迁移（**启动时、Tauri Builder 之前**自动执行）。
//!
//! 本模块处理两类历史用户：
//!
//! 1. **Fntv-Plus Tauri2 版**（identifier `com.fntv.plus.tauri2` → `com.anibox.app`）
//!    —— 两者数据结构完全一致，整目录复制即可。
//! 2. **Fntv-Plus Electron 版 3.9.0 及更早**（userData `%APPDATA%\fntv`，安装目录
//!    `%LOCALAPPDATA%\Programs\Fntv-Plus`）—— 数据结构不同，需要**字段级映射**：
//!    `account/domain/accessCode/exitMode/...` → `username/server/access_code/exit_mode/...`，
//!    旧访问码/密码是 AES-256-CBC(hex)，新配置是 AES-256-GCM(`enc:v1:`)，迁移时解密再重加密。
//!
//! ## 时机（硬约束）
//! WebView2 一创建就会锁住 `%LOCALAPPDATA%\<identifier>\EBWebView`（leveldb 被进程持有），
//! 那时再搬 LOCALAPPDATA 必然失败。所以 [`run`] 必须在 `tauri::Builder` **之前**调用。
//!
//! ## 安全策略
//! * **不覆盖已有数据**：目标文件/目录已存在就跳过（逐文件判断）。
//! * **不删除源数据**：迁移是"抄一份过去"，旧目录原样保留，用户可回滚。
//! * **原子落位**：整目录迁移先复制到 `.tmp`，最后 `rename`；中途崩溃只留 tmp。
//! * **失败不阻塞启动**：所有 IO 错误只记录，返回报告由调用方写日志。
//! * **幂等**：完成后在 `%APPDATA%\com.anibox.app\migration.json` 写标记
//!   （来源路径 / 版本 / 时间 / 结果），再次启动直接跳过。
//! * **版本闸门**：Electron 旧版只在 `<= 3.9.0` 时迁移；更高版本不重复迁移。
//!   版本从旧安装包的 `resources/app.asar` 内 `package.json` 读取（失败时视为旧版）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// 旧 Tauri2 identifier（改动前的值，写死在这里：迁移是一次性的，读配置反而增加失败面）
const OLD_ID: &str = "com.fntv.plus.tauri2";
/// 新 identifier（与 tauri.conf.json 的 identifier 必须一致）
const NEW_ID: &str = "com.anibox.app";
/// 迁移标记文件名（落在新数据目录根部）
const MARKER_FILE: &str = "migration.json";
/// 旧 Electron 数据的"只读归档"子目录（不参与新功能读取，只为不丢数据）
const LEGACY_DIR: &str = "legacy-electron";
/// 允许迁移的旧版本上限（含）
const LEGACY_MAX_VERSION: (u64, u64, u64) = (3, 9, 0);
/// 旧图片缓存迁移上限：超过就跳过（避免启动时复制几百 MB 阻塞）
const MAX_LEGACY_CACHE_BYTES: u64 = 512 * 1024 * 1024;
/// 旧访问码 / 历史密码的 AES-256-CBC 密钥种子（与 Electron 版 `fn_config/config.ts` 一致）
const LEGACY_CBC_KEY_SEED: &str = "U2XDcFsV6rdTE9wB5ZHvy6BW9hBTKJ1H";

// 旧 Electron 版把「观影记录 / 评分 / 海报缓存」存在 Chromium localStorage（LevelDB）里。
// 新版本是 history.json / ratings.json，所以这里从 LevelDB 原始字节里把值抠出来再映射。
const LS_EVENTS: &str = "fntv_wh_events_v1";
const LS_LOCAL: &str = "fntv_wh_local_v1";
const LS_RATINGS: &str = "fntv_wh_ratings_v1";
const LS_POSTERS: &str = "fntv_wh_poster_cache_v1";
/// 2000-01-01 前的时间戳视为脏数据（与 history.rs 的 MIN_VALID_MS 一致）
const MIN_VALID_TS: u64 = 946_684_800_000;
/// LevelDB 扫描总量上限（避免异常大的旧目录拖慢启动）
const MAX_LEVELDB_SCAN_BYTES: u64 = 128 * 1024 * 1024;

/// 迁移结果（只用于日志，调用方不需要分支处理）
pub struct Report {
    pub appdata: Option<String>,
    pub localappdata: Option<String>,
    pub electron: Option<String>,
}

fn roaming() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

fn local() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ───────────────────────── Tauri2 → AniBox（整目录） ─────────────────────────

/// 新目录里是否已有"核心数据"（决定是否还能并入旧 Tauri2 数据）。
/// 安装器可能提前创建了 `%APPDATA%\com.anibox.app` 并只放了一个 `bili_cookie.txt`，
/// 这种目录不能视为"已完成迁移"，否则 Tauri2 老用户的配置会被整目录跳过。
fn has_core_data(dir: &Path) -> bool {
    ["config.json", "history.json", "watch_days.json"]
        .iter()
        .any(|n| dir.join(n).is_file())
}

/// 把 src 合并进已存在的 dst（逐文件：目标存在就跳过，不覆盖）。
fn merge_dir(src: &Path, dst: &Path) -> Result<u64, String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建 {} 失败: {e}", dst.display()))?;
    let mut n = 0u64;
    for entry in std::fs::read_dir(src).map_err(|e| format!("读 {} 失败: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        if ft.is_dir() {
            n += merge_dir(&from, &to)?;
        } else if ft.is_file() && !to.exists() {
            std::fs::copy(&from, &to).map_err(|e| format!("复制 {} 失败: {e}", from.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

/// 把一个目录搬到新位置。
/// 返回 Some(描述) = 做了迁移；None = 无需迁移（新目录已在 / 旧目录不在 / 环境变量缺失）。
fn move_dir(root: &Path, label: &str) -> Option<String> {
    let old = root.join(OLD_ID);
    let new = root.join(NEW_ID);
    // 新目录已存在且已有核心数据 ⇒ 已经迁移过（或用户全新安装后跑过），绝不再动
    if new.exists() && has_core_data(&new) {
        return None;
    }
    // 旧目录不存在 ⇒ 全新用户，没什么可搬
    if !old.exists() {
        return None;
    }
    // 先复制到同目录下的临时名，最后一步 rename 原子落位（防"半份迁移"）
    let tmp = root.join(format!("{NEW_ID}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    match copy_dir(&old, &tmp) {
        Ok(n) => {
            // 目标已存在但没有核心数据（例如安装器只放了 bili_cookie.txt）：
            // 合并进去，保留已有文件；否则原子 rename。
            let result = if new.exists() {
                match merge_dir(&tmp, &new) {
                    Ok(0) => Ok(n),
                    Ok(m) => Ok(n.max(m)),
                    Err(e) => Err(e),
                }
            } else {
                match std::fs::rename(&tmp, &new) {
                    Ok(()) => Ok(n),
                    Err(e) => Err(format!("rename: {e}")),
                }
            };
            match result {
                Ok(count) => {
                    let _ = std::fs::remove_dir_all(&tmp);
                    write_marker_entry(
                        &new,
                        json!({
                            "kind": "tauri2",
                            "path": old.display().to_string(),
                            "version": Value::Null,
                            "result": "ok",
                            "copied_files": count,
                            "skipped": [],
                            "errors": [],
                        }),
                    );
                    Some(format!("{label}: 复制 {count} 个文件"))
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&tmp);
                    Some(format!("{label}: 迁移失败（{e}）—— 将按全新安装启动"))
                }
            }
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Some(format!("{label}: 迁移失败（{e}）—— 将按全新安装启动"))
        }
    }
}

/// 递归复制（只处理文件与目录；不做符号链接跟随 —— 这两个目录里不该有链接）
fn copy_dir(src: &Path, dst: &Path) -> Result<u64, String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建 {} 失败: {e}", dst.display()))?;
    let mut n = 0u64;
    for entry in std::fs::read_dir(src).map_err(|e| format!("读 {} 失败: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        if ft.is_dir() {
            n += copy_dir(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to).map_err(|e| format!("复制 {} 失败: {e}", from.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

// ───────────────────────── Electron 3.9.0 及更早 → AniBox ─────────────────────────

/// 旧的 Electron 用户数据目录候选（按优先级）。
fn legacy_electron_dirs(roaming_root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if p.is_dir() && !out.contains(&p) {
            out.push(p);
        }
    };
    // 明确候选：Electron 的 app name 是 package.json 的 name（fntv），
    // 部分版本 / 打包方式可能用 productName（Fntv-Plus）。
    for name in ["fntv", "Fntv-Plus", "FntvPlus"] {
        push(roaming_root.join(name));
    }
    // 兜底扫描：名字含 fntv 的目录（排除 Tauri2 标识、更新缓存、开发版 .fntv-dev）
    if let Ok(rd) = std::fs::read_dir(roaming_root) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if !name.contains("fntv") {
                continue;
            }
            if name.contains("tauri") || name.contains("updater") || name.starts_with(".fntv-dev") {
                continue;
            }
            push(e.path());
        }
    }
    drop(push);
    out
}

/// 目标目录里是否已有某类迁移的成功标记（幂等判据）。
fn marker_has_kind(new_dir: &Path, kind: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(new_dir.join(MARKER_FILE)) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    v.get("sources")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter().any(|e| {
                e.get("kind").and_then(Value::as_str) == Some(kind)
                    && matches!(
                        e.get("result").and_then(Value::as_str),
                        Some("ok") | Some("skipped")
                    )
            })
        })
        .unwrap_or(false)
}

/// 写入/追加一条迁移标记（原子写；Windows 下 rename 不能覆盖，先删再改名）。
fn write_marker_entry(new_dir: &Path, entry: Value) {
    let path = new_dir.join(MARKER_FILE);
    let mut root = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({ "schema": 1, "sources": [] }));
    if !root.get("sources").map(Value::is_array).unwrap_or(false) {
        root["sources"] = json!([]);
    }
    if let Some(arr) = root.get_mut("sources").and_then(Value::as_array_mut) {
        arr.push(entry);
    }
    root["schema"] = json!(1);
    root["completed_at_unix"] = json!(now_unix());
    let Ok(text) = serde_json::to_string_pretty(&root) else {
        return;
    };
    if std::fs::create_dir_all(new_dir).is_err() {
        return;
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Electron 旧数据迁移（可在测试中直接传入临时 roaming 根目录）。
fn migrate_electron_at(roaming_root: &Path) -> Option<String> {
    let new_dir = roaming_root.join(NEW_ID);
    // 已迁移过 ⇒ 直接返回（幂等）
    if marker_has_kind(&new_dir, "electron") {
        return None;
    }
    let dirs = legacy_electron_dirs(roaming_root);
    if dirs.is_empty() {
        return None;
    }
    if std::fs::create_dir_all(&new_dir).is_err() {
        return Some("Electron: 无法创建新数据目录，跳过".into());
    }

    let mut copied = 0u64;
    let mut skipped: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut sources: Vec<Value> = Vec::new();

    for old in &dirs {
        let version = detect_legacy_version(old);
        if !should_migrate_version(version.as_deref()) {
            skipped.push(format!(
                "{}: 版本 {} > 3.9.0，跳过",
                old.display(),
                version.as_deref().unwrap_or("unknown")
            ));
            continue;
        }

        // 1) 配置字段映射：目标不存在才写（绝不覆盖新数据）
        let new_cfg = new_dir.join("config.json");
        if new_cfg.exists() {
            skipped.push(format!("{}: 目标 config.json 已存在，不覆盖", old.display()));
        } else if let Some(mapped) = map_config_file(&old.join("config.json")) {
            match write_json_atomic(&new_cfg, &mapped) {
                Ok(()) => copied += 1,
                Err(e) => errors.push(format!("写 config.json 失败: {e}")),
            }
        }

        // 2) 旧 localStorage 里的观影记录 / 评分 → history.json / ratings.json
        //    （Electron 版没有 history.json；数据在 Chromium LevelDB 的 fntv_wh_* 键里）
        if !new_dir.join("history.json").exists() {
            if let Some(hist) = legacy_history_from_dir(old) {
                match write_json_atomic(&new_dir.join("history.json"), &hist) {
                    Ok(()) => copied += 1,
                    Err(e) => errors.push(format!("写 history.json 失败: {e}")),
                }
            }
        }
        if !new_dir.join("ratings.json").exists() {
            if let Some(ratings) = legacy_ratings_from_dir(old) {
                match write_json_atomic(&new_dir.join("ratings.json"), &ratings) {
                    Ok(()) => copied += 1,
                    Err(e) => errors.push(format!("写 ratings.json 失败: {e}")),
                }
            }
        }

        // 3) 旧配置原样归档（含加密字段，便于追溯/回滚）
        if copy_file_if_absent(
            &old.join("config.json"),
            &new_dir.join(LEGACY_DIR).join("config.json"),
        )
        .unwrap_or(false)
        {
            copied += 1;
        }

        // 3) 自定义登录背景（旧版 settings:pick-login-bg 的落盘目录）
        copied += copy_dir_if_absent(&old.join("login-bg"), &new_dir.join("login-bg")).unwrap_or(0);

        // 4) 自定义媒体库封面（若旧版/旧 Tauri2 有）
        copied += copy_dir_if_absent(&old.join("library-covers"), &new_dir.join("library-covers"))
            .unwrap_or(0);
        if copy_file_if_absent(
            &old.join("library-covers.json"),
            &new_dir.join("library-covers.json"),
        )
        .unwrap_or(false)
        {
            copied += 1;
        }

        // 5) 图片缓存：新缓存引擎（sha256 + .bin/.meta）读不了旧格式（sha1 + data-uri .txt），
        //    所以放进 legacy-electron/ 归档，保证"数据不丢"但不参与新缓存。
        let dst_cache = new_dir.join("cache").join(LEGACY_DIR);
        let old_img = old.join("cache").join("img");
        if old_img.is_dir() {
            if dir_size(&old_img) > MAX_LEGACY_CACHE_BYTES {
                skipped.push(format!(
                    "{}: cache/img 超过 {}MB，跳过",
                    old.display(),
                    MAX_LEGACY_CACHE_BYTES >> 20
                ));
            } else {
                copied += copy_dir_if_absent(&old_img, &dst_cache.join("img")).unwrap_or(0);
            }
        }
        if let Ok(rd) = std::fs::read_dir(old.join("cache")) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("json") {
                    if copy_file_if_absent(&p, &dst_cache.join(e.file_name())).unwrap_or(false) {
                        copied += 1;
                    }
                }
            }
        }

        // 6) 旧功能的状态文件（新版本暂不读取，但归档保存，避免用户数据丢失）
        for name in [
            "playback-speed.json",
            "skip-manual.json",
            "trakt_auth.json",
            "danmu-api-mem.json",
            "bangumi_subject_cache.json",
            "bangumi_subject_meta.json",
            "bangumi_ep_descs.json",
            "bangumi_sync_status.json",
            "douban_id_cache.json",
            "douban_collect_cache.json",
            "watch_history_cache.json",
        ] {
            if copy_file_if_absent(&old.join(name), &new_dir.join(LEGACY_DIR).join(name))
                .unwrap_or(false)
            {
                copied += 1;
            }
        }

        sources.push(json!({
            "path": old.display().to_string(),
            "version": version,
        }));
    }

    // 7) 弹幕 Cookie：旧版写在安装目录（uosc_danmaku 脚本目录）里，而不是 userData。
    //    安装器卸载旧版前会先复制一份到新数据目录；这里再兜底一次（旧程序若还在）。
    if let Some(cookie) = find_old_bili_cookie() {
        if copy_file_if_absent(&cookie, &new_dir.join("bili_cookie.txt")).unwrap_or(false) {
            copied += 1;
        }
    }

    let result = if sources.is_empty() {
        "skipped"
    } else if errors.is_empty() {
        "ok"
    } else {
        "partial"
    };
    write_marker_entry(
        &new_dir,
        json!({
            "kind": "electron",
            "sources": sources,
            "result": result,
            "copied_files": copied,
            "skipped": skipped,
            "errors": errors,
        }),
    );

    if sources.is_empty() {
        return Some(format!(
            "Electron: 未迁移（{}）",
            skipped.first().cloned().unwrap_or_else(|| "无可用来源".into())
        ));
    }
    Some(format!(
        "Electron: 复制 {copied} 个文件（{} 个来源）",
        sources.len()
    ))
}

/// 旧版安装目录候选（弹幕 Cookie / 版本号都在这里）。
fn legacy_install_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |p: Option<PathBuf>| {
        if let Some(p) = p {
            if p.is_dir() && !out.contains(&p) {
                out.push(p);
            }
        }
    };
    if let Some(local) = local() {
        push(Some(local.join("Programs").join("Fntv-Plus")));
    }
    for key in ["PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMW6432"] {
        if let Some(pf) = std::env::var_os(key) {
            push(Some(PathBuf::from(pf).join("Fntv-Plus")));
        }
    }
    drop(push);
    out
}

fn find_old_bili_cookie() -> Option<PathBuf> {
    for dir in legacy_install_dirs() {
        for rel in [
            "third_party/fntv-mpv/portable_config/scripts/uosc_danmaku/bili_cookie.txt",
            "third_party/fntv-mpv/portable_config/bili_cookie.txt",
        ] {
            let p = dir.join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

// ───────────────────────── 旧 localStorage → history.json / ratings.json ─────────────────────────

/// 旧 Electron userData 下的 LevelDB 目录（主 + 各 Partitions）。
fn legacy_leveldb_dirs(base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |p: PathBuf| {
        if p.is_dir() && !out.contains(&p) {
            out.push(p);
        }
    };
    push(base.join("Local Storage").join("leveldb"));
    if let Ok(rd) = std::fs::read_dir(base.join("Partitions")) {
        for e in rd.flatten() {
            push(e.path().join("Local Storage").join("leveldb"));
        }
    }
    drop(push);
    out
}

/// 从旧 LevelDB 的 `.log/.ldb` 原始字节里取出某个 localStorage 键的 JSON 值。
/// Chromium 以 UTF-16LE 存字符串，部分工具/版本写 UTF-8，两种都试。
fn read_legacy_ls_value(base: &Path, key: &str) -> Option<Value> {
    let mut best: Option<(usize, Value)> = None;
    let mut budget = MAX_LEVELDB_SCAN_BYTES;
    for dir in legacy_leveldb_dirs(base) {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext != "log" && ext != "ldb" {
                continue;
            }
            let Ok(meta) = e.metadata() else {
                continue;
            };
            if meta.len() > budget {
                continue;
            }
            budget -= meta.len();
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let utf16: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let text16 = String::from_utf16_lossy(&utf16);
            if let Some(v) = extract_ls_value(&text16, key) {
                let len = v.to_string().len();
                if best.as_ref().map(|(l, _)| len > *l).unwrap_or(true) {
                    best = Some((len, v));
                }
            }
            if let Ok(text8) = std::str::from_utf8(&bytes) {
                if let Some(v) = extract_ls_value(text8, key) {
                    let len = v.to_string().len();
                    if best.as_ref().map(|(l, _)| len > *l).unwrap_or(true) {
                        best = Some((len, v));
                    }
                }
            }
        }
    }
    best.map(|(_, v)| v)
}

/// 在文本里找 key，取其后 200 字符内的第一个平衡 JSON 对象/数组。
fn extract_ls_value(text: &str, key: &str) -> Option<Value> {
    let mut from = 0usize;
    let mut best: Option<Value> = None;
    while let Some(pos) = text[from..].find(key) {
        let after = from + pos + key.len();
        let window = &text[after..];
        let rel = window.find(['{', '[']);
        if let Some(rel) = rel {
            if rel <= 200 {
                if let Some(json) = balanced_json_value(&window[rel..]) {
                    if let Ok(v) = serde_json::from_str::<Value>(json) {
                        let bigger = best
                            .as_ref()
                            .map(|old| v.to_string().len() > old.to_string().len())
                            .unwrap_or(true);
                        if bigger {
                            best = Some(v);
                        }
                    }
                }
            }
        }
        from = after;
    }
    best
}

/// 取字符串开头第一个平衡的 `{...}` 或 `[...]`（正确处理 JSON 字符串内的括号/转义）。
fn balanced_json_value(s: &str) -> Option<&str> {
    let open = s.chars().next()?;
    let close = match open {
        '{' => '}',
        '[' => ']',
        _ => return None,
    };
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
                c if c == open => depth += 1,
                c if c == close => {
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

/// 旧事件 + 本地记录 + 海报缓存 → 新 `history.json` 数组。
fn map_legacy_history(events: Option<&Value>, local: Option<&Value>, posters: Option<&Value>) -> Option<Value> {
    let poster_of = |key: &str| -> String {
        posters
            .and_then(Value::as_object)
            .and_then(|m| m.get(key))
            .and_then(|v| v.get("poster"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };

    // key → (played_at, 记录)；同一条目保留最近一次
    let mut by_key: std::collections::HashMap<String, (u64, Value)> =
        std::collections::HashMap::new();

    if let Some(arr) = events.and_then(Value::as_array) {
        for e in arr {
            let key = e.get("key").and_then(Value::as_str).unwrap_or("");
            let ts = e.get("ts").and_then(Value::as_u64).unwrap_or(0);
            if key.is_empty() || ts < MIN_VALID_TS {
                continue;
            }
            let name = e.get("name").and_then(Value::as_str).unwrap_or("");
            let typ = e.get("type").and_then(Value::as_str).unwrap_or("");
            let ms = e.get("ms").and_then(Value::as_u64).unwrap_or(0);
            let rec = json!({
                "guid": key,
                "series_key": "",
                "title": if name.is_empty() { key } else { name },
                "episode": "",
                "poster": poster_of(key),
                "tmdb_id": "",
                "bgm_id": "",
                "type": typ,
                "played_at": ts,
                "source": "fnos",
                "source_name": "",
                "parent_guid": "",
                "duration_ms": 0,
                "watched_ms": ms,
                "progress": 0.0,
                "finished": false,
            });
            match by_key.get(key) {
                Some((old_ts, _)) if *old_ts >= ts => {}
                _ => {
                    by_key.insert(key.to_string(), (ts, rec));
                }
            }
        }
    }

    // 本地列表补充事件里没有的条目（更老的版本只记本地列表，或事件被滚动淘汰）
    if let Some(arr) = local.and_then(Value::as_array) {
        for it in arr {
            let key = it.get("key").and_then(Value::as_str).unwrap_or("");
            let ts = it
                .get("lastPlayedAt")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if key.is_empty() || ts < MIN_VALID_TS || by_key.contains_key(key) {
                continue;
            }
            let name = it.get("name").and_then(Value::as_str).unwrap_or("");
            let typ = it.get("type").and_then(Value::as_str).unwrap_or("");
            let rec = json!({
                "guid": key,
                "series_key": "",
                "title": if name.is_empty() { key } else { name },
                "episode": "",
                "poster": poster_of(key),
                "tmdb_id": "",
                "bgm_id": "",
                "type": typ,
                "played_at": ts,
                "source": "fnos",
                "source_name": "",
                "parent_guid": "",
                "duration_ms": 0,
                "watched_ms": 0,
                "progress": 0.0,
                "finished": false,
            });
            by_key.insert(key.to_string(), (ts, rec));
        }
    }

    if by_key.is_empty() {
        return None;
    }
    let mut list: Vec<(u64, Value)> = by_key.into_values().collect();
    list.sort_by(|a, b| b.0.cmp(&a.0));
    Some(Value::Array(list.into_iter().map(|(_, v)| v).collect()))
}

/// 旧评分 `{key:{r,rv}}` → 新 `ratings.json` `{key:{score,review,updated_at}}`。
fn map_legacy_ratings(raw: &Value) -> Option<Value> {
    let obj = raw.as_object()?;
    let now_ms = now_unix().saturating_mul(1000);
    let mut out = serde_json::Map::new();
    for (key, v) in obj {
        let score = v.get("r").and_then(Value::as_u64).unwrap_or(0).min(5) as u8;
        let review = v.get("rv").and_then(Value::as_str).unwrap_or("");
        if score == 0 && review.is_empty() {
            continue;
        }
        out.insert(
            key.clone(),
            json!({
                "score": score,
                "review": review,
                "updated_at": now_ms,
            }),
        );
    }
    if out.is_empty() {
        None
    } else {
        Some(Value::Object(out))
    }
}

fn legacy_history_from_dir(base: &Path) -> Option<Value> {
    let events = read_legacy_ls_value(base, LS_EVENTS);
    let local = read_legacy_ls_value(base, LS_LOCAL);
    let posters = read_legacy_ls_value(base, LS_POSTERS);
    map_legacy_history(events.as_ref(), local.as_ref(), posters.as_ref())
}

fn legacy_ratings_from_dir(base: &Path) -> Option<Value> {
    read_legacy_ls_value(base, LS_RATINGS).and_then(|v| map_legacy_ratings(&v))
}

// ───────────────────────── 旧配置 → 新配置 字段映射 ─────────────────────────

fn map_config_file(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let old: Value = serde_json::from_str(&text).ok()?;
    map_legacy_config(&old)
}

/// 旧 Electron config.json（扁平 camelCase）→ 新 Config（snake_case）的字段映射。
/// 只输出**有把握**的字段；缺失字段留给 `Config` 的 `serde(default)`。
fn map_legacy_config(old: &Value) -> Option<Value> {
    let get = |k: &str| {
        old.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let get_bool = |k: &str| old.get(k).and_then(Value::as_bool);

    let domain = get("domain");
    let account = get("account");
    let token = get("token");
    if domain.is_empty() && account.is_empty() && token.is_empty() {
        return None;
    }

    let mut m = serde_json::Map::new();
    m.insert(
        "server".into(),
        json!(normalize_server(
            &domain,
            get_bool("useHttps").unwrap_or(false)
        )),
    );
    m.insert("username".into(), json!(account));
    m.insert("token".into(), json!(token));

    // 密码：旧版把各账号密码加密存在 config.history[] 里
    if let Some(pw) = find_legacy_password(old, &account, &domain) {
        if !pw.is_empty() {
            m.insert("password".into(), json!(pw));
        }
    }

    // 访问码：AES-256-CBC(hex) → 解密 → AES-256-GCM(enc:v1:)
    let ac = get("accessCode");
    if !ac.is_empty() {
        if let Some(plain) = decrypt_legacy_cbc_hex(&ac) {
            if !plain.is_empty() {
                m.insert(
                    "access_code".into(),
                    json!(crate::config::encrypt_access_code(&plain)),
                );
            }
        }
    }

    if let Some(v) = get_bool("nasProxyEnabled") {
        m.insert("nas_proxy_enabled".into(), json!(v));
    }
    // 播放器路径只在文件真实存在时映射（旧安装目录可能已被卸载）
    let mpv = get("mpvPlayerPath");
    if !mpv.is_empty() && Path::new(&mpv).exists() {
        m.insert("mpv_path".into(), json!(mpv));
    }
    let pot = get("potPlayerPath");
    if !pot.is_empty() && Path::new(&pot).exists() {
        m.insert("potplayer_path".into(), json!(pot));
    }
    match get("defaultPlayer").as_str() {
        "potplayer" => {
            m.insert("player_kind".into(), json!("potplayer"));
        }
        "mpv" => {
            m.insert("player_kind".into(), json!("mpv"));
        }
        _ => {}
    }
    match get("exitMode").as_str() {
        "direct" => {
            m.insert("exit_mode".into(), json!("exit"));
        }
        "minimize" => {
            m.insert("exit_mode".into(), json!("tray"));
        }
        "ask" => {
            m.insert("exit_mode".into(), json!("ask"));
        }
        _ => {}
    }
    if let Some(v) = get_bool("perfModeEnabled") {
        m.insert("perf_mode".into(), json!(v));
    }
    if let Some(v) = get_bool("carouselLogoEnabled") {
        m.insert("carousel_logo".into(), json!(v));
    }
    let bgm = get("bangumiToken");
    if !bgm.is_empty() {
        m.insert("bgm_token".into(), json!(bgm));
    }
    if let Some(v) = get_bool("debugEnabled") {
        m.insert("log_debug".into(), json!(v));
    }

    // 自建弹幕源（danmu_api）
    if let Some(v) = get_bool("danmuApiEnabled") {
        m.insert("danmu_api_enabled".into(), json!(v));
    }
    let base = get("danmuApiBase");
    let base = base.trim_end_matches('/');
    if !base.is_empty() {
        m.insert("danmu_api_base".into(), json!(base));
    }
    if let Some(arr) = old.get("danmuApiBases").and_then(Value::as_array) {
        let list: Vec<String> = arr
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !list.is_empty() {
            m.insert("danmu_api_bases".into(), json!(list));
        }
    }

    // 匿名使用统计
    if let Some(v) = get_bool("statsEnabled") {
        m.insert("usage_stats_enabled".into(), json!(v));
    }
    for (old_key, new_key) in [
        ("statsAnonId", "usage_stats_anon_id"),
        ("statsLastPingDay", "usage_stats_last_ping_day"),
    ] {
        let v = get(old_key);
        if !v.is_empty() {
            m.insert(new_key.into(), json!(v));
        }
    }
    if let Some(v) = get_bool("statsLastPingOk") {
        m.insert("usage_stats_last_ping_ok".into(), json!(v));
    }
    if let Some(arr) = old.get("statsPendingDays").and_then(Value::as_array) {
        let list: Vec<String> = arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        m.insert("usage_stats_pending_days".into(), json!(list));
    }

    Some(Value::Object(m))
}

fn normalize_server(domain: &str, use_https: bool) -> String {
    let d = domain.trim().trim_end_matches('/');
    if d.is_empty() {
        return String::new();
    }
    if d.contains("://") {
        d.to_string()
    } else {
        format!("{}://{}", if use_https { "https" } else { "http" }, d)
    }
}

fn host_of(s: &str) -> String {
    let s = s.trim();
    let s = s.split("://").nth(1).unwrap_or(s);
    let s = s.split('/').next().unwrap_or(s);
    s.trim_end_matches('/').to_lowercase()
}

fn find_legacy_password(old: &Value, account: &str, domain: &str) -> Option<String> {
    let arr = old.get("history")?.as_array()?;
    let host = host_of(domain);
    let mut fallback: Option<String> = None;
    for item in arr {
        let encrypted = item.get("password").and_then(Value::as_str).unwrap_or("");
        if encrypted.is_empty() {
            continue;
        }
        let Some(plain) = decrypt_legacy_cbc_hex(encrypted) else {
            continue;
        };
        let acc = item.get("account").and_then(Value::as_str).unwrap_or("");
        let dom = item.get("domain").and_then(Value::as_str).unwrap_or("");
        if acc == account && (host.is_empty() || host_of(dom) == host) {
            return Some(plain);
        }
        if fallback.is_none() {
            fallback = Some(plain);
        }
    }
    fallback
}

/// 旧 Electron 版的 `crypto.createCipheriv('aes-256-cbc', key, zeroIV)` + PKCS#7。
/// 输出是 hex；失败返回 None（单字段失败不影响整体迁移）。
fn decrypt_legacy_cbc_hex(hex_str: &str) -> Option<String> {
    use aes::cipher::{BlockDecrypt, KeyInit};
    use aes::Aes256;

    let bytes = hex_decode(hex_str.trim())?;
    if bytes.is_empty() || bytes.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes256::new_from_slice(LEGACY_CBC_KEY_SEED.as_bytes()).ok()?;
    let mut prev = [0u8; 16]; // IV = 16 个 0
    let mut out = Vec::with_capacity(bytes.len());
    for chunk in bytes.chunks_exact(16) {
        let mut ct = [0u8; 16];
        ct.copy_from_slice(chunk);
        let mut block = ct;
        cipher.decrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
            &mut block,
        ));
        for i in 0..16 {
            block[i] ^= prev[i];
        }
        out.extend_from_slice(&block);
        prev = ct;
    }
    // PKCS#7 去填充
    let pad = *out.last()? as usize;
    if pad == 0 || pad > 16 || pad > out.len() {
        return None;
    }
    if !out[out.len() - pad..].iter().all(|&b| b == pad as u8) {
        return None;
    }
    out.truncate(out.len() - pad);
    String::from_utf8(out).ok()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    for pair in b.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

// ───────────────────────── 版本识别（≤ 3.9.0 才迁移） ─────────────────────────

fn should_migrate_version(version: Option<&str>) -> bool {
    match version.and_then(parse_version) {
        Some(v) => v <= LEGACY_MAX_VERSION,
        None => true, // 读不到版本时按旧版处理（目录名 fntv 只被 ≤3.9.0 使用）
    }
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let mut it = s.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it
        .next()
        .map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    Some((major, minor, patch))
}

fn detect_legacy_version(userdata: &Path) -> Option<String> {
    // 1) 旧 config 里开发者自定义版本号（设置-通用-版号切换）
    if let Ok(text) = std::fs::read_to_string(userdata.join("config.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            for key in ["customVersion", "version"] {
                if let Some(s) = v.get(key).and_then(Value::as_str) {
                    if parse_version(s).is_some() {
                        return Some(s.to_string());
                    }
                }
            }
        }
    }
    // 2) 安装目录里的 resources/app/package.json 或 app.asar
    for dir in legacy_install_dirs() {
        let unpacked = dir.join("resources").join("app").join("package.json");
        if let Some(v) = version_from_package_json(&unpacked) {
            return Some(v);
        }
        let asar = dir.join("resources").join("app.asar");
        if let Some(v) = version_from_asar(&asar) {
            return Some(v);
        }
    }
    None
}

fn version_from_package_json(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("version")?.as_str().map(str::to_string)
}

/// 从 ASAR 头部定位 `package.json` 并读出 `version`（只读必要字节，不整包加载）。
fn version_from_asar(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 16];
    f.read_exact(&mut head).ok()?;
    let header_len = u32::from_le_bytes(head[12..16].try_into().ok()?) as usize;
    if header_len == 0 || header_len > 32 * 1024 * 1024 {
        return None;
    }
    let mut header = vec![0u8; header_len];
    f.read_exact(&mut header).ok()?;
    let header: Value = serde_json::from_slice(&header).ok()?;
    let entry = header.get("files")?.get("package.json")?;
    let offset: u64 = entry.get("offset")?.as_str()?.parse().ok()?;
    let size = entry.get("size")?.as_u64()?;
    if size > 1024 * 1024 {
        return None;
    }
    let data_start = 16u64 + header_len as u64 + offset;
    f.seek(SeekFrom::Start(data_start)).ok()?;
    let mut buf = vec![0u8; size as usize];
    f.read_exact(&mut buf).ok()?;
    let pkg: Value = serde_json::from_slice(&buf).ok()?;
    pkg.get("version")?.as_str().map(str::to_string)
}

// ───────────────────────── 小工具 ─────────────────────────

fn copy_file_if_absent(src: &Path, dst: &Path) -> std::io::Result<bool> {
    if !src.is_file() || dst.exists() {
        return Ok(false);
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dst)?;
    Ok(true)
}

fn copy_dir_if_absent(src: &Path, dst: &Path) -> Result<u64, String> {
    if !src.is_dir() || dst.exists() {
        return Ok(0);
    }
    copy_dir(src, dst)
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            total = total.saturating_add(dir_size(&p));
        } else if let Ok(m) = e.metadata() {
            total = total.saturating_add(m.len());
        }
    }
    total
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 在 Tauri 初始化**之前**调用（见 run() 开头）。
pub fn run() -> Report {
    let appdata = roaming().as_deref().and_then(|r| move_dir(r, "APPDATA"));
    let localappdata = local().as_deref().and_then(|r| move_dir(r, "LOCALAPPDATA"));
    let electron = roaming().as_deref().and_then(migrate_electron_at);
    Report {
        appdata,
        localappdata,
        electron,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "anibox-migrate-test-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// 迁移的**三条边界**（离线、不碰真实 APPDATA）：
    ///   ① 新目录已在 → 不动（防重复迁移覆盖新数据）
    ///   ② 旧目录不在 → 不动（全新用户）
    ///   ③ 只有旧目录在 → 搬过去，且**内容一致**、旧目录保留
    #[test]
    fn move_dir_only_when_new_absent_and_old_present() {
        let base = temp_base("move");

        // ② 全新用户：旧目录不存在 → 不迁移
        assert!(move_dir(&base, "t").is_none(), "旧目录不存在时不该迁移");

        // ③ 只有旧目录 → 迁移，内容跟着走，旧目录保留
        let old = base.join(OLD_ID);
        std::fs::create_dir_all(old.join("cache/sub")).unwrap();
        std::fs::write(old.join("config.json"), b"{\"server\":\"x\"}").unwrap();
        std::fs::write(old.join("cache/sub/a.bin"), b"abc").unwrap();
        let r = move_dir(&base, "t");
        assert!(r.is_some(), "旧目录在、新目录不在时必须迁移");
        let new = base.join(NEW_ID);
        assert_eq!(
            std::fs::read(new.join("config.json")).unwrap(),
            b"{\"server\":\"x\"}"
        );
        assert_eq!(std::fs::read(new.join("cache/sub/a.bin")).unwrap(), b"abc");
        assert!(old.exists(), "旧目录必须保留（回滚余地）");
        assert!(new.join(MARKER_FILE).is_file(), "必须写迁移标记");

        // ① 新目录已在 → 不再动（哪怕旧目录也在）
        std::fs::write(new.join("config.json"), b"{\"server\":\"NEW\"}").unwrap();
        assert!(move_dir(&base, "t").is_none(), "新目录已存在时不该再迁移");
        assert_eq!(
            std::fs::read(new.join("config.json")).unwrap(),
            b"{\"server\":\"NEW\"}",
            "已有新数据不能被旧目录覆盖"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 安装器可能提前建好新目录并只放一个 bili_cookie.txt：
    /// 此时必须把 Tauri2 旧目录"合并"进去，不能因为目录存在就整目录跳过。
    #[test]
    fn merges_tauri2_into_precreated_dir_with_cookie() {
        let base = temp_base("merge");
        let old = base.join(OLD_ID);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("config.json"), b"{\"server\":\"T2\"}").unwrap();
        let new = base.join(NEW_ID);
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("bili_cookie.txt"), b"SESSDATA=x").unwrap();

        let r = move_dir(&base, "t");
        assert!(r.is_some(), "目录已存在但无核心数据时仍应合并");
        assert_eq!(
            std::fs::read(new.join("config.json")).unwrap(),
            b"{\"server\":\"T2\"}"
        );
        assert_eq!(
            std::fs::read(new.join("bili_cookie.txt")).unwrap(),
            b"SESSDATA=x",
            "预置的 Cookie 不能被覆盖/删除"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// identifier 常量必须与 tauri.conf.json 一致 —— 不一致会导致"迁到的目录不是程序真正读的目录"。
    #[test]
    fn new_id_matches_tauri_conf() {        let conf = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"),
        )
        .expect("读 tauri.conf.json");
        assert!(
            conf.contains(&format!("\"identifier\": \"{NEW_ID}\"")),
            "migrate.rs 的 NEW_ID({NEW_ID}) 与 tauri.conf.json 的 identifier 不一致"
        );
    }

    /// 旧 AES-256-CBC 密文必须能解出原文（用 Node crypto 生成的固定向量做交叉验证）。
    #[test]
    fn decrypts_legacy_cbc_vectors() {
        assert_eq!(
            decrypt_legacy_cbc_hex("bd52aaeb74c8d473ddd0f42a530200a6").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            decrypt_legacy_cbc_hex("a02beec83fa92d82e41e6590dbaf11ef").as_deref(),
            Some("secret-pw")
        );
        assert_eq!(
            decrypt_legacy_cbc_hex("c0cd01c9f71cc73bb0757fb628cf2a7e").as_deref(),
            Some("246810")
        );
        // 非法输入不 panic
        assert!(decrypt_legacy_cbc_hex("zz").is_none());
        assert!(decrypt_legacy_cbc_hex("abcd").is_none());
    }

    /// 字段映射：旧 camelCase → 新 snake_case，缺失字段不写（交给 serde default）。
    #[test]
    fn maps_legacy_config_fields() {
        let old = json!({
            "account": "EIKO",
            "domain": "100.66.1.2:17777",
            "useHttps": true,
            "token": "tok",
            "accessCode": "c0cd01c9f71cc73bb0757fb628cf2a7e", // "246810"
            "history": [
                {"account": "EIKO", "domain": "100.66.1.2:17777",
                 "password": "a02beec83fa92d82e41e6590dbaf11ef"}, // "secret-pw"
                {"account": "other", "domain": "x", "password": "bd52aaeb74c8d473ddd0f42a530200a6"}
            ],
            "exitMode": "minimize",
            "defaultPlayer": "potplayer",
            "nasProxyEnabled": true,
            "bangumiToken": "bgm-token",
            "debugEnabled": false,
            "perfModeEnabled": true,
            "carouselLogoEnabled": false,
            "danmuApiEnabled": true,
            "danmuApiBase": "http://127.0.0.1:22352/abc/",
            "danmuApiBases": ["http://127.0.0.1:22352/abc/", "http://nas:22352/abc"],
            "statsEnabled": false,
            "statsAnonId": "anon",
            "statsLastPingDay": "2026-10-10",
            "statsLastPingOk": true,
            "statsPendingDays": ["2026-10-09"]
        });
        let mapped = map_legacy_config(&old).expect("应能映射");
        assert_eq!(mapped["server"], json!("https://100.66.1.2:17777"));
        assert_eq!(mapped["username"], json!("EIKO"));
        assert_eq!(mapped["token"], json!("tok"));
        assert_eq!(mapped["password"], json!("secret-pw"));
        assert_eq!(mapped["exit_mode"], json!("tray"));
        assert_eq!(mapped["player_kind"], json!("potplayer"));
        assert_eq!(mapped["nas_proxy_enabled"], json!(true));
        assert_eq!(mapped["bgm_token"], json!("bgm-token"));
        assert_eq!(mapped["log_debug"], json!(false));
        assert_eq!(mapped["perf_mode"], json!(true));
        assert_eq!(mapped["carousel_logo"], json!(false));
        assert_eq!(mapped["danmu_api_enabled"], json!(true));
        assert_eq!(mapped["danmu_api_base"], json!("http://127.0.0.1:22352/abc"));
        assert_eq!(
            mapped["danmu_api_bases"],
            json!(["http://127.0.0.1:22352/abc", "http://nas:22352/abc"])
        );
        assert_eq!(mapped["usage_stats_enabled"], json!(false));
        assert_eq!(mapped["usage_stats_anon_id"], json!("anon"));
        assert_eq!(mapped["usage_stats_last_ping_day"], json!("2026-10-10"));
        assert_eq!(mapped["usage_stats_last_ping_ok"], json!(true));
        assert_eq!(mapped["usage_stats_pending_days"], json!(["2026-10-09"]));
        // 访问码必须是新的 GCM 格式，且能解回原文
        let ac = mapped["access_code"].as_str().unwrap();
        assert!(crate::config::is_encrypted_access_code(ac));
        assert_eq!(crate::config::decrypt_access_code(ac), "246810");
        // 未映射字段不应出现
        assert!(mapped.get("history").is_none());
    }

    /// 旧 localStorage（事件/本地列表/评分）→ 新 history.json / ratings.json 的映射。
    #[test]
    fn maps_legacy_localstorage_history_and_ratings() {
        let events = json!([
            {"ts": 1_700_000_000_000u64, "key": "g1", "name": "剧A", "type": "剧集", "ms": 5000},
            {"ts": 1_700_000_100_000u64, "key": "g1", "name": "剧A", "type": "剧集"},
            {"ts": 1u64, "key": "bad"} // 脏时间戳，应被丢弃
        ]);
        let local = json!([
            {"key": "g2", "name": "本地片", "type": "其他", "lastPlayedAt": 1_700_000_200_000u64}
        ]);
        let posters = json!({"g1": {"poster": "/p1.jpg"}});

        let hist = map_legacy_history(Some(&events), Some(&local), Some(&posters)).unwrap();
        let arr = hist.as_array().unwrap();
        assert_eq!(arr.len(), 2, "同一条目只保留最近一次，脏数据丢弃");
        let g1 = arr
            .iter()
            .find(|r| r["guid"] == json!("g1"))
            .expect("应包含 g1");
        assert_eq!(g1["played_at"], json!(1_700_000_100_000u64));
        assert_eq!(g1["poster"], json!("/p1.jpg"));
        assert_eq!(g1["title"], json!("剧A"));
        let g2 = arr
            .iter()
            .find(|r| r["guid"] == json!("g2"))
            .expect("应包含 g2");
        assert_eq!(g2["source"], json!("fnos"));

        let ratings = json!({
            "g1": {"r": 5, "rv": "好看"},
            "g2": {"r": 0, "rv": ""} // 空评分不迁移
        });
        let r = map_legacy_ratings(&ratings).unwrap();
        assert_eq!(r["g1"]["score"], json!(5));
        assert_eq!(r["g1"]["review"], json!("好看"));
        assert!(r.get("g2").is_none());
    }

    /// LevelDB 原始字节里的值提取：对象/数组都要能取到，且能容忍前后二进制噪声。
    #[test]
    fn extracts_json_value_after_key_from_noisy_bytes() {
        let text = "\u{1}junk\u{2}fntv_wh_events_v1\u{3}[{\"ts\":1700000000000,\"key\":\"g\"}]\u{4}tail";
        let v = extract_ls_value(text, "fntv_wh_events_v1").unwrap();
        assert_eq!(v[0]["key"], json!("g"));
        let text2 = "xxfntv_wh_ratings_v1yy{\"g\":{\"r\":4,\"rv\":\"ok\"}}zz";
        let v2 = extract_ls_value(text2, "fntv_wh_ratings_v1").unwrap();
        assert_eq!(v2["g"]["r"], json!(4));
    }

    /// 合成一个最小 ASAR，验证版本号提取（旧安装包没有解包的 package.json 时走这里）。
    #[test]
    fn reads_version_from_synthetic_asar() {
        let base = temp_base("asar");
        let pkg = br#"{"name":"fntv","version":"3.8.0"}"#;
        let header = json!({
            "files": { "package.json": { "size": pkg.len(), "offset": "0" } }
        });
        let hb = serde_json::to_vec(&header).unwrap();
        let mut data = Vec::new();
        data.extend_from_slice(&[4, 0, 0, 0]);
        data.extend_from_slice(&(hb.len() as u32).to_le_bytes());
        data.extend_from_slice(&(hb.len() as u32).to_le_bytes());
        data.extend_from_slice(&(hb.len() as u32).to_le_bytes());
        data.extend_from_slice(&hb);
        data.extend_from_slice(pkg);
        let p = base.join("app.asar");
        std::fs::write(&p, data).unwrap();
        assert_eq!(version_from_asar(&p).as_deref(), Some("3.8.0"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn version_gate_allows_only_up_to_3_9_0() {
        assert!(should_migrate_version(Some("3.9.0")));
        assert!(should_migrate_version(Some("3.8.0")));
        assert!(should_migrate_version(Some("v3.9.0-beta")));
        assert!(!should_migrate_version(Some("3.9.1")));
        assert!(!should_migrate_version(Some("4.0.0")));
        assert!(should_migrate_version(None), "版本未知时按旧版处理");
        assert_eq!(parse_version("3.9.0"), Some((3, 9, 0)));
        assert_eq!(parse_version("v4.0.0-beta.1"), Some((4, 0, 0)));
    }

    /// 端到端（临时目录）：旧 Electron 目录 → 新目录，配置映射 + 标记 + 幂等 + 不覆盖。
    #[test]
    fn migrates_electron_config_once_and_never_overwrites() {
        let base = temp_base("electron");
        let old = base.join("fntv");
        std::fs::create_dir_all(old.join("cache/img")).unwrap();
        std::fs::create_dir_all(old.join("login-bg")).unwrap();
        std::fs::write(
            old.join("config.json"),
            serde_json::to_vec(&json!({
                "account": "EIKO",
                "domain": "http://nas:18888",
                "token": "tok",
                "exitMode": "direct",
                "history": [{"account":"EIKO","domain":"http://nas:18888",
                             "password":"a02beec83fa92d82e41e6590dbaf11ef"}]
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(old.join("login-bg/bg.png"), b"PNG").unwrap();
        std::fs::write(old.join("cache/img/a.txt"), b"data:image/png;base64,xx").unwrap();
        std::fs::write(old.join("playback-speed.json"), b"{\"version\":1}").unwrap();

        let r = migrate_electron_at(&base);
        assert!(r.is_some(), "旧目录存在时必须迁移");
        let new = base.join(NEW_ID);
        let cfg: Value =
            serde_json::from_slice(&std::fs::read(new.join("config.json")).unwrap()).unwrap();
        assert_eq!(cfg["server"], json!("http://nas:18888"));
        assert_eq!(cfg["username"], json!("EIKO"));
        assert_eq!(cfg["password"], json!("secret-pw"));
        assert_eq!(cfg["exit_mode"], json!("exit"));
        assert!(new.join("login-bg/bg.png").is_file());
        assert!(new.join("cache/legacy-electron/img/a.txt").is_file());
        assert!(new.join("legacy-electron/playback-speed.json").is_file());
        assert!(
            new.join("legacy-electron/config.json").is_file(),
            "旧配置需原样归档"
        );
        let marker: Value =
            serde_json::from_slice(&std::fs::read(new.join(MARKER_FILE)).unwrap()).unwrap();
        assert_eq!(marker["sources"][0]["kind"], json!("electron"));
        assert_eq!(marker["sources"][0]["result"], json!("ok"));

        // 幂等：第二次直接返回 None，不改动已有文件
        std::fs::write(new.join("config.json"), b"{\"server\":\"NEW\"}").unwrap();
        assert!(migrate_electron_at(&base).is_none(), "有标记时不得重复迁移");
        assert_eq!(
            std::fs::read(new.join("config.json")).unwrap(),
            b"{\"server\":\"NEW\"}",
            "已有新配置不能被覆盖"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 目标 config 已存在（但没有标记）时：其余文件仍补齐，config 保持不动。
    #[test]
    fn keeps_existing_target_config_but_copies_other_files() {
        let base = temp_base("keep-cfg");
        let old = base.join("fntv");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(
            old.join("config.json"),
            br#"{"account":"OLD","domain":"http://old"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(old.join("login-bg")).unwrap();
        std::fs::write(old.join("login-bg/a.png"), b"y").unwrap();

        let new = base.join(NEW_ID);
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("config.json"), br#"{"server":"NEW"}"#).unwrap();

        assert!(migrate_electron_at(&base).is_some());
        assert_eq!(
            std::fs::read(new.join("config.json")).unwrap(),
            br#"{"server":"NEW"}"#
        );
        assert!(new.join("login-bg/a.png").is_file());
        assert!(new.join("legacy-electron/config.json").is_file());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 旧目录缺失 → 不创建新目录、不写标记。
    #[test]
    fn no_legacy_dir_is_noop() {
        let base = temp_base("none");
        assert!(migrate_electron_at(&base).is_none());
        assert!(
            !base.join(NEW_ID).exists(),
            "无旧数据时不应创建新目录"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 版本高于 3.9.0 → 跳过迁移（但仍写 skipped 标记，避免每次启动重试）。
    #[test]
    fn skips_newer_legacy_version() {
        let base = temp_base("newer");
        let old = base.join("fntv");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(
            old.join("config.json"),
            br#"{"customVersion":"3.9.1","account":"A","domain":"http://x","token":"t"}"#,
        )
        .unwrap();
        let r = migrate_electron_at(&base);
        assert!(r.is_some(), "应报告跳过");
        let new = base.join(NEW_ID);
        assert!(!new.join("config.json").exists(), "高版本不得迁移配置");
        let marker: Value =
            serde_json::from_slice(&std::fs::read(new.join(MARKER_FILE)).unwrap()).unwrap();
        assert_eq!(marker["sources"][0]["result"], json!("skipped"));
        // 第二次不再重试
        assert!(migrate_electron_at(&base).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
