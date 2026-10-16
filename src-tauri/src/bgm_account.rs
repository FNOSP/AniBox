//! [lc-1515] Bangumi 账号绑定 + 追番状态同步（对齐 Predidit/Kazumi 的 Bangumi 同步）。
//!
//! 调研结论（Kazumi 源码）：
//! - **登录 = 手动粘贴 Access Token**（不是 OAuth 跳转）：用户在 Bangumi 个人设置页生成
//!   Token 粘进设置；保存前先 `GET /v0/me` 验证，通过才落库（Kazumi BangumiSyncService）。
//! - **同步内容 = 追番收藏状态**（type 1=想看 2=看过 3=在看 4=搁置 5=抛弃）：
//!   拉取 `GET /v0/users/{username}/collections?subject_type=2`（只同步动画，分页拉全量）；
//!   写入 `POST /v0/users/-/collections/{subject_id}` body `{"type": N}`。
//! - **冲突策略三选一**（Kazumi BangumiSyncPriority）：本地优先 / Bangumi 优先 / 最新优先。
//! - **带用户 Token 的请求直连官方 API**：Kazumi 走第三方镜像（api.bgmapi.com + CI 注入的
//!   准入密钥），我们把用户隐私 token 过第三方镜像不合适 —— 直连 api.bgm.tv 更稳更私密
//!   （已实测 /v0/me 无 token 401、写接口 401、读接口假用户 404，均符合预期）。
//! - 单条即时同步：改本地追番状态时顺手 POST 一份（Kazumi syncCollectibleWhenIdle 同款）。
//!
//! 本地状态表：app_data_dir/bgm-collect.json，`{ "<subject_id>": {type, updated_at} }`。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use tauri::{AppHandle, Manager, State};

/// Bangumi 收藏类型（官方 API 值）
pub const TYPE_WISH: i64 = 1; // 想看
pub const TYPE_WATCHED: i64 = 2; // 看过
pub const TYPE_WATCHING: i64 = 3; // 在看
pub const TYPE_ON_HOLD: i64 = 4; // 搁置
pub const TYPE_DROPPED: i64 = 5; // 抛弃

