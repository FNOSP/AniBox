//! [lc-2230] 123 云盘取链所需的签名与文件元数据。
//!
//! 我们原来的实现有两个硬伤（2026-11 实测）：
//! 1. `file/download_info` 用 **GET + ?fileId=** —— 实测 `404 page not found`；
//!    正确的是 **POST JSON**，且 body 必须带 `etag / fileId / fileName / s3keyFlag / size / type`
//!    六项（缺一项上游回 20101 未登录以外的参数错）。
//! 2. 认证用 `Authorization: Bearer <access_token>`，不是 Cookie；请求 URL 还要
//!    带 **K/V 签名**（`signPath`，K=crc32(替换表编码的本地时间)，V=ts-rand-crc32）。
//!
//! 算法与字段口径来自 `nianzhibai/91`（MIT）`backend/internal/drives/p123`，
//! 并经本机实测核对（未带 token 时返回 `{"code":20101,"message":"未登录"}`，
//! 说明方法/路径/签名形状已被上游接受，只差凭据）。

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::cloud::{client, now_ms_pub, require_cred};

const P123_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36";

pub(crate) fn p123_ua() -> &'static str {
    P123_UA
}

const MAIN_API: &str = "https://api.123278.com/b/api";
const PLATFORM: &str = "web";
const APP_VERSION: &str = "3";

// ───────────────────────── K/V 签名 ─────────────────────────

