//! AniBox —— 自绘 UI + 直调飞牛 API + mpv 双层窗口内嵌。
//! 命令面向前端：配置 / 登录 / 媒体库 / 播放控制。

mod ai_title;
mod anilist;
mod access_code;
mod authx;
mod bgm;
mod bgm_account;
mod cloud115;
// [bili-account] B 站账号（Cookie 读写/体检）+ Node 检测 + 一键自建弹幕源
mod bili_account;
// [bili-qr] B 站二维码扫码登录（generate/poll，Cookie 落盘复用 bili_account）
mod bili_qr;
mod config;
mod danmaku;
mod danmaku_shim;
mod danmaku_sources;
pub mod subtitle_sources;
mod disk_cache;
// [lc-1651] 本地收藏（侧边栏「收藏」写入端；飞牛 item/favorite 在本机 NAS 上未实现）
mod favorites;
// [lc-1599] 应用内反馈（工单制）：提交/列表/会话/回复（脱敏与本地索引见 feedback.rs 头部）
mod feedback;
mod fnapi;
// [progress] 把播放进度回传飞牛 `play/record`（「继续观看」的数据源；旧项目同名链路）
mod fnos_progress;
// [gamepad] 全局手柄：原生 gilrs 后台轮询（mpv/PotPlayer 前台时同样可用）
mod gamepad;
// [lc-1615] 本地播放历史（观影记录页数据源之一）：history.json 落盘 + 与飞牛 play/list 合并
mod history;
mod libmpv;
// [library-cover] 首页媒体库卡片自定义封面：app_data_dir 落盘 + /__cover 协议读取
mod libcover;
// [lc-1701] 演员页「入库状态」：作品批量与飞牛媒体库比对（强标识 + 归一化标题/年份）
mod libmatch;
mod imgproto;
mod migrate;
// [lc-1720] 手动获取元数据：本地按剧 guid 覆盖飞牛返回（不写飞牛）
mod metadata_override;
mod mirror_sign;
mod mpv;
mod nas_subtitle;
// [speed] 按剧集（item guid）持久化 mpv 播放速度（app_data_dir/playback-speed.json）
mod playback_speed;
// [player] 统一播放启动封装（mpv / 本机 PotPlayer）+ 本地 HTTP relay
mod player;
mod proxy;
mod tmdb;
// [update] 全量更新（自建下载器 + NSIS）+ 前端资源热补丁覆盖层（fntvweb 协议）
mod update;
// [lc-1663] 匿名使用统计（心跳上报，移植自旧项目 usageStats.ts）
mod usage_stats;
mod watched;
mod webrs;
// [lc-1747] webrs 常驻隐藏 WebView 单例服务（嗅探 / 反爬验证共用基建）
mod webrs_webview;
// [webview2] Windows WebView2 Runtime 检测 + 应用内下载兜底（见模块头注释）
mod webview2;
// [title] 展示标题繁 → 简（仅 mpv/#EXTINF 等展示字段，匹配元数据保持原文）
mod zh;

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
    /// 内置自建弹幕源 danmu_api 子进程（127.0.0.1:22352）
    pub builtin_child: Mutex<Option<std::process::Child>>,
    /// [player] 当前由本应用拉起的 PotPlayer PID（用于新播放前/退出时清理）
    pub potplayer_pid: Mutex<Option<u32>>,
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
            builtin_child: Mutex::new(None),
            potplayer_pid: Mutex::new(None),
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
    // [lc-1623] 防误伤（BGM 与飞牛解耦）：这是**整份覆盖**配置的写入口（登录页在用）。
    // BGM 的账号三件套（token / 用户名 / 记忆开关）属于 BGM 自己的账号状态，
    // **只能**由 bgm_account_bind / unbind / set_remember 修改。飞牛登录、设置面板、
    // 登录页等任何 save_config 都一律保留当前值 —— 前端 Config 是挂载时的旧快照，
    // 允许它回写会把刚绑定的 token 冲掉（用户报「莫名退出账号」的根因之一）。
    let mut config = config;
    let old_engine = crate::danmaku::resolve_engine(&*state.config.read().await).to_string();
    {
        let cur = state.config.read().await;
        config.bgm_token = cur.bgm_token.clone();
        config.bgm_username = cur.bgm_username.clone();
        config.bgm_remember = cur.bgm_remember;
        // [访问码] 只能由 login / logout 流程维护；登录页整份快照回写不得冲掉当前值
        config.access_code = cur.access_code.clone();
        // [lc-1661] 日志设置只能由 feedback_log_settings_set 修改（前端 Config 快照不含/易过期）。
        config.log_debug = cur.log_debug;
        config.log_modules = cur.log_modules.clone();
        // [lc-1662] 匿名统计开关只能由 usage_stats_set 修改
        config.usage_stats_enabled = cur.usage_stats_enabled;
        // [lc-1663] 统计运行态（匿名 ID / 上次上报 / 欠报）同样只由统计模块维护
        config.usage_stats_anon_id = cur.usage_stats_anon_id.clone();
        config.usage_stats_last_ping_day = cur.usage_stats_last_ping_day.clone();
        config.usage_stats_last_ping_ok = cur.usage_stats_last_ping_ok;
        config.usage_stats_pending_days = cur.usage_stats_pending_days.clone();
        // [gamepad] 手柄设置只能由 set_settings 修改（登录页的整份 Config 快照不含/易过期）
        config.gamepad_enabled = cur.gamepad_enabled;
        config.gamepad_deadzone = cur.gamepad_deadzone;
        config.gamepad_direction_threshold = cur.gamepad_direction_threshold;
        config.gamepad_repeat_delay = cur.gamepad_repeat_delay;
        config.gamepad_repeat_interval = cur.gamepad_repeat_interval;
        // 弹幕引擎只能由 set_settings 修改（登录页快照缺该字段时会被 serde default
        // 静默写回 "node"，把用户设的 rust 重置掉）
        config.danmu_engine = cur.danmu_engine.clone();
        // 自建源开关/地址同样只由 set_settings 修改（避免登录页旧快照把用户配置冲掉）
        config.danmu_api_enabled = cur.danmu_api_enabled;
        config.danmu_api_base = cur.danmu_api_base.clone();
        config.danmu_api_bases = cur.danmu_api_bases.clone();
    }
    config::save(&app, &config).await?;
    // [lc-1661] 日志设置随整份配置保存后同步到运行态（本函数保留旧值，实际不会变，稳妥起见仍同步）
    libmpv::apply_settings(config.log_debug, config.log_modules.clone());
    // [lc-1561] 自建弹幕接口配置桥给 Node 侧客户端（改完立即生效，见 write_danmu_config）
    danmaku_shim::write_danmu_config(&config)?;
    // [lc-1561] 同步 Lua 侧闸门（只写开关、绝不写地址，见 sync_lua_gate 注释）
    danmaku_shim::sync_lua_gate(
        &config.mpv_path,
        config.danmu_api_enabled,
        &crate::danmaku::effective_base(&config),
    );
    let builtin_active = config.danmu_api_enabled && crate::danmaku::uses_builtin(&config);
    *state.config.write().await = config;
    // 内置自建源：仅在内置源生效时才重配，避免每次保存都触发
    if builtin_active {
        crate::danmaku::builtin::spawn_reconfigure(app.clone());
    }
    // 弹幕引擎运行期切换：node ⇄ rust（收掉旧服务、按新引擎重启）
    let new_engine = crate::danmaku::resolve_engine(&*state.config.read().await).to_string();
    if old_engine != new_engine {
        danmaku_shim::spawn_reconfigure(app.clone());
    }
    Ok(())
}

/// [lc-1561] 测试某个自建弹幕接口地址是否可达（设置面板「测试连接」）。
///
/// 用与客户端相同的口径：请求 `/api/v2/search/anime?keyword=…` 只看**能否连通 + 是否返回
/// JSON**（不校验搜索结果，那是业务）。返回人话结论，对齐主仓库 testConnection 的提示风格。
#[tauri::command]
async fn test_danmu_api(app: AppHandle, url: String) -> Result<String, String> {
    // 地址留空 = 测试内置实例（零配置默认源）。
    let is_builtin = url.trim().is_empty();
    let base_owned = if is_builtin {
        crate::danmaku::builtin::base()
    } else {
        url.trim().trim_end_matches('/').to_string()
    };
    let base = base_owned.as_str();
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
            // 内置源连不上时给出可操作的原因：优先区分「本机没装 Node.js」，
            // 否则只提示服务未启动，避免只看到笼统的「连接被拒绝」。
            if is_builtin {
                if let Err(node_err) = crate::danmaku::builtin::resolve_node(&app) {
                    return Err(format!(
                        "内置弹幕源启动失败：{node_err}（将自动降级为内置 B 站弹幕）"
                    ));
                }
                return Err(format!(
                    "内置弹幕源未就绪：服务未启动或端口不通（{e}）。请确认已启用「自建弹幕接口」，或稍后重试"
                ));
            }
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
    access_code: Option<String>,
) -> Result<Value, String> {
    if server.trim().is_empty() || username.trim().is_empty() || password.is_empty() {
        return Err("请提供完整的登录信息。".into());
    }

    let raw = server.trim().to_string();
    // [端口探测] 对齐旧项目 auth.ts：裸填私有 IP 且未带端口时，试 fnOS 常见端口。
    let mut server = match fnapi::detect_fn_os_port(&raw).await {
        Some(port) => {
            let stripped = raw.replace('：', ":");
            let stripped = stripped
                .trim_start_matches("http://")
                .trim_start_matches("https://");
            let host = stripped.split('/').next().unwrap_or("").split(':').next().unwrap_or("");
            format!("http://{host}:{port}")
        }
        None => fnapi::normalize_server(&raw),
    };

    // [访问码] 与旧项目一致：非空时先向 fnOS 网关 /access_code_verify 预验证并建立会话；
    // 网关授权 Cookie 由 fnapi 的进程内会话表携带到后续登录/API 请求。
    let access_code = access_code.unwrap_or_default().trim().to_string();
    if !access_code.is_empty() {
        match access_code::establish(&server, &access_code).await {
            Ok(resolved) if !resolved.is_empty() => server = resolved,
            Ok(_) => {}
            Err(access_code::AccessCodeError::Rejected) => {
                return Err("访问码错误，请检查后重试。".into());
            }
            Err(access_code::AccessCodeError::Network(e)) => {
                return Err(format!(
                    "无法连接到访问码验证服务，请检查地址、证书或网络连接。（{e}）"
                ));
            }
        }
    }

    let client = fnapi::new_client();
    let data = match client
        .call(
            &server,
            "/v/api/v1/login",
            "POST",
            "",
            Some(json!({ "app_name": "trimemedia-web", "username": username, "password": password })),
        )
        .await
    {
        Ok(d) => d,
        // [访问码] 未填访问码但服务器开启了门禁：接口会返回门户 HTML 而非 JSON。
        // 给出与旧项目一致的明确引导，而不是笼统的「地址/端口填错」。
        Err(e) if e.contains("返回了网页") => {
            return Err("登录被访问码门禁拦截（服务器返回网页）。请在登录页填写「访问码」后重试。".into());
        }
        Err(e) => return Err(e),
    };
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
    cfg.access_code = access_code;
    config::save(&app, &cfg).await?;
    *state.config.write().await = cfg.clone();
    Ok(json!({ "ok": true }))
}

#[tauri::command]
async fn logout(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // [lc-1623] 退出登录 = **只清飞牛侧**（会话 token），绝不动 `bgm_token` 与任何 BGM 追番状态：
    //   BGM 的登录态是独立的（cfg.bgm_token，见 bgm_account.rs），它的绑定/解绑只由
    //   bgm_account_bind / bgm_account_unbind 触发 —— 飞牛退出不该牵连它。
    //   两处修正（原实现只改内存、不落盘，退出后重启会因磁盘上 token 还在而"自动登录"）：
    //     · 补 config::save 落盘，让退出真正生效；
    //     · 保留 server / username / password（供重新登录预填，server 亦是"曾配置过"的路由信号）。
    let mut cfg = state.config.read().await.clone();
    cfg.token = String::new();
    config::save(&app, &cfg).await?;
    *state.config.write().await = cfg;
    Ok(())
}

