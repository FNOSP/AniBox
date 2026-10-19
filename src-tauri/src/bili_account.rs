//! [bili-account] B 站账号（Cookie）读写 + Node 环境检测 + 一键自建弹幕源。
//!
//! 现有能力盘点（改造前）：
//!   · Cookie 存储：`<uosc_danmaku 脚本目录>/bili_cookie.txt`（单行，SESSDATA=...；整串 Cookie 头）。
//!     由 `third_party/fntv-mpv/.../bili_danmaku.js` 的 `_load_cookie()` 读取，
//!     `cookie_check()` 调 `api.bilibili.com/x/web-interface/nav` 判断 `isLogin` 并回 `uname`。
//!   · 弹幕 shim 端点 `GET /danmaku-cookie-status`（danmaku-shim.cjs）复用同一份体检逻辑，
//!     并把结果写入 `bili_cookie_status.json` 供 Lua 面板读。
//!   · 配置体系：`config.json`（config.rs）+ `danmu_api_enabled/bases/engine` 字段。
//!
//! 本模块只补「设置面板需要的读写面」，不改动上述消费端：
//!   · 读：优先问运行中的 shim（与面板/Lua 完全同一份体检），shim 未起时直连 nav 兜底。
//!   · 写：原子写 `bili_cookie.txt`（沿用现有存储，不新建孤立配置，也不把 Cookie 复制进 config.json）。
//!   · 清除：删除 `bili_cookie.txt`。
//!
//! 安全：Cookie 只用于请求头/落盘，**绝不写日志**；错误信息里也不回显 Cookie 内容。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, State};

use crate::AppState;
use crate::{config, danmaku_shim};

const NAV_URL: &str = "https://api.bilibili.com/x/web-interface/nav";
const SHIM_PORT: u16 = crate::danmaku_shim::SHIM_PORT;

/// 由当前 mpv 路径推导 uosc_danmaku 脚本目录（与弹幕 shim 使用同一套规则）。
pub(crate) async fn script_dir_async(state: &AppState) -> Option<PathBuf> {
    let mpv_path = state.config.read().await.mpv_path.clone();
    crate::danmaku_shim::script_dir_from_mpv(&mpv_path)
}

pub(crate) fn cookie_path(dir: &Path) -> PathBuf {
    dir.join("bili_cookie.txt")
}

fn read_cookie_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 归一化用户粘贴的 Cookie：去掉 `Cookie:` 前缀与换行，压缩空白，保留 `; ` 分隔。
/// 返回 Err 时只说明缺什么，**不回显输入内容**。
pub(crate) fn normalize_cookie(raw: &str) -> Result<String, String> {
    let mut s = raw.replace(['\r', '\n', '\t'], " ");
    s = s.trim().to_string();
    if let Some(rest) = s.strip_prefix("Cookie:") {
        s = rest.trim().to_string();
    }
    // 压缩连续空白（Cookie 值里不会出现空格；SESSDATA 已 URL 编码）
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    if s.is_empty() {
        return Err("Cookie 不能为空".into());
    }
    // 只粘贴了 SESSDATA 的值（无 `=`）→ 自动补键名
    if !s.contains('=') {
        s = format!("SESSDATA={s}");
    }
    if !s.contains("SESSDATA=") {
        return Err("Cookie 中缺少 SESSDATA（请从浏览器开发者工具复制完整 Cookie）".into());
    }
    Ok(s)
}

/// 原子写 Cookie 文件：先写同目录临时文件再 rename，避免半截文件被引擎读到。
pub(crate) fn write_cookie_file(path: &Path, cookie: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| "无法确定 Cookie 保存目录".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("创建脚本目录失败: {e}"))?;
    let tmp = path.with_extension("txt.tmp");
    std::fs::write(&tmp, format!("{cookie}\n")).map_err(|e| format!("写入 Cookie 失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("保存 Cookie 失败: {e}")
    })
}

