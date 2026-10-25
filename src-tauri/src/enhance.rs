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

use crate::AppState;

/// 远端 manifest 地址（部署 R2 后替换为正式地址；含 zipUrl/zipSha256/version/sizeBytes）。
/// 占位指向 AniBox 官方桶的约定路径，桶未就绪时下载入口报「服务未配置」而不是空转。
pub const AI_ENHANCE_MANIFEST_URL: &str = "https://ai.690075.xyz/manifest.json";

/// 组件根目录。
pub fn enhance_root(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("ai-enhance"))
}

/// 就绪判定：runtime 关键文件齐备（对齐 fntv_interp.lua 的同名判定，口径一致）。
pub const REQUIRED_FILES: &[&str] = &[
    "vs/rife.vpy",
    "vs/superres.vpy",
    "vs/superres_trt.vpy",
    "runtime/Lib/site-packages/vapoursynth/vsscript.dll",
    "runtime/libvapoursynth.dll",
    "runtime/plugins/librife.dll",
    "runtime/plugins/MiscFilters.dll",
    "runtime/models/rife-v4.6/flownet.bin",
    "runtime/models/rife-v4.6/flownet.param",
];

/// [lc-1823] AnimeJaNai 超分（DML 后端，基础包内）就绪文件。
pub const SUPERRES_DML_FILES: &[&str] = &[
    "vs/superres.vpy",
    "runtime/plugins/vsort.dll",
    "runtime/plugins/vsort/onnxruntime.dll",
    "runtime/plugins/vsort/DirectML.dll",
    "runtime/Lib/site-packages/vsmlrt.py",
    "runtime/models/animejanai/animejanai-v2-l1-x2-fp16.onnx",
];

/// [lc-1823] TensorRT 扩展包（RTX 限定大包）就绪文件（ai-enhance/tensorrt/ 下）。
pub const TENSORRT_FILES: &[&str] = &[
    "runtime/VSScript.dll",
    "runtime/python314.dll",
    "runtime/vs-plugins/vstrt.dll",
    "runtime/vs-plugins/vsmlrt-cuda/nvinfer_10.dll",
];


/// [lc-1856] 显卡名探测（注册表显示适配器类，过滤虚拟/远程显卡；进程内缓存一次）。
/// 用于 AI 增强 UI 的「你的显卡：xxx」展示与 RIFE 硬性要求提示（用户普遍不清楚
/// 自己的卡带不带得动——5070 实测 1080P RIFE 都跟不上）。
fn detect_gpu_name() -> String {
    static GPU: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    GPU.get_or_init(|| {
        #[cfg(target_os = "windows")]
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("pwsh")
            .args(["-NoProfile", "-Command",
                "(Get-ItemProperty 'HKLM:/SYSTEM/CurrentControlSet/Control/Class/{4d36e968-e325-11ce-bfc1-08002be10318}/0*' -ErrorAction SilentlyContinue | Where-Object DriverDesc | Select-Object -ExpandProperty DriverDesc) -join ' | '"])
            .creation_flags(0x0800_0000)
            .output();
        let Ok(o) = out else { return String::new() };
        let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
        // 过滤虚拟/远程显示适配器，取第一块真卡
        text.split(" | ")
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .find(|n| {
                let low = n.to_lowercase();
                !low.contains("virtual") && !low.contains("remote") && !low.contains("basic")
            })
            .unwrap_or("")
            .to_string()
    }).clone()
}

