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
use std::time::Duration;
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

    /// 实测（网络，#[ignore]）：应用内搜索扇出 —— 迅雷 oracle 必须返回真实条目；
    /// assrt 在无外网环境会被跳过（单源失败不拖垮整体）。验证时 -- --ignored。
    #[tokio::test]
    #[ignore]
    async fn subtitle_search_live() {
        let v = subtitle_search("葬送的芙莉莲".into()).await.expect("search 不应 Err");
        let items = v.get("items").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let xunlei = items.iter().filter(|i| i.get("kind").and_then(|k| k.as_str()) == Some("xunlei")).count();
        assert!(xunlei > 0, "迅雷源应有结果，实际: {v}");
        for i in &items {
            assert!(i.get("id").and_then(|x| x.as_str()).unwrap_or("") != "");
            assert!(i.get("title").and_then(|x| x.as_str()).unwrap_or("") != "");
        }
        println!("search OK: {} 条（迅雷 {}）", items.len(), xunlei);
    }

    #[test]
    fn kind_whitelist() {
        assert!(valid_kind("assrt"));
        assert!(valid_kind("opensubtitles"));
        assert!(valid_kind("xunlei"));
        assert!(!valid_kind("subhd"));
    }
}

// ───────────────────────── [lc-1783] 应用内字幕搜索/下载（复用外置源列表）─────────────────────────
// 面向 SubtitlePickerModal（详情页入口）：搜索走本模块扇出（与 mpv 侧 sub-assrt.lua 同源同参数），
// 下载落地 %TEMP%/anibox-subtitles（与 nas_subtitle/shim 手动菜单互享缓存），
// mpv 在播时自动 sub-add select 挂主字幕轨（弹幕在副字幕轨，不冲突）。

fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 AniBox/4")
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("subtitle http client")
    })
}

/// assrt token：用户在列表里配的优先，否则内置共享。
fn effective_assrt_token(list: &[SubtitleSource]) -> String {
    list.iter()
        .find(|s| s.kind == "assrt" && s.enabled && !s.token.trim().is_empty())
        .map(|s| s.token.trim().to_string())
        .unwrap_or_else(|| DEFAULT_ASSRT_TOKEN.to_string())
}

fn source_item(kind: &str, id: &str, title: &str, hint: &str) -> Value {
    json!({ "kind": kind, "id": id, "title": title, "hint": hint })
}

/// 读源列表（内部版，不经 AppHandle）。文件缺失 → 内置默认。
async fn read_list() -> Vec<SubtitleSource> {
    let p = if let Ok(ad) = std::env::var("APPDATA") {
        std::path::PathBuf::from(ad).join("com.anibox.app/subtitle-sources.json")
    } else {
        return defaults();
    };
    match std::fs::read_to_string(p) {
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
    }
}

