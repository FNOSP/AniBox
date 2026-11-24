//! [lc-2348] 弹幕内容屏蔽（屏蔽词 / 屏蔽类型）—— 桌面端设置面板的真实读写通道。
//!
//! ## 为什么需要这个模块
//!
//! `uosc_danmaku/modules/parse.lua` 早就有两套屏蔽机制：
//!   - **类型屏蔽**：`danmaku_block_types.json`（字符串数组，如 `["top","color"]`），
//!     由 `is_type_blocked()` 消费；lc-1335 已加 `reload_block_types()` 运行时刷新。
//!   - **内容屏蔽**：`options.blacklist_path` 指向的文件，每行一条 Lua pattern，
//!     由 `is_blacklisted()` 在 `parse_danmaku_files()` 里逐条过滤。
//!
//! 但**桌面端从来没有编辑入口**（全仓 grep `blacklist|屏蔽|blocklist` 在
//! `src/components/settings/SettingsPanel.tsx` 零命中），且 `options.lua` 里
//! `blacklist_path = ""` 默认空串 → 内容屏蔽从未生效。只有 fpk 网页端
//! （`fntv-plus.user.js:19366`）有「屏蔽词（每行一条，支持正则）」textarea。
//!
//! ## 本模块做三件事
//!
//! 1. **写屏蔽类型**：读写 JSON 文件（MPV 快捷键 Ctrl+K 菜单写的是同一份文件，
//!    两边共用一个真源，不另立配置项）。
//! 2. **写屏蔽词**：写纯文本文件（每行一条），并把 `options.blacklist_path` 指向它 ——
//!    这需要写 `script-opts/uosc_danmaku.conf` 的 `blacklist_path=` 行。
//! 3. **通知运行中的会话重载**：`script-message` 走与 enhance.rs 相同的推送路径。
//!
//! ## 路径策略（关键，对齐 scripts/fntv_shader.lua 的既有做法）
//!
//! **文件落用户数据目录，不落安装目录**：
//!   - 首选：`%APPDATA%\com.anibox.app\danmaku\danmaku_block_types.json`
//!            `%APPDATA%\com.anibox.app\danmaku\danmaku_blacklist.txt`
//!   - 兜底（无 APPDATA）：`<mpv 配置目录>/scripts/uosc_danmaku/...`
//!
//! 为什么不能直接写 `mpv_portable_config_dir`：那个函数在**开发态解析到仓库里的
//! `third_party/`**（用户写设置会污染 git 工作区），在**安装态解析到资源目录**
//! （升级覆盖安装会把用户规则冲掉）。fntv_shader.lua 已经因同样原因把用户着色器
//! 配置迁到 `%APPDATA%\com.anibox.app\`，本模块沿用同一约定。
//!
//! 代价：Lua 侧的 `options.blacklist_path` 必须写**绝对路径**（不能写 `~~/`，
//! 因为文件不在 mpv 配置目录里）。`ensure_blacklist_path_in_conf` 负责这件事。
//! `danmaku_block_types.json` 则由 parse.lua 的 `options.block_types_path` 指向 ——
//! 这一行也在同一个 conf 里改写。
//!
//! ## Lua pattern 不等于正则（重要）
//!
//! `is_blacklisted` 用的是 Lua `str:match(pattern)`，语法与 PCRE 有差异：
//!   - 字符类`%d` 而非 `\d`；`%s`、`%w`、`%a` 同理；`%` 本身是转义符
//!   - `.` 通配、`*`/`+`/`-`（非贪婪）/`?` 修饰符、`^`/`$` 锚点都有
//!   - **不支持** `\b`、`(?=...)`、`(?:...)`、`{n,m}`（Lua 用 `%d%d` 重复）
//!
//! 因此本模块**不做语法转换也不校验**——用户按 Lua pattern 写，写错了 `pcall` 会静默
//! 跳过那条规则（parse.lua 已有此保护）。前端提示语必须写「Lua pattern」而非「正则」，
//! 照抄 AniCh 的 `\d+` 示例会在 Lua 里不工作（它用的是 Dart RegExp）。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use crate::config::mpv_portable_config_dir;

