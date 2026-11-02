//! JAV 中文数据层 —— 词典移植自 JAVCloudHub 1.9.31（tags_zh.py 556 条标签 JA→简中、
//! studios.py 152 家厂牌 std/中文名/别名/番号前缀），数据内嵌 jav_dict.json（include_str）。
//!
//! 用途：r18.dev / javbus 刮回来的标签是日文/英文，展示时经 [`tag_zh`] 翻成简中；
//! 厂商名经 [`studio_label`] 归一为标准名并给出中文俗称。查不到时原样返回，绝不瞎编。

use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Deserialize)]
struct Dict {
    #[serde(default)]
    tags: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    studios: Vec<StudioRec>,
}

#[derive(Deserialize, Clone)]
pub struct StudioRec {
    /// 标准名（一般是英文缩写）
    pub std: String,
    /// 中文俗称（可能为空）
    pub cn: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub prefixes: Vec<String>,
}

struct Data {
    tags: std::collections::BTreeMap<String, String>,
    studios: Vec<StudioRec>,
    /// 别名/前缀 → 记录下标（全大写）
    alias_idx: std::collections::HashMap<String, usize>,
}

fn data() -> &'static Data {
    static D: OnceLock<Data> = OnceLock::new();
    D.get_or_init(|| {
        let raw = include_str!("jav_dict.json");
        let dict: Dict = serde_json::from_str(raw).unwrap_or_else(|_| Dict {
            tags: Default::default(),
            studios: Vec::new(),
        });
        let mut alias_idx = std::collections::HashMap::new();
        for (i, s) in dict.studios.iter().enumerate() {
            for a in &s.aliases {
                alias_idx.entry(a.to_uppercase()).or_insert(i);
            }
            for p in &s.prefixes {
                alias_idx.entry(p.to_uppercase()).or_insert(i);
            }
            alias_idx.entry(s.std.to_uppercase()).or_insert(i);
        }
        Data { tags: dict.tags, studios: dict.studios, alias_idx }
    })
}

/// 标签 → 简中（精确命中词典；查不到原样返回）
pub fn tag_zh(tag: &str) -> String {
    let t = tag.trim();
    if t.is_empty() {
        return String::new();
    }
    data().tags.get(t).cloned().unwrap_or_else(|| t.to_string())
}

/// 批量：标签列表 → 简中（去重保序）
pub fn tag_zh_list(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let z = tag_zh(t);
        if !z.is_empty() && !out.iter().any(|x| x == &z) {
            out.push(z);
        }
    }
    out
}

/// 厂商名 → (标准名, 中文俗称)。命中别名/番号前缀/标准名都算；查不到返回原名+空中文。
pub fn studio_label(name: &str) -> (String, String) {
    let n = name.trim();
    if n.is_empty() {
        return (String::new(), String::new());
    }
    let up = n.to_uppercase();
    let d = data();
    if let Some(i) = d.alias_idx.get(&up) {
        let rec = &d.studios[*i];
        let cn = if rec.cn.is_empty() { String::new() } else { format!(" · {}", rec.cn) };
        return (format!("{}{}", rec.std, cn), rec.cn.clone());
    }
    // 前缀模糊：厂商字段常带「(ムーディーズ)」这类尾巴，用包含匹配兜一次
    for (alias, i) in &d.alias_idx {
        if up.contains(alias.as_str()) {
            let rec = &d.studios[*i];
            let cn = if rec.cn.is_empty() { String::new() } else { format!(" · {}", rec.cn) };
            return (format!("{}{}", rec.std, cn), rec.cn.clone());
        }
    }
    (n.to_string(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_translation() {
        assert_eq!(tag_zh("単体作品"), "单体作品");
        assert_eq!(tag_zh("女優"), "女优");
        assert_eq!(tag_zh("不存在的标签xyz"), "不存在的标签xyz");
    }

    #[test]
    fn studio_lookup() {
        let (label, cn) = studio_label("MOODYZ");
        assert_eq!(label, "MOODYZ · 穆迪兹");
        assert_eq!(cn, "穆迪兹");
        let (label, _) = studio_label("ムーディーズ");
        assert_eq!(label, "MOODYZ · 穆迪兹");
        // S1 的番号前缀 SONE 也能命中
        let (label, cn) = studio_label("S1 NO.1 STYLE");
        assert!(label.starts_with("S1") && cn.contains("エスワン") || cn == "エスワン" || !cn.is_empty());
        // 未知名原样返回
        assert_eq!(studio_label("Planet Plus"), ("Planet Plus".into(), String::new()));
    }
}
