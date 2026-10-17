//! 更新模块：全量更新（自建下载器 + NSIS）+ 前端资源热补丁覆盖层。
//!
//! 设计要点（对齐旧项目 Fntv-Plus 的 updateChecker / mirrorInstaller / patchApplier）：
//!   · 更新清单走 Gitee 公开 raw（`update-check.json`），可用 `FNTV_UPDATE_MANIFEST` 覆盖；
//!   · 全量包走自建 GitHub 镜像 `/app/resolve` 解析出文件，再流式下载（Range 断点续传），
//!     镜像可用 `FNTV_GH_MIRROR` 覆盖；解析失败回退清单里的 `downloadUrl`；
//!   · 安装包下载完不跳 GitHub，直接拉起 NSIS 安装器（GUI 安装，避免静默安装的 UAC 坑）；
//!   · 热补丁只覆盖**前端资源**：落盘到 `app_data_dir/web/<version>/`，由自定义协议
//!     `fntvweb` 优先读取；Rust 侧改动仍需全量更新。
//!
//! 自定义协议：Windows 形态 `http://fntvweb.localhost/<path>`，mac/linux `fntvweb://localhost/<path>`。
//! 只有存在已应用补丁（`web/active.json`）时才在启动时切到该协议；未打补丁时继续用 Tauri 内置
//! `tauri://` 资源协议（行为与改造前完全一致，降低风险）。补丁目录缺失时自动回退内置资源。
//!
//! 已知限制：
//!   · dev（`debug_assertions`）下前端由 Vite 提供，覆盖层不生效（补丁可下载/落盘/回滚，但不会
//!     覆盖 Vite 页面）；release 才切换自定义协议。
//!   · 补丁只能覆盖前端静态资源；Rust/插件/原生依赖的改动必须发全量包。
//!   · 自定义协议下不处理 Range 请求（前端资源都是小文件，WebView 不会对大 JS/CSS 发 Range）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};
use tauri::http::{Request, Response};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

use crate::libmpv::trace;

/// 默认更新清单（Gitee 公开 raw，随发版 push 到 release 分支）。
pub const DEFAULT_MANIFEST_URL: &str =
    "https://gitee.com/YDMY007/fntv-plus/raw/release/resource/wiki/update-check.json";
/// 自建 GitHub 镜像：主址 + workers.dev 兜底。
pub const DEFAULT_MIRRORS: [&str; 2] = [
    "https://gh.690075.xyz",
    "https://fntv-gh-mirror.122983191.workers.dev",
];
/// 占位：项目地址（等 AniBox 仓库定名后替换）。
pub const REPO_URL_PLACEHOLDER: &str = "https://github.com/YDMY007/Fntv-Plus";
/// 占位：软件说明（前端「关于」页展示）。
pub const APP_DESCRIPTION_PLACEHOLDER: &str = "AniBox —— 飞牛影视（fnOS）桌面客户端";

/// 自定义协议名（不要随意改：改了 = 旧补丁目录失效）。
#[allow(dead_code)]
pub const WEB_SCHEME: &str = "fntvweb";

const PROGRESS_EVENT_UPDATE: &str = "update-progress";
const PROGRESS_EVENT_PATCH: &str = "patch-progress";

const MAX_PATCH_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_PATCH_FILE_BYTES: usize = 32 * 1024 * 1024;

// ───────────────────────── 取消标志 / 覆盖层状态 ─────────────────────────

static CANCEL: AtomicBool = AtomicBool::new(false);

fn web_active() -> &'static RwLock<Option<String>> {
    static A: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    A.get_or_init(|| RwLock::new(None))
}

fn original_window_url() -> &'static RwLock<Option<String>> {
    static U: OnceLock<RwLock<Option<String>>> = OnceLock::new();
    U.get_or_init(|| RwLock::new(None))
}