/// 直连 B 站 nav 接口做一次体检（shim 未运行时的兜底）。
pub(crate) async fn nav_check(cookie: &str) -> Value {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return json!({
                "loggedIn": false,
                "status": "error",
                "reason": format!("构造请求失败: {e}"),
                "uname": Value::Null,
                "cookiePresent": true,
            })
        }
    };
    let resp = client
        .get(NAV_URL)
        .header("Cookie", cookie)
        .header("User-Agent", "Mozilla/5.0")
        .header("Referer", "https://www.bilibili.com")
        .send()
        .await;
    match resp {
        Ok(r) => match r.json::<Value>().await {
            Ok(v) => {
                let data = v.get("data").cloned().unwrap_or(Value::Null);
                let is_login = data.get("isLogin").and_then(|x| x.as_bool()).unwrap_or(false);
                let uname = data.get("uname").and_then(|x| x.as_str()).map(|s| s.to_string());
                json!({
                    "loggedIn": is_login,
                    "status": if is_login { "valid" } else { "expired" },
                    "reason": if is_login { Value::Null } else { json!("Cookie 已过期或无效") },
                    "uname": uname,
                    "cookiePresent": true,
                })
            }
            Err(e) => json!({
                "loggedIn": false,
                "status": "error",
                "reason": format!("B 站返回内容无法解析: {e}"),
                "uname": Value::Null,
                "cookiePresent": true,
            }),
        },
        Err(e) => json!({
            "loggedIn": false,
            "status": "error",
            "reason": format!("请求 B 站失败: {e}"),
            "uname": Value::Null,
            "cookiePresent": true,
        }),
    }
}

/// 问运行中的 shim 要一次体检结果（与 Lua 面板/播放链路同一份逻辑）。
/// 端口没起或响应异常返回 None，由调用方兜底。
async fn shim_cookie_status() -> Option<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .ok()?;
    let url = format!("http://127.0.0.1:{SHIM_PORT}/danmaku-cookie-status");
    let v = client.get(&url).send().await.ok()?.json::<Value>().await.ok()?;
    Some(v)
}

fn status_json(cookie_present: bool, v: Value) -> Value {
    let status = v
        .get("status")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("cookie_status").and_then(|x| x.as_str()))
        .unwrap_or(if cookie_present { "unknown" } else { "missing" })
        .to_string();
    let logged_in = v
        .get("loggedIn")
        .and_then(|x| x.as_bool())
        .or_else(|| v.get("ok").and_then(|x| x.as_bool()))
        .unwrap_or(false);
    let uname = v
        .get("uname")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let reason = v
        .get("reason")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    json!({
        "loggedIn": logged_in,
        "status": status,
        "uname": uname,
        "reason": reason,
        "cookiePresent": cookie_present,
    })
}

/// 把一次体检结果写进 `bili_cookie_status.json`（与弹幕 shim 同一份契约）。
///
/// 为什么登录/退出时也要写：MPV 起播的自动判定会读这个文件决定「B站 是否已登录」。
/// 如果只在 shim 启动时体检，用户刚登录完再起播会读到登录前的旧结论（missing/expired），
/// 被误判成未登录而直接走弹弹play。这里在保存/清除 Cookie 后立即刷新，保证
/// 「登录 B站 后恢复原优先级」不需要等下一次体检。
fn write_cookie_status(dir: &Path, payload: &Value) {
    let path = dir.join("bili_cookie_status.json");
    match serde_json::to_string_pretty(payload) {
        Ok(body) => {
            if let Err(e) = std::fs::write(&path, body) {
                crate::libmpv::trace(&format!("[bili-account] 写 Cookie 状态失败: {e}"));
            }
        }
        Err(e) => crate::libmpv::trace(&format!("[bili-account] 序列化 Cookie 状态失败: {e}")),
    }
}

/// 设置面板「B 站账号」状态：读 Cookie 文件 + 体检。
#[tauri::command]
pub async fn bili_account_status(state: State<'_, AppState>) -> Result<Value, String> {
    let Some(dir) = script_dir_async(&state).await else {
        return Ok(json!({
            "loggedIn": false,
            "status": "no_script_dir",
            "uname": Value::Null,
            "reason": "未找到 uosc_danmaku 脚本目录（请先在「播放器」里选择有效的 mpv 路径）",
            "cookiePresent": false,
        }));
    };
    let path = cookie_path(&dir);
    let cookie = read_cookie_file(&path);
    if cookie.is_none() {
        return Ok(status_json(false, json!({ "status": "missing" })));
    }
    // 优先复用 shim 的体检（其 cookie_check 会重读文件，nav 结果有 10 分钟缓存）
    if let Some(v) = shim_cookie_status().await {
        let out = status_json(true, v);
        if out.get("status").and_then(|x| x.as_str()) != Some("unknown") {
            return Ok(out);
        }
    }
    Ok(status_json(true, nav_check(cookie.as_deref().unwrap_or("")).await))
}

