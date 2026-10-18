//! 系列级匹配记忆（对齐 Node `danmu-api.cjs` 的 `seriesMem`）。
//!
//! 落盘 `%TEMP%/anibox-danmu-mem/danmu-api-mem.json`，version=1，最多 300 条，
//! 按最后使用时间保留最新的。记忆命中时可直接复用条目定位分集，跳过标题搜索；
//! 记忆失效（条目被删/集号定位不到/弹幕为空）由调用方落回正常搜索。

use crate::danmaku::types::SearchHit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

const MEM_VERSION: u32 = 1;
const MEM_MAX: usize = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemEntry {
    anime_id: i64,
    anime_title: String,
    episode_count: i64,
    source: String,
    season_tier: u8,
    ts: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MemFile {
    version: u32,
    series: BTreeMap<String, MemEntry>,
}

fn mem_file() -> PathBuf {
    std::env::temp_dir()
        .join("anibox-danmu-mem")
        .join("danmu-api-mem.json")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn load() -> MemFile {
    let raw = match std::fs::read_to_string(mem_file()) {
        Ok(s) => s,
        Err(_) => return MemFile::default(),
    };
    match serde_json::from_str::<MemFile>(&raw) {
        Ok(f) if f.version == MEM_VERSION => f,
        _ => MemFile::default(),
    }
}

fn save(file: &MemFile) {
    let mut entries: Vec<(&String, &MemEntry)> = file.series.iter().collect();
    entries.sort_by(|a, b| b.1.ts.cmp(&a.1.ts));
    entries.truncate(MEM_MAX);
    let mut trimmed = BTreeMap::new();
    for (k, v) in entries {
        trimmed.insert(k.clone(), v.clone());
    }
    let out = MemFile {
        version: MEM_VERSION,
        series: trimmed,
    };
    let path = mem_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(body) = serde_json::to_string_pretty(&out) {
        let _ = std::fs::write(path, body);
    }
}

/// 进程内串行化记忆文件读写（避免并发请求互相覆盖）。
fn mem_lock() -> &'static Mutex<()> {
    static L: Mutex<()> = Mutex::new(());
    &L
}

/// 读取某个系列键的记忆条目。
pub fn recall(key: &str) -> Option<SearchHit> {
    if key.is_empty() {
        return None;
    }
    let _g = mem_lock().lock().unwrap_or_else(|e| e.into_inner());
    let file = load();
    file.series.get(key).map(|m| SearchHit {
        anime_id: m.anime_id,
        anime_title: m.anime_title.clone(),
        episode_count: m.episode_count,
        source: m.source.clone(),
        season_tier: m.season_tier,
    })
}

/// 记住某个系列命中的条目。
pub fn remember(key: &str, hit: &SearchHit) {
    if key.is_empty() || hit.anime_id == 0 {
        return;
    }
    let _g = mem_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut file = load();
    file.series.insert(
        key.to_string(),
        MemEntry {
            anime_id: hit.anime_id,
            anime_title: hit.anime_title.clone(),
            episode_count: hit.episode_count,
            source: hit.source.clone(),
            season_tier: hit.season_tier,
            ts: now_ms(),
        },
    );
    save(&file);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_then_recall() {
        let key = format!("test-series-{}", std::process::id());
        let hit = SearchHit {
            anime_id: 42,
            anime_title: "测试番".into(),
            episode_count: 12,
            source: "bilibili".into(),
            season_tier: 0,
        };
        remember(&key, &hit);
        let got = recall(&key).expect("应能读到刚写入的记忆");
        assert_eq!(got.anime_id, 42);
        assert_eq!(got.anime_title, "测试番");
        // 清理
        let _g = mem_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut file = load();
        file.series.remove(&key);
        save(&file);
    }
}