// ───────────────────────── 前端契约结构 ─────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub repo_url: String,
    pub description: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub has_update: bool,
    pub current_version: String,
    pub latest_version: String,
    /// "hotfix" | "full"
    pub update_type: String,
    pub notes: String,
    pub published_at: String,
    pub download_url: Option<String>,
    pub patch_url: Option<String>,
    pub html_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResult {
    pub path: String,
    pub size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchCheck {
    pub has_patch: bool,
    pub version: String,
    pub notes: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchApplyResult {
    pub applied: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    phase: String,
    loaded: u64,
    total: u64,
    percent: u32,
    message: String,
}

fn emit_progress(
    app: &AppHandle,
    event: &str,
    phase: &str,
    loaded: u64,
    total: u64,
    message: impl Into<String>,
) {
    let percent = if total > 0 {
        ((loaded.saturating_mul(100)) / total).min(100) as u32
    } else {
        0
    };
    let _ = app.emit(
        event,
        Progress {
            phase: phase.to_string(),
            loaded,
            total,
            percent,
            message: message.into(),
        },
    );
}

// ───────────────────────── 清单结构 ─────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    version: String,
    #[serde(default, alias = "tag_name", alias = "tagName")]
    tag_name: String,
    #[serde(default, alias = "publishedAt")]
    date: String,
    #[serde(default, alias = "changelog")]
    notes: String,
    #[serde(default, rename = "downloadUrl", alias = "download_url")]
    download_url: String,
    #[serde(default, rename = "patchUrl", alias = "patch_url")]
    patch_url: String,
    #[serde(default, rename = "htmlUrl", alias = "html_url")]
    html_url: String,
}

impl Manifest {
    fn raw_version(&self) -> String {
        if !self.version.trim().is_empty() {
            self.version.trim().to_string()
        } else {
            self.tag_name.trim().to_string()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PatchManifest {
    #[serde(default)]
    version: String,
    #[serde(default, rename = "minAppVersion")]
    min_app_version: String,
    #[serde(default)]
    files: Vec<PatchFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct PatchFile {
    #[serde(default)]
    target: String,
    #[serde(default)]
    b64: String,
    #[serde(default)]
    sha256: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ActivePointer {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    previous: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResolveResp {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    file: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    url: String,
    #[serde(default)]
    error: Option<String>,
}

struct Asset {
    file: String,
    size: u64,
    url: String,
}

// ───────────────────────── 纯逻辑（可单测） ─────────────────────────

/// 剥掉历史类型后缀 `-hotfix` / `-full` / `-test`（允许带序号），并去掉前导 v。
pub fn strip_suffix(v: &str) -> String {
    let s = v.trim();
    let s = s
        .strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s);
    if let Some(i) = s.rfind('-') {
        let suffix = s[i + 1..].to_ascii_lowercase();
        if suffix.starts_with("hotfix") || suffix.starts_with("full") || suffix.starts_with("test") {
            return s[..i].to_string();
        }
    }
    s.to_string()
}

/// `-test` 版本绝不向用户推送。
pub fn is_test_version(v: &str) -> bool {
    let s = v.trim();
    let s = s
        .strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s);
    match s.rfind('-') {
        Some(i) => s[i + 1..].to_ascii_lowercase().starts_with("test"),
        None => false,
    }
}

/// 纯数字版本（缺失位按 0 处理）。
pub fn parse_version(v: &str) -> Vec<u64> {
    strip_suffix(v)
        .split('.')
        .map(|p| {
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let a = parse_version(a);
    let b = parse_version(b);
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = *a.get(i).unwrap_or(&0);
        let y = *b.get(i).unwrap_or(&0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

fn num_at(v: &[u64], i: usize) -> u64 {
    *v.get(i).unwrap_or(&0)
}

/// 判定更新类型：patch 位 +1 = hotfix；minor/major +1 = full；不更新/测试版 = None。
pub fn classify_update(current: &str, latest: &str) -> Option<&'static str> {
    if is_test_version(latest) {
        return None;
    }
    let l = parse_version(latest);
    let c = parse_version(current);
    if compare_versions(latest, current) != std::cmp::Ordering::Greater {
        return None;
    }
    let full = num_at(&l, 0) > num_at(&c, 0)
        || (num_at(&l, 0) == num_at(&c, 0) && num_at(&l, 1) > num_at(&c, 1));
    Some(if full { "full" } else { "hotfix" })
}

/// 补丁是否适用于当前应用版本（版本更新 + 最低版本满足 + 至少一个文件）。
fn patch_applicable(current: &str, patch: &PatchManifest) -> bool {
    if patch.files.is_empty() || patch.version.trim().is_empty() {
        return false;
    }
    if compare_versions(&patch.version, current) != std::cmp::Ordering::Greater {
        return false;
    }
    if !patch.min_app_version.trim().is_empty()
        && compare_versions(current, &patch.min_app_version) == std::cmp::Ordering::Less
    {
        return false;
    }
    true
}

/// 只允许前端资源的相对路径；拒绝绝对路径、盘符、`..`、NUL。
pub fn sanitize_rel_path(target: &str) -> Result<String, String> {
    if target.trim().is_empty() {
        return Err("空路径".into());
    }
    if target.contains('\0') {
        return Err("路径包含 NUL".into());
    }
    let t = target.trim().replace('\\', "/");
    if t.starts_with('/') {
        return Err(format!("拒绝绝对路径: {target}"));
    }
    let bytes = t.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(format!("拒绝盘符路径: {target}"));
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in t.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return Err(format!("拒绝路径穿越: {target}")),
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        return Err(format!("空路径: {target}"));
    }
    Ok(parts.join("/"))
}

/// 安装包文件名只允许单段安全字符（避免 `../` 与路径分隔符）。
fn sanitize_file_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("空文件名".into());
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains('\0') {
        return Err(format!("非法文件名: {name}"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
    {
        return Err(format!("文件名含非法字符: {name}"));
    }
    Ok(name.to_string())
}

fn normalize_display_version(v: &str) -> String {
    let s = v.trim();
    s.strip_prefix('v')
        .or_else(|| s.strip_prefix('V'))
        .unwrap_or(s)
        .to_string()
}

fn version_dir_name(v: &str) -> String {
    normalize_display_version(v)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum ResumeAction {
    /// 残片不存在或不可信：从头写。
    Restart,
    /// 从 `n` 字节续写。
    Append(u64),
    /// 残片长度已等于目标大小：直接收尾。
    Complete,
}

fn resume_action(part_len: Option<u64>, total: u64) -> ResumeAction {
    match part_len {
        Some(n) if n > 0 => {
            if total > 0 && n == total {
                ResumeAction::Complete
            } else if total > 0 && n < total {
                ResumeAction::Append(n)
            } else {
                // n > total，或总大小未知 → 残片不可信
                ResumeAction::Restart
            }
        }
        _ => ResumeAction::Restart,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn decode_b64(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(&clean)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&clean))
        .map_err(|e| format!("base64 解码失败: {e}"))
}

fn url_file_name(url: &str) -> Option<String> {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    let name = no_query.rsplit('/').next()?.trim();
    if name.is_empty() {
        None
    } else {
        let decoded = urlencoding::decode(name)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| name.to_string());
        Some(decoded)
    }
}

fn default_installer_name(version: &str) -> String {
    let v = normalize_display_version(version);
    let safe: String = v
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("AniBox_{safe}_win_x64.exe")
}

// ───────────────────────── HTTP 客户端 ─────────────────────────

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(format!("AniBox/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        // 大文件用“读超时”（空闲 60s 才断），不要用总时长超时
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn manifest_url() -> String {
    std::env::var("FNTV_UPDATE_MANIFEST")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_MANIFEST_URL.to_string())
}

fn mirror_hosts() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Ok(h) = std::env::var("FNTV_GH_MIRROR") {
        let h = h.trim().trim_end_matches('/').to_string();
        if !h.is_empty() {
            out.push(h);
        }
    }
    for d in DEFAULT_MIRRORS {
        if !out.iter().any(|x| x == d) {
            out.push(d.to_string());
        }
    }
    out
}

async fn fetch_manifest() -> Result<Manifest, String> {
    let url = manifest_url();
    let resp = client()
        .get(&url)
        .header("Cache-Control", "no-cache")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("更新清单请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("更新清单 HTTP {}", resp.status()));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取更新清单失败: {e}"))?;
    serde_json::from_str::<Manifest>(&text).map_err(|e| format!("更新清单解析失败: {e}"))
}

async fn resolve_asset(host: &str, version: &str) -> Result<Asset, String> {
    let v = normalize_display_version(version);
    let tag = format!("v{v}");
    let url = format!(
        "{host}/app/resolve?os=windows&arch=x64&tag={}",
        urlencoding::encode(&tag)
    );
    let resp = client()
        .get(&url)
        .timeout(std::time::Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| format!("{host} 解析失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{host} 解析 HTTP {}", resp.status()));
    }
    let d: ResolveResp = resp
        .json()
        .await
        .map_err(|e| format!("{host} 解析响应非法: {e}"))?;
    if !d.ok || d.url.trim().is_empty() || d.file.trim().is_empty() {
        return Err(format!(
            "{host} 解析失败: {}",
            d.error.unwrap_or_else(|| "缺少 url/file".into())
        ));
    }
    Ok(Asset {
        file: d.file,
        size: d.size,
        url: d.url,
    })
}

async fn fetch_patch(url: &str) -> Result<PatchManifest, String> {
    let resp = client()
        .get(url)
        .header("Cache-Control", "no-cache")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("补丁清单请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("补丁清单 HTTP {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取补丁清单失败: {e}"))?;
    if bytes.len() > MAX_PATCH_JSON_BYTES {
        return Err("补丁清单过大".into());
    }
    serde_json::from_slice::<PatchManifest>(&bytes).map_err(|e| format!("补丁清单解析失败: {e}"))
}

// ───────────────────────── 目录 / 指针 ─────────────────────────

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {e}"))
}

fn web_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("web"))
}

fn active_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(web_root(app)?.join("active.json"))
}

fn read_active_blocking(app: &AppHandle) -> ActivePointer {
    let Ok(p) = active_path(app) else {
        return ActivePointer::default();
    };
    let Ok(bytes) = std::fs::read(&p) else {
        return ActivePointer::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

async fn read_active(app: &AppHandle) -> ActivePointer {
    let Ok(p) = active_path(app) else {
        return ActivePointer::default();
    };
    let Ok(bytes) = tokio::fs::read(&p).await else {
        return ActivePointer::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

async fn write_active(app: &AppHandle, ptr: &ActivePointer) -> Result<(), String> {
    let p = active_path(app)?;
    if let Some(parent) = p.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建补丁目录失败: {e}"))?;
    }
    let s = serde_json::to_string_pretty(ptr).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.tmp");
    tokio::fs::write(&tmp, s)
        .await
        .map_err(|e| format!("写补丁指针失败: {e}"))?;
    tokio::fs::rename(&tmp, &p)
        .await
        .map_err(|e| format!("替换补丁指针失败: {e}"))
}

/// 启动时载入覆盖层指针；release 且存在补丁时切到自定义协议。
pub fn init(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if let Ok(u) = win.url() {
            *original_window_url().write().unwrap() = Some(u.to_string());
        }
    }
    let ptr = read_active_blocking(app);
    let active = ptr.version.as_deref().and_then(|v| {
        let dir_name = version_dir_name(v);
        let root = app_data_dir(app).ok()?.join("web").join(&dir_name);
        if root.is_dir() {
            Some(dir_name)
        } else {
            None
        }
    });
    if let Some(v) = &active {
        trace(&format!("[update] 已应用前端补丁覆盖层: web/{v}"));
    }
    *web_active().write().unwrap() = active.clone();

    #[cfg(not(debug_assertions))]
    if active.is_some() {
        // 延迟一点点，避开 WebView 首次加载与 navigate 的竞态（窗口此时仍是隐藏的）
        let h = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if let Some(win) = h.get_webview_window("main") {
                match win.navigate(web_entry_url()) {
                    Ok(_) => trace("[update] 已切换到 fntvweb 覆盖层协议"),
                    Err(e) => trace(&format!("[update] 切换覆盖层协议失败（回退内置资源）: {e}")),
                }
            }
        });
    }
}

// ───────────────────────── 自定义协议：前端资源 ─────────────────────────

#[allow(dead_code)]
fn web_entry_url() -> tauri::Url {
    #[cfg(target_os = "windows")]
    {
        tauri::Url::parse("http://fntvweb.localhost/index.html").expect("fntvweb url")
    }
    #[cfg(not(target_os = "windows"))]
    {
        tauri::Url::parse("fntvweb://localhost/index.html").expect("fntvweb url")
    }
}

fn mime_for(path: &str) -> String {
    let ext = path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let m = match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    m.to_string()
}

fn bytes_response(bytes: Vec<u8>, mime: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(200)
        .header("Content-Type", mime)
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "no-cache")
        .body(bytes)
        .unwrap_or_else(|_| plain(500, "response build failed"))
}

fn plain(status: u16, msg: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(msg.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

async fn read_overlay(app: &AppHandle, rel: &str) -> Option<Vec<u8>> {
    let version = web_active().read().ok()?.clone()?;
    let root = web_root(app).ok()?.join(version);
    let file = root.join(rel);
    // 二次防护：确保解析后的路径仍在覆盖目录内
    let canon_root = tokio::fs::canonicalize(&root).await.ok()?;
    let canon_file = tokio::fs::canonicalize(&file).await.ok()?;
    if !canon_file.starts_with(&canon_root) {
        return None;
    }
    tokio::fs::read(canon_file).await.ok()
}

/// 自定义协议处理：覆盖层优先 → 内置资源 → SPA 兜底。
pub async fn handle_web_asset(app: AppHandle, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let raw_path = request.uri().path();
    let decoded = urlencoding::decode(raw_path)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| raw_path.to_string());
    let trimmed = decoded.trim_start_matches('/');
    let rel = if trimmed.is_empty() {
        "index.html".to_string()
    } else {
        match sanitize_rel_path(trimmed) {
            Ok(p) => p,
            Err(_) => return plain(400, "bad path"),
        }
    };

    // 1) 补丁覆盖层
    if let Some(bytes) = read_overlay(&app, &rel).await {
        return bytes_response(bytes, &mime_for(&rel));
    }

    // 2) 内嵌前端资源
    if let Some(asset) = app.asset_resolver().get(rel.clone()) {
        return bytes_response(asset.bytes, &asset.mime_type);
    }

    // 3) SPA 兜底：无扩展名的路由一律回 index.html
    let last = rel.rsplit('/').next().unwrap_or("");
    if !last.contains('.') {
        if let Some(bytes) = read_overlay(&app, "index.html").await {
            return bytes_response(bytes, &mime_for("index.html"));
        }
        if let Some(asset) = app.asset_resolver().get("index.html".to_string()) {
            return bytes_response(asset.bytes, &asset.mime_type);
        }
    }

    plain(404, "not found")
}

// ───────────────────────── 命令：应用信息 ─────────────────────────

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    let pkg = app.package_info();
    let desc = if pkg.description.trim().is_empty() {
        APP_DESCRIPTION_PLACEHOLDER.to_string()
    } else {
        pkg.description.to_string()
    };
    AppInfo {
        name: pkg.name.clone(),
        version: pkg.version.to_string(),
        repo_url: REPO_URL_PLACEHOLDER.to_string(),
        description: desc,
    }
}

// ───────────────────────── 命令：更新检查 ─────────────────────────

#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    let m = match fetch_manifest().await {
        Ok(m) => m,
        Err(e) => {
            // 与旧项目一致：清单不可达时静默视为“无更新”，不打扰用户
            trace(&format!("[update] 检查失败（视为无更新）: {e}"));
            return Ok(UpdateInfo {
                has_update: false,
                current_version: current,
                latest_version: String::new(),
                update_type: "full".into(),
                notes: String::new(),
                published_at: String::new(),
                download_url: None,
                patch_url: None,
                html_url: REPO_URL_PLACEHOLDER.to_string(),
            });
        }
    };

    let latest_raw = m.raw_version();
    let latest = normalize_display_version(&latest_raw);
    let kind = classify_update(&current, &latest_raw);
    let has_update = kind.is_some();
    let update_type = kind.unwrap_or("full").to_string();
    trace(&format!(
        "[update] 当前 {current} / 最新 {latest} / 类型 {update_type} / hasUpdate={has_update}"
    ));

    let download_url = if m.download_url.trim().is_empty() {
        None
    } else {
        Some(m.download_url.trim().to_string())
    };
    let patch_url = if m.patch_url.trim().is_empty() {
        None
    } else {
        Some(m.patch_url.trim().to_string())
    };
    let html_url = if !m.html_url.trim().is_empty() {
        m.html_url.trim().to_string()
    } else if let Some(d) = &download_url {
        d.clone()
    } else {
        REPO_URL_PLACEHOLDER.to_string()
    };

    Ok(UpdateInfo {
        has_update,
        current_version: current,
        latest_version: latest,
        update_type,
        notes: m.notes,
        published_at: m.date,
        download_url,
        patch_url,
        html_url,
    })
}

// ───────────────────────── 命令：全量下载 ─────────────────────────

#[tauri::command]
pub async fn update_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub async fn update_download(app: AppHandle, version: String) -> Result<DownloadResult, String> {
    CANCEL.store(false, Ordering::SeqCst);
    emit_progress(&app, PROGRESS_EVENT_UPDATE, "checking", 0, 0, "正在解析安装包…");

    let res = download_inner(&app, &version).await;
    if let Err(e) = &res {
        emit_progress(&app, PROGRESS_EVENT_UPDATE, "error", 0, 0, e.clone());
        trace(&format!("[update] 下载失败: {e}"));
    }
    res
}

async fn download_inner(app: &AppHandle, version: &str) -> Result<DownloadResult, String> {
    // 1) 镜像解析（逐个 host 试）
    let mut asset: Option<Asset> = None;
    for host in mirror_hosts() {
        match resolve_asset(&host, version).await {
            Ok(a) => {
                trace(&format!(
                    "[update] 镜像解析成功: {} {}B ← {host}",
                    a.file, a.size
                ));
                asset = Some(a);
                break;
            }
            Err(e) => trace(&format!("[update] {e}")),
        }
    }

    // 2) 解析失败 → 清单 downloadUrl 兜底
    let (url, file, size) = match asset {
        Some(a) => (a.url, a.file, a.size),
        None => {
            let m = fetch_manifest()
                .await
                .map_err(|e| format!("镜像解析失败且清单不可用: {e}"))?;
            let dl = m.download_url.trim().to_string();
            if dl.is_empty() {
                return Err("镜像解析失败，清单也未提供 downloadUrl".into());
            }
            let file = url_file_name(&dl).unwrap_or_else(|| default_installer_name(version));
            trace(&format!("[update] 回退清单 downloadUrl: {dl}"));
            (dl, file, 0)
        }
    };

    let file = sanitize_file_name(&file)?;
    let dir = app_data_dir(app)?.join("updates");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("创建下载目录失败: {e}"))?;
    let part = dir.join(format!("{file}.part"));
    let final_path = dir.join(&file);

    // 3) 断点续传判定
    let existing = tokio::fs::metadata(&part).await.ok().map(|m| m.len());
    let mut start = match resume_action(existing, size) {
        ResumeAction::Complete => {
            let _ = tokio::fs::remove_file(&final_path).await;
            tokio::fs::rename(&part, &final_path)
                .await
                .map_err(|e| format!("保存安装包失败: {e}"))?;
            emit_progress(
                app,
                PROGRESS_EVENT_UPDATE,
                "done",
                size,
                size,
                "安装包已就绪",
            );
            return Ok(DownloadResult {
                path: final_path.to_string_lossy().to_string(),
                size,
            });
        }
        ResumeAction::Append(n) => n,
        ResumeAction::Restart => {
            let _ = tokio::fs::remove_file(&part).await;
            0
        }
    };

    emit_progress(
        app,
        PROGRESS_EVENT_UPDATE,
        "downloading",
        start,
        size,
        if start > 0 {
            format!("继续下载（已下 {start} 字节）")
        } else {
            "开始下载".into()
        },
    );

    // 4) 发起请求（带 Range 续传）
    let mut req = client().get(&url);
    if start > 0 {
        req = req.header("Range", format!("bytes={start}-"));
    }
    let mut resp = req
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {e}"))?;

    let status = resp.status();
    let mut append = false;
    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        append = start > 0;
        if !append {
            start = 0;
        }
    } else if status == reqwest::StatusCode::OK {
        // 服务端不支持续传 → 从头写
        if start > 0 {
            let _ = tokio::fs::remove_file(&part).await;
            start = 0;
        }
    } else {
        return Err(format!("下载失败: HTTP {status}"));
    }

    let total = if size > 0 {
        size
    } else {
        start + resp.content_length().unwrap_or(0)
    };

    let mut f = if append {
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&part)
            .await
    } else {
        tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&part)
            .await
    }
    .map_err(|e| format!("打开临时文件失败: {e}"))?;

    let mut loaded = start;
    let mut last_emit = tokio::time::Instant::now();
    loop {
        if CANCEL.load(Ordering::SeqCst) {
            let _ = f.flush().await;
            return Err("已取消（断点已保留，可重试续传）".into());
        }
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                let _ = f.flush().await;
                return Err(format!("下载中断（断点已保留）: {e}"));
            }
        };
        if chunk.is_empty() {
            continue;
        }
        f.write_all(&chunk)
            .await
            .map_err(|e| format!("写入安装包失败: {e}"))?;
        loaded += chunk.len() as u64;
        if last_emit.elapsed() >= std::time::Duration::from_millis(150) {
            emit_progress(
                app,
                PROGRESS_EVENT_UPDATE,
                "downloading",
                loaded,
                total,
                "下载中…",
            );
            last_emit = tokio::time::Instant::now();
        }
    }
    f.flush()
        .await
        .map_err(|e| format!("刷新安装包失败: {e}"))?;
    drop(f);

    // 5) 长度校验
    let got = tokio::fs::metadata(&part)
        .await
        .map_err(|e| format!("读取临时文件失败: {e}"))?
        .len();
    if size > 0 && got != size {
        return Err(format!(
            "下载不完整（{got}/{size} 字节），已保留断点，可重试续传"
        ));
    }

    // 6) 原子收尾
    let _ = tokio::fs::remove_file(&final_path).await;
    tokio::fs::rename(&part, &final_path)
        .await
        .map_err(|e| format!("保存安装包失败: {e}"))?;

    emit_progress(
        app,
        PROGRESS_EVENT_UPDATE,
        "done",
        got,
        got,
        "安装包已就绪",
    );
    trace(&format!(
        "[update] 安装包已就绪: {}（{got} 字节）",
        final_path.display()
    ));
    Ok(DownloadResult {
        path: final_path.to_string_lossy().to_string(),
        size: got,
    })
}

