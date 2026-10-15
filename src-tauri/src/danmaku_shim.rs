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

/// shim 脚本落到临时目录（`include_str!` 内嵌 → 首次运行写出，之后按内容长度判断是否需要刷新）
fn materialize_shim_js() -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join("anibox-danmaku-shim.cjs");
    let need_write = match std::fs::read(&path) {
        Ok(existing) => existing.len() != SHIM_JS.len(),
        Err(_) => true,
    };
    if need_write {
        std::fs::write(&path, SHIM_JS).map_err(|e| format!("写出弹幕 shim 脚本失败: {e}"))?;
    }
    Ok(path)
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
    let js = materialize_shim_js()?;

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
