//! [lc-2244] 进程内 zip 解压（AI 增强组件包用）。
//!
//! 背景：解压此前外调 `pwsh Expand-Archive`，而 pwsh 是 PowerShell **7**，
//! Windows 不自带（系统只带 5.1 的 `powershell.exe`）——干净机器上
//! `Command::new("pwsh")` 直接 ENOENT，报错文案就是用户反馈的
//! 「启动解压失败: program not found」。组件下载因此在这类机器上**必然失败**。
//!
//! 现在改成纯 Rust 解压：不依赖 pwsh / tar / 7z / Expand-Archive 任何外部程序，
//! 这是「用户点了下载就一定能装好」的前提。
//!
//! 安全约定：条目路径一律经 [`zip::read::ZipFile::enclosed_name`] 校验——
//! 它拒绝绝对路径与能逃出目标目录的 `..`，避免恶意/损坏 zip 覆盖用户文件。
//! 只解压 deflate / stored（见 Cargo.toml 的 feature 选择），遇到其它压缩方法
//! 明确报错而不是写出半截文件。

use std::io;
use std::path::{Path, PathBuf};

/// 解压 `zip_path` 到 `dest`（不存在则创建）。
///
/// 返回已解压的条目数（含目录条目之外的文件），便于调用方判断"空包"这类异常。
pub fn extract_to(zip_path: &Path, dest: &Path) -> Result<usize, String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开组件包失败: {e}"))?;
    let mut archive = zip::ZipArchive::new(io::BufReader::new(file))
        // 走到这里说明 zip 结构本身坏了（SHA-256 校验已在前一步挡掉绝大多数情况）
        .map_err(|e| format!("组件包损坏或不是有效的 zip: {e}"))?;

    std::fs::create_dir_all(dest).map_err(|e| format!("创建解压目录失败: {e}"))?;

    let mut files = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取组件包条目失败（第 {} 项）: {e}", i + 1))?;

        // 拒绝绝对路径与越界 `..`：合法包里的路径一定是相对路径。
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!("组件包含非法路径条目: {}", entry.name()));
        };
        // Windows 上 zip 里的反斜杠分隔路径也要归一（enclosed_name 只按 '/' 切分）。
        let rel = normalize_sep(&rel);
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out_path = dest.join(&rel);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("创建目录失败 {}: {e}", out_path.display()))?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录失败 {}: {e}", parent.display()))?;
        }
        let mut out = std::fs::File::create(&out_path)
            .map_err(|e| format!("写入文件失败 {}: {e}", out_path.display()))?;
        io::copy(&mut entry, &mut out)
            .map_err(|e| format!("解压文件失败 {}: {e}", out_path.display()))?;
        files += 1;
    }
    Ok(files)
}

/// 把路径里的 `\` 归一成平台分隔符，并压掉空段与 `.`。
///
/// Windows 上 `Path` 本来就认 `\`，但非 Windows 平台（CI 的 mac/linux）不认：
/// 一个写成 `runtime\plugins\x.dll` 的条目会在 mac 上变成单个文件名。
/// 因此统一按 `/` 与 `\` 双分隔切分后重组（不含 `\` 时也要走，`./a//b` 同样要归一）。
fn normalize_sep(rel: &Path) -> PathBuf {
    let s = rel.to_string_lossy();
    s.split(['/', '\\'])
        .filter(|seg| !seg.is_empty() && *seg != ".")
        .collect::<PathBuf>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// 造一个最小 zip（deflate）。
    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("anibox-zip-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn extracts_files_and_nested_dirs() {
        let d = tmp("plain");
        let zp = d.join("a.zip");
        make_zip(
            &zp,
            &[
                ("vs/rife.vpy", b"hello"),
                ("runtime/plugins/x.dll", b"bin"),
                ("top.txt", b"t"),
            ],
        );
        let out = d.join("out");
        let n = extract_to(&zp, &out).unwrap();
        assert_eq!(n, 3);
        assert_eq!(std::fs::read(out.join("vs/rife.vpy")).unwrap(), b"hello");
        assert_eq!(
            std::fs::read(out.join("runtime/plugins/x.dll")).unwrap(),
            b"bin"
        );
        assert_eq!(std::fs::read(out.join("top.txt")).unwrap(), b"t");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn normalizes_backslash_separators() {
        // 部分工具产出的 zip 用 `\` 作分隔符；不能把它当成单个文件名。
        assert_eq!(
            normalize_sep(Path::new("runtime\\plugins\\x.dll")),
            PathBuf::from("runtime").join("plugins").join("x.dll")
        );
        assert_eq!(
            normalize_sep(Path::new("vs/rife.vpy")),
            PathBuf::from("vs/rife.vpy")
        );
        assert_eq!(normalize_sep(Path::new("./a//b")), PathBuf::from("a/b"));
    }

    #[test]
    fn rejects_parent_dir_escape() {
        // 越界条目必须报错，不能写到目标目录外面。
        let d = tmp("escape");
        let zp = d.join("evil.zip");
        make_zip(&zp, &[("a/../../evil.txt", b"pwn")]);
        let out = d.join("out");
        let err = extract_to(&zp, &out).unwrap_err();
        assert!(err.contains("非法路径"), "got: {err}");
        assert!(!d.join("evil.txt").exists(), "不得写到目标目录之外");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_zip_yields_zero_files() {
        let d = tmp("empty");
        let zp = d.join("e.zip");
        make_zip(&zp, &[]);
        let out = d.join("out");
        assert_eq!(extract_to(&zp, &out).unwrap(), 0);
        assert!(out.is_dir());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_zip_reports_error() {
        let d = tmp("corrupt");
        let zp = d.join("bad.zip");
        std::fs::write(&zp, b"not a zip at all").unwrap();
        let err = extract_to(&zp, &d.join("out")).unwrap_err();
        assert!(err.contains("损坏") || err.contains("不是有效的 zip"), "got: {err}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
