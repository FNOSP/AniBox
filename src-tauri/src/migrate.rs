//! [lc-1440] 改名（Fntv-Plus Tauri2 → **AniBox**）的数据目录迁移。
//!
//! 为什么必须有这一步：Tauri 的 `app_data_dir()` 由 `tauri.conf.json` 的 **identifier** 决定，
//! 而 identifier 从 `com.fntv.plus.tauri2` 改成了 `com.anibox.app` ⇒ 新旧目录完全是两个路径。
//! 不迁移的后果是"用户数据凭空消失"：
//!   · `%APPDATA%\com.fntv.plus.tauri2\config.json` —— 登录服务器/账号/token/mpv 路径全在里面
//!   · `%APPDATA%\...\cache` —— 图片与详情落盘缓存（实测 125MB，120 天 TTL）
//!   · `%LOCALAPPDATA%\...\EBWebView` —— WebView2 配置目录，**localStorage 在里面**
//!     （主题、侧栏状态、隐藏媒体库、设置面板镜像、首页快照；实测 308MB）
//!
//! **时机是硬约束**：WebView2 一旦创建就会**锁住**它的 profile 目录（`EBWebView` 里的
//! leveldb 被进程持有），那时再想搬 LOCALAPPDATA 就会失败。所以本函数必须在
//! `tauri::Builder` **之前**调用（见 `run()` 开头）。
//!
//! 策略：**只在新目录不存在、旧目录存在时**迁移。复制到同目录下的临时名、最后一步
//! `rename` 原子落位 —— 中途崩溃只会留下 tmp（下次启动清掉重来），绝不会出现
//! "半份新目录被当作已迁移"。旧目录**不删除**——迁移是"抄一份过去"，
//! 旧目录留作回滚余地（用户确认新版没问题后可自行删除）。

use std::path::{Path, PathBuf};

/// 旧 identifier（改动前的值，写死在这里：迁移是一次性的，读配置反而增加失败面）
const OLD_ID: &str = "com.fntv.plus.tauri2";
/// 新 identifier（与 tauri.conf.json 的 identifier 必须一致）
const NEW_ID: &str = "com.anibox.app";

/// 迁移结果（只用于日志，调用方不需要分支处理）
pub struct Report {
    pub appdata: Option<String>,
    pub localappdata: Option<String>,
}

fn roaming() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

fn local() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

/// 把一个目录搬到新位置。
/// 返回 Some(描述) = 做了迁移；None = 无需迁移（新目录已在 / 旧目录不在 / 环境变量缺失）。
fn move_dir(root: &Path, label: &str) -> Option<String> {
    let old = root.join(OLD_ID);
    let new = root.join(NEW_ID);
    // 新目录已存在 ⇒ 已经迁移过（或用户全新安装后跑过），绝不再动
    if new.exists() {
        return None;
    }
    // 旧目录不存在 ⇒ 全新用户，没什么可搬
    if !old.exists() {
        return None;
    }
    // 先复制到同目录下的临时名，最后一步 rename 原子落位（见文件头：防"半份迁移"）
    let tmp = root.join(format!("{NEW_ID}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    match copy_dir(&old, &tmp) {
        Ok(n) => match std::fs::rename(&tmp, &new) {
            Ok(()) => Some(format!("{label}: 复制 {n} 个文件")),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp);
                Some(format!("{label}: 迁移失败（{e}）—— 将按全新安装启动"))
            }
        },
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Some(format!("{label}: 迁移失败（{e}）—— 将按全新安装启动"))
        }
    }
}

/// 递归复制（只处理文件与目录；不做符号链接跟随 —— 这两个目录里不该有链接）
fn copy_dir(src: &Path, dst: &Path) -> Result<u64, String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建 {} 失败: {e}", dst.display()))?;
    let mut n = 0u64;
    for entry in std::fs::read_dir(src).map_err(|e| format!("读 {} 失败: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        if ft.is_dir() {
            n += copy_dir(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to).map_err(|e| format!("复制 {} 失败: {e}", from.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

/// 在 Tauri 初始化**之前**调用（见 run() 开头）。
pub fn run() -> Report {
    Report {
        appdata: roaming().as_deref().and_then(|r| move_dir(r, "APPDATA")),
        localappdata: local().as_deref().and_then(|r| move_dir(r, "LOCALAPPDATA")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 迁移的**三条边界**（离线、不碰真实 APPDATA）：
    ///   ① 新目录已在 → 不动（防重复迁移覆盖新数据）
    ///   ② 旧目录不在 → 不动（全新用户）
    ///   ③ 只有旧目录在 → 搬过去，且**内容一致**、旧目录保留
    /// 这条逻辑错了会毁用户数据（登录态/缓存），所以必须有确定性测试钉住。
    #[test]
    fn move_dir_only_when_new_absent_and_old_present() {
        let base = std::env::temp_dir().join(format!("anibox-migrate-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        // ② 全新用户：旧目录不存在 → 不迁移
        std::fs::create_dir_all(&base).unwrap();
        assert!(move_dir(&base, "t").is_none(), "旧目录不存在时不该迁移");

        // ③ 只有旧目录 → 迁移，内容跟着走，旧目录保留
        let old = base.join(OLD_ID);
        std::fs::create_dir_all(old.join("cache/sub")).unwrap();
        std::fs::write(old.join("config.json"), b"{\"server\":\"x\"}").unwrap();
        std::fs::write(old.join("cache/sub/a.bin"), b"abc").unwrap();
        let r = move_dir(&base, "t");
        assert!(r.is_some(), "旧目录在、新目录不在时必须迁移");
        let new = base.join(NEW_ID);
        assert_eq!(std::fs::read(new.join("config.json")).unwrap(), b"{\"server\":\"x\"}");
        assert_eq!(std::fs::read(new.join("cache/sub/a.bin")).unwrap(), b"abc");
        assert!(old.exists(), "旧目录必须保留（回滚余地）");

        // ① 新目录已在 → 不再动（哪怕旧目录也在）
        std::fs::write(new.join("config.json"), b"{\"server\":\"NEW\"}").unwrap();
        assert!(move_dir(&base, "t").is_none(), "新目录已存在时不该再迁移");
        assert_eq!(
            std::fs::read(new.join("config.json")).unwrap(),
            b"{\"server\":\"NEW\"}",
            "已有新数据不能被旧目录覆盖"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// identifier 常量必须与 tauri.conf.json 一致 —— 不一致会导致"迁到的目录不是程序真正读的目录"
    /// （表现为迁移"成功"但用户数据依然全丢）。这里直接读配置钉住它。
    #[test]
    fn new_id_matches_tauri_conf() {
        let conf = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"),
        )
        .expect("读 tauri.conf.json");
        assert!(
            conf.contains(&format!("\"identifier\": \"{NEW_ID}\"")),
            "migrate.rs 的 NEW_ID({NEW_ID}) 与 tauri.conf.json 的 identifier 不一致"
        );
    }
}