// ───────────────────────── 命令：安装 ─────────────────────────

#[tauri::command]
pub async fn update_install(app: AppHandle, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("安装包不存在: {path}"));
    }

    // 安装前清掉补丁覆盖层：否则旧补丁会影子覆盖新安装包的前端资源
    clear_web_overlay(&app).await;

    // 尽量停掉子进程（失败不阻塞安装）
    stop_children(&app).await;

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(&p)
            .spawn()
            .map_err(|e| format!("启动安装器失败: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&p)
            .spawn()
            .map_err(|e| format!("打开安装包失败: {e}"))?;
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        std::process::Command::new(&p)
            .spawn()
            .map_err(|e| format!("启动安装器失败: {e}"))?;
    }

    trace(&format!("[update] 已拉起安装器: {path}，应用即将退出"));
    // 给安装器一点启动时间再退出
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    app.exit(0);
    Ok(())
}

async fn stop_children(app: &AppHandle) {
    use crate::AppState;
    let st = app.state::<AppState>();
    if let Ok(mut slot) = st.mpv.try_lock() {
        if let Some(m) = slot.as_ref() {
            let _ = m.shutdown().await;
        }
        *slot = None;
    }
    if let Ok(mut pc) = st.proxy_child.try_lock() {
        if let Some(c) = pc.as_mut() {
            let _ = c.kill();
        }
        *pc = None;
    }
    if let Ok(mut dc) = st.danmaku_child.try_lock() {
        if let Some(c) = dc.as_mut() {
            let _ = c.kill();
        }
        *dc = None;
    }
}

