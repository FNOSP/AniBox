//! 更新模块：全量更新（自建下载器 + NSIS）+ 前端资源热补丁覆盖层。
//!
//! 设计要点（对齐旧项目 Fntv-Plus 的 updateChecker / mirrorInstaller / patchApplier）：
//!   · 更新清单按固定顺序依次尝试，**命中即停**：
//!       1) 自建 GitHub 镜像 `/version/{owner}/{repo}?branch=release`（含 workers.dev 兜底）；
//!       2) Gitee 公开 raw；
//!       3) 原生 GitHub raw（`release` 分支的 `resource/wiki/update-check.json`）。
//!     每源 5 秒超时，失败（网络 / 404 / JSON 解析失败 / 清单无版本号）才进入下一源；
//!     可用 `FNTV_UPDATE_MANIFEST` 覆盖为单一清单地址（本地调试）。
//!   · 全量包走自建 GitHub 镜像 `/app/resolve` 解析出文件，再流式下载（Range 断点续传），
//!     镜像可用 `FNTV_GH_MIRROR` 覆盖；解析失败回退清单里的 `downloadUrl`（同样按上面的源顺序）；
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

/// 默认更新清单（Gitee 公开 raw，随发版 push 到 release 分支）—— 三源里的第二源。
pub const DEFAULT_MANIFEST_URL: &str =
    "https://gitee.com/YDMY007/fntv-plus/raw/release/resource/wiki/update-check.json";
/// 原生 GitHub 清单（AniBox 仓库 release 分支，路径与仓库现状一致：`resource/wiki/update-check.json`）—— 第三源。
pub const NATIVE_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/FNOSP/AniBox/release/resource/wiki/update-check.json";
/// 自建 GitHub 镜像：主址 + workers.dev 兜底。
pub const DEFAULT_MIRRORS: [&str; 2] = [
    "https://gh.690075.xyz",
    "https://fntv-gh-mirror.122983191.workers.dev",
];
/// 镜像的版本检测路径（worker 的 `/version/{owner}/{repo}`，读 raw 清单并归一化；签名轨）。
/// 注意：path+query 必须与签名时完全一致，否则镜像返回 bad_signature。
const MIRROR_VERSION_PATH: &str = "/version/FNOSP/AniBox?branch=release";
/// 每个更新源的单次请求超时（启动检测不能卡住；三源最坏 15s）。
const MANIFEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// 项目地址（AniBox 正式仓库）。
pub const REPO_URL_PLACEHOLDER: &str = "https://github.com/FNOSP/AniBox";
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

/// 检测冷却（内存态，重启即重置）：
///   · `check`        —— 整次 update_check（启动自动检测也走这里）
///   · `probe:<id>`   —— 单源探测
/// 同一 key 在 30 秒内只真正请求一次，其余请求直接返回剩余冷却时间。
const CHECK_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);

fn cooldowns() -> &'static std::sync::Mutex<std::collections::HashMap<&'static str, std::time::Instant>> {
    static M: OnceLock<std::sync::Mutex<std::collections::HashMap<&'static str, std::time::Instant>>> =
        OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn cooldown_left(key: &'static str) -> u64 {
    let Ok(map) = cooldowns().lock() else {
        return 0;
    };
    match map.get(key) {
        Some(t) => {
            let elapsed = t.elapsed();
            if elapsed < CHECK_COOLDOWN {
                (CHECK_COOLDOWN - elapsed).as_secs() + 1
            } else {
                0
            }
        }
        None => 0,
    }
}

fn cooldown_mark(key: &'static str) {
    if let Ok(mut map) = cooldowns().lock() {
        map.insert(key, std::time::Instant::now());
    }
}

/// update_check 的 30 秒结果缓存（避免启动检测 / 用户连点重复打源）。
fn check_cache() -> &'static std::sync::Mutex<Option<(std::time::Instant, UpdateInfo)>> {
    static C: OnceLock<std::sync::Mutex<Option<(std::time::Instant, UpdateInfo)>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(None))
}

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

#[derive(Clone, Serialize)]
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
    /// 当前平台标识（win / mac / linux）——前端据此展示，避免把其它平台包误当本机安装包。
    #[serde(default)]
    pub platform: String,
    /// 当前架构标识（x64 / arm64）。
    #[serde(default)]
    pub arch: String,
    /// 本次结果实际命中的更新源（镜像 / Gitee / 原生 GitHub）。
    #[serde(default)]
    pub source: String,
    /// 冷却剩余秒数（命中缓存时 > 0）。
    #[serde(default)]
    pub cooldown_remaining: u64,
    /// 是否来自 30s 冷却缓存（未真正发起网络请求）。
    #[serde(default)]
    pub from_cache: bool,
}

