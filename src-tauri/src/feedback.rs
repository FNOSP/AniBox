//! [lc-1599] 应用内反馈（工单制）—— App 侧：提交 / 列表 / 会话 / 回复。
//!
//! 服务端（已冻结，由主对话部署）：https://stats.690075.xyz
//!   · POST /feedback                { message, contact?, log?, aid?, v?, os?, arch?, page? } → { ok, id }
//!   · GET  /feedback/ticket?id=     → { ok, ticket, messages }
//!   · POST /feedback/reply          { id, text } → { ok, ts }
//!   · GET  /feedback/mine?ids=a,b,c （≤20 个）→ { ok, items }
//!
//! 本地状态（都在 app_data_dir，即 %APPDATA%/com.anibox.app）：
//!   · 匿名 ID：config.json 的 feedback_aid（首次使用时生成 32 位 hex 随机串落盘，不要求密码学强度）
//!   · 工单索引：feedback.json（[{ id, ts, title }]，新提交前插、最多 50 条）
//!
//! 日志附件（include_log=true 时）：
//!   · 读 %TEMP%/anibox.log 尾部（默认 512KB、硬上限 2MB；文件不存在/为空则跳过）
//!   · 先脱敏（见本文件「脱敏」段 —— 密码/token/手机号/邮箱/本机用户名；**保留域名与 NAS 地址**，
//!     那是排查必需的上下文）
//!   · 头部拼环境块：`anibox 0.1.0 · Windows x64 · 2026-10-16 12:00 · page=detail`
//!
//! 错误约定：网络类错误一律翻成可读中文（「无法连接反馈服务，请检查网络」这类）；
//! feedback_list 网络失败时**降级**为本地数据 + status="unknown"，不报错（列表永远可用）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Manager, State};

use crate::{config, libmpv, AppState};

/// 反馈服务基址（任务规格冻结）
const API_BASE: &str = "https://stats.690075.xyz";
/// 单次请求超时：15s（任务规格）
const TIMEOUT_SECS: u64 = 15;
/// 日志尾部默认读取量：512KB
const LOG_TAIL_BYTES: u64 = 512 * 1024;
/// 日志尾部硬上限：2MB（读取量无论如何不超过这个数）
const LOG_TAIL_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// 本地索引最多保留条数
const INDEX_MAX: usize = 50;
/// /feedback/mine 单次查询上限（服务端约定 ≤20 个 id）
const MINE_BATCH_MAX: usize = 20;

// ───────────────────────── HTTP ─────────────────────────

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("AniBox/0.1")
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .build()
            .expect("feedback http client")
    })
}

/// 把 reqwest 错误翻成人话（用户能在反馈面板里看懂的那种）。
fn net_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "反馈服务响应超时，请稍后重试".into()
    } else if e.is_connect() {
        "无法连接反馈服务，请检查网络".into()
    } else {
        format!("反馈服务请求失败：{e}")
    }
}

/// 发请求 → 解析 JSON → 统一错误文案。业务层 `ok:false` 也在这里拦。
async fn send_json(req: reqwest::RequestBuilder) -> Result<Value, String> {
    let resp = req.send().await.map_err(|e| net_err(&e))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取反馈服务响应失败：{e}"))?;
    let v: Value =
        serde_json::from_str(&body).map_err(|_| format!("反馈服务返回异常（HTTP {status}）"))?;
    if !status.is_success() {
        return Err(format!("反馈服务返回错误（HTTP {status}）"));
    }
    if v.get("ok").and_then(|x| x.as_bool()) == Some(false) {
        let msg = v
            .get("error")
            .or_else(|| v.get("message"))
            .and_then(|x| x.as_str())
            .unwrap_or("服务端拒绝了请求");
        return Err(format!("反馈服务：{msg}"));
    }
    Ok(v)
}

// ───────────────────────── 本地索引 / 匿名 ID / 时间 ─────────────────────────

/// 本地工单索引条目（title = 提交内容前 40 字，用于列表展示）
#[derive(Serialize, Deserialize, Clone)]
struct TicketMeta {
    id: String,
    ts: u64,
    title: String,
}

fn index_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录：{e}"))?;
    Ok(dir.join("feedback.json"))
}

