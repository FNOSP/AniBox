//! AniBox —— 自绘 UI + 直调飞牛 API + mpv 双层窗口内嵌。
//! 命令面向前端：配置 / 登录 / 媒体库 / 播放控制。

mod authx;
mod bgm;
mod bgm_account;
mod config;
mod danmaku_shim;
mod disk_cache;
mod fnapi;
mod libmpv;
mod imgproto;
mod migrate;
mod mirror_sign;
mod mpv;
mod proxy;
mod tmdb;
mod webrs;

/// [lc-1417] 详情页数据的落盘 TTL：**120 天**（用户指定）。
/// 覆盖：飞牛条目详情 / 季列表 / 集列表 / TMDB 详情。
/// 为什么给这么长：这些是**元数据**（简介、剧照、演职员、集号…），一经刮削基本不变；
/// 用户明确要求"数据拉了直接本地落盘不要老是拉取"。过期也不清空 —— 命中逻辑同 disk_cache
/// （TTL 内直出、过期则回源，回源失败仍可用旧值兜底，见 json_cached_conditional）。
/// 想立刻更新：设置面板的「清理缓存」会把它们一起清掉，下次进详情页重新拉。
const DETAIL_TTL_SECS: u64 = 120 * 24 * 3600;
/// [lc-1578] **季/集列表**的落盘 TTL —— 与上面的 item 详情**刻意分开**：
///   · item 详情是重请求（简介/海报/演职/评分全在里面），值得 120 天长缓存；
///   · 季/集列表**随"新集入库"变化**，长缓存必然显示旧集数。事故：`detail:episodes` 沿用
///     120 天 TTL ⇒ 用户在飞牛里补了第 3 集，App 仍返回缓存里的 2 集（实测缓存 17:04 写入、
///     23:25 仍被命中返回），用户报「飞牛里三集、软件只显示两集」。
/// 用户定的节奏：**10 分钟一刷** —— 10 分钟内的来回切页仍走缓存（吸收 React 双挂载/反复进退），
/// 新集最多滞后 10 分钟。
const DETAIL_LIST_TTL_SECS: u64 = 10 * 60;

use config::Config;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{Mutex, RwLock};

pub struct AppState {
    pub config: RwLock<Config>,
    pub mpv: Mutex<Option<Arc<mpv::MpvSession>>>,
    pub proxy_child: Mutex<Option<std::process::Child>>,
    /// [lc-1560] 弹幕 shim 子进程（node 跑 danmaku-shim.js，供 uosc_danmaku 调 22347）
    pub danmaku_child: Mutex<Option<std::process::Child>>,
    /// 最近一次前端主题（player_stop 后恢复 SWCA 染色用）
    pub last_light: std::sync::Mutex<Option<bool>>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            config: RwLock::new(Config::default()),
            mpv: Mutex::new(None),
            proxy_child: Mutex::new(None),
            danmaku_child: Mutex::new(None),
            last_light: std::sync::Mutex::new(None),
        }
    }
}

// ───────────────────────── 配置 ─────────────────────────

#[tauri::command]
async fn get_config(state: State<'_, AppState>) -> Result<Config, String> {
    Ok(state.config.read().await.clone())
}

#[tauri::command]
async fn save_config(app: AppHandle, state: State<'_, AppState>, config: Config) -> Result<(), String> {
    config::save(&app, &config).await?;
    // [lc-1561] 自建弹幕接口配置桥给 Node 侧客户端（改完立即生效，见 write_danmu_config）
    danmaku_shim::write_danmu_config(&config)?;
    // [lc-1561] 同步 Lua 侧闸门（只写开关、绝不写地址，见 sync_lua_gate 注释）
    danmaku_shim::sync_lua_gate(
        &config.mpv_path,
        config.danmu_api_enabled,
        &config.danmu_api_base,
    );
    *state.config.write().await = config;
    Ok(())
}

/// [lc-1561] 测试某个自建弹幕接口地址是否可达（设置面板「测试连接」）。
///
/// 用与客户端相同的口径：请求 `/api/v2/search/anime?keyword=…` 只看**能否连通 + 是否返回
/// JSON**（不校验搜索结果，那是业务）。返回人话结论，对齐主仓库 testConnection 的提示风格。
#[tauri::command]
async fn test_danmu_api(url: String) -> Result<String, String> {
    let base = url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("请先填写服务地址".into());
    }
    if !base.starts_with("http://") && !base.starts_with("https://") {
        return Err("地址需以 http:// 或 https:// 开头".into());
    }
    let api = format!("{base}/api/v2/search/anime?keyword=%E6%B5%8B%E8%AF%95");
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()
    {
        Ok(c) => c,
        Err(e) => return Err(format!("构造请求失败: {e}")),
    };
    match client.get(&api).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Ok(format!("❌ 服务返回 {status} —— 地址可达，但路径或 TOKEN 可能不对"));
            }
            if body.trim_start().starts_with('{') || body.trim_start().starts_with('[') {
                Ok("✅ 连接成功（服务返回了 JSON）".into())
            } else {
                Ok("⚠️ 地址可达，但返回的不是 JSON —— 确认填的是 danmu_api 服务地址（通常带 TOKEN 路径）".into())
            }
        }
        Err(e) => {
            let hint = if e.is_timeout() {
                "连接超时：服务未响应（确认容器在跑、端口正确）"
            } else if e.is_connect() {
                "连接被拒绝：服务未启动或端口不通"
            } else {
                "请求失败"
            };
            Ok(format!("❌ {hint} —— {e}"))
        }
    }
}

// ───────────────────────── 登录 / 会话 ─────────────────────────

#[tauri::command]
async fn login(
    app: AppHandle,
    state: State<'_, AppState>,
    server: String,
    username: String,
    password: String,
) -> Result<Value, String> {
    let client = fnapi::new_client();
    let server = fnapi::normalize_server(&server);
    let data = client
        .call(
            &server,
            "/v/api/v1/login",
            "POST",
            "",
            Some(json!({ "app_name": "trimemedia-web", "username": username, "password": password })),
        )
        .await?;
    let token = data
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "登录响应缺少 token".to_string())?
        .to_string();

    // 固化真实基址：80 端口会把 API 302 到实际端口（如 :17777），图片/播放都依赖它
    let server = client.resolve_final_base(&server, &token).await;

    let mut cfg = state.config.read().await.clone();
    cfg.server = server;
    cfg.username = username;
    cfg.token = token;
    config::save(&app, &cfg).await?;
    *state.config.write().await = cfg.clone();
    Ok(json!({ "ok": true }))
}

#[tauri::command]
async fn logout(state: State<'_, AppState>) -> Result<(), String> {
    let mut cfg = state.config.write().await;
    cfg.token = String::new();
    Ok(())
}

#[tauri::command]
async fn check_session(state: State<'_, AppState>, app: AppHandle) -> Result<Value, String> {
    let mut cfg = state.config.read().await.clone();
    if cfg.token.is_empty() || cfg.server.is_empty() {
        return Err("未登录".into());
    }
    let client = fnapi::new_client();
    let data = client
        .call(&cfg.server, "/v/api/v1/user/info", "GET", &cfg.token, None)
        .await?;
    // 会话有效时顺手把重定向后的真实基址固化（老配置可能还停在 80 端口）
    let final_base = client.resolve_final_base(&cfg.server, &cfg.token).await;
    if final_base != cfg.server {
        cfg.server = final_base;
        let _ = config::save(&app, &cfg);
        *state.config.write().await = cfg;
    }
    Ok(data)
}

// ───────────────────────── 媒体库 / 条目 ─────────────────────────

#[tauri::command]
async fn get_media_libs(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(&cfg.server, "/v/api/v1/mediadb/list", "GET", &cfg.token, None)
        .await
}