/// `update_source_probe` 的单项结果。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeItem {
    pub id: String,
    pub label: String,
    pub url: String,
    pub ok: bool,
    /// HTTP 状态码（网络层失败时为 None）
    pub status: Option<u16>,
    pub elapsed_ms: u64,
    pub version: String,
    pub error: String,
    /// >0 表示本次因冷却被跳过（未发请求）
    pub cooldown_remaining: u64,
    pub skipped: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    pub items: Vec<ProbeItem>,
    /// 整个探测的冷却剩余秒数（>0 时前端禁用按钮）
    pub cooldown_remaining: u64,
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
    /// 瞬时速度（字节/秒）；非下载阶段为 0。
    speed: u64,
    message: String,
}

fn emit_progress(
    app: &AppHandle,
    event: &str,
    phase: &str,
    loaded: u64,
    total: u64,
    speed: u64,
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
            speed,
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

// ───────────────────────── 平台 / 架构映射 ─────────────────────────

/// 当前编译目标的平台标识（与镜像 worker `/app/resolve` 的 `os` 参数一致）。
pub fn target_os_slug() -> &'static str {
    os_slug_for(std::env::consts::OS)
}

/// 当前编译目标的架构标识（worker `arch` 参数：x64 / arm64）。
pub fn target_arch_slug() -> &'static str {
    arch_slug_for(std::env::consts::ARCH)
}

/// `std::env::consts::OS` → worker 的 `os` 参数（windows→win、macos→mac、linux→linux）。
pub fn os_slug_for(os: &str) -> &'static str {
    match os.trim().to_ascii_lowercase().as_str() {
        "windows" | "win" => "win",
        "macos" | "mac" | "darwin" | "osx" => "mac",
        _ => "linux",
    }
}

/// `std::env::consts::ARCH` → worker 的 `arch` 参数（x86_64→x64、aarch64→arm64）。
pub fn arch_slug_for(arch: &str) -> &'static str {
    match arch.trim().to_ascii_lowercase().as_str() {
        "aarch64" | "arm64" => "arm64",
        _ => "x64",
    }
}

/// 平台对应的安装包扩展名（小写，带点）。
pub fn platform_ext(os: &str) -> &'static str {
    match os {
        "win" => ".exe",
        "mac" => ".dmg",
        _ => ".appimage",
    }
}

/// 架构在文件名里可能出现的写法（别名）。
pub fn arch_aliases(arch: &str) -> &'static [&'static str] {
    match arch {
        "arm64" => &["arm64", "aarch64"],
        _ => &["x64", "x86_64", "amd64"],
    }
}

/// 从文件名/URL 判断架构；无法判断时返回 None。
pub fn detect_url_arch(url: &str) -> Option<&'static str> {
    let name = url_file_name(url).unwrap_or_default().to_ascii_lowercase();
    for arch in ["arm64", "x64"] {
        if arch_aliases(arch).iter().any(|a| name.contains(a)) {
            return Some(arch);
        }
    }
    None
}

/// 扩展名是否属于该平台。
pub fn ext_matches_platform(url: &str, os: &str) -> bool {
    url_file_name(url)
        .map(|n| n.to_ascii_lowercase().ends_with(platform_ext(os)))
        .unwrap_or(false)
}

/// 文件名明显属于其它架构时返回其架构（无法判断/一致时返回 None）。
pub fn arch_mismatch(url: &str, arch: &str) -> Option<&'static str> {
    match detect_url_arch(url) {
        Some(a) if a != arch => Some(a),
        _ => None,
    }
}

/// 校验下载地址是否适用于当前平台/架构（**不静默放过无法判断的情况**）。
pub fn check_asset_for_platform(url: &str, os: &str, arch: &str) -> Result<(), String> {
    if url.trim().is_empty() {
        return Err("下载地址为空".into());
    }
    if !ext_matches_platform(url, os) {
        let got = url_file_name(url)
            .and_then(|n| n.rsplit('.').next().map(|e| format!(".{}", e.to_ascii_lowercase())))
            .unwrap_or_else(|| "（无法解析）".into());
        return Err(format!(
            "安装包类型 {got} 与当前平台 {os} 不匹配（应为 {}）",
            platform_ext(os)
        ));
    }
    match detect_url_arch(url) {
        Some(a) if a == arch => Ok(()),
        Some(a) => Err(format!("安装包架构 {a} 与当前架构 {arch} 不匹配")),
        None => Err(format!(
            "无法从文件名判断安装包架构（当前 {arch}），为避免下错包已拒绝"
        )),
    }
}