async fn load_index(app: &AppHandle) -> Vec<TicketMeta> {
    match index_path(app) {
        Ok(p) => match tokio::fs::read_to_string(&p).await {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    }
}

async fn save_index(app: &AppHandle, list: &[TicketMeta]) -> Result<(), String> {
    let p = index_path(app)?;
    if let Some(parent) = p.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建数据目录失败：{e}"))?;
    }
    tokio::fs::write(
        &p,
        serde_json::to_string_pretty(list).map_err(|e| e.to_string())?,
    )
    .await
    .map_err(|e| format!("写入反馈索引失败：{e}"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 匿名 ID：首次使用时生成 32 位 hex（两个 u64 随机数拼接；不要求密码学强度，见任务规格）。
async fn ensure_aid(app: &AppHandle, state: &State<'_, AppState>) -> Result<String, String> {
    let mut cfg = state.config.write().await;
    if cfg.feedback_aid.is_empty() {
        let a: u64 = rand::random();
        let b: u64 = rand::random();
        cfg.feedback_aid = format!("{a:016x}{b:016x}");
        config::save(app, &cfg).await?;
    }
    Ok(cfg.feedback_aid.clone())
}

/// 列表标题 = 提交内容前 40 字（换行/连续空白先折叠成单个空格，避免列表行被撑破）
fn make_title(message: &str) -> String {
    message
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(40)
        .collect()
}

fn os_label() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

fn arch_label() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// 本地时间 `YYYY-MM-DD HH:MM`（Windows 走 GetLocalTime；其他平台退化为 UTC —— 本软件当前只发 Windows）。
#[cfg(target_os = "windows")]
fn now_local_str() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut st: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute
    )
}

#[cfg(not(target_os = "windows"))]
fn now_local_str() -> String {
    // 民用历算法（Howard Hinnant days_from_civil 的逆变换），UTC
    let secs = now_secs();
    let days = (secs / 86400) as i64;
    let (hh, mm) = ((secs % 86400) / 3600, (secs % 3600) / 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if mo <= 2 { 1 } else { 0 };
    format!("{y:04}-{mo:02}-{d:02} {hh:02}:{mm:02}")
}

/// 环境块（拼在日志正文头部）：版本 / 系统 / 架构 / 本地时间 / 页面
fn env_block(page: Option<&str>) -> String {
    let page = page
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("-");
    format!(
        "anibox {} · {} {} · {} · page={}",
        env!("CARGO_PKG_VERSION"),
        os_label(),
        arch_label(),
        now_local_str(),
        page
    )
}

// ───────────────────────── 日志尾部 ─────────────────────────

/// MPV 播放器日志（mpv.rs 启动时 `--log-file` 写到这里；与诊断日志同目录）。
fn mpv_log_path() -> std::path::PathBuf {
    std::env::temp_dir().join("anibox-mpv.log")
}

/// 读任意日志文件尾部。`max_bytes` 会被硬上限 LOG_TAIL_MAX_BYTES 夹住；
/// 文件不存在/为空返回 None。截断时丢掉第一个半行（避免半行/半个 UTF-8 字符开头）。
fn read_tail_path(p: &std::path::Path, max_bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let meta = std::fs::metadata(p).ok()?;
    let len = meta.len();
    if len == 0 {
        return None;
    }
    let want = max_bytes.min(LOG_TAIL_MAX_BYTES);
    let start = len.saturating_sub(want);
    let mut f = std::fs::File::open(p).ok()?;
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf).ok()?;
    let slice = if start > 0 {
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => &buf[i + 1..],
            None => &buf[..],
        }
    } else {
        &buf[..]
    };
    let text = String::from_utf8_lossy(slice).into_owned();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// 读诊断日志尾部。默认 LOG_TAIL_BYTES；文件不存在/为空返回 None（跳过日志附件）。
fn read_log_tail() -> Option<String> {
    read_tail_path(&libmpv::log_path(), LOG_TAIL_BYTES)
}

// ───────────────────────── 脱敏 ─────────────────────────
// 逐条正则替换，全部大小写不敏感；替换串用 $1 保留键名。
//   · 密码类 → 全遮蔽（8 个 *）
//   · token 类 → 部分遮蔽（留前 4 后 4，中间 *，上限 10 个）
//   · URL 敏感参数 → 值部分遮蔽
//   · 手机号 → 前 3 后 4 中间 *
//   · 邮箱 → 局部名只留前 2 字符（≤2 字符全 *）
//   · 本机用户名路径 → $1<USER>
// 域名 / NAS 地址（192.168.x.x、host:port）**不动** —— 排查必需。
//
// 脱敏器内部异常一律吞掉（宁可不脱敏也不能崩）：每条规则独立 catch_unwind，
// 且实现本身全部按字符（chars）处理、不依赖字节切片索引 —— 不产生 panic 是主保证
// （release 构建 panic=abort，catch_unwind 只是 debug 下的第二层保险）。

struct Rules {
    password: Option<regex::Regex>,
    token: Option<regex::Regex>,
    bearer: Option<regex::Regex>,
    mc_token: Option<regex::Regex>,
    url_param: Option<regex::Regex>,
    phone: Option<regex::Regex>,
    email: Option<regex::Regex>,
    win_path: Option<regex::Regex>,
    nix_path: Option<regex::Regex>,
}

fn build_re(pat: &str) -> Option<regex::Regex> {
    regex::RegexBuilder::new(pat)
        .case_insensitive(true)
        .build()
        .ok()
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| Rules {
        password: build_re(r#"(password|pwd|passwd)['":\s]*['"]?([^'",\s}]+)"#),
        token: build_re(
            r#"(token|authorization|access_token|refresh_token|api_key|apikey|secret)['":\s*]*['"]?([^'",\s}]+)"#,
        ),
        // [lc-1599] 补两条原项目没有的：
        //   ① `Authorization: Bearer <JWT>` —— 原规则只遮到 "Bearer" 这个词本身，后面 JWT 裸露；
        //   ② URL 编码形态的 Trim-MC-token（`%3D` 分隔）—— 普通 token 规则字符类不含 `=`/`%`，会漏。
        bearer: build_re(r#"(bearer\s+)(\S+)"#),
        mc_token: build_re(r#"(trim-mc-token(?:%3D|=))([^&\s"']+)"#),
        url_param: build_re(r#"([?&](?:token|key|secret|password|pwd)=)([^&\s]+)"#),
        phone: build_re(r#"\b(1[3-9]\d{9})\b"#),
        email: build_re(r#"\b([A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,})\b"#),
        win_path: build_re(r#"([A-Za-z]:[\\/]+Users[\\/]+)([^\\\/\r\n"']+)"#),
        nix_path: build_re(r#"(\/(?:home|Users)\/)([^\/\r\n"']+)"#),
    })
}

/// 单条规则执行；规则缺失或内部异常 → 返回原文（宁可不脱敏）。
fn apply_rule(text: &str, f: impl FnOnce(&str) -> Option<String>) -> String {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(text))) {
        Ok(Some(s)) => s,
        _ => text.to_string(),
    }
}

/// token 类部分遮蔽：留前 4 后 4，中间 *（上限 10 个）；短值降级为更保守的遮蔽。
fn mask_partial(v: &str) -> String {
    let n = v.chars().count();
    if n <= 4 {
        return "****".into();
    }
    if n <= 8 {
        let head: String = v.chars().take(2).collect();
        let tail: String = v.chars().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect();
        return format!("{head}****{tail}");
    }
    let head: String = v.chars().take(4).collect();
    let tail: String = v.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    let stars = "*".repeat((n - 8).min(10));
    format!("{head}{stars}{tail}")
}

pub fn redact(text: &str) -> String {
    let r = rules();
    let mut out = text.to_string();

    // ⓪ [lc-1599] Bearer <JWT>：**必须在 token 规则之前** —— 否则 "authorization: Bearer xxx"
    //   会被 token 规则先吃掉 "Bearer" 这个词，留下后面的 JWT 裸露。
    if let Some(re) = &r.bearer {
        out = apply_rule(&out, |t| {
            Some(
                re.replace_all(t, |caps: &regex::Captures| {
                    let key = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let val = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    format!("{key}{}", mask_partial(val))
                })
                .into_owned(),
            )
        });
    }

    // ① 密码类：全遮蔽（8 个 *），保留键名
    if let Some(re) = &r.password {
        out = apply_rule(&out, |t| Some(re.replace_all(t, "${1}********").into_owned()));
    }
    // ② token 类：部分遮蔽
    if let Some(re) = &r.token {
        out = apply_rule(&out, |t| {
            Some(
                re.replace_all(t, |caps: &regex::Captures| {
                    let key = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let val = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    format!("{key}{}", mask_partial(val))
                })
                .into_owned(),
            )
        });
    }
    // ②b [lc-1599] URL 编码形态的 Trim-MC-token（`%3D` 或 `=` 分隔）
    if let Some(re) = &r.mc_token {
        out = apply_rule(&out, |t| {
            Some(
                re.replace_all(t, |caps: &regex::Captures| {
                    let key = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let val = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    format!("{key}{}", mask_partial(val))
                })
                .into_owned(),
            )
        });
    }
    // ③ URL 敏感参数：值部分遮蔽（键名与 "=" 在 $1 里，保留）
    if let Some(re) = &r.url_param {
        out = apply_rule(&out, |t| {
            Some(
                re.replace_all(t, |caps: &regex::Captures| {
                    let key = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let val = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    format!("{key}{}", mask_partial(val))
                })
                .into_owned(),
            )
        });
    }
    // ④ 手机号：前 3 后 4 中间 *（正则已保证 11 位 ASCII 数字）
    if let Some(re) = &r.phone {
        out = apply_rule(&out, |t| {
            Some(
                re.replace_all(t, |caps: &regex::Captures| {
                    let v = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let head: String = v.chars().take(3).collect();
                    let tail: String =
                        v.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
                    format!("{head}****{tail}")
                })
                .into_owned(),
            )
        });
    }
    // ⑤ 邮箱：局部名只留前 2 字符（≤2 字符全 *），域名保留
    if let Some(re) = &r.email {
        out = apply_rule(&out, |t| {
            Some(
                re.replace_all(t, |caps: &regex::Captures| {
                    let full = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    match full.split_once('@') {
                        Some((local, domain)) => {
                            let n = local.chars().count();
                            let masked = if n <= 2 {
                                "*".repeat(n)
                            } else {
                                format!("{}***", local.chars().take(2).collect::<String>())
                            };
                            format!("{masked}@{domain}")
                        }
                        None => full.to_string(),
                    }
                })
                .into_owned(),
            )
        });
    }
    // ⑥ 本机用户名路径（Windows 盘符 + Unix /home|/Users）
    if let Some(re) = &r.win_path {
        out = apply_rule(&out, |t| Some(re.replace_all(t, "${1}<USER>").into_owned()));
    }
    if let Some(re) = &r.nix_path {
        out = apply_rule(&out, |t| Some(re.replace_all(t, "${1}<USER>").into_owned()));
    }
    out
}

// ───────────────────────── 命令 ─────────────────────────

/// 统一提交入口：POST /feedback → 返回工单号（feedback_submit / 手动日志上传共用）。
async fn post_feedback(body: &Value) -> Result<String, String> {
    let v = send_json(http().post(format!("{API_BASE}/feedback")).json(body)).await?;
    v.get("id")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "反馈服务未返回工单号".to_string())
}

/// 提交反馈。成功后写本地索引并返回工单号。
/// include_log=true 时附带日志尾部（已脱敏 + 环境块）；文件不存在/为空则不带 log 字段。
#[tauri::command]
pub async fn feedback_submit(
    app: AppHandle,
    state: State<'_, AppState>,
    message: String,
    contact: Option<String>,
    include_log: bool,
    page: Option<String>,
) -> Result<Value, String> {
    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("反馈内容不能为空".into());
    }
    let aid = ensure_aid(&app, &state).await?;

    let mut log_field: Option<String> = None;
    if include_log {
        match read_log_tail() {
            Some(raw) => {
                log_field = Some(format!("{}\n{}", env_block(page.as_deref()), redact(&raw)));
            }
            None => libmpv::trace("[feedback] 日志为空/不存在，跳过附件"),
        }
    }

    let mut body = json!({
        "message": message,
        "aid": aid,
        "v": env!("CARGO_PKG_VERSION"),
        "os": os_label(),
        "arch": arch_label(),
    });
    if let Some(c) = contact.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        body["contact"] = json!(c);
    }
    if let Some(p) = page.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        body["page"] = json!(p);
    }
    if let Some(l) = &log_field {
        body["log"] = json!(l);
    }

    let id = post_feedback(&body).await?;

    // 本地索引：前插 + 截断（写失败不挡提交 —— 工单已在服务端建立，只是本地列表少一条）
    let mut list = load_index(&app).await;
    list.insert(
        0,
        TicketMeta {
            id: id.clone(),
            ts: now_secs(),
            title: make_title(&message),
        },
    );
    list.truncate(INDEX_MAX);
    if let Err(e) = save_index(&app, &list).await {
        libmpv::trace(&format!("[feedback] 本地索引写入失败（不影响工单）: {e}"));
    }
    libmpv::trace(&format!(
        "[feedback] 工单已提交 id={id} 带日志={}",
        log_field.is_some()
    ));

    Ok(json!({ "id": id }))
}

/// 本地工单列表 + 服务端状态合并。
/// 服务端 /feedback/mine 单次上限 20 个 id ⇒ 只同步**最近 20 条**，更早的保持 status="unknown"；
/// 网络失败整体降级（全部 unknown），**不报错**（列表永远可用）。
#[tauri::command]
pub async fn feedback_list(app: AppHandle) -> Result<Value, String> {
    let local = load_index(&app).await;
    if local.is_empty() {
        return Ok(json!([]));
    }

    let ids: Vec<String> = local
        .iter()
        .take(MINE_BATCH_MAX)
        .map(|t| t.id.clone())
        .collect();
    let mut remote: HashMap<String, Value> = HashMap::new();
    if let Ok(v) = fetch_mine(&ids).await {
        if let Some(items) = v.get("items").and_then(|x| x.as_array()) {
            for it in items {
                if let Some(id) = it.get("id").and_then(|x| x.as_str()) {
                    remote.insert(id.to_string(), it.clone());
                }
            }
        }
    }

    let items: Vec<Value> = local
        .iter()
        .enumerate()
        .map(|(i, t)| merge_item(t, if i < MINE_BATCH_MAX { remote.get(&t.id) } else { None }))
        .collect();
    Ok(json!(items))
}

async fn fetch_mine(ids: &[String]) -> Result<Value, String> {
    let q: Vec<String> = ids
        .iter()
        .map(|s| urlencoding::encode(s).into_owned())
        .collect();
    send_json(http().get(format!("{API_BASE}/feedback/mine?ids={}", q.join(",")))).await
}

/// 合并单条：本地 id/ts/title + 远端 status/updated/lastRole/lastTs（远端缺失 → unknown/null）
fn merge_item(t: &TicketMeta, r: Option<&Value>) -> Value {
    let field = |k: &str| r.and_then(|v| v.get(k)).cloned().unwrap_or(Value::Null);
    json!({
        "id": t.id,
        "ts": t.ts,
        "title": t.title,
        "status": r.and_then(|v| v.get("status")).and_then(|x| x.as_str()).unwrap_or("unknown"),
        "updated": field("updated"),
        "lastRole": field("lastRole"),
        "lastTs": field("lastTs"),
    })
}

/// 工单会话：服务端响应原样透传（{ ok, ticket, messages }）。
#[tauri::command]
pub async fn feedback_thread(id: String) -> Result<Value, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("缺少工单号".into());
    }
    send_json(http().get(format!(
        "{API_BASE}/feedback/ticket?id={}",
        urlencoding::encode(id)
    )))
    .await
}

