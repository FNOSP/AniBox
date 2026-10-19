//! 应用配置：app_data_dir/config.json。PoC 默认复用 Fntv-Plus 的 mpv / Go 代理分发。

use serde::{Deserialize, Serialize};

// ───────────────────────── [访问码] 磁盘加密 ─────────────────────────
//
// 背景：旧项目 Fntv-Plus 的 `fn_config/config.ts` 把访问码用 AES-256-CBC 加密后写进
// config.json（密钥是源码里的固定 32 字符串，IV 全零）。AniBox 之前是明文落盘，本模块
// 补齐等价能力。
//
// 方案：AES-256-GCM（带认证标签，能发现篡改/损坏），密钥由**与旧项目相同的固定种子**
// SHA-256 派生（不直接把种子当密钥用，也不把密钥/明文/密文写进任何日志或错误信息）。
// 存储格式：`enc:v1:<base64(nonce[12] || ciphertext+tag)>`。
// 兼容：读到无前缀的值按旧明文处理；下次 save 时自动转成密文。空值仍存空串。
const ACCESS_CODE_ENC_PREFIX: &str = "enc:v1:";
/// 与 Fntv-Plus 相同的密钥种子（仅用于 KDF；换掉会导致旧密文无法解密，勿改）。
const ACCESS_CODE_KEY_SEED: &str = "U2XDcFsV6rdTE9wB5ZHvy6BW9hBTKJ1H";
const ACCESS_CODE_NONCE_LEN: usize = 12;

fn access_code_key() -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(ACCESS_CODE_KEY_SEED.as_bytes());
    hasher.finalize().into()
}

/// 访问码是否已是本模块产出的密文格式。
pub fn is_encrypted_access_code(value: &str) -> bool {
    value.starts_with(ACCESS_CODE_ENC_PREFIX)
}