async fn clear_web_overlay(app: &AppHandle) {
    if let Ok(root) = web_root(app) {
        let _ = tokio::fs::remove_dir_all(&root).await;
    }
    *web_active().write().unwrap() = None;
}

// ───────────────────────── 命令：热补丁 ─────────────────────────

#[tauri::command]
pub async fn patch_check(app: AppHandle) -> Result<PatchCheck, String> {
    let current = app.package_info().version.to_string();
    let m = fetch_manifest().await?;
    let patch_url = m.patch_url.trim().to_string();
    if patch_url.is_empty() {
        return Ok(PatchCheck {
            has_patch: false,
            version: String::new(),
            notes: String::new(),
        });
    }
    let patch = match fetch_patch(&patch_url).await {
        Ok(p) => p,
        Err(e) => {
            trace(&format!("[patch] 检查失败（视为无补丁）: {e}"));
            return Ok(PatchCheck {
                has_patch: false,
                version: String::new(),
                notes: String::new(),
            });
        }
    };
    let applicable = patch_applicable(&current, &patch);
    Ok(PatchCheck {
        has_patch: applicable,
        version: normalize_display_version(&patch.version),
        notes: m.notes,
    })
}

#[tauri::command]
pub async fn patch_apply(app: AppHandle, version: String) -> Result<PatchApplyResult, String> {
    CANCEL.store(false, Ordering::SeqCst);
    let res = patch_apply_inner(&app, &version).await;
    match &res {
        Ok(_) => {}
        Err(e) => {
            emit_progress(&app, PROGRESS_EVENT_PATCH, "error", 0, 0, e.clone());
            trace(&format!("[patch] 应用失败: {e}"));
        }
    }
    res
}