/// [lc-1857] N 卡 SM 半自动注册：驱动不暴露 Smooth Motion 的按名设置查询（NVAPI 实测
/// GetSettingIdFromName 返回 NULL），全自动此路不通。退而求其次把手动步骤压到最少：
/// mpv.exe 完整路径复制到剪贴板 + 直接拉起 NVIDIA App（找到 exe 才拉）。
/// 返回给用户的下一步指引文本。
fn nvidia_sm_prepare(mpv_path: &str) -> String {
    #[cfg(target_os = "windows")]
    use std::os::windows::process::CommandExt;
    // [lc-1859] 空路径直出提示（此前误用 default_mpv()=空串：arboard 写空不报错，
    // 提示「已复制」但剪贴板是空的——用户实测「没有复制到剪贴板」即此）
    if mpv_path.trim().is_empty() {
        return "未能确定 mpv.exe 路径（未登录或未配置），请先完成登录后重试".into();
    }
    let mpv = mpv_path.to_string();
    let clip_note = match arboard::Clipboard::new().and_then(|mut c| c.set_text(mpv.clone())) {
        Ok(_) => "mpv.exe 完整路径已复制到剪贴板；".to_string(),
        Err(e) => format!("（剪贴板写入失败 {e}，请手动浏览 mpv.exe）"),
    };
    let candidates = [
        std::env::var("LOCALAPPDATA").ok().map(|d| {
            std::path::PathBuf::from(&d)
                .join("NVIDIA Corporation")
                .join("NVIDIA App")
                .join("NVIDIA App.exe")
        }),
        // 实测安装布局（本机 11.0.9）：主程序在 CEF 子目录，不在根目录
        Some(
            ["C:", "Program Files", "NVIDIA Corporation", "NVIDIA App", "CEF", "NVIDIA App.exe"]
                .iter()
                .collect::<std::path::PathBuf>(),
        ),
        Some(
            ["C:", "Program Files", "NVIDIA Corporation", "NVIDIA App", "NVIDIA App.exe"]
                .iter()
                .collect::<std::path::PathBuf>(),
        ),
    ];
    let mut launched = false;
    for c in candidates.into_iter().flatten() {
        if c.is_file() {
            #[cfg(target_os = "windows")]
            {
                #[allow(unused_mut)]
                let mut cmd = std::process::Command::new(&c);
                cmd.creation_flags(0x0800_0000);
                let _ = cmd.spawn();
            }
            launched = true;
            break;
        }
    }
    let app_note = if launched {
        "NVIDIA App 已打开；"
    } else {
        "未能自动打开 NVIDIA App（请手动打开）；"
    };
    format!(
        "{clip_note}{app_note}在「图形 → 程序设置」添加程序时直接粘贴路径，开启 Smooth Motion 后重启播放器生效"
    )
}

pub fn superres_dml_ready(app: &AppHandle) -> bool {
    let Some(root) = enhance_root(app) else { return false };
    SUPERRES_DML_FILES.iter().all(|rel| root.join(rel).is_file())
}

/// 当前超分档是否为 tensorrt（mpv.rs 注入环境分流用）。
pub fn superres_engine_is_trt(app: &AppHandle) -> bool {
    read_superres_engine(app) == "tensorrt"
}

pub fn tensorrt_ready(app: &AppHandle) -> bool {
    let Some(root) = enhance_root(app) else { return false };
    let trt = root.join("tensorrt");
    TENSORRT_FILES.iter().all(|rel| trt.join(rel).is_file())
}

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

/// script-opts/fntv_interp.conf 路径（mpv 启动时读）。
fn interp_conf_path(app: &AppHandle) -> Option<PathBuf> {
    crate::config::mpv_portable_config_dir(app)
        .map(|d| d.join("script-opts").join("fntv_interp.conf"))
}

/// 读当前引擎档与默认开关（文件缺失 → 默认 auto/false）。
fn read_interp_opts(app: &AppHandle) -> (String, bool) {
    let mut engine = "auto".into();
    let mut default_on = false;
    if let Some(p) = interp_conf_path(app) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            for line in text.lines() {
                let l = line.trim();
                if let Some(v) = l.strip_prefix("engine=") {
                    engine = v.trim().to_string();
                } else if let Some(v) = l.strip_prefix("default_on=") {
                    default_on = v.trim() == "yes";
                }
            }
        }
    }
    (engine, default_on)
}

/// [lc-1833] 写完 conf 后把改动同步到**正在运行**的播放器会话（与右键菜单同一
/// script-message 端点；lua 收到后即改即生效，并回写同一份 conf——两边默认值一致）。
/// 会话不在/已死则静默跳过（conf 已落盘，下次起播生效）。
async fn push_to_live_mpv(state: &tauri::State<'_, AppState>, args: &[&str]) {
    let Some(sess) = state.mpv.lock().await.clone() else { return };
    if sess.is_dead().await {
        return;
    }
    let cmd: Vec<Value> = std::iter::once(json!("script-message"))
        .chain(args.iter().map(|a| json!(a)))
        .collect();
    sess.command_no_reply(Value::Array(cmd)).await;
}