/// crc32（IEEE，与 Go `crc32.ChecksumIEEE` 同表）。这里手写查表版，
/// 避免为一次签名引入 crate（`crc32fast` 虽是传递依赖，但不值得为它加显式依赖）。
fn crc32_ieee(data: &[u8]) -> u32 {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc = table[((crc ^ (*b as u32)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// 123 的字符替换表（官方 web 客户端混淆时间戳用，索引 = 数字字符 - '0'）。
/// 照抄 91 `signPath` 的 26 项表（其中第 25 项是重复的 's'，不修正 —— 照抄口径）。
/// 实际只用得到下标 0..=9：`0→a 1→d 2→e 3→f 4→g 5→h 6→l 7→m 8→y 9→i`。
const TIME_TABLE: [u8; 26] = *b"adefghlmyijnopkqrstubcvwsz";

/// 生成本地时间（CST，+8）的 `YYYYMMDDHHmm`，逐位做替换表映射。
fn encoded_local_time() -> String {
    // 不引 chrono：直接按 UTC 秒 + 8h 手算日历（123 只认时分，日期只需要正确）。
    let secs = (now_ms_pub() / 1000) as i64 + 8 * 3600;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = rem / 3600;
    let mi = (rem % 3600) / 60;
    let digits = format!("{y:04}{m:02}{d:02}{hh:02}{mi:02}");
    digits
        .bytes()
        .map(|b| TIME_TABLE[(b.saturating_sub(b'0')) as usize % TIME_TABLE.len()] as char)
        .collect()
}

/// 天数（1970-01-01 起）→ (年, 月, 日)。Howard Hinnant 的 civil_from_days。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `signPath`：返回 (K, V)。K=timeSign，V=`ts-rand-crc32`。
fn sign_path(api_path: &str) -> (String, String) {
    let random = format!("{:.0}", 1e7 * (rand::random::<f64>()));
    let ts = (now_ms_pub() / 1000).to_string();
    let time_sign = crc32_ieee(encoded_local_time().as_bytes()).to_string();
    let data = format!("{ts}|{random}|{api_path}|{PLATFORM}|{APP_VERSION}|{time_sign}");
    let data_sign = crc32_ieee(data.as_bytes());
    (time_sign, format!("{ts}-{random}-{data_sign}"))
}

/// 带签名的 123 业务 POST。`path` 形如 `/file/download_info`。
pub(crate) async fn p123_post_signed(path: &str, body: Value) -> Result<Value, String> {
    let cred = require_cred("123", &["access_token"])?;
    let token = cred.get("access_token").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let (k, v) = sign_path(path);
    let url = format!("{MAIN_API}{path}?{k}={v}");
    let resp = client()
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .header("platform", PLATFORM)
        .header("app-version", APP_VERSION)
        .header("Origin", "https://www.123pan.com")
        .header("Referer", "https://www.123pan.com/")
        .header("User-Agent", p123_ua())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("123 请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("123 响应读取失败: {e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|_| {
        format!("123 返回非 JSON（HTTP {}）：{}", status.as_u16(), text.chars().take(120).collect::<String>())
    })?;
    let code = v.get("code").and_then(|x| x.as_i64()).unwrap_or(0);
    if code != 0 {
        let msg = v.get("message").and_then(|x| x.as_str()).unwrap_or("");
        return Err(format!("123 接口 {path} 被拒: code={code} {msg}"));
    }
    Ok(v)
}

// ───────────────────────── 文件元数据 ─────────────────────────

/// 取下载地址所需的六项元数据（`etag` / 数字 `fileId` / `fileName` / `s3keyFlag` / `size` / `type`）。
pub(crate) struct P123Meta {
    pub etag: String,
    pub numeric_id: i64,
    pub name: String,
    pub s3key_flag: String,
    pub size: u64,
}

/// 进程内缓存：`fileId → 元数据`。列目录时顺手灌入，取链时命中即零请求；
/// 未命中才按目录递归找（`download_info` 拿不到就退回逐层 list）。
fn meta_cache() -> &'static Mutex<HashMap<String, P123Meta>> {
    static M: OnceLock<Mutex<HashMap<String, P123Meta>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remember_meta(id: &str, m: P123Meta) {
    if let Ok(mut guard) = meta_cache().lock() {
        if guard.len() > 8192 {
            guard.clear(); // 简单上限：媒体库扫描可能塞很多条目
        }
        guard.insert(id.to_string(), m);
    }
}

/// 从一次 `file/list/new` 响应里把所有文件元数据灌进缓存。
/// 列目录本来就拿到了这些字段，不存下来等于每次取链都要再翻一遍目录（还容易撞限流）。
pub(crate) fn cache_from_list_response(v: &Value) {
    let Some(list) = v
        .pointer("/data/InfoList")
        .or_else(|| v.pointer("/data/fileList"))
        .or_else(|| v.pointer("/data/list"))
        .and_then(|x| x.as_array())
    else {
        return;
    };
    for it in list {
        let is_dir = it.get("Type").and_then(|x| x.as_i64()) == Some(1)
            || it.get("type").and_then(|x| x.as_i64()) == Some(1);
        if is_dir {
            continue;
        }
        let Some(id) = it
            .get("FileId")
            .or_else(|| it.get("fileId"))
            .or_else(|| it.get("file_id"))
        else {
            continue;
        };
        let key = id
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| id.as_i64().map(|n| n.to_string()))
            .unwrap_or_default();
        if key.is_empty() {
            continue;
        }
        let numeric = id
            .as_i64()
            .or_else(|| id.as_str().and_then(|s| s.parse::<i64>().ok()))
            .unwrap_or(0);
        if numeric == 0 {
            continue;
        }
        remember_meta(
            &key,
            P123Meta {
                etag: first_str(it, &["Etag", "etag"]),
                numeric_id: numeric,
                name: first_str(it, &["FileName", "filename", "fileName", "name"]),
                s3key_flag: first_str(it, &["S3KeyFlag", "s3keyFlag"]),
                size: it
                    .get("Size")
                    .or_else(|| it.get("size"))
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0),
            },
        );
    }
}

fn first_str(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
            return s.to_string();
        }
    }
    String::new()
}

/// 取文件元数据：先查缓存，未命中则从根目录递归找（深度上限 6，防无限下钻）。
pub(crate) async fn p123_metadata(file_id: &str) -> Result<P123Meta, String> {
    let id = file_id.trim();
    if id.is_empty() {
        return Err("123 文件 id 为空".into());
    }
    if let Some(m) = get_cached(id) {
        return Ok(m);
    }
    let mut visited: Vec<String> = Vec::new();
    match find_in_dir(id, "0", 0, &mut visited).await? {
        Some(m) => Ok(m),
        None => Err(format!(
            "123 找不到文件 {id} 的元数据（请重新浏览该目录后再播放，或确认文件仍在盘内）"
        )),
    }
}

impl P123Meta {
    /// 缓存需要 Clone；这里做个显式结构复制，避免整个结构 derive Clone 后
    /// 被误用到需要「独占所有权」的调用点。
    fn clone_for_cache(&self) -> P123Meta {
        P123Meta {
            etag: self.etag.clone(),
            numeric_id: self.numeric_id,
            name: self.name.clone(),
            s3key_flag: self.s3key_flag.clone(),
            size: self.size,
        }
    }
}

fn get_cached(id: &str) -> Option<P123Meta> {
    let g = meta_cache().lock().ok()?;
    g.get(id).map(|m| m.clone_for_cache())
}

async fn find_in_dir(
    target: &str,
    dir: &str,
    depth: usize,
    visited: &mut Vec<String>,
) -> Result<Option<P123Meta>, String> {
    if depth > 6 || visited.iter().any(|d| d == dir) {
        return Ok(None);
    }
    visited.push(dir.to_string());
    let v = p123_post_signed(
        "/file/list/new",
        json!({
            "driveId": 0,
            "limit": 100,
            "next": "0",
            "orderBy": "file_id",
            "orderDirection": "desc",
            "parentFileId": dir,
            "trashed": false,
            "SearchData": "",
            "Page": 1,
            "OnlyLookAbnormalFile": 0,
            "event": "homeListFile",
            "operateType": 4,
            "inDirectSpace": false,
        }),
    )
    .await?;
    cache_from_list_response(&v);
    if let Some(m) = get_cached(target) {
        return Ok(Some(m));
    }
    let dirs: Vec<String> = v
        .pointer("/data/InfoList")
        .or_else(|| v.pointer("/data/fileList"))
        .or_else(|| v.pointer("/data/list"))
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter(|it| {
                    it.get("Type").and_then(|x| x.as_i64()) == Some(1)
                        || it.get("type").and_then(|x| x.as_i64()) == Some(1)
                })
                .filter_map(|it| {
                    it.get("FileId")
                        .or_else(|| it.get("fileId"))
                        .and_then(|x| {
                            x.as_str().map(|s| s.to_string()).or_else(|| x.as_i64().map(|n| n.to_string()))
                        })
                })
                .collect()
        })
        .unwrap_or_default();
    for d in dirs {
        if let Some(m) = Box::pin(find_in_dir(target, &d, depth + 1, visited)).await? {
            return Ok(Some(m));
        }
    }
    Ok(None)
}