#[tauri::command]
async fn check_session(state: State<'_, AppState>, app: AppHandle) -> Result<Value, String> {
    let mut cfg = state.config.read().await.clone();
    if cfg.token.is_empty() || cfg.server.is_empty() {
        return Err("未登录".into());
    }
    // [访问码] 对齐旧项目 winctrl.setupCookieRestore：启动时用持久化访问码重建网关会话。
    // 进程内 Cookie 表随重启清空，不重建的话已开访问码的服务器会把 /user/info 拦成门户 HTML。
    // 被拒绝 → 会话不可用，回登录页；网络失败 → 同样回登录页（登录页会回填访问码，点一次登录即可）。
    if !cfg.access_code.is_empty() {
        match access_code::establish(&cfg.server, &cfg.access_code).await {
            Ok(base) if !base.is_empty() && base != cfg.server => {
                cfg.server = base;
                let _ = config::save(&app, &cfg).await;
                *state.config.write().await = cfg.clone();
            }
            Ok(_) => {}
            Err(access_code::AccessCodeError::Rejected) => {
                return Err("访问码错误，请检查后重试。".into());
            }
            Err(access_code::AccessCodeError::Network(e)) => {
                return Err(format!("访问码会话重建失败：{e}"));
            }
        }
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

/// [lc-1727] item/list 单页条数：默认 60（历史值），上限 200。
/// 前端「滚动加载更多」逐页取；这里只做区间收敛，避免误传 0/负数/巨大值把 NAS 拖死。
const ITEM_LIST_PAGE_SIZE: i64 = 60;
const ITEM_LIST_PAGE_SIZE_MAX: i64 = 200;

/// [lc-1727] 单页条数收敛：缺省/非法 → 60；正数 → 夹到 [1, 200]。
fn item_list_page_size(raw: Option<i64>) -> i64 {
    match raw {
        Some(n) if n > 0 => n.min(ITEM_LIST_PAGE_SIZE_MAX),
        _ => ITEM_LIST_PAGE_SIZE,
    }
}

/// [lc-1727] 分页终止判定（纯函数，前端 `shouldLoadMore` 同口径）：
/// - `loaded`：已累计的条目数（含本页）
/// - `total`：服务端返回的总数（None/0 = 未知）
/// - `page_size`：每页条数
///
/// 返回 true = 还应继续翻页。规则：
/// 1. total 已知（>0）且 loaded 已达标 → 停；
/// 2. 本页不足一页（loaded 不是 page_size 的整数倍）→ 停（最后一页）；
/// 3. 其余继续。
/// 空页（本页 0 条）由调用方直接判停 —— loaded 不变时这里无法区分「空页」与「整页」。
fn should_load_more(loaded: usize, total: Option<i64>, page_size: usize) -> bool {
    if page_size == 0 {
        return false;
    }
    if let Some(t) = total {
        if t > 0 && loaded as i64 >= t {
            return false;
        }
    }
    loaded % page_size == 0
}

/// 条目分页查询（形状经测试 NAS 实测 + FlyNarwhal 源码互证）：
/// - 库根浏览：只传 ancestor_guid（parent_guid 传库 guid 服务端返回 0 条）；
///   实测同库白名单 151 条 vs 无过滤 2635 条剧集混排 —— Episode 全靠白名单滤掉，
///   tags.type 白名单不含 Season/Episode（对齐 FlyNarwhal libraryBrowseValues）。
/// - 文件夹钻取（未识别 Directory/fv_*）：只传 parent_guid，exclude_grouped_video=0
///   （FlyNarwhal 注释：钻取带 1 会让原始视频文件夹返回空），sort_title ASC 对齐网页。
/// - [lc-1727] 分页：`page`（1 起）+ `page_size`（默认 60，可传参，上限 200）。
///   响应里的 `total` 是**服务端总数**（不受本页条数限制），前端据此判断是否还有下一页。
#[tauri::command]
async fn get_items(
    app: AppHandle,
    state: State<'_, AppState>,
    ancestor_guid: Option<String>,
    parent_guid: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let browse_types = json!(["Movie", "TV", "Directory", "Video", "LiveChannel"]);
    let page = page.unwrap_or(1).max(1);
    let page_size = item_list_page_size(page_size);
    let body = if let Some(parent) = parent_guid {
        json!({
            "parent_guid": parent,
            "exclude_grouped_video": 0,
            "sort_type": "ASC",
            "sort_column": "sort_title",
            "page_size": page_size,
            "page": page,
            "tags": { "type": browse_types }
        })
    } else {
        json!({
            "ancestor_guid": ancestor_guid,
            "exclude_grouped_video": 1,
            "sort_type": "DESC",
            "sort_column": "create_time",
            "page_size": page_size,
            "page": page,
            "tags": { "type": browse_types }
        })
    };
    let mut v = client
        .call(&cfg.server, "/v/api/v1/item/list", "POST", &cfg.token, Some(body))
        .await?;
    // [lc-1720] 本地手动元数据覆盖（只读叠加，不写飞牛）
    metadata_override::apply_response_list(&app, &mut v);
    Ok(v)
}

/// 继续观看：GET /v/api/v1/play/list（实测为 snake_case 字段，前端读取时双格式兼容）
#[tauri::command]
async fn get_continue_watching(state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let mut v = client
        .call(&cfg.server, "/v/api/v1/play/list", "GET", &cfg.token, None)
        .await?;
    // [lc-1774] 集条目的 parent_guid 是**季** guid，直接给详情页会整页渲染成
    // 「Season 1」。复用观影记录的 enrich_series（季 guid → item/detail →
    // 剧 guid），为每个集条目补 seriesGuid（剧级）；字段先归一到 camelCase。
    // play/list 的 data 直接就是条目数组（实测 {code,data:[...]}，无 list 包层）
    if let Some(list) = v.get_mut("data").and_then(|d| d.as_array_mut()) {
        for it in list.iter_mut() {
            let snake = it.get("parent_guid").and_then(|x| x.as_str()).unwrap_or("");
            let camel = it.get("parentGuid").and_then(|x| x.as_str()).unwrap_or("");
            if !snake.is_empty() && camel.is_empty() {
                it["parentGuid"] = json!(snake);
            }
        }
        crate::history::enrich_series(list, &cfg.server, &cfg.token).await;
        // [lc-1774] 富化出剧 guid 后**直接改写条目**为剧级形态（guid=剧、type=TV）：
        // 前端卡片原样透传即可，不再依赖它自己挑 guid 字段（挑错就是 Season 1 / 错剧）。
        // 保留 season/episode 号供详情页定位；电影条目（type=Movie/Video）不动。
        for it in list.iter_mut() {
            let t = it.get("type").and_then(|x| x.as_str()).unwrap_or("");
            if t == "Episode" || t == "Season" {
                let series = it.get("seriesGuid").and_then(|x| x.as_str()).unwrap_or("");
                if !series.is_empty() {
                    it["guid"] = json!(series);
                    it["type"] = json!("TV");
                }
            }
        }
    }
    Ok(v)
}

/// [lc-1615] 观影记录：飞牛 play/list ∪ 本机播放历史（history.rs）。
/// 合并规则见 history::merge（同 guid 取更新的那条；飞牛条目无时间戳时以本机时间为准）；
/// 剧集条目再做「季→剧 guid + 竖版海报」富化（缓存优先、并发受限）。
/// 飞牛拉取失败不整体报错：本机记录照常返回，前端据 fnosOk/fnosError 给出提示。
#[tauri::command]
async fn get_watch_history(app: AppHandle, state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let local = history::load(&app);
    // [lc-1669] 老用户迁移：把旧 history.json 的播放记录回填到日级台账（热力图数据源）。
    // 幂等：只补 `tracked=false` 的旧天，新口径天不会被覆盖。
    history::migrate_days_from_history(&app, &local);
    // [lc-1670] 旧 Electron 版 localStorage 里的热力图台账（fntv_wh_daycount_v1）也补进来
    history::migrate_legacy_electron_days(&app);
    let mut fnos: Option<Value> = None;
    let mut fnos_error = String::new();
    if cfg.token.is_empty() {
        fnos_error = "未登录".into();
    } else {
        let client = fnapi::new_client();
        match client
            .call(&cfg.server, "/v/api/v1/play/list", "GET", &cfg.token, None)
            .await
        {
            Ok(v) => fnos = Some(v),
            Err(e) => {
                libmpv::trace(&format!("[lc-1615] play/list 拉取失败（仅返回本机记录）: {e}"));
                fnos_error = e;
            }
        }
    }
    let mut items = history::merge(fnos.as_ref(), local);
    // [lc-1667] 观影记录口径：无封面的个人视频默认不计算；[lc-1747] 在线源（webrs）恢复，不再按 source 过滤。
    // 在「富化 / 聚合」之前过滤 —— 省掉这些条目无谓的网络富化。
    items.retain(|it| {
        let s = |k: &str| it.get(k).and_then(|x| x.as_str()).unwrap_or("");
        history::visible_in_history(s("type"), s("poster"))
    });
    history::enrich_series(&mut items, &cfg.server, &cfg.token).await;
    // [lc-1616] 富化（拿到剧 guid）之后再按**作品**聚合：同一部剧只出一张卡。
    let mut items = history::aggregate_by_series(&items);
    // [lc-1728] 本地手动元数据覆盖在观影记录「作品卡」上也生效（标题/海报/评分/年份）。
    // 此前只在 item/list、item/detail 等链路应用，观影记录行永远显示飞牛原始标题 ——
    // 用户在弹窗里选定的元数据"看起来没反应"。
    metadata_override::apply_to_items(&app, &mut items);
    // 把用户自己的评分/评语（ratings.json）贴到作品上，前端浮层可零额外往返直接显示。
    let ratings = history::load_ratings(&app);
    for it in items.iter_mut() {
        if let Some(k) = it.get("guid").and_then(|x| x.as_str()) {
            if let Some(r) = ratings.get(k) {
                it["myRating"] = json!(r.score);
                it["myReview"] = json!(r.review);
            }
        }
    }
    // [lc-1668] 热力图数据：来自**日级台账**（播放过就算，同一部剧一天只算一次），
    // 与「作品清单」的过滤口径解耦 —— 个人视频即便因无封面不进列表，播放活动仍计入日历。
    let heat = history::heat_days(&history::load_days(&app));
    Ok(json!({
        "items": items,
        "fnosOk": fnos.is_some(),
        "fnosError": fnos_error,
        "heat": heat,
    }))
}

/// [lc-1616] 取某作品「我的评分/评语」（键 = 作品 guid；无则 null）。
#[tauri::command]
fn get_my_rating(app: AppHandle, key: String) -> Option<history::MyRating> {
    history::get_rating(&app, &key)
}

/// [lc-1631] 年度报告数据源：本机播放记录（事件级）+ 日级台账，**不触网**（秒开）。
/// 前端据 years 渲染年份下拉；events 算月度节奏/观影时刻/连续天数/TOP；days 兜底活跃天数。
/// 飞牛侧的作品级事实（看过哪些、进度）由前端复用 get_watch_history 的 items，不在此重复拉取。
#[tauri::command]
fn get_watch_report(app: AppHandle) -> Value {
    history::report_data(&app)
}

/// [lc-1631] 播放结束回填真实观看时长/进度/看完状态到对应记录（guid 由 history-recorded
/// 事件告知前端）。起播时这些量不存在，只能事后补；拿不到（播放器崩溃等）就保持 0，
/// 报告侧按「进度 × 总时长」折算，绝不编造。
#[tauri::command]
fn record_watch_session(app: AppHandle, guid: String, watched_ms: u64, progress: f64, finished: bool) {
    history::backfill_session(&app, &guid, watched_ms, progress, finished);
}

/// [progress] 播放中的进度上报（前端每 10s 一次 + 起播/暂停/切集/退出时立即一次）。
///
/// 两条链路一起走：
///   ① 本地：只更新**已存在**的那条记录（起播时由 [`play_item`] 写入），按剧级去重，绝不新增；
///   ② 飞牛：`POST /v/api/v1/play/record` —— 首页「继续观看」读的是服务端 `play/list`，
///      不回传这里就永远不会有进度/卡片（旧项目 Fntv-Plus 同链路）。
///
/// `guid` = 本地历史记录键（`history-recorded` 给的那条，整季自动切集时保持不变）；
/// `item_guid` = **当前正在播的那一集**（前端从 mpv `filename` 解析，切集后会变）；
/// 在线源（`webrs:`）不回传飞牛。
#[tauri::command]
async fn report_watch_progress(
    app: AppHandle,
    state: State<'_, AppState>,
    guid: String,
    item_guid: Option<String>,
    position_ms: u64,
    duration_ms: u64,
) -> Result<bool, String> {
    let local = history::update_progress(&app, &guid, position_ms, duration_ms);

    let cfg = state.config.read().await.clone();
    let item = item_guid
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(guid.as_str())
        .to_string();
    if !item.starts_with("webrs:") && !cfg.token.is_empty() && position_ms > 0 && duration_ms > 0 {
        match fnos_progress::report(
            &cfg,
            &item,
            position_ms as f64 / 1000.0,
            duration_ms as f64 / 1000.0,
        )
        .await
        {
            Ok(()) => {
                // 首页「继续观看」刷新信号（前端防抖后重新拉 play/list）。
                let _ = app.emit(
                    "watch-progress",
                    json!({
                        "guid": guid,
                        "itemGuid": item,
                        "positionMs": position_ms,
                        "durationMs": duration_ms,
                        // [speed] 当前播放速度（本地跟踪器；仅用于前端展示/调试，
                        // 飞牛 play/record 无速率字段，不随远程回传）。
                        "speed": fnos_progress::current_speed(),
                    }),
                );
            }
            Err(e) => libmpv::trace(&format!("[progress] 飞牛 play/record 失败: {e}")),
        }
    }
    Ok(local)
}

/// [lc-1616] 写某作品「我的评分/评语」：score 夹到 0..=5；score=0 且评语空 = 清除。
/// 只落本机 `app_data_dir/ratings.json`，**不**触碰任何第三方（BGM 同步由前端按需另调）。
#[tauri::command]
fn set_my_rating(app: AppHandle, key: String, score: u8, review: String) -> Result<Value, String> {
    history::set_rating(&app, &key, score, review);
    Ok(json!({ "ok": true }))
}

/// 季列表：GET /v/api/v1/season/list/{剧guid}
/// 注意：episode/list 的入参是**季** guid —— 传剧 guid 实测返回 data:null。
/// [lc-1578] 缓存 TTL = DETAIL_LIST_TTL_SECS（10 分钟），**不是** item 详情那档 120 天。
/// [lc-1595] `force` = **跳过缓存读**（仍然写回）：播放结束/窗口回焦时前端用它拿最新状态。
#[tauri::command]
async fn get_seasons(
    state: State<'_, AppState>,
    guid: String,
    force: Option<bool>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let key = format!("detail:seasons:{}", guid);
    if !force.unwrap_or(false) {
        if let Some(v) = disk_cache::json_get(&key, DETAIL_LIST_TTL_SECS) {
            return Ok(v);
        }
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
/// [lc-1595] `force` = **跳过缓存读**（仍然写回）：观看进度（`ts`/`watched`）随每次播放变化，
/// 播放结束/窗口回焦时必须拿最新的一份，否则详情页停在旧进度（用户实测：看完第 3 集退出仍是旧状态）。
#[tauri::command]
async fn get_episodes(
    app: AppHandle,
    state: State<'_, AppState>,
    guid: String,
    force: Option<bool>,
    series_guid: Option<String>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let key = format!("detail:episodes:{}", guid);
    let mut v = if !force.unwrap_or(false) {
        match disk_cache::json_get(&key, DETAIL_LIST_TTL_SECS) {
            Some(v) => v,
            None => {
                let client = fnapi::new_client();
                let path = format!("/v/api/v1/episode/list/{}", guid);
                let v = client.call(&cfg.server, &path, "GET", &cfg.token, None).await?;
                disk_cache::json_put(&key, &v);
                v
            }
        }
    } else {
        let client = fnapi::new_client();
        let path = format!("/v/api/v1/episode/list/{}", guid);
        let v = client.call(&cfg.server, &path, "GET", &cfg.token, None).await?;
        disk_cache::json_put(&key, &v);
        v
    };
    // [lc-1720] 集列表：补剧级标题/封面/简介（不覆盖集标题与集号）
    if let Some(sg) = series_guid.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(list) = v.as_array_mut() {
            metadata_override::apply_to_list(&app, list, sg);
        }
    }
    Ok(v)
}

/* ───────────────── [lc-1629] 飞牛「每集/条目自定义元数据」写接口 ─────────────────
   背景：飞牛有时把集数/条目识别错，用户在飞牛网页端可以「编辑元数据 / 手动匹配影片 /
   解除匹配影片 / 刷新元数据」，改完飞牛侧会重新匹配成正确元数据。本组命令把这几件事搬进
   App 详情页（只读的展示早已接入，这里补上**写**）。
   接口形状来自飞牛影视 Web 前端 bundle（`/v/assets/` 下的 JS 路由表）与 Fntv-Plus 参考实现，
   并用只读探针实测核对（getEditDetail / scrap.search 已实打确认；写命令未对用户库发起真实写入）：
     · 读   POST /item/getEditDetail  body {item_guid}         → 完整可编辑元数据（含 *_locked 字段）
     · 写   POST /item/saveEditDetail body = 上面那份全量对象（改哪些填哪些）+ 对应 `*_locked:true`
     · 刷新 POST /item/refresh        body {item_guid, refresh_mode}
     · 搜   POST /scrap/search        body {source, source_keyword, lan, item_guid?}
     · 匹配 POST /scrap/rescrap       body {item_guid, source_id, source, type, media_guids,
                                            season_number, episode_number, scrap_tv_type}
     · 解除 DELETE /scrap/:guid
   鉴权与其它接口一致（Authorization + Cookie + Authx 签名；nonce 由 fnapi 统一追加）。
   ⚠ 写命令只做「提交」，**不在此做任何批量/自动写**，全部由用户在前端点按钮触发。
   [lc-1632] rescrap 的请求体已由用户从飞牛网页端 F12 **实测抓包**逐字段确认（不再是推断）：
     source 实测为 "trim_id"、source_id 即 trim id（如 tt220542），两者均由前端把**选中的那条
     搜索结果原样透传**；media_guids 实测为 []；集级匹配带 season_number/episode_number（对话框
     输入）与 scrap_tv_type="SingleEpisode"。实测请求另带 `x-trim-client: web` /
     `x-trim-client-version` 两个头 —— 那是网页端客户端标识，**鉴权不依赖**它，故此处不加；
     仅在实测出现写入被拒时再补（见 fnapi 的请求头拼装）。 */

/// 读取条目/剧集的完整可编辑元数据（编辑元数据弹层的初始值来源）。
#[tauri::command]
async fn get_edit_detail(state: State<'_, AppState>, guid: String) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(
            &cfg.server,
            "/v/api/v1/item/getEditDetail",
            "POST",
            &cfg.token,
            Some(json!({ "item_guid": guid })),
        )
        .await
}

/// 保存条目/剧集元数据：`payload` = getEditDetail 返回的全量对象（前端只改 title/overview
/// 等目标字段并置 `xxx_locked:true`，其余原样回传，避免飞牛把没传的字段清空 —— 对齐参考实现）。
#[tauri::command]
async fn save_edit_detail(state: State<'_, AppState>, payload: Value) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    // nonce 由 fnapi::call 在最终 body 上追加（签名对最终字节计算）。
    client
        .call(
            &cfg.server,
            "/v/api/v1/item/saveEditDetail",
            "POST",
            &cfg.token,
            Some(payload),
        )
        .await
}

/// 刷新元数据：让飞牛按当前匹配重新刮削该条目（`mode` = "immediate"|"await"，缺省 immediate）。
#[tauri::command]
async fn refresh_item(
    state: State<'_, AppState>,
    guid: String,
    mode: Option<String>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    client
        .call(
            &cfg.server,
            "/v/api/v1/item/refresh",
            "POST",
            &cfg.token,
            Some(json!({
                "item_guid": guid,
                "refresh_mode": mode.unwrap_or_else(|| "immediate".into()),
            })),
        )
        .await
}

/// 手动匹配影片 · 第一步：按标题/TMDB id/IMDB id 搜索刮削源。
/// `source` = "title"|"tmdb_id"|"imdb_id"（飞牛搜索源枚举），`keyword` 为搜索词。
#[tauri::command]
async fn scrap_search(
    state: State<'_, AppState>,
    source: String,
    keyword: String,
    lan: Option<String>,
    item_guid: Option<String>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let mut body = json!({
        "source": source,
        "source_keyword": keyword,
        "lan": lan.unwrap_or_else(|| "zh-CN".into()),
    });
    if let Some(g) = item_guid {
        body["item_guid"] = json!(g);
    }
    client
        .call(&cfg.server, "/v/api/v1/scrap/search", "POST", &cfg.token, Some(body))
        .await
}

/// 手动匹配影片 · 第二步：把选中的搜索结果应用到条目（触发飞牛重新匹配）。
/// [lc-1632] 形参逐字段对齐飞牛网页端**实测**请求体（POST /scrap/rescrap）：
/// `item_guid` / `source_id` / `source` / `type` / `media_guids` / `season_number` /
/// `episode_number` / `scrap_tv_type` —— 字段名一律照实测，不改名、不省略。
/// 其中 `source`/`source_id`/`type` 由前端把**选中的那条搜索结果原样透传**（实测
/// source="trim_id"、source_id=trim id 如 tt220542、type="TV"），这里不做任何映射。
/// `item_guid` 是**被匹配条目自身**的 guid（不是搜索结果里的 id）。
#[tauri::command]
async fn scrap_rescrap(
    state: State<'_, AppState>,
    item_guid: String,
    source_id: String,
    source: String,
    r#type: String,
    media_guids: Vec<String>,
    season_number: Option<i64>,
    episode_number: Option<i64>,
    scrap_tv_type: Option<String>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let mut body = json!({
        "item_guid": item_guid,
        "source_id": source_id,
        "source": source,
        "type": r#type,
        "media_guids": media_guids,
    });
    // 集级匹配才带这三项（Movie / 整剧匹配飞牛不传，避免用 0 覆盖飞牛侧）；
    // scrap_tv_type 实测固定为 "SingleEpisode"。
    if let (Some(s), Some(e)) = (season_number, episode_number) {
        body["season_number"] = json!(s);
        body["episode_number"] = json!(e);
        body["scrap_tv_type"] = json!(scrap_tv_type.unwrap_or_else(|| "SingleEpisode".into()));
    }
    client
        .call(&cfg.server, "/v/api/v1/scrap/rescrap", "POST", &cfg.token, Some(body))
        .await
}

/// 解除匹配影片：删除该条目的刮削关联（`DELETE /scrap/:guid`）。
#[tauri::command]
async fn scrap_unscrap(state: State<'_, AppState>, guid: String) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let path = format!("/v/api/v1/scrap/{}", guid);
    client.call(&cfg.server, &path, "DELETE", &cfg.token, None).await
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

/// 收藏列表：本地收藏 ∪ 飞牛 `POST /v/api/v1/favorite/list`（响应 {total,list} 同 item/list）。
/// [lc-1651] 本机 NAS 的 `item/favorite` 路由未实现（返回 Not Implemented），收藏由客户端
/// `favorites.rs` 落盘；这里把两边合并、本地在前（按加入时间倒序），飞牛哪天支持了自动带上。
#[tauri::command]
async fn get_favorites(
    app: AppHandle,
    state: State<'_, AppState>,
    page: Option<i64>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let remote = if cfg.token.is_empty() {
        None
    } else {
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
            .ok()
    };
    let local = favorites::list(&app);
    let mut seen: std::collections::HashSet<String> = local
        .iter()
        .filter_map(|v| v.get("guid").and_then(|x| x.as_str()).map(str::to_string))
        .collect();
    let mut list = local;
    if let Some(data) = remote {
        if let Some(arr) = data.get("list").and_then(|x| x.as_array()) {
            for it in arr {
                let g = it.get("guid").and_then(|x| x.as_str()).unwrap_or("");
                if g.is_empty() || !seen.insert(g.to_string()) {
                    continue;
                }
                list.push(it.clone());
            }
        }
    }
    // [lc-1744] 收藏列表里的每一条**本来就是已收藏**：显式打上 `is_favorite: 1`。
    // 前端海报卡此前只靠逐卡 `favorite_get(guid)` 判定，收藏页一旦回查失败 / 键不一致，
    // 心形就会被刷成空心（用户报的「收藏页红心没点亮」）。列表标记作为权威信号，
    // 卡片据此直接点亮，不再依赖逐卡回查。
    for it in list.iter_mut() {
        if let Some(obj) = it.as_object_mut() {
            obj.insert("is_favorite".into(), json!(1));
        }
    }
    let mut out = json!({ "total": list.len(), "list": list });
    // [lc-1720] 收藏卡片同样吃本地元数据覆盖
    metadata_override::apply_response_list(&app, &mut out);
    Ok(out)
}

/// 全局分类浏览：不带 ancestor/parent，按 tags.type 过滤（电影/电视节目/直播/其他）
#[tauri::command]
async fn get_items_global(
    app: AppHandle,
    state: State<'_, AppState>,
    type_filter: Option<Vec<String>>,
    page: Option<i64>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let mut v = client
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
        .await?;
    metadata_override::apply_response_list(&app, &mut v);
    Ok(v)
}

/// [lc-1701] 演员页「入库状态」批量匹配。
/// 前端一次传入演员的全部作品（`works: [{ tmdbId?, bgmId?, title, originalTitle?, year?, mediaType }]`），
/// 后端分页拉取全局白名单（Movie/TV）媒体库后**本地一次性比对**，避免前端 N 次请求。
/// 返回 `{ owned: bool[], guids: (string|null)[], libraryCount }`，与入参下标一一对应。
/// 拉库失败直接返回 Err（前端据此隐藏徽章，不阻塞演员页）。
const LIB_MATCH_PAGE_SIZE: usize = 60;
const LIB_MATCH_MAX_PAGES: usize = 20; // 上限 1200 条（库内影视条目通常远低于此）

#[tauri::command]
async fn match_library_works(state: State<'_, AppState>, arg: Value) -> Result<Value, String> {
    let works = arg
        .get("works")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if works.is_empty() {
        return Ok(json!({ "owned": [], "guids": [], "libraryCount": 0 }));
    }
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let mut items: Vec<Value> = Vec::new();
    for page in 1..=LIB_MATCH_MAX_PAGES {
        let v = client
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
                    "page": page,
                    "page_size": LIB_MATCH_PAGE_SIZE
                })),
            )
            .await?;
        let list = v
            .get("list")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let n = list.len();
        items.extend(list);
        // [lc-1727] 空页 / 本页不足一页 → 已到底（与前端 shouldLoadMore 同口径）
        if n == 0 || !should_load_more(items.len(), None, LIB_MATCH_PAGE_SIZE) {
            break;
        }
    }
    let idx = libmatch::build_index(&items);
    let mut owned: Vec<bool> = Vec::with_capacity(works.len());
    let mut guids: Vec<Option<String>> = Vec::with_capacity(works.len());
    for w in &works {
        let key = libmatch::WorkKey::from_json(w);
        let g = libmatch::match_work(&idx, &key);
        owned.push(g.is_some());
        guids.push(g);
    }
    Ok(json!({
        "owned": owned,
        "guids": guids,
        "libraryCount": items.len()
    }))
}