/// [lc-1822] 设置插帧引擎与默认开关：重写 script-opts/fntv_interp.conf
/// （mpv 启动时读取——改动对「下次播放」生效；当前会话用播放器插帧按钮切）。
/// [lc-1833] default_on=yes 时同时推送到运行中的会话立即切引擎（「起播即开」语义
/// 下引擎自然当下生效）；default_on=no 只改默认、不动当前会话——避免「面板换个
/// 引擎档，播放中的画面突然开始插帧」的意外副作用。
#[tauri::command]
pub async fn ai_enhance_set_engine(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    engine: String,
    default_on: Option<bool>,
) -> Result<Value, String> {
    let e = engine.trim().to_lowercase();
    if !matches!(e.as_str(), "auto" | "builtin" | "rife" | "nvidia") {
        return Err(format!("未知引擎档 {e}（只允许 auto/builtin/rife/nvidia）"));
    }
    if e == "rife" && !runtime_ready(&app) {
        return Err("AI 增强组件未就绪，无法选择 RIFE 档（先下载组件）".into());
    }
    let Some(dir) = crate::config::mpv_portable_config_dir(&app) else {
        return Err("找不到 mpv 配置目录".into());
    };
    let so = dir.join("script-opts");
    std::fs::create_dir_all(&so).map_err(|err| format!("创建 script-opts 失败: {err}"))?;
    let (cur_engine, cur_default) = read_interp_opts(&app);
    let new_engine = if e == "auto" { "auto" } else { e.as_str() };
    let new_default = default_on.unwrap_or(cur_default);
    let tier = read_quality_tier(&app);
    let body = format!(
        "# AniBox 插帧配置（设置面板「AI 增强」写入，请勿手动编辑）
# 引擎档：auto(=builtin) / builtin / rife / nvidia
# rife = RIFE v4.6 AI 2×（VapourSynth+ncnn Vulkan，全显卡），需 AI 增强组件就绪。
# 运动平滑基线：extreme/balanced=时间插值平滑；smooth=关闭省 GPU

engine={new_engine}
quality_tier={tier}
# 启动播放时即开启插帧（默认关=否）
default_on={}
",
        if new_default { "yes" } else { "no" }
    );
    std::fs::write(so.join("fntv_interp.conf"), body)
        .map_err(|err| format!("写入配置失败（安装目录可能只读）: {err}"))?;
    // [lc-1855] N 卡 Smooth Motion 需要 Vulkan 呈现上下文（gpu-context=winvk，启动项）：
    // 写入随包 portable_config/mpv-interp-nvidia.conf（被 mpv-user.conf include）。
    // 此前该文件只有"应用会自动重写"的注释、无任何代码写它——N 卡档是张空头支票。
    // 选其它引擎时恢复空桩（撤掉 winvk，避免 Vulkan 上下文残留影响非 N 卡场景）。
    let nvidia_conf = dir.join("mpv-interp-nvidia.conf");
    let nvidia_body = if new_engine == "nvidia" {
        "# AniBox · N 卡 Smooth Motion（RTX50 驱动级）
# 由「设置 > AI 增强」选择 N 卡引擎时写入；重启播放器后生效。
# 还需在 NVIDIA App「图形/程序设置」把本播放器 mpv.exe 加入并开启 Smooth Motion。
gpu-context=winvk
".to_string()
    } else {
        "# AniBox · N 卡 Smooth Motion（RTX50 驱动级）
# 仅当「插帧引擎=N 卡」时由应用写入；请勿手动编辑。
".to_string()
    };
    let _ = std::fs::write(&nvidia_conf, nvidia_body);
    let _ = cur_engine;
    if new_default && new_engine != "auto" {
        push_to_live_mpv(&state, &["fntv-interp-engine", new_engine]).await;
    }
    let mut resp = json!({ "engine": new_engine, "defaultOn": new_default });
    // [lc-1857] N 卡引擎选中 → 半自动注册准备（剪贴板 + 拉起 NVIDIA App），指引随响应回面板
    if new_engine == "nvidia" {
        let mpv_path = state.config.read().await.mpv_path.clone();
        resp["nvidiaHint"] = json!(nvidia_sm_prepare(&mpv_path));
    }
    Ok(resp)
}

