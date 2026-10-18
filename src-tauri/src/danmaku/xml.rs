//! 弹幕 XML：条目计数、按条合并去重、写盘 + out 路径白名单。
//!
//! 与 `danmaku-shim.cjs::isSafeDanmakuPath` / `bili-runner.cjs::writeMergedDanmakuXml`
//! 同口径：消费方只认 `<d p="...">text</d>` 条目，因此合并时按整行去重、统一重写为
//! `<danmaku>` 根。

use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[allow(dead_code)]
fn re_entry() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?s)<d\s[^>]*p="[^"]*"[^>]*>.*?</d>"#).unwrap())
}
fn re_d_tag() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<d\s").unwrap())
}

/// 统计弹幕条目数（对齐 Node `(body.match(/<d\s/g) || []).length`）。
pub fn count_entries(body: &str) -> i64 {
    re_d_tag().find_iter(body).count() as i64
}

/// 抽取全部 `<d ...>...</d>` 条目行（trim 后）。
#[allow(dead_code)]
pub fn extract_lines(body: &str) -> Vec<String> {
    re_entry()
        .find_iter(body)
        .map(|m| m.as_str().trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// 词法规范化路径（不做磁盘访问；`..` 会被消解），统一小写 + 反斜杠。
fn normalize_path(p: &str) -> String {
    let replaced = p.replace('/', "\\");
    let mut parts: Vec<&str> = Vec::new();
    for part in replaced.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("\\").to_lowercase()
}

fn allowed_roots() -> Vec<String> {
    let public = std::env::var("PUBLIC").unwrap_or_else(|_| r"C:\Users\Public".into());
    let programdata = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
    let temp = std::env::temp_dir().to_string_lossy().to_string();
    let mut out = Vec::new();
    for base in [public, programdata, temp] {
        out.push(normalize_path(&format!(
            "{}\\fnos-danmaku",
            base.trim_end_matches(['\\', '/'])
        )));
    }
    out
}

/// out 路径白名单：只允许 `%PUBLIC%/fnos-danmaku`、`%ProgramData%/fnos-danmaku`、
/// `%TEMP%/fnos-danmaku`（及其子目录）。
pub fn is_safe_path(p: &str) -> bool {
    if p.trim().is_empty() {
        return false;
    }
    let norm = normalize_path(p);
    allowed_roots()
        .iter()
        .any(|root| norm == *root || norm.starts_with(&format!("{root}\\")))
}

/// 确保父目录存在后写文本。
fn write_text(out: &str, body: &str) -> std::io::Result<()> {
    let path = PathBuf::from(out);
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() && !dir.exists() {
            std::fs::create_dir_all(dir)?;
        }
    }
    std::fs::write(path, body)
}

/// 原样写 B 站兼容 XML（自建源 `/api/v2/comment?...&format=xml` 的响应体）。
pub fn write_raw(out: &str, body: &str) -> std::io::Result<()> {
    write_text(out, body)
}

/// 按条目合并去重，统一 `<danmaku>` 根后写盘，返回去重后的条目数。
#[allow(dead_code)]
pub fn merge_and_write(out: &str, lines: &[String]) -> std::io::Result<i64> {
    let mut seen = std::collections::HashSet::new();
    let mut uniq: Vec<&str> = Vec::new();
    for l in lines {
        if seen.insert(l.as_str()) {
            uniq.push(l.as_str());
        }
    }
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<danmaku>\n{}\n</danmaku>\n",
        uniq.join("\n")
    );
    write_text(out, &body)?;
    Ok(uniq.len() as i64)
}

/// 从磁盘读 XML 并抽条目（不存在返回空）。
#[allow(dead_code)]
pub fn read_lines(out: &str) -> Vec<String> {
    std::fs::read_to_string(Path::new(out))
        .map(|b| extract_lines(&b))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_and_extract() {
        let body = r#"<?xml version="1.0"?><i><d p="1,2,25,16777215,0,0,0,1">你好</d><d p="3,2,25,16777215,0,0,0,2">再见</d></i>"#;
        assert_eq!(count_entries(body), 2);
        let lines = extract_lines(body);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("你好"));
    }

    #[test]
    fn merge_dedups_and_rewrites_root() {
        let dir = std::env::temp_dir().join("fnos-danmaku");
        let out = dir.join("test-merge.xml");
        let out_s = out.to_string_lossy().to_string();
        let a = r#"<d p="1,2,25,1,0,0,0,1">A</d>"#.to_string();
        let b = r#"<d p="2,2,25,1,0,0,0,2">B</d>"#.to_string();
        let n = merge_and_write(&out_s, &[a.clone(), b.clone(), a.clone()]).unwrap();
        assert_eq!(n, 2);
        let body = std::fs::read_to_string(&out).unwrap();
        assert!(body.starts_with("<?xml"));
        assert!(body.contains("<danmaku>"));
        assert_eq!(count_entries(&body), 2);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn whitelist_allows_temp_and_rejects_outside() {
        let ok = std::env::temp_dir()
            .join("fnos-danmaku")
            .join("x.xml")
            .to_string_lossy()
            .to_string();
        assert!(is_safe_path(&ok));
        assert!(!is_safe_path(r"C:\Windows\System32\evil.xml"));
        assert!(!is_safe_path(r"C:\Users\Public\fnos-danmaku-evil\x.xml"));
        // 越界穿越必须被拒
        let escape = std::env::temp_dir()
            .join("fnos-danmaku")
            .join("..")
            .join("..")
            .join("evil.xml")
            .to_string_lossy()
            .to_string();
        assert!(!is_safe_path(&escape));
        assert!(!is_safe_path(""));
    }
}