/// 保存用户粘贴的 Cookie（SESSDATA 或完整 Cookie 头），并立即返回体检结果。
#[tauri::command]
pub async fn bili_set_cookie(state: State<'_, AppState>, cookie: String) -> Result<Value, String> {
    let normalized = normalize_cookie(&cookie)?;
    let Some(dir) = script_dir_async(&state).await else {
        return Err("未找到 uosc_danmaku 脚本目录（请先在「播放器」里选择有效的 mpv 路径）".into());
    };
    write_cookie_file(&cookie_path(&dir), &normalized)?;
    // 写完后直接用新值体检（不依赖 shim 的旧缓存）
    let mut v = nav_check(&normalized).await;
    if v.get("status").and_then(|x| x.as_str()) == Some("error") {
        // 网络失败时再试一次 shim（它可能能连通）
        if let Some(s) = shim_cookie_status().await {
            v = s;
        }
    }
    let out = status_json(true, v);
    // [auto-dd] 登录后立即刷新状态文件：MPV 起播据此恢复「B站 优先」。
    write_cookie_status(&dir, &out);
    Ok(out)
}

/// 退出 B 站账号：删除 Cookie 文件。
#[tauri::command]
pub async fn bili_clear_cookie(state: State<'_, AppState>) -> Result<Value, String> {
    let Some(dir) = script_dir_async(&state).await else {
        return Ok(status_json(false, json!({ "status": "missing" })));
    };
    let path = cookie_path(&dir);
    match std::fs::remove_file(&path) {
        Ok(_) => {
            let out = status_json(false, json!({ "status": "missing" }));
            // [auto-dd] 退出后立即把状态写成 missing：避免旧「valid」让起播判定误以为仍已登录。
            write_cookie_status(&dir, &out);
            Ok(out)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let out = status_json(false, json!({ "status": "missing" }));
            write_cookie_status(&dir, &out);
            Ok(out)
        }
        Err(e) => Err(format!("删除 Cookie 失败: {e}")),
    }
}

// ───────────────────────── Node 检测 / 一键自建源 ─────────────────────────

/// 把 `node`（PATH 里的相对名）解析成绝对路径用于展示；解析失败时原样返回。
pub(crate) fn display_node_path(p: &Path) -> String {
    if p.components().count() > 1 {
        return p.display().to_string();
    }
    let (cmd, arg) = if cfg!(windows) {
        ("where", "node")
    } else {
        ("which", "node")
    };
    if let Ok(out) = Command::new(cmd)
        .arg(arg)
        .stdin(Stdio::null())
        .output()
    {
        if out.status.success() {
            if let Some(line) = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim())
                .find(|l| !l.is_empty())
            {
                return line.to_string();
            }
        }
    }
    p.display().to_string()
}

