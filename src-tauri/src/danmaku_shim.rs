//! [lc-1560] 弹幕 shim 守护：给 MPV 的 uosc_danmaku Lua 脚本提供 127.0.0.1:22347 的 HTTP 端点。
//!
//! 背景（迁移遗留）：主仓库用 Electron 主进程内的 `playbackShim.ts` 提供这组端点
//! （Lua 侧硬编码 `127.0.0.1:22347`，见 uosc_danmaku 的 main.lua / menu.lua / apis/extra.lua）。
//! Tauri2 迁移时这部分没带过来 ⇒ B站弹幕下载失败 ⇒ 弹幕只剩弹弹play 兜底
//! （mpv 日志实证：「自动补源：B站弹幕下载失败（确认本地代理 127.0.0.1:22347 已随应用启动）」）。
//!
//! 做法：**不重写 B站 逻辑**，直接复用 mpv 便携配置里现成的 `bili_danmaku.js`
//! （纯 Node 内置模块，实测可独立 require）。Rust 侧只负责：
//!   ① 找到脚本目录（由 `mpv_path` 推导：<mpv 目录>/portable_config/scripts/uosc_danmaku）
//!   ② 用系统 node 拉起 `assets/danmaku-shim.js`（HTTP 路由 → 调 JS → 回 JSON）
//!   ③ 探活端口；退出时收掉子进程
//!
//! 端口分工：22347 = 弹幕 shim（本模块）；webrs 的嗅探 beacon 从 22348 起找
//! （[22348,22349,22350]，见 webrs.rs::ensure_beacon）—— 所以 Rust 引擎下 Node 内部端口
//! 固定用 22351，**必须避开 beacon 候选范围**，否则 beacon 先占 22348 时 Node 起不来。

use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const SHIM_PORT: u16 = 22347;
/// Rust 引擎开启时 Node shim 退到内部端口（B站/字幕反代目标）。
/// 22351 刻意避开 webrs beacon 的候选范围 [22348,22349,22350]。
pub const NODE_INTERNAL_PORT: u16 = 22351;

/// shim 脚本本体（随 bin 打包，见 tauri.conf.json 的 resources）
const SHIM_JS: &str = include_str!("../assets/danmaku-shim.cjs");

fn port_ready(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// [lc-1561] Node 侧要落盘的源码：shim 本体 + 移植的两个模块 + 支撑桩。
///
/// 全部 `include_str!` 编进二进制（打包不需要额外的 resources 条目），首次运行写出；
/// 之后按**内容**判断是否需要刷新（比长度可靠：改注释长度不变也要重写）。
const NODE_FILES: &[(&str, &str)] = &[
    ("anibox-danmaku-shim.cjs", SHIM_JS),
    // 主仓库 danmuApi.ts / biliRunner.ts 的 tsc 转译产物（业务逻辑一字未改）
    (
        "anibox-danmu-api.cjs",
        include_str!("../assets/danmu-api.cjs"),
    ),
    (
        "anibox-bili-runner.cjs",
        include_str!("../assets/bili-runner.cjs"),
    ),
    // 上述两者的环境桩（electron/logger/fn_config/proxyAgent + 脚本目录通道）
    (
        "anibox-danmu-deps.cjs",
        include_str!("../assets/danmu-api-deps.cjs"),
    ),
];

/// 由 mpv 路径推导 uosc_danmaku 脚本目录：<mpv 目录>/portable_config/scripts/uosc_danmaku
pub fn script_dir_from_mpv(mpv_path: &str) -> Option<PathBuf> {
    let exe = Path::new(mpv_path);
    let dir = exe.parent()?;
    let cand = dir.join("portable_config").join("scripts").join("uosc_danmaku");
    if cand.join("bili_danmaku.js").is_file() {
        return Some(cand);
    }
    // 兜底：便携配置也可能叫 scripts（少数布局）；再试一层根目录下的 scripts
    let cand2 = dir.join("scripts").join("uosc_danmaku");
    if cand2.join("bili_danmaku.js").is_file() {
        return Some(cand2);
    }
    None
}

/// [lc-1561] 移植产物之间的相互 require 用的是**中性文件名**（anibox-danmu-api.cjs 等），
/// 所以落盘时统一改名 —— 保留 assets 里带语义的原文件名便于对照上游。
/// 顺序：shim 引 `bili-runner` → `danmu-api` → `deps`，全部先落盘再启动。
fn rewrite_requires(src: &str) -> String {
    src.replace("./danmu-api.cjs", "./anibox-danmu-api.cjs")
        .replace("./bili-runner.cjs", "./anibox-bili-runner.cjs")
        .replace("./danmu-api-deps.cjs", "./anibox-danmu-deps.cjs")
}

/// 落盘全部 Node 源码（内容变化的文件才重写），返回 shim 主文件路径。
fn materialize_node_files() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir();
    let mut shim_path = dir.join("anibox-danmaku-shim.cjs");
    for (name, src) in NODE_FILES {
        let path = dir.join(name);
        let body = rewrite_requires(src);
        let need_write = match std::fs::read(&path) {
            Ok(existing) => existing != body.as_bytes(),
            Err(_) => true,
        };
        if need_write {
            std::fs::write(&path, body).map_err(|e| format!("写出 {name} 失败: {e}"))?;
        }
        if *name == "anibox-danmaku-shim.cjs" {
            shim_path = path;
        }
    }
    Ok(shim_path)
}