/// 条目详情：端点与响应字段对齐 FlyNarwhal（GET /v/api/v1/item/{guid}，camelCase 字段）
/// [lc-1417] **落盘缓存 120 天**：这是详情页最重的一次请求（简介/海报/演职员/评分全在里面），
/// 原来每次进页面都重拉 —— 用户反馈"每次打开是重新拉了一遍数据吗，要等一会才出现"。
/// [lc-1629] `force` = **跳过缓存读**（仍然写回）：编辑元数据/手动匹配/刷新元数据后，
/// 飞牛侧内容已变，详情页必须拿最新一份，否则标题/简介停在旧值（缓存 120 天，不 force 就看不到改动）。
#[tauri::command]
async fn get_item_detail(
    app: AppHandle,
    state: State<'_, AppState>,
    guid: String,
    force: Option<bool>,
    series_guid: Option<String>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let key = format!("detail:item:{}", guid);
    let mut v = if !force.unwrap_or(false) {
        match disk_cache::json_get(&key, DETAIL_TTL_SECS) {
            Some(v) => v,
            None => {
                let client = fnapi::new_client();
                let path = format!("/v/api/v1/item/{}", guid);
                let v = client.call(&cfg.server, &path, "GET", &cfg.token, None).await?;
                disk_cache::json_put(&key, &v);
                v
            }
        }
    } else {
        let client = fnapi::new_client();
        let path = format!("/v/api/v1/item/{}", guid);
        let v = client.call(&cfg.server, &path, "GET", &cfg.token, None).await?;
        disk_cache::json_put(&key, &v);
        v
    };
    // [lc-1720] 本地手动元数据覆盖（剧根条目全字段；集条目仅补剧级信息）
    metadata_override::apply_to_item(&app, &mut v, series_guid.as_deref());
    Ok(v)
}

/// 全局聚合搜索（飞牛 `GET /v/api/v1/search/list?q=`，对齐 FlyNarwhal
/// `media_remote_data_source.dart:196` 的实际调用方式）。
///
/// 说明：
/// - 上游只接受 `q` 一个查询参数，**没有服务端分类过滤**；`category` 由前端在结果上过滤
///   （与 FlyNarwhal `capsule_search_box.dart:_filterItems` 一致）。这里仅透传回显，便于前端契约稳定。
/// - `page` 同理：该接口一次返回全部命中（`data` 是数组），不做分页；保留参数只为契约兼容。
/// - 返回统一视图：`guid/title/type/poster/year/rating/episodes/seasons/works/overview`，
///   并附 `item`（上游原始对象，供详情页直接复用）。缺失字段留空，不臆造。
#[tauri::command]
async fn search_items(
    app: AppHandle,
    state: State<'_, AppState>,
    query: String,
    category: Option<String>,
    page: Option<i64>,
) -> Result<Value, String> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Ok(json!({
            "list": [], "total": 0, "query": q,
            "category": category, "page": page.unwrap_or(1)
        }));
    }
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let v = match client
        .call_get_query(
            &cfg.server,
            "/v/api/v1/search/list",
            &cfg.token,
            &[("q", q.clone())],
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            // 兼容分支：个别 fnOS 版本的该路由只接受 POST body（历史网页实现）。
            // 仅在「路由未实现/方法不允许」时降级重试，网络/鉴权错误照常上报。
            let lower = e.to_ascii_lowercase();
            let method_rejected = lower.contains("not implemented")
                || lower.contains("method not allowed")
                || lower.contains("404")
                || lower.contains("405");
            if method_rejected {
                client
                    .call(
                        &cfg.server,
                        "/v/api/v1/search/list",
                        "POST",
                        &cfg.token,
                        Some(json!({ "q": q.clone() })),
                    )
                    .await?
            } else {
                return Err(e);
            }
        }
    };
    // 上游 data 直接是数组；兼容个别版本包成 { list: [...] } 的形态。
    let mut resp = if v.is_array() {
        json!({ "list": v })
    } else if v.get("list").is_some() {
        v
    } else {
        json!({ "list": [] })
    };
    // 本地手动元数据覆盖（只读叠加，与 get_items / get_items_global 同口径）
    metadata_override::apply_response_list(&app, &mut resp);
    let mut out: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(list) = resp.get("list").and_then(|x| x.as_array()) {
        for raw in list {
            let view = search_item_view(raw);
            let guid = view.get("guid").and_then(|x| x.as_str()).unwrap_or("");
            // guid 缺失或重复：跳过（搜索接口偶发重复命中）
            if guid.is_empty() || seen.insert(guid.to_string()) {
                out.push(view);
            }
        }
    }
    let total = out.len();
    Ok(json!({
        "list": out,
        "total": total,
        "query": q,
        "category": category,
        "page": page.unwrap_or(1)
    }))
}

fn search_str(o: &serde_json::Map<String, Value>, keys: &[&str]) -> String {
    for k in keys {
        if let Some(v) = o.get(*k) {
            if let Some(s) = v.as_str() {
                let s = s.trim();
                if !s.is_empty() {
                    return s.to_string();
                }
            } else if let Some(n) = v.as_i64() {
                return n.to_string();
            } else if let Some(n) = v.as_f64() {
                return n.to_string();
            }
        }
    }
    String::new()
}

