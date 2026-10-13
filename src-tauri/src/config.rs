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
}

/// 默认 2GB（原内存层是 192MB；磁盘层给到 GB 级才装得下一批原图）
fn default_cache_limit_mb() -> u64 {
    2048
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
    match config_path(app) {
        Ok(p) => match tokio::fs::read_to_string(&p).await {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Config::default(),
        },
        Err(_) => Config::default(),
    }
}

pub async fn save(app: &tauri::AppHandle, cfg: &Config) -> Result<(), String> {
    let p = config_path(app)?;
    if let Some(parent) = p.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建数据目录失败: {}", e))?;
    }
    let s = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    tokio::fs::write(&p, s)
        .await
        .map_err(|e| format!("写入配置失败: {}", e))
}