/// 就绪则直接返回；否则拉起 node 跑 shim 并等待端口监听（最长 10s）。
///
/// `mpv_path`：本机 mpv.exe 路径（用于推导脚本目录）。
/// `holder`：会话级子进程槽（进程常驻，退出时由调用方收）。
/// `engine`：`"node"`（默认，Node 监听 22347）| `"rust"`（Rust 监听 22347，Node 退到 22348）。
pub async fn ensure(
    mpv_path: &str,
    holder: &tokio::sync::Mutex<Option<std::process::Child>>,
    engine: &str,
) -> Result<(), String> {
    let rust_mode = engine.eq_ignore_ascii_case("rust");
    let node_port = if rust_mode { NODE_INTERNAL_PORT } else { SHIM_PORT };

    // Rust 引擎：先确保 Rust HTTP 服务已接管 22347（幂等）
    if rust_mode {
        crate::danmaku::start_engine(NODE_INTERNAL_PORT)?;
    }

    // Node 侧已在跑 → 复用
    if port_ready(node_port) {
        return Ok(());
    }
    // 已持有子进程但端口没通 → 先收掉再重拉（避免僵尸占位）
    {
        let mut g = holder.lock().await;
        if let Some(mut c) = g.take() {
            let _ = c.kill();
        }
    }

    let dir = script_dir_from_mpv(mpv_path)
        .ok_or_else(|| format!("未找到 uosc_danmaku 脚本目录（由 mpv 路径推导）：{mpv_path}"))?;
    let js = materialize_node_files()?;

    crate::libmpv::trace(&format!(
        "[danmaku-shim] 拉起 node {} {}（脚本目录 {}，引擎 {}）",
        js.display(),
        node_port,
        dir.display(),
        if rust_mode { "rust" } else { "node" }
    ));

    use std::os::windows::process::CommandExt as _;
    // [lc-1560] shim 的输出**落日志文件**而不是 null：它的诊断只在 stderr/stdout 里
    // （B站 搜索/分P/cid 解析、Cookie 体检、异常栈）。原实现丢进 Stdio::null 导致
    // 弹幕出问题时"应用日志里一个字都没有"，只能靠进程列表猜 —— 迁移期这个盲区
    // 正是本模块被漏掉整轮的原因。文件与 mpv 日志同级（%TEMP%），追加写。
    let shim_log = std::env::temp_dir().join("anibox-danmaku-shim.log");
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&shim_log)
        .ok();
    let err = out.as_ref().and_then(|f| f.try_clone().ok());
    let child = std::process::Command::new("node")
        .arg(&js)
        .arg(&dir)
        .arg(node_port.to_string())
        // stdin 用 piped：JS 侧监听 stdin end → 父进程（我们）退出时自动跟退，不留孤儿
        .stdin(Stdio::piped())
        .stdout(out.map(Stdio::from).unwrap_or_else(Stdio::null))
        .stderr(err.map(Stdio::from).unwrap_or_else(Stdio::null))
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .map_err(|e| {
            format!("拉起弹幕 shim 失败（需要系统装有 Node.js）: {e}")
        })?;
    *holder.lock().await = Some(child);

    for _ in 0..50 {
        if port_ready(node_port) {
            crate::libmpv::trace(&format!(
                "[danmaku-shim] 就绪（127.0.0.1:{node_port} 已监听）"
            ));
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err(format!(
        "弹幕 shim 启动超时（127.0.0.1:{node_port} 未监听）"
    ))
}

/// [lc-1561] 同步「自建弹幕接口」开关到 uosc_danmaku 的 script-opts（Lua 侧闸门）。
///
/// 为什么需要：Lua 的自动补源判断是 `options.auto_load_extra or options.danmu_api_enabled`
/// （main.lua:909/972、apis/dandanplay.lua:368/466）。用户若关掉「B站弹幕搜索」而只开自建源，
/// 没有这个键 Lua 根本不会去请求本地 shim —— 自建源就永远不会被问到。
///
/// ⚠️ **绝不把服务地址写进 conf**（对齐主仓库 lc-1104）：portable_config 是 git 跟踪、
/// 随安装包分发的目录，写进去等于把用户的内网地址（含 TOKEN 路径）摊到工作区与发布目录。
/// 地址只留在应用自己的 config.json；Lua 侧也零读取（真正发请求的是 Node 客户端）。
/// 未启用或地址为空 → **删键**（options.lua 默认 false ⇒ 完全走内置 B站 链路）。
pub fn sync_lua_gate(mpv_path: &str, enabled: bool, base: &str) {
    let Some(script_dir) = script_dir_from_mpv(mpv_path) else {
        return;
    };
    // script-opts 与 scripts 同级
    let Some(portable) = script_dir.parent().and_then(|p| p.parent()) else {
        return;
    };
    let conf_dir = portable.join("script-opts");
    let target = conf_dir.join("uosc_danmaku.conf");

    let on = enabled && !base.trim().is_empty();
    let mut lines: Vec<String> = std::fs::read_to_string(&target)
        .map(|s| s.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default();
    // 清掉历史行（含地址行：老版本可能写过，升级后自动消失）
    lines.retain(|l| {
        let t = l.trim_start();
        !t.starts_with("danmu_api_enabled=")
            && !t.starts_with("danmu_api_base=")
            && !t.starts_with("# 自建弹幕接口")
    });
    while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    if on {
        lines.push("# 自建弹幕接口（danmu_api）：由应用设置面板控制，作为弹幕优选源；未命中自动降级内置B站弹幕".into());
        lines.push("danmu_api_enabled=yes".into());
    }
    if std::fs::create_dir_all(&conf_dir).is_err() {
        return;
    }
    let mut body = lines.join("\n");
    body.push('\n');
    if let Err(e) = std::fs::write(&target, body) {
        crate::libmpv::trace(&format!("[danmaku-shim] 写自建源闸门失败: {e}"));
    }
}

/// 应用启动时后台预热（用户首次播放时 shim 已就绪，省一次冷启动等待）。
pub fn spawn_warmup(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        use tauri::Manager;
        let st = app.state::<crate::AppState>();
        let (mpv_path, engine) = {
            let cfg = st.config.read().await;
            (
                cfg.mpv_path.clone(),
                crate::danmaku::resolve_engine(&cfg).to_string(),
            )
        };
        if let Err(e) = ensure(&mpv_path, &st.danmaku_child, &engine).await {
            crate::libmpv::trace(&format!("[danmaku-shim] 预热失败（不影响本地播放）: {e}"));
        }
    });
}