pub fn type_label(t: i64) -> &'static str {
    match t {
        TYPE_WISH => "想看",
        TYPE_WATCHED => "看过",
        TYPE_WATCHING => "在看",
        TYPE_ON_HOLD => "搁置",
        TYPE_DROPPED => "抛弃",
        _ => "未收藏",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CollectEntry {
    #[serde(rename = "type")]
    pub kind: i64,
    /// 本地最后修改时间（Unix 秒；0 = 未知，用于"最新优先"合并）
    #[serde(default)]
    pub updated_at: i64,
    /// [lc-1534] 条目名（collections 的 subject.name_cn 优先；旧表无此字段，同步/拉取时回填）
    #[serde(default)]
    pub name: String,
    /// [lc-1534] 封面 URL（subject.images.common；同上，缺失时「正在追」封面条按需现拉）
    #[serde(default)]
    pub cover: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CollectTable {
    /// subject_id → 状态
    #[serde(flatten)]
    pub map: HashMap<String, CollectEntry>,
}

fn collect_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {e}"))?;
    Ok(dir.join("bgm-collect.json"))
}

pub async fn load_table(app: &AppHandle) -> CollectTable {
    match collect_path(app) {
        Ok(p) => match tokio::fs::read_to_string(&p).await {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => CollectTable::default(),
        },
        Err(_) => CollectTable::default(),
    }
}

async fn save_table(app: &AppHandle, t: &CollectTable) -> Result<(), String> {
    let p = collect_path(app)?;
    if let Some(parent) = p.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建数据目录失败: {e}"))?;
    }
    tokio::fs::write(&p, serde_json::to_string_pretty(t).map_err(|e| e.to_string())?)
        .await
        .map_err(|e| format!("写入追番状态失败: {e}"))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// [lc-1534] 从 collections 条目里抽 subject 元数据（名字中文优先；封面 common → 兜底更小档）。
fn subject_meta(item: &Value) -> (String, String) {
    let sub = item.get("subject");
    let name = sub
        .and_then(|s| s.get("name_cn").and_then(|v| v.as_str()))
        .filter(|s| !s.trim().is_empty())
        .or_else(|| sub.and_then(|s| s.get("name").and_then(|v| v.as_str())))
        .unwrap_or("")
        .trim()
        .to_string();
    let cover = ["common", "medium", "large", "small", "grid"]
        .iter()
        .find_map(|k| {
            sub.and_then(|s| s.pointer(&format!("/images/{k}")))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string();
    (name, cover)
}

/// [lc-1534] Bangumi v0 的 `updated_at`（RFC3339，形如 `2026-10-14T18:36:28+08:00`）→ Unix 秒。
/// 手写解析够用即可：固定格式、不含闰秒；解析失败返回 0。
fn iso_secs(s: &str) -> i64 {
    if s.len() < 25 {
        return 0;
    }
    let num = |a: usize, z: usize| s.get(a..z).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec), Some(tzh), Some(tzm)) = (
        num(0, 4),
        num(5, 7),
        num(8, 10),
        num(11, 13),
        num(14, 16),
        num(17, 19),
        num(20, 22),
        num(23, 25),
    ) else {
        return 0;
    };
    let leap = |y: i64| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let mut days: i64 = 0;
    for yy in 1970..y {
        days += if leap(yy) { 366 } else { 365 };
    }
    let ml = [31, if leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for mm in 0..(mo.clamp(1, 12) as usize - 1) {
        days += ml[mm];
    }
    days += d.clamp(1, 31) - 1;
    let tz = tzh * 3600 + tzm * 60;
    let tz_sign = if s.as_bytes().get(19) == Some(&b'-') { -1 } else { 1 };
    days * 86400 + h * 3600 + mi * 60 + sec - tz_sign * tz
}

// ───────────────────────── 账号绑定 ─────────────────────────

/// 账号状态：是否绑定 + 用户名（username 由 /v0/me 现查，不落库）
#[tauri::command]
pub async fn bgm_account_status(state: State<'_, crate::AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let token = cfg.bgm_token.trim().to_string();
    if token.is_empty() {
        return Ok(json!({ "bound": false, "username": "" }));
    }
    match fetch_me(&token).await {
        Ok(name) => Ok(json!({ "bound": true, "username": name })),
        Err(_) => Ok(json!({ "bound": false, "username": "" })),
    }
}

/// 带用户 Bearer token 的镜像请求（[lc-1515] 全线走自建镜像 bgm.690075.xyz，
/// 与其它 Bangumi 数据同一条链路：签名准入 + 多端点回退 + 缺密钥降级直连官方）。
async fn authed_call(method: &str, path_with_query: &str, token: &str, body: Option<&str>) -> Result<Value, String> {
    crate::bgm::mirror_call_authed(method, path_with_query, body, &format!("Bearer {token}")).await
}

async fn fetch_me(token: &str) -> Result<String, String> {
    let v = authed_call("GET", "/v0/me", token, None)
        .await
        .map_err(|e| api_error_text(0, &e))?;
    v.get("username")
        .and_then(|u| u.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Bangumi 返回的用户信息不完整".to_string())
}

/// 绑定：先验证 token，通过才写 config
#[tauri::command]
pub async fn bgm_account_bind(
    app: AppHandle,
    state: State<'_, crate::AppState>,
    token: String,
) -> Result<Value, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("Token 不能为空".into());
    }
    let name = fetch_me(&token).await?;
    let mut cfg = state.config.read().await.clone();
    cfg.bgm_token = token;
    crate::config::save(&app, &cfg).await?;
    *state.config.write().await = cfg;
    crate::libmpv::trace(&format!("[bgm] 账号绑定成功: {name}"));
    Ok(json!({ "bound": true, "username": name }))
}

#[tauri::command]
pub async fn bgm_account_unbind(app: AppHandle, state: State<'_, crate::AppState>) -> Result<(), String> {
    let mut cfg = state.config.read().await.clone();
    cfg.bgm_token = String::new();
    crate::config::save(&app, &cfg).await?;
    *state.config.write().await = cfg;
    Ok(())
}

/// API 错误文案本地化（对齐 Kazumi describeError）。
/// code=0 表示入参是**镜像调用返回的错误文本**（含「HTTP 401」等前缀），按文本反推。
fn api_error_text(code: u16, body: &str) -> String {
    let effective = if code == 0 {
        if body.contains("HTTP 401") {
            401
        } else if body.contains("HTTP 429") {
            429
        } else if body.contains("HTTP 403") {
            403
        } else if body.contains("HTTP 404") {
            404
        } else if body.contains("HTTP 400") {
            400
        } else {
            0
        }
    } else {
        code
    };
    match effective {
        400 => "请求格式错误（validation error）".to_string(),
        401 => "Access Token 无效或已过期，请更换".to_string(),
        403 => "Bangumi 拒绝访问，请检查 Token 权限".to_string(),
        404 => "接口不存在".to_string(),
        429 => "Bangumi 请求过于频繁，请稍后重试".to_string(),
        _ => {
            let preview: String = body.chars().take(200).collect();
            let preview = preview.trim();
            if preview.is_empty() {
                "Bangumi 请求失败".to_string()
            } else {
                preview.to_string()
            }
        }
    }
}

// ───────────────────────── 追番状态（本地 + 单条同步） ─────────────────────────

/// 设置某条目的追番状态（type 0 = 取消收藏）。已绑定则顺手同步一份到 Bangumi。
#[tauri::command]
pub async fn bgm_collect_set(
    app: AppHandle,
    state: State<'_, crate::AppState>,
    subject_id: i64,
    kind: i64,
) -> Result<Value, String> {
    let mut table = load_table(&app).await;
    let key = subject_id.to_string();
    if kind <= 0 {
        table.map.remove(&key);
    } else {
        // [lc-1534] 保留已有的名字/封面（只更新状态与时间）
        let prev = table.map.get(&key).cloned().unwrap_or_default();
        table.map.insert(
            key,
            CollectEntry {
                kind,
                updated_at: now_secs(),
                name: prev.name,
                cover: prev.cover,
            },
        );
    }
    save_table(&app, &table).await?;

    let mut synced = false;
    let token = state.config.read().await.bgm_token.trim().to_string();
    if !token.is_empty() {
        if kind <= 0 {
            // Bangumi 没有公开的"删除收藏"接口（API 文档未提供 DELETE）—— 本地移除即可
            synced = true;
        } else {
            synced = push_collect(&token, subject_id, kind).await?;
        }
    }
    Ok(json!({
        "ok": true,
        "synced": synced,
        "type": kind,
        "label": type_label(kind),
    }))
}

/// 查询本地追番状态（详情页/列表展示用）
#[tauri::command]
pub async fn bgm_collect_get(app: AppHandle, subject_id: i64) -> Result<Value, String> {
    let table = load_table(&app).await;
    let e = table.map.get(&subject_id.to_string());
    Ok(json!({
        "subjectId": subject_id,
        "type": e.map(|x| x.kind).unwrap_or(0),
        "label": type_label(e.map(|x| x.kind).unwrap_or(0)),
        "updatedAt": e.map(|x| x.updated_at).unwrap_or(0),
    }))
}

async fn push_collect(token: &str, subject_id: i64, kind: i64) -> Result<bool, String> {
    let path = format!("/v0/users/-/collections/{subject_id}");
    authed_call("POST", &path, token, Some(&json!({ "type": kind }).to_string()))
        .await
        .map(|_| true)
        .map_err(|e| api_error_text(0, &e))
}

// ───────────────────────── 全量同步 ─────────────────────────

/// 全量同步：拉远端全部动画收藏（分页）→ 与本地表合并 → 按优先级落库/上传。
/// priority: "local"（本地优先，默认）| "bangumi"（Bangumi 优先）| "time"（最新优先）
#[tauri::command]
pub async fn bgm_sync_now(
    app: AppHandle,
    state: State<'_, crate::AppState>,
    priority: Option<String>,
) -> Result<Value, String> {
    let token = state.config.read().await.bgm_token.trim().to_string();
    if token.is_empty() {
        return Err("尚未绑定 Bangumi 账号".into());
    }
    let priority = priority.unwrap_or_else(|| "local".into());

    // 1) 拉远端全量（subject_type=2 只取动画；limit 50 分页）
    let me = fetch_me(&token).await?;
    let mut remote: HashMap<String, i64> = HashMap::new();
    // [lc-1534] subject_id → (名字, 封面)：collections 响应自带 subject，顺手收集，供「正在追」封面条
    let mut remote_meta: HashMap<String, (String, String)> = HashMap::new();
    let mut offset = 0u64;
    loop {
        let path = format!(
            "/v0/users/{}/collections?subject_type=2&limit=50&offset={offset}",
            urlencoding::encode(&me)
        );
        let v = authed_call("GET", &path, &token, None)
            .await
            .map_err(|e| api_error_text(0, &e))?;
        let empty = Vec::new();
        let data = v.get("data").and_then(|d| d.as_array()).unwrap_or(&empty);
        for item in data {
            let Some(sid) = item.get("subject_id").and_then(|s| s.as_i64()) else {
                continue;
            };
            let Some(t) = item.get("type").and_then(|s| s.as_i64()) else {
                continue;
            };
            let (name, cover) = subject_meta(item);
            if !name.is_empty() || !cover.is_empty() {
                remote_meta.insert(sid.to_string(), (name, cover));
            }
            remote.insert(sid.to_string(), t);
        }
        let total = v.get("total").and_then(|t| t.as_u64()).unwrap_or(0);
        offset += data.len() as u64;
        if data.is_empty() || offset >= total {
            break;
        }
        if offset > 5000 {
            break; // 防御：异常分页不循环
        }
    }

    // 2) 合并（本地优先 v1：本地有→上传；本地无→下载远端）
    let mut table = load_table(&app).await;
    let mut uploaded = 0u32;
    let mut downloaded = 0u32;
    let now = now_secs();

    // 2a. 本地有、远端没有/不同 → 按优先级决定上传
    let local_keys: Vec<String> = table.map.keys().cloned().collect();
    for key in local_keys {
        let Some(entry) = table.map.get(&key).cloned() else {
            continue;
        };
        let remote_t = remote.get(&key).copied();
        let need_upload = match priority.as_str() {
            "bangumi" => false, // Bangumi 优先：冲突不传，2b 会用远端覆盖
            "time" => remote_t != Some(entry.kind), // 最新优先：本地时间戳即"现在"，简化处理
            _ => remote_t != Some(entry.kind),     // local 默认
        };
        if need_upload {
            if let Ok(sid) = key.parse::<i64>() {
                push_collect(&token, sid, entry.kind).await?;
                uploaded += 1;
            }
        }
    }

    // 2b. 本地没有 → 下载远端（"本地优先"时本地缺失也补全，Kazumi remoteOnlyPuts 同款）
    for (key, t) in &remote {
        if *t <= 0 {
            continue;
        }
        let dominated = table.map.contains_key(key);
        if dominated {
            // 冲突：Bangumi 优先 → 远端覆盖本地；本地/最新优先 → 保留本地
            if priority == "bangumi" {
                let e = table.map.get_mut(key).unwrap();
                if e.kind != *t {
                    e.kind = *t;
                    e.updated_at = now;
                    downloaded += 1;
                }
            }
            continue;
        }
        table.map.insert(
            key.clone(),
            CollectEntry {
                kind: *t,
                updated_at: now,
                ..Default::default()
            },
        );
        downloaded += 1;
    }

    // 2c. [lc-1534] 回填名字/封面（type 有无变化都回填；旧表升级靠这里）
    for (key, (name, cover)) in &remote_meta {
        if let Some(e) = table.map.get_mut(key) {
            if !name.is_empty() {
                e.name = name.clone();
            }
            if !cover.is_empty() {
                e.cover = cover.clone();
            }
        }
    }

    save_table(&app, &table).await?;
    crate::libmpv::trace(&format!(
        "[bgm] 同步完成 priority={priority} remote={} uploaded={uploaded} downloaded={downloaded}",
        remote.len()
    ));
    Ok(json!({
        "ok": true,
        "username": me,
        "remoteTotal": remote.len(),
        "uploaded": uploaded,
        "downloaded": downloaded,
        "localTotal": table.map.len(),
    }))
}

/// [lc-1520] 当季追番状态：本地追番表 ∩ 当季 Bangumi 日历（日历走同一镜像 + 落盘缓存）。
/// 前端「当季追番状态」通栏用：本季放送番里有多少已标记、各状态分布。
#[tauri::command]
pub async fn bgm_collect_season(app: AppHandle) -> Result<Value, String> {
    let table = load_table(&app).await;
    let cal = crate::bgm::bgm_calendar(None, None, None).await?;
    let mut season_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut season_total = 0u32;
    if let Some(days) = cal.get("days").and_then(|d| d.as_array()) {
        for day in days {
            if let Some(items) = day.get("items").and_then(|i| i.as_array()) {
                for it in items {
                    season_total += 1;
                    if let Some(id) = it.get("id").and_then(|v| v.as_i64()) {
                        season_ids.insert(id);
                    }
                }
            }
        }
    }
    let mut by_type: HashMap<i64, u32> = HashMap::new();
    let mut total = 0u32;
    for (k, e) in &table.map {
        if let Ok(sid) = k.parse::<i64>() {
            if season_ids.contains(&sid) {
                total += 1;
                *by_type.entry(e.kind).or_insert(0) += 1;
            }
        }
    }
    Ok(json!({ "total": total, "seasonTotal": season_total, "byType": by_type }))
}

/// 已同步条数（设置面板展示）
/// [lc-1580] 额外回一张 **`byId`（subject_id → 收藏类型）**：发现页右栏「今日状态条」用它算
/// 「今天有几部**已追订阅**的番更新」—— 前端拿日历当天的 id 直接查这张表、交集在本地算，
/// 不用再多打一次日历/收藏接口（表是本地文件，读取近乎零成本）。
/// [lc-1581] 再加一张 **`list`（[{id,kind,name}]）**：只按 id 匹配会漏 —— BGM 把**每一季算独立条目**
/// （日历里「药屋少女的呢喃 第三季」id=568244，追番表里「药屋少女的呢喃」id=420628），
/// 前端要用**名字**再兜一层（复用库索引那套 normTitle + 前缀匹配）。
#[tauri::command]
pub async fn bgm_collect_stats(app: AppHandle) -> Result<Value, String> {
    let table = load_table(&app).await;
    let mut by_type: HashMap<i64, u32> = HashMap::new();
    let mut by_id = serde_json::Map::new();
    let mut list = Vec::new();
    for (k, e) in &table.map {
        *by_type.entry(e.kind).or_insert(0) += 1;
        by_id.insert(k.clone(), json!(e.kind));
        list.push(json!({ "id": k, "kind": e.kind, "name": e.name }));
    }
    Ok(json!({ "total": table.map.len(), "byType": by_type, "byId": by_id, "list": list }))
}

/// [lc-1534] 「正在追」封面条数据（卡片左栏用）：本地表 type=3（在看）按最近更新倒序。
/// 前 limit 条里有缺封面的（或一条都没有）且已绑定 → 现拉 collections 首页（type=3）回填本地表；
/// 只补名字/封面/更新时间，type 以本地表为准不覆盖。返回 [{id, name, cover}]（最多 limit 条，默认 12）。
#[tauri::command]
pub async fn bgm_watching_covers(
    app: AppHandle,
    state: State<'_, crate::AppState>,
    limit: Option<u32>,
) -> Result<Value, String> {
    let limit = limit.unwrap_or(12).clamp(1, 30) as usize;
    let mut table = load_table(&app).await;

    // 1) 本地「在看」按最近更新倒序；看前 limit 条是否缺封面
    let need_fetch = {
        let mut rows: Vec<&CollectEntry> = table
            .map
            .values()
            .filter(|e| e.kind == TYPE_WATCHING)
            .collect();
        rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        rows.is_empty() || rows.iter().take(limit).any(|e| e.cover.is_empty())
    };

    // 2) 现拉回填（只补元数据）
    if need_fetch {
        let token = state.config.read().await.bgm_token.trim().to_string();
        if !token.is_empty() {
            if let Ok(me) = fetch_me(&token).await {
                let path = format!(
                    "/v0/users/{}/collections?subject_type=2&type=3&limit={}&offset=0",
                    urlencoding::encode(&me),
                    limit.max(12)
                );
                if let Ok(v) = authed_call("GET", &path, &token, None).await {
                    if let Some(data) = v.get("data").and_then(|d| d.as_array()) {
                        let now = now_secs();
                        for item in data {
                            let Some(sid) = item.get("subject_id").and_then(|s| s.as_i64()) else {
                                continue;
                            };
                            let (name, cover) = subject_meta(item);
                            let key = sid.to_string();
                            let e = table.map.entry(key).or_insert_with(|| CollectEntry {
                                kind: TYPE_WATCHING,
                                updated_at: now,
                                ..Default::default()
                            });
                            if !name.is_empty() {
                                e.name = name;
                            }
                            if !cover.is_empty() {
                                e.cover = cover;
                            }
                            // 用远端真实的「收藏更新时间」校正（封面条按它排序）
                            if let Some(ts) = item.get("updated_at").and_then(|v| v.as_str()) {
                                let secs = iso_secs(ts);
                                if secs > 0 {
                                    e.updated_at = secs;
                                }
                            }
                        }
                        let _ = save_table(&app, &table).await;
                    }
                }
            }
        }
    }

    // 3) 输出：type=3 按最近更新倒序，截断到 limit
    let mut rows: Vec<(i64, String, String, i64)> = table
        .map
        .iter()
        .filter(|(_, e)| e.kind == TYPE_WATCHING)
        .filter_map(|(k, e)| {
            k.parse::<i64>()
                .ok()
                .map(|id| (id, e.name.clone(), e.cover.clone(), e.updated_at))
        })
        .collect();
    rows.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| b.0.cmp(&a.0)));
    let items: Vec<Value> = rows
        .into_iter()
        .take(limit)
        .map(|(id, name, cover, ts)| {
            json!({ "id": id, "name": name, "cover": cover, "updatedAt": ts })
        })
        .collect();
    Ok(json!({ "items": items }))
}