/// 条目分页查询（形状经测试 NAS 实测 + FlyNarwhal 源码互证）：
/// - 库根浏览：只传 ancestor_guid（parent_guid 传库 guid 服务端返回 0 条）；
///   实测同库白名单 151 条 vs 无过滤 2635 条剧集混排 —— Episode 全靠白名单滤掉，
///   tags.type 白名单不含 Season/Episode（对齐 FlyNarwhal libraryBrowseValues）。
/// - 文件夹钻取（未识别 Directory/fv_*）：只传 parent_guid，exclude_grouped_video=0
///   （FlyNarwhal 注释：钻取带 1 会让原始视频文件夹返回空），sort_title ASC 对齐网页。
#[tauri::command]
async fn get_items(
    state: State<'_, AppState>,
    ancestor_guid: Option<String>,
    parent_guid: Option<String>,
    page: Option<i64>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let browse_types = json!(["Movie", "TV", "Directory", "Video", "LiveChannel"]);
    let body = if let Some(parent) = parent_guid {
        json!({
            "parent_guid": parent,
            "exclude_grouped_video": 0,
            "sort_type": "ASC",
            "sort_column": "sort_title",
            "page_size": 60,
            "page": page.unwrap_or(1),
            "tags": { "type": browse_types }
        })
    } else {
        json!({
            "ancestor_guid": ancestor_guid,
            "exclude_grouped_video": 1,
            "sort_type": "DESC",
            "sort_column": "create_time",
            "page_size": 60,
            "page": page.unwrap_or(1),
            "tags": { "type": browse_types }
        })
    };
    client
        .call(&cfg.server, "/v/api/v1/item/list", "POST", &cfg.token, Some(body))
        .await
}

/// 继续观看：GET /v/api/v1/play/list（camelCase 字段）
#[tauri::command]
async fn get_continue_watching(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(&cfg.server, "/v/api/v1/play/list", "GET", &cfg.token, None)
        .await
}

/// 季列表：GET /v/api/v1/season/list/{剧guid}
/// 注意：episode/list 的入参是**季** guid —— 传剧 guid 实测返回 data:null。
/// [lc-1578] 缓存 TTL = DETAIL_LIST_TTL_SECS（10 分钟），**不是** item 详情那档 120 天。
#[tauri::command]
async fn get_seasons(state: State<'_, AppState>, guid: String) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let key = format!("detail:seasons:{}", guid);
    if let Some(v) = disk_cache::json_get(&key, DETAIL_LIST_TTL_SECS) {
        return Ok(v);
    }
    let client = fnapi::new_client();
    let path = format!("/v/api/v1/season/list/{}", guid);
    let v = client.call(&cfg.server, &path, "GET", &cfg.token, None).await?;
    disk_cache::json_put(&key, &v);
    Ok(v)
}

/// 集列表：GET /v/api/v1/episode/list/{guid} —— guid 必须是季 guid（见 get_seasons 注释）
/// [lc-1578] 缓存 TTL = DETAIL_LIST_TTL_SECS（10 分钟）：**这是"集数不刷新"事故的正主**
/// （见该常量注释）—— 新集入库后最多 10 分钟就能看到。
#[tauri::command]
async fn get_episodes(state: State<'_, AppState>, guid: String) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let key = format!("detail:episodes:{}", guid);
    if let Some(v) = disk_cache::json_get(&key, DETAIL_LIST_TTL_SECS) {
        return Ok(v);
    }
    let client = fnapi::new_client();
    let path = format!("/v/api/v1/episode/list/{}", guid);
    let v = client.call(&cfg.server, &path, "GET", &cfg.token, None).await?;
    disk_cache::json_put(&key, &v);
    Ok(v)
}

/// 媒体统计：GET /v/api/v1/mediadb/sum —— 一个接口给全所有计数
/// （库 guid→数量、favorite、movie/tv/live/video 分类数、total）
#[tauri::command]
async fn get_media_sum(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(&cfg.server, "/v/api/v1/mediadb/sum", "GET", &cfg.token, None)
        .await
}

/// 收藏列表：POST /v/api/v1/favorite/list（响应 {total,list} 同 item/list）
#[tauri::command]
async fn get_favorites(
    state: State<'_, AppState>,
    page: Option<i64>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(
            &cfg.server,
            "/v/api/v1/favorite/list",
            "POST",
            &cfg.token,
            Some(json!({
                "tags": { "type": [] },
                "sort_type": "DESC",
                "sort_column": "create_time",
                "exclude_grouped_video": 1,
                "page": page.unwrap_or(1),
                "page_size": 60
            })),
        )
        .await
}

/// 全局分类浏览：不带 ancestor/parent，按 tags.type 过滤（电影/电视节目/直播/其他）
#[tauri::command]
async fn get_items_global(
    state: State<'_, AppState>,
    type_filter: Option<Vec<String>>,
    page: Option<i64>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(
            &cfg.server,
            "/v/api/v1/item/list",
            "POST",
            &cfg.token,
            Some(json!({
                "tags": { "type": type_filter.unwrap_or_default() },
                "sort_type": "DESC",
                "sort_column": "create_time",
                "exclude_grouped_video": 1,
                "page": page.unwrap_or(1),
                "page_size": 60
            })),
        )
        .await
}

/// 条目详情：端点与响应字段对齐 FlyNarwhal（GET /v/api/v1/item/{guid}，camelCase 字段）
/// [lc-1417] **落盘缓存 120 天**：这是详情页最重的一次请求（简介/海报/演职员/评分全在里面），
/// 原来每次进页面都重拉 —— 用户反馈"每次打开是重新拉了一遍数据吗，要等一会才出现"。
#[tauri::command]
async fn get_item_detail(state: State<'_, AppState>, guid: String) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let key = format!("detail:item:{}", guid);
    if let Some(v) = disk_cache::json_get(&key, DETAIL_TTL_SECS) {
        return Ok(v);
    }
    let client = fnapi::new_client();
    let path = format!("/v/api/v1/item/{}", guid);
    let v = client.call(&cfg.server, &path, "GET", &cfg.token, None).await?;
    disk_cache::json_put(&key, &v);
    Ok(v)
}

/// 轮播数据：全局白名单查询（对齐主仓库 itemListApi.ts —— type:['Movie','TV']、
/// 最近更新在前、排除未识别/直播/个人视频），不限定库。
#[tauri::command]
async fn get_carousel_items(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(
            &cfg.server,
            "/v/api/v1/item/list",
            "POST",
            &cfg.token,
            Some(json!({
                "tags": { "type": ["Movie", "TV"] },
                "sort_type": "DESC",
                "sort_column": "create_time",
                "exclude_grouped_video": 1,
                "page": 1,
                "page_size": 24
            })),
        )
        .await
}

// ───────────────────────── 播放 / mpv 控制 ─────────────────────────

// [lc-106] 内嵌宿主窗 / WebView 挖洞 / 裁洞同步 已随「独立 mpv 窗口」模式整体移除。
// 需要回看请用 git 历史（lc-105 及之前）。

/// [lc-102] 静默重登 —— 等价于主仓库 Electron 版播放前的 `refreshSessionCookie`（media.ts:537）。
/// 差别只在凭据来源：它从 Electron 的 `persist:fntv` 会话里读 Cookie 塞进代理 URL；
/// 我们没有那个会话，但配置文件里存着账号密码（含「记住密码/自动登录」）⇒ 直接重登拿新 token，
/// 新 token 会写回配置并用于后续代理 URL 的 `cookie=Trim-MC-token=...` 参数。
async fn relogin(app: &AppHandle, cfg: &Config) -> Result<Config, String> {
    let client = fnapi::new_client();
    let server = fnapi::normalize_server(&cfg.server);
    let data = client
        .call(
            &server,
            "/v/api/v1/login",
            "POST",
            "",
            Some(json!({ "app_name": "trimemedia-web", "username": cfg.username, "password": cfg.password })),
        )
        .await?;
    let token = data
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or("重登响应缺少 token")?
        .to_string();
    let server = client.resolve_final_base(&server, &token).await;
    let mut c = cfg.clone();
    c.server = server;
    c.token = token;
    config::save(app, &c).await?;
    Ok(c)
}