/// 应用内字幕搜索：并行扇出 迅雷(名称) + assrt + OpenSubtitles(若配 key)。
/// 返回 {items:[{kind,id,title,hint}]}；单源失败不拖垮整体（跳过即可）。
#[tauri::command]
pub async fn subtitle_search(query: String) -> Result<Value, String> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Err("搜索词为空".into());
    }
    let list = read_list().await;
    let token = effective_assrt_token(&list);
    let osub_key = list
        .iter()
        .find(|s| s.kind == "opensubtitles" && s.enabled && !s.token.trim().is_empty())
        .map(|s| s.token.trim().to_string());
    let want_xunlei = list.iter().any(|s| s.kind == "xunlei" && s.enabled);
    let want_assrt = list.iter().any(|s| s.kind == "assrt" && s.enabled);

    let enc = urlencoding::encode(&q).to_string();
    let xunlei_fut = async {
        if !want_xunlei {
            return Vec::new();
        }
        let url = format!("https://api-shoulei-ssl.xunlei.com/oracle/subtitle?gcid=&cid=&name={enc}");
        let Ok(v) = http_client().get(&url).send().await else { return Vec::new() };
        let Ok(v) = v.json::<Value>().await else { return Vec::new() };
        let mut out = Vec::new();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for s in arr.iter().take(10) {
                let url = s.get("url").and_then(|x| x.as_str()).unwrap_or("");
                if url.is_empty() {
                    continue;
                }
                let name = s.get("name").and_then(|x| x.as_str()).unwrap_or("字幕");
                let simple = s.get("simple_name").and_then(|x| x.as_str()).unwrap_or("");
                let ext = s.get("ext").and_then(|x| x.as_str()).unwrap_or("");
                // 应用内只收裸字幕文件（压缩包此路不处理；mpv 菜单那条路也一样）
                if !matches!(ext, "ass" | "srt" | "ssa" | "vtt") {
                    continue;
                }
                let hint = format!("迅雷·{}", if simple.is_empty() { ext } else { simple });
                out.push(source_item("xunlei", url, name, &hint));
            }
        }
        out
    };
    let assrt_fut = async {
        if !want_assrt {
            return Vec::new();
        }
        let url = format!("https://api.assrt.net/v1/sub/search?token={token}&q={enc}&no_muxer=1&pos=0");
        let Ok(v) = http_client().get(&url).send().await else { return Vec::new() };
        let Ok(v) = v.json::<Value>().await else { return Vec::new() };
        let mut out = Vec::new();
        if v.get("status").and_then(|s| s.as_i64()) == Some(0) {
            if let Some(subs) = v.pointer("/sub/subs").and_then(|s| s.as_array()) {
                for s in subs.iter().take(10) {
                    let id = s
                        .get("id")
                        .or_else(|| s.get("fileid"))
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0);
                    if id == 0 {
                        continue;
                    }
                    let name = s
                        .get("video_chinese_name")
                        .and_then(|x| x.as_str())
                        .filter(|x| !x.is_empty())
                        .or_else(|| s.get("native_name").and_then(|x| x.as_str()))
                        .or_else(|| s.get("videoname").and_then(|x| x.as_str()))
                        .unwrap_or("字幕");
                    let lang = s.pointer("/lang/desc").and_then(|x| x.as_str()).unwrap_or("");
                    let hint = format!("assrt·{}", lang);
                    out.push(source_item("assrt", &id.to_string(), name, &hint));
                }
            }
        }
        out
    };
    let osub_fut = async {
        let Some(key) = osub_key else { return Vec::new() };
        let url = format!("https://api.opensubtitles.com/api/v1/subtitles?query={enc}&languages=zh-cn,zh,en");
        let Ok(resp) = http_client()
            .get(&url)
            .header("Api-Key", &key)
            .send()
            .await
        else {
            return Vec::new();
        };
        let Ok(v) = resp.json::<Value>().await else { return Vec::new() };
        let mut out = Vec::new();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for e in arr.iter().take(10) {
                let Some(file) = e.pointer("/attributes/files/0") else { continue };
                let fid = file.get("file_id").and_then(|x| x.as_i64()).unwrap_or(0);
                if fid == 0 {
                    continue;
                }
                let name = file
                    .get("file_name")
                    .and_then(|x| x.as_str())
                    .or_else(|| e.pointer("/attributes/release").and_then(|x| x.as_str()))
                    .unwrap_or("字幕");
                let lang = e.pointer("/attributes/language").and_then(|x| x.as_str()).unwrap_or("");
                let hint = format!("OpenSubs·{}", lang);
                out.push(source_item("osub", &fid.to_string(), name, &hint));
            }
        }
        out
    };

    let (a, b, c) = tokio::join!(xunlei_fut, assrt_fut, osub_fut);
    let mut items = a;
    items.extend(b);
    items.extend(c);
    Ok(json!({ "query": q, "items": items }))
}