fn search_num(o: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<i64> {
    for k in keys {
        if let Some(v) = o.get(*k) {
            if let Some(n) = v.as_i64() {
                return Some(n);
            }
            if let Some(s) = v.as_str() {
                if let Ok(n) = s.trim().parse::<i64>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// [lc-1726] 取第一个**正数**字段（0/缺失继续找下一个）。
/// 季/集数用：`local_*`（实际入库）为 0 时仍能回退到 `number_*`。
fn search_num_pos(o: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|k| search_num(o, &[*k]).filter(|n| *n > 0))
}

/// 把上游条目归一为搜索下拉用的统一视图；`item` 保留原始对象。
fn search_item_view(raw: &Value) -> Value {
    let empty = serde_json::Map::new();
    let o = raw.as_object().unwrap_or(&empty);
    let guid = search_str(o, &["guid", "id"]);
    let title = search_str(o, &["title", "name", "tv_title"]);
    let ty = search_str(o, &["type"]);
    let mut poster = search_str(o, &["poster", "profile", "avatar", "image"]);
    if poster.is_empty() {
        if let Some(first) = o
            .get("poster_list")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
        {
            if let Some(s) = first.as_str() {
                poster = s.to_string();
            } else if let Some(p) = first.get("path").and_then(|x| x.as_str()) {
                poster = p.to_string();
            }
        }
    }
    let date = search_str(o, &["release_date", "first_air_date", "air_date", "date"]);
    let year = {
        let head: String = date.chars().take(4).collect();
        if head.len() == 4 && head.chars().all(|c| c.is_ascii_digit()) {
            head
        } else {
            String::new()
        }
    };
    let rating = search_str(o, &["vote_average", "rating"]);
    // [lc-1726] 季/集数与海报卡同口径：优先 local_*（实际入库），再回退 number_*。
    let episodes = search_num_pos(o, &["local_number_of_episodes", "number_of_episodes"]);
    let seasons = search_num_pos(o, &["local_number_of_seasons", "number_of_seasons"]);
    let works = search_num(o, &["number_of_item", "number_of_works"]);
    let overview = search_str(o, &["overview", "description", "biography"]);
    json!({
        "guid": guid,
        "title": title,
        "type": ty,
        "poster": poster,
        "year": year,
        "rating": rating,
        "episodes": episodes,
        "seasons": seasons,
        "works": works,
        "overview": overview,
        "item": raw.clone()
    })
}

/// 轮播数据：全局白名单查询（对齐主仓库 itemListApi.ts —— type:['Movie','TV']、
/// 最近更新在前、排除未识别/直播/个人视频），不限定库。
#[tauri::command]
async fn get_carousel_items(app: AppHandle, state: State<'_, AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    let client = fnapi::new_client();
    let mut v = client
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
        .await?;
    metadata_override::apply_response_list(&app, &mut v);
    Ok(v)
}

/* ───────────────── [版本] 同一剧/集的多版本（多文件）─────────────────
   照抄 FlyNarwhal 的语义与展示方式（lib/ui/features/movie_detail/movie_detail_screen.dart
   的 `_MediaSourceBoxes` / `_initializeSelection` / `_onVideoStreamSelected`）：

   · **识别**：不是客户端按剧名/季集/文件名去猜，而是飞牛服务端已经把「同一剧集的两个版本」
     归到同一个 item 下；`GET /v/api/v1/stream/list/{guid}` 的 `video_streams[]` 会为每个
     媒体文件返回一条记录（各自的 `media_guid`）。客户端判定「多版本」= video_streams.len() > 1。
   · **展示**：详情页把每条视频流渲染成一个可点选的「版本盒子」，标签为
     `分辨率大写 + 色域`（`color_range_type == "DolbyVision"` 显示「杜比视界」）。
   · **默认版本**：`POST /v/api/v1/play/info {item_guid}` 返回的 `media_guid`
     （服务端记住的上次播放版本；无记录时是服务端默认版本）；没有则取第一条。
   · **播放**：把选中的 `media_guid` 回传给 `play/info`（见 resolve_play_guid），
     服务端据此返回该文件对应的拉流 guid / 音频字幕流。

   本命令只读，不写任何服务端状态。 */
#[tauri::command]
async fn get_media_versions(state: State<'_, AppState>, guid: String) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    if cfg.token.is_empty() {
        return Err("未登录".into());
    }
    let client = fnapi::new_client();
    let path = format!("/v/api/v1/stream/list/{}", guid);
    let raw = client
        .call(&cfg.server, &path, "GET", &cfg.token, None)
        .await?;
    // 兼容两种响应形态：{code,data:{...}} 或直接 {...}
    let data = raw.get("data").cloned().unwrap_or(raw);
    let streams = data
        .get("video_streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let str_of = |s: &Value, k: &str| {
        s.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let num_of = |s: &Value, k: &str| {
        s.get(k)
            .and_then(|v| v.as_i64())
            .or_else(|| s.get(k).and_then(|v| v.as_f64()).map(|f| f as i64))
            .unwrap_or(0)
    };
    // [版本文案] 上游 `video_streams[].title` 实测恒为空、云端/STRM 条目分辨率与色域也空，
    // 唯一可区分信息在同响应的 `files[]`：`files[].guid === video_streams[].media_guid`。
    // 这里只取 `file_name`（绝不带 `path`，避免把 NAS 目录暴露给前端）。
    let file_names: std::collections::HashMap<String, String> = data
        .get("files")
        .and_then(|v| v.as_array())
        .map(|files| {
            files
                .iter()
                .filter_map(|f| {
                    let g = str_of(f, "guid");
                    let name = str_of(f, "file_name");
                    if g.is_empty() || name.is_empty() {
                        None
                    } else {
                        Some((g, name))
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let versions: Vec<Value> = streams
        .iter()
        .map(|s| {
            let media_guid = str_of(s, "media_guid");
            json!({
                "media_guid": media_guid,
                "file_name": file_names.get(&media_guid).cloned().unwrap_or_default(),
                "title": str_of(s, "title"),
                "resolution_type": str_of(s, "resolution_type"),
                "color_range_type": str_of(s, "color_range_type"),
                "codec_name": str_of(s, "codec_name"),
                "codec_type": str_of(s, "codec_type"),
                "width": num_of(s, "width"),
                "height": num_of(s, "height"),
                "bit_depth": num_of(s, "bit_depth"),
                "dv_profile": num_of(s, "dv_profile"),
                "bps": num_of(s, "bps"),
                "duration": num_of(s, "duration"),
                "is_bluray": s.get("is_bluray").and_then(|v| v.as_bool()).unwrap_or(false),
                // 与 FlyNarwhal `VideoStream.isNotExist` 同口径（ext1 == -1 为 STRM 等占位流）
                "is_not_exist": num_of(s, "ext1") == -1,
            })
        })
        .filter(|v| v.get("media_guid").and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false))
        .collect();
    // 默认选中 = play/info 的 media_guid（服务端记住的上次播放版本）。
    // 失败不致命：前端回落到本地记忆 / 第一条。
    let selected_media_guid = client
        .call(
            &cfg.server,
            "/v/api/v1/play/info",
            "POST",
            &cfg.token,
            Some(json!({ "item_guid": guid })),
        )
        .await
        .ok()
        .and_then(|p| {
            p.get("media_guid")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_default();
    Ok(json!({
        "versions": versions,
        "selected_media_guid": selected_media_guid,
    }))
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


/// [lc-1615] 播放解析结果。原先是 `(guid, type, title)` 三元组；观影记录要在「成功起播」
/// 那一刻落盘海报/锚点 id/季集号等元数据，而 play/info 的响应本就包含它们 —— 就地扩展，
/// 免得起播后再补一次网络请求。
struct PlayResolution {
    /// 拉流用 guid（Episode/Movie）
    play_guid: String,
    /// [progress] 该条目的媒体/流 guid（play/info 顶层字段）——回传飞牛 `play/record` 必需。
    /// 缺任何一个都只影响「继续观看」回传，不影响本地播放/本地历史。
    media_guid: String,
    video_guid: String,
    audio_guid: String,
    subtitle_guid: String,
    /// 类型（Movie / Episode / ...）
    kind: String,
    /// 展示标题（`剧名 - SxxEyy: 集名` 形态，供 mpv force-media-title）
    media_title: String,
    /// 剧名（电影为空）
    tv_title: String,
    /// 集名（item.title；电影 = 片名）
    ep_title: String,
    /// 海报（NAS 相对路径；可能为空）
    poster: String,
    /// trim_id 锚点解析出的 TMDB / Bangumi id（可能为空）
    tmdb_id: String,
    bgm_id: String,
    /// 季号 / 集号（0 = 无）
    season: i64,
    episode: i64,
    /// 所属季 guid（剧集 = play/info 的 parent_guid；电影为空）
    parent_guid: String,
    /// [speed] **剧级** guid（跨季也同一部剧）：play/info 顶层或 item 上的
    /// `series_guid`/`seriesGuid`。飞牛多数版本没有该字段 → 空串，倍速键回退到
    /// 「季 guid / 剧名」规则（与 `history::series_key_of` 一致）。
    series_guid: String,
    /// [lc-1631] 条目总时长（ms；0 = 未知）。play/info 的 item.duration（秒）折算，
    /// 年度报告按「进度 × 总时长」折算观看量时需要。
    duration_ms: u64,
    /// [live] 电视直播直链（`play/info.live_channels[].path`）。
    ///
    /// 直播**不能**走 Go 代理 `playvideo`（该接口对直播返回 500，旧项目 Fntv-Plus
    /// `media.ts:926-941` 的 lc-631/lc-632 结论相同），必须把 path 原样交给播放器。
    /// `Some` 时 `play_guid`/`media_guid` 等进度字段均无意义。
    live_url: Option<String>,
}

/// [live] 从 `play/info` 响应里挑选直播线路，返回 `(path, 线路名)`。
///
/// 规则（照抄旧项目 Fntv-Plus `processSingleMedia`）：
///   1. 优先第一条 `can_play != 0`（缺省视为可播）且有非空 `path` 的线路；
///   2. 若没有可播标记，退回第一条有非空 `path` 的线路；
///   3. 完全没有 `live_channels` / 无可用 path → `None`（按普通媒体走代理）。
fn pick_live_channel(info: &Value) -> Option<(String, Option<String>)> {
    let arr = info.get("live_channels")?.as_array()?;
    let has_path = |c: &&Value| {
        c.get("path")
            .and_then(|p| p.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    };
    let chan = arr
        .iter()
        .find(|c| {
            has_path(c)
                && c.get("can_play")
                    .and_then(|x| x.as_i64())
                    .map(|n| n != 0)
                    .unwrap_or(true)
        })
        .or_else(|| arr.iter().find(has_path))?;
    let path = chan.get("path")?.as_str()?.trim().to_string();
    if path.is_empty() {
        return None;
    }
    let name = chan
        .get("file_name")
        .and_then(|p| p.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some((path, name))
}

/// [lc-105] 解析「真正要拉流的 guid」：`POST /v/api/v1/play/info {item_guid}` → 取回 `data.guid`
/// （Episode/Movie）。**mpv 路径与页面内 `<video>` 路径共用这一份**（lc-102/103 实测结论：
/// 代理只认具体媒体的 guid，给容器 guid 会 500）。会话失效时会用保存的密码静默重登一次。
async fn resolve_play_guid(
    app: &AppHandle,
    state: &AppState,
    cfg: &mut Config,
    guid: &str,
    media_guid: Option<&str>,
) -> Result<PlayResolution, String> {
    let client = fnapi::new_client();
    // [版本] 照抄 FlyNarwhal：选中某个版本时把它的 `media_guid` 一并交给 play/info，
    // 服务端返回该媒体文件对应的拉流 guid / 音频字幕流；不传则由服务端按
    // 「上次播放的版本」/ 默认版本决定（与飞鲸详情页的默认选中逻辑一致）。
    let media_guid = media_guid.map(|s| s.trim()).filter(|s| !s.is_empty());
    let body = || {
        let mut b = json!({ "item_guid": guid });
        if let Some(mg) = media_guid {
            b["media_guid"] = json!(mg);
        }
        b
    };
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
                    // 部分版本把集号/季号写成字符串（"3"），别因此退化成 0。
                    .or_else(|| {
                        it.get(k)
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.trim().parse::<i64>().ok())
                    })
                    .unwrap_or(0)
            };
            // [progress] 飞牛 `play/record` 需要的流 guid（play/info 顶层，实测同旧项目）。
            let top_s = |k: &str| {
                info.get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string()
            };
            // [live] 电视直播：play/info 顶层 `live_channels[]` 才是可播地址
            // （如 `http://192.168.31.170:1905/{streamId}`，网关 302 → 外部 CDN HLS）。
            // 与旧项目 Fntv-Plus `src/main/handlers/plugins/media.ts:926-941` 一致。
            let live = pick_live_channel(&info);
            let live_url = live.as_ref().map(|(u, _)| u.clone());
            let live_name = live.as_ref().and_then(|(_, n)| n.clone());
            if let Some(u) = &live_url {
                libmpv::trace(&format!(
                    "[play/info] 直播线路: {} → {}",
                    live_name.as_deref().unwrap_or("线路"),
                    &u.chars().take(120).collect::<String>()
                ));
            }
            let ep_title = get_s("title")
                .or_else(|| get_s("name"))
                // 最后兜底：个别条目只有文件名（带扩展名，聊胜于无）。
                .or_else(|| get_s("file_name"))
                .or_else(|| {
                    let t = top_s("title");
                    (!t.is_empty()).then_some(t)
                })
                .or_else(|| {
                    let t = top_s("name");
                    (!t.is_empty()).then_some(t)
                })
                .or_else(|| live_name.clone())
                .unwrap_or_default();
            // [title] 剧名来源：`item.tv_title` 并非每集都有；飞牛 `play/info` 的条目上
            // 更常见的字段是 `item.parent_title`（旧项目 skip/douban 同步都用
            // `parent_title || title`）。只读 tv_title 会让剧集标题退化成「只剩集标题」，
            // 弹幕按番名匹配自然失败。这里按 tv_title → parent_title → 顶层同名字段逐级兜底。
            let tv_title = get_s("tv_title")
                .or_else(|| get_s("parent_title"))
                .or_else(|| {
                    let t = top_s("parent_title");
                    (!t.is_empty()).then_some(t)
                })
                .or_else(|| {
                    let t = top_s("tv_title");
                    (!t.is_empty()).then_some(t)
                });
            // 季/集号同样做 snake_case / camelCase 双格式兼容（飞牛各版本不一致）。
            let season = get_n("season_number").max(get_n("seasonNumber"));
            let episode = get_n("episode_number").max(get_n("episodeNumber"));
            // [lc-1561] 季/集号缺失时不拼 "S0E0"：那会在弹幕面板的番名里留下 "- : " 残渣
            // （fnOS 未提供 season/episode 时 season=episode=0 → "番名 - S0E0: 集名" →
            // uosc_danmaku 的 bili_fast_parse 剥掉 "S0E0" 后只剩 "番名 - : 集名"）。
            // 现在统一交给 `playlist_episode_title`：集号 > 0 才拼 SxxEyy（季号缺省记 0，
            // 特别篇也能看出集号），集名缺失时保留 SxxEyy 而不留裸冒号。
            // [lc-1615] tv_title 之后还要进历史记录，这里只借用不移动。
            let media_title = playlist_episode_title(
                tv_title.as_deref().unwrap_or(""),
                season,
                episode,
                &ep_title,
            );
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
            // [speed] 剧级 guid（优先于季 guid，跨季也能同剧共享倍速）：
            // 飞牛多数版本的 play/info 不带这个字段 → 空串，调用方回退季 guid。
            let series_guid = info
                .get("series_guid")
                .and_then(|x| x.as_str())
                .or_else(|| info.get("seriesGuid").and_then(|x| x.as_str()))
                .or_else(|| it.get("series_guid").and_then(|x| x.as_str()))
                .or_else(|| it.get("seriesGuid").and_then(|x| x.as_str()))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default();
            // [live] 直播无剧集概念，不写弹幕匹配元数据（避免 uosc_danmaku 空搜）。
            if live_url.is_none() {
                danmaku_shim::write_play_meta(&g, &parent_guid, &media_title, season, episode);
            }
            libmpv::trace(&format!(
                "[play/info] ok type={t} 原始={guid} → 拉流用={g} 标题={media_title:?} 系列键={parent_guid}"
            ));
            if matches!(t, "TV" | "Season" | "Collection" | "Directory" | "Folder") {
                return Err(format!(
                    "「{t}」没有可播放的具体媒体 —— 请在选集里点具体一集再播放"
                ));
            }
            // [lc-1615] 历史记录元数据（海报/锚点 id）就地取：play/info 响应里现成，不用二次请求。
            let poster = history::pick_img(it.get("posters"));
            let (tmdb_id, bgm_id) = history::parse_trim_ids(&get_s("trim_id").unwrap_or_default());
            // [lc-1631] 总时长（秒 → 毫秒）：年度报告折算观看量的依据。
            let dur_sec = get_n("duration");
            let duration_ms = if dur_sec > 0 { (dur_sec as u64) * 1000 } else { 0 };
            let (media_guid, video_guid, audio_guid, subtitle_guid) = (
                top_s("media_guid"),
                top_s("video_guid"),
                top_s("audio_guid"),
                top_s("subtitle_guid"),
            );
            Ok(PlayResolution {
                play_guid: g,
                media_guid,
                video_guid,
                audio_guid,
                subtitle_guid,
                kind: if live_url.is_some() {
                    "LiveChannel".to_string()
                } else {
                    t.to_string()
                },
                media_title,
                tv_title: tv_title.unwrap_or_default(),
                ep_title,
                poster,
                tmdb_id,
                bgm_id,
                season,
                episode,
                parent_guid,
                series_guid,
                duration_ms,
                live_url,
            })
        }
        Err(e) => {
            libmpv::trace(&format!("[play/info] 失败: {e}"));
            Err(format!("获取播放信息失败：{e}"))
        }
    }
}

/// [playlist] 播放列表条目（整季连播用）。
#[derive(Clone)]
struct PlaylistEntry {
    guid: String,
    title: String,
    duration_secs: f64,
    /// [theintrodb] 该集的季/集号（0 = episode/list 未提供）：随 skip-metadata 下发给
    /// smart_skip，使整季连播时每一集都能按自己的集号查片头片尾，而不是沿用起播集。
    season: i64,
    episode: i64,
}

/// [playlist] 拼单集展示标题（`resolve_play_guid` 的 media_title 与整季 #EXTINF 共用）：
/// `剧名 - SxxEyy: 集名`；集名为空时保留 `剧名 - SxxEyy`（不留下裸的 `": "`）；
/// 季集号缺失时退化为 `剧名 - 集名` / `剧名` / 集名。
///
/// 判据是 **集号 > 0**（不是「季号 > 0 且集号 > 0」）：飞牛对部分剧集只给
/// `episode_number`、季号缺省为 0（特别篇/未分季），旧写法会把 `S0E3` 整个丢掉，
/// 弹幕脚本与 uosc 就看不出这是第几集。电影/个人视频的集号是 0，因此不会出现 `S0E0`。
///
/// [title] 拼装前先把剧名/集名转简体：本函数是 `media_title`、整季 `#EXTINF`、
/// `write_play_meta` 展示字段与 PotPlayer 标题的**共同源头**，在这里转换即可一次覆盖，
/// 且格式（`剧名 - S1E3: 集名`）不受影响。`PlayResolution` 里的原始 `tv_title`/
/// `ep_title` 保持繁体原样，继续供弹幕/跳过/历史匹配使用。
fn playlist_episode_title(tv: &str, season: i64, episode: i64, ep_title: &str) -> String {
    let tv = zh::to_simplified(tv.trim());
    let ep_title = zh::to_simplified(ep_title.trim());
    let tv = tv.as_str();
    let ep_title = ep_title.as_str();
    let has_ep = episode > 0;
    let se = if has_ep {
        format!("S{}E{}", season.max(0), episode)
    } else {
        String::new()
    };
    match (tv.is_empty(), has_ep, ep_title.is_empty()) {
        (false, true, false) => format!("{tv} - {se}: {ep_title}"),
        (false, true, true) => format!("{tv} - {se}"),
        (false, false, false) => format!("{tv} - {ep_title}"),
        (false, false, true) => tv.to_string(),
        (true, true, false) => format!("{se}: {ep_title}"),
        (true, true, true) => se,
        (true, false, _) => ep_title.to_string(),
    }
}

/// [playlist] 拉整季集列表（优先命中详情页同一份 10 分钟磁盘缓存，避免重复请求）。
async fn season_episode_values(cfg: &Config, season_guid: &str) -> Result<Vec<Value>, String> {
    let key = format!("detail:episodes:{}", season_guid);
    if let Some(v) = disk_cache::json_get(&key, DETAIL_LIST_TTL_SECS) {
        if let Some(arr) = v.as_array() {
            return Ok(arr.clone());
        }
    }
    let client = fnapi::new_client();
    let path = format!("/v/api/v1/episode/list/{}", season_guid);
    let v = client.call(&cfg.server, &path, "GET", &cfg.token, None).await?;
    disk_cache::json_put(&key, &v);
    Ok(v.as_array().cloned().unwrap_or_default())
}

/// [playlist] 构建多条目 M3U 播放列表：每条 = 本机 Go 代理 URL（含 token），
/// `#EXTINF` 标题用于 mpv/PotPlayer 的 media-title 与播放列表面板显示。
///
/// URL 时效说明：这里写入的是 `http://127.0.0.1:22346/api/v1/playvideo/<guid>` 形式的
/// **本机代理地址**，不是源站直链；真正的云盘/网盘直链由 Go 代理在 mpv 请求该条目时
/// 按需解析。因此一次性构建整季列表是安全的（不会提前消耗/过期任何直链），
/// 上下集切换也不需要后端再解析一次。token 失效时由既有 end-file error 链路提示。
fn build_playlist_m3u(cfg: &Config, entries: &[PlaylistEntry]) -> String {
    let mut s = String::from("#EXTM3U\n");
    for e in entries {
        let title = e.title.replace(['\r', '\n'], " ");
        let dur = if e.duration_secs > 0.0 {
            e.duration_secs.round() as i64
        } else {
            -1
        };
        s.push_str(&format!("#EXTINF:{dur},{title}\n"));
        s.push_str(&proxy::play_url(cfg, &e.guid));
        s.push('\n');
    }
    s
}

/// [theintrodb] 组装 smart_skip 的 `skip-metadata` 载荷（guid → 元数据）。
///
/// 键必须与 Lua 侧 `mutils.extract_id_and_query` 的解析结果一致：它从播放地址
/// `/api/v1/playvideo/<guid>?...` 里取 guid。整季播放列表每条都是本机代理 URL，
/// 所以键正好是 `PlaylistEntry::guid`；单条播放则用 `play/info` 返回的 `play_guid`。
///
/// 值：`{ tmdb, season?, episode? }`。tmdb 为空或非数字时返回**空对象** ——
/// 没有 TMDB 锚点就查不到 theintrodb，下发空表不会误触发（Lua 侧 `meta == nil` 时静默等待）。
fn build_skip_metadata(
    tmdb_id: &str,
    current_guid: &str,
    season: i64,
    episode: i64,
    entries: &[PlaylistEntry],
) -> Value {
    let tmdb = tmdb_id.trim();
    let mut map = serde_json::Map::new();
    if tmdb.is_empty() || !tmdb.bytes().all(|b| b.is_ascii_digit()) {
        return Value::Object(map);
    }
    let meta = |season: i64, episode: i64| {
        let mut m = serde_json::Map::new();
        m.insert("tmdb".to_string(), json!(tmdb));
        // 0 = 未知：省略该键，Lua 侧按「无季集号」查询（电影/单集不拼 S0E0）。
        if season > 0 {
            m.insert("season".to_string(), json!(season));
        }
        if episode > 0 {
            m.insert("episode".to_string(), json!(episode));
        }
        Value::Object(m)
    };
    if entries.is_empty() {
        if !current_guid.is_empty() {
            map.insert(current_guid.to_string(), meta(season, episode));
        }
    } else {
        for e in entries {
            if !e.guid.is_empty() {
                map.insert(e.guid.clone(), meta(e.season, e.episode));
            }
        }
    }
    Value::Object(map)
}

/// [theintrodb] 把 skip 元数据推给 mpv 内的 smart_skip 脚本（fire-and-forget）。
///
/// 用 `script-message skip-metadata <json>` 而不是写配置/重启脚本：
///   · 脚本在 `file-loaded` 时按当前播放地址的 guid 查表，整季列表一次下发即可逐集命中；
///   · 失败（mpv 已退出/管道断开）只影响兜底，不影响播放；
///   · 空表不发，避免无意义的脚本消息。
async fn push_skip_metadata(state: &AppState, payload: Value) {
    // 空对象也要发：脚本侧 `skip_meta_map = meta` 是**整表替换**，
    // 发空表可清掉上一部片的残留，避免换片后仍按旧表查（换集/换片必须同步）。
    if !payload.is_object() {
        return;
    }
    // 先克隆 Arc 再释放锁：command_no_reply 内部可能等待写管道，不占着 state.mpv。
    let session = state.mpv.lock().await.clone();
    if let Some(m) = session {
        m.command_no_reply(json!([
            "script-message",
            "skip-metadata",
            payload.to_string()
        ]))
        .await;
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
    let res = resolve_play_guid(&app, &state, &mut cfg, &guid, None).await?;
    // [live] 直播直链不走 Go 代理（当前前端未使用本入口，仍保持一致语义）。
    let url = match res.live_url.clone() {
        Some(live) => live,
        None => {
            proxy::ensure(&cfg.proxy_path, &state.proxy_child).await?;
            proxy::play_url(&cfg, &res.play_guid)
        }
    };
    Ok(json!({
        "url": url,
        "guid": res.play_guid,
        "type": res.kind,
    }))
}

#[tauri::command]
async fn play_item(
    state: State<'_, AppState>,
    app: AppHandle,
    guid: String,
    media_guid: Option<String>,
) -> Result<(), String> {
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
    let res = resolve_play_guid(&app, &state, &mut cfg, &guid, media_guid.as_deref()).await?;
    stage("guid-resolved");
    // [live] 直播直链：不经 Go 代理，也不回传进度（直播没有 media_guid/duration）。
    let is_live = res.live_url.is_some();
    if is_live {
        fnos_progress::begin("", "");
    } else {
        // [progress] 起播即记住这一集的流 guid：首帧进度回传时无需再发一次 play/info。
        fnos_progress::remember(fnos_progress::PlayMeta {
            item_guid: res.play_guid.clone(),
            media_guid: res.media_guid.clone(),
            video_guid: res.video_guid.clone(),
            audio_guid: res.audio_guid.clone(),
            subtitle_guid: res.subtitle_guid.clone(),
        });
        // 退出兜底跟踪器：series guid = 本地历史记录键（下面 history::record 用的同一个 guid）。
        //
        // [speed] 同时算一个**剧级倍速键**（与 history 去重键同一套规则）：
        // 同一部剧的所有集共用一个键 → 任意一集调的倍速，切到同剧其它集沿用；
        // 换剧则用该剧自己的记录（无记录回 1.0）。拿不到剧级标识（直播 / 本地文件 /
        // 在线源）时为空串 → playback_speed 不持久化，保持 1.0。
        //   · guid 用 play/info 解析出的实际条目 guid（电影 = 自身）；
        //   · title 只取「剧名」层级（tv_title → 顶层 media_title），绝不用集标题 ——
        //     否则同剧各集会被拆成不同键，又退回「按集」。
        let speed_key = if !res.series_guid.is_empty() {
            // 有剧级 guid（跨季同一部剧）→ 最精确的剧级键。
            format!("series:{}", res.series_guid)
        } else {
            history::series_key_from_parts(
                &res.play_guid,
                &res.parent_guid,
                &res.kind,
                &res.tmdb_id,
                &res.bgm_id,
                if !res.tv_title.is_empty() {
                    &res.tv_title
                } else {
                    &res.media_title
                },
            )
        };
        fnos_progress::begin_with_speed_key(&guid, &res.play_guid, &speed_key);
    }
    let direct_url = if let Some(live) = res.live_url.clone() {
        live
    } else {
        proxy::ensure(&cfg.proxy_path, &state.proxy_child).await?;
        stage("proxy-ready");
        proxy::play_url(&cfg, &res.play_guid)
    };

    let display_title: String = res
        .media_title
        .replace(['\u{300E}', '\u{300F}', '\u{300C}', '\u{300D}', '\u{3010}', '\u{3011}', '\u{3014}', '\u{3015}', '\u{300A}', '\u{300B}', '\u{3008}', '\u{3009}', '\u{201C}', '\u{201D}', '\u{0027}', '\u{2019}'], "")
        .trim()
        .to_string();

    // 播放列表：剧集 → 拉整季 `episode/list` 构建多条目列表；电影/单集/列表拉取失败 →
    // 回退单条目（行为与修复前一致，确保单集电影不受影响）。
    let (kind, _player_path) = player::resolve_player(&cfg)?;
    let mut entries: Vec<PlaylistEntry> = Vec::new();
    let mut start_index = 0usize;
    if res.kind == "Episode" && !res.parent_guid.is_empty() {
        match season_episode_values(&cfg, &res.parent_guid).await {
            Ok(list) if list.len() > 1 => {
                for ep in &list {
                    let g = ep
                        .get("guid")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if g.is_empty() {
                        continue;
                    }
                    // [title] 逐集字段兼容：飞牛 `episode/list` 各版本字段名不一致
                    // （snake_case / camelCase），集标题还可能只在 `name` / `file_name` 上。
                    // 取不到时按「缺省」处理：季号回落起播集的季号，集号/集名留空
                    // （宁可只显示剧名，也不要凭空造一个集号去喂 skip-metadata）。
                    let num_of = |keys: &[&str]| -> Option<i64> {
                        keys.iter().find_map(|k| {
                            ep.get(*k).and_then(|v| {
                                v.as_i64().or_else(|| {
                                    v.as_str().and_then(|s| s.trim().parse::<i64>().ok())
                                })
                            })
                        })
                    };
                    let se = num_of(&["season_number", "seasonNumber"]).unwrap_or(res.season);
                    let no = num_of(&["episode_number", "episodeNumber"]).unwrap_or(0);
                    let t = ["title", "name", "file_name", "fileName"]
                        .iter()
                        .find_map(|k| {
                            ep.get(*k)
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                        })
                        .unwrap_or("")
                        .to_string();
                    let dur = ep
                        .get("duration")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    entries.push(PlaylistEntry {
                        guid: g,
                        title: playlist_episode_title(&res.tv_title, se, no, &t),
                        duration_secs: dur,
                        season: se,
                        episode: no,
                    });
                }
                // 定位被点击的那一集：play/info 返回的 guid 与 episode/list 的 guid 通常一致；
                // 两者任一命中即可。找不到就整季回退单集，避免"点了 A 却从第 1 集播"。
                match entries
                    .iter()
                    .position(|e| e.guid == res.play_guid)
                    .or_else(|| entries.iter().position(|e| e.guid == guid))
                {
                    Some(idx) => start_index = idx,
                    None => {
                        libmpv::trace("[playlist] 当前集不在 episode/list 中，回退单集播放");
                        entries.clear();
                    }
                }
            }
            Ok(_) => {}
            Err(e) => libmpv::trace(&format!("[playlist] 拉取整季集列表失败，回退单集: {e}")),
        }
    }
    let multi = entries.len() > 1;

    // 自带 mpv：多条目 M3U（自然顺序 + --playlist-start 定位），单条沿用原单条目 M3U8；
    // PotPlayer：一律播「点击集」的代理直链（relay 包可读名）。
    //
    // [lc-1751] 旧版在 PotPlayer 多集时把**本地 m3u 文件路径**交给 player::launch ——
    // launch_potplayer 会把它注册进本地 relay，relay 再拿 reqwest 去取 `C:\...\anibox-playlist.m3u`
    // 这种 Windows 路径 → reqwest URL 构建直接报错 → 每个请求都 502，PotPlayer 窗口报
    // 「服务已关闭或地址错误」（真机日志：19 连发 `转发失败 … builder error for url`）。
    // 对标原项目 Fntv-Plus potplayer.ts（lc-297）：PotPlayer 起播只带**单集** shim URL，
    // 其余集在窗口就绪后用 `/Current /add` 追加进面板（见下方 panel append），从不喂 m3u 文件。
    let mut playlist_start: Option<usize> = None;
    let url = if is_live {
        // [live] 直播：直链原样交给播放器（mpv 自己跟随 302 拉 HLS；
        // PotPlayer 由 player::launch 的 direct 分支跳过 relay）。
        direct_url.clone()
    } else if kind == "potplayer" {
        // [lc-1751] PotPlayer（单集/多集一致）：播「点击集」的代理直链（launch 内经 relay
        // 包可读名 URL）；整季面板其余集在起播后经 `/Current /add` 追加（见下方 panel append）。
        direct_url.clone()
    } else if multi {
        let path = std::env::temp_dir().join("anibox-playlist.m3u");
        let m3u = build_playlist_m3u(&cfg, &entries);
        std::fs::write(&path, m3u).map_err(|e| format!("写播放列表失败: {e}"))?;
        playlist_start = Some(start_index);
        path.to_string_lossy().to_string()
    } else {
        let m3u8_path = std::env::temp_dir().join("anibox-play.m3u8");
        let m3u8 = format!("#EXTM3U\n#EXTINF:-1,{}\n{}\n", display_title, direct_url);
        std::fs::write(&m3u8_path, m3u8).map_err(|e| format!("写播放列表失败: {e}"))?;
        m3u8_path.to_string_lossy().to_string()
    };
    stage(if multi { "playlist-prepared" } else { "url-prepared" });

    // ② 统一入口：mpv 内部复用会话/新建；PotPlayer 逐集拉起。
    let duration_secs = if res.duration_ms > 0 {
        res.duration_ms as f64 / 1000.0
    } else {
        0.0
    };
    // [lc-1751] PotPlayer 的 relay 可读名（PotPlayer 把 URL 末段当标题显示）：多集时用
    // 「剧名 - S01E03: 集标题」逐集形态（对标原项目 shim 名），其余沿用剧名。
    let player_title = if kind == "potplayer" && !is_live && multi {
        entries
            .get(start_index)
            .map(|e| e.title.clone())
            .unwrap_or_else(|| display_title.clone())
    } else {
        display_title.clone()
    };
    let launch_res = player::launch(
        &app,
        &state,
        &cfg,
        &url,
        &player_title,
        0.0,
        duration_secs,
        None,
        &[],
        playlist_start,
        is_live,
    )
    .await;
    stage("player-launched");

    // [lc-1615] 落一笔本机播放历史（观影记录页数据源）：走到这里 = 已解析出可播地址并
    // 交给播放器（URL 已在启动参数里，mpv 一启动即开始加载）。
    // [lc-1710] 先记录、后传播播放器错误：mpv 的 IPC 连接超时（mpv.rs）或 PotPlayer
    // 启动失败时，用户其实已经看到播放窗口/已起播，若因 `?` 提前返回就会丢记录 ——
    // 新规则要求「只要起播就必须有观影记录」，故此处不再受 launch 结果影响。
    history::record(
        &app,
        history::WatchRecord {
            // 条目 guid：仅用于回填对位/打开详情；去重由 history 内部按剧级 series_key 完成
            guid: guid.clone(),
            title: if !res.tv_title.is_empty() {
                res.tv_title.clone()
            } else if !res.ep_title.is_empty() {
                res.ep_title.clone()
            } else {
                res.media_title.clone()
            },
            episode: history::episode_label(res.season, res.episode),
            poster: res.poster.clone(),
            tmdb_id: res.tmdb_id.clone(),
            bgm_id: res.bgm_id.clone(),
            media_type: res.kind.clone(),
            played_at: history::now_ms(),
            source: "fnos".into(),
            source_name: String::new(),
            parent_guid: res.parent_guid.clone(),
            // [lc-1631] 起播时能拿到的真实字段：条目总时长。实际观看时长（watched_ms）
            // 要播完才知道 → 留 0，由前端会话计时在播放结束回填（record_watch_session）。
            duration_ms: res.duration_ms,
            watched_ms: 0,
            progress: 0.0,
            finished: false,
            ..Default::default()
        },
    );
    // 记录已落盘，再决定是否向调用方报播放器启动错误。
    launch_res?;

    // [lc-1751] PotPlayer 整季面板追加（对标 Fntv-Plus potplayer.ts populatePlaylistPanel，
    // lc-297）：起播只带点击集；窗口就绪后把其余集以可读名 relay URL 经 `/Current /add`
    // 追加进 PotPlayer 播放列表面板 —— 面板显示整季、本集播完由 PotPlayer 顺序连播。
    // 库内播放经 Go 代理自带鉴权（token 在 URL 里），relay 目标无需额外播放头。
    // 延时 + 代次号双保险：窗口未就绪时 /Current 会丢；用户已切下一部时追加必须放弃。
    if kind == "potplayer" && multi && !is_live {
        let urls: Vec<String> = player::panel_append_order(entries.len(), start_index)
            .into_iter()
            .filter_map(|i| entries.get(i))
            .map(|e| {
                let id = player::relay_register(player::RelayTarget {
                    url: proxy::play_url(&cfg, &e.guid),
                    headers: player::HttpHeaders::default(),
                });
                player::relay_url_named_ext(&id, &e.title, "mp4")
            })
            .collect();
        if !urls.is_empty() {
            crate::libmpv::trace(&format!(
                "[playlist] PotPlayer 面板追加安排：{} 集（点击集={}）",
                urls.len(),
                start_index
            ));
            player::schedule_potplayer_panel_append(
                _player_path.clone(),
                urls,
                player::potplayer_generation(),
            );
        }
    }

    // [theintrodb] 真实接入：把「guid → TMDB/季/集」下发给 mpv 内的 smart_skip 脚本。
    //
    // 触发链：这里 `script-message skip-metadata` → `scripts/smart_skip/main.lua`
    // 的 `register_script_message('skip-metadata')` → 写入 `skip_meta_map` →
    // `file-loaded` 时 `load_server_config` 在飞牛无跳过数据的情况下调用
    // `try_theintrodb_fallback` → `api.get_theintrodb` 请求 v4 `/media?tmdb_id=...`。
    //
    // 整季播放列表一次下发全部集：mpv 自动切集时 `path` 变成下一条代理 URL，
    // Lua 按 guid 查表自然取到新一集的季集号，不会沿用上一集。
    // 失败安全：消息只是「尽力而为」，mpv 未就绪/已退出都只影响兜底，不影响播放。
    if kind == "mpv" && !is_live {
        let payload = build_skip_metadata(
            &res.tmdb_id,
            &res.play_guid,
            res.season,
            res.episode,
            &entries,
        );
        push_skip_metadata(&state, payload).await;
    }

    // ③ [lc-1557] probe 只在后台跑，**不挡起播**。
    //
    // probe 会对代理直链发一次真实 GET —— NAS/网络慢时要等 20s（reqwest 超时上限），
    // 期间用户对着 idle 品牌页干等。现在 loadfile 的入口是**本地 m3u8 文件**（秒开），
    // mpv 打开内层直链失败时本就会走 end-file error → 我们已有的错误展示链路，
    // probe 的"提前报错"价值不再值得它挡路。
    //
    // 两条路谁先到都行：probe 若先于 mpv 发现代理 500，照样把代理给的明确文案 emit 给前端。
    // [live] 直播不预检：源是持续流/302 网关，GET 预检没有意义，还可能误报错误。
    if !is_live {
        let probe_url = direct_url.clone();
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

    // 在线源站点头的清理由 player::launch 的复用分支负责（fire-and-forget），这里不再直接操作会话。
    Ok(())
}

// [lc-100] 自绘浮层控制条（bar.html / playerbar 窗口）已**整体删除**。
// 它从 lc-090 起就默认关闭且架构上无法被唤起（视频区内鼠标归 mpv），属于纯死代码：
// 一个额外窗口 + 一个额外页面 + 一条懒创建路径，还曾经自己冒出来（lc-090 的 bug）。
// 底栏统一由 mpv 原生 uosc 提供；需要的话从 git 历史里取回（lc-099 之前的版本）。

#[tauri::command]
async fn player_stop(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // mpv：回 idle（实例保留）；PotPlayer：无 IPC，直接结束进程并补 ended 事件。
    player::stop_all(&app, &state, true).await;
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

/// [playlist] 下一集 / 上一集：直接操作 mpv 播放列表（uosc 底栏的「上一集/下一集」按钮
/// 走的就是 input.conf 里的 `playlist-prev`/`playlist-next`；这两个命令供手柄/页面调用）。
/// 单集场景下 mpv 的 playlist-next/prev 会返回 success 但不动作，属预期。
#[tauri::command]
async fn mpv_next_episode(state: State<'_, AppState>) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        m.command(json!(["playlist-next"])).await?;
    }
    Ok(())
}

#[tauri::command]
async fn mpv_prev_episode(state: State<'_, AppState>) -> Result<(), String> {
    let slot = state.mpv.lock().await;
    if let Some(m) = slot.as_ref() {
        m.command(json!(["playlist-prev"])).await?;
    }
    Ok(())
}

/// [playlist] 当前播放列表位置：`{ pos, count }`（pos 从 0 开始，-1 = 空闲）。
/// 前端/手柄可用于显示「第 N/M 集」或做边界判断。
#[tauri::command]
async fn mpv_playlist_info(state: State<'_, AppState>) -> Result<Value, String> {
    let slot = state.mpv.lock().await;
    let Some(m) = slot.as_ref() else {
        return Ok(json!({ "pos": -1, "count": 0 }));
    };
    let pos = m
        .command(json!(["get_property", "playlist-pos"]))
        .await
        .ok()
        .and_then(|v| v.get("data").and_then(|d| d.as_i64()))
        .unwrap_or(-1);
    let count = m
        .command(json!(["get_property", "playlist-count"]))
        .await
        .ok()
        .and_then(|v| v.get("data").and_then(|d| d.as_i64()))
        .unwrap_or(0);
    Ok(json!({ "pos": pos, "count": count }))
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
        // [player] 播放器选择（默认自带 mpv；potplayer 路径空 = 自动探测）
        "playerKind": cfg.player_kind,
        "potplayerPath": cfg.potplayer_path,
        // [lc-1561] 自建弹幕接口（地址非敏感，明文回填；多地址逐行）
        "danmuApiEnabled": cfg.danmu_api_enabled,
        "danmuApiBases": cfg.danmu_api_bases,
        "danmuApiBase": cfg.danmu_api_base,
        // [lc-1753] 在线资源搜索（默认关闭；开启前用户已知悉「不存储不分发资源」声明）
        "webrsPanelEnabled": cfg.webrs_panel_enabled,
        // 弹幕引擎（camelCase 映射 `danmu_engine`）
        "danmuEngine": cfg.danmu_engine,
        // [gamepad] 全局手柄（camelCase；范围 clamp 在 set_settings）
        "gamepadEnabled": cfg.gamepad_enabled,
        "gamepadDeadzone": cfg.gamepad_deadzone,
        "gamepadDirectionThreshold": cfg.gamepad_direction_threshold,
        "gamepadRepeatDelay": cfg.gamepad_repeat_delay,
        "gamepadRepeatInterval": cfg.gamepad_repeat_interval,
        // [ui-opacity] 界面透明度（百分比；clamp 在 set_settings）
        "uiOpacity": cfg.ui_opacity,
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
    let old_engine = crate::danmaku::resolve_engine(&*state.config.read().await).to_string();
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
        // "continue" 为已移除的历史取值：老 config 仍可读取（前端归一为 home），但不再接受写入
        if !matches!(s, "home" | "discover") {
            return Err(format!("launchPage 非法：{s}（只允许 home/discover）"));
        }
        cfg.launch_page = s.to_string();
    }
    if let Some(v) = arg.get("autoLogin") {
        cfg.auto_login = v.as_bool().ok_or("autoLogin 必须是布尔值")?;
    }
    // [player] 播放器选择：mpv（默认）/ potplayer。potplayer 路径空 = 自动探测。
    if let Some(v) = arg.get("playerKind") {
        let s = v.as_str().unwrap_or("");
        if !matches!(s, "mpv" | "potplayer") {
            return Err(format!("playerKind 非法：{s}（只允许 mpv/potplayer）"));
        }
        cfg.player_kind = s.to_string();
    }
    if let Some(v) = arg.get("potplayerPath") {
        let s = v.as_str().ok_or("potplayerPath 必须是字符串")?.trim().to_string();
        if s.is_empty() {
            cfg.potplayer_path.clear();
        } else {
            // 非空路径必须真实存在且为 .exe（立即反馈，避免保存后才在起播时报错）
            cfg.potplayer_path = player::validate_potplayer_path(&s)?;
        }
    }
    // [lc-1561] 自建弹幕接口：多地址（逐行去空/去尾斜杠/去重，上限 4 条 —— 与原项目同口径）。
    // 改完**立即**桥给 Node 客户端并同步 Lua 闸门（不这样得等下次启动才生效）。
    let mut danmu_changed = false;
    // [lc-1753] 在线资源搜索开关：纯 UI 布尔，无联动副作用。
    if let Some(v) = arg.get("webrsPanelEnabled") {
        cfg.webrs_panel_enabled = v.as_bool().ok_or("webrsPanelEnabled 必须是布尔值")?;
    }
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
    // 弹幕引擎：node（默认/稳定）| rust（测试版）。非法值直接报错，避免静默回落。
    if let Some(v) = arg.get("danmuEngine") {
        let s = v.as_str().unwrap_or("");
        if !matches!(s, "node" | "rust") {
            return Err(format!("danmuEngine 非法：{s}（只允许 node/rust）"));
        }
        cfg.danmu_engine = s.to_string();
    }
    // [gamepad] 数值型设置：类型必须正确，超出范围按契约 clamp（不报错——前端滑块可能
    // 因浮点步进给出 0.5000001，这种边界值静默夹住比让 UI 报错更合理）。
    if let Some(v) = arg.get("gamepadEnabled") {
        cfg.gamepad_enabled = v.as_bool().ok_or("gamepadEnabled 必须是布尔值")?;
    }
    if let Some(v) = arg.get("gamepadDeadzone") {
        let n = v.as_f64().ok_or("gamepadDeadzone 必须是数字")? as f32;
        cfg.gamepad_deadzone = n.clamp(
            config::GAMEPAD_DEADZONE_MIN,
            config::GAMEPAD_DEADZONE_MAX,
        );
    }
    if let Some(v) = arg.get("gamepadDirectionThreshold") {
        let n = v.as_f64().ok_or("gamepadDirectionThreshold 必须是数字")? as f32;
        cfg.gamepad_direction_threshold = n.clamp(
            config::GAMEPAD_DIRECTION_THRESHOLD_MIN,
            config::GAMEPAD_DIRECTION_THRESHOLD_MAX,
        );
    }
    if let Some(v) = arg.get("gamepadRepeatDelay") {
        let n = v.as_f64().ok_or("gamepadRepeatDelay 必须是数字")?;
        cfg.gamepad_repeat_delay =
            (n.round() as i64).clamp(
                config::GAMEPAD_REPEAT_DELAY_MIN as i64,
                config::GAMEPAD_REPEAT_DELAY_MAX as i64,
            ) as u32;
    }
    if let Some(v) = arg.get("gamepadRepeatInterval") {
        let n = v.as_f64().ok_or("gamepadRepeatInterval 必须是数字")?;
        cfg.gamepad_repeat_interval =
            (n.round() as i64).clamp(
                config::GAMEPAD_REPEAT_INTERVAL_MIN as i64,
                config::GAMEPAD_REPEAT_INTERVAL_MAX as i64,
            ) as u32;
    }
    // [ui-opacity] 界面/侧边栏透明度**锁死 85**（产品要求）：无论前端传什么，一律写 85。
    // 前端已移除滑块；此处兜底，避免旧客户端把持久化值改回 60/100。
    if arg.get("uiOpacity").is_some() {
        cfg.ui_opacity = config::UI_OPACITY_LOCKED;
    }
    config::save(&app, &cfg).await?;
    *state.config.write().await = cfg.clone();
    // [lc-1561] 弹幕接口副作用：只在所有校验通过、配置成功落盘后再同步
    // （否则同一次调用若后续校验失败返回 Err，临时配置/Lua 闸门已改而配置未改，UI 回滚后不一致）
    if danmu_changed {
        danmaku_shim::write_danmu_config(&cfg)?;
        danmaku_shim::sync_lua_gate(&cfg.mpv_path, cfg.danmu_api_enabled, &crate::danmaku::effective_base(&cfg));
        // 内置自建源：地址/开关变化后按需启动或收掉
        crate::danmaku::builtin::spawn_reconfigure(app.clone());
    }
    // 弹幕引擎运行期切换：node ⇄ rust（与 save_config 同一路径，收旧服务/按新引擎重启）
    let new_engine = crate::danmaku::resolve_engine(&cfg).to_string();
    if old_engine != new_engine {
        danmaku_shim::spawn_reconfigure(app.clone());
    }
    // [gamepad] 热更新后台轮询线程的阈值/开关（无需重启）
    gamepad::update_settings(&cfg);
    Ok(json!({
        "exitMode": cfg.exit_mode,
        "perfMode": cfg.perf_mode,
        "carouselLogo": cfg.carousel_logo,
        "launchPage": cfg.launch_page,
        "autoLogin": cfg.auto_login,
        "playerKind": cfg.player_kind,
        "potplayerPath": cfg.potplayer_path,
        "danmuApiEnabled": cfg.danmu_api_enabled,
        "danmuApiBases": cfg.danmu_api_bases,
        "danmuApiBase": cfg.danmu_api_base,
        "webrsPanelEnabled": cfg.webrs_panel_enabled,
        "danmuEngine": cfg.danmu_engine,
        "gamepadEnabled": cfg.gamepad_enabled,
        "gamepadDeadzone": cfg.gamepad_deadzone,
        "gamepadDirectionThreshold": cfg.gamepad_direction_threshold,
        "gamepadRepeatDelay": cfg.gamepad_repeat_delay,
        "gamepadRepeatInterval": cfg.gamepad_repeat_interval,
        // [ui-opacity] 界面透明度（百分比；clamp 在 set_settings）
        "uiOpacity": cfg.ui_opacity,
    }))
}

/// [player] 自动探测本机 PotPlayer：返回 `{ path }`（未找到时 path 为空串）。
#[tauri::command]
async fn detect_potplayer() -> Result<Value, String> {
    Ok(json!({ "path": player::detect_potplayer_path().unwrap_or_default() }))
}

/// [player] 校验用户选择的 PotPlayer 路径：返回 `{ valid, resolved }`。
/// 不合法时 valid=false 且 resolved 为空（不抛错，方便前端即时提示）。
#[tauri::command]
async fn validate_potplayer_path(path: String) -> Result<Value, String> {
    match player::validate_potplayer_path(&path) {
        Ok(resolved) => Ok(json!({ "valid": true, "resolved": resolved })),
        Err(_) => Ok(json!({ "valid": false, "resolved": "" })),
    }
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
    // [upgrade-3.9] 旧 Electron 版（≤3.9.0）用户数据：字段映射 + 缓存/封面/Cookie 归档
    if let Some(d) = &mig.electron {
        libmpv::trace(&format!("[upgrade-3.9] 旧 Electron 数据迁移: {d}"));
    }
    if mig.appdata.is_none() && mig.localappdata.is_none() && mig.electron.is_none() {
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
        // [update] 前端资源覆盖层协议：补丁目录优先，缺失回退内嵌资源（见 update.rs）
        .register_asynchronous_uri_scheme_protocol("fntvweb", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                let response = update::handle_web_asset(app, request).await;
                responder.respond(response);
            });
        })

        .setup(|app| {
            // VEH 崩溃记录器：任意致命异常（D3D 视频输出崩因定位）写出异常码+故障模块+偏移，
            // 落 %TEMP%/anibox.log（lc-082 视频输出专项）
            libmpv::install_crash_logger();
            // [update] 载入前端补丁覆盖层指针；release 且存在补丁时切到 fntvweb 协议
            update::init(app.handle());
            // 启动时从磁盘读回配置（token/路径），否则每次重启都要重新登录
            let cfg = tauri::async_runtime::block_on(config::load(app.handle()));
            // [lc-1661] 日志分流设置：必须在任何模块开始写日志前注入（migrate 阶段用的是默认值）
            libmpv::apply_settings(cfg.log_debug, cfg.log_modules.clone());
            // [lc-1663] 匿名使用统计心跳：20s 后首报，之后每 6h 检查；失败静默、不阻塞启动
            usage_stats::spawn(app.handle().clone());
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
            // [progress] 上次进程被强杀（任务管理器 / kill）前落盘的最后进度：启动时补报一次。
            // 必须在 config 就绪之后（补报要读 token / 服务器地址）。
            fnos_progress::replay_pending(app.handle());
            // [gamepad] 全局手柄轮询（默认关闭；后台线程，退出时由 RunEvent::Exit 停止）
            gamepad::spawn(app.handle().clone(), gamepad::GamepadSettings::from(&cfg));
            // [lc-1561] 自建弹幕接口配置桥（Node 侧每次读盘 ⇒ 这里先写一份，后续 save_config
            // 再覆盖；保证 shim 一启动就能读到「启用/地址」，不会误判为未启用）。
            if let Err(e) = danmaku_shim::write_danmu_config(&cfg) {
                libmpv::trace(&format!("[danmaku-shim] 写自建源配置失败（不影响内置 B站）: {e}"));
            }
            danmaku_shim::sync_lua_gate(&cfg.mpv_path, cfg.danmu_api_enabled, &crate::danmaku::effective_base(&cfg));
            // [lc-1560] 启动即**预热弹幕 shim**（B站弹幕下载经它，见 danmaku_shim.rs）。
            // 首次播放时 shim 已在监听，省一次 node 冷启动；失败不影响本地播放。
            danmaku_shim::spawn_warmup(app.handle().clone());
            // 内置自建弹幕源 danmu_api：启动即后台预热（首次播放直接可用；失败降级内置 B站）
            crate::danmaku::builtin::spawn_warmup(app.handle().clone());
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
                    match play_item(h3.state::<AppState>(), h3.clone(), guid, None).await {
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
            // [lc-1582] AniList 官方 API：当日放送表（补 BGM 缺的「几点更新」→ 右栏行的「第 N 话 · HH:MM」）
            anilist::anilist_day_schedule,
            // [lc-1589] AniList：往季番表（「总表查询」弹窗：选年份+季度看该季列表）
            anilist::anilist_season,
            // [lc-1576] 评分分布（详情页「评分透视」兜底：BGM 无 rating.count 时改用 TMDB 的）
            tmdb::tmdb_rating_details,
            // 演员/人物页：TMDB 人物详情 + 参演作品（combined_credits 去重排序）
            tmdb::tmdb_person,
            // 详情页单集封面兜底：飞牛缺单集海报时按季拉 TMDB 剧照（镜像优先，120 天落盘）
            tmdb::tmdb_season_stills,
            // [lc-119] Bangumi（全线走自建镜像，见 bgm.rs 头部说明）
            bgm::bgm_match,
            bgm::bgm_subject,
            bgm::bgm_episodes,
            bgm::bgm_characters,
            bgm::bgm_persons,
            // 人物（演员/制作人员）详情 + 参演作品
            bgm::bgm_person,
            bgm::bgm_person_works,
            bgm::bgm_comments,
            // [lc-1412] 评论（长影评，/p1/.../reviews）：与「吐槽」分开的两个数据源
            bgm::bgm_reviews,
            // [lc-1401] 关联条目（详情页「关联条目」标签页：片头曲/原声集/游戏/画集…按关系分组）
            bgm::bgm_related,
            // [lc-123] 发现页右栏：每日新番表（镜像 /calendar 路由，见 bgm.rs）
            bgm::bgm_calendar,
            // [lc-1605] 发现页右栏「总表查询」弹窗：往季番表（镜像 /v0/subjects?year&month 路由）
            bgm::bgm_season,
            // [lc-1720] 手动获取元数据（TMDB + BGM 镜像搜索；选定后仅本地保存）
            metadata_override::metadata_search,
            metadata_override::metadata_get_override,
            metadata_override::metadata_set_override,
            metadata_override::metadata_clear_override,
            get_config,
            save_config,
            // [lc-1561] 自建弹幕接口连通测试（设置面板）
            test_danmu_api,
            // [bili-account] B 站账号 Cookie 读写/体检 + Node 检测 + 一键自建弹幕源
            bili_account::bili_account_status,
            bili_account::bili_set_cookie,
            bili_account::bili_clear_cookie,
            bili_account::detect_node,
            bili_account::setup_builtin_danmaku,
            // [bili-qr] B 站二维码扫码登录
            bili_qr::bili_qr_generate,
            bili_qr::bili_qr_poll,
            // [danmaku] 自建弹幕源 Rust 版：只跑三跳的调试命令
            danmaku::danmaku_fetch_selfhosted,
            // [danmaku-diagnose] 设置面板「诊断」：Node / 自建源 / 业务匹配 / shim / Cookie / 播放上下文
            danmaku::diagnose::danmaku_diagnose,
            // [lc-1434] 设置面板（通用/外观）：白名单子集读写 + 启动页查询
            get_settings,
            set_settings,
            // [gamepad] 全局手柄：当前状态 / 触发一次检测
            gamepad::gamepad_status,
            gamepad::gamepad_detect,
            // [player] 播放器选择：自动探测 / 路径校验
            detect_potplayer,
            validate_potplayer_path,
            get_launch_page,
            // [lc-1436] 诊断：打开日志/缓存/数据目录；开机自启
            open_path,
            set_autostart,
            // [lc-1599] 应用内反馈（工单制）：提交（日志尾部脱敏 + 环境块）/ 列表（本地索引+远端状态合并）/
            // 会话（原样透传）/ 回复
            feedback::feedback_submit,
            feedback::feedback_list,
            feedback::feedback_thread,
            feedback::feedback_reply,
            // [lc-1599] 设置面板「日志/诊断」迁移到反馈面板：日志信息 / 打开日志或目录 /
            // 读取脱敏日志（复制）/ 诊断信息 / 导出诊断包 / 手动日志文件上传
            feedback::feedback_log_info,
            feedback::feedback_open_log,
            feedback::feedback_read_log,
            feedback::feedback_diagnostics,
            feedback::feedback_export_diagnostics,
            // [lc-1661] 日志设置（调试开关 + 组件过滤，持久化到 config.json）
            feedback::feedback_log_settings_get,
            feedback::feedback_log_settings_set,
            // [lc-1662] 匿名使用统计开关（默认开）
            // [lc-1663] 心跳上报链路在 usage_stats 模块内（启动 20s 后首报，之后 6h 检查）
            usage_stats::usage_stats_get,
            usage_stats::usage_stats_set,
            // [lc-1435] 关闭询问的回答（exit → 走 RunEvent::Exit 退场清理）
            resolve_close,
            // [update] 应用信息 / 全量更新（自建下载器 + NSIS）/ 前端热补丁
            update::app_info,
            // [webview2] 设置「关于」页：WebView2 Runtime 检测（缺失时给下载入口）
            webview2::check_webview2,
            update::update_check,
            update::update_source_probe,
            update::update_download,
            update::update_cancel,
            update::update_install,
            update::patch_check,
            update::patch_apply,
            update::patch_rollback,
            // 缓存面板三要素：占用 / 上限 / 清理
            cache_stats,
            cache_set_limit,
            cache_clear,
            login,
            logout,
            check_session,
            get_media_libs,
            get_media_sum,
            // [library-cover] 首页媒体库卡片自定义封面（保存/读取/恢复默认）
            libcover::get_library_covers,
            libcover::save_library_cover,
            libcover::reset_library_cover,
            get_favorites,
            favorites::favorite_get,
            favorites::favorite_toggle,
            // [lc-1654] 海报卡「已看」切换（飞牛 item/watched，失败降级本地 watched.json）
            watched::watched_get_all,
            watched::set_item_watched,
            get_items_global,
            get_items,
            // [lc-1725] 全局聚合搜索（顶栏胶囊：GET /v/api/v1/search/list?q=）
            search_items,
            // [lc-1701] 演员页「入库状态」批量匹配
            match_library_works,
            get_continue_watching,
            // [lc-1615] 观影记录：飞牛 play/list ∪ 本机播放历史（合并/富化见 history.rs）
            get_watch_history,
            get_my_rating,
            set_my_rating,
            // [lc-1631] 年度报告数据源 + 播放结束回填真实观看时长
            get_watch_report,
            record_watch_session,
            report_watch_progress,
            get_seasons,
            get_episodes,
            get_edit_detail,
            save_edit_detail,
            refresh_item,
            scrap_search,
            scrap_rescrap,
            scrap_unscrap,
            get_item_detail,
            get_carousel_items,
            // [版本] 同一剧/集的多版本（多文件）：stream/list 的 video_streams
            get_media_versions,
            play_item,
            cloud115::cloud115_qr_start,
            cloud115::cloud115_qr_poll,
            cloud115::cloud115_status,
            cloud115::cloud115_logout,
            cloud115::cloud115_list,
            cloud115::cloud115_play,
            danmaku_sources::danmaku_sources_get,
            danmaku_sources::danmaku_sources_set,
            subtitle_sources::subtitle_sources_get,
            subtitle_sources::subtitle_sources_set,
            subtitle_sources::subtitle_search,
            subtitle_sources::subtitle_download,
            player_stop,
            mpv_toggle_pause,
            player_source,
            mpv_set_ui_fullscreen,
            mpv_seek,
            mpv_set_volume,
            mpv_set_rate,
            // [playlist] 整季播放列表：上下集操作 + 当前位置查询
            mpv_next_episode,
            mpv_prev_episode,
            mpv_playlist_info,
            // [lc-1515] Bangumi 账号绑定 + 追番状态同步（Kazumi 同款）
            bgm_account::bgm_account_status,
            bgm_account::bgm_account_bind,
            bgm_account::bgm_account_unbind,
            bgm_account::bgm_account_set_remember,
            bgm_account::bgm_collect_set,
            bgm_account::bgm_collect_get,
            bgm_account::bgm_sync_now,
            bgm_account::bgm_collect_stats,
            bgm_account::bgm_collect_season,
            bgm_account::bgm_watching_covers,
            // [lc-1597] 观看进度过 80% → 把该集在 BGM 标「看过」（PUT .../episodes/{id} {type:2}）
            bgm_account::bgm_episode_watched,
            // [lc-1500] 在线资源（Kazumi 规则源）：发现页无源条目 → 搜索/选集/解析直链 → mpv
            // [lc-1501] 多源聚合搜索 + 用户加源/删源（Kazumi「播放来源 / 我的规则」同款）
            webrs::webrs_rules,
            webrs::webrs_search,
            webrs::webrs_search_multi,
            webrs::webrs_add_rule,
            webrs::webrs_remove_rule,
            webrs::webrs_chapters,
            webrs::webrs_resolve,
            webrs::webrs_play,
            // [lc-1606] 反爬验证闭环（图片验证码 / 自动点击 / 自定义 JS）
            webrs::webrs_captcha_start,
            webrs::webrs_captcha_submit,
            webrs::webrs_captcha_cancel,
            // [lc-1747] 可见窗手动验证兜底（隐藏窗自动验证兜不住的交互挑战）
            webrs::webrs_captcha_visible,
            // [lc-1608] 在线规则目录 + 一键更新（拉 KazumiRules index.json / 装/更新单条 / 查可更新）
            webrs::webrs_catalog,
            webrs::webrs_install_rule,
            webrs::webrs_update_rule,
            webrs::webrs_check_updates,
            // [lc-1757] 在线采集源目录（TMDB 剧集侧，上游 = 本仓库 docs/maccms-catalog/）
            webrs::webrs_maccms_catalog,
            webrs::webrs_maccms_install
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                // [lc-1747] 销毁 webrs 常驻隐藏窗（嗅探 / 验证单例），别把 WebView2 进程留到退出后
                webrs_webview::shutdown_webviews(app_handle);
                // [progress][fix] 退出前**同步** flush 最后一次进度（网络上限 3s），
                // 必须放在杀 mpv / 丢异步运行时之前 —— 否则 `flush_on_exit` spawn 出去的任务
                // 根本没机会跑，这就是「直接关掉软件最后一次进度丢失」的根因。
                let _ = tauri::async_runtime::block_on(fnos_progress::flush_on_exit_sync(app_handle));
                // 退出时清场：mpv 子进程 + 嵌入窗口 + Go 代理
                let st = app_handle.state::<AppState>();
                // [gamepad] 停止手柄轮询线程（≤8ms 自行退出）
                gamepad::stop();
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
                // 内置 danmu_api 子进程同样收掉
                if let Some(mut bc) = st.builtin_child.blocking_lock().take() {
                    let _ = bc.kill();
                }
                // Rust 弹幕引擎（若开启）停掉监听线程
                danmaku::stop_engine();
                // [player] PotPlayer 子进程（不 emit 事件：应用正在退出）
                let _ = tauri::async_runtime::block_on(player::stop_potplayer(
                    app_handle,
                    &st,
                    false,
                ));
                let mut pc = st.proxy_child.blocking_lock();
                if let Some(c) = pc.as_mut() {
                    let _ = c.kill();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    /// [lc-1727] 媒体库分页终止条件（纯函数）：60 一页 + 滚动加载更多的判停口径。
    /// 背景：v4.0.0 里库列表只请求了第 1 页（60 条）就再也不翻页，
    /// 超过 60 条的库从第 61 条起永远看不到 —— 这条测试锁住「什么时候该继续翻」。
    #[test]
    fn item_pagination_stop_rules() {
        // 首页整页、总数更多 → 继续
        assert!(super::should_load_more(60, Some(151), 60));
        // total 已知且已达标 → 停（即使刚好整页）
        assert!(!super::should_load_more(120, Some(120), 60));
        assert!(!super::should_load_more(180, Some(151), 60));
        // 最后一页不足一页 → 停（total 缺失时同样能停）
        assert!(!super::should_load_more(151, Some(151), 60));
        assert!(!super::should_load_more(151, None, 60));
        // total 缺失且刚好整页 → 继续（可能还有下一页）
        assert!(super::should_load_more(120, None, 60));
        // total=0 视为未知
        assert!(super::should_load_more(60, Some(0), 60));
        // 防御：page_size=0 不翻页
        assert!(!super::should_load_more(60, Some(600), 0));
    }

    /// [lc-1727] page_size 收敛：默认 60，正数夹到 [1, 200]，非法值回默认。
    #[test]
    fn item_list_page_size_clamps() {
        assert_eq!(super::item_list_page_size(None), 60);
        assert_eq!(super::item_list_page_size(Some(0)), 60);
        assert_eq!(super::item_list_page_size(Some(-5)), 60);
        assert_eq!(super::item_list_page_size(Some(30)), 30);
        assert_eq!(super::item_list_page_size(Some(200)), 200);
        assert_eq!(super::item_list_page_size(Some(5000)), 200);
    }

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

    /// [live] 直播线路挑选：优先可播线路；全部不可播时退回第一条有 path 的；
    /// 没有 live_channels / 没有 path 时返回 None（普通媒体继续走 Go 代理）。
    #[test]
    fn pick_live_channel_rules() {
        use serde_json::json;
        // 第一条 can_play=0，第二条可播 → 选第二条
        let v = json!({ "live_channels": [
            { "path": "http://gw/1", "file_name": "线路1", "can_play": 0 },
            { "path": "http://gw/2", "file_name": "线路2", "can_play": 1 },
        ]});
        assert_eq!(
            super::pick_live_channel(&v),
            Some(("http://gw/2".to_string(), Some("线路2".to_string())))
        );
        // 全部不可播 → 仍退回第一条有 path 的（避免直播完全播不了）
        let v = json!({ "live_channels": [
            { "path": "http://gw/1", "can_play": 0 },
            { "path": "http://gw/2", "can_play": 0 },
        ]});
        assert_eq!(
            super::pick_live_channel(&v),
            Some(("http://gw/1".to_string(), None))
        );
        // 缺省 can_play 视为可播
        let v = json!({ "live_channels": [ { "path": " http://gw/9 ", "file_name": "CCTV" } ]});
        assert_eq!(
            super::pick_live_channel(&v),
            Some(("http://gw/9".to_string(), Some("CCTV".to_string())))
        );
        // 空 path / 无 live_channels → None
        let v = json!({ "live_channels": [ { "path": "  ", "can_play": 1 } ]});
        assert_eq!(super::pick_live_channel(&v), None);
        assert_eq!(super::pick_live_channel(&json!({ "type": "Movie" })), None);
    }

    /// [playlist] 单集标题拼接：完整季集号 / 只有番名+集名 / 只有集名 三种形态。
    #[test]
    fn playlist_episode_title_shapes() {
        assert_eq!(
            super::playlist_episode_title("番名", 1, 5, "集名"),
            "番名 - S1E5: 集名"
        );
        // 集标题缺失：保留 SxxExx，不留裸的 ": "。
        assert_eq!(
            super::playlist_episode_title("番名", 1, 5, ""),
            "番名 - S1E5"
        );
        // 特别篇/未分季：季号缺省为 0 时仍要带集号（旧写法会整段丢掉）。
        assert_eq!(
            super::playlist_episode_title("番名", 0, 3, "集名"),
            "番名 - S0E3: 集名"
        );
        // 电影/个人视频：集号 0 → 不出现 S0E0。
        assert_eq!(
            super::playlist_episode_title("电影", 0, 0, ""),
            "电影"
        );
        assert_eq!(
            super::playlist_episode_title("番名", 0, 0, "集名"),
            "番名 - 集名"
        );
        assert_eq!(
            super::playlist_episode_title("", 1, 5, "集名"),
            "S1E5: 集名"
        );
        assert_eq!(super::playlist_episode_title("", 0, 0, "集名"), "集名");
    }

    /// [title] 繁体剧名/集名 → 简体，且格式严格保持 `剧名 - S1E3: 集名`。
    #[test]
    fn playlist_episode_title_converts_traditional() {
        assert_eq!(
            super::playlist_episode_title(
                "亂世千金倪亞．利斯頓轉生為嬌弱千金的弒神武人華麗無雙錄",
                1,
                3,
                "舞台落幕之後"
            ),
            "乱世千金倪亚．利斯顿转生为娇弱千金的弑神武人华丽无双录 - S1E3: 舞台落幕之后"
        );
        // 集名缺失：`剧名 - S1E3`，不出现裸冒号。
        assert_eq!(
            super::playlist_episode_title("愛蜜莉雅", 1, 3, ""),
            "爱蜜莉雅 - S1E3"
        );
        // 特别篇：S0Exx 同样保留。
        assert_eq!(
            super::playlist_episode_title("繁體番名", 0, 7, "特別篇"),
            "繁体番名 - S0E7: 特别篇"
        );
        // 电影：无 SxxExx，只拼 `剧名 - 集名`。
        assert_eq!(
            super::playlist_episode_title("劇場版", 0, 0, "覺醒"),
            "剧场版 - 觉醒"
        );
        // 非中文原样保留（英文/数字/假名/拉丁标点）。
        assert_eq!(
            super::playlist_episode_title("Re:Zero", 1, 3, "Memory Snow"),
            "Re:Zero - S1E3: Memory Snow"
        );
        assert_eq!(
            super::playlist_episode_title("この素晴らしい世界に祝福を！", 0, 0, "第1話"),
            "この素晴らしい世界に祝福を！ - 第1话"
        );
    }

    /// [playlist] 整季 M3U：每条一个 #EXTINF + 本机代理 URL，顺序与条目一致。
    #[test]
    fn playlist_m3u_has_every_entry_in_order() {
        let cfg = crate::config::Config::default();
        let entries = vec![
            super::PlaylistEntry {
                guid: "ep1".into(),
                title: "番名 - S1E1: 第一集".into(),
                duration_secs: 1440.0,
                season: 1,
                episode: 1,
            },
            super::PlaylistEntry {
                guid: "ep2".into(),
                title: "番名 - S1E2: 第二集".into(),
                duration_secs: 0.0,
                season: 1,
                episode: 2,
            },
        ];
        let out = super::build_playlist_m3u(&cfg, &entries);
        assert!(out.starts_with("#EXTM3U\n"));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 5, "首行 + 2×(EXTINF+URL)");
        assert_eq!(lines[1], "#EXTINF:1440,番名 - S1E1: 第一集");
        assert!(lines[2].contains("/api/v1/playvideo/ep1?"));
        assert_eq!(lines[3], "#EXTINF:-1,番名 - S1E2: 第二集");
        assert!(lines[4].contains("/api/v1/playvideo/ep2?"));
    }

    /// [theintrodb] skip-metadata 载荷：整季按 guid 分集、单条用 play_guid、
    /// 无 TMDB 锚点（空/非数字）时下发空表（不误触发兜底）。
    #[test]
    fn skip_metadata_maps_each_episode_by_guid() {
        let entries = vec![
            super::PlaylistEntry {
                guid: "ep1".into(),
                title: "S1E1".into(),
                duration_secs: 0.0,
                season: 1,
                episode: 1,
            },
            super::PlaylistEntry {
                guid: "ep2".into(),
                title: "S1E2".into(),
                duration_secs: 0.0,
                season: 1,
                episode: 2,
            },
        ];
        let m = super::build_skip_metadata("12345", "ep1", 1, 1, &entries);
        assert_eq!(m.pointer("/ep1/tmdb").and_then(|v| v.as_str()), Some("12345"));
        assert_eq!(m.pointer("/ep1/season").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(m.pointer("/ep2/episode").and_then(|v| v.as_i64()), Some(2));

        // 单条（电影/单集）：用 play_guid 作键；季集号 0 时省略，避免拼 S0E0。
        let single = super::build_skip_metadata("777", "movie-guid", 0, 0, &[]);
        assert_eq!(
            single.pointer("/movie-guid/tmdb").and_then(|v| v.as_str()),
            Some("777")
        );
        assert!(single.pointer("/movie-guid/season").is_none());

        // 没有 TMDB id / 非数字：空对象（Lua 侧当作「暂无元数据」）。
        for bad in ["", "  ", "tt123", "bg456"] {
            let empty = super::build_skip_metadata(bad, "g", 1, 1, &[]);
            assert_eq!(empty.as_object().map(|o| o.len()), Some(0), "{bad:?}");
        }
    }
}
