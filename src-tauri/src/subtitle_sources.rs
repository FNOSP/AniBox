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

/// [lc-1784] OpenSubtitles 内置共享 Api-Key：构建期由 secrets/opensubtitles.local 注入
/// （见 build.rs emit_opensub_key）。官方预期「一应用一 key」模式；匿名下载配额按
/// IP+key 计（每用户约 5 次/天），内置不会随用户数挤兑。未注入 = 空串 = 源不可用（非致命）。
const BUILTIN_OPENSUB_KEY: &str = match option_env!("ANIBOX_OPENSUB_KEY") {
    Some(v) => v,
    None => "",
};

/// [lc-1786] 生效的 OpenSubtitles key：use_builtin=true → 内置共享（未注入=None）；
/// false → 用户 token（空则回落内置，防手滑失效）。
fn effective_osub_key(list: &[SubtitleSource], custom: Option<&str>) -> Option<String> {
    // [lc-1789] 第一层：设置面板「自定义密钥」（config.json）——直接顶掉内置（含未注入的情形）
    if let Some(c) = custom.filter(|c| !c.trim().is_empty()) {
        return Some(c.trim().to_string());
    }
    if let Some(k) = list
        .iter()
        .find(|s| s.kind == "opensubtitles" && s.enabled && !s.use_builtin && !s.token.trim().is_empty())
        .map(|s| s.token.trim().to_string())
    {
        return Some(k);
    }
    if BUILTIN_OPENSUB_KEY.is_empty() {
        None
    } else {
        Some(BUILTIN_OPENSUB_KEY.to_string())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSource {
    /// assrt | opensubtitles | xunlei
    pub kind: String,
    pub name: String,
    /// assrt 的 token / opensubtitles 的 Api-Key；xunlei 不需要（空串）
    #[serde(default)]
    pub token: String,
    /// [lc-1786] 凭证来源开关：true=内置共享（打包注入；token 字段忽略），
    /// false=用上面的 token（用户自填）。旧 JSON 无此字段 → 默认 true（与旧「空 token=内置」语义兼容）。
    /// xunlei 无凭证，忽略。
    #[serde(default = "default_true")]
    pub use_builtin: bool,
    pub enabled: bool,
}

fn default_true() -> bool {
    true
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
            use_builtin: false,
            enabled: true,
        },
        SubtitleSource {
            kind: "assrt".into(),
            name: "assrt.net".into(),
            token: String::new(),
            use_builtin: true,
            enabled: true,
        },
        SubtitleSource {
            kind: "opensubtitles".into(),
            name: "OpenSubtitles".into(),
            token: String::new(),
            use_builtin: true,
            enabled: true,
        },
    ]
}

fn valid_kind(k: &str) -> bool {
    k == "assrt" || k == "opensubtitles" || k == "xunlei" || k == "acgrip" || k == "subhd"
}