/// 引擎切换（运行期）：收掉 Node 子进程、停 Rust 服务，再按新引擎重启。
///
/// 由 `save_config` 在检测到 `danmu_engine` 变化时触发。切换期间弹幕请求可能短暂失败
/// （Lua 侧有重试/降级），属预期。
pub fn spawn_reconfigure(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        use tauri::Manager;
        let st = app.state::<crate::AppState>();
        let (mpv_path, engine) = {
            let cfg = st.config.read().await;
            (
                cfg.mpv_path.clone(),
                crate::danmaku::resolve_engine(&cfg).to_string(),
            )
        };
        crate::libmpv::trace(&format!("[danmaku-shim] 引擎切换为 {engine}，正在重启服务"));
        if let Some(mut c) = st.danmaku_child.lock().await.take() {
            let _ = c.kill();
        }
        crate::danmaku::stop_engine();
        // 给旧端口一点释放时间
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if let Err(e) = ensure(&mpv_path, &st.danmaku_child, &engine).await {
            crate::libmpv::trace(&format!("[danmaku-shim] 引擎切换重启失败: {e}"));
        }
    });
}

/// [lc-1561] 把设置面板的自建弹幕接口配置**桥接**给 Node 侧客户端。
///
/// shim 里的 `danmu-api-deps.cjs` 每次读盘，所以这里只要在配置变化时覆盖写一次即可 ——
/// 用户在设置面板改完**立即生效**，不需要重启应用、也不需要重启 shim（与主仓库 fnConfig
/// 的「每次读 config.json」行为等价）。
///
/// 写在 %TEMP%（与 shim 自身的落点一致）。注意：**不含任何第三方凭据**，只有用户自建的
/// 服务地址（用户自己的内网地址，本来也要落进 config.json）。
pub fn write_danmu_config(cfg: &crate::config::Config) -> Result<(), String> {
    let path = std::env::temp_dir().join("anibox-danmu-config.json");
    // 零配置：用户没填地址时自动指向内置实例（含 TOKEN 路径前缀）。
    let bases = crate::danmaku::effective_bases(cfg);
    let base = crate::danmaku::effective_base(cfg);
    let payload = serde_json::json!({
        "enabled": cfg.danmu_api_enabled,
        "base": base,
        "bases": bases,
        // 飞牛地址/令牌：系列记忆键解析（play-meta 缺失时的兜底）与自建源无关，仅供桩里的
        // readConfig 兼容主仓库形状；真源仍是 config.json 本身。
        "server": cfg.server,
        "token": cfg.token,
    });
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写自建源配置失败: {e}"))?;
    Ok(())
}