/// [lc-105] 解析「真正要拉流的 guid」：`POST /v/api/v1/play/info {item_guid}` → 取回 `data.guid`
/// （Episode/Movie）。**mpv 路径与页面内 `<video>` 路径共用这一份**（lc-102/103 实测结论：
/// 代理只认具体媒体的 guid，给容器 guid 会 500）。会话失效时会用保存的密码静默重登一次。
async fn resolve_play_guid(
    app: &AppHandle,
    state: &AppState,
    cfg: &mut Config,
    guid: &str,
) -> Result<(String, String, String), String> {
    let client = fnapi::new_client();
    let body = || json!({ "item_guid": guid });
    let mut probe = client
        .call(&cfg.server, "/v/api/v1/play/info", "POST", &cfg.token, Some(body()))
        .await;
    if probe.is_err() && !cfg.password.is_empty() {
        libmpv::trace("[play/info] 失败，尝试用保存的密码静默重登后重试");
        match relogin(app, cfg).await {
            Ok(fresh) => {
                *state.config.write().await = fresh.clone();
                *cfg = fresh;
                probe = client
                    .call(&cfg.server, "/v/api/v1/play/info", "POST", &cfg.token, Some(body()))
                    .await;
            }
            Err(e) => libmpv::trace(&format!("[play/info] 重登失败: {e}")),
        }
    }
    match probe {
        Ok(info) => {
            let t = info.get("type").and_then(|x| x.as_str()).unwrap_or("");
            let mut g = guid.to_string();
            if let Some(x) = info.get("guid").and_then(|x| x.as_str()) {
                if !x.is_empty() {
                    g = x.to_string();
                }
            }
            // [lc-1556] 提取标题（供 mpv 的 force-media-title）：uosc_danmaku 靠
            // media-title 解析番名/集数来走「自建源 → B站 → 弹弹play」的加载链，
            // 没有标题时脚本会直接跳过自动匹配（用户看到的"变成兜底弹弹play"）。
            // 标题拼法对齐主仓库 mpv.ts getTitle()：`剧名 - S1E5: 集名`（电影只有集名）。
            let it = info.get("item").cloned().unwrap_or_else(|| json!({}));
            let get_s = |k: &str| {
                it.get(k)
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            let get_n = |k: &str| {
                it.get(k)
                    .and_then(|v| v.as_i64())
                    .or_else(|| it.get(k).and_then(|v| v.as_f64()).map(|f| f as i64))
                    .unwrap_or(0)
            };
            let ep_title = get_s("title").unwrap_or_else(|| get_s("name").unwrap_or_default());
            let tv_title = get_s("tv_title");
            let season = get_n("season_number");
            let episode = get_n("episode_number");
            // [lc-1561] 季/集号缺失时不拼 "S0E0"：那会在弹幕面板的番名里留下 "- : " 残渣
            // （fnOS 未提供 season/episode 时 season=episode=0 → "番名 - S0E0: 集名" →
            // uosc_danmaku 的 bili_fast_parse 剥掉 "S0E0" 后只剩 "番名 - : 集名"）。
            // 主仓库 getTitle 靠 tvTitle 是否存在分支；这里再按「季集号是否有效」细一档，
            // 三种形态：完整 S/E、只有番名+集名、只有集名。
            let media_title = match tv_title {
                Some(tv) if !tv.is_empty() && season > 0 && episode > 0 => {
                    format!("{tv} - S{season}E{episode}: {ep_title}")
                }
                Some(tv) if !tv.is_empty() && !ep_title.is_empty() => {
                    format!("{tv} - {ep_title}")
                }
                Some(tv) if !tv.is_empty() => tv,
                _ => ep_title.clone(),
            };
            // [lc-1561] 自建源的「系列级匹配记忆」用季 guid 做键（同一部剧的各集共用，
            // 命中一次后续集免搜索）。主仓库要再发一次 play/info 才拿到（resolveSeriesKey），
            // 这里响应就在手上，直取省一次往返。字段名与主仓库同口径：
            // parent_guid 优先，其次 item.parent_guid；都没有则退回本集 guid。
            let parent_guid = info
                .get("parent_guid")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .or_else(|| {
                    it.get("parent_guid")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| g.clone());
            danmaku_shim::write_play_meta(&g, &parent_guid);
            libmpv::trace(&format!(
                "[play/info] ok type={t} 原始={guid} → 拉流用={g} 标题={media_title:?} 系列键={parent_guid}"
            ));
            if matches!(t, "TV" | "Season" | "Collection" | "Directory" | "Folder") {
                return Err(format!(
                    "「{t}」没有可播放的具体媒体 —— 请在选集里点具体一集再播放"
                ));
            }
            Ok((g, t.to_string(), media_title))
        }
        Err(e) => {
            libmpv::trace(&format!("[play/info] 失败: {e}"));
            Err(format!("获取播放信息失败：{e}"))
        }
    }
}

/// [lc-105] **页面内播放（B站/爱奇艺式真内嵌）**的直链 + 元信息。
/// 前端把返回的 url 直接喂给页面里的 `<video>` —— 视频成为 DOM 的一部分：
/// 随页面滚动/圆角/裁剪，HTML 控件随便盖，没有窗口套窗口、没有挖洞。
/// 代理吐的是标准 HTTP MP4 且支持 Range（lc-103 修好 guid 之后实测），浏览器可直接播。
/// 浏览器不支持的容器（MKV 等）由前端 onError 回退到 mpv 路径。
#[tauri::command]
async fn player_source(
    state: State<'_, AppState>,
    app: AppHandle,
    guid: String,
) -> Result<Value, String> {
    let mut cfg = state.config.read().await.clone();
    if cfg.token.is_empty() {
        return Err("未登录".into());
    }
    proxy::ensure(&cfg.proxy_path, &state.proxy_child).await?;
    let (play_guid, t, _title) = resolve_play_guid(&app, &state, &mut cfg, &guid).await?;
    Ok(json!({
        "url": proxy::play_url(&cfg, &play_guid),
        "guid": play_guid,
        "type": t,
    }))
}

#[tauri::command]
async fn play_item(state: State<'_, AppState>, app: AppHandle, guid: String) -> Result<(), String> {
    let mut cfg = state.config.read().await.clone();
    if cfg.token.is_empty() {
        return Err("未登录".into());
    }

    // [lc-1551] 顺序重排：**mpv 先起来**，网络准备全部并行。
    //
    // 实测耗时（anibox.log）：launch 开始 → file-loaded 共 3.26s，而 mpv 自报的
    // 加载只用 292ms、spawn 只用 259ms ⇒ 约 3s 全花在「等网络」（resolve_play_guid
    // 要飞牛 API 往返解析出真正的 Episode guid；probe 要等 Go 代理给出可播流）。
    // 原实现把这些**串行放在启动 mpv 之前**，用户点击后要干等数秒才看到窗口。
    //
    // 现在：点击 → 立刻拉起 mpv 窗口（用户马上有反馈）→ 同时并行解析 guid /
    // 起代理 / 预检直链 → 全部就绪后 loadfile。失败则回滚（关掉刚起的空窗）。
    // 保留 lc-109 的原意：失败仍给明确错误，而不是"开个空窗干等着"。
    let _ = app.emit("mpv-load", json!({ "phase": "start" }));

    // [lc-1551] 阶段计时：每步都记一条，下次「还慢」时有数可查（不再靠猜）。
    let t_play = std::time::Instant::now();
    let stage = |name: &str| {
        libmpv::trace(&format!("[play/stage] {name} @ {}ms", t_play.elapsed().as_millis()));
    };

    // ① [lc-1559] **先解析，再启动 mpv（URL 直接放进命令行）**——起播零 IPC 依赖。
    //
    // 为什么不再"先起窗口再 loadfile"：实测（stage 日志）mpv 启动/加载头几秒内
    // 事件循环被脚本、滤镜、D3D 初始化占住，**任何经 IPC 发送的命令都会卡到秒级**
    // （loadfile/no-reply 写管道也会阻塞）——这是 mpv --input-ipc-server 的固有行为，
    // 不是我们锁的问题（换 spawn_blocking 也一样卡）。
    //
    // 原项目（主仓库 mpv.ts）本来就是**先拼好 M3U8 播放列表、再把文件路径交给 mpv
    // 启动参数**——mpv 一启动就开始播，全程不依赖 IPC。本仓库 lc-106 改独立窗口时
    // 引入了"先 idle 启动 + IPC loadfile"的形态，正是本次一系列起播卡顿的根源。
    //
    // 现在回归主仓库形态：
    //   解析 guid（飞牛 API）→ 写单条目 M3U8（#EXTINF 标题 → media-title → 弹幕链）
    //   → mpv 命令行直接带该 M3U8 ⇒ 窗口出现即开始加载，无 idle 空窗。
    let (play_guid, _t, media_title) = resolve_play_guid(&app, &state, &mut cfg, &guid).await?;
    stage("guid-resolved");
    proxy::ensure(&cfg.proxy_path, &state.proxy_child).await?;
    stage("proxy-ready");
    let direct_url = proxy::play_url(&cfg, &play_guid);

    let display_title: String = media_title
        .replace(['\u{300E}', '\u{300F}', '\u{300C}', '\u{300D}', '\u{3010}', '\u{3011}', '\u{3014}', '\u{3015}', '\u{300A}', '\u{300B}', '\u{3008}', '\u{3009}', '\u{201C}', '\u{201D}', '\u{0027}', '\u{2019}'], "")
        .trim()
        .to_string();

    // 单条目 M3U8（#EXTINF 标题 → mpv 的 media-title → uosc_danmaku 解析番名/集数）
    let m3u8_path = std::env::temp_dir().join("anibox-play.m3u8");
    let m3u8 = format!("#EXTM3U\n#EXTINF:-1,{}\n{}\n", display_title, direct_url);
    std::fs::write(&m3u8_path, m3u8).map_err(|e| format!("写播放列表失败: {e}"))?;
    let url = m3u8_path.to_string_lossy().to_string();
    stage("m3u8-written");

    let session: std::sync::Arc<crate::mpv::MpvSession>;

    // ② 拉起/复用 mpv。
    //
    // [lc-1560] **会话复用时必须自己 loadfile**（lc-1559 重构漏掉的分支）：
    // 原来只有"新建会话"这一支把 url 放进 mpv 命令行；已存在的会话直接拿去用了，
    // **url 根本没被加载** ⇒ 关掉 mpv 窗口后（会话死 → 走新建，正常）与**连续点下一集**
    // （会话活着 → 什么都不发生）表现不同，后者"点了没反应"。
    // 标题同理：新会话走命令行（优先级最高、容器自带 Title tag 顶不掉）；复用会话只能
    // IPC `set`，此时 mpv 不在启动期、IPC 不卡，实测能生效（探针 B 已验）。
    {
        let mut slot = state.mpv.lock().await;
        let existing = match slot.as_ref() {
            Some(m) if !m.is_dead().await => Some(m.clone()),
            Some(m) => {
                m.shutdown().await;
                None
            }
            None => None,
        };
        let s = match existing {
            Some(m) => {
                // 复用：先定标题再 loadfile（顺序不能反，标题要在文件加载前就位）
                let _ = m
                    .command(json!(["set", "force-media-title", display_title.clone()]))
                    .await;
                m.command(json!(["loadfile", url.clone(), "replace"])).await?;
                // [lc-1555 延续] 复用会话时 mpv 可能停在上一集的暂停态/播完态，
                // 本次是明确的"播这个"意图 → 无条件恢复播放（只影响起始态）。
                let _ = m.command(json!(["set", "pause", "no"])).await;
                stage("loadfile(reuse)");
                m
            }
            None => {
                mpv::MpvSession::launch(
                    app.clone(),
                    &cfg.mpv_path,
                    Some(url.clone()),
                    Some(display_title.clone()),
                )
                .await?
            }
        };
        *slot = Some(s.clone());
        session = s;
    }
    stage("mpv-launched(with-url)");

    // ③ [lc-1557] probe 只在后台跑，**不挡起播**。
    //
    // probe 会对代理直链发一次真实 GET —— NAS/网络慢时要等 20s（reqwest 超时上限），
    // 期间用户对着 idle 品牌页干等。现在 loadfile 的入口是**本地 m3u8 文件**（秒开），
    // mpv 打开内层直链失败时本就会走 end-file error → 我们已有的错误展示链路，
    // probe 的"提前报错"价值不再值得它挡路。
    //
    // 两条路谁先到都行：probe 若先于 mpv 发现代理 500，照样把代理给的明确文案 emit 给前端。
    let probe_url = direct_url.clone();
    {
        let app2 = app.clone();
        tokio::spawn(async move {
            if let Err(e) = proxy::probe(&probe_url).await {
                libmpv::trace(&format!("[probe] 直链预检失败: {e}"));
                let _ = app2.emit(
                    "mpv-load",
                    json!({ "phase": "error", "error": format!("直链预检失败：{e}") }),
                );
            }
        });
    }
    stage("probe-spawned");

    // [lc-1500] 清理在线资源（webrs_play）写入的站点头，避免污染**后续**播放。
    // [lc-1554] 这两条放 loadfile **之后**：`referrer` / `http-header-fields` 只在打开 URL
    // 那一刻被读取，而本次加载的是 Go 代理的本地直链（127.0.0.1:22346），残留站点 referrer
    // 对它无影响；清理的意义只是"为下一次播放恢复默认"，放后面完全等价。
    // [lc-1558] 且不等回包（fire-and-forget）：实测 set 同样会卡到秒级；这两条只影响"下一次"
    // 播放，晚几百毫秒生效无碍，不能拖住本次起播。
    // [lc-1556] 播放期保留主题底（lc-081 清 backdrop 是为救 spawn 时代「进播放页卡死」，
    // 现在视频是进程内 D3D 输出，清掉反而让页面透出桌面）；player_stop 里恢复逻辑保留。
    let _ = session.command(json!(["set", "referrer", ""])).await;
    let _ = session
        .command(json!(["set", "http-header-fields", ""]))
        .await;

    Ok(())
}

// [lc-100] 自绘浮层控制条（bar.html / playerbar 窗口）已**整体删除**。
// 它从 lc-090 起就默认关闭且架构上无法被唤起（视频区内鼠标归 mpv），属于纯死代码：
// 一个额外窗口 + 一个额外页面 + 一条懒创建路径，还曾经自己冒出来（lc-090 的 bug）。
// 底栏统一由 mpv 原生 uosc 提供；需要的话从 git 历史里取回（lc-099 之前的版本）。

#[tauri::command]
async fn player_stop(state: State<'_, AppState>) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        m.command(json!(["stop"])).await?;
    }
    Ok(())
}

#[tauri::command]
async fn mpv_toggle_pause(state: State<'_, AppState>) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        m.command(json!(["cycle", "pause"])).await?;
    }
    Ok(())
}

