//! 应用配置：app_data_dir/config.json。PoC 默认复用 Fntv-Plus 的 mpv / Go 代理分发。

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    /// NAS 地址，如 http://192.168.31.170:18888
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub token: String,
    /// 记住的密码（勾选「记住密码」时随 config.json 落盘，与 token 同级；后续可迁 keyring 凭据库）
    #[serde(default)]
    pub password: String,
    /// 启动自动登录：token 失效时用记住的密码静默重登（依赖 password 非空）
    #[serde(default)]
    pub auto_login: bool,
    #[serde(default = "default_mpv")]
    pub mpv_path: String,
    #[serde(default = "default_proxy")]
    pub proxy_path: String,
    /// 对齐 Fntv-Plus cfg.nasProxyEnabled（useNasLocal 参数）
    #[serde(default)]
    pub nas_proxy_enabled: bool,
    /// 图片/数据落盘缓存上限（MB；**0 = 不限**）。落盘目录 = app_data_dir/cache（见 disk_cache.rs）。
    /// 设置面板「缓存」一栏读写它；超上限按最后访问时间 LRU 淘汰。
    #[serde(default = "default_cache_limit_mb")]
    pub cache_limit_mb: u64,

    /* ── [lc-1434] 设置面板新增项（原 Fntv-Plus 设置面板的同名功能）──
       全部带 serde default：老 config.json 里没有这些键时按默认值走，**不需要迁移脚本**。 */
    /// 关闭按钮行为："exit"（直接退出）| "tray"（最小化到托盘）| "ask"（每次询问，默认）
    #[serde(default = "default_exit_mode")]
    pub exit_mode: String,
    /// 性能模式：关动画、关磨砂（低配机救急）。纯前端开关，存这里是为了跨设备/重启保持
    #[serde(default)]
    pub perf_mode: bool,
    /// 首页轮播是否用影片透明 Logo 代替文字标题（飞牛条目自带 logo，见 lc-570）
    #[serde(default = "default_true")]
    pub carousel_logo: bool,
    /// 启动页："home"（首页，默认）| "continue"（继续观看）
    #[serde(default = "default_launch_page")]
    pub launch_page: String,
    /// [lc-1515] Bangumi 个人 Access Token（追番状态同步用；用户手动粘贴，本地保存）
    #[serde(default)]
    pub bgm_token: String,
    /// [lc-1660] 是否「记住上次账号」：false 时**磁盘上不保留** bgm_token（内存中仍保留，供本次会话使用）。
    /// 老 config.json 无此键 → 默认 true（与历史行为一致，token 继续落盘）。
    #[serde(default = "default_true")]
    pub bgm_remember: bool,
    /// [lc-1660] 最近一次校验成功的 Bangumi 用户名（缓存，供网络/镜像失败时状态接口回显）。
    #[serde(default)]
    pub bgm_username: String,

    /* ── [lc-1561] 自建弹幕接口（danmu_api）—— 对齐主仓库 fn_config 的同名项 ──
       用户在自己的 NAS/Docker 上部署 https://github.com/huangxd-/danmu_api（LogVar 弹幕API），
       它把哔哩/爱奇艺/优酷/腾讯等平台的弹幕聚合到同一集，密度显著高于本应用内置的
       「直连 B站 单源 + 阈值聚合」。作为弹幕**优选源**：命中即用，未命中自动降级内置 B站。
       默认关闭 = 完全走内置 B站 链路（与接入前行为一致）。 */
    /// 自建弹幕接口开关
    #[serde(default)]
    pub danmu_api_enabled: bool,
    /// 单地址（恒等于 bases[0]，兼容旧读取方与 uosc conf 同步）
    #[serde(default)]
    pub danmu_api_base: String,
    /// 多地址列表（故障转移；主仓库上限 4 条）
    #[serde(default)]
    pub danmu_api_bases: Vec<String>,

    /* ── [lc-1599] 应用内反馈（工单制）── */
    /// 反馈系统匿名设备 ID：首次提交反馈时生成 32 位 hex 随机串并落盘（不要求密码学强度）。
    /// 仅用于把同一台设备的工单归组（「我的工单」状态查询），不含任何个人信息。
    #[serde(default)]
    pub feedback_aid: String,

    /* ── [lc-1661] 日志设置（独立报错日志 + 调试开关 + 组件过滤）──
       老 config.json 无这些键 → serde default：debug=true（保持原有常开行为）、
       modules 为空 = 全开。不需要迁移脚本。 */
    /// 调试日志总开关：false 时普通 info/debug 不再写 anibox.log；
    /// WARN/ERROR 级仍写 anibox-error.log（独立报错日志不受此开关影响）。
    #[serde(default = "default_true")]
    pub log_debug: bool,
    /// 组件级日志开关（键：mpv/bgm/mirror/feedback/history/webrs/other）。
    /// 缺省（空 map / 缺键）视为全开。
    #[serde(default)]
    pub log_modules: std::collections::BTreeMap<String, bool>,

    /* ── [lc-1662] 匿名使用统计开关 ──
       旧项目 `statsEnabled` 口径：**默认开启**（`getStatsEnabled()` 返回 `statsEnabled !== false`）。
       说明文案（旧项目「关于」页）：仅上报「随机匿名 ID + 版本号 + 系统类型 + 日期」，
       不含账号 / IP / 媒体库 / 文件路径；每天最多一次，可随时关闭。 */
    #[serde(default = "default_true")]
    pub usage_stats_enabled: bool,
}

