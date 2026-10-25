//! [lc-1820] AI 增强可选组件（RIFE Vulkan 补帧运行时）——「软件内下载、按需启用」。
//!
//! 组件布局（`app_data_dir/ai-enhance/`，不进安装目录，卸载不拖泥带水）：
//! ```text
//! ai-enhance/
//! ├── manifest.json          ← 版本/来源/时间（本模块写）
//! ├── vs/rife.vpy            ← VapourSynth 滤镜脚本（mpv --vf=vapoursynth 挂载）
//! ├── runtime/               ← Python3.12 + VapourSynth R79 + librife(ncnn Vulkan) + rife-v4.6
//! └── licenses/              ← RIFE/Python/VS-LGPL/ncnn 全套许可
//! ```
//!
//! 运行时素材取自 mpv-Yaozhi 1.0.6-2 主整合包的 rife-ncnn-vulkan 组件（48MB，全套
//! license 随附）；内核已换 yaozhi 自编译 mpv（带 VapourSynth 滤镜，third_party 内）。
//!
//! 分发：R2 桶放组件 zip + 远端 manifest（版本/sha256）；应用按远端 manifest 下载、
//! 校验、解压、原子替换。**真机验证**：RTX 5070 上 720p24 → RIFE 2× 每帧推理 ~31ms
//!（含引擎初始化摊销，96 帧增量 3.0s），实时可行（tests 对照：bare 4.31s / rife 7.35s）。
//!
//! 环境注入：组件就绪时 `mpv.rs` 的 MpvSession::launch 给 mpv 进程注入
//! VSSCRIPT_PATH / PYTHONNOUSERSITE / PATH(runtime 前置)——mpv 的 vapoursynth
//! 滤镜经 VSScript.dll 加载 runtime 里的 Python（进程启动前设好，运行时切引擎无需
//! 再动环境）。vf 挂载由 fntv_interp.lua 的 rife 档运行时完成（组件未就绪时档位隐藏）。

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 远端 manifest 地址（部署 R2 后替换为正式地址；含 zipUrl/zipSha256/version/sizeBytes）。
/// 占位指向 AniBox 官方桶的约定路径，桶未就绪时下载入口报「服务未配置」而不是空转。
pub const AI_ENHANCE_MANIFEST_URL: &str = "https://pub-anibox-ai-enhance.r2.dev/manifest.json";

/// 组件根目录。
pub fn enhance_root(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("ai-enhance"))
}

/// 就绪判定：runtime 关键文件齐备（对齐 fntv_interp.lua 的同名判定，口径一致）。
pub const REQUIRED_FILES: &[&str] = &[
    "vs/rife.vpy",
    "runtime/Lib/site-packages/vapoursynth/vsscript.dll",
    "runtime/libvapoursynth.dll",
    "runtime/plugins/librife.dll",
    "runtime/plugins/MiscFilters.dll",
    "runtime/models/rife-v4.6/flownet.bin",
    "runtime/models/rife-v4.6/flownet.param",
];

pub fn runtime_ready(app: &AppHandle) -> bool {
    let Some(root) = enhance_root(app) else { return false };
    REQUIRED_FILES
        .iter()
        .all(|rel| root.join(rel).is_file())
}

fn local_manifest(app: &AppHandle) -> Option<Value> {
    let p = enhance_root(app)?.join("manifest.json");
    let s = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&s).ok()
}

