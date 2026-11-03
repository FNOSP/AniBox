//! JAV 设置（jav-settings.json）—— JavHub 设置面板的后端。
//!
//! 区块与字段对照 JAVCloudHub 1.9.31 的设置页（逆向 page-settings.js 的 SECTIONS），
//! 按 AniBox 架构做了取舍：
//!   · 扫描规则 / 定时扫描 / 网络与代理 / 刮削站点 / 刮削行为 / 内置 AI → 全量落地；
//!   · 数据源 / 刮削目录 → 管理在「网盘接入」页，设置里只读展示；
//!   · 存储位置 / 播放与导出 / 服务与窗口 / 桌面与托盘 → 不适用（AniBox 自身架构已覆盖）；
//!   · 删除 → AniBox 还没有网盘文件删除原语，暂缺（后续接 provider delete 再补）。
//!
//! 存储：`app_data_dir/jav-settings.json`，深合并读盘（老文件缺字段自动补默认）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

pub const DEFAULT_VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "avi", "wmv", "mov", "ts", "m2ts", "iso", "rmvb", "flv", "mpg", "mpeg", "webm", "wmv",
];
pub const DEFAULT_SKIP_DIRS: &[&str] = &[
    "$recycle.bin", "system volume information", "@eadir", "#recycle", ".clouddrive",
    "lost+found", "recycle", "$recycle", ".trash", "trash",
];
/// 内置刮削站点（顺序即优先级；id 对齐 jav_library 的引擎名）
pub const SITE_DEFS: &[(&str, &str, &[&str])] = &[
    // [lc-1958] JavBus 排最前：标题是中文（繁体），是「原软件标题都是中文」的正主；
    // r18dev/jav321 等只有英文/日文标题，降级为兜底
    ("javbus", "JavBus（中文标题）", &["censored", "uncensored"]),
    ("r18dev", "R18.dev（公开 JSON，最稳）", &["censored", "uncensored"]),
    ("fc2", "FC2 官方站（需代理）", &["fc2"]),
    ("jav321", "JAV321（不吃 CF，兜底）", &["censored"]),
    ("mgstage", "MGStage 官方（需代理）", &["censored"]),
    ("heyzo", "HEYZO 官方（需代理）", &["uncensored"]),
];

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct ScanSettings {
    pub max_depth: u32,
    pub merge_multipart: bool,
    pub prune_missing: bool,
    pub video_exts: Vec<String>,
    pub skip_dirs: Vec<String>,
    pub skip_prefixes: Vec<String>,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            max_depth: 3,
            merge_multipart: true,
            prune_missing: true,
            video_exts: DEFAULT_VIDEO_EXTS.iter().map(|s| s.to_string()).collect(),
            skip_dirs: DEFAULT_SKIP_DIRS.iter().map(|s| s.to_string()).collect(),
            skip_prefixes: vec![".".to_string()],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoScanSettings {
    pub enabled: bool,
    /// 分钟；读用时夹紧到 5 分钟 ~ 7 天
    pub interval_minutes: u32,
    pub scrape_new: bool,
    pub cleanup_missing: bool,
    /// 上次跑完的时刻（ms）；0 = 还没跑过
    pub last_run_ms: u64,
}

impl Default for AutoScanSettings {
    fn default() -> Self {
        Self { enabled: false, interval_minutes: 360, scrape_new: true, cleanup_missing: true, last_run_ms: 0 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct NetworkSettings {
    /// 手填代理（http://127.0.0.1:10808 / socks5://…）；留空 = 只看系统代理
    pub proxy: String,
    /// 关掉就不读环境变量 / 注册表系统代理，强制直连
    pub trust_env: bool,
    pub timeout_seconds: u32,
    pub user_agent: String,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            proxy: String::new(),
            trust_env: true,
            timeout_seconds: 25,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36".into(),
        }
    }
}

/// 一个刮削站点：配置只存差异（id + 开关 + 地址覆盖），名称/支持分类来自 SITE_DEFS
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SiteCfg {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub base: String,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct ScrapeSettings {
    /// 站点顺序即优先级；缺失的内置站点在读取时自动补到末尾
    pub sites: Vec<SiteCfg>,
    /// 两条请求之间的礼貌间隔（秒）
    pub interval_seconds: f64,
    /// 参与刮削的分类（fc2/censored/uncensored）；无番号 unknown 本就不刮
    pub categories: Vec<String>,
    pub max_attempts: u32,
}

impl Default for ScrapeSettings {
    fn default() -> Self {
        Self {
            sites: SITE_DEFS.iter().map(|(id, _, _)| SiteCfg { id: id.to_string(), enabled: true, base: String::new() }).collect(),
            interval_seconds: 1.5,
            categories: vec!["fc2".into(), "censored".into(), "uncensored".into()],
            max_attempts: 3,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettings {
    pub enabled: bool,
    /// OpenAI 兼容根（不含 /chat/completions），默认 DeepSeek
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    pub model: String,
    /// 译名目标语言（zh / zh-TW / en …），提示词按此拼
    pub target_lang: String,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://api.deepseek.com/v1".into(),
            api_key: String::new(),
            model: "deepseek-chat".into(),
            target_lang: "zh".into(),
        }
    }
}

/// 飞牛媒体库接入：对哪些库启用番号刮削（用户在 设置 → 数据源 里勾选）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FnosLibCfg {
    pub guid: String,
    pub title: String,
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FnosSettings {
    pub libs: Vec<FnosLibCfg>,
}

/// 本地文件夹数据源：用户在设置里挑的本地视频目录（绝对路径列表）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalSettings {
    pub dirs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct JavSettings {
    pub scan: ScanSettings,
    pub auto_scan: AutoScanSettings,
    pub network: NetworkSettings,
    pub scrape: ScrapeSettings,
    pub ai: AiSettings,
    pub fnos: FnosSettings,
    pub local: LocalSettings,
}

pub fn settings_path() -> Option<PathBuf> {
    crate::cloud::app_data_dir().map(|d| d.join("jav-settings.json"))
}

/// 读设置：文件缺失/字段缺失都落默认（深合并语义 = serde(default) 整struct兜底）
pub fn load() -> JavSettings {
    let Some(p) = settings_path() else { return JavSettings::default() };
    let Ok(s) = std::fs::read_to_string(p) else { return JavSettings::default() };
    let mut v: JavSettings = serde_json::from_str(&s).unwrap_or_default();
    normalize(&mut v);
    v
}

/// 站点列表补齐（新内置站点自动出现在末尾）+ 数值夹紧
pub fn normalize(v: &mut JavSettings) {
    // [lc-1958] 配置里缺的站点补到**最前**（按 SITE_DEFS 顺序）——新引擎要立刻生效，
    // 不能 append 到尾部变成最低优先级（javbus 加进来的意义就是抢在英文源前面）
    for (id, _, _) in SITE_DEFS.iter().rev() {
        if !v.scrape.sites.iter().any(|s| &s.id == id) {
            v.scrape.sites.insert(0, SiteCfg { id: id.to_string(), enabled: true, base: String::new() });
        }
    }
    v.scrape.sites.retain(|s| SITE_DEFS.iter().any(|(id, _, _)| &s.id == id));
    v.scan.max_depth = v.scan.max_depth.clamp(1, 6);
    v.auto_scan.interval_minutes = v.auto_scan.interval_minutes.clamp(5, 10080);
    if v.network.timeout_seconds == 0 {
        v.network.timeout_seconds = 25;
    }
    if v.ai.target_lang.trim().is_empty() {
        v.ai.target_lang = "zh".into();
    }
    v.local.dirs = v
        .local
        .dirs
        .iter()
        .map(|d| d.trim().trim_end_matches(['/', '\\']).to_string())
        .filter(|d| !d.is_empty())
        .collect::<Vec<_>>();
    v.local.dirs.dedup();
}

pub fn save(v: &JavSettings) -> Result<(), String> {
    let mut v = v.clone();
    normalize(&mut v);
    let Some(p) = settings_path() else { return Err("app_data_dir 未初始化".into()) };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let s = serde_json::to_string_pretty(&v).map_err(|e| format!("序列化设置失败: {e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写设置失败: {e}"))
}

/// 某分类下按设置顺序要尝试的引擎：(引擎 id, 地址覆盖)
pub fn engine_order(v: &JavSettings, category: &str) -> Vec<(String, String)> {
    v.scrape
        .sites
        .iter()
        .filter(|s| s.enabled && SITE_DEFS.iter().any(|(id, _, cats)| id == &s.id && cats.contains(&category)))
        .map(|s| (s.id.clone(), s.base.trim().to_string()))
        .collect()
}

// ───────────────────────── 命令 ─────────────────────────

#[tauri::command]
pub fn jav_settings_get() -> Result<Value, String> {
    let v = load();
    let mut base = serde_json::to_value(&v).map_err(|e| format!("序列化设置失败: {e}"))?;
    // 附加只读信息：数据目录位置（高级节展示用）+ 站点静态信息（名称/支持分类）
    if let Some(dir) = crate::cloud::app_data_dir() {
        base["paths"] = json!({
            "dataDir": dir.to_string_lossy(),
            "settingsFile": "jav-settings.json",
            "libraryFile": "jav-library.json",
            "indexFile": "cloud-library.json",
        });
    }
    base["siteDefs"] = json!(SITE_DEFS.iter().map(|(id, name, cats)| json!({
        "id": id, "name": name, "categories": cats,
    })).collect::<Vec<_>>());
    Ok(base)
}

#[tauri::command]
pub fn jav_settings_set(settings: Value) -> Result<Value, String> {
    let mut v: JavSettings = serde_json::from_value(settings).map_err(|e| format!("设置格式不对: {e}"))?;
    normalize(&mut v);
    save(&v)?;
    Ok(json!({ "ok": true }))
}

/// 「立即跑一轮」定时扫描的同一套例行动作（设置页按钮 / 后台调度共用）
pub async fn auto_scan_once(app: &tauri::AppHandle) -> Result<Value, String> {
    let mut cfg = load();
    // 找出所有配了媒体根目录的盘，逐盘重扫
    let providers = crate::cloud_library::providers_with_roots();
    if providers.is_empty() {
        return Err("还没有任何盘配过媒体目录（去「网盘接入」点「加入媒体库」）".into());
    }
    let mut results: Vec<Value> = Vec::new();
    for p in providers {
        match crate::cloud_library::cloud_library_scan(app.clone(), p.clone(), Some(false)).await {
            Ok(r) => {
                // 自动刮削新入库：只刮待刮削（force=false 天然增量）
                let mut scrape = json!({ "skipped": true });
                if cfg.auto_scan.scrape_new {
                    if let Ok(s) = crate::jav_library::jav_library_scrape(app.clone(), p.clone(), Some(false)).await {
                        scrape = s;
                    }
                }
                results.push(json!({ "provider": p, "scan": r, "scrape": scrape }));
            }
            Err(e) => results.push(json!({ "provider": p, "error": e })),
        }
    }
    cfg.auto_scan.last_run_ms = now_ms();
    save(&cfg)?;
    Ok(json!({ "ranAtMs": cfg.auto_scan.last_run_ms, "results": results }))
}

#[tauri::command]
pub async fn jav_auto_scan_run(app: tauri::AppHandle) -> Result<Value, String> {
    auto_scan_once(&app).await
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 后台定时扫描（应用启动时 spawn，默认关闭无任何动作）。
/// 每分钟醒来看一眼是否到期；到期跑一轮并记录时间。间隔夹紧 5 分钟 ~ 7 天。
pub fn spawn_auto_scan(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let cfg = load();
            if !cfg.auto_scan.enabled {
                continue;
            }
            let interval = cfg.auto_scan.interval_minutes.clamp(5, 10080) as u64 * 60_000;
            if cfg.auto_scan.last_run_ms != 0 && now_ms().saturating_sub(cfg.auto_scan.last_run_ms) < interval {
                continue;
            }
            // 后台轮次失败只记日志，不打扰用户（下一轮会再试）
            if let Err(e) = auto_scan_once(&app).await {
                eprintln!("[jav auto-scan] 定时扫描失败: {e}");
            }
        }
    });
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_javhub() {
        let v = JavSettings::default();
        assert_eq!(v.scan.max_depth, 3);
        assert!(v.scan.merge_multipart && v.scan.prune_missing);
        assert!(!v.auto_scan.enabled);
        assert_eq!(v.auto_scan.interval_minutes, 360);
        assert!(v.network.trust_env);
        assert_eq!(v.ai.base_url, "https://api.deepseek.com/v1");
        assert_eq!(v.scrape.sites.len(), SITE_DEFS.len());
        assert!(v.scrape.sites[0].id == "r18dev");
    }

    #[test]
    fn engine_order_respects_settings() {
        let mut v = JavSettings::default();
        // 停掉 r18dev 后，有码站点的首选变成 jav321
        v.scrape.sites.iter_mut().for_each(|s| {
            if s.id == "r18dev" {
                s.enabled = false;
            }
        });
        let order = engine_order(&v, "censored");
        assert_eq!(order[0].0, "jav321");
        // fc2 分类只可能有 fc2 引擎
        let order = engine_order(&v, "fc2");
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].0, "fc2");
    }

    #[test]
    fn normalize_clamps_and_backfills() {
        let mut v = JavSettings::default();
        v.scan.max_depth = 99;
        v.auto_scan.interval_minutes = 1;
        v.scrape.sites.clear();
        normalize(&mut v);
        assert_eq!(v.scan.max_depth, 6);
        assert_eq!(v.auto_scan.interval_minutes, 5);
        assert_eq!(v.scrape.sites.len(), SITE_DEFS.len());
    }
}
