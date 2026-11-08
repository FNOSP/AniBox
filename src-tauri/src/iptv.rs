//! [lc-2077] 发现页 IPTV 栏 —— 直播源数据**直接复用**开源聚合项目 vbskycn/iptv（GPL-3.0）。
//!
//! 该项目每 6 小时在服务端自动采集/测速/更新一份 `iptv4.m3u`（约 435 个频道，按 央视/卫视/
//! 电影/纪录/儿童/地方 分组，`#EXTINF` 自带 tvg-logo 台标）。我们只做「取 + 解析 + 播」：
//!   · 数据源：主链 `https://live.zbds.top/tv/iptv4.m3u`（实测国内直连 0.4s 可达），
//!     备用 gh-proxy.com 的 raw 镜像；超时/失败逐个回退，全挂时回吐过期缓存（旧数据比报错强）。
//!   · 缓存：disk_cache JSON 层，TTL 6h（与上游更新节奏一致）。
//!   · 播放：频道 URL 原样交 `player::launch`（`direct=true` 跳过 Go 代理 —— 与飞牛直播
//!     直链同一口径：`playvideo` 代理对直播不可用，见 lib.rs [live] 注释），不落进度、不记观影历史。
//!
//! GPL-3.0 合规：数据来源已在发现页 IPTV 栏底部注明（前端）+ 此处注释；只消费其公开订阅
//! 地址、不分发其代码。参考 README：https://github.com/vbskycn/iptv
//!
//! NOT IMPLEMENTED（后续接入）：EPG 节目单（m3u 头部 x-tvg-url 有现成地址）、用户自定义
//! 订阅地址、频道内搜索。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

/// 数据源顺序：主链在前，备用在后（上游 README 承认部分运营商会污染主域名）。
const IPTV_SOURCES: &[&str] = &[
    "https://live.zbds.top/tv/iptv4.m3u",
    "https://gh-proxy.com/raw.githubusercontent.com/vbskycn/iptv/refs/heads/master/tv/iptv4.m3u",
];
/// 缓存 TTL：6 小时（上游每 6 小时更新一次，拉太勤没有意义）。
const IPTV_TTL_SECS: u64 = 6 * 3600;
/// 缓存键（iptv4 = IPv4 源；IPv6 源上游自己都承认大部分已失效，不采）。
const IPTV_CACHE_KEY: &str = "iptv:m3u:iptv4";

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("AniBox/1.0")
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default()
    })
}

/// 读 `key="value"` 形态的 EXTINF 属性（tvg-name / tvg-logo / group-title）。
fn ext_attr(attrs: &str, key: &str) -> String {
    let pat = format!("{key}=\"");
    if let Some(i) = attrs.find(&pat) {
        let rest = &attrs[i + pat.len()..];
        if let Some(j) = rest.find('"') {
            return rest[..j].trim().to_string();
        }
    }
    String::new()
}

/// 解析 m3u 文本 → 分组频道列表（保持出现顺序）。
/// 同组同名只留第一条：上游已是「每频道择优」，后面的重复多为备用线路（换台都用不上）。
fn parse_m3u(text: &str) -> Vec<Value> {
    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    let mut seen: HashMap<(String, String), ()> = HashMap::new();
    // cur = 上一条 #EXTINF 解析出的（频道名, 台标, 分组），等下一行非注释行给 URL。
    let mut cur: Option<(String, String, String)> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTINF") {
            // 标准形态：#EXTINF:-1 tvg-name="…" tvg-logo="…" group-title="…", 频道名
            let after = rest.split_once(':').map(|x| x.1).unwrap_or(rest);
            let (attrs, name) = match after.split_once(',') {
                Some((a, n)) => (a, n.trim()),
                None => (after, ""),
            };
            let name = if name.is_empty() {
                ext_attr(attrs, "tvg-name")
            } else {
                name.to_string()
            };
            let logo = ext_attr(attrs, "tvg-logo");
            let group = {
                let g = ext_attr(attrs, "group-title");
                if g.is_empty() { "其他".to_string() } else { g }
            };
            cur = Some((name, logo, group));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        // 非注释非空行 = 上一条 EXTINF 的播放地址。
        if let Some((name, logo, group)) = cur.take() {
            if name.is_empty() || line.is_empty() {
                continue;
            }
            let key = (group.clone(), name.clone());
            if seen.contains_key(&key) {
                continue;
            }
            seen.insert(key, ());
            let entry = json!({ "name": name, "logo": logo, "url": line });
            match groups.iter_mut().find(|(g, _)| *g == group) {
                Some((_, rows)) => rows.push(entry),
                None => groups.push((group, vec![entry])),
            }
        }
    }
    groups
        .into_iter()
        .map(|(name, channels)| json!({ "name": name, "channels": channels }))
        .collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 发现页 IPTV 栏频道列表（分组，含台标；缓存 6h，全源失败回吐过期缓存）。