/// 下载字幕并（mpv 在播时）挂载。kind ∈ xunlei(url 直链)/assrt(sub id 两段式)/osub(file_id 两段式)。
/// 返回 {path, mounted, title}。
#[tauri::command]
pub async fn subtitle_download(
    state: tauri::State<'_, crate::AppState>,
    kind: String,
    id: String,
    filename: Option<String>,
) -> Result<Value, String> {
    let dir = std::env::temp_dir().join("anibox-subtitles");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建缓存目录失败: {e}"))?;

    let (url, fallback_name) = match kind.as_str() {
        "xunlei" => (id.clone(), filename.clone().unwrap_or_else(|| "字幕.srt".into())),
        "assrt" => {
            let list = read_list().await;
            let token = effective_assrt_token(&list);
            let durl = format!("https://api.assrt.net/v1/sub/detail?token={token}&id={id}");
            let v: Value = http_client()
                .get(&durl)
                .send()
                .await
                .map_err(|e| format!("assrt 详情请求失败: {e}"))?
                .json()
                .await
                .map_err(|e| format!("assrt 详情解析失败: {e}"))?;
            if v.get("status").and_then(|s| s.as_i64()) != Some(0) {
                let msg = v.get("errmsg").and_then(|x| x.as_str()).unwrap_or("unknown");
                return Err(format!("assrt 详情失败: {msg}"));
            }
            let files = v
                .pointer("/sub/subs/0/filelist")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            let pick = files
                .iter()
                .find(|f| {
                    let name = f.get("f").and_then(|x| x.as_str()).unwrap_or("");
                    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
                    matches!(ext.as_str(), "ass" | "srt" | "ssa" | "vtt")
                })
                .or_else(|| files.first())
                .ok_or("assrt 详情无字幕文件")?;
            let url = pick.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if url.is_empty() {
                return Err("assrt 详情无下载地址".into());
            }
            let name = pick.get("f").and_then(|x| x.as_str()).unwrap_or("字幕.srt").to_string();
            (url, name)
        }
        "osub" => {
            let list = read_list().await;
            let key = list
                .iter()
                .find(|s| s.kind == "opensubtitles" && s.enabled && !s.token.trim().is_empty())
                .map(|s| s.token.trim().to_string());
            let Some(key) = key else {
                return Err("未配置 OpenSubtitles Api-Key".into());
            };
            let fid = id.parse::<i64>().map_err(|_| "file_id 非法")?;
            let v: Value = http_client()
                .post("https://api.opensubtitles.com/api/v1/download")
                .header("Api-Key", &key)
                .json(&json!({ "file_id": fid }))
                .send()
                .await
                .map_err(|e| format!("OpenSubtitles 请求失败: {e}"))?
                .json()
                .await
                .map_err(|e| format!("OpenSubtitles 解析失败: {e}"))?;
            let link = v.get("link").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if link.is_empty() {
                return Err("OpenSubtitles 未返回下载链接".into());
            }
            let name = v.get("file_name").and_then(|x| x.as_str()).unwrap_or("subtitle.srt").to_string();
            (link, name)
        }
        _ => return Err(format!("未知源类型 {kind}")),
    };

    // 文件名安全化（去 Windows 非法字符）+ 落地
    let safe: String = fallback_name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let path = dir.join(format!("web_{}", safe));
    let bytes = http_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("字幕下载失败: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("字幕读取失败: {e}"))?;
    std::fs::write(&path, &bytes).map_err(|e| format!("字幕写盘失败: {e}"))?;

    // 挂载：mpv 在播则 sub-add select（idle 会话会失败 → mounted:false，文件已在缓存随时可手动挂）
    let mut mounted = false;
    let session = state.mpv.lock().await.clone();
    if let Some(m) = session {
        let title = safe
            .rsplit_once('.')
            .map(|(t, _)| t.to_string())
            .unwrap_or_else(|| safe.clone());
        mounted = m
            .command(json!(["sub-add", path.to_string_lossy(), "select", title]))
            .await
            .is_ok();
    }
    Ok(json!({
        "path": path.to_string_lossy(),
        "title": safe,
        "mounted": mounted,
    }))
}