/// 本平台安装包的标准命名（同时含平台与架构标识，避免覆盖错包）。
fn platform_installer_name(version: &str, os: &str, arch: &str) -> String {
    let safe = version_dir_name(version);
    format!("AniBox_{safe}_{os}_{arch}{}", platform_ext(os))
}

/// 保证本地保存的文件名带平台/架构标识；缺标识时改用标准命名。
fn ensure_platform_file_name(file: &str, version: &str, os: &str, arch: &str) -> String {
    let ext_ok = file.to_ascii_lowercase().ends_with(platform_ext(os));
    let arch_ok = detect_url_arch(file) == Some(arch);
    if ext_ok && arch_ok {
        file.to_string()
    } else {
        platform_installer_name(version, os, arch)
    }
}

/// 按已知命名规则构造本平台安装包候选地址：
/// 镜像 `/app/{tag}/{file}`（主址 + workers 兜底）→ GitHub Releases 直链。
pub fn candidate_asset_urls(version: &str, os: &str, arch: &str) -> Vec<String> {
    let v = normalize_display_version(version);
    if v.is_empty() {
        return Vec::new();
    }
    let tag = format!("v{v}");
    let ext = platform_ext(os);
    let files: Vec<String> = ["AniBox", "Fntv-Plus"]
        .iter()
        .map(|product| format!("{product}_{v}_{os}_{arch}{ext}"))
        .collect();
    let mut out: Vec<String> = Vec::new();
    for host in mirror_hosts() {
        for f in &files {
            out.push(format!("{host}/app/{tag}/{f}"));
        }
    }
    for f in &files {
        out.push(format!(
            "https://github.com/FNOSP/AniBox/releases/download/{tag}/{f}"
        ));
    }
    out
}

/// 依次 HEAD 探测候选地址，命中（2xx）即返回 (url, 大小)。
async fn resolve_platform_candidates(version: &str, os: &str, arch: &str) -> Option<(String, u64)> {
    for url in candidate_asset_urls(version, os, arch) {
        let resp = client()
            .head(&url)
            .timeout(std::time::Duration::from_secs(6))
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let size = r.content_length().unwrap_or(0);
                trace(&format!("[update] 本平台候选安装包命中: {url}（{size}B）"));
                return Some((url, size));
            }
            Ok(r) => trace(&format!("[update] 候选安装包不可用（HTTP {}）: {url}", r.status())),
            Err(e) => trace(&format!("[update] 候选安装包探测失败（{e}）: {url}")),
        }
    }
    None
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

/// 更新源标识（用于日志与下载回退说明；顺序即优先级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestSource {
    Mirror,
    Gitee,
    GitHub,
    Custom,
}

impl ManifestSource {
    fn label(self) -> &'static str {
        match self {
            ManifestSource::Mirror => "自建镜像",
            ManifestSource::Gitee => "Gitee",
            ManifestSource::GitHub => "原生 GitHub",
            ManifestSource::Custom => "自定义清单",
        }
    }
}