/// [lc-093] App 形态（内嵌小窗 / 全页全屏）→ mpv 的 `fullscreen` 属性镜像。
/// uosc 用它当底栏双形态判据（小窗只留 播放暂停+上下一集 | 时间 | 全屏；全屏保持原样）。
#[tauri::command]
async fn mpv_set_ui_fullscreen(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        m.set_ui_fullscreen(on).await?;
    }
    Ok(())
}

#[tauri::command]
async fn mpv_seek(state: State<'_, AppState>, seconds: f64, absolute: bool) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        let mode = if absolute { "absolute" } else { "relative" };
        m.command(json!(["seek", seconds, mode])).await?;
    }
    Ok(())
}

/// [lc-085] 输入转发：视频是原生子窗（浮在 WebView 之上）且 libmpv 内嵌模式下 mpv 不自行
/// 接收鼠标/键盘（它建的嵌入子窗是 disabled 的）——鼠标事件由页面代收后经此命令转成 mpv 的
/// `mouse` / `keypress` 命令。uosc 底栏出现、左键暂停、右键菜单、滚轮音量、双击全屏全靠这条通道。
/// 坐标语义：相对视频窗客户区的**设备像素**（前端已按 dpr 换算）。
/// 按钮编码同 mpv：0=左 1=中 2=右；负数=释放（-1 左 / -2 中 / -3 右）；滚轮 3=上 4=下。
#[tauri::command]
async fn mpv_input(
    state: State<'_, AppState>,
    kind: String,
    x: f64,
    y: f64,
    button: Option<i64>,
    key: Option<String>,
) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        let cmd = match kind.as_str() {
            "move" => json!(["mouse", x as i64, y as i64]),
            "button" | "wheel" => json!(["mouse", x as i64, y as i64, button.unwrap_or(0)]),
            "key" => json!(["keypress", key.unwrap_or_default()]),
            _ => return Ok(()),
        };
        if kind != "move" {
            libmpv::trace(&format!("input {} -> {:?}", kind, cmd)); // 低频事件留痕（move 太频繁不记）
        }
        // 鼠标位置偶发越界等返回非 success，属正常，不向页面冒错
        let _ = m.command(cmd).await;
    }
    Ok(())
}

