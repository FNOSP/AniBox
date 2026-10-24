//! [lc-1781] 字幕源外置管理：`app_data_dir/subtitle-sources.json`。
//!
//! 与弹幕源（lc-1780 danmaku-api-sources.json）同一套架构，补齐字幕侧：
//!   · assrt.net（token 可换；默认沿用 sub-assrt.lua 的共享 token）
//!   · opensubtitles.com（用户自己的 Api-Key；v1 REST：/api/v1/subtitles）
//!   · 迅雷（sub.xmp.sandai.net:8000/subxlx.json，**按文件哈希精确匹配**，
//!     无需账号；仅在播放本地文件时可用——URL 流没法算哈希）
//! 消费方是 mpv 侧 `sub-assrt.lua`（多源扇出搜索/下载，uosc 菜单呈现）；
//! 本模块只管列表 CRUD + 哈希助手（`--subtitle-hash` CLI 子命令复用本模块函数）。
//!
//! 哈希算法（pub 供 main.rs CLI 与未来 UI 复用）：
//!   · gcid（迅雷 xhash）：文件按 n=max(size/0x8000,1) 等长分段（段长 size/n 向下取整，
//!     尾部余量不计），各段 SHA1 摘要顺次拼接后再整体 SHA1，40 hex。
//!   · opensub hash：MD5(前 64KB + 后 64KB + size 小端 8 字节)，32 hex（官方 moviehash 算法）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest as Sha1Digest, Sha1};
use tauri::{AppHandle, Manager};

const MAX_SOURCES: usize = 12;
/// 内置共享 assrt token（与 sub-assrt.lua 的默认一致；用户可在设置里换成自己的）。
const DEFAULT_ASSRT_TOKEN: &str = "tNjXZUnOJWcHznHDyalNMYqqP6IdDdpQ";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSource {
    /// assrt | opensubtitles | xunlei
    pub kind: String,
    pub name: String,
    /// assrt 的 token / opensubtitles 的 Api-Key；xunlei 不需要（空串）
    #[serde(default)]
    pub token: String,
    pub enabled: bool,
}

fn file_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|d| d.join("subtitle-sources.json"))
        .map_err(|e| format!("拿不到 app_data_dir: {e}"))
}

fn defaults() -> Vec<SubtitleSource> {
    vec![
        SubtitleSource {
            kind: "xunlei".into(),
            name: "迅雷（按文件哈希精确匹配）".into(),
            token: String::new(),
            enabled: true,
        },
        SubtitleSource {
            kind: "assrt".into(),
            name: "assrt.net".into(),
            token: DEFAULT_ASSRT_TOKEN.into(),
            enabled: true,
        },
    ]
}

fn valid_kind(k: &str) -> bool {
    k == "assrt" || k == "opensubtitles" || k == "xunlei"
}

/// 读列表（缺失/损坏 → 内置默认；坏项丢弃，面板一保存即修复）。
#[tauri::command]
pub async fn subtitle_sources_get(app: AppHandle) -> Result<Value, String> {
    let p = file_path(&app)?;
    let list: Vec<SubtitleSource> = match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str::<Value>(&s)
            .ok()
            .and_then(|v| v.get("sources").and_then(|x| x.as_array()).cloned())
            .map(|arr| {
                arr.into_iter()
                    .filter_map(|s| serde_json::from_value::<SubtitleSource>(s).ok())
                    .filter(|s| valid_kind(&s.kind))
                    .collect()
            })
            .filter(|l: &Vec<SubtitleSource>| !l.is_empty())
            .unwrap_or_else(defaults),
        Err(_) => defaults(),
    };
    Ok(json!({ "sources": list }))
}

/// 写列表（全量覆盖）：kind 白名单、xunlei 不需要 token、assrt/opensubtitles 必须有 token。
#[tauri::command]
pub async fn subtitle_sources_set(app: AppHandle, sources: Vec<SubtitleSource>) -> Result<(), String> {
    if sources.len() > MAX_SOURCES {
        return Err(format!("字幕源最多 {} 条", MAX_SOURCES));
    }
    for (i, s) in sources.iter().enumerate() {
        if !valid_kind(&s.kind) {
            return Err(format!("第 {} 个源的 kind 非法（assrt/opensubtitles/xunlei）", i + 1));
        }
        if s.kind != "xunlei" && s.token.trim().is_empty() {
            return Err(format!("「{}」需要填写 token / Api-Key", s.name));
        }
        if s.name.trim().is_empty() {
            return Err(format!("第 {} 个源缺少名称", i + 1));
        }
    }
    let p = file_path(&app)?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let body = serde_json::to_string_pretty(&json!({ "sources": sources }))
        .map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&p, body).map_err(|e| format!("写入失败: {e}"))
}