/// 123 的下载地址可能把真实下一跳藏在 `params`（base64）里。
pub(crate) fn p123_unwrap_params(raw: &str) -> String {
    use base64::Engine;
    let Ok(u) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    let Some(params) = u
        .query_pairs()
        .find(|(k, _)| k == "params")
        .map(|(_, v)| v.to_string())
    else {
        return raw.to_string();
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(params.trim())
        .ok()
        .and_then(|b| String::from_utf8(b).ok());
    match decoded {
        Some(s) if s.starts_with("http") => s,
        _ => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_reference_vector() {
        // 标准向量：crc32("123456789") = 0xCBF43926
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_ieee(b""), 0);
    }

    #[test]
    fn civil_dates_are_correct() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2026-11-18 = 1970-01-01 起 20775 天（Python datetime 交叉核对）
        assert_eq!(civil_from_days(20_775), (2026, 11, 18));
        // 闰日
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn encoded_time_has_expected_shape() {
        let s = encoded_local_time();
        assert_eq!(s.len(), 12, "YYYYMMDDHHmm 必须 12 位: {s}");
        // 替换表里全是小写字母，绝不能残留数字（残留=映射表算错）
        assert!(s.chars().all(|c| c.is_ascii_lowercase()), "{s}");
    }

    #[test]
    fn signature_parts_are_well_formed() {
        let (k, v) = sign_path("/file/download_info");
        assert!(k.parse::<u32>().is_ok(), "K 必须是 crc32 数字串: {k}");
        let parts: Vec<&str> = v.split('-').collect();
        assert_eq!(parts.len(), 3, "V 必须是 ts-rand-crc32: {v}");
        assert!(parts[0].parse::<i64>().is_ok());
        assert!(parts[2].parse::<u32>().is_ok());
    }

    #[test]
    fn params_unwrap_handles_plain_and_wrapped() {
        use base64::Engine;
        let inner = "https://cdn.example.com/a.mp4?sign=1";
        let wrapped = format!(
            "https://yun.123pan.cn/Origin?params={}",
            base64::engine::general_purpose::STANDARD.encode(inner)
        );
        assert_eq!(p123_unwrap_params(&wrapped), inner);
        // 没有 params / 不是 http 开头 → 原样返回
        assert_eq!(p123_unwrap_params(inner), inner);
        let bad = "https://yun.123pan.cn/Origin?params=!!!";
        assert_eq!(p123_unwrap_params(bad), bad);
    }

    #[test]
    fn list_response_populates_cache() {
        let v = json!({
            "data": { "InfoList": [
                { "FileId": 111, "FileName": "a.mp4", "Type": 0, "Etag": "abc", "S3KeyFlag": "k1", "Size": 9 },
                { "FileId": 222, "FileName": "dir", "Type": 1 }
            ]}
        });
        cache_from_list_response(&v);
        let m = get_cached("111").expect("文件条目应入缓存");
        assert_eq!(m.etag, "abc");
        assert_eq!(m.numeric_id, 111);
        assert_eq!(m.s3key_flag, "k1");
        assert_eq!(m.size, 9);
        assert!(get_cached("222").is_none(), "目录条目不该入缓存");
    }
}