/// 与 tauri.conf.json 的 identifier 一致（fntv_shader.lua 的 APP_ID 同值）。
const APP_ID: &str = "com.anibox.app";
const BLOCK_TYPES_FILE: &str = "danmaku_block_types.json";
const BLACKLIST_FILE: &str = "danmaku_blacklist.txt";

/// 屏蔽类型全集与顺序（与 `bili_danmaku.js:_filter_danmaku`、
/// `parse.lua:MODE_BLOCK_TAG`、fpk `embyWall.ts:1842` 三处保持一致）。
const BLOCK_TYPE_KEYS: &[&str] = &["scroll", "top", "bottom", "reverse", "advanced", "color"];

/// 用户数据目录候选（按优先级）。首选 APPDATA（可写、升级不覆盖），兜底 mpv 配置目录。
fn user_dir_candidates(app: &AppHandle) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata = appdata.trim();
        if !appdata.is_empty() {
            out.push(PathBuf::from(appdata).join(APP_ID).join("danmaku"));
        }
    }
    // 兜底：mpv 便携配置目录下的 uosc_danmaku（`ensure_portable_config` 在非 Windows
    // 会把它复制到 ~/.config/mpv，那个位置本来就可写）
    if let Some(base) = mpv_portable_config_dir(app) {
        out.push(base.join("scripts").join("uosc_danmaku"));
    }
    out
}

/// 当前使用的数据目录（首个存在或可创建的；都不行则返回第一个候选由后续报错）。
fn user_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let cands = user_dir_candidates(app);
    if cands.is_empty() {
        return Err("无法确定弹幕屏蔽配置目录（无 APPDATA 且找不到 mpv 配置目录）".into());
    }
    // 已存在的优先（避免两个候选都可用时切来切去）
    for p in &cands {
        if p.is_dir() {
            return Ok(p.clone());
        }
    }
    let first = cands[0].clone();
    std::fs::create_dir_all(&first).map_err(|e| format!("创建配置目录失败（{}）: {e}", first.display()))?;
    Ok(first)
}

fn block_types_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(user_dir(app)?.join(BLOCK_TYPES_FILE))
}

fn blacklist_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(user_dir(app)?.join(BLACKLIST_FILE))
}

fn conf_path(app: &AppHandle) -> Result<PathBuf, String> {
    let base = mpv_portable_config_dir(app)
        .ok_or_else(|| "找不到随包 mpv 的 portable_config 目录".to_string())?;
    Ok(base.join("script-opts").join("uosc_danmaku.conf"))
}

/// 读屏蔽类型：文件不存在/为空/非法 一律返回空数组（= 不屏蔽任何类型）。
fn read_block_types(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        if let Some(s) = item.as_str() {
            let s = s.trim().to_lowercase();
            // 只接受已知键，防止把脏数据写回文件后 `_filter_danmaku` 静默失效
            if BLOCK_TYPE_KEYS.contains(&s.as_str()) && !out.iter().any(|x: &String| x == &s) {
                out.push(s);
            }
        }
    }
    // 按固定顺序输出（文件内容稳定，diff 友好）
    BLOCK_TYPE_KEYS
        .iter()
        .filter(|k| out.iter().any(|x| x == *k))
        .map(|k| k.to_string())
        .collect()
}