// ───────────────────── 弹幕缓存清理（设置面板「清空缓存并重新搜索」）─────────────────────
//
// 缓存目录与键（与 uosc_danmaku/apis/extra.lua:565-566、danmaku-shim.cjs:84-95、
// xml.rs:52-64 同口径）：
//   %PUBLIC%\fnos-danmaku（首选；回退 %ProgramData%、%TEMP%）
//   B站/自建源合并结果：bili_danmaku_<safe_title>_<episode>.xml
//   （safe_title = 解析出的番名去掉 \/:*?"<>|；episode = 集数，电影/无集为 0）
// 其它运行期文件（danmaku-<pid>.ass/json、dandanplay_<id>.json、danmaku_source_cache.json）
// 由 Lua 的 clear_danmaku_refetch 负责，这里只处理按条目可定位的 XML 缓存。

/// 弹幕 XML 缓存根目录（去重，只返回存在的目录由调用方判断）。
pub fn cache_roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for var in ["PUBLIC", "ProgramData"] {
        if let Ok(base) = std::env::var(var) {
            let base = base.trim();
            if !base.is_empty() {
                out.push(Path::new(base).join("fnos-danmaku"));
            }
        }
    }
    out.push(std::env::temp_dir().join("fnos-danmaku"));
    out
}