/// 追加工单消息（用户回复）。
#[tauri::command]
pub async fn feedback_reply(id: String, text: String) -> Result<Value, String> {
    let id = id.trim().to_string();
    let text = text.trim().to_string();
    if id.is_empty() {
        return Err("缺少工单号".into());
    }
    if text.is_empty() {
        return Err("回复内容不能为空".into());
    }
    let body = json!({ "id": id, "text": text });
    send_json(http().post(format!("{API_BASE}/feedback/reply")).json(&body)).await
}

// ───────────────────────── 日志 / 诊断工具（设置面板迁移） ─────────────────────────
// 对应旧 Electron 项目：
//   · settings:open-log / open-error-log / open-mpv-log / export-log
//   · settings:diagnostics（汇总运行态 + 复制）
// [lc-1661] 新增独立报错日志 anibox-error.log + 调试/组件日志开关；
// 旧项目的「手动选择日志文件上传」已按需求移除（不再有 feedback:upload-log）。

fn file_size(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn app_data_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录：{e}"))
}

fn diagnostics_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app_data_dir(app)?.join("diagnostics"))
}

fn or_unset(s: &str) -> &str {
    if s.is_empty() { "(未设置)" } else { s }
}

fn or_saved(s: &str) -> &str {
    if s.is_empty() { "无" } else { "已保存" }
}

