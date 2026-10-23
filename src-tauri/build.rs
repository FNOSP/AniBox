//! 构建脚本：注入飞牛 fnOS 影视客户端的签名材料（公开客户端凭据，**不入库**）。
//!
//! 来源文件：`<repo>/secrets/fn-authx.local`（被 `.gitignore` 忽略；示例见同目录
//! `fn-authx.local.example`）。解析后以 `cargo:rustc-env` 注入编译期环境变量
//! `ANIBOX_FN_AUTHX_KEY` / `ANIBOX_FN_AUTHX_SECRET`，运行时由
//! `authx.rs` / `danmaku_shim.rs` 通过 `option_env!` 读取。
//!
//! 文件缺失或字段为空时**不中断构建**，只打印 `cargo:warning`：相关功能（NAS 字幕
//! 搜索/下载等需要飞牛网关签名的接口）降级为「不带 Authx 头」，请求照常发出。

use std::path::{Path, PathBuf};

/// 允许在 local 文件里出现的键名 → 目标编译期环境变量名。
const KEYS: &[(&str, &str)] = &[
    ("KEY", "ANIBOX_FN_AUTHX_KEY"),
    ("ANIBOX_FN_AUTHX_KEY", "ANIBOX_FN_AUTHX_KEY"),
    ("SECRET", "ANIBOX_FN_AUTHX_SECRET"),
    ("ANIBOX_FN_AUTHX_SECRET", "ANIBOX_FN_AUTHX_SECRET"),
];

fn main() {
    emit_authx_materials();
    emit_common_controls_dependency();
    tauri_build::build()
}

/// [lc-1749] 给链接器补 Common-Controls v6 清单依赖（Windows/MSVC）。
///
/// 背景：正式 `anibox.exe` 的清单由 tauri_build 经 winres 嵌入（含
/// Microsoft.Windows.Common-Controls v6，tray-icon/muda 引入的 `TaskDialogIndirect`
/// 等导入需要它）。但 **lib 单测可执行文件没有清单** —— 一旦（因代码引用或链接器
/// GC 的边际变化）测试二进制引用到那些导入，加载器解析到 comctl32 v5.82，找不到
/// `TaskDialogIndirect`，进程直接以 `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` 退出，
/// `cargo test --lib` 整体跑不起来，且换个写法又"自愈"，极难定位。
/// 链接器会把该依赖并入清单（对正式 exe 是与 winres 嵌入清单的同名去重合并，不改变行为）。
fn emit_common_controls_dependency() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }
    println!(
        "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' \
         name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
         publicKeyToken='6595b64144ccf1df' language='*' processorArchitecture='*'"
    );
}

fn emit_authx_materials() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    // src-tauri/ → 仓库根
    let repo_root = manifest.parent().map(Path::to_path_buf).unwrap_or(manifest);
    let secrets_dir = repo_root.join("secrets");
    let file = secrets_dir.join("fn-authx.local");

    // 只在路径存在时声明 rerun-if-changed（不存在的路径会导致每次构建都重跑脚本）。
    if secrets_dir.is_dir() {
        println!("cargo:rerun-if-changed={}", secrets_dir.display());
    }
    if file.is_file() {
        println!("cargo:rerun-if-changed={}", file.display());
    }

    let parsed = std::fs::read_to_string(&file).ok().map(|body| parse(&body));

    let mut emitted = 0usize;
    for env_name in ["ANIBOX_FN_AUTHX_KEY", "ANIBOX_FN_AUTHX_SECRET"] {
        let value = parsed
            .as_ref()
            .and_then(|m| m.iter().find(|(k, _)| k == env_name).map(|(_, v)| v.clone()))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            // 文件缺失时的兜底：构建进程环境变量（便于 `set ANIBOX_FN_AUTHX_KEY=... && 打包.cmd`）
            .or_else(|| {
                std::env::var(env_name)
                    .ok()
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            });
        match value {
            Some(v) => {
                println!("cargo:rustc-env={env_name}={v}");
                emitted += 1;
            }
            None => println!(
                "cargo:warning=未注入 {env_name}（secrets/fn-authx.local 缺失或缺该字段）：\
                 飞牛签名相关功能（NAS 字幕搜索/下载）将降级为未签名请求"
            ),
        }
    }

    if emitted == 0 {
        println!(
            "cargo:warning=未注入飞牛签名材料（secrets/fn-authx.local 不存在或为空），\
             相关功能将不可用；配置方法见 docs/SECURITY-notes.md"
        );
    }
}

/// 解析 `KEY=value` 行；忽略空行与 `#` 注释；值两侧引号会被去掉。
/// 返回 (目标环境变量名, 值) 列表。
fn parse(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        if let Some((_, target)) = KEYS.iter().find(|(name, _)| name.eq_ignore_ascii_case(k)) {
            let v = v.trim().trim_matches('"').trim_matches('\'').trim();
            out.push((target.to_string(), v.to_string()));
        }
    }
    out
}
