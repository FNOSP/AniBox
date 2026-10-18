//! 内置自建弹幕源（danmu_api）—— 本地独立运行、零配置。
//!
//! 目标：用户不需要自己部署 danmu_api。应用随包携带上游源码（`src-tauri/resources/danmu-api`），
//! 启动时用本机 Node 拉起一个**只监听 127.0.0.1** 的实例，作为「自建弹幕源」的默认地址。
//!
//! 端口分工（沿用现有约定，避免互相踩）：
//!   22347 = 弹幕 shim（uosc_danmaku 硬编码）
//!   22348–22350 = webrs 嗅探 beacon 候选
//!   22351 = Rust 引擎下 Node 内部端口
//!   **22352 = 内置 danmu_api 主服务**；22353 = 上游附带的代理端口（可配置）
//!
//! 关键事实（已读上游源码确认，见 `THIRD_PARTY_NOTICES.md`）：
//!   · 鉴权是 **URL 路径前缀**（`http://host:port/<TOKEN>/api/v2/...`），不是 query。
//!   · 上游固定把配置写在 `<server.js 上级>/config/.env`，**没有配置目录环境变量**。
//!     所以随包目录只读时不能原地运行：这里把整棵运行树复制到可写的 AppData 目录，
//!     再在副本的 `config/.env` 里写入端口/TOKEN/监听地址。
//!   · 上游主端口读 `DANMU_API_PORT`；代理端口原本硬编码 5321，本仓库做了最小补丁
//!     支持 `DANMU_API_PROXY_PORT`；监听地址补丁支持 `DANMU_API_HOST`。
//!
//! 生命周期：幂等启动 → 就绪探测（HTTP GET `/`，最长 15s）→ 失败只记日志 → 退出 kill。
//! 父进程退出时 stdin 管道关闭，`anibox-launch.cjs` 收到 end 后主动退出，避免孤儿进程。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tauri::{AppHandle, Manager};

/// 内置 danmu_api 主服务端口。
pub const BUILTIN_PORT: u16 = 22352;
/// 上游附带的代理端口（只监听本机；避免与常见 5321 冲突）。
pub const BUILTIN_PROXY_PORT: u16 = 22353;
/// 内置实例固定 TOKEN。上游按「路径第一段」校验，客户端 base 直接带上即可。
pub const BUILTIN_TOKEN: &str = "anibox-local";
/// 运行树版本标记：上游版本变化或本模块调整时需要更新，触发重新复制。
const VENDOR_VERSION: &str = "danmu-api-1.21.6-anibox1";

const SERVER_ENTRY: &str = "danmu_api/server.js";
const LAUNCHER: &str = "anibox-launch.cjs";

/// 内置实例的访问基址（已含 TOKEN 路径前缀）。
pub fn base() -> String {
    format!("http://127.0.0.1:{BUILTIN_PORT}/{BUILTIN_TOKEN}")
}

/// 运行树根目录：优先 `%LOCALAPPDATA%/AniBox/danmu-api-runtime`，否则 `%TEMP%`。
fn runtime_dir() -> PathBuf {
    match std::env::var_os("LOCALAPPDATA") {
        Some(d) => PathBuf::from(d).join("AniBox").join("danmu-api-runtime"),
        None => std::env::temp_dir().join("anibox-danmu-builtin"),
    }
}

/// 随包资源根目录：打包后为 `resource_dir()`；开发期为 `src-tauri/resources`。
fn resource_root(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(d) = app.path().resource_dir() {
        if d.is_dir() {
            return Some(d);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    if dev.is_dir() {
        return Some(dev);
    }
    None
}

/// 上游源码目录（必须含 `danmu_api/server.js`）。
fn vendor_dir(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(d) = app.path().resource_dir() {
        candidates.push(d.join("danmu-api"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("danmu-api"));
            candidates.push(dir.join("resources").join("danmu-api"));
        }
    }
    // 开发期兜底
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("danmu-api"),
    );
    candidates
        .into_iter()
        .find(|d| d.join(SERVER_ENTRY).is_file())
}

/// 解析 Node 运行时：优先随包/显式配置，其次系统 PATH。
///
/// 当前仓库**没有**随包 node.exe（已核查），所以正常路径是系统 PATH；
/// 若将来把 node 放进 resources 或 exe 同目录，会优先生效。
pub fn resolve_node(app: &AppHandle) -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("ANIBOX_NODE_PATH") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("node.exe"));
            candidates.push(dir.join("node").join("node.exe"));
        }
    }
    if let Some(root) = resource_root(app) {
        candidates.push(root.join("node.exe"));
        candidates.push(root.join("node").join("node.exe"));
    }
    for c in candidates {
        if c.is_file() {
            return Ok(c);
        }
    }
    // 系统 PATH
    let probe = Command::new("node")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();
    if probe.map(|o| o.status.success()).unwrap_or(false) {
        return Ok(PathBuf::from("node"));
    }
    Err("未找到 Node.js（可设置 ANIBOX_NODE_PATH，或安装系统 Node.js）".into())
}