#[tauri::command]
async fn mpv_set_volume(state: State<'_, AppState>, volume: f64) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        m.command(json!(["set_property", "volume", volume])).await?;
    }
    Ok(())
}

#[tauri::command]
async fn mpv_set_rate(state: State<'_, AppState>, rate: f64) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        m.command(json!(["set_property", "speed", rate])).await?;
    }
    Ok(())
}

// ───────────────────────── 缓存（图片/数据落盘） ─────────────────────────

/// 缓存统计：占用字节 / 条目数 / 上限(MB) / 目录。设置面板「缓存」一栏的三个要素之二。
#[tauri::command]
async fn cache_stats() -> Result<Value, String> {
    let (bytes, files) = disk_cache::stats();
    Ok(json!({
        "bytes": bytes,
        "files": files,
        "limitMb": disk_cache::limit_mb(),
        "dir": disk_cache::root_display(),
    }))
}

/// 改上限（MB；0 = 不限）：写回 config.json + 立即按新上限淘汰一次（磁盘与内存同步）。
#[tauri::command]
async fn cache_set_limit(
    app: AppHandle,
    state: State<'_, AppState>,
    limit_mb: u64,
) -> Result<Value, String> {
    let mut cfg = state.config.read().await.clone();
    cfg.cache_limit_mb = limit_mb;
    config::save(&app, &cfg).await?;
    *state.config.write().await = cfg;
    disk_cache::set_limit_mb(limit_mb);
    let (bytes, files) = disk_cache::stats();
    Ok(json!({ "bytes": bytes, "files": files, "limitMb": disk_cache::limit_mb(), "dir": disk_cache::root_display() }))
}

/// 清理缓存：**内存层与磁盘层一起清**（清了盘内存还留着旧图 = 占用对不上、还看得到旧图）。
#[tauri::command]
async fn cache_clear() -> Result<Value, String> {
    disk_cache::clear()?;
    let (bytes, files) = disk_cache::stats();
    Ok(json!({ "bytes": bytes, "files": files, "limitMb": disk_cache::limit_mb(), "dir": disk_cache::root_display() }))
}

// ───────────────────────── [lc-1434] 设置面板（通用/外观/诊断） ─────────────────────────
// 背景：原 Electron 项目（Fntv-Plus）设置面板里有一批通用项，本软件缺。
// 这里只搬**对新客户端有意义**的：退出行为、启动页、性能模式、轮播 Logo；
// 那些给"注入进飞牛网页"用的设置（详情页美化/隐藏原生按钮/界面语言/轮播样式四选一）
// 在新架构（自有 React UI）下没有作用对象，不搬。
//
// 与 `get_config`/`save_config` 的分工：那对命令是**全量 Config**（登录页在用，含密码/token）；
// 这一对是**设置面板专用的白名单子集**——面板只该碰这几个键，不给它整份配置的读写面。

/// 设置面板读：只回面板关心的字段（**不含密码/token** —— 面板不需要，也不该拿到）。
#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    Ok(json!({
        "exitMode": cfg.exit_mode,
        "perfMode": cfg.perf_mode,
        "carouselLogo": cfg.carousel_logo,
        "launchPage": cfg.launch_page,
        "autoLogin": cfg.auto_login,
        // [lc-1561] 自建弹幕接口（地址非敏感，明文回填；多地址逐行）
        "danmuApiEnabled": cfg.danmu_api_enabled,
        "danmuApiBases": cfg.danmu_api_bases,
        "danmuApiBase": cfg.danmu_api_base,
    }))
}

/// 设置面板写：接受上述键的**任意子集**（前端只发改动的那个），逐个白名单校验后落盘。
/// 非法值直接 Err（不静默改成默认值——静默会让 UI 与配置悄悄不一致，排查时极难发现）。
#[tauri::command]
async fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    arg: Value,
) -> Result<Value, String> {
    let mut cfg = state.config.read().await.clone();
    if let Some(v) = arg.get("exitMode") {
        let s = v.as_str().unwrap_or("");
        if !matches!(s, "exit" | "tray" | "ask") {
            return Err(format!("exitMode 非法：{s}（只允许 exit/tray/ask）"));
        }
        cfg.exit_mode = s.to_string();
    }
    if let Some(v) = arg.get("perfMode") {
        cfg.perf_mode = v.as_bool().ok_or("perfMode 必须是布尔值")?;
    }
    if let Some(v) = arg.get("carouselLogo") {
        cfg.carousel_logo = v.as_bool().ok_or("carouselLogo 必须是布尔值")?;
    }
    if let Some(v) = arg.get("launchPage") {
        let s = v.as_str().unwrap_or("");
        if !matches!(s, "home" | "continue") {
            return Err(format!("launchPage 非法：{s}（只允许 home/continue）"));
        }
        cfg.launch_page = s.to_string();
    }
    if let Some(v) = arg.get("autoLogin") {
        cfg.auto_login = v.as_bool().ok_or("autoLogin 必须是布尔值")?;
    }
    // [lc-1561] 自建弹幕接口：多地址（逐行去空/去尾斜杠/去重，上限 4 条 —— 与原项目同口径）。
    // 改完**立即**桥给 Node 客户端并同步 Lua 闸门（不这样得等下次启动才生效）。
    let mut danmu_changed = false;
    if let Some(v) = arg.get("danmuApiEnabled") {
        cfg.danmu_api_enabled = v.as_bool().ok_or("danmuApiEnabled 必须是布尔值")?;
        danmu_changed = true;
    }
    if let Some(v) = arg.get("danmuApiBases") {
        let arr = v.as_array().ok_or("danmuApiBases 必须是数组")?;
        let mut list: Vec<String> = arr
            .iter()
            .filter_map(|x| x.as_str())
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        list.dedup();
        list.truncate(4);
        cfg.danmu_api_base = list.first().cloned().unwrap_or_default();
        cfg.danmu_api_bases = list;
        danmu_changed = true;
    }
    if danmu_changed {
        danmaku_shim::write_danmu_config(&cfg)?;
        danmaku_shim::sync_lua_gate(&cfg.mpv_path, cfg.danmu_api_enabled, &cfg.danmu_api_base);
    }
    config::save(&app, &cfg).await?;
    *state.config.write().await = cfg.clone();
    Ok(json!({
        "exitMode": cfg.exit_mode,
        "perfMode": cfg.perf_mode,
        "carouselLogo": cfg.carousel_logo,
        "launchPage": cfg.launch_page,
        "autoLogin": cfg.auto_login,
        "danmuApiEnabled": cfg.danmu_api_enabled,
        "danmuApiBases": cfg.danmu_api_bases,
        "danmuApiBase": cfg.danmu_api_base,
    }))
}

/// [lc-1434] 启动页：**登录成功后、首页数据加载前**被调用（前端路由据此决定落点）。
/// 独立成一条命令而不是复用 get_settings：它要能被"最小可用"地调用，
/// 不依赖前端已把整份设置读回来（登录流程早期就用到）。
#[tauri::command]
async fn get_launch_page(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.config.read().await.launch_page.clone())
}