async fn patch_apply_inner(app: &AppHandle, version: &str) -> Result<PatchApplyResult, String> {
    emit_progress(
        app,
        PROGRESS_EVENT_PATCH,
        "checking",
        0,
        0,
        "正在获取补丁清单…",
    );

    let current = app.package_info().version.to_string();
    let m = fetch_manifest().await?;
    let patch_url = m.patch_url.trim().to_string();
    if patch_url.is_empty() {
        return Err("更新清单未提供 patchUrl".into());
    }

    emit_progress(
        app,
        PROGRESS_EVENT_PATCH,
        "downloading",
        0,
        0,
        "正在下载补丁…",
    );
    let patch = fetch_patch(&patch_url).await?;

    let want = normalize_display_version(version);
    let got = normalize_display_version(&patch.version);
    if !want.is_empty() && want != got {
        return Err(format!("补丁版本不匹配（请求 {want}，清单 {got}）"));
    }
    if !patch_applicable(&current, &patch) {
        return Err(format!(
            "补丁不适用于当前版本 {current}（补丁 {}，最低 {})",
            patch.version, patch.min_app_version
        ));
    }

    emit_progress(
        app,
        PROGRESS_EVENT_PATCH,
        "applying",
        0,
        patch.files.len() as u64,
        "正在写入补丁文件…",
    );

    let root = web_root(app)?;
    let ver_dir = version_dir_name(&got);
    let staging = root.join(format!(".staging-{ver_dir}-{}", std::process::id()));
    if staging.exists() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
    }
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|e| format!("创建补丁暂存目录失败: {e}"))?;

    // 写入 + 校验（任一步失败：清暂存，原覆盖层保持不变）
    let apply_res = write_patch_files(&staging, &patch).await;
    if let Err(e) = apply_res {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }

    // 原子切换：旧版本目录删掉，暂存目录 rename 成正式目录
    let final_dir = root.join(&ver_dir);
    if final_dir.exists() {
        let _ = tokio::fs::remove_dir_all(&final_dir).await;
    }
    tokio::fs::rename(&staging, &final_dir)
        .await
        .map_err(|e| format!("启用补丁目录失败: {e}"))?;

    // 记录指针（previous 用于回滚）
    let prev = read_active(app).await;
    let previous = prev
        .version
        .as_deref()
        .map(version_dir_name)
        .filter(|v| v != &ver_dir && root.join(v).is_dir());
    write_active(
        app,
        &ActivePointer {
            version: Some(ver_dir.clone()),
            previous,
        },
    )
    .await?;
    *web_active().write().unwrap() = Some(ver_dir.clone());

    emit_progress(
        app,
        PROGRESS_EVENT_PATCH,
        "done",
        patch.files.len() as u64,
        patch.files.len() as u64,
        "补丁已应用",
    );
    trace(&format!(
        "[patch] 已应用补丁 {got}（{} 个文件）",
        patch.files.len()
    ));

    reload_or_navigate(app);
    Ok(PatchApplyResult { applied: true })
}