/// 递归复制目录（跳过 `node_modules/.bin` 里的符号链接不影响，普通文件即可）。
fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目录 {} 失败: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("读取 {} 失败: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)
                .map_err(|e| format!("复制 {} 失败: {e}", from.display()))?;
        }
        // 符号链接/其它类型直接跳过（上游源码与 npm 运行时依赖里没有必须保留的链接）
    }
    Ok(())
}

/// 写入运行副本的 `config/.env`：从 `.env.example` 继承默认项，再覆盖端口/TOKEN/监听地址。
fn write_config_env(runtime: &Path) -> Result<(), String> {
    let config_dir = runtime.join("config");
    std::fs::create_dir_all(&config_dir).map_err(|e| format!("创建 config 失败: {e}"))?;
    let example = config_dir.join(".env.example");
    let mut lines: Vec<String> = std::fs::read_to_string(&example)
        .map(|s| s.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default();
    let keys = [
        "TOKEN",
        "DANMU_API_PORT",
        "DANMU_API_PROXY_PORT",
        "DANMU_API_HOST",
    ];
    lines.retain(|l| {
        let t = l.trim_start();
        !keys
            .iter()
            .any(|k| t.starts_with(&format!("{k}=")) || t.starts_with(&format!("# {k}=")))
    });
    lines.push(format!("TOKEN={BUILTIN_TOKEN}"));
    lines.push(format!("DANMU_API_PORT={BUILTIN_PORT}"));
    lines.push(format!("DANMU_API_PROXY_PORT={BUILTIN_PROXY_PORT}"));
    lines.push("DANMU_API_HOST=127.0.0.1".to_string());
    let body = lines.join("\n") + "\n";
    std::fs::write(config_dir.join(".env"), body)
        .map_err(|e| format!("写 config/.env 失败: {e}"))
}

/// 启动器：上游 server.js 不读 stdin，这里补一个「父进程退出即退出」的看门脚本。
const LAUNCHER_JS: &str = r#"// AniBox 内置 danmu_api 启动器：父进程（Tauri）退出时 stdin 关闭 → 主动退出。
process.stdin.resume();
process.stdin.on('end', () => process.exit(0));
process.on('SIGTERM', () => process.exit(0));
import('./danmu_api/server.js').catch((e) => {
  console.error('[anibox-launcher] failed to start:', e);
  process.exit(1);
});
"#;

/// 准备可写运行树（幂等）：版本标记匹配则跳过复制。
fn prepare_runtime_blocking(vendor: &Path, runtime: &Path) -> Result<(), String> {
    let marker = runtime.join(".anibox-version");
    let version_ok = std::fs::read_to_string(&marker)
        .map(|s| s.trim() == VENDOR_VERSION)
        .unwrap_or(false);
    let tree_ok = runtime.join(SERVER_ENTRY).is_file()
        && runtime.join("node_modules").is_dir()
        && runtime.join("config/.env.example").is_file();

    if !(version_ok && tree_ok) {
        if runtime.exists() {
            std::fs::remove_dir_all(runtime)
                .map_err(|e| format!("清理旧运行目录 {} 失败: {e}", runtime.display()))?;
        }
        copy_dir(vendor, runtime)?;
        std::fs::write(runtime.join(LAUNCHER), LAUNCHER_JS)
            .map_err(|e| format!("写启动器失败: {e}"))?;
        write_config_env(runtime)?;
        std::fs::write(&marker, VENDOR_VERSION).map_err(|e| format!("写版本标记失败: {e}"))?;
    } else {
        // 用户可能删过 .env：补回来即可，不重拷整棵树。
        if !runtime.join("config/.env").is_file() {
            write_config_env(runtime)?;
        }
        if !runtime.join(LAUNCHER).is_file() {
            std::fs::write(runtime.join(LAUNCHER), LAUNCHER_JS)
                .map_err(|e| format!("写启动器失败: {e}"))?;
        }
    }
    Ok(())
}

/// 串行化整个 ensure（含运行树复制与进程启动），避免 warmup/reconfigure 并发复制。
static ENSURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 就绪探测：`GET http://127.0.0.1:22352/` 返回 200。
async fn probe() -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Ok(mut s) = tokio::net::TcpStream::connect(("127.0.0.1", BUILTIN_PORT)).await else {
        return false;
    };
    let req = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if s.write_all(req).await.is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    match tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => {
            let head = String::from_utf8_lossy(&buf[..n]);
            head.starts_with("HTTP/") && head.contains(" 200")
        }
        _ => false,
    }
}