/// 加密访问码。空串保持空串（表示未设置）；已带前缀的值原样返回（避免重复加密）。
pub fn encrypt_access_code(plain: &str) -> String {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    use base64::Engine as _;

    if plain.is_empty() || is_encrypted_access_code(plain) {
        return plain.to_string();
    }
    let cipher = Aes256Gcm::new_from_slice(&access_code_key()).expect("AES-256 密钥长度固定");
    let nonce_bytes: [u8; ACCESS_CODE_NONCE_LEN] = rand::random();
    let nonce = Nonce::from_slice(&nonce_bytes);
    // 只有超长（> 2^36 字节）才会失败，正常访问码不可能触发。
    let ciphertext = cipher
        .encrypt(nonce, plain.as_bytes())
        .expect("AES-256-GCM 加密失败");
    let mut blob = Vec::with_capacity(ACCESS_CODE_NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    format!(
        "{}{}",
        ACCESS_CODE_ENC_PREFIX,
        base64::engine::general_purpose::STANDARD.encode(blob)
    )
}

/// 解密访问码。无前缀 = 旧明文，原样返回；损坏/无法解密的密文返回空串
/// （与旧项目 decryptSafe 一致：单字段失败不炸整个配置读取）。
pub fn decrypt_access_code(stored: &str) -> String {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    use base64::Engine as _;

    if stored.is_empty() {
        return String::new();
    }
    let Some(encoded) = stored.strip_prefix(ACCESS_CODE_ENC_PREFIX) else {
        return stored.to_string();
    };
    let Ok(blob) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return String::new();
    };
    if blob.len() <= ACCESS_CODE_NONCE_LEN {
        return String::new();
    }
    let (nonce_bytes, ciphertext) = blob.split_at(ACCESS_CODE_NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(&access_code_key()).expect("AES-256 密钥长度固定");
    match cipher.decrypt(Nonce::from_slice(nonce_bytes), ciphertext) {
        Ok(plain) => String::from_utf8(plain).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

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
    /// [访问码] fnOS 网关「应用访问码」：留空表示服务器未开启门禁。
    /// 与旧项目一致，登录时用于预验证并在启动时重建网关会话（敏感值不写日志）。
    /// **内存中始终是明文**；落盘时由 `for_disk` 加密为 `enc:v1:<base64>`（见本文件顶部）。
    #[serde(default)]
    pub access_code: String,
    /// 启动自动登录：token 失效时用记住的密码静默重登（依赖 password 非空）
    #[serde(default)]
    pub auto_login: bool,
    #[serde(default = "default_mpv")]
    pub mpv_path: String,
    /// 播放器类型："mpv"（随包自带，默认）| "potplayer"（本机安装的 PotPlayer）
    /// 老 config.json 无此键 → 默认 mpv，行为与接入前完全一致。
    #[serde(default = "default_player_kind")]
    pub player_kind: String,
    /// PotPlayer 可执行文件路径（空 = 自动探测常见安装位置）
    #[serde(default)]
    pub potplayer_path: String,
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
    /// 启动页："home"（首页，默认）| "discover"（发现页）
    /// 历史取值 "continue" 已移除；老 config 读到后由前端归一为 home。
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
       应用现已**内置**一份 danmu_api（本地独立运行、零配置）：用户不填地址时自动使用
       `http://127.0.0.1:22352/<TOKEN>`；也可继续填写自部署地址覆盖。
       作为弹幕**优选源**：命中即用，未命中自动降级内置 B站。
       默认开启 = 开箱即用内置源；关闭则完全走内置 B站 链路。 */
    /// 自建弹幕接口开关（默认开启：使用内置实例或用户填写的地址）
    #[serde(default = "default_true")]
    pub danmu_api_enabled: bool,
    /// 单地址（恒等于 bases[0]，兼容旧读取方与 uosc conf 同步）
    #[serde(default)]
    pub danmu_api_base: String,
    /// 多地址列表（故障转移；主仓库上限 4 条）
    #[serde(default)]
    pub danmu_api_bases: Vec<String>,
    /// 弹幕引擎："node"（默认，行为与接入前完全一致）| "rust"（Rust 直接接管 22347，
    /// 自建源三跳由 Rust 实现，B站/字幕端点反代到内部 Node 端口）。
    /// 环境变量 `ANIBOX_DANMAKU_ENGINE=rust` 可临时覆盖（便于测试）。
    #[serde(default = "default_danmu_engine")]
    pub danmu_engine: String,

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
    /// [lc-1663] 机器级匿名 ID（系统机器标识 SHA-256 前 31 位 hex；拿不到时回退 32 位随机 hex）。
    #[serde(default)]
    pub usage_stats_anon_id: String,
    /// 上次上报日期 YYYY-MM-DD（空 = 从未上报）
    #[serde(default)]
    pub usage_stats_last_ping_day: String,
    /// 上次上报是否成功（仅用于状态展示）
    #[serde(default)]
    pub usage_stats_last_ping_ok: bool,
    /// 上报失败攒下的欠报日期（YYYY-MM-DD，最多 7 天，网络恢复后补发）
    #[serde(default)]
    pub usage_stats_pending_days: Vec<String>,

    /* ── [gamepad] 全局手柄（原生 gilrs 监听，mpv/PotPlayer 在前台时同样可用）──
       全部带 serde default：老 config.json 没有这些键时按默认值走，**不需要迁移脚本**。
       语义与取值范围见设置面板契约（get_settings/set_settings 做白名单 + clamp）。 */
    /// 手柄总开关（默认开：接上手柄即可直接使用；用户可在设置里显式关闭）
    /// 注意：bool 的 `#[serde(default)]` 默认 false，这里必须用自定义默认函数，
    /// 否则旧 config.json 缺该字段时会回落到 false。
    #[serde(default = "default_gamepad_enabled")]
    pub gamepad_enabled: bool,
    /// 左摇杆死区（0.10–0.50，默认 0.30）：模长低于此值视为回中
    #[serde(default = "default_gamepad_deadzone")]
    pub gamepad_deadzone: f32,
    /// 方向触发阈值（0.20–0.80，默认 0.40）：单轴绝对值达到才判为该方向
    #[serde(default = "default_gamepad_direction_threshold")]
    pub gamepad_direction_threshold: f32,
    /// 长按首次重复延迟（毫秒，100–800，默认 320）
    #[serde(default = "default_gamepad_repeat_delay")]
    pub gamepad_repeat_delay: u32,
    /// 长按重复间隔（毫秒，50–400，默认 130）
    #[serde(default = "default_gamepad_repeat_interval")]
    pub gamepad_repeat_interval: u32,

    /* ── [ui-opacity] 界面透明度（设置面板「外观」）──
       纯前端外观项，存这里是为了重启后保持；老 config.json 缺该键 → 默认 85。 */
    /// 界面不透明度百分比（60–100，默认 85）：应用外壳底色与侧栏玻璃面板。
    #[serde(default = "default_ui_opacity")]
    pub ui_opacity: u8,
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

/// [gamepad] 手柄总开关默认开启。独立成函数而不是复用 `default_true`，
/// 便于以后单独调整手柄默认值，也让 serde 属性自解释。
fn default_gamepad_enabled() -> bool {
    true
}

/// [bundle-mpv] 默认留空：真实路径在 `load()` 里由 `resolve_bundled_paths` 按
/// 「安装目录 resource_dir → exe 同级 → 开发仓库」顺序解析。写死开发机绝对路径
/// 会让安装包在用户机器上必然找不到 mpv/proxy。
fn default_mpv() -> String {
    String::new()
}

fn default_player_kind() -> String {
    "mpv".into()
}

fn default_proxy() -> String {
    String::new()
}

fn default_danmu_engine() -> String {
    "node".into()
}

// [gamepad] 取值边界集中在这里：config 默认值、set_settings 的 clamp、gamepad 运行时共用。
pub const GAMEPAD_DEADZONE_MIN: f32 = 0.10;
pub const GAMEPAD_DEADZONE_MAX: f32 = 0.50;
pub const GAMEPAD_DIRECTION_THRESHOLD_MIN: f32 = 0.20;
pub const GAMEPAD_DIRECTION_THRESHOLD_MAX: f32 = 0.80;
pub const GAMEPAD_REPEAT_DELAY_MIN: u32 = 100;
pub const GAMEPAD_REPEAT_DELAY_MAX: u32 = 800;
pub const GAMEPAD_REPEAT_INTERVAL_MIN: u32 = 50;
pub const GAMEPAD_REPEAT_INTERVAL_MAX: u32 = 400;

fn default_gamepad_deadzone() -> f32 {
    0.30
}

fn default_gamepad_direction_threshold() -> f32 {
    0.40
}

fn default_gamepad_repeat_delay() -> u32 {
    320
}

fn default_gamepad_repeat_interval() -> u32 {
    130
}

// [ui-opacity] 取值边界集中在这里：config 默认值、set_settings 的 clamp、前端滑块共用口径。
pub const UI_OPACITY_MIN: u8 = 60;
pub const UI_OPACITY_MAX: u8 = 100;

fn default_ui_opacity() -> u8 {
    85
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: String::new(),
            username: String::new(),
            token: String::new(),
            password: String::new(),
            access_code: String::new(),
            auto_login: false,
            mpv_path: default_mpv(),
            player_kind: default_player_kind(),
            potplayer_path: String::new(),
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
            danmu_api_enabled: true,
            danmu_api_base: String::new(),
            danmu_api_bases: Vec::new(),
            danmu_engine: default_danmu_engine(),
            feedback_aid: String::new(),
            log_debug: true,
            log_modules: std::collections::BTreeMap::new(),
            usage_stats_enabled: true,
            usage_stats_anon_id: String::new(),
            usage_stats_last_ping_day: String::new(),
            usage_stats_last_ping_ok: false,
            usage_stats_pending_days: Vec::new(),
            gamepad_enabled: default_gamepad_enabled(),
            gamepad_deadzone: default_gamepad_deadzone(),
            gamepad_direction_threshold: default_gamepad_direction_threshold(),
            gamepad_repeat_delay: default_gamepad_repeat_delay(),
            gamepad_repeat_interval: default_gamepad_repeat_interval(),
            ui_opacity: default_ui_opacity(),
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

/// [bundle-mpv] 随包运行时资源的查找根。
///
/// 打包态（NSIS，installMode=currentUser）资源被放在安装目录（= `resource_dir()`）：
///   `<安装目录>\third_party\fntv-mpv\mpv.exe`
///   `<安装目录>\third_party\proxy\proxy.exe`
/// 开发态没有复制资源，回落到仓库根的 `third_party/`（`src-tauri/..`）。
fn bundled_candidates(app: &tauri::AppHandle, rel: &str) -> Vec<std::path::PathBuf> {
    use tauri::Manager;
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        out.push(dir.join(rel));
        out.push(dir.join("resources").join(rel));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join(rel));
            out.push(dir.join("resources").join(rel));
        }
    }
    // 开发态：src-tauri/../third_party/...（以及 src-tauri/third_party/... 的兼容位）
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    out.push(manifest.join("..").join(rel));
    out.push(manifest.join(rel));
    out
}

fn find_bundled(app: &tauri::AppHandle, rel: &str) -> Option<std::path::PathBuf> {
    bundled_candidates(app, rel)
        .into_iter()
        .find(|p| p.is_file())
}

fn path_is_file(p: &str) -> bool {
    !p.trim().is_empty() && std::path::Path::new(p).is_file()
}

/// [bundle-mpv] 把空路径 / 失效路径（例如从开发机迁移来的 `D:/GitHub/AniBox/...`）
/// 解析到随包资源。已有的有效用户配置保持不变。
///
/// 必须在 `config::load()` 里执行：mpv / proxy 的所有使用点（player、mpv、
/// danmaku_shim、proxy 预热）都直接读 `cfg.mpv_path` / `cfg.proxy_path`，
/// 在入口归一化一次即可，无需改每个调用点。
pub fn resolve_bundled_paths(app: &tauri::AppHandle, cfg: &mut Config) {
    if !path_is_file(&cfg.mpv_path) {
        if let Some(p) = find_bundled(app, "third_party/fntv-mpv/mpv.exe") {
            cfg.mpv_path = p.to_string_lossy().into_owned();
        }
    }
    if !path_is_file(&cfg.proxy_path) {
        if let Some(p) = find_bundled(app, "third_party/proxy/proxy.exe") {
            cfg.proxy_path = p.to_string_lossy().into_owned();
        }
    }
}

pub async fn load(app: &tauri::AppHandle) -> Config {
    let mut cfg = load_raw(app).await;
    resolve_bundled_paths(app, &mut cfg);
    cfg
}

async fn load_raw(app: &tauri::AppHandle) -> Config {
    let Ok(p) = config_path(app) else {
        return Config::default();
    };
    let s = match tokio::fs::read_to_string(&p).await {
        Ok(s) => s,
        Err(_) => return Config::default(),
    };
    match serde_json::from_str::<Config>(&s) {
        Ok(c) => from_disk(c),
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
/// [访问码] 同时把 access_code 转成密文：内存里始终是明文（登录/会话重建直接用），
/// 只有写到磁盘的这一份加密。抽成纯函数便于单测。
fn for_disk(cfg: &Config) -> Config {
    let mut disk = cfg.clone();
    if !disk.bgm_remember {
        disk.bgm_token.clear();
    }
    disk.access_code = encrypt_access_code(&disk.access_code);
    disk
}

/// 磁盘 → 内存：解密 access_code（旧明文原样保留），其余字段不变。
fn from_disk(mut cfg: Config) -> Config {
    cfg.access_code = decrypt_access_code(&cfg.access_code);
    cfg
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
    fn player_fields_have_backward_compatible_defaults() {
        // 老 config.json 没有播放器键 → 默认自带 mpv、PotPlayer 路径为空（自动探测）
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.player_kind, "mpv", "默认播放器必须是自带 mpv");
        assert_eq!(c.potplayer_path, "");

        // 显式选择 PotPlayer 可被解析
        let c2: Config =
            serde_json::from_str(r#"{"player_kind":"potplayer","potplayer_path":"C:\\P\\PotPlayerMini64.exe"}"#)
                .unwrap();
        assert_eq!(c2.player_kind, "potplayer");
        assert_eq!(c2.potplayer_path, r"C:\P\PotPlayerMini64.exe");
    }

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
        // [访问码] 老 config.json 无该键 → 默认空（服务器未开启门禁）
        assert_eq!(c.access_code, "");
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
        // [lc-1663] 运行态字段默认空
        assert!(c2.usage_stats_anon_id.is_empty());
        assert!(c2.usage_stats_last_ping_day.is_empty());
        assert!(!c2.usage_stats_last_ping_ok);
        assert!(c2.usage_stats_pending_days.is_empty());
    }

    #[test]
    fn gamepad_fields_have_safe_defaults() {
        // 老 config.json 没有手柄键 → 默认开启 + 契约默认值（不需要迁移）
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(c.gamepad_enabled, "手柄默认开启");
        assert_eq!(c.gamepad_deadzone, 0.30);
        assert_eq!(c.gamepad_direction_threshold, 0.40);
        assert_eq!(c.gamepad_repeat_delay, 320);
        assert_eq!(c.gamepad_repeat_interval, 130);

        // 显式值可被解析
        let c2: Config = serde_json::from_str(
            r#"{"gamepad_enabled":true,"gamepad_deadzone":0.25,"gamepad_direction_threshold":0.5,"gamepad_repeat_delay":200,"gamepad_repeat_interval":100}"#,
        )
        .unwrap();
        assert!(c2.gamepad_enabled);
        assert_eq!(c2.gamepad_deadzone, 0.25);
        assert_eq!(c2.gamepad_direction_threshold, 0.5);
        assert_eq!(c2.gamepad_repeat_delay, 200);
        assert_eq!(c2.gamepad_repeat_interval, 100);
    }

    #[test]
    fn ui_opacity_has_safe_default() {
        // 老 config.json 没有 ui_opacity → 默认 85（不需要迁移）
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.ui_opacity, 85);
        // 显式值可被解析
        let c2: Config = serde_json::from_str(r#"{"ui_opacity":75}"#).unwrap();
        assert_eq!(c2.ui_opacity, 75);
        assert_eq!(UI_OPACITY_MIN, 60);
        assert_eq!(UI_OPACITY_MAX, 100);
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

    // ───────────────────────── [访问码] 加解密 ─────────────────────────

    #[test]
    fn access_code_encrypt_decrypt_roundtrip() {
        for plain in ["1234", "a-b_c 中文 访问码", "x"] {
            let enc = encrypt_access_code(plain);
            assert!(enc.starts_with("enc:v1:"), "密文必须带版本前缀");
            assert_ne!(enc, plain, "密文不能等于明文");
            assert_eq!(decrypt_access_code(&enc), plain, "解密必须还原明文");
        }
    }

    #[test]
    fn access_code_encryption_uses_random_nonce() {
        // 同一明文两次加密结果应不同（随机 nonce），但都能解回同一明文
        let a = encrypt_access_code("same-code");
        let b = encrypt_access_code("same-code");
        assert_ne!(a, b, "随机 nonce 应让密文每次不同");
        assert_eq!(decrypt_access_code(&a), "same-code");
        assert_eq!(decrypt_access_code(&b), "same-code");
    }

    #[test]
    fn access_code_legacy_plaintext_is_passed_through() {
        // 旧 config.json 的明文访问码：读取时原样返回（兼容），不被当成密文
        assert_eq!(decrypt_access_code("legacy-plain-123"), "legacy-plain-123");
        assert!(!is_encrypted_access_code("legacy-plain-123"));
        // 且下一次落盘会被加密
        let mut c = Config::default();
        c.access_code = "legacy-plain-123".into();
        let disk = for_disk(&c);
        assert!(is_encrypted_access_code(&disk.access_code));
        assert_eq!(decrypt_access_code(&disk.access_code), "legacy-plain-123");
        // 内存副本保持明文
        assert_eq!(c.access_code, "legacy-plain-123");
    }

    #[test]
    fn access_code_empty_stays_empty() {
        assert_eq!(encrypt_access_code(""), "");
        assert_eq!(decrypt_access_code(""), "");
        let disk = for_disk(&Config::default());
        assert_eq!(disk.access_code, "", "空访问码落盘仍为空串");
    }

    #[test]
    fn access_code_corrupt_ciphertext_yields_empty() {
        use base64::Engine as _;
        // 损坏的密文不能让配置读取崩溃，返回空串（旧项目 decryptSafe 语义）
        assert_eq!(decrypt_access_code("enc:v1:not-base64!!"), "");
        assert_eq!(decrypt_access_code("enc:v1:"), "");
        // 合法 base64 但长度不足 / 认证失败
        let short = format!("enc:v1:{}", base64::engine::general_purpose::STANDARD.encode([0u8; 8]));
        assert_eq!(decrypt_access_code(&short), "");
        let tampered = {
            let mut enc = encrypt_access_code("secret").into_bytes();
            let last = enc.len() - 1;
            enc[last] = if enc[last] == b'A' { b'B' } else { b'A' };
            String::from_utf8(enc).unwrap()
        };
        assert_eq!(decrypt_access_code(&tampered), "", "GCM 认证应拒绝篡改密文");
    }

    #[test]
    fn access_code_is_encrypted_in_serialized_disk_config() {
        // 端到端：内存明文 → for_disk → JSON 序列化 → 反序列化 → from_disk 还原明文
        let mut c = Config::default();
        c.access_code = "9876".into();
        let json = serde_json::to_string(&for_disk(&c)).unwrap();
        assert!(!json.contains("9876"), "落盘 JSON 不得包含访问码明文");
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert!(is_encrypted_access_code(&parsed.access_code));
        assert_eq!(from_disk(parsed).access_code, "9876");
    }

    #[test]
    fn access_code_already_encrypted_is_not_double_encrypted() {
        let once = encrypt_access_code("1234");
        assert_eq!(encrypt_access_code(&once), once, "重复加密应保持幂等");
    }
}