async fn write_patch_files(staging: &std::path::Path, patch: &PatchManifest) -> Result<(), String> {
    for f in patch.files.iter() {
        if CANCEL.load(Ordering::SeqCst) {
            return Err("已取消".into());
        }
        let rel = sanitize_rel_path(&f.target)?;
        let bytes = decode_b64(&f.b64)?;
        if bytes.len() > MAX_PATCH_FILE_BYTES {
            return Err(format!("补丁文件过大: {rel}"));
        }
        if f.sha256.trim().is_empty() {
            return Err(format!("补丁文件缺少 sha256，拒绝应用: {rel}"));
        }
        let actual = sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(f.sha256.trim()) {
            return Err(format!(
                "sha256 校验失败: {rel}（期望 {}，实际 {actual}）",
                f.sha256.trim()
            ));
        }
        let out = staging.join(&rel);
        if let Some(parent) = out.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("创建补丁子目录失败: {e}"))?;
        }
        tokio::fs::write(&out, &bytes)
            .await
            .map_err(|e| format!("写入补丁文件失败 {rel}: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn patch_rollback(app: AppHandle) -> Result<(), String> {
    let ptr = read_active(&app).await;
    let root = web_root(&app)?;

    if let Some(prev) = ptr.previous.as_deref().map(version_dir_name) {
        if root.join(&prev).is_dir() {
            write_active(
                &app,
                &ActivePointer {
                    version: Some(prev.clone()),
                    previous: None,
                },
            )
            .await?;
            *web_active().write().unwrap() = Some(prev.clone());
            trace(&format!("[patch] 已回滚到上一版补丁覆盖层: {prev}"));
            reload_or_navigate(&app);
            return Ok(());
        }
    }

    // 没有上一版 → 完全回退到内嵌前端
    let _ = tokio::fs::remove_file(active_path(&app)?).await;
    *web_active().write().unwrap() = None;
    trace("[patch] 已回滚到内嵌前端资源");

    #[cfg(not(debug_assertions))]
    if let Some(win) = app.get_webview_window("main") {
        let current_is_overlay = win
            .url()
            .map(|u| u.scheme() == WEB_SCHEME)
            .unwrap_or(false);
        if current_is_overlay {
            let target = original_window_url()
                .read()
                .ok()
                .and_then(|u| u.clone())
                .and_then(|u| tauri::Url::parse(&u).ok());
            if let Some(t) = target {
                let _ = win.navigate(t);
            }
        } else {
            let _ = win.reload();
        }
    }
    #[cfg(debug_assertions)]
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.reload();
    }
    Ok(())
}