#[tauri::command]
pub async fn iptv_channels_get() -> Result<Value, String> {
    if let Some(v) = crate::disk_cache::json_get(IPTV_CACHE_KEY, IPTV_TTL_SECS) {
        return Ok(v);
    }
    let mut last_err = String::new();
    for src in IPTV_SOURCES {
        let resp = match client().get(*src).send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("{src}: {e}");
                continue;
            }
        };
        if !resp.status().is_success() {
            last_err = format!("{src}: HTTP {}", resp.status());
            continue;
        }
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                last_err = format!("{src}: {e}");
                continue;
            }
        };
        if !text.contains("#EXTINF") {
            last_err = format!("{src}: 返回内容不是 m3u");
            continue;
        }
        let groups = parse_m3u(&text);
        if groups.is_empty() {
            last_err = format!("{src}: 频道解析为空");
            continue;
        }
        let v = json!({ "updated": now_ms(), "groups": groups });
        crate::disk_cache::json_put(IPTV_CACHE_KEY, &v);
        return Ok(v);
    }
    // 全源失败：过期缓存也回吐（标记 stale 让前端可以提示「数据不是最新的」）。
    if let Some(mut v) = crate::disk_cache::json_get(IPTV_CACHE_KEY, 0) {
        if let Some(o) = v.as_object_mut() {
            o.insert("stale".into(), json!(true));
        }
        return Ok(v);
    }
    Err(format!("直播源拉取失败：{last_err}"))
}

/// 播放 IPTV 频道：直链原样交播放器（direct=true 跳过 Go 代理/relay，与飞牛直播同口径）。
/// 不落进度、不记观影历史（直播无进度概念，history 只服务点播）。
#[tauri::command]
pub async fn iptv_play(
    state: tauri::State<'_, crate::AppState>,
    app: tauri::AppHandle,
    title: String,
    url: String,
) -> Result<(), String> {
    let cfg = state.config.read().await.clone();
    crate::fnos_progress::begin("", "");
    crate::player::launch(
        &app, &state, &cfg, &url, title.trim(), 0.0, 0.0, None, &[], None, true,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 解析：分组按出现顺序、同组同名去重（留首条）、EXTINF 属性与标题提取、
    /// 无 tvg-logo / 无 group-title 的兜底。
    #[test]
    fn parse_m3u_groups_dedup_and_attrs() {
        let m3u = concat!(
            "#EXTM3U x-tvg-url=\"http://epg/e.xml\"\n",
            "#EXTINF:-1 tvg-name=\"CCTV1\" tvg-id=\"CCTV1\" tvg-logo=\"http://l/cctv1.png\" group-title=\"央视频道\", CCTV1\nhttp://a/1.m3u8\n",
            "#EXTINF:-1 tvg-name=\"CCTV1\" tvg-logo=\"http://l/cctv1-b.png\" group-title=\"央视频道\", CCTV1\nhttp://a/2.m3u8\n",
            "#EXTINF:-1 tvg-name=\"BTV\" group-title=\"卫视频道\", 北京卫视\nhttp://b/1.m3u8\n",
            "#EXTINF:-1 tvg-name=\"X\", 无台标台\nhttp://c/1.m3u8\n",
            "#EXTVLCOPT:http-user-agent=ok\n",
        );
        let groups = parse_m3u(m3u);
        assert_eq!(groups.len(), 3); // 央视 / 卫视 / 其他（无 group-title 兜底）
        assert_eq!(groups[0]["name"], "央视频道");
        let chs = groups[0]["channels"].as_array().unwrap();
        assert_eq!(chs.len(), 1, "同组同名只留第一条");
        assert_eq!(chs[0]["url"], "http://a/1.m3u8");
        assert_eq!(chs[0]["logo"], "http://l/cctv1.png");
        assert_eq!(groups[1]["name"], "卫视频道");
        assert_eq!(groups[1]["channels"][0]["name"], "北京卫视");
        assert_eq!(groups[2]["name"], "其他");
        assert_eq!(groups[2]["channels"][0]["name"], "无台标台");
        assert_eq!(groups[2]["channels"][0]["logo"], "");
        // #EXTVLCOPT 等其它注释行不产生条目、不吞 URL
    }

    /// 标题缺失时回退 tvg-name；空名频道丢弃。
    #[test]
    fn parse_m3u_name_fallback() {
        let m3u = concat!(
            "#EXTINF:-1 tvg-name=\"CGTN\" group-title=\"央视频道\"\nhttp://a/1.m3u8\n",
            "#EXTINF:-1 group-title=\"央视频道\",\nhttp://a/2.m3u8\n",
        );
        let groups = parse_m3u(m3u);
        let chs = groups[0]["channels"].as_array().unwrap();
        assert_eq!(chs.len(), 1);
        assert_eq!(chs[0]["name"], "CGTN");
    }
}