/// [lc-1436] 开机自启：Windows 走 HKCU\...\Run 注册表（**不引入 auto-launch crate**：
/// 一行注册表键就是它的全部实现，多一个依赖只为省 20 行不值）。
/// 值 = 当前 exe 全路径（`current_exe`）——dev 下是 target/debug 里的 exe，
/// 自启后指向开发构建属预期（用户从 dev 开的自启，就该由 dev 的 exe 承担）。
#[tauri::command]
async fn set_autostart(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
            KEY_SET_VALUE, REG_SZ,
        };
        const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
        const VALUE_NAME: &str = "AniBox";
        let to_wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };

        let subkey = to_wide(RUN_KEY);
        let mut hkey: HKEY = std::ptr::null_mut();
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null_mut(),
                0, // REG_OPTION_NON_VOLATILE
                KEY_SET_VALUE,
                std::ptr::null_mut(),
                &mut hkey,
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            return Err(format!("打开注册表 Run 键失败（错误码 {rc}）"));
        }
        // 旧版（FntvPlus）留下的自启值指向已不存在的旧 exe，顺手清掉；rc=2 表示本来就没有
        let legacy = to_wide("FntvPlus");
        unsafe { RegDeleteValueW(hkey, legacy.as_ptr()) };
        let vname = to_wide(VALUE_NAME);
        let result = if enabled {
            let exe = std::env::current_exe()
                .map_err(|e| format!("取当前程序路径失败：{e}"))?
                .to_string_lossy()
                .to_string();
            // 带引号：路径里有空格时（Program Files）不带引号会被系统截断成错误的命令
            let quoted = format!("\"{exe}\"");
            let data = to_wide(&quoted);
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    data.as_ptr() as *const u8,
                    data.len() * std::mem::size_of::<u16>(),
                )
            };
            let rc = unsafe {
                RegSetValueExW(hkey, vname.as_ptr(), 0, REG_SZ, bytes.as_ptr(), bytes.len() as u32)
            };
            if rc != 0 {
                Err(format!("写自启注册表失败（错误码 {rc}）"))
            } else {
                libmpv::trace(&format!("[lc-1436] 开机自启已开启 → {quoted}"));
                Ok(())
            }
        } else {
            // 删不存在的值会返回 ERROR_FILE_NOT_FOUND(2)：那就是"本来没开"，当成功处理
            let rc = unsafe { RegDeleteValueW(hkey, vname.as_ptr()) };
            unsafe { RegCloseKey(hkey) };
            if rc != 0 && rc != 2 {
                return Err(format!("删自启注册表失败（错误码 {rc}）"));
            }
            libmpv::trace("[lc-1436] 开机自启已关闭");
            return Ok(());
        };
        unsafe { RegCloseKey(hkey) };
        result
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = enabled;
        Ok(()) // 非 Windows 暂不实现（本软件当前只发 Windows）
    }
}

/// [lc-1436] 诊断：在文件管理器里打开日志/缓存/数据目录。
/// kind 白名单三选一 —— 不接受任意路径（那等于给前端一个"打开任意位置"的能力，没必要）。
#[tauri::command]
async fn open_path(app: AppHandle, kind: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let path: std::path::PathBuf = match kind.as_str() {
        "log" => {
            let p = libmpv::log_path();
            if !p.exists() {
                return Err("诊断日志尚未生成（本进程还没写过日志）".into());
            }
            p
        }
        "cache" => {
            let p = disk_cache::root_path().ok_or("缓存目录尚未初始化")?;
            if !p.exists() {
                return Err("缓存目录尚未生成".into());
            }
            p
        }
        "app" => {
            use tauri::Manager;
            app.path()
                .app_data_dir()
                .map_err(|e| format!("无法解析数据目录：{e}"))?
        }
        other => return Err(format!("未知的打开目标：{other}（只允许 log/cache/app）")),
    };
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| format!("打开目录失败：{e}"))
}

/// [lc-1435] 关闭询问的**回答**：exit_mode == "ask" 时窗口拦截了关闭并 emit `close-ask`，
/// 前端弹确认框后回调这里。exit 走 app.exit(0) —— 与托盘退出同一条路，
/// 保证 RunEvent::Exit 的退场清理（杀 mpv/代理子进程）一定会跑到。
#[tauri::command]
async fn resolve_close(app: AppHandle, action: String) -> Result<(), String> {
    match action.as_str() {
        "exit" => {
            libmpv::trace("[lc-1435] 用户选择退出程序");
            app.exit(0);
            Ok(())
        }
        "hide" => {
            use tauri::Manager;
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.hide();
            }
            libmpv::trace("[lc-1435] 用户选择最小化到托盘");
            Ok(())
        }
        other => Err(format!("未知的关闭动作：{other}（只允许 exit/hide）")),
    }
}

// ───────────────────────── 入口 ─────────────────────────

/// [lc-1435] 系统托盘：左键点击显示主窗口；右键菜单「显示主窗口 / 退出」。
/// 为什么要托盘：设置面板的「关闭按钮 → 最小化到托盘」需要一个能回来的入口，
/// 没有托盘的话隐藏窗口 = 应用"消失"（只能去任务管理器杀进程）。
/// 退出菜单项直接 app.exit(0) —— 与 resolve_close("exit") 同一条路，退场清理必然执行。
fn install_tray(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::Manager;

    let show_i = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        // 左键单击不弹菜单（Windows 习惯：左键=显示窗口，右键=菜单）
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
            }
            "quit" => {
                libmpv::trace("[lc-1435] 托盘菜单 → 退出");
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
            }
        });
    // 图标：用应用自身图标（tauri.conf.json 的 bundle icon）；取不到就不带图标建托盘
    // （Windows 上没图标的托盘项仍然可点，只是没有图案）
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    libmpv::trace("[lc-1435] 系统托盘已创建");
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
// ───────────────────────── 窗口视觉：SWCA 动态模糊 + OS 级圆角 ─────────────────────────
// windowEffects 的 acrylic 在 Win11 走 DWM 系统底板（自带系统圆角/边框，lc-029 已弃用）。
// 这里改用 SWCA（SetWindowCompositionAttribute， undocumented API，无系统边框副作用），
// 再用 SetWindowRgn 把整个窗口（含模糊层）裁成圆角矩形——模糊不会从方角露出。

#[cfg(target_os = "windows")]
mod winvisual {
    /// SWCA 会重新启用 DWM 边框（且边框跟随 SetWindowRgn 圆角轮廓 → 圆角外白线），必须在其后关掉
    pub fn disable_dwm_border(hwnd: isize) {
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        let color_none: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
        unsafe {
            DwmSetWindowAttribute(
                hwnd as _,
                34, // DWMWA_BORDER_COLOR
                &color_none as *const u32 as *const core::ffi::c_void,
                4,
            );
        }
    }

    /// DWM 系统亚克力底板（DWMSBT_TRANSIENTWINDOW）：DWM 原生绘制、无 SWCA 的 1px 白边；
    /// 明暗由 DWMWA_USE_IMMERSIVE_DARK_MODE 跟随应用主题
    pub fn set_backdrop(hwnd: isize, dark: bool) {
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        unsafe {
            let dark_flag: u32 = if dark { 1 } else { 0 };
            DwmSetWindowAttribute(
                hwnd as _,
                20, // DWMWA_USE_IMMERSIVE_DARK_MODE
                &dark_flag as *const u32 as *const core::ffi::c_void,
                4,
            );
            let backdrop: u32 = 3; // DWMSBT_TRANSIENTWINDOW（亚克力）
            DwmSetWindowAttribute(
                hwnd as _,
                38, // DWMWA_SYSTEMBACKDROP_TYPE
                &backdrop as *const u32 as *const core::ffi::c_void,
                4,
            );
        }
        disable_dwm_border(hwnd);
    }

    /// DWM 系统圆角（DWMWCP_ROUND）：抗锯齿完美、自动跟随窗口尺寸；
    /// SWCA 模糊层与 webview 一起被系统圆角化（SetWindowRgn 硬边裁剪有锯齿，弃用）
    pub fn set_system_round_corner(hwnd: isize) {
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        let pref: u32 = 2; // DWMWCP_ROUND
        unsafe {
            DwmSetWindowAttribute(
                hwnd as _,
                33, // DWMWA_WINDOW_CORNER_PREFERENCE
                &pref as *const u32 as *const core::ffi::c_void,
                4,
            );
        }
        disable_dwm_border(hwnd);
    }

    pub fn hwnd_isize(win: &tauri::Window) -> Option<isize> {
        win.hwnd().ok().map(|h| h.0 as isize)
    }