/// 读列表（缺失/损坏 → 内置默认；坏项丢弃，面板一保存即修复）。
#[tauri::command]
pub async fn subtitle_sources_get(app: AppHandle) -> Result<Value, String> {
    let p = file_path(&app)?;
    let mut list: Vec<SubtitleSource> = match std::fs::read_to_string(&p) {
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
    // [lc-1786] 旧 JSON 迁移：补 osub 默认条目并立即落盘（用户之后删除即显式不用）
    let need_migrate = p.is_file() && !list.iter().any(|s| s.kind == "opensubtitles");
    if !list.iter().any(|s| s.kind == "opensubtitles") {
        if let Some(d) = defaults().into_iter().find(|s| s.kind == "opensubtitles") {
            list.push(d);
        }
    }
    if need_migrate {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(body) = serde_json::to_string_pretty(&json!({ "sources": list })) {
            let _ = std::fs::write(&p, body);
        }
    }
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
        // [lc-1786] 内置开关关闭（use_builtin=false）时自定义 token 必填；开启时 token 忽略（可留空）。
        if !s.use_builtin && (s.kind == "assrt" || s.kind == "opensubtitles") && s.token.trim().is_empty() {
            return Err(format!("「{}」已关闭内置凭证，需填写自定义 token / Api-Key", s.name));
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
    fn acg_cookie_merge_overwrites() {
        let merged = acg_merge_cookie("a=1; b=2", &["b=3; path=/".to_string(), "c=4".to_string()]).unwrap();
        assert_eq!(merged, "a=1; b=3; c=4");
        assert!(acg_merge_cookie("a=1", &[]).is_none());
        let del = acg_merge_cookie("a=1; b=2", &["b=deleted".to_string()]).unwrap();
        assert_eq!(del, "a=1");
    }

    #[test]
    fn acg_strip_tags_unescapes() {
        assert_eq!(strip_tags("<b>你好&nbsp;</b>&amp;再见"), "你好 &再见");
    }

    #[test]
    fn kind_whitelist() {
        assert!(valid_kind("assrt"));
        assert!(valid_kind("opensubtitles"));
        assert!(valid_kind("xunlei"));
        assert!(valid_kind("subhd"));
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

/// [lc-1786] assrt 凭证：use_builtin=true → 内置共享；false → 用户 token（空则回落内置，
/// 避免手滑关了开关没填 token 导致整源失效）。
fn effective_assrt_token(list: &[SubtitleSource], custom: Option<&str>) -> String {
    // [lc-1789] 第一层：设置面板「自定义密钥」（config.json）——直接顶掉内置
    if let Some(c) = custom.filter(|c| !c.trim().is_empty()) {
        return c.trim().to_string();
    }
    list.iter()
        .find(|s| s.kind == "assrt" && s.enabled && !s.use_builtin && !s.token.trim().is_empty())
        .map(|s| s.token.trim().to_string())
        .unwrap_or_else(|| DEFAULT_ASSRT_TOKEN.to_string())
}

/// [lc-1784] 启动时把内置共享 Key 落地到 `app_data_dir/opensub-builtin.key`
/// （仅本机；与用户在 subtitle-sources.json 自填 key 同等暴露面），供 mpv 侧
/// sub-assrt.lua 读取——否则 lua 拿不到编译期注入的常量。内置为空则删掉旧文件。
pub fn sync_builtin_key(app: &AppHandle) {
    let Ok(dir) = app.path().app_data_dir() else { return };
    let p = dir.join("opensub-builtin.key");
    if BUILTIN_OPENSUB_KEY.is_empty() {
        let _ = std::fs::remove_file(&p);
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&p, BUILTIN_OPENSUB_KEY);
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
    let mut list = match std::fs::read_to_string(p) {
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
    // [lc-1786] 迁移：旧 JSON 无 osub 条目 → 补默认（use_builtin=true）。内置共享的意义是
    // 零配置可用；用户之后删除 = 显式不用，不再复活。
    if !list.iter().any(|s| s.kind == "opensubtitles") {
        if let Some(d) = defaults().into_iter().find(|s| s.kind == "opensubtitles") {
            list.push(d);
        }
    }
    list
}

/// 应用内字幕搜索：并行扇出 迅雷(名称) + assrt + OpenSubtitles(若配 key)。
/// 返回 {items:[{kind,id,title,hint}]}；单源失败不拖垮整体（跳过即可）。
#[tauri::command]
pub async fn subtitle_search(
    state: tauri::State<'_, crate::AppState>,
    query: String,
) -> Result<Value, String> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Err("搜索词为空".into());
    }
    let (custom_assrt, custom_osub) = {
        let cfg = state.config.read().await;
        (
            cfg.sub_assrt_token.trim().to_string(),
            cfg.sub_osub_key.trim().to_string(),
        )
    };
    let list = read_list().await;
    let token = effective_assrt_token(&list, Some(&custom_assrt));
    let osub_key = effective_osub_key(&list, Some(&custom_osub));
    let want_osub = list.iter().any(|s| s.kind == "opensubtitles" && s.enabled);
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
        if !want_osub {
            return Vec::new();
        }
        let Some(key) = osub_key else { return Vec::new() };
        let url = format!("https://api.opensubtitles.com/api/v1/subtitles?query={enc}&languages=zh-cn,zh,en");
        let Ok(resp) = http_client()
            .get(&url)
            .header("Api-Key", &key)
            .header("User-Agent", "AniBox v4")
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

    // [lc-1790] ACG 论坛：配了账号且还没 Cookie 时先登录（一次）；未配账号也允许游客搜索
    let acg_fut = async {
        if !list.iter().any(|s| s.kind == "acgrip" && s.enabled) {
            return Vec::new();
        }
        let cfg = state.config.read().await;
        let (u, p) = (cfg.acg_user.trim().to_string(), cfg.acg_pass.trim().to_string());
        drop(cfg);
        if !u.is_empty() && !p.is_empty() && !acg_load_cookie().contains("auth") {
            if let Err(e) = acg_login(&u, &p).await {
                crate::libmpv::trace(&format!("[subtitle] ACG 登录失败（继续游客搜索）: {e}"));
            }
        }
        match acg_search(&q).await {
            Ok(v) => v,
            Err(e) => {
                crate::libmpv::trace(&format!("[subtitle] ACG 源不可用: {e}"));
                Vec::new()
            }
        }
    };

    // [lc-1791] SubHD：搜索免登录（下载才需要账号）
    let subhd_fut = async {
        if !list.iter().any(|s| s.kind == "subhd" && s.enabled) {
            return Vec::new();
        }
        match subhd_search(&q).await {
            Ok(v) => v,
            Err(e) => {
                crate::libmpv::trace(&format!("[subtitle] SubHD 源不可用: {e}"));
                Vec::new()
            }
        }
    };

    let (a, b, c, d, e) = tokio::join!(xunlei_fut, assrt_fut, osub_fut, acg_fut, subhd_fut);
    let mut items = a;
    items.extend(b);
    items.extend(c);
    items.extend(d);
    items.extend(e);
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
            let custom = state.config.read().await.sub_assrt_token.trim().to_string();
            let list = read_list().await;
            let token = effective_assrt_token(&list, Some(&custom));
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
            let custom = state.config.read().await.sub_osub_key.trim().to_string();
            let list = read_list().await;
            let Some(key) = effective_osub_key(&list, Some(&custom)) else {
                return Err("OpenSubtitles 不可用（未配置 Api-Key 且应用未内置）".into());
            };
            let fid = id.parse::<i64>().map_err(|_| "file_id 非法")?;
            let v: Value = http_client()
                .post("https://api.opensubtitles.com/api/v1/download")
                .header("Api-Key", &key)
                .header("User-Agent", "AniBox v4")
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
        "acgrip" => {
            // 帖子 id → 页内第一个字幕附件直链
            acg_download(&id).await?
        }
        "subhd" => {
            // [lc-1791] SubHD：prepare-download → /down 直链字节（自带 referer/cookie，独立落盘）
            let (email, pass) = {
                let cfg = state.config.read().await;
                (cfg.subhd_email.trim().to_string(), cfg.subhd_pass.trim().to_string())
            };
            let (bytes, fname) = subhd_download(&id, &email, &pass).await?;
            let dir = std::env::temp_dir().join("anibox-subtitles");
            std::fs::create_dir_all(&dir).map_err(|e2| format!("建缓存目录失败: {e2}"))?;
            let safe: String = fname
                .chars()
                .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
                .collect();
            let path = dir.join(format!("subhd_{}", safe));
            std::fs::write(&path, &bytes).map_err(|e2| format!("字幕写盘失败: {e2}"))?;
            let mut mounted = false;
            let session = state.mpv.lock().await.clone();
            if let Some(m) = session {
                let title = safe.rsplit_once('.').map(|(t, _)| t.to_string()).unwrap_or_else(|| safe.clone());
                mounted = m
                    .command(json!(["sub-add", path.to_string_lossy(), "select", title]))
                    .await
                    .is_ok();
            }
            return Ok(json!({
                "path": path.to_string_lossy(),
                "title": safe,
                "mounted": mounted,
            }));
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

// ───────────────────────── [lc-1790] ACG 字幕论坛（bbs.acgrip.com，Discuz）─────────────────────────
// 账号源：设置面板「论坛账号」填用户名/密码 → 登录换 Cookie（缓存 app_data_dir/acgrip-cookie.txt）
// → 搜索 search.php（结果=帖子）→ 下载打开帖子取第一个字幕附件（mod=attachment&aid=）。
// ⚠ Cloudflare：论坛在 CF 后面，非浏览器 TLS 指纹的请求可能 403 —— 此时返回明确错误
// （「被论坛防火墙拦截」），不影响其它源。
// SubHD（subhd.me）经实测：免登录可搜，但详情页版本列表与下载链接全部由前端 JS 组装
// （无静态 dl 链接），reqwest 复刻不可靠 —— 不做，待后续用内置浏览器（open_browser_window
// 同款 WebView）承接。

const ACG_HOST: &str = "https://bbs.acgrip.com";

fn acg_cookie_path() -> Option<std::path::PathBuf> {
    std::env::var("APPDATA").ok().map(|ad| {
        std::path::PathBuf::from(ad)
            .join("com.anibox.app")
            .join("acgrip-cookie.txt")
    })
}

fn acg_ua() -> &'static str {
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
}

fn acg_load_cookie() -> String {
    acg_cookie_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn acg_store_cookie(cookie: &str) {
    if let Some(p) = acg_cookie_path() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&p, cookie);
    }
}

/// 从 Set-Cookie 头收集/合并 Cookie（极简 jar：键值对合并，新值覆盖旧值）。
fn acg_merge_cookie(old: &str, set_cookies: &[String]) -> Option<String> {
    let mut map: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for kv in old.split(';').filter_map(|s| {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        s.split_once('=')
    }) {
        map.insert(kv.0.to_string(), kv.1.to_string());
    }
    let mut changed = false;
    for sc in set_cookies {
        // Set-Cookie: name=value; path=/ ...
        if let Some(pair) = sc.split(';').next() {
            if let Some((k, v)) = pair.split_once('=') {
                if v.is_empty() || v.eq_ignore_ascii_case("deleted") {
                    if map.remove(k.trim()).is_some() {
                        changed = true;
                    }
                } else {
                    map.insert(k.trim().to_string(), v.trim().to_string());
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return None;
    }
    Some(
        map.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

async fn acg_get(path_q: &str, referer: Option<&str>) -> Result<(String, u16, String), String> {
    let url = format!("{ACG_HOST}{path_q}");
    let mut req = http_client()
        .get(&url)
        .header("user-agent", acg_ua())
        .header("accept-language", "zh-CN,zh;q=0.9");
    let cookie = acg_load_cookie();
    if !cookie.is_empty() {
        req = req.header("cookie", &cookie);
    }
    if let Some(r) = referer {
        req = req.header("referer", r);
    }
    let resp = req.send().await.map_err(|e| format!("ACG 请求失败: {e}"))?;
    let status = resp.status().as_u16();
    let setc: Vec<String> = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(|s| s.to_string()))
        .collect();
    let body = resp.text().await.unwrap_or_default();
    if let Some(merged) = acg_merge_cookie(&cookie, &setc) {
        acg_store_cookie(&merged);
    }
    Ok((body, status, cookie))
}

/// 登录（formhash 两段式）。成功条件：返回体含欢迎/成功标记且拿到 Cookie。
async fn acg_login(user: &str, pass: &str) -> Result<(), String> {
    let (page, st, _) = acg_get("/member.php?mod=logging&action=login&infloat=yes", None).await?;
    if st == 403 {
        return Err("ACG 论坛被 Cloudflare 拦截（非浏览器请求被拦），此源在当前网络不可用".into());
    }
    let formhash = extract_first(&page, r#"name="formhash" value="([a-f0-9]+)""#)
        .or_else(|| extract_first(&page, r#"formhash=([a-f0-9]+)"#))
        .ok_or("ACG 登录页未取到 formhash")?;
    let body = format!(
        "formhash={formhash}&username={}&password={}&loginfield=username&cookietime=2592000&questionid=&answer=&handlekey=login&loginsubmit=yes&lssubmit=yes&inajax=1",
        urlencoding::encode(user),
        urlencoding::encode(pass)
    );
    let url = format!("{ACG_HOST}/member.php?mod=logging&action=login&loginsubmit=yes&lssubmit=yes&inajax=1");
    let resp = http_client()
        .post(&url)
        .header("user-agent", acg_ua())
        .header("content-type", "application/x-www-form-urlencoded")
        .header("referer", format!("{ACG_HOST}/member.php?mod=logging&action=login"))
        .header("cookie", acg_load_cookie())
        .body(body)
        .send()
        .await
        .map_err(|e| format!("ACG 登录请求失败: {e}"))?;
    let text = resp.text().await.unwrap_or_default();
    if text.contains("succeedhandle_login") || text.contains("欢迎") || text.contains("成功") {
        return Ok(());
    }
    // 失败时把响应里的提示抽出来
    let msg = extract_first(&text, r#"CDATA\[(.{2,80})\]"#).unwrap_or_else(|| "用户名或密码错误".into());
    Err(format!("ACG 登录失败: {msg}"))
}

fn extract_first(html: &str, re: &str) -> Option<String> {
    let re = regex::Regex::new(re).ok()?;
    re.captures(html)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

/// HTML 实体/标签清洗（结果标题展示用）。
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

/// ACG 搜索（结果=帖子列表）。未登录时 Discuz 也能搜，但附件下载需要登录。
async fn acg_search(query: &str) -> Result<Vec<Value>, String> {
    let kw = urlencoding::encode(query);
    let (body, st, _) = acg_get(&format!("/search.php?mod=forum&adv=yes&srchtxt={kw}"), Some(ACG_HOST)).await?;
    if st == 403 {
        return Err("ACG 论坛被 Cloudflare 拦截".into());
    }
    if body.contains("verify_check") || body.contains("seccode") {
        return Err("ACG 搜索触发验证码（短时间搜索过频），稍后再试".into());
    }
    let re = regex::Regex::new(
        r#"(?s)<a[^>]+href="(?:https?://bbs\.acgrip\.com)?/(?:forum\.php\?mod=viewthread&tid=|thread-\d+-1-1\.html|thread-search\.php[^"]*tid=)(\d+)[^"]*"[^>]*>(.{2,120}?)</a>"#,
    )
    .map_err(|e| e.to_string())?;
    let mut out: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in re.captures_iter(&body) {
        let tid = c.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        let title = strip_tags(c.get(2).map(|m| m.as_str()).unwrap_or(""));
        if tid.is_empty() || title.is_empty() || !seen.insert(tid.clone()) {
            continue;
        }
        out.push(source_item("acgrip", &tid, &title, "ACG论坛·帖子"));
        if out.len() >= 10 {
            break;
        }
    }
    Ok(out)
}

/// ACG 下载：帖子页取第一个字幕附件（mod=attachment&aid=）。
async fn acg_download(tid: &str) -> Result<(String, String), String> {
    let (body, st, _) = acg_get(&format!("/forum.php?mod=viewthread&tid={tid}"), Some(ACG_HOST)).await?;
    if st == 403 {
        return Err("ACG 论坛被 Cloudflare 拦截".into());
    }
    if body.contains("您尚未登录") || body.contains("请先登录") {
        return Err("ACG 附件下载需登录（设置→播放器→字幕源→论坛账号）".into());
    }
    // 附件：aid 与文件名（Discuz：filedown/attachment 链接 + 附近文件名文本）
    let re = regex::Regex::new(r#"(?s)href="(?:https?://bbs\.acgrip\.com)?/(?:forum\.php\?mod=attachment&|attachment\.php\?)(?:aid|id)=(\d+)[^"]*"[^>]*>(.{0,80}?)</a>"#)
        .map_err(|e| e.to_string())?;
    let mut pick: Option<(String, String)> = None;
    for c in re.captures_iter(&body) {
        let aid = c.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        let name = strip_tags(c.get(2).map(|m| m.as_str()).unwrap_or(""));
        let ext_ok = |n: &str| {
            let e = n.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            matches!(e.as_str(), "srt" | "ass" | "ssa" | "vtt" | "sub" | "zip" | "rar" | "7z")
        };
        // 优先裸字幕文件名；都不带扩展名时取第一个附件
        if !aid.is_empty() && ext_ok(&name) {
            pick = Some((aid, name));
            break;
        }
        if pick.is_none() && !aid.is_empty() && !name.is_empty() {
            pick = Some((aid, if name.contains('.') { name } else { format!("{name}.zip") }));
        }
    }
    let (aid, name) = pick.ok_or("帖子内没有找到字幕附件")?;
    let url = format!("{ACG_HOST}/forum.php?mod=attachment&aid={aid}");
    Ok((url, name))
}

// ───────────────────────── [lc-1791] SubHD（subhd.me）─────────────────────────
// 官方 JS（/public/js/subhd.js）实证链路：
//   登录 POST /set/login（email/password）→ 登录态详情页渲染 .subtitle-prepare-download
//   按钮（data-sid）→ POST /api/sub/prepare-download {"sid":…} → {success,url:"/down/<t>"}
//   → GET https://subhd.me/down/<t>（302 → dl.subhd.me，需 referer）。
// 搜索免登录（实测 200）。下载需免费账号（设置面板「SubHD 账号」邮箱/密码）。
// Cookie 复用 ACG 的极简 jar（subhd-cookie.txt）。

const SUBHD_HOST: &str = "https://subhd.me";

fn subhd_cookie_path() -> Option<std::path::PathBuf> {
    std::env::var("APPDATA").ok().map(|ad| {
        std::path::PathBuf::from(ad)
            .join("com.anibox.app")
            .join("subhd-cookie.txt")
    })
}

fn subhd_load_cookie() -> String {
    subhd_cookie_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// 带 Cookie 的 GET（回落通用 UA 常量在 acg_ua；SubHD 同款浏览器 UA 即可）。
async fn subhd_get(path_q: &str, referer: Option<&str>) -> Result<(String, u16), String> {
    let url = if path_q.starts_with("http") {
        path_q.to_string()
    } else {
        format!("{SUBHD_HOST}{path_q}")
    };
    let mut req = http_client()
        .get(&url)
        .header("user-agent", acg_ua())
        .header("accept-language", "zh-CN,zh;q=0.9");
    let ck = subhd_load_cookie();
    if !ck.is_empty() {
        req = req.header("cookie", &ck);
    }
    if let Some(r) = referer {
        req = req.header("referer", r);
    }
    let resp = req.send().await.map_err(|e| format!("SubHD 请求失败: {e}"))?;
    let status = resp.status().as_u16();
    let setc: Vec<String> = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(|s| s.to_string()))
        .collect();
    let body = resp.text().await.unwrap_or_default();
    if let Some(merged) = acg_merge_cookie(&ck, &setc) {
        if let Some(p) = subhd_cookie_path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&p, &merged);
        }
    }
    Ok((body, status))
}

/// 登录：GET /set/login 拿初始 Cookie → POST email/password → 复查登录页确认。
async fn subhd_login(email: &str, pass: &str) -> Result<(), String> {
    let _ = subhd_get("/set/login", Some(SUBHD_HOST)).await?;
    let body = format!("email={}&password={}", urlencoding::encode(email), urlencoding::encode(pass));
    let resp = http_client()
        .post(format!("{SUBHD_HOST}/set/login"))
        .header("user-agent", acg_ua())
        .header("content-type", "application/x-www-form-urlencoded")
        .header("referer", format!("{SUBHD_HOST}/set/login"))
        .header("cookie", subhd_load_cookie())
        .body(body)
        .send()
        .await
        .map_err(|e| format!("SubHD 登录请求失败: {e}"))?;
    let setc: Vec<String> = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(|s| s.to_string()))
        .collect();
    let text = resp.text().await.unwrap_or_default();
    let ck = subhd_load_cookie();
    if let Some(merged) = acg_merge_cookie(&ck, &setc) {
        if let Some(p) = subhd_cookie_path() {
            let _ = std::fs::write(&p, &merged);
        }
    }
    if text.contains("密码") && text.contains("email") && text.len() < 20000 {
        return Err("SubHD 登录失败（邮箱或密码错误）".into());
    }
    // 复查：登录页不再出现密码框 = 已登录
    let (page, _) = subhd_get("/set/login", Some(SUBHD_HOST)).await?;
    if page.contains(r#"name="password""#) {
        return Err("SubHD 登录未生效（站点风控？稍后再试）".into());
    }
    Ok(())
}

/// 搜索（免登录）：/search/<kw> → /d/<id> + 标题。
async fn subhd_search(query: &str) -> Result<Vec<Value>, String> {
    let kw = urlencoding::encode(query);
    let (body, st) = subhd_get(&format!("/search/{kw}"), Some(SUBHD_HOST)).await?;
    if st != 200 {
        return Err(format!("SubHD 搜索返回 {st}"));
    }
    if body.contains("please login") {
        return Err("SubHD 要求登录后才可搜索（在设置里配置 SubHD 账号）".into());
    }
    let re = regex::Regex::new(r#"(?s)<a[^>]+href="(?:https?://subhd\.me)?(/d/(\d+))"[^>]*>(.{2,120}?)</a>"#)
        .map_err(|e| e.to_string())?;
    let mut out: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in re.captures_iter(&body) {
        let id = c.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
        let title = strip_tags(c.get(3).map(|m| m.as_str()).unwrap_or(""));
        if id.is_empty() || title.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        out.push(source_item("subhd", &id, &title, "SubHD·条目"));
        if out.len() >= 10 {
            break;
        }
    }
    Ok(out)
}

/// 下载：详情页（登录态）取第一个版本 sid → prepare-download → /down 直链字节。
async fn subhd_download(did: &str, email: &str, pass: &str) -> Result<(Vec<u8>, String), String> {
    // 有凭证且无会话 → 先登录
    if !email.is_empty() && !pass.is_empty() {
        if let Err(e) = subhd_login(email, pass).await {
            crate::libmpv::trace(&format!("[subtitle] SubHD 登录失败: {e}"));
        }
    }
    let (page, st) = subhd_get(&format!("/d/{did}"), Some(SUBHD_HOST)).await?;
    if st != 200 {
        return Err(format!("SubHD 详情页返回 {st}"));
    }
    let sid = extract_first(&page, r#"data-sid="(\d+)""#)
        .ok_or("详情页没有可下载版本（SubHD 下载需登录，请在设置配置账号）")?;
    // 版本行标题（sid 附近文本兜底）
    let name = extract_first(&page, r#"(?s)data-sid="\d+"[^>]*>.{0,200}?download-prepare-text[^>]*>\s*([^<]{2,60})"#)
        .map(|n| strip_tags(&n))
        .unwrap_or_else(|| format!("subhd_{sid}.zip"));
    let display = if name.contains('.') { name } else { format!("{name}.zip") };
    // prepare-download（官方 JS 同款）
    let resp = http_client()
        .post(format!("{SUBHD_HOST}/api/sub/prepare-download"))
        .header("user-agent", acg_ua())
        .header("content-type", "application/json; charset=utf-8")
        .header("referer", format!("{SUBHD_HOST}/d/{did}"))
        .header("cookie", subhd_load_cookie())
        .body(json!({ "sid": sid }).to_string())
        .send()
        .await
        .map_err(|e| format!("SubHD prepare-download 失败: {e}"))?;
    let v: Value = resp.json().await.map_err(|e| format!("SubHD 响应解析失败: {e}"))?;
    if v.get("success").and_then(|x| x.as_bool()) != Some(true) {
        let msg = v.get("msg").and_then(|x| x.as_str()).unwrap_or("未知错误");
        return Err(format!("SubHD 不允许下载: {msg}"));
    }
    let down_path = v.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if !down_path.starts_with("/down/") {
        return Err("SubHD 未返回下载地址".into());
    }
    // 下载直链（302 → dl.subhd.me；referer 必带）
    let dl = format!("{SUBHD_HOST}{down_path}");
    let bytes = http_client()
        .get(&dl)
        .header("user-agent", acg_ua())
        .header("referer", format!("{SUBHD_HOST}/d/{did}"))
        .header("cookie", subhd_load_cookie())
        .send()
        .await
        .map_err(|e| format!("SubHD 下载失败: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("SubHD 下载读取失败: {e}"))?;
    Ok((bytes.to_vec(), display))
}