fn reload_or_navigate(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    #[cfg(debug_assertions)]
    {
        // dev 下前端由 Vite 提供，覆盖层不生效；仅刷新
        trace("[patch] dev 模式：补丁已落盘，但覆盖层只在 release 生效");
        let _ = win.reload();
    }
    #[cfg(not(debug_assertions))]
    {
        let on_overlay = win
            .url()
            .map(|u| u.scheme() == WEB_SCHEME)
            .unwrap_or(false);
        if on_overlay {
            let _ = win.reload();
        } else if let Err(e) = win.navigate(web_entry_url()) {
            trace(&format!("[patch] 切换覆盖层协议失败: {e}"));
        }
    }
}

// ───────────────────────── 单测 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_suffix_handles_all_kinds() {
        assert_eq!(strip_suffix("v3.6.1-hotfix"), "3.6.1");
        assert_eq!(strip_suffix("3.6.1-full"), "3.6.1");
        assert_eq!(strip_suffix("3.6.1-test2"), "3.6.1");
        assert_eq!(strip_suffix("V3.6.1"), "3.6.1");
        assert_eq!(strip_suffix("3.6.1"), "3.6.1");
        // 非类型后缀不该被剥
        assert_eq!(strip_suffix("3.6.1-rc1"), "3.6.1-rc1");
    }

    #[test]
    fn test_version_detection() {
        assert!(is_test_version("3.6.1-test"));
        assert!(is_test_version("v3.6.1-test12"));
        assert!(!is_test_version("3.6.1-hotfix"));
        assert!(!is_test_version("3.6.1"));
    }

    #[test]
    fn version_compare_is_numeric() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("3.6.0", "3.6.1"), Ordering::Less);
        assert_eq!(compare_versions("3.10.0", "3.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("v3.6.1-hotfix", "3.6.1"), Ordering::Equal);
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn classify_hotfix_vs_full() {
        assert_eq!(classify_update("0.1.0", "0.1.1"), Some("hotfix"));
        assert_eq!(classify_update("0.1.0", "0.2.0"), Some("full"));
        assert_eq!(classify_update("0.1.0", "1.0.0"), Some("full"));
        assert_eq!(classify_update("0.1.0", "0.1.0"), None);
        assert_eq!(classify_update("0.1.0", "0.0.9"), None);
        assert_eq!(classify_update("0.1.0", "0.1.1-test"), None);
    }

    #[test]
    fn manifest_parses_gitee_shape() {
        let m: Manifest = serde_json::from_str(
            r#"{
                "version": "3.9.1-hotfix",
                "date": "2026-10-17",
                "notes": "修复若干问题",
                "downloadUrl": "https://gh.690075.xyz/app/v3.9.1/x.exe",
                "patchUrl": "https://example.com/patch-3.9.1.json"
            }"#,
        )
        .unwrap();
        assert_eq!(m.raw_version(), "3.9.1-hotfix");
        assert_eq!(m.date, "2026-10-17");
        assert_eq!(m.notes, "修复若干问题");
        assert!(m.download_url.starts_with("https://gh.690075.xyz"));
        assert!(m.patch_url.ends_with("patch-3.9.1.json"));
    }

    #[test]
    fn manifest_falls_back_to_tag_name() {
        let m: Manifest = serde_json::from_str(r#"{"tag_name":"v3.9.0"}"#).unwrap();
        assert_eq!(m.raw_version(), "v3.9.0");
    }

    #[test]
    fn path_traversal_is_rejected() {
        assert!(sanitize_rel_path("../secret.js").is_err());
        assert!(sanitize_rel_path("assets/../../x.js").is_err());
        assert!(sanitize_rel_path("/etc/passwd").is_err());
        assert!(sanitize_rel_path("C:\\Windows\\x.js").is_err());
        assert!(sanitize_rel_path("").is_err());
        assert!(sanitize_rel_path("./").is_err());
    }

    #[test]
    fn safe_relative_paths_normalize() {
        assert_eq!(sanitize_rel_path("index.html").unwrap(), "index.html");
        assert_eq!(sanitize_rel_path("./assets/app.js").unwrap(), "assets/app.js");
        assert_eq!(
            sanitize_rel_path("assets\\sub\\x.css").unwrap(),
            "assets/sub/x.css"
        );
        assert_eq!(sanitize_rel_path("a//b.js").unwrap(), "a/b.js");
    }

    #[test]
    fn file_name_is_single_segment() {
        assert!(sanitize_file_name("AniBox_0.1.1_win_x64.exe").is_ok());
        assert!(sanitize_file_name("../evil.exe").is_err());
        assert!(sanitize_file_name("a/b.exe").is_err());
        assert!(sanitize_file_name("a\\b.exe").is_err());
        assert!(sanitize_file_name("").is_err());
    }

    #[test]
    fn resume_offsets() {
        assert_eq!(resume_action(None, 100), ResumeAction::Restart);
        assert_eq!(resume_action(Some(0), 100), ResumeAction::Restart);
        assert_eq!(resume_action(Some(40), 100), ResumeAction::Append(40));
        assert_eq!(resume_action(Some(100), 100), ResumeAction::Complete);
        // 残片比目标大 → 不可信
        assert_eq!(resume_action(Some(120), 100), ResumeAction::Restart);
        // 总大小未知 → 从头
        assert_eq!(resume_action(Some(40), 0), ResumeAction::Restart);
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn base64_decode_roundtrip() {
        use base64::Engine;
        let raw = b"hello anibox";
        let enc = base64::engine::general_purpose::STANDARD.encode(raw);
        assert_eq!(decode_b64(&enc).unwrap(), raw);
        // 带换行/空白的 base64 也要能解
        let spaced = enc
            .as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(decode_b64(&spaced).unwrap(), raw);
    }

    #[test]
    fn patch_applicability_rules() {
        let mk = |ver: &str, min: &str, files: usize| PatchManifest {
            version: ver.into(),
            min_app_version: min.into(),
            files: (0..files)
                .map(|i| PatchFile {
                    target: format!("f{i}.js"),
                    b64: String::new(),
                    sha256: String::new(),
                })
                .collect(),
        };
        assert!(patch_applicable("0.1.0", &mk("0.1.1", "0.1.0", 1)));
        // 不比当前新
        assert!(!patch_applicable("0.1.1", &mk("0.1.1", "", 1)));
        // 低于最低版本
        assert!(!patch_applicable("0.0.9", &mk("0.1.1", "0.1.0", 1)));
        // 没有文件
        assert!(!patch_applicable("0.1.0", &mk("0.1.1", "", 0)));
    }

    #[test]
    fn version_dir_is_safe() {
        assert_eq!(version_dir_name("v0.1.1-hotfix"), "0.1.1-hotfix");
        assert_eq!(version_dir_name("../x"), ".._x");
        assert!(!version_dir_name("a/b").contains('/'));
    }

    #[test]
    fn url_file_name_extracts_basename() {
        assert_eq!(
            url_file_name("https://x/y/AniBox_0.1.1.exe?token=1").unwrap(),
            "AniBox_0.1.1.exe"
        );
        assert_eq!(url_file_name("https://x/y/"), None);
    }
}