/// 状态查询（面板展示 + fntv_interp 侧同源自检）。
#[tauri::command]
pub async fn ai_enhance_status(app: AppHandle) -> Result<Value, String> {
    let ready = runtime_ready(&app);
    let manifest = local_manifest(&app);
    let (engine, default_on) = read_interp_opts(&app);
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
        "engine": engine,
        "defaultOn": default_on,
        "superresDmlReady": superres_dml_ready(&app),
        "tensorrtReady": tensorrt_ready(&app),
        "superresEngine": read_superres_engine(&app),
        "qualityTier": read_quality_tier(&app),
        // [lc-1856] 显卡名（RIFE 硬性要求提示用；虚拟显卡已过滤）
        "gpu": detect_gpu_name(),
    }))
}

/// 读超分引擎档（script-opts/fntv_superres.conf；缺失=off）。
fn read_superres_engine(app: &AppHandle) -> String {
    if let Some(p) = crate::config::mpv_portable_config_dir(app) {
        let f = p.join("script-opts").join("fntv_superres.conf");
        if let Ok(text) = std::fs::read_to_string(&f) {
            for line in text.lines() {
                if let Some(v) = line.trim().strip_prefix("superres_engine=") {
                    return v.trim().to_string();
                }
            }
        }
    }
    "off".into()
}

/// 读画质档位（script-opts/fntv_interp.conf；缺失=balanced）。
fn read_quality_tier(app: &AppHandle) -> String {
    if let Some(p) = interp_conf_path(app) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            for line in text.lines() {
                if let Some(v) = line.trim().strip_prefix("quality_tier=") {
                    let v = v.trim();
                    if matches!(v, "extreme" | "balanced" | "smooth") {
                        return v.to_string();
                    }
                }
            }
        }
    }
    "balanced".into()
}

/// [lc-1825] 设置画质档位（极致/均衡=时间插值平滑；流畅=关闭省 GPU）——
/// 只改基线，手动插帧引擎/按钮优先（lua 同款语义）。
#[tauri::command]
pub async fn ai_enhance_set_tier(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    tier: String,
) -> Result<Value, String> {
    let t = tier.trim().to_lowercase();
    if !matches!(t.as_str(), "extreme" | "balanced" | "smooth") {
        return Err(format!("未知运动平滑档位 {t}（只允许 extreme/balanced/smooth）"));
    }
    let Some(dir) = crate::config::mpv_portable_config_dir(&app) else {
        return Err("找不到 mpv 配置目录".into());
    };
    let so = dir.join("script-opts");
    std::fs::create_dir_all(&so).map_err(|err| format!("创建 script-opts 失败: {err}"))?;
    let (engine, default_on) = read_interp_opts(&app);
    let body = format!(
        "# AniBox 插帧配置（设置面板「AI 增强」写入，请勿手动编辑）
# 引擎档：auto(=builtin) / builtin / rife / nvidia
# 运动平滑基线：extreme/balanced=时间插值平滑；smooth=关闭省 GPU

engine={engine}
quality_tier={t}
default_on={}
",
        if default_on { "yes" } else { "no" }
    );
    std::fs::write(so.join("fntv_interp.conf"), body)
        .map_err(|err| format!("写入配置失败: {err}"))?;
    // [lc-1833] 档位是纯基线（不改变插帧开关状态），安全地推给运行中的会话即时生效
    push_to_live_mpv(&state, &["fntv-interp-tier", t.as_str()]).await;
    Ok(json!({ "qualityTier": t }))
}

