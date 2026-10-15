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
//! 端口分工：22347 = 弹幕 shim（本模块）；webrs 的嗅探 beacon 已让到 22348+（见 webrs.rs）。

use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const SHIM_PORT: u16 = 22347;

/// shim 脚本本体（随 bin 打包，见 tauri.conf.json 的 resources）
const SHIM_JS: &str = include_str!("../assets/danmaku-shim.cjs");

fn port_ready() -> bool {
    std::net::TcpStream::connect(("127.0.0.1", SHIM_PORT)).is_ok()
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
pub async fn ensure(
    mpv_path: &str,
    holder: &tokio::sync::Mutex<Option<std::process::Child>>,
) -> Result<(), String> {
    // 已在跑（例如上一个实例/自己先前拉过）→ 复用
    if port_ready() {
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
        "[danmaku-shim] 拉起 node {} {}（脚本目录 {}）",
        js.display(),
        SHIM_PORT,
        dir.display()
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
        .arg(SHIM_PORT.to_string())
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
        if port_ready() {
            crate::libmpv::trace("[danmaku-shim] 就绪（127.0.0.1:22347 已监听）");
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Err("弹幕 shim 启动超时（127.0.0.1:22347 未监听）".into())
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
        let mpv_path = st.config.read().await.mpv_path.clone();
        if let Err(e) = ensure(&mpv_path, &st.danmaku_child).await {
            crate::libmpv::trace(&format!("[danmaku-shim] 预热失败（不影响本地播放）: {e}"));
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
    let bases: Vec<String> = cfg
        .danmu_api_bases
        .iter()
        .map(|b| b.trim().trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty())
        .collect();
    let payload = serde_json::json!({
        "enabled": cfg.danmu_api_enabled,
        "base": cfg.danmu_api_base.trim().trim_end_matches('/'),
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