/// 读屏蔽词：**保留原始行**（不 trim 内部、不丢弃看起来像注释的行——
/// Lua pattern 里 `--` 不是注释，用户可能真要屏蔽含 `--` 的文本）。
/// 只丢弃空行与首尾空白。
fn read_blacklist(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// 改写 `script-opts/uosc_danmaku.conf` 里的单个 `key=value` 行（存在则替换，不存在则追加）。
///
/// 只碰这一行，其余内容原样保留 —— 这个 conf 有大量带注释的手写配置，
/// 整体重写会丢用户/我们的调整。已注释掉的行（`#key=`）不动。
fn upsert_conf_line(app: &AppHandle, key: &str, value: &str) -> Result<(), String> {
    let conf = conf_path(app)?;
    let line = format!("{key}={value}");

    let existing = std::fs::read_to_string(&conf).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(|l| l.to_string()).collect();

    let mut found = false;
    for l in lines.iter_mut() {
        let trimmed = l.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(after) = trimmed.strip_prefix(key) {
            if after.trim_start().starts_with('=') {
                *l = line.clone();
                found = true;
                break;
            }
        }
    }
    if !found {
        if let Some(dir) = conf.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建 script-opts 目录失败: {e}"))?;
        }
        lines.push(line);
    }
    let mut body = lines.join("\n");
    if !body.ends_with('\n') {
        body.push('\n');
    }
    std::fs::write(&conf, body).map_err(|e| format!("写入 uosc_danmaku.conf 失败: {e}"))?;
    Ok(())
}

/// mpv 的 conf 里路径统一用正斜杠（Windows 下反斜杠在 mpv 的 expand-path 里也可，
/// 但正斜杠在所有平台都安全，且与仓库里其它 conf 的写法一致）。
fn conf_slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// 把改动推给正在播放的会话：`script-message danmaku-reload-filter`。
/// parse.lua 侧新增的监听器收到后重读屏蔽类型与屏蔽词（见同批改动）。
async fn push_filter_reload(state: &tauri::State<'_, crate::AppState>) {
    let Some(sess) = state.mpv.lock().await.clone() else {
        return;
    };
    if sess.is_dead().await {
        return;
    }
    sess.command_no_reply(json!(["script-message", "danmaku-reload-filter"]))
        .await;
}

/// 读当前屏蔽配置（屏蔽类型 + 屏蔽词行）。
#[tauri::command]
pub fn danmaku_filter_get(app: AppHandle) -> Result<Value, String> {
    let bt_path = block_types_path(&app)?;
    let bl_path = blacklist_path(&app)?;
    let block_types = read_block_types(&bt_path);
    let blacklist = read_blacklist(&bl_path);
    Ok(json!({
        "ok": true,
        // 供前端渲染成开关/列表；available 是全集（顺序固定）
        "blockTypes": block_types,
        "availableTypes": BLOCK_TYPE_KEYS,
        "blacklist": blacklist,
        // 前端提示语里要显示 Lua pattern 而非正则，这里把语法差异透出来
        "patternSyntax": "lua",
        "blockTypesPath": bt_path.to_string_lossy(),
        "blacklistPath": bl_path.to_string_lossy(),
    }))
}