/// 有序候选源：镜像（主址 + workers 兜底）→ Gitee → 原生 GitHub。
/// `FNTV_UPDATE_MANIFEST` 设置时只走该地址（本地调试 / 测试用）。
fn manifest_candidates() -> Vec<(ManifestSource, String)> {
    if let Ok(u) = std::env::var("FNTV_UPDATE_MANIFEST") {
        let u = u.trim().to_string();
        if !u.is_empty() {
            return vec![(ManifestSource::Custom, u)];
        }
    }
    let mut out: Vec<(ManifestSource, String)> = Vec::new();
    for host in mirror_hosts() {
        out.push((ManifestSource::Mirror, format!("{host}{MIRROR_VERSION_PATH}")));
    }
    out.push((ManifestSource::Gitee, DEFAULT_MANIFEST_URL.to_string()));
    out.push((ManifestSource::GitHub, NATIVE_MANIFEST_URL.to_string()));
    out
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

/// 镜像 `/version` 走签名轨；未配置 `FNTV_GH_SECRET` 时不签名（镜像会 401，随后回退下一源）。
fn gh_secret() -> Option<String> {
    crate::mirror_sign::resolve_secret("FNTV_GH_SECRET", option_env!("FNTV_GH_SECRET"))
}

fn manifest_request(source: ManifestSource, url: &str) -> reqwest::RequestBuilder {
    let mut req = client()
        .get(url)
        .header("Cache-Control", "no-cache")
        .timeout(MANIFEST_TIMEOUT);
    if source == ManifestSource::Mirror {
        if let Some(secret) = gh_secret() {
            req = crate::mirror_sign::apply_auth(req, &secret, "GET", MIRROR_VERSION_PATH, "");
        }
    }
    req
}

async fn fetch_manifest_at(source: ManifestSource, url: &str) -> Result<Manifest, String> {
    let resp = manifest_request(source, url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let text = resp.text().await.map_err(|e| format!("读取失败: {e}"))?;
    let m: Manifest =
        serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败: {e}"))?;
    if m.raw_version().trim().is_empty() {
        return Err("清单缺少版本号".into());
    }
    Ok(m)
}

/// 依次尝试三源，**第一个成功解析出有效清单即停止**（后续源不再请求）。
async fn fetch_manifest_ordered() -> Result<(Manifest, ManifestSource), String> {
    let mut errors: Vec<String> = Vec::new();
    for (source, url) in manifest_candidates() {
        match fetch_manifest_at(source, &url).await {
            Ok(m) => {
                trace(&format!(
                    "[update] 使用更新源: {}（{}）→ 版本 {}",
                    source.label(),
                    url,
                    m.raw_version()
                ));
                return Ok((m, source));
            }
            Err(e) => {
                trace(&format!(
                    "[update] 更新源 {} 不可用（{}）: {e}",
                    source.label(),
                    url
                ));
                errors.push(format!("{}: {e}", source.label()));
            }
        }
    }
    Err(format!("所有更新源均失败（{}）", errors.join("；")))
}

/// 兼容旧调用点：只要清单，不要源标识。
async fn fetch_manifest() -> Result<Manifest, String> {
    fetch_manifest_ordered().await.map(|(m, _)| m)
}

async fn resolve_asset(host: &str, version: &str) -> Result<Asset, String> {
    let v = normalize_display_version(version);
    let tag = format!("v{v}");
    // worker 的 os 参数只接受 win|mac|linux|fnos，arch 接受 x64|arm64（含别名）
    let os = target_os_slug();
    let arch = target_arch_slug();
    let url = format!(
        "{host}/app/resolve?os={}&arch={}&tag={}",
        urlencoding::encode(os),
        urlencoding::encode(arch),
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
    // 防御：worker 若返回了明显属于其它架构的包（如 arm64 机器拿到 x64 包），拒绝而不是静默下载
    if let Some(other) = arch_mismatch(&d.file, arch) {
        return Err(format!(
            "{host} 返回的安装包架构 {other} 与当前 {arch} 不匹配（{}）",
            d.file
        ));
    }
    Ok(Asset {
        file: d.file,
        size: d.size,
        url: d.url,
    })
}

fn speed_bps(delta: u64, elapsed: std::time::Duration) -> u64 {
    let secs = elapsed.as_secs_f64();
    if secs > 0.05 {
        (delta as f64 / secs) as u64
    } else {
        0
    }
}

async fn fetch_patch(url: &str) -> Result<PatchManifest, String> {
    fetch_patch_with_progress(None, url).await
}

/// 下载补丁清单；`app` 有值时推送 `patch-progress`（百分比 + 速度）。
async fn fetch_patch_with_progress(
    app: Option<&AppHandle>,
    url: &str,
) -> Result<PatchManifest, String> {
    let mut resp = client()
        .get(url)
        .header("Cache-Control", "no-cache")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("补丁清单请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("补丁清单 HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut bytes: Vec<u8> = Vec::new();
    let mut last_emit = tokio::time::Instant::now();
    let mut last_len = 0usize;
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return Err(format!("读取补丁清单失败: {e}")),
        };
        if chunk.is_empty() {
            continue;
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_PATCH_JSON_BYTES {
            return Err("补丁清单过大".into());
        }
        if last_emit.elapsed() >= std::time::Duration::from_millis(150) {
            if let Some(app) = app {
                let delta = (bytes.len() - last_len) as u64;
                let speed = speed_bps(delta, last_emit.elapsed());
                emit_progress(
                    app,
                    PROGRESS_EVENT_PATCH,
                    "downloading",
                    bytes.len() as u64,
                    total,
                    speed,
                    "正在下载补丁…",
                );
            }
            last_emit = tokio::time::Instant::now();
            last_len = bytes.len();
        }
    }
    if let Some(app) = app {
        emit_progress(
            app,
            PROGRESS_EVENT_PATCH,
            "downloading",
            bytes.len() as u64,
            if total > 0 { total } else { bytes.len() as u64 },
            0,
            "补丁下载完成",
        );
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

    // 冷却：30 秒内重复调用直接返回上次结果（启动自动检测也受此约束）
    let left = cooldown_left("check");
    if left > 0 {
        if let Ok(cache) = check_cache().lock() {
            if let Some((_, info)) = cache.as_ref() {
                let mut info = info.clone();
                info.cooldown_remaining = left;
                info.from_cache = true;
                trace(&format!("[update] 命中冷却缓存（剩余 {left}s），不重复请求更新源"));
                return Ok(info);
            }
        }
    }
    cooldown_mark("check");

    let (m, source) = match fetch_manifest_ordered().await {
        Ok(v) => v,
        Err(e) => {
            // 与旧项目一致：清单不可达时静默视为“无更新”，不打扰用户
            trace(&format!("[update] 检查失败（视为无更新）: {e}"));
            let info = UpdateInfo {
                has_update: false,
                current_version: current,
                latest_version: String::new(),
                update_type: "full".into(),
                notes: String::new(),
                published_at: String::new(),
                download_url: None,
                patch_url: None,
                html_url: REPO_URL_PLACEHOLDER.to_string(),
                platform: target_os_slug().to_string(),
                arch: target_arch_slug().to_string(),
                source: String::new(),
                cooldown_remaining: 0,
                from_cache: false,
            };
            if let Ok(mut cache) = check_cache().lock() {
                *cache = Some((std::time::Instant::now(), info.clone()));
            }
            return Ok(info);
        }
    };

    let latest_raw = m.raw_version();
    let latest = normalize_display_version(&latest_raw);
    let kind = classify_update(&current, &latest_raw);
    let has_update = kind.is_some();
    let update_type = kind.unwrap_or("full").to_string();
    trace(&format!(
        "[update] 当前 {current} / 最新 {latest} / 类型 {update_type} / hasUpdate={has_update} / 源 {source:?}"
    ));

    let os = target_os_slug();
    let arch = target_arch_slug();
    let mut download_url = if m.download_url.trim().is_empty() {
        None
    } else {
        Some(m.download_url.trim().to_string())
    };

    // 清单里的 downloadUrl 可能指向其它平台（例如 Windows exe 发给 mac/linux）。
    // 命中更新时先校验；不匹配就尝试按命名规则找本平台包，找不到则清空，避免前端误导。
    if has_update {
        if let Some(dl) = download_url.clone() {
            if let Err(e) = check_asset_for_platform(&dl, os, arch) {
                trace(&format!(
                    "[update] 清单 downloadUrl 不适用于当前平台（{os}/{arch}）: {e}；尝试按命名规则查找"
                ));
                download_url = resolve_platform_candidates(&latest, os, arch)
                    .await
                    .map(|(u, _)| u);
                if download_url.is_none() {
                    trace("[update] 未找到本平台候选安装包，downloadUrl 置空（请到 Releases 手动下载）");
                }
            }
        }
    }

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

    let info = UpdateInfo {
        has_update,
        current_version: current,
        latest_version: latest,
        update_type,
        notes: m.notes,
        published_at: m.date,
        download_url,
        patch_url,
        html_url,
        platform: os.to_string(),
        arch: arch.to_string(),
        source: source.label().to_string(),
        cooldown_remaining: 0,
        from_cache: false,
    };
    if let Ok(mut cache) = check_cache().lock() {
        *cache = Some((std::time::Instant::now(), info.clone()));
    }
    Ok(info)
}

// ───────────────────────── 命令：检测源探测 ─────────────────────────

async fn probe_one(
    id: &'static str,
    label: &'static str,
    source: ManifestSource,
    url: &str,
    respect_cooldown: bool,
) -> ProbeItem {
    // 同一源 30 秒内只真正请求一次
    if respect_cooldown {
        let left = cooldown_left(id);
        if left > 0 {
            return ProbeItem {
                id: id.to_string(),
                label: label.to_string(),
                url: url.to_string(),
                ok: false,
                status: None,
                elapsed_ms: 0,
                version: String::new(),
                error: String::new(),
                cooldown_remaining: left,
                skipped: true,
            };
        }
        cooldown_mark(id);
    }

    let started = std::time::Instant::now();
    let resp = match manifest_request(source, url).send().await {
        Ok(r) => r,
        Err(e) => {
            return ProbeItem {
                id: id.to_string(),
                label: label.to_string(),
                url: url.to_string(),
                ok: false,
                status: None,
                elapsed_ms: started.elapsed().as_millis() as u64,
                version: String::new(),
                error: format!("请求失败: {e}"),
                cooldown_remaining: 0,
                skipped: false,
            };
        }
    };
    let status = resp.status();
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if !status.is_success() {
        return ProbeItem {
            id: id.to_string(),
            label: label.to_string(),
            url: url.to_string(),
            ok: false,
            status: Some(status.as_u16()),
            elapsed_ms,
            version: String::new(),
            error: format!("HTTP {status}"),
            cooldown_remaining: 0,
            skipped: false,
        };
    }
    let text = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            return ProbeItem {
                id: id.to_string(),
                label: label.to_string(),
                url: url.to_string(),
                ok: false,
                status: Some(status.as_u16()),
                elapsed_ms,
                version: String::new(),
                error: format!("读取失败: {e}"),
                cooldown_remaining: 0,
                skipped: false,
            };
        }
    };
    match serde_json::from_str::<Manifest>(&text) {
        Ok(m) if !m.raw_version().trim().is_empty() => ProbeItem {
            id: id.to_string(),
            label: label.to_string(),
            url: url.to_string(),
            ok: true,
            status: Some(status.as_u16()),
            elapsed_ms,
            version: m.raw_version(),
            error: String::new(),
            cooldown_remaining: 0,
            skipped: false,
        },
        Ok(_) => ProbeItem {
            id: id.to_string(),
            label: label.to_string(),
            url: url.to_string(),
            ok: false,
            status: Some(status.as_u16()),
            elapsed_ms,
            version: String::new(),
            error: "清单缺少版本号".into(),
            cooldown_remaining: 0,
            skipped: false,
        },
        Err(e) => ProbeItem {
            id: id.to_string(),
            label: label.to_string(),
            url: url.to_string(),
            ok: false,
            status: Some(status.as_u16()),
            elapsed_ms,
            version: String::new(),
            error: format!("JSON 解析失败: {e}"),
            cooldown_remaining: 0,
            skipped: false,
        },
    }
}

/// 依次探测三个源（镜像主址 → workers 兜底 → Gitee → 原生 GitHub），只读、不改更新状态。
#[tauri::command]
pub async fn update_source_probe() -> Result<ProbeReport, String> {
    let mut items: Vec<ProbeItem> = Vec::new();

    // 自建镜像：主址失败再试 workers 兜底（同属一个「源」）
    let hosts = mirror_hosts();
    let mut mirror_item: Option<ProbeItem> = None;
    for (idx, host) in hosts.iter().enumerate() {
        let url = format!("{host}{MIRROR_VERSION_PATH}");
        let item = probe_one(
            "mirror",
            "自建镜像",
            ManifestSource::Mirror,
            &url,
            idx == 0,
        )
        .await;
        let stop = item.ok || item.skipped;
        mirror_item = Some(item);
        if stop {
            break;
        }
    }
    if let Some(item) = mirror_item {
        items.push(item);
    }

    items.push(
        probe_one(
            "gitee",
            "Gitee",
            ManifestSource::Gitee,
            DEFAULT_MANIFEST_URL,
            true,
        )
        .await,
    );
    items.push(
        probe_one(
            "github",
            "原生 GitHub",
            ManifestSource::GitHub,
            NATIVE_MANIFEST_URL,
            true,
        )
        .await,
    );

    let cooldown_remaining = items
        .iter()
        .map(|i| i.cooldown_remaining)
        .max()
        .unwrap_or(0)
        .max(if items.iter().any(|i| !i.skipped) {
            CHECK_COOLDOWN.as_secs()
        } else {
            0
        });
    trace(&format!(
        "[update] 源探测完成: {}",
        items
            .iter()
            .map(|i| format!(
                "{}={}",
                i.label,
                if i.skipped {
                    format!("冷却{}s", i.cooldown_remaining)
                } else if i.ok {
                    format!("OK({}ms)", i.elapsed_ms)
                } else {
                    format!("FAIL({})", i.error)
                }
            ))
            .collect::<Vec<_>>()
            .join(" / ")
    ));
    Ok(ProbeReport {
        items,
        cooldown_remaining,
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
    emit_progress(&app, PROGRESS_EVENT_UPDATE, "checking", 0, 0, 0, "正在解析安装包…");

    let res = download_inner(&app, &version).await;
    if let Err(e) = &res {
        emit_progress(&app, PROGRESS_EVENT_UPDATE, "error", 0, 0, 0, e.clone());
        trace(&format!("[update] 下载失败: {e}"));
    }
    res
}

async fn download_inner(app: &AppHandle, version: &str) -> Result<DownloadResult, String> {
    // 1) 镜像解析（逐个 host 试；`/app/resolve` 免签名）
    let mut asset: Option<Asset> = None;
    for host in mirror_hosts() {
        match resolve_asset(&host, version).await {
            Ok(a) => {
                trace(&format!(
                    "[update] 下载源: 自建镜像 {host} → {} {}B",
                    a.file, a.size
                ));
                asset = Some(a);
                break;
            }
            Err(e) => trace(&format!("[update] 镜像不可用: {e}")),
        }
    }

    // 2) 解析失败 → 清单 downloadUrl 兜底（清单本身也按 镜像 → Gitee → 原生 GitHub 取）
    //    兜底时必须校验平台/架构，绝不把其它平台的包静默下给用户。
    let (url, file, size) = match asset {
        Some(a) => {
            let file = ensure_platform_file_name(
                &a.file,
                version,
                target_os_slug(),
                target_arch_slug(),
            );
            (a.url, file, a.size)
        }
        None => {
            let (m, source) = fetch_manifest_ordered()
                .await
                .map_err(|e| format!("镜像解析失败且清单不可用: {e}"))?;
            let dl = m.download_url.trim().to_string();
            let os = target_os_slug();
            let arch = target_arch_slug();
            let compat = if dl.is_empty() {
                Err("清单未提供 downloadUrl".to_string())
            } else {
                check_asset_for_platform(&dl, os, arch)
            };
            match compat {
                Ok(()) => {
                    let file = url_file_name(&dl)
                        .map(|n| ensure_platform_file_name(&n, version, os, arch))
                        .unwrap_or_else(|| platform_installer_name(version, os, arch));
                    trace(&format!(
                        "[update] 下载源: {}（清单 downloadUrl）→ {dl}",
                        source.label()
                    ));
                    (dl, file, 0)
                }
                Err(e) => {
                    trace(&format!(
                        "[update] 清单 downloadUrl 不可用（{os}/{arch}）: {e}；尝试按命名规则查找本平台安装包"
                    ));
                    match resolve_platform_candidates(version, os, arch).await {
                        Some((u, sz)) => {
                            let file = url_file_name(&u)
                                .map(|n| ensure_platform_file_name(&n, version, os, arch))
                                .unwrap_or_else(|| platform_installer_name(version, os, arch));
                            trace(&format!("[update] 下载源: 本平台候选安装包 → {u}"));
                            (u, file, sz)
                        }
                        None => {
                            return Err(format!(
                                "未找到适用于当前平台（{os}/{arch}）的安装包：{e}。\
                                 请前往 {REPO_URL_PLACEHOLDER}/releases 手动下载对应平台的安装包"
                            ));
                        }
                    }
                }
            }
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
                0,
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
        0,
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
    let mut last_loaded = start;
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
            let speed = speed_bps(loaded - last_loaded, last_emit.elapsed());
            emit_progress(
                app,
                PROGRESS_EVENT_UPDATE,
                "downloading",
                loaded,
                total,
                speed,
                "下载中…",
            );
            last_emit = tokio::time::Instant::now();
            last_loaded = loaded;
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
        0,
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
            emit_progress(&app, PROGRESS_EVENT_PATCH, "error", 0, 0, 0, e.clone());
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
        0,
        "正在获取补丁清单…",
    );

    let current = app.package_info().version.to_string();
    let m = fetch_manifest().await?;
    let patch_url = m.patch_url.trim().to_string();
    if patch_url.is_empty() {
        return Err("更新清单未提供 patchUrl".into());
    }

    let patch = fetch_patch_with_progress(Some(app), &patch_url).await?;

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
        0,
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
        0,
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

    // ── 平台 / 架构映射 ──

    #[test]
    fn os_slug_mapping() {
        assert_eq!(os_slug_for("windows"), "win");
        assert_eq!(os_slug_for("win"), "win");
        assert_eq!(os_slug_for("macos"), "mac");
        assert_eq!(os_slug_for("darwin"), "mac");
        assert_eq!(os_slug_for("linux"), "linux");
        assert_eq!(os_slug_for("freebsd"), "linux");
        assert!(matches!(target_os_slug(), "win" | "mac" | "linux"));
    }

    #[test]
    fn arch_slug_mapping() {
        assert_eq!(arch_slug_for("x86_64"), "x64");
        assert_eq!(arch_slug_for("amd64"), "x64");
        assert_eq!(arch_slug_for("aarch64"), "arm64");
        assert_eq!(arch_slug_for("arm64"), "arm64");
        assert!(matches!(target_arch_slug(), "x64" | "arm64"));
    }

    #[test]
    fn platform_extension() {
        assert_eq!(platform_ext("win"), ".exe");
        assert_eq!(platform_ext("mac"), ".dmg");
        assert_eq!(platform_ext("linux"), ".appimage");
    }

    #[test]
    fn url_arch_detection() {
        assert_eq!(detect_url_arch("AniBox_1.0.0_win_x64.exe"), Some("x64"));
        assert_eq!(detect_url_arch("AniBox_1.0.0_mac_arm64.dmg"), Some("arm64"));
        assert_eq!(detect_url_arch("Fntv-Plus_1.0.0_linux_x86_64.AppImage"), Some("x64"));
        assert_eq!(detect_url_arch("Fntv-Plus_1.0.0_linux_aarch64.AppImage"), Some("arm64"));
        assert_eq!(detect_url_arch("AniBox_1.0.0_mac.dmg"), None);
    }

    #[test]
    fn platform_compat_accepts_matching_assets() {
        assert!(check_asset_for_platform("https://x/AniBox_1.0_win_x64.exe", "win", "x64").is_ok());
        assert!(check_asset_for_platform("https://x/AniBox_1.0_mac_arm64.dmg", "mac", "arm64").is_ok());
        assert!(check_asset_for_platform("https://x/AniBox_1.0_linux_x86_64.AppImage", "linux", "x64").is_ok());
    }

    #[test]
    fn platform_compat_rejects_wrong_platform_and_arch() {
        // Windows exe 发给 mac
        let e = check_asset_for_platform("https://x/AniBox_1.0_win_x64.exe", "mac", "arm64").unwrap_err();
        assert!(e.contains("不匹配"), "{e}");
        // 架构不符
        let e = check_asset_for_platform("https://x/AniBox_1.0_mac_arm64.dmg", "mac", "x64").unwrap_err();
        assert!(e.contains("架构"), "{e}");
        // 无法判断架构 → 明确报错，不静默放过
        let e = check_asset_for_platform("https://x/AniBox_1.0_mac.dmg", "mac", "x64").unwrap_err();
        assert!(e.contains("无法从文件名判断"), "{e}");
        // 无扩展名
        assert!(check_asset_for_platform("https://x/download", "win", "x64").is_err());
        // 空地址
        assert!(check_asset_for_platform("", "win", "x64").is_err());
    }

    #[test]
    fn candidate_urls_follow_naming_rule() {
        let urls = candidate_asset_urls("1.2.3", "mac", "arm64");
        assert!(urls.iter().any(|u| u.ends_with("/app/v1.2.3/AniBox_1.2.3_mac_arm64.dmg")));
        assert!(urls.iter().any(|u| u.ends_with("/app/v1.2.3/Fntv-Plus_1.2.3_mac_arm64.dmg")));
        assert!(urls.iter().any(|u| u.contains("github.com/FNOSP/AniBox/releases/download/v1.2.3/")));
        // 去掉 v 前缀，不重复
        assert_eq!(candidate_asset_urls("v1.2.3", "win", "x64"), candidate_asset_urls("1.2.3", "win", "x64"));
        assert!(candidate_asset_urls("", "win", "x64").is_empty());
    }

    #[test]
    fn installer_name_carries_platform() {
        assert_eq!(platform_installer_name("v1.2.3", "linux", "arm64"), "AniBox_1.2.3_linux_arm64.appimage");
        // 正确命名的文件保持不变
        assert_eq!(
            ensure_platform_file_name("AniBox_1.2.3_win_x64.exe", "1.2.3", "win", "x64"),
            "AniBox_1.2.3_win_x64.exe"
        );
        // 缺架构标识 / 平台不符 → 改用标准命名，避免覆盖错包
        assert_eq!(
            ensure_platform_file_name("setup.exe", "1.2.3", "win", "x64"),
            "AniBox_1.2.3_win_x64.exe"
        );
        assert_eq!(
            ensure_platform_file_name("AniBox_1.2.3_win_x64.exe", "1.2.3", "mac", "arm64"),
            "AniBox_1.2.3_mac_arm64.dmg"
        );
    }
}