    pub fn hwnd_isize_wv(win: &tauri::WebviewWindow) -> Option<isize> {
        win.hwnd().ok().map(|h| h.0 as isize)
    }
}

/// dev 诊断：eval 进来的 JS 用它把观测写回诊断日志（可视验证/自动化反馈通道）
#[tauri::command]
fn dev_log(msg: String) {
    libmpv::trace(&format!("JS: {}", msg));
}

#[tauri::command]
fn set_acrylic_tint(
    window: tauri::Window,
    state: State<'_, AppState>,
    light: bool,
) -> Result<(), String> {
    *state.last_light.lock().unwrap() = Some(light);
    #[cfg(target_os = "windows")]
    {
        use winvisual::{hwnd_isize, set_backdrop};
        if let Some(hwnd) = hwnd_isize(&window) {
            set_backdrop(hwnd, !light);
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (window, light);
    Ok(())
}

pub fn run() {
    // [lc-1440] 改名迁移（Fntv-Plus Tauri2 → AniBox）**必须在 Builder 之前**：
    //   identifier 变了 ⇒ app_data_dir / WebView profile 目录全换路径，
    //   而 WebView2 一创建就会锁住它的 profile（EBWebView 里的 leveldb），那时再搬就晚了。
    //   详见 migrate.rs 头部。
    let mig = migrate::run();
    // 日志系统此时还没建（libmpv::trace 走的是追加写文件，无需初始化，可以直接用）
    if let Some(d) = &mig.appdata {
        libmpv::trace(&format!("[lc-1440] 数据目录迁移 APPDATA → {d}"));
    }
    if let Some(d) = &mig.localappdata {
        libmpv::trace(&format!("[lc-1440] 数据目录迁移 LOCALAPPDATA → {d}"));
    }
    if mig.appdata.is_none() && mig.localappdata.is_none() {
        libmpv::trace("[lc-1440] 无需迁移（新目录已存在，或旧目录不存在）");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        // 协议名沿用 "fntvimg" 不改名：图片 URL 会进 WebView 缓存与 localStorage，改名 = 老缓存全 404
        .register_asynchronous_uri_scheme_protocol("fntvimg", |ctx, request, responder| {
            // 带会话鉴权的图片代理：WebView 里 <img> 直连 NAS 不带 Cookie 会全裂图
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                let response = imgproto::handle(app, request).await;
                responder.respond(response);
            });
        })

        .setup(|app| {
            // VEH 崩溃记录器：任意致命异常（D3D 视频输出崩因定位）写出异常码+故障模块+偏移，
            // 落 %TEMP%/anibox.log（lc-082 视频输出专项）
            libmpv::install_crash_logger();
            // 启动时从磁盘读回配置（token/路径），否则每次重启都要重新登录
            let cfg = tauri::async_runtime::block_on(config::load(app.handle()));
            // 图片/数据落盘缓存：目录在应用数据目录（不是 %TEMP%），上限来自 config，启动即按上限淘汰一次
            {
                let root = app
                    .path()
                    .app_data_dir()
                    .map(|d| d.join("cache"))
                    .unwrap_or_else(|_| std::env::temp_dir().join("anibox-cache"));
                disk_cache::init(root, cfg.cache_limit_mb);
            }
            *app.state::<AppState>().config.blocking_write() = cfg.clone();
            // [lc-1561] 自建弹幕接口配置桥（Node 侧每次读盘 ⇒ 这里先写一份，后续 save_config
            // 再覆盖；保证 shim 一启动就能读到「启用/地址」，不会误判为未启用）。
            if let Err(e) = danmaku_shim::write_danmu_config(&cfg) {
                libmpv::trace(&format!("[danmaku-shim] 写自建源配置失败（不影响内置 B站）: {e}"));
            }
            danmaku_shim::sync_lua_gate(&cfg.mpv_path, cfg.danmu_api_enabled, &cfg.danmu_api_base);
            // [lc-1560] 启动即**预热弹幕 shim**（B站弹幕下载经它，见 danmaku_shim.rs）。
            // 首次播放时 shim 已在监听，省一次 node 冷启动；失败不影响本地播放。
            danmaku_shim::spawn_warmup(app.handle().clone());
            // [lc-1552] 启动即**预热 Go 代理**：冷启动 proxy.exe 要 2.5s（实测
            // [play/stage] proxy-ready @3035ms），而它只在首次播放时才被拉起 ⇒
            // 第一次点播放要白等这 2.5s（后续播放代理已驻留，只要 477ms）。
            // 这里在后台静默拉起来，用户第一次点播放时它已经在监听了。
            // 失败无副作用（ensure 内部只 log，不 panic；真播放时还会再试一次）。
            {
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let st = h.state::<AppState>();
                    let path = st.config.read().await.proxy_path.clone();
                    let holder = &st.proxy_child;
                    if let Err(e) = proxy::ensure(&path, holder).await {
                        libmpv::trace(&format!("[warmup] Go 代理预热失败（不影响使用）: {e}"));
                    }
                });
            }
            // Win11 会给窗口画一圈系统细边框 —— 用户眼中的「外面又包了一层线条」，
            // DWMWA_BORDER_COLOR(34) 设为 DWMWA_COLOR_NONE(0xFFFFFFFE) 关掉（旧系统调用失败无副作用）
            #[cfg(target_os = "windows")]
            if let Some(win) = app.get_webview_window("main") {
                use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
                if let Ok(hwnd) = win.hwnd() {
                    let color_none: u32 = 0xFFFF_FFFE;
                    unsafe {
                        DwmSetWindowAttribute(
                            hwnd.0 as _,
                            34, // DWMWA_BORDER_COLOR
                            &color_none as *const u32 as *const core::ffi::c_void,
                            4,
                        );
                    }
                }
            }
            // dev 期开 DevTools（9222）：**改为按需开启**，不再每次启动都弹（用户反馈）。
// 需要时用 `set FNTV_DEVTOOLS=1` 再跑 dev.cmd，或运行时在窗口里按 F12 / Ctrl+Shift+I。
// [lc-115] 原来是无条件 open_devtools()，每次 dev 启动都会弹一个开发者工具窗。
            #[cfg(debug_assertions)]
            if std::env::var("FNTV_DEVTOOLS").ok().as_deref() == Some("1") {
                if let Some(wv) = app.get_webview_window("main") {
                    let _ = wv.open_devtools();
                    libmpv::trace("DevTools 已开启（FNTV_DEVTOOLS=1）");
                }
            }
            // SWCA 动态模糊 + 窗口级圆角（默认浅色 tint；主题切换由前端 set_acrylic_tint 更新）
            //
            // [lc-1414] **可用 FNTV_NO_ACRYLIC=1 关掉亚克力底板**（排障/观感二选一）。
            //   为什么加这个开关：用户在「演职人员」页看到背景有个**大椭圆遮挡**，截图里是一段巨大的
            //   弧形边界 + 半透明浅紫灰内部。我把 App.css 全量扫过一遍（clip-path / 各类 gradient /
            //   border-radius / mask / filter:blur 逐条列出），**没有任何规则能画出那个形状** ——
            //   唯一能画弧的是 .theme-veil 的 clip-path:circle()，而它已加"三保险"清理（lc-1406）。
            //   因此最可能是**窗口级 DWM 亚克力的模糊层边界**：窗口 transparent:true，
            //   内容一旦半透明，亚克力底板就透出来，其模糊边界呈弧形。
            //   关掉它即可判定：椭圆消失 ⇒ 就是亚克力；仍在 ⇒ 另有来源（那就要 DevTools 点选确认）。
            #[cfg(target_os = "windows")]
            if let Some(win) = app.get_webview_window("main") {
                use winvisual::{hwnd_isize_wv, set_backdrop, set_system_round_corner};
                if let Some(hwnd) = hwnd_isize_wv(&win) {
                    let no_acrylic = std::env::var("FNTV_NO_ACRYLIC").ok().as_deref() == Some("1");
                    if no_acrylic {
                        libmpv::trace("FNTV_NO_ACRYLIC=1 → 跳过亚克力底板（仅保留系统圆角）");
                    } else {
                        set_backdrop(hwnd, false); // 默认浅色
                    }
                    set_system_round_corner(hwnd);
                }
            }
            // 窗口以 visible:false 创建：延迟到透明化/圆角区域/模糊全部就位后再显示，
            // 消除首帧残留（圆角外露白/旧画面）。
            // [lc-106 修复] 此块在 lc-106 清理挖洞代码时被误删 ⇒ 窗口永远不 show ⇒ "dev 启动后窗口没了"。
            let h2 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                if let Some(win) = h2.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            });
            // dev 无人值守复现钩子（lc-082 视频输出崩溃专项）：FNTV_AUTOPLAY_TEST=<guid> 时
            // 启动 5s 后直接走与前端相同的 play_item 真链路（含 bounds 预置），结果写诊断日志。
            #[cfg(debug_assertions)]
            if let Ok(guid) = std::env::var("FNTV_AUTOPLAY_TEST") {
                let h3 = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
                    match play_item(h3.state::<AppState>(), h3.clone(), guid).await {
                        Ok(_) => libmpv::trace("AUTOPLAY: play_item ok"),
                        Err(e) => libmpv::trace(&format!("AUTOPLAY play_item err: {}", e)),
                    }
                });
            }
            // dev 诊断钩子：FNTV_EVAL_JS=<js> 启动 5s 后在主 webview 执行（配合 AUTOPLAY 做可视验证）
            #[cfg(debug_assertions)]
            if let Ok(js) = std::env::var("FNTV_EVAL_JS") {
                let h4 = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
                    if let Some(win) = h4.get_webview_window("main") {
                        let _ = win.eval(&js);
                        libmpv::trace("EVAL_JS executed");
                    }
                });
            }
            install_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // [lc-1435] 关闭按钮三态：exit 放行 / tray 隐藏 / ask 交给前端弹框。
            // 为什么用 on_window_event 而不是前端拦：WebView 里拦不住原生关闭按钮
            // （那是 OS 的窗口消息，不经过 JS），必须在这一层拦。
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                use tauri::Manager;
                let app = window.app_handle();
                let mode = tauri::async_runtime::block_on(async {
                    app.state::<AppState>().config.read().await.exit_mode.clone()
                });
                match mode.as_str() {
                    "exit" => {
                        libmpv::trace("[lc-1435] CloseRequested → exit（直接退出）");
                    }
                    "tray" => {
                        api.prevent_close();
                        let _ = window.hide();
                        libmpv::trace("[lc-1435] CloseRequested → tray（隐藏到托盘）");
                    }
                    _ => {
                        // "ask"（含非法值兜底）：拦住并问前端；前端回 resolve_close 决定去向
                        api.prevent_close();
                        let _ = app.emit("close-ask", ());
                        libmpv::trace("[lc-1435] CloseRequested → ask（已 emit close-ask）");
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            set_acrylic_tint,
            dev_log,
            mpv_input,
            tmdb::tmdb_show_details,
            // [lc-128] 演员表**主源 = TMDB**（trim_id 精确锚点 → IMDb find → 标题搜索带相似度门槛），
            // 结果落盘缓存；飞牛/Bangumi 侧只在 TMDB 无数据时兜底（前端负责展示优先级）
            tmdb::tmdb_credits,
            // [lc-123] 发现页：TMDB 当季热门/即将播出（一条命令带 mode 参数）
            tmdb::tmdb_discover,
            // [lc-1577] 发现页左栏「媒体库」搜索框（真实 TMDB 搜索）
            tmdb::tmdb_search,
            // [lc-1576] 评分分布（详情页「评分透视」兜底：BGM 无 rating.count 时改用 TMDB 的）
            tmdb::tmdb_rating_details,
            // [lc-119] Bangumi（全线走自建镜像，见 bgm.rs 头部说明）
            bgm::bgm_match,
            bgm::bgm_subject,
            bgm::bgm_episodes,
            bgm::bgm_characters,
            bgm::bgm_persons,
            bgm::bgm_comments,
            // [lc-1412] 评论（长影评，/p1/.../reviews）：与「吐槽」分开的两个数据源
            bgm::bgm_reviews,
            // [lc-1401] 关联条目（详情页「关联条目」标签页：片头曲/原声集/游戏/画集…按关系分组）
            bgm::bgm_related,
            // [lc-123] 发现页右栏：每日新番表（镜像 /calendar 路由，见 bgm.rs）
            bgm::bgm_calendar,
            get_config,
            save_config,
            // [lc-1561] 自建弹幕接口连通测试（设置面板）
            test_danmu_api,
            // [lc-1434] 设置面板（通用/外观）：白名单子集读写 + 启动页查询
            get_settings,
            set_settings,
            get_launch_page,
            // [lc-1436] 诊断：打开日志/缓存/数据目录；开机自启
            open_path,
            set_autostart,
            // [lc-1435] 关闭询问的回答（exit → 走 RunEvent::Exit 退场清理）
            resolve_close,
            // 缓存面板三要素：占用 / 上限 / 清理
            cache_stats,
            cache_set_limit,
            cache_clear,
            login,
            logout,
            check_session,
            get_media_libs,
            get_media_sum,
            get_favorites,
            get_items_global,
            get_items,
            get_continue_watching,
            get_seasons,
            get_episodes,
            get_item_detail,
            get_carousel_items,
            play_item,
            player_stop,
            mpv_toggle_pause,
            player_source,
            mpv_set_ui_fullscreen,
            mpv_seek,
            mpv_set_volume,
            mpv_set_rate,
            // [lc-1515] Bangumi 账号绑定 + 追番状态同步（Kazumi 同款）
            bgm_account::bgm_account_status,
            bgm_account::bgm_account_bind,
            bgm_account::bgm_account_unbind,
            bgm_account::bgm_collect_set,
            bgm_account::bgm_collect_get,
            bgm_account::bgm_sync_now,
            bgm_account::bgm_collect_stats,
            bgm_account::bgm_collect_season,
            bgm_account::bgm_watching_covers,
            // [lc-1500] 在线资源（Kazumi 规则源）：发现页无源条目 → 搜索/选集/解析直链 → mpv
            // [lc-1501] 多源聚合搜索 + 用户加源/删源（Kazumi「播放来源 / 我的规则」同款）
            webrs::webrs_rules,
            webrs::webrs_search,
            webrs::webrs_search_multi,
            webrs::webrs_add_rule,
            webrs::webrs_remove_rule,
            webrs::webrs_chapters,
            webrs::webrs_resolve,
            webrs::webrs_play
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                // 退出时清场：mpv 子进程 + 嵌入窗口 + Go 代理
                let st = app_handle.state::<AppState>();
                if let Ok(mut slot) = st.mpv.try_lock() {
                    if let Some(m) = slot.as_ref() {
                        let _ = tauri::async_runtime::block_on(m.shutdown());
                    }
                    *slot = None;
                }
                // [lc-1560] 弹幕 shim 子进程同样收掉（别留孤儿 node）
                if let Some(mut dc) = st.danmaku_child.blocking_lock().take() {
                    let _ = dc.kill();
                }
                let mut pc = st.proxy_child.blocking_lock();
                if let Some(c) = pc.as_mut() {
                    let _ = c.kill();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    /// [lc-1578] 哨兵：**季/集列表**的 TTL 必须远短于 item 详情（10 分钟 vs 120 天）。
    /// 反过来（列表也挂 120 天）正是用户报的「飞牛里三集、软件只显示两集」——这条测试让它变红。
    /// 真机取证（2026-10-15）：`detail:episodes` 缓存 17:04 写入（当时 2 集）、23:25 仍被命中返回，
    /// 而飞牛侧已是 3 集。
    #[test]
    fn detail_list_ttl_stays_short() {
        assert_eq!(super::DETAIL_LIST_TTL_SECS, 600, "集数刷新节奏：10 分钟一刷");
        assert!(
            super::DETAIL_LIST_TTL_SECS * 1000 < super::DETAIL_TTL_SECS,
            "列表 TTL 必须远小于 item 详情 TTL（否则新集长时间不可见）"
        );
    }
}