/// 写屏蔽类型 + 屏蔽词，并通知运行中的会话重载。
///
/// `block_types`：传 null/缺省 = 不改；传数组 = 全量覆盖（空数组 = 清空屏蔽）。
/// `blacklist`：传 null/缺省 = 不改；传字符串或数组 = 全量覆盖。
#[tauri::command]
pub async fn danmaku_filter_set(
    state: tauri::State<'_, crate::AppState>,
    app: AppHandle,
    block_types: Option<Vec<String>>,
    blacklist: Option<Value>,
) -> Result<Value, String> {
    let mut wrote_any = false;

    if let Some(types) = block_types {
        // 只保留已知键 + 去重 + 固定顺序
        let mut kept: Vec<String> = Vec::new();
        for t in types {
            let s = t.trim().to_lowercase();
            if BLOCK_TYPE_KEYS.contains(&s.as_str()) && !kept.iter().any(|x| x == &s) {
                kept.push(s);
            }
        }
        let ordered: Vec<String> = BLOCK_TYPE_KEYS
            .iter()
            .filter(|k| kept.iter().any(|x| x == *k))
            .map(|k| k.to_string())
            .collect();
        let path = block_types_path(&app)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
        }
        let body = serde_json::to_string(&ordered).map_err(|e| format!("序列化屏蔽类型失败: {e}"))?;
        std::fs::write(&path, body + "\n").map_err(|e| format!("写入 danmaku_block_types.json 失败: {e}"))?;
        // 真源已迁到用户目录，必须让 Lua 的 options.block_types_path 指向它
        // （默认值 `~~/scripts/uosc_danmaku/danmaku_block_types.json` 会读安装目录那份，
        //  导致「面板改了但 Ctrl+K 看到的、实际生效的都不是同一份」）
        upsert_conf_line(&app, "block_types_path", &conf_slash(&path))?;
        wrote_any = true;
    }

    if let Some(bl) = blacklist {
        // 接受字符串（每行一条）或字符串数组；统一归一成去重后的行列表
        let raw_lines: Vec<String> = match bl {
            Value::String(s) => s.lines().map(|l| l.to_string()).collect(),
            Value::Array(arr) => arr
                .into_iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            other => return Err(format!("blacklist 参数类型不支持: {other}")),
        };
        let mut lines: Vec<String> = Vec::new();
        for l in raw_lines {
            let t = l.trim();
            if t.is_empty() {
                continue;
            }
            if !lines.iter().any(|x| x == t) {
                lines.push(t.to_string());
            }
        }
        let path = blacklist_path(&app)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
        }
        // 空列表 = 写空文件（不是删文件：删除会让 conf 里的路径指向不存在的文件，
        // parse.lua 的 load_blacklist_patterns 对不存在文件返回空表，行为等价但留个空文件更明确）
        let body = if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n") + "\n"
        };
        std::fs::write(&path, body).map_err(|e| format!("写入 danmaku_blacklist.txt 失败: {e}"))?;
        // 让 parse.lua 真的能找到它（绝对路径，因为文件不在 mpv 配置目录里）
        upsert_conf_line(&app, "blacklist_path", &conf_slash(&path))?;
        wrote_any = true;
    }

    if !wrote_any {
        return Err("block_types 与 blacklist 至少要传一个".into());
    }

    // 推给运行中的会话；没有会话就只落盘（下次加载生效）
    push_filter_reload(&state).await;

    let bt_path = block_types_path(&app)?;
    let bl_path = blacklist_path(&app)?;
    Ok(json!({
        "ok": true,
        "blockTypes": read_block_types(&bt_path),
        "blacklist": read_blacklist(&bl_path),
        // restartNeeded 恒 false：两条链路都能运行时重载（block_types 走 lc-1335 的
        // reload_block_types，黑名单走本批新增的 reload_blacklist）
        "restartNeeded": false,
    }))
}

/// 当前生效的屏蔽词条数（供设置面板显示「N 条规则」而不必拉全量）。
#[tauri::command]
pub fn danmaku_filter_count(app: AppHandle) -> Result<Value, String> {
    let bt = read_block_types(&block_types_path(&app)?);
    let bl = read_blacklist(&blacklist_path(&app)?);
    Ok(json!({ "blockTypes": bt.len(), "blacklist": bl.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_block_types_ignores_unknown_and_dedupes() {
        let dir = std::env::temp_dir().join(format!("anibox-bt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bt.json");
        std::fs::write(&p, r#"["top","TOP","bogus","color","top"]"#).unwrap();
        let got = read_block_types(&p);
        assert_eq!(got, vec!["top".to_string(), "color".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_block_types_handles_missing_and_garbage() {
        let dir = std::env::temp_dir().join(format!("anibox-bt2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 不存在
        assert!(read_block_types(&dir.join("nope.json")).is_empty());
        // 非 JSON
        let p = dir.join("bad.json");
        std::fs::write(&p, "not json").unwrap();
        assert!(read_block_types(&p).is_empty());
        // 是 JSON 但不是数组
        let p2 = dir.join("obj.json");
        std::fs::write(&p2, r#"{"top":true}"#).unwrap();
        assert!(read_block_types(&p2).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_blacklist_keeps_lua_pattern_chars() {
        let dir = std::env::temp_dir().join(format!("anibox-bl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bl.txt");
        // 含 Lua 特有的 %d、以及 -- 这种 Lua 里不是注释的文本
        std::fs::write(&p, "广告\n\n  %d+集  \nA--B\n").unwrap();
        let got = read_blacklist(&p);
        assert_eq!(got, vec!["广告".to_string(), "%d+集".to_string(), "A--B".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