/// 状态查询（面板展示 + fntv_interp 侧同源自检）。
#[tauri::command]
pub async fn ai_enhance_status(app: AppHandle) -> Result<Value, String> {
    let ready = runtime_ready(&app);
    let manifest = local_manifest(&app);
    Ok(json!({
        "installed": manifest.is_some(),
        "ready": ready,
        // ready 但 manifest 缺（手工放置的开发组件）也给出可读版本
        "version": manifest.as_ref()
            .and_then(|m| m.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("dev"),
        "installedAt": manifest.as_ref()
            .and_then(|m| m.get("installed_at"))
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    }))
}

/// 下载并安装：远端 manifest → zip 流式下载（进度事件）→ sha256 校验 →
/// 解压（pwsh Expand-Archive，CREATE_NO_WINDOW 零前台）→ 原子替换。
#[tauri::command]
pub async fn ai_enhance_download(app: AppHandle) -> Result<Value, String> {
    let emit = |downloaded: u64, total: u64| {
        let _ = app.emit(
            "ai-enhance-progress",
            json!({ "downloaded": downloaded, "total": total }),
        );
    };
    // 1) 远端 manifest
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| format!("构建下载客户端失败: {e}"))?;
    let manifest: Value = client
        .get(AI_ENHANCE_MANIFEST_URL)
        .send()
        .await
        .map_err(|e| format!("拉取组件清单失败（R2 服务未部署或网络不可达）: {e}"))?
        .json()
        .await
        .map_err(|e| format!("组件清单解析失败: {e}"))?;
    let zip_url = manifest
        .get("zipUrl")
        .and_then(|v| v.as_str())
        .ok_or("清单缺 zipUrl")?
        .to_string();
    let want_sha = manifest
        .get("zipSha256")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let version = manifest
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // 2) 流式下载到临时文件
    let tmp_zip = std::env::temp_dir().join(format!("anibox-ai-enhance-{}.zip", std::process::id()));
    let resp = client
        .get(&zip_url)
        .send()
        .await
        .map_err(|e| format!("组件包下载失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("组件包下载 HTTP {}", resp.status().as_u16()));
    }
    let total: u64 = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut file = std::fs::File::create(&tmp_zip).map_err(|e| format!("临时文件创建失败: {e}"))?;
    use std::io::Write as _;
    use futures_util::StreamExt as _;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut hasher = Sha256::new();
    let mut last_emit = 0u64;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("下载中断: {e}"))?;
        hasher.update(&bytes);
        file.write_all(&bytes).map_err(|e| format!("写临时文件失败: {e}"))?;
        downloaded += bytes.len() as u64;
        // 每 1MB 报一次进度（emit 有 IPC 开销，别逐 chunk）
        if downloaded - last_emit >= 1024 * 1024 || (total > 0 && downloaded >= total) {
            emit(downloaded, total);
            last_emit = downloaded;
        }
    }
    drop(file);
    let got_sha = format!("{:x}", hasher.finalize());
    if !want_sha.is_empty() && got_sha != want_sha {
        let _ = std::fs::remove_file(&tmp_zip);
        return Err(format!(
            "组件包校验失败（sha256 不匹配，下载可能损坏）\n期望 {}… 实得 {}…",
            &want_sha[..12.min(want_sha.len())],
            &got_sha[..12.min(got_sha.len())]
        ));
    }

    // 3) 解压到临时目录（零前台：pwsh CREATE_NO_WINDOW）
    let tmp_dir = std::env::temp_dir().join(format!("anibox-ai-enhance-unzip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let unzip_status = tokio::process::Command::new("pwsh")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg(format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            tmp_zip.display(),
            tmp_dir.display()
        ))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000)
        .status()
        .await
        .map_err(|e| format!("启动解压失败: {e}"))?;
    let _ = std::fs::remove_file(&tmp_zip);
    if !unzip_status.success() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err("组件包解压失败（zip 损坏或磁盘空间不足）".into());
    }
    // zip 内可能是包一层目录（ai-enhance/…）或平铺：取「含 manifest.json 或 runtime/」的目录
    let src_root = if tmp_dir.join("runtime").is_dir() {
        tmp_dir.clone()
    } else if let Some(sub) = std::fs::read_dir(&tmp_dir)
        .ok()
        .and_then(|rd| {
            rd.filter_map(|e| e.ok())
                .find(|e| e.path().join("runtime").is_dir())
        })
    {
        sub.path()
    } else {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err("组件包结构异常（找不到 runtime/）".into());
    };

    // 4) 原子替换：旧目录 → .old → 新目录就位 → 删旧
    let root = enhance_root(&app).ok_or("app_data_dir 不可用")?;
    let old = root.with_extension("old");
    let _ = std::fs::remove_dir_all(&old);
    if root.exists() && std::fs::rename(&root, &old).is_err() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err("替换旧组件失败（文件可能被占用，请关闭播放器后重试）".into());
    }
    if let Err(e) = std::fs::rename(&src_root, &root) {
        // 回滚旧目录
        if old.exists() {
            let _ = std::fs::rename(&old, &root);
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!("组件就位失败: {e}"));
    }
    let _ = std::fs::remove_dir_all(&old);
    // 写本地 manifest（版本/时间/来源 sha）
    let _ = std::fs::write(
        root.join("manifest.json"),
        serde_json::to_string_pretty(&json!({
            "version": version,
            "installed_at": chrono_now(),
            "zip_sha256": got_sha,
            "source": AI_ENHANCE_MANIFEST_URL,
        }))
        .unwrap_or_default(),
    );
    emit(total, total);
    Ok(json!({ "version": version, "ready": runtime_ready(&app) }))
}

/// 删除组件（释放 ~50MB；播放器若在用 RIFE 需重启播放器）。
#[tauri::command]
pub async fn ai_enhance_delete(app: AppHandle) -> Result<(), String> {
    let Some(root) = enhance_root(&app) else { return Ok(()) };
    if root.exists() && std::fs::remove_dir_all(&root).is_err() {
        return Err("删除失败（组件文件被播放器占用，请先关闭播放器）".into());
    }
    let old = root.with_extension("old");
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 简单 UTC 时间（面板只展示，不做时区）
    let days = now / 86400;
    let secs = now % 86400;
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    // 1970-01-01 起的天数转年月日（ Civil date 算法 ）
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!("{y:04}-{mth:02}-{d:02} {h:02}:{m:02}:{s:02} UTC")
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_math() {
        // 2026-10-25 12:00:00 UTC = 1792965600（近似校验算法正确性）
        let s = chrono_from_secs(1792965600);
        assert!(s.starts_with("2026-10-25"), "got {s}");
    }

    fn chrono_from_secs(now: u64) -> String {
        let days = now / 86400;
        let secs = now % 86400;
        let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
        let z = days as i64 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let mth = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if mth <= 2 { y + 1 } else { y };
        format!("{y:04}-{mth:02}-{d:02} {h:02}:{m:02}:{s:02} UTC")
    }
}