pub(crate) fn node_version(p: &Path) -> String {
    Command::new(p)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// 检测本机 Node：`{ found, version, path }`。不下载、不安装。
#[tauri::command]
pub async fn detect_node(app: AppHandle) -> Result<Value, String> {
    match crate::danmaku::builtin::resolve_node(&app) {
        Ok(p) => {
            let version = node_version(&p);
            if version.is_empty() {
                return Ok(json!({
                    "found": false,
                    "version": "",
                    "path": display_node_path(&p),
                    "error": "找到了 node 但无法执行 --version",
                }));
            }
            Ok(json!({
                "found": true,
                "version": version,
                "path": display_node_path(&p),
            }))
        }
        Err(e) => Ok(json!({
            "found": false,
            "version": "",
            "path": "",
            "error": e,
        })),
    }
}

fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// 端口开着时确认是不是我们的 shim（GET /health）。
async fn shim_health_ok() -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    else {
        return false;
    };
    match client
        .get(format!("http://127.0.0.1:{SHIM_PORT}/health"))
        .send()
        .await
    {
        Ok(r) => r
            .json::<Value>()
            .await
            .map(|v| v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false))
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// 一键免配置自建弹幕源：
///   1. 确认本机 Node 可用；
///   2. 把「自建弹幕接口」写成启用（地址留空 = 内置源；已有自定义地址则尊重）；
///   3. 启动/复用内置 danmu_api（127.0.0.1:22352）；
///   4. 启动/复用弹幕 shim（127.0.0.1:22347）。
///
/// 幂等：每一步都是「已就绪即复用」，重复点击不会产生多个 node 进程。
/// 端口 22347 被非本服务占用时明确报冲突，而不是盲目 kill。
#[tauri::command]
pub async fn setup_builtin_danmaku(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let node = crate::danmaku::builtin::resolve_node(&app)
        .map_err(|e| format!("未检测到可用的 Node.js：{e}"))?;
    let version = node_version(&node);
    if version.is_empty() {
        return Err("Node.js 存在但无法执行 --version，请检查安装".into());
    }

    // 先做端口预检：22347 开着但不是我们的 shim → 冲突，直接失败（不杀别人的进程）
    let shim_state = if port_open(SHIM_PORT) {
        if shim_health_ok().await {
            "already-running"
        } else {
            return Err(format!(
                "端口 {SHIM_PORT} 已被其他程序占用（不是 AniBox 弹幕服务），请先关闭占用程序后重试"
            ));
        }
    } else {
        "stopped"
    };

    let mut cfg = state.config.read().await.clone();
    cfg.danmu_api_enabled = true;
    // 地址留空才使用内置源；用户已填自定义地址时保留，避免一键按钮覆盖既有部署。
    config::save(&app, &cfg).await?;
    *state.config.write().await = cfg.clone();

    danmaku_shim::write_danmu_config(&cfg)?;
    danmaku_shim::sync_lua_gate(
        &cfg.mpv_path,
        cfg.danmu_api_enabled,
        &crate::danmaku::effective_base(&cfg),
    );

    let using_builtin = crate::danmaku::uses_builtin(&cfg);
    if using_builtin {
        crate::danmaku::builtin::ensure(&app, &state.builtin_child)
            .await
            .map_err(|e| format!("内置弹幕源启动失败：{e}"))?;
    }

    let engine = crate::danmaku::resolve_engine(&cfg).to_string();
    danmaku_shim::ensure(&cfg.mpv_path, &state.danmaku_child, &engine)
        .await
        .map_err(|e| format!("弹幕服务启动失败：{e}"))?;

    Ok(json!({
        "ok": true,
        "nodeVersion": version,
        "nodePath": display_node_path(&node),
        "usingBuiltin": using_builtin,
        "base": crate::danmaku::effective_base(&cfg),
        "engine": engine,
        "shim": shim_state,
        "builtin": if using_builtin { "ready" } else { "skipped-custom-base" },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_sessdata_only() {
        let c = normalize_cookie("  SESSDATA=abc%2C123  ").unwrap();
        assert_eq!(c, "SESSDATA=abc%2C123");
    }

    #[test]
    fn normalize_strips_cookie_prefix_and_newlines() {
        let c = normalize_cookie("Cookie: SESSDATA=a;\n bili_jct=b\r\n").unwrap();
        assert_eq!(c, "SESSDATA=a; bili_jct=b");
    }

    #[test]
    fn normalize_accepts_bare_sessdata_value() {
        let c = normalize_cookie("abc%2C123%2Cdef").unwrap();
        assert_eq!(c, "SESSDATA=abc%2C123%2Cdef");
    }

    #[test]
    fn normalize_rejects_missing_sessdata() {
        assert!(normalize_cookie("bili_jct=b").is_err());
        assert!(normalize_cookie("   ").is_err());
    }

    #[test]
    fn cookie_path_lives_in_script_dir() {
        let p = cookie_path(Path::new("/x/uosc_danmaku"));
        assert_eq!(p, Path::new("/x/uosc_danmaku/bili_cookie.txt"));
    }
}