fn re_se() -> &'static regex::Regex {
    static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?i)^(.*?)\s*-\s*S\d+\s*E\s*(\d+)").unwrap())
}

fn re_cn_ep() -> &'static regex::Regex {
    static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"第\s*(\d+)\s*[话集回話]").unwrap())
}

/// 从 mpv 的 `media-title`（`剧名 - S1E2: 集名`）解析出「番名 + 集数」。
/// 与 Lua 的 bili_fast_parse/guess_bili_title_ep_v2 口径一致：番名取 ` - SxEy` 之前，
/// 无季集标记时整串都当番名（电影/单集 → ep=0）。
pub fn parse_media_title(media_title: &str) -> (String, i64) {
    let s = media_title.trim();
    if s.is_empty() {
        return (String::new(), 0);
    }
    if let Some(c) = re_se().captures(s) {
        let title = c.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        let ep = c.get(2).and_then(|m| m.as_str().parse::<i64>().ok()).unwrap_or(0);
        if !title.is_empty() {
            return (title, ep);
        }
    }
    if let Some(c) = re_cn_ep().captures(s) {
        let ep = c.get(1).and_then(|m| m.as_str().parse::<i64>().ok()).unwrap_or(0);
        let title = s[..c.get(0).map(|m| m.start()).unwrap_or(0)].trim().to_string();
        if !title.is_empty() {
            return (title, ep);
        }
    }
    (s.to_string(), 0)
}