fn default_cache_limit_mb() -> u64 {
    2048
}

fn default_exit_mode() -> String {
    "ask".into()
}

fn default_launch_page() -> String {
    "home".into()
}

fn default_true() -> bool {
    true
}

fn default_mpv() -> String {
    "D:/GitHub/Fntv-Plus/third_party/fntv-mpv/mpv.exe".into()
}

fn default_proxy() -> String {
    "D:/GitHub/Fntv-Plus/third_party/proxy/proxy.exe".into()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: String::new(),
            username: String::new(),
            token: String::new(),
            password: String::new(),
            auto_login: false,
            mpv_path: default_mpv(),
            proxy_path: default_proxy(),
            nas_proxy_enabled: false,
            cache_limit_mb: default_cache_limit_mb(),
            exit_mode: default_exit_mode(),
            perf_mode: false,
            carousel_logo: true,
            launch_page: default_launch_page(),
            bgm_token: String::new(),
            bgm_remember: true,
            bgm_username: String::new(),
            danmu_api_enabled: false,
            danmu_api_base: String::new(),
            danmu_api_bases: Vec::new(),
            feedback_aid: String::new(),
            log_debug: true,
            log_modules: std::collections::BTreeMap::new(),
            usage_stats_enabled: true,
        }
    }
}

fn config_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析数据目录: {}", e))?;
    Ok(dir.join("config.json"))
}

pub async fn load(app: &tauri::AppHandle) -> Config {
    let Ok(p) = config_path(app) else {
        return Config::default();
    };
    let s = match tokio::fs::read_to_string(&p).await {
        Ok(s) => s,
        Err(_) => return Config::default(),
    };
    match serde_json::from_str::<Config>(&s) {
        Ok(c) => c,
        Err(e) => {
            // [lc-1660] 解析失败不再静默丢配置：先把损坏文件备份，再回落默认。
            // 备份失败也不阻塞启动（下次 save 会覆盖），但至少尝试留痕。
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let bak = p.with_file_name(format!("config.json.bak-{ts}"));
            match tokio::fs::rename(&p, &bak).await {
                Ok(_) => crate::libmpv::trace(&format!(
                    "[config] 解析失败（{e}），已备份为 {}",
                    bak.display()
                )),
                Err(be) => crate::libmpv::trace(&format!(
                    "[config] 解析失败（{e}），备份也失败（{be}），使用默认配置"
                )),
            }
            Config::default()
        }
    }
}

/// [lc-1660] 落盘净化：`bgm_remember = false` 时清空磁盘上的 token（内存副本不受影响）。
/// 抽成纯函数便于单测。
fn for_disk(cfg: &Config) -> Config {
    let mut disk = cfg.clone();
    if !disk.bgm_remember {
        disk.bgm_token.clear();
    }
    disk
}

pub async fn save(app: &tauri::AppHandle, cfg: &Config) -> Result<(), String> {
    let p = config_path(app)?;
    if let Some(parent) = p.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建数据目录失败: {}", e))?;
    }
    let s = serde_json::to_string_pretty(&for_disk(cfg)).map_err(|e| e.to_string())?;
    // [lc-1660] 原子写：先写同目录临时文件，再 rename 覆盖。rename 失败时旧文件保持完整。
    let tmp = p.with_extension("json.tmp");
    tokio::fs::write(&tmp, s)
        .await
        .map_err(|e| format!("写入临时配置失败: {}", e))?;
    tokio::fs::rename(&tmp, &p).await.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("替换配置文件失败: {}", e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_true_is_true() {
        assert!(default_true());
    }

    #[test]
    fn new_fields_have_safe_defaults() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(c.bgm_remember, "缺省应记住账号");
        assert_eq!(c.bgm_username, "");
        assert_eq!(c.bgm_token, "");
    }

    #[test]
    fn log_settings_have_safe_defaults() {
        // 老 config.json 没有日志键 → debug 默认开、组件表为空（= 全开）
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(c.log_debug, "调试日志默认开启（保持原有行为）");
        assert!(c.log_modules.is_empty(), "缺省组件表为空 = 全开");

        // 显式关闭与组件过滤可被解析
        let c2: Config =
            serde_json::from_str(r#"{"log_debug":false,"log_modules":{"mpv":false}}"#).unwrap();
        assert!(!c2.log_debug);
        assert_eq!(c2.log_modules.get("mpv"), Some(&false));
    }

    #[test]
    fn usage_stats_defaults_to_on() {
        // 旧项目 statsEnabled 默认 true（statsEnabled !== false）
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(c.usage_stats_enabled, "匿名统计默认开启");
        let c2: Config = serde_json::from_str(r#"{"usage_stats_enabled":false}"#).unwrap();
        assert!(!c2.usage_stats_enabled);
    }

    #[test]
    fn for_disk_keeps_token_when_remembering() {
        let mut c = Config::default();
        c.bgm_token = "tok".into();
        c.bgm_remember = true;
        assert_eq!(for_disk(&c).bgm_token, "tok");
    }

    #[test]
    fn for_disk_clears_token_when_not_remembering() {
        let mut c = Config::default();
        c.bgm_token = "tok".into();
        c.bgm_remember = false;
        let disk = for_disk(&c);
        assert_eq!(disk.bgm_token, "");
        // 内存副本（入参）不被修改
        assert_eq!(c.bgm_token, "tok");
        // 其它账号字段保留
        c.bgm_username = "alice".into();
        assert_eq!(for_disk(&c).bgm_username, "alice");
    }
}