/// 诊断文本（不含 token/password 明文；只报告“是否已保存”）。
fn diagnostics_text(app: &AppHandle, cfg: &config::Config) -> String {
    let log = libmpv::log_path();
    let errlog = libmpv::error_log_path();
    let mpv = mpv_log_path();
    let data_dir = app_data_dir(app).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let yes = |b: bool| if b { "开" } else { "关" };
    let modules: Vec<String> = libmpv::MODULE_KEYS
        .iter()
        .map(|k| {
            format!(
                "{}={}",
                k,
                yes(cfg.log_modules.get(*k).copied().unwrap_or(true))
            )
        })
        .collect();
    let lines = [
        "== 基本信息 ==".to_string(),
        format!("版本: {}", env!("CARGO_PKG_VERSION")),
        format!("系统: {} {} ({})", os_label(), std::env::consts::OS, arch_label()),
        format!("本地时间: {}", now_local_str()),
        format!("应用数据目录: {data_dir}"),
        String::new(),
        "== 日志 ==".to_string(),
        format!("诊断日志: {}（{} 字节）", log.to_string_lossy(), file_size(&log)),
        format!("报错日志: {}（{} 字节）", errlog.to_string_lossy(), file_size(&errlog)),
        format!("MPV 日志: {}（{} 字节）", mpv.to_string_lossy(), file_size(&mpv)),
        format!("调试日志: {}", yes(cfg.log_debug)),
        format!("组件日志: {}", modules.join(" ")),
        format!(
            "匿名使用统计: {} · 上次上报: {}",
            yes(cfg.usage_stats_enabled),
            if cfg.usage_stats_last_ping_day.is_empty() {
                "未上报".to_string()
            } else {
                format!(
                    "{}（{}）",
                    cfg.usage_stats_last_ping_day,
                    if cfg.usage_stats_last_ping_ok { "成功" } else { "失败" }
                )
            }
        ),
        String::new(),
        "== 连接与账号 ==".to_string(),
        format!("NAS 地址: {}", or_unset(&cfg.server)),
        format!("账号: {}", or_unset(&cfg.username)),
        format!("Token: {}", or_saved(&cfg.token)),
        format!("记住密码: {}", or_saved(&cfg.password)),
        format!("自动登录: {}", yes(cfg.auto_login)),
        String::new(),
        "== 播放与网络 ==".to_string(),
        format!("MPV 路径: {}", or_unset(&cfg.mpv_path)),
        format!("代理路径: {}", or_unset(&cfg.proxy_path)),
        format!("NAS 本地代理: {}", yes(cfg.nas_proxy_enabled)),
        format!("缓存上限: {} MB（0 = 不限）", cfg.cache_limit_mb),
        String::new(),
        "== 其它设置 ==".to_string(),
        format!("关闭行为: {}", or_unset(&cfg.exit_mode)),
        format!("性能模式: {}", yes(cfg.perf_mode)),
        format!("轮播 Logo: {}", yes(cfg.carousel_logo)),
        format!("启动页: {}", or_unset(&cfg.launch_page)),
        format!("Bangumi 用户: {}", or_unset(&cfg.bgm_username)),
        format!("Bangumi Token: {}", or_saved(&cfg.bgm_token)),
        format!("记住 Bangumi Token: {}", yes(cfg.bgm_remember)),
        format!("自建弹幕接口: {}", yes(cfg.danmu_api_enabled)),
        format!("弹幕接口数量: {}", cfg.danmu_api_bases.len()),
    ];
    lines.join("\n")
}