// ───────────────────────── 哈希 ─────────────────────────

/// 迅雷 gcid：分段 SHA1。读取大文件用 8MB 缓冲逐段流式处理，不整读内存。
pub fn gcid_hash(path: &std::path::Path) -> Result<String, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(|e| format!("打开文件失败: {e}"))?;
    let size = f.metadata().map_err(|e| format!("读文件大小失败: {e}"))?.len();
    if size == 0 {
        return Err("空文件无法计算哈希".into());
    }
    let n = std::cmp::max(size / 0x8000, 1);
    let seg = size / n; // 向下取整；尾部余量不计（与测试向量同口径）
    let mut acc: Vec<u8> = Vec::with_capacity((n as usize) * 20);
    let mut buf = vec![0u8; seg as usize];
    for _ in 0..n {
        f.read_exact(&mut buf).map_err(|e| format!("读段失败: {e}"))?;
        acc.extend_from_slice(&Sha1::digest(&buf));
    }
    let _ = f.seek(SeekFrom::Start(0));
    Ok(hex(&Sha1::digest(&acc)))
}

/// OpenSubtitles moviehash：MD5(前 64KB + 后 64KB + size LE u64)。
pub fn opensub_hash(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("打开文件失败: {e}"))?;
    let size = f.metadata().map_err(|e| format!("读文件大小失败: {e}"))?.len();
    const CHUNK: usize = 64 * 1024;
    let mut head = vec![0u8; CHUNK.min(size as usize)];
    f.read_exact(&mut head).map_err(|e| format!("读头部失败: {e}"))?;
    // 官方算法：head 与 tail 窗口都取 min(64KB, size)——小文件等于整文件读两遍
    let tail_len = CHUNK.min(size as usize);
    let mut tail = vec![0u8; tail_len];
    if tail_len > 0 {
        use std::io::{Seek, SeekFrom};
        f.seek(SeekFrom::Start(size - tail_len as u64))
            .map_err(|e| format!("seek 失败: {e}"))?;
        f.read_exact(&mut tail).map_err(|e| format!("读尾部失败: {e}"))?;
    }
    let mut all = Vec::with_capacity(head.len() + tail.len() + 8);
    all.extend_from_slice(&head);
    all.extend_from_slice(&tail);
    all.extend_from_slice(&(size as u64).to_le_bytes());
    Ok(hex(md5::compute(&all).as_ref()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}


#[cfg(test)]
mod tests {
    use super::*;

    /// 向量由独立 Python 实现生成（双向验证：算法抄错必然对不上）。
    #[test]
    fn hash_vectors_match_python() {
        let dir = std::env::temp_dir().join("anibox-subhash-test");
        std::fs::create_dir_all(&dir).unwrap();
        // d1: 131072 字节（0..255 循环 ×512）
        let d1: Vec<u8> = (0..131072u32).map(|i| (i % 256) as u8).collect();
        // d2: 300 字节全零
        let d2 = vec![0u8; 300];
        // d3: 'AB'×50000 + 'TAILMARKER'×1000
        let mut d3 = b"AB".iter().copied().cycle().take(100_000).collect::<Vec<u8>>();
        d3.extend_from_slice(b"TAILMARKER".iter().copied().cycle().take(10_000).collect::<Vec<u8>>().as_slice());
        for (name, data, gcid_exp, os_exp) in [
            ("d1", &d1, "deb94dafa574d8f941daca091a5b30f7ac04909c", "2dec773d80ba2d5ad3adc01f465bd7c9"),
            ("d2", &d2, "edbc60282ca9f4685fa81d3df457bcf4fa2d26d7", "4782ed638aab3d5035f59e987b8f52a2"),
            ("d3", &d3, "6000abbccf7ea1e2fec75cadb169ceb6a361de6c", "06bca296b05556e259f88de9e5f6b5ca"),
        ] {
            let p = dir.join(format!("{name}.bin"));
            std::fs::write(&p, data).unwrap();
            assert_eq!(gcid_hash(&p).unwrap(), gcid_exp, "{name} gcid");
            assert_eq!(opensub_hash(&p).unwrap(), os_exp, "{name} oshash");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kind_whitelist() {
        assert!(valid_kind("assrt"));
        assert!(valid_kind("opensubtitles"));
        assert!(valid_kind("xunlei"));
        assert!(!valid_kind("subhd"));
    }
}