fn log_line(msg: &str) {
    crate::libmpv::trace(&format!("[danmu-builtin] {msg}"));
}

/// 端口是否已被监听（仅用于启动前预检）。
fn port_in_use(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// 幂等确保内置实例就绪。
///
/// · 端口已就绪 → 直接复用（不杀别人的进程）。
/// · 已持有子进程但端口不通 → 收掉重拉。
/// · 任何失败只返回错误，由调用方记日志并降级（不影响本地播放）。
pub async fn ensure(
    app: &AppHandle,
    holder: &tokio::sync::Mutex<Option<Child>>,
) -> Result<(), String> {
    // S2：同一时刻只允许一个 ensure 进入（复制运行树 + 启动进程都串行化）。
    let _guard = ENSURE_LOCK.lock().await;

    if probe().await {
        return Ok(());
    }
    // S3：代理端口 22353 必须空闲；被非本实例占用时上游会 bind 失败并退出。
    if port_in_use(BUILTIN_PROXY_PORT) {
        let msg = format!(
            "端口 {BUILTIN_PROXY_PORT} 已被其他进程占用，内置 danmu_api 无法启动代理服务"
        );
        log_line(&msg);
        return Err(msg);
    }
    {
        let mut g = holder.lock().await;
        if let Some(mut c) = g.take() {
            let _ = c.kill();
        }
    }

    let vendor = vendor_dir(app)
        .ok_or_else(|| "未找到内置 danmu_api 源码（resources/danmu-api）".to_string())?;
    let node = resolve_node(app)?;
    let runtime = runtime_dir();

    let vendor2 = vendor.clone();
    let runtime2 = runtime.clone();
    tokio::task::spawn_blocking(move || prepare_runtime_blocking(&vendor2, &runtime2))
        .await
        .map_err(|e| format!("准备运行目录失败: {e}"))??;

    let log_path = std::env::temp_dir().join("anibox-danmu-builtin.log");
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();
    let err = out.as_ref().and_then(|f| f.try_clone().ok());

    log_line(&format!(
        "拉起 {} {}（cwd {}，端口 {BUILTIN_PORT}）",
        node.display(),
        LAUNCHER,
        runtime.display()
    ));

    let mut cmd = Command::new(&node);
    cmd.arg(LAUNCHER)
        .current_dir(&runtime)
        .env("DANMU_API_PORT", BUILTIN_PORT.to_string())
        .env("DANMU_API_PROXY_PORT", BUILTIN_PROXY_PORT.to_string())
        .env("DANMU_API_HOST", "127.0.0.1")
        .env("TOKEN", BUILTIN_TOKEN)
        .stdin(Stdio::piped())
        .stdout(out.map(Stdio::from).unwrap_or_else(Stdio::null))
        .stderr(err.map(Stdio::from).unwrap_or_else(Stdio::null));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("拉起内置 danmu_api 失败（Node: {}）: {e}", node.display()))?;
    *holder.lock().await = Some(child);

    for _ in 0..75 {
        if probe().await {
            log_line("就绪（127.0.0.1:22352）");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err("内置 danmu_api 启动超时（127.0.0.1:22352 未就绪，详见 %TEMP%/anibox-danmu-builtin.log）".into())
}

/// 应用启动/配置变化时后台预热（失败只记日志）。
pub fn spawn_warmup(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let st = app.state::<crate::AppState>();
        let active = {
            let cfg = st.config.read().await;
            cfg.danmu_api_enabled && crate::danmaku::uses_builtin(&cfg)
        };
        if !active {
            return;
        }
        if let Err(e) = ensure(&app, &st.builtin_child).await {
            log_line(&format!("预热失败（不影响本地播放，将降级内置 B站）: {e}"));
        }
    });
}

/// 配置变化后重配：不再使用内置源则收掉子进程；仍使用则确保就绪。
pub fn spawn_reconfigure(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let st = app.state::<crate::AppState>();
        let active = {
            let cfg = st.config.read().await;
            cfg.danmu_api_enabled && crate::danmaku::uses_builtin(&cfg)
        };
        if !active {
            if let Some(mut c) = st.builtin_child.lock().await.take() {
                let _ = c.kill();
                log_line("已停用内置源，收掉内置 danmu_api 子进程");
            }
            return;
        }
        if let Err(e) = ensure(&app, &st.builtin_child).await {
            log_line(&format!("重配失败: {e}"));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_contains_token_path_prefix() {
        assert_eq!(base(), "http://127.0.0.1:22352/anibox-local");
    }

    #[test]
    fn ports_do_not_collide_with_existing_ranges() {
        assert_eq!(BUILTIN_PORT, 22352);
        assert_eq!(BUILTIN_PROXY_PORT, 22353);
        assert!(![22347u16, 22348, 22349, 22350, 22351].contains(&BUILTIN_PORT));
    }
}