/// 日志文件信息（面板展示 + 按钮可用性）。
#[tauri::command]
pub async fn feedback_log_info() -> Result<Value, String> {
    let log = libmpv::log_path();
    let errlog = libmpv::error_log_path();
    let mpv = mpv_log_path();
    let dir = log.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    Ok(json!({
        "path": log.to_string_lossy(),
        "dir": dir.to_string_lossy(),
        "size": file_size(&log),
        "exists": log.exists(),
        // [lc-1661] 独立报错日志
        "errorLogPath": errlog.to_string_lossy(),
        "errorLogSize": file_size(&errlog),
        "errorLogExists": errlog.exists(),
        "mpvPath": mpv.to_string_lossy(),
        "mpvSize": file_size(&mpv),
        "mpvExists": mpv.exists(),
        "maxBytes": LOG_TAIL_MAX_BYTES,
    }))
}

/// [lc-1661] 固定契约 JSON：{ debug, modules(全 7 键), errorLogPath }。
fn log_settings_value(debug: bool, modules: &BTreeMap<String, bool>) -> Value {
    let mut m = serde_json::Map::new();
    for k in libmpv::MODULE_KEYS {
        m.insert(k.to_string(), json!(modules.get(k).copied().unwrap_or(true)));
    }
    json!({
        "debug": debug,
        "modules": Value::Object(m),
        "errorLogPath": libmpv::error_log_path().to_string_lossy(),
    })
}