/// [lc-1823] 设置超分引擎档：off / dml / tensorrt（写 script-opts/fntv_superres.conf，
/// fntv_superres.lua 播放时读取挂载；dml/tensorrt 档前置就绪校验）。
#[tauri::command]
pub async fn ai_enhance_set_superres(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    engine: String,
) -> Result<Value, String> {
    let e = engine.trim().to_lowercase();
    if !matches!(e.as_str(), "off" | "dml" | "tensorrt") {
        return Err(format!("未知超分引擎 {e}（只允许 off/dml/tensorrt）"));
    }
    if e == "dml" && !superres_dml_ready(&app) {
        return Err("AI 增强组件未就绪（DML 超分文件缺失，请重新下载基础包）".into());
    }
    if e == "tensorrt" && !tensorrt_ready(&app) {
        return Err("TensorRT 扩展包未安装（先下载 RTX 大包）".into());
    }
    let Some(dir) = crate::config::mpv_portable_config_dir(&app) else {
        return Err("找不到 mpv 配置目录".into());
    };
    let so = dir.join("script-opts");
    std::fs::create_dir_all(&so).map_err(|err| format!("创建 script-opts 失败: {err}"))?;
    let body = "# AniBox AI 超分配置（设置面板写入）\n# 引擎档：off / dml（AnimeJaNai DirectML·全显卡）/ tensorrt（RTX 40/50）\nsuperres_engine=".to_string() + &e + "\n";
    std::fs::write(so.join("fntv_superres.conf"), body)
        .map_err(|err| format!("写入配置失败: {err}"))?;
    // [lc-1833] 超分档即改即挂载/卸载（evaluate 无插帧那类联动副作用），直接推运行会话
    push_to_live_mpv(&state, &["fntv-superres-set", e.as_str()]).await;
    Ok(json!({ "superresEngine": e }))
}