/// 比较用的标题归一化：去掉空白与标点，统一小写（保留 CJK/字母/数字）。
fn norm_title(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !matches!(
                    c,
                    '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '，' | '。' | '！'
                        | '？' | '、' | '；' | '：' | '·' | '—' | '–' | '-' | '_' | '.' | '('
                        | ')' | '[' | ']' | '【' | '】' | '「' | '」' | '『' | '』' | '!' | ','
                        | ';' | '\'' | '’' | '“' | '”' | '《' | '》'
                )
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 判断缓存文件名是否属于「当前条目」（纯函数，便于单测）。
fn cache_file_matches(name: &str, wanted_title: &str, episode: i64) -> bool {
    if wanted_title.is_empty() {
        return false;
    }
    let Some(stem) = name.strip_suffix(".xml") else { return false };
    let Some(rest) = stem.strip_prefix("bili_danmaku_") else { return false };
    let Some((title_part, ep_str)) = rest.rsplit_once('_') else { return false };
    let Ok(ep) = ep_str.parse::<i64>() else { return false };
    if ep != episode {
        return false;
    }
    let nf = norm_title(title_part);
    if nf.is_empty() {
        return false;
    }
    nf.contains(wanted_title) || (nf.chars().count() >= 2 && wanted_title.contains(&nf))
}

/// 收集「当前条目」对应的缓存 XML。定位规则：
///   · 文件名必须是 `bili_danmaku_<title>_<ep>.xml` 且 ep 与当前集数一致；
///   · 文件名里的番名（归一化后）必须包含当前番名（防止误删同集数的其它番）。
/// 定位不到（番名为空）时返回空 —— 调用方应提示改用「清空全部缓存」。
pub fn collect_current_cache_files(title: &str, episode: i64) -> Vec<PathBuf> {
    let nt = norm_title(title);
    if nt.is_empty() {
        return Vec::new();
    }
    let mut files = Vec::new();
    for root in cache_roots() {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if cache_file_matches(name, &nt, episode) {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// 收集「全部弹幕缓存」文件（`bili_danmaku_*.xml` + 运行期 danmaku 中间文件）。
pub fn collect_all_cache_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in cache_roots() {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let hit = name.starts_with("bili_danmaku_")
                || name.starts_with("bili_d")
                || name.starts_with("dandanplay_")
                || (name.starts_with("danmaku-") && (name.ends_with(".ass") || name.ends_with(".json")))
                || (name.starts_with("bahamut-") && name.ends_with(".json"));
            if hit {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// 先移到临时目录再删（失败可回滚）：全部移动成功后才删除备份目录。
/// 任一移动失败 → 把已移动的文件移回原处并返回错误，保证原缓存不被破坏。
fn move_to_trash_then_delete(paths: &[PathBuf]) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let trash = std::env::temp_dir().join(format!("anibox-danmaku-trash-{stamp}"));
    std::fs::create_dir_all(&trash)
        .map_err(|e| format!("创建弹幕缓存备份目录失败：{e}"))?;

    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for p in paths {
        let Some(fname) = p.file_name() else { continue };
        let dest = trash.join(fname);
        match std::fs::rename(p, &dest) {
            Ok(()) => moved.push((p.clone(), dest)),
            Err(e) => {
                for (orig, bak) in moved.iter().rev() {
                    let _ = std::fs::rename(bak, orig);
                }
                let _ = std::fs::remove_dir_all(&trash);
                return Err(format!("移动弹幕缓存失败（已回滚，原缓存保留）：{e}"));
            }
        }
    }

    let names = moved
        .iter()
        .filter_map(|(p, _)| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
        .collect();
    // 删除失败不影响结果（文件已移出缓存目录），只是临时目录里会留一份备份。
    let _ = std::fs::remove_dir_all(&trash);
    Ok(names)
}

/// 清空「当前条目」缓存，返回被清理的文件名。
pub fn clear_current_cache(title: &str, episode: i64) -> Result<Vec<String>, String> {
    if norm_title(title).is_empty() {
        return Err("无法从当前条目标题定位弹幕缓存，请改用「清空全部缓存」".into());
    }
    let files = collect_current_cache_files(title, episode);
    move_to_trash_then_delete(&files)
}

/// 清空全部弹幕缓存，返回被清理的文件名。
pub fn clear_all_cache() -> Result<Vec<String>, String> {
    let files = collect_all_cache_files();
    move_to_trash_then_delete(&files)
}

/// [lc-1561] 把本次播放的条目元信息写给 Node 侧：自建源的「系列级匹配记忆」需要季 guid
/// （原项目 `resolveSeriesKey` 要再发一次 play/info HTTP 才拿到；我们在 play_item 里已经
/// 解析过响应，直传更省一次往返）。
pub fn write_play_meta(episode_guid: &str, parent_guid: &str) {
    let path = std::env::temp_dir().join("anibox-play-meta.json");
    let payload = serde_json::json!({
        "episodeGuid": episode_guid,
        "parentGuid": parent_guid,
    });
    if let Ok(s) = serde_json::to_string_pretty(&payload) {
        let _ = std::fs::write(&path, s);
    }
}


#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn parse_media_title_se_and_cn() {
        let (t, e) = parse_media_title("世界最强魔女，就此开始 - S1E2: 试着使用了互联网！");
        assert_eq!(t, "世界最强魔女，就此开始");
        assert_eq!(e, 2);

        let (t, e) = parse_media_title("某剧场版");
        assert_eq!(t, "某剧场版");
        assert_eq!(e, 0);

        let (t, e) = parse_media_title("番名 第12话 副标题");
        assert_eq!(t, "番名");
        assert_eq!(e, 12);
    }

    #[test]
    fn cache_file_match_requires_episode_and_title() {
        let nt = norm_title("世界最强魔女，就此开始");
        // 同番同集（Lua 侧解析出的标题会带副标题）→ 命中
        assert!(cache_file_matches(
            "bili_danmaku_世界最强魔女，就此开始 - 试着使用了互联网！_2.xml",
            &nt,
            2
        ));
        // 同番不同集 → 不命中
        assert!(!cache_file_matches(
            "bili_danmaku_世界最强魔女，就此开始 - 试着使用了互联网！_3.xml",
            &nt,
            2
        ));
        // 同集但别的番 → 不命中
        assert!(!cache_file_matches("bili_danmaku_另一部番_2.xml", &nt, 2));
        // 非弹幕缓存文件 → 不命中
        assert!(!cache_file_matches("danmaku-1234.ass", &nt, 2));
    }
}