/// 读日志设置（调试开关 + 组件过滤 + 报错日志路径）。
#[tauri::command]
pub async fn feedback_log_settings_get(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await;
    Ok(log_settings_value(cfg.log_debug, &cfg.log_modules))
}

/// 写日志设置：接受 debug / modules 的任意子集，持久化到 config.json 并立即生效。
/// 未提供的键保持原值；未知组件键忽略（契约只认 7 个固定键）。
#[tauri::command]
pub async fn feedback_log_settings_set(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Value,
) -> Result<Value, String> {
    let mut cfg = state.config.read().await.clone();
    if let Some(v) = settings.get("debug") {
        cfg.log_debug = v.as_bool().ok_or("debug 必须是布尔值")?;
    }
    if let Some(v) = settings.get("modules") {
        let obj = v.as_object().ok_or("modules 必须是对象")?;
        for k in libmpv::MODULE_KEYS {
            if let Some(b) = obj.get(k) {
                let b = b
                    .as_bool()
                    .ok_or_else(|| format!("modules.{k} 必须是布尔值"))?;
                cfg.log_modules.insert(k.to_string(), b);
            }
        }
        // 归一化：缺省键补 true（config.json 可读、前端回填不丢项）
        for k in libmpv::MODULE_KEYS {
            cfg.log_modules.entry(k.to_string()).or_insert(true);
        }
    }
    config::save(&app, &cfg).await?;
    libmpv::apply_settings(cfg.log_debug, cfg.log_modules.clone());
    *state.config.write().await = cfg.clone();
    Ok(log_settings_value(cfg.log_debug, &cfg.log_modules))
}

/// 打开日志文件 / 日志目录 / 报错日志 / MPV 日志 / 诊断包目录。
/// kind: "log" | "dir" | "error" | "mpv" | "diagnostics"
#[tauri::command]
pub async fn feedback_open_log(app: AppHandle, kind: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let target: std::path::PathBuf = match kind.as_str() {
        "log" => {
            let p = libmpv::log_path();
            if !p.exists() {
                return Err("诊断日志尚未生成（本进程还没写过日志）".into());
            }
            p
        }
        "dir" => {
            let p = libmpv::log_path();
            p.parent()
                .map(|x| x.to_path_buf())
                .ok_or("无法定位日志目录")?
        }
        // [lc-1661] 独立报错日志：不存在时创建空文件再打开（用户可能想看“确实没报错”）
        "error" => {
            let p = libmpv::error_log_path();
            if !p.exists() {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&p)
                    .map_err(|e| format!("创建报错日志失败：{e}"))?;
            }
            p
        }
        "mpv" => {
            let p = mpv_log_path();
            if !p.exists() {
                return Err("MPV 日志尚未生成（请先用 MPV 播放一次）".into());
            }
            p
        }
        "diagnostics" => {
            let p = diagnostics_dir(&app)?;
            std::fs::create_dir_all(&p).map_err(|e| format!("创建诊断目录失败：{e}"))?;
            p
        }
        other => return Err(format!("未知的日志目标：{other}")),
    };
    app.opener()
        .open_path(target.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| format!("打开失败：{e}"))
}

/// 读取诊断日志尾部并脱敏，供「复制日志」使用（绝不返回明文凭据）。
#[tauri::command]
pub async fn feedback_read_log(max_bytes: Option<u64>) -> Result<Value, String> {
    let want = max_bytes.unwrap_or(LOG_TAIL_BYTES);
    match read_tail_path(&libmpv::log_path(), want) {
        Some(raw) => {
            let text = redact(&raw);
            Ok(json!({
                "text": text,
                "bytes": text.len(),
                "path": libmpv::log_path().to_string_lossy(),
            }))
        }
        None => Ok(json!({ "text": "", "bytes": 0, "path": libmpv::log_path().to_string_lossy() })),
    }
}

/// 诊断信息（面板「诊断信息」区块 + 复制）。
#[tauri::command]
pub async fn feedback_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let cfg = state.config.read().await;
    Ok(json!({ "text": diagnostics_text(&app, &cfg) }))
}