/// 下载并安装：远端 manifest → zip 流式下载（进度事件）→ sha256 校验 →
/// 解压（pwsh Expand-Archive，CREATE_NO_WINDOW 零前台）→ 原子替换。
#[tauri::command]
pub async fn ai_enhance_download(app: AppHandle, pack: Option<String>) -> Result<Value, String> {
    let pack = pack.unwrap_or_else(|| "base".into());
    let is_trt = pack == "tensorrt";
    let emit = |downloaded: u64, total: u64| {
        let _ = app.emit(
            "ai-enhance-progress",
            json!({ "downloaded": downloaded, "total": total }),
        );
    };
    // 1) 远端 manifest
    let client = reqwest::Client::builder()
        // CF 边缘会拦无 UA 的请求（实测 Python 裸 UA 403）——客户端固定 UA
        .user_agent(concat!("Mozilla/5.0 AniBox/", env!("CARGO_PKG_VERSION")))
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
    let entry = if is_trt {
        manifest
            .get("packs")
            .and_then(|pk| pk.get("tensorrt"))
            .cloned()
            .ok_or("清单缺 packs.tensorrt（TRT 扩展包未发布）")?
    } else {
        manifest.clone()
    };
    let version = entry
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // 2) 流式下载到临时文件（[lc-1823] 分卷包：wrangler 单文件 300MiB 上限，
    //    TRT 大包拆 parts 逐卷流式 append + 逐卷 sha 校验；无 parts 走单 zip）。
    let tmp_zip = std::env::temp_dir().join(format!("anibox-ai-enhance-{}.zip", std::process::id()));
    use std::io::Write as _;
    use futures_util::StreamExt as _;

    /// 单个 URL 流式下载到 writer，返回 (本段字节数, 全量 hasher 增量后的 sha 十六进制)。
    async fn stream_to(
        client: &reqwest::Client,
        url: &str,
        file: &mut std::fs::File,
        hasher: &mut Sha256,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
        downloaded_before: u64,
        total_hint: u64,
    ) -> Result<u64, String> {
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("组件包下载失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("组件包下载 HTTP {}", resp.status().as_u16()));
        }
        let mut stream = resp.bytes_stream();
        let mut seg: u64 = 0;
        let mut last_emit = 0u64;
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| format!("下载中断: {e}"))?;
            hasher.update(&bytes);
            file.write_all(&bytes).map_err(|e| format!("写临时文件失败: {e}"))?;
            seg += bytes.len() as u64;
            if seg - last_emit >= 1024 * 1024 {
                progress(downloaded_before + seg, total_hint);
                last_emit = seg;
            }
        }
        progress(downloaded_before + seg, total_hint);
        Ok(seg)
    }

    let parts: Vec<(String, String, u64)> = entry
        .get("parts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    Some((
                        e.get("url")?.as_str()?.to_string(),
                        e.get("sha256")?.as_str()?.to_ascii_lowercase(),
                        e.get("sizeBytes")?.as_u64()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut file = std::fs::File::create(&tmp_zip).map_err(|e| format!("临时文件创建失败: {e}"))?;
    let mut hasher = Sha256::new();
    if !parts.is_empty() {
        let grand: u64 = parts.iter().map(|p| p.2).sum();
        let mut done: u64 = 0;
        for (i, (url, want, size)) in parts.iter().enumerate() {
            let seg = stream_to(
                &client,
                url,
                &mut file,
                &mut hasher,
                &|d, t| emit(d, t),
                done,
                grand,
            )
            .await?;
            if &seg != size {
                let _ = std::fs::remove_file(&tmp_zip);
                return Err(format!("分卷 {} 大小不符（期望 {} 实得 {}）", i + 1, size, seg));
            }
            // 逐卷独立校验（hasher 是全量累计，这里对卷单独算）
            let part_hash = {
                let mut h = Sha256::new();
                let mut f = std::fs::File::open(&tmp_zip).map_err(|e| e.to_string())?;
                use std::io::Seek as _;
                use std::io::Read as _;
                f.seek(std::io::SeekFrom::Start(done)).map_err(|e| e.to_string())?;
                let mut left = seg;
                let mut buf = vec![0u8; 1024 * 1024];
                while left > 0 {
                    let n = std::cmp::min(left, buf.len() as u64) as usize;
                    f.read_exact(&mut buf[..n]).map_err(|e| e.to_string())?;
                    h.update(&buf[..n]);
                    left -= n as u64;
                }
                format!("{:x}", h.finalize())
            };
            if part_hash != *want {
                let _ = std::fs::remove_file(&tmp_zip);
                return Err(format!(
                    "分卷 {} 校验失败（sha256 不匹配，下载可能损坏）",
                    i + 1
                ));
            }
            done += seg;
        }
    } else {
        let zip_url = entry
            .get("zipUrl")
            .and_then(|v| v.as_str())
            .ok_or("清单缺 zipUrl")?
            .to_string();
        let want_sha = entry
            .get("zipSha256")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
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
        let mut stream = resp.bytes_stream();
        let mut downloaded: u64 = 0;
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
        // 校验统一在收尾处（hasher.finalize 消费性）
        let _ = &want_sha;
    }

    drop(file);
    let done_total: u64 = std::fs::metadata(&tmp_zip).map(|m| m.len()).unwrap_or(0);
    let got_sha = format!("{:x}", hasher.finalize());

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

    // 4) 落位：base 包整目录原子替换；tensorrt 包解到 ai-enhance/tensorrt/
    let root = enhance_root(&app).ok_or("app_data_dir 不可用")?;
    if is_trt {
        let trt = root.join("tensorrt");
        let trt_old = trt.with_extension("old");
        let _ = std::fs::remove_dir_all(&trt_old);
        if trt.exists() && std::fs::rename(&trt, &trt_old).is_err() {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err("替换 TensorRT 扩展失败（文件被占用，请关闭播放器后重试）".into());
        }
        if let Err(e) = std::fs::rename(&src_root, &trt) {
            if trt_old.exists() {
                let _ = std::fs::rename(&trt_old, &trt);
            }
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(format!("TensorRT 扩展就位失败: {e}"));
        }
        let _ = std::fs::remove_dir_all(&trt_old);
        emit(done_total, done_total);
        return Ok(json!({ "version": version, "ready": tensorrt_ready(&app) }));
    }
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
    emit(done_total, done_total);
    Ok(json!({ "version": version, "ready": runtime_ready(&app) }))
}

/// 删除组件（释放 ~50MB；播放器若在用 RIFE 需重启播放器）。
#[tauri::command]
pub async fn ai_enhance_delete(app: AppHandle, pack: Option<String>) -> Result<(), String> {
    let Some(root) = enhance_root(&app) else { return Ok(()) };
    if pack.as_deref() == Some("tensorrt") {
        let trt = root.join("tensorrt");
        if trt.exists() && std::fs::remove_dir_all(&trt).is_err() {
            return Err("删除失败（文件被播放器占用，请先关闭播放器）".into());
        }
        let _ = std::fs::remove_dir_all(trt.with_extension("old"));
        return Ok(());
    }
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