// ── 最小 ZIP（store 模式，无第三方依赖）：诊断包只需打包 2~3 个文本文件 ──

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn zip_store(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<(String, u32, u32, u32)> = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        // local file header
        out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 name
        out.extend_from_slice(&0u16.to_le_bytes()); // method = store
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0x21u16.to_le_bytes()); // mod date = 1980-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.push((name.clone(), crc, size, offset));
    }
    let cd_offset = out.len() as u32;
    for (name, crc, size, offset) in &central {
        out.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // method = store
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0x21u16.to_le_bytes()); // mod date = 1980-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out.extend_from_slice(&0u16.to_le_bytes()); // disk
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
    }
    let cd_size = out.len() as u32 - cd_offset;
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk
    out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
    out.extend_from_slice(&(central.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment
    out
}

/// 导出诊断包（zip：diagnostics.txt + log-settings.json + 脱敏 anibox.log +
/// 脱敏 anibox-error.log + 脱敏 anibox-mpv.log）。
/// 写入 app_data_dir/diagnostics/，返回 zip 路径；前端随后打开该目录。
#[tauri::command]
pub async fn feedback_export_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let cfg = state.config.read().await;
    let text = diagnostics_text(&app, &cfg);
    let log = read_tail_path(&libmpv::log_path(), LOG_TAIL_MAX_BYTES)
        .map(|s| redact(&s))
        .unwrap_or_else(|| "（日志为空或不存在）".to_string());
    // [lc-1661] 独立报错日志（同样脱敏）
    let errlog = read_tail_path(&libmpv::error_log_path(), LOG_TAIL_MAX_BYTES)
        .map(|s| redact(&s))
        .unwrap_or_else(|| "（报错日志为空或不存在）".to_string());
    let mpv = read_tail_path(&mpv_log_path(), LOG_TAIL_MAX_BYTES)
        .map(|s| redact(&s))
        .unwrap_or_else(|| "（MPV 日志为空或不存在）".to_string());
    // [lc-1662] log-settings.json 里附上匿名统计开关（排查用；不改变 get/set 的固定契约）
    let mut settings_val = log_settings_value(cfg.log_debug, &cfg.log_modules);
    if let Some(o) = settings_val.as_object_mut() {
        o.insert(
            "usageStatsEnabled".to_string(),
            json!(cfg.usage_stats_enabled),
        );
        o.insert(
            "usageStatsLastPingDay".to_string(),
            json!(cfg.usage_stats_last_ping_day),
        );
        o.insert(
            "usageStatsLastPingOk".to_string(),
            json!(cfg.usage_stats_last_ping_ok),
        );
        o.insert(
            "usageStatsPendingDays".to_string(),
            json!(cfg.usage_stats_pending_days),
        );
    }
    let settings_json = serde_json::to_string_pretty(&settings_val)
        .map_err(|e| format!("序列化日志设置失败：{e}"))?;

    let stamp = now_local_str()
        .replace('-', "")
        .replace(' ', "")
        .replace(':', "");
    let dir = diagnostics_dir(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建诊断目录失败：{e}"))?;
    let path = dir.join(format!("anibox-diagnostics-{stamp}.zip"));
    let files = vec![
        ("diagnostics.txt".to_string(), text.into_bytes()),
        ("log-settings.json".to_string(), settings_json.into_bytes()),
        ("anibox.log".to_string(), log.into_bytes()),
        ("anibox-error.log".to_string(), errlog.into_bytes()),
        ("anibox-mpv.log".to_string(), mpv.into_bytes()),
    ];
    std::fs::write(&path, zip_store(&files)).map_err(|e| format!("写入诊断包失败：{e}"))?;
    libmpv::trace(&format!("[feedback] 诊断包已导出: {}", path.to_string_lossy()));
    Ok(json!({ "path": path.to_string_lossy(), "dir": dir.to_string_lossy() }))
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_full_mask() {
        let out = redact(r#"password: "hunter2""#);
        assert!(!out.contains("hunter2"), "密码必须被全遮蔽: {out}");
        assert!(out.contains("password"), "键名保留: {out}");
        // 大小写不敏感
        let out2 = redact("PASSWORD=TopSecret99");
        assert!(!out2.contains("TopSecret99"), "大小写不敏感: {out2}");
    }

    #[test]
    fn token_partial_mask() {
        let out = redact("token: abcdefghijklmn");
        assert!(!out.contains("abcdefghijklmn"), "{out}");
        assert!(out.contains("abcd******klmn"), "留前4后4、中间*上限10: {out}");
        // 短值（≤4）更保守：全遮
        let short = redact("token: ab");
        assert!(!short.contains("ab"), "{short}");
    }

    #[test]
    fn url_param_masked() {
        let out = redact("GET https://nas.example.com:18888/x?key=abcdefgh123456&y=1");
        assert!(!out.contains("abcdefgh123456"), "{out}");
        assert!(out.contains("?key=abcd"), "键名与等号保留: {out}");
        assert!(out.contains("&y=1"), "后续参数不动: {out}");
    }

    #[test]
    fn phone_mask() {
        let out = redact("联系 13812345678 或 15987654321 都行");
        assert!(out.contains("138****5678"), "{out}");
        assert!(out.contains("159****4321"), "{out}");
        assert!(!out.contains("13812345678"), "{out}");
    }

    #[test]
    fn email_mask() {
        let out = redact("mail: zhangsan@example.com");
        assert!(out.contains("zh***@example.com"), "{out}");
        assert!(!out.contains("zhangsan@"), "{out}");
        // ≤2 字符的局部名全 *
        let short = redact("ab@x.com");
        assert!(short.contains("**@x.com"), "{short}");
    }

    #[test]
    fn user_paths_masked() {
        let out = redact(r"C:\Users\alice\AppData\Local\Temp\x.log");
        assert!(out.contains(r"C:\Users\<USER>\AppData"), "{out}");
        assert!(!out.contains("alice"), "{out}");
        let out2 = redact("/home/bob/.config/app/conf.json");
        assert!(out2.contains("/home/<USER>/.config"), "{out2}");
        assert!(!out2.contains("bob"), "{out2}");
        let out3 = redact("/Users/carol/Library/Logs");
        assert!(out3.contains("/Users/<USER>/Library"), "{out3}");
    }

    #[test]
    fn domain_and_nas_preserved() {
        let s = "GET http://192.168.31.170:18888/v/api/v1/item ok, host=stats.690075.xyz";
        assert_eq!(redact(s), s, "域名与 NAS 地址必须原样保留（排查必需）");
    }

    #[test]
    fn combined_realistic_line() {
        let line = r#"[ 1234ms] [imgproto] ok user=C:\Users\admin\Pictures 手机=13900001111 mail=some.user@example.org token=abc1234567890"#;
        let out = redact(line);
        assert!(!out.contains("admin"), "{out}");
        assert!(!out.contains("13900001111"), "{out}");
        assert!(!out.contains("some.user@example.org"), "{out}");
        assert!(!out.contains("abc1234567890"), "{out}");
    }

    #[test]
    fn no_panic_on_weird_input() {
        // 多字节字符 + 截断值：字符级处理，不允许 panic
        let _ = redact("token: 中文值abc");
        let _ = redact("password: 密码");
        let _ = redact("");
        let _ = redact("🔑token: 🎫🎫🎫🎫🎫🎫🎫🎫🎫🎫");
    }

    /// [lc-1599] 补的两条：`Bearer <JWT>` 与 URL 编码的 Trim-MC-token（原项目会漏）。
    #[test]
    fn bearer_and_mc_token_masked() {
        let out = redact("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc.def");
        assert!(!out.contains("eyJhbGciOiJIUzI1NiJ9"), "Bearer 后的 JWT 必须遮蔽: {out}");
        let out2 = redact(
            "loadfile http://127.0.0.1:22346/api/v1/playvideo/x?cookie=Trim-MC-token%3D9fd9d6a023544bbbb411baac6de1d803&z=1",
        );
        assert!(
            !out2.contains("9fd9d6a023544bbbb411baac6de1d803"),
            "URL 编码的 Trim-MC-token 必须遮蔽: {out2}"
        );
        assert!(out2.contains("&z=1"), "后续参数不动: {out2}");
    }

    #[test]
    fn log_settings_value_has_contract_shape() {
        // [lc-1661] 固定契约：debug + 全 7 键 modules + errorLogPath
        let v = log_settings_value(true, &BTreeMap::new());
        assert_eq!(v.get("debug").and_then(|x| x.as_bool()), Some(true));
        let m = v.get("modules").and_then(|x| x.as_object()).expect("modules");
        for k in libmpv::MODULE_KEYS {
            assert_eq!(m.get(k).and_then(|x| x.as_bool()), Some(true), "缺省全开: {k}");
        }
        assert!(v.get("errorLogPath").and_then(|x| x.as_str()).is_some());
        assert!(v
            .get("errorLogPath")
            .and_then(|x| x.as_str())
            .unwrap()
            .ends_with("anibox-error.log"));

        // 显式关闭的组件如实反映
        let mut mods = BTreeMap::new();
        mods.insert("mpv".to_string(), false);
        let v2 = log_settings_value(false, &mods);
        assert_eq!(v2.get("debug").and_then(|x| x.as_bool()), Some(false));
        assert_eq!(
            v2["modules"]["mpv"].as_bool(),
            Some(false),
            "组件开关回读"
        );
        assert_eq!(v2["modules"]["bgm"].as_bool(), Some(true), "未设置的键全开");
    }

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn zip_store_has_signatures() {
        let z = zip_store(&[("a.txt".to_string(), b"hello".to_vec())]);
        assert_eq!(&z[0..4], b"PK\x03\x04", "local file header");
        assert_eq!(&z[z.len() - 22..z.len() - 18], b"PK\x05\x06", "EOCD");
        assert!(z.windows(5).any(|w| w == b"a.txt"), "文件名写入");
    }

    #[test]
    fn diagnostics_zip_contains_required_entries() {
        // [lc-1661/1662] 诊断包固定清单
        let files = vec![
            ("diagnostics.txt".to_string(), b"d".to_vec()),
            ("log-settings.json".to_string(), b"{}".to_vec()),
            ("anibox.log".to_string(), b"l".to_vec()),
            ("anibox-error.log".to_string(), b"e".to_vec()),
            ("anibox-mpv.log".to_string(), b"m".to_vec()),
        ];
        let z = zip_store(&files);
        for (name, _) in &files {
            assert!(
                z.windows(name.len()).any(|w| w == name.as_bytes()),
                "缺少 {name}"
            );
        }
    }
}
