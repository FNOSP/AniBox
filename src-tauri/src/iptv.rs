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
/// [lc-2102] **多源合并**：单源的线路质量参差（实测同一份数据里 12.7MB/s 与 43KB/s
/// 并存、大量分片 404），换任何一个单源都解决不了 —— 真正的解法是「多个聚合源合并，
/// 同一频道攒出更多备线，播放时探活择优」。以下按可用性排序，逐个尝试、能拉到的都合并。
const IPTV_SOURCES: &[&str] = &[
    // 主源：vbskycn（每 6h 更新，频道少但央视/卫视线路实测最好，湖南卫视 3.1MB/s）
    "https://live.zbds.top/tv/iptv4.m3u",
    // vbskycn 备用镜像（主域名偶发被运营商污染）
    "https://gh-proxy.com/raw.githubusercontent.com/vbskycn/iptv/refs/heads/master/tv/iptv4.m3u",
    // 次源：suxuang/myIPTV（1273 频道，含咪咕/移动 IPTV 线路；单条质量差但能补备线）
    "https://raw.githubusercontent.com/suxuang/myIPTV/main/ipv4.m3u",
    // 补充：iptv-org 中国区（156 条，以境外/地方台为主，补冷门台）
    "https://iptv-org.github.io/iptv/countries/cn.m3u",
];
/// 缓存 TTL：6 小时（与主源更新节奏一致）。
const IPTV_TTL_SECS: u64 = 6 * 3600;
/// 缓存键（IPv6 源实测已彻底失效（只剩「更新时间」一条），不采）。
/// [lc-2102] v3：改为多源合并（每频道 urls 更多 + 记录 source 数），键升版强制重取。
const IPTV_CACHE_KEY: &str = "iptv:m3u:merged:v3";

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
    // [lc-2096] 同一频道**收集全部线路**（`urls` 数组），不再只留第一条 ——
    // 实测上游大量首条线路已死（CCTV8 43KB/s、CCTV5 切片 404），必须有多条可回退。
    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    // (group, name) → (group_idx, chan_idx)
    let mut at: HashMap<(String, String), (usize, usize)> = HashMap::new();
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
            // 上游把「更新时间」元数据也伪装成一个分组（内含一条无关 mp4）—— 不是频道，滤掉
            if group == "更新时间" {
                cur = None;
                continue;
            }
            // [lc-2096] 「支持作者」是作者的打赏广告位（tvg-id=izbds，URL 指向百度网盘的
            // 宣传 mp4），不是电视频道 —— 无论落在哪个分组都剔除。
            if name.contains("支持作者") || ext_attr(attrs, "tvg-id") == "izbds" {
                cur = None;
                continue;
            }
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
            if let Some(&(gi, ci)) = at.get(&key) {
                // 同频道再来一条 = 备用线路：追加进 urls（去重），并补首个非空台标
                let ch = &mut groups[gi].1[ci];
                if let Some(arr) = ch.get_mut("urls").and_then(|v| v.as_array_mut()) {
                    if !arr.iter().any(|x| x.as_str() == Some(line)) {
                        arr.push(json!(line));
                    }
                }
                if ch.get("logo").and_then(|v| v.as_str()).unwrap_or("").is_empty() && !logo.is_empty() {
                    ch["logo"] = json!(logo);
                }
                continue;
            }
            let entry = json!({ "name": name, "logo": logo, "url": line, "urls": [line] });
            let gi = match groups.iter().position(|(g, _)| *g == group) {
                Some(i) => i,
                None => {
                    groups.push((group.clone(), Vec::new()));
                    groups.len() - 1
                }
            };
            groups[gi].1.push(entry);
            at.insert(key, (gi, groups[gi].1.len() - 1));
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

/// 拉一个源并解析（失败返回 Err 供调用方记录）。
async fn fetch_one(src: &str) -> Result<Vec<Value>, String> {
    let resp = client()
        .get(src)
        .send()
        .await
        .map_err(|e| format!("{src}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{src}: HTTP {}", resp.status()));
    }
    let text = resp.text().await.map_err(|e| format!("{src}: {e}"))?;
    if !text.contains("#EXTINF") {
        return Err(format!("{src}: 返回内容不是 m3u"));
    }
    let groups = parse_m3u(&text);
    if groups.is_empty() {
        return Err(format!("{src}: 频道解析为空"));
    }
    Ok(groups)
}

/// 把多个源的解析结果合并成一份：按 (分组名, 频道名) 归并，线路全部并进 `urls`
/// （去重、保留顺序 —— 主源线路排在前，探活时优先命中最稳的那条）。
///
/// [lc-2102] 分组名做归一：不同源写法不一（「央视频道」vs「📺央视频道」vs「CCTV」），
/// 去 emoji/空白后同名的合并到同一组，否则用户会看到两三份「央视频道」。
fn merge_groups(mut acc: Vec<Value>, incoming: Vec<Value>) -> Vec<Value> {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation())
            .filter(|c| !matches!(*c, '\u{1F300}'..='\u{1FAFF}' | '\u{2600}'..='\u{27BF}'))
            .collect::<String>()
            .to_lowercase()
    };
    // 现有组的归一化名 → 组下标
    let mut at: HashMap<String, usize> = HashMap::new();
    for (i, g) in acc.iter().enumerate() {
        if let Some(n) = g.get("name").and_then(|v| v.as_str()) {
            at.insert(norm(n), i);
        }
    }
    for g in incoming {
        let gname = g.get("name").and_then(|v| v.as_str()).unwrap_or("其他").to_string();
        let gi = match at.get(&norm(&gname)) {
            Some(&i) => i,
            None => {
                acc.push(json!({ "name": gname, "channels": [] }));
                at.insert(norm(&gname), acc.len() - 1);
                acc.len() - 1
            }
        };
        let chans = g.get("channels").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let dst = acc[gi].get_mut("channels").and_then(|v| v.as_array_mut());
        let Some(dst) = dst else { continue };
        for ch in chans {
            let Some(name) = ch.get("name").and_then(|v| v.as_str()) else { continue };
            let new_urls: Vec<String> = ch
                .get("urls")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            // 频道判等：归一化同名（「CCTV-1」与「CCTV1」视为同一台）
            let existing = dst.iter_mut().find(|c| {
                c.get("name").and_then(|v| v.as_str()).map(norm).unwrap_or_default() == norm(name)
            });
            match existing {
                Some(cur) => {
                    if let Some(arr) = cur.get_mut("urls").and_then(|v| v.as_array_mut()) {
                        for u in new_urls {
                            if !arr.iter().any(|x| x.as_str() == Some(u.as_str())) {
                                arr.push(json!(u));
                            }
                        }
                    }
                    if cur.get("logo").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
                        if let Some(l) = ch.get("logo").and_then(|v| v.as_str()) {
                            if !l.is_empty() {
                                cur["logo"] = json!(l);
                            }
                        }
                    }
                }
                None => dst.push(ch),
            }
        }
    }
    acc
}

/// 发现页 IPTV 栏频道列表（多源合并，缓存 6h；全部失败回吐过期缓存）。
///
/// [lc-2102] 改为**并行拉全部源并合并**（原来是「第一个成功的源就用」）：单源线路质量
/// 参差，多源合并后每个频道能攒到 2-20 条备线，播放时 `iptv_play` 探活择优。
/// 任一源成功即返回（不因某个源挂掉就整体失败）。
#[tauri::command]
pub async fn iptv_channels_get() -> Result<Value, String> {
    if let Some(v) = crate::disk_cache::json_get(IPTV_CACHE_KEY, IPTV_TTL_SECS) {
        return Ok(v);
    }
    // 并行拉取（各源互不阻塞；总耗时 ≈ 最慢的那个）
    let mut set = tokio::task::JoinSet::new();
    for (i, src) in IPTV_SOURCES.iter().enumerate() {
        let src = src.to_string();
        set.spawn(async move { (i, fetch_one(&src).await) });
    }
    // 按源顺序收集（JoinSet 完成顺序不定，必须按索引排 —— 主源线路要排在最前）
    let mut got: Vec<(usize, Result<Vec<Value>, String>)> = Vec::new();
    while let Some(r) = set.join_next().await {
        match r {
            Ok(v) => got.push(v),
            Err(e) => crate::libmpv::trace(&format!("[iptv] 源任务失败: {e}")),
        }
    }
    got.sort_by_key(|(i, _)| *i);

    let mut errs = Vec::new();
    let mut merged: Vec<Value> = Vec::new();
    let mut source_count = 0usize;
    for (_, res) in got {
        match res {
            Ok(groups) => {
                source_count += 1;
                merged = merge_groups(merged, groups);
            }
            Err(e) => errs.push(e),
        }
    }
    if source_count > 0 && !merged.is_empty() {
        // 统计：频道总数 + 有多条备线的频道数（前端/日志用来判断合并效果）
        let total: usize = merged
            .iter()
            .map(|g| g.get("channels").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0))
            .sum();
        let multi = merged
            .iter()
            .flat_map(|g| g.get("channels").and_then(|v| v.as_array()).cloned().unwrap_or_default())
            .filter(|c| {
                c.get("urls").and_then(|v| v.as_array()).map(|a| a.len() > 1).unwrap_or(false)
            })
            .count();
        crate::libmpv::trace(&format!(
            "[iptv] 合并 {source_count} 个源：{total} 频道（{multi} 个有多条备线）"
        ));
        let v = json!({
            "updated": now_ms(),
            "groups": merged,
            "sources": source_count,
            "multiLine": multi,
        });
        crate::disk_cache::json_put(IPTV_CACHE_KEY, &v);
        return Ok(v);
    }
    // 全部失败：过期缓存也回吐（标记 stale 让前端可以提示「数据不是最新的」）。
    if let Some(mut v) = crate::disk_cache::json_get(IPTV_CACHE_KEY, 0) {
        if let Some(o) = v.as_object_mut() {
            o.insert("stale".into(), json!(true));
        }
        return Ok(v);
    }
    Err(format!("直播源拉取失败：{}", errs.join("；")))
}

/// 单条线路探活 + 测速：拉 HLS manifest + 抓第一个分片 128KB，返回吞吐（字节/秒）。
/// 0 = 不可用（manifest 失败 / 无分片 / 分片非 2xx）。
///
/// [lc-2102] 从「能不能播」升级为「**跑多快**」：合并多源后一个频道常有 2～20 条备线，
/// 只挑「第一条能通」可能挑到 43KB/s 的死链（实测 CCTV8 就是），必须按实测速度择优。
/// 直播至少需要 ~300KB/s（1080p 约 2-4Mbps），低于阈值的线路不选。
async fn probe_speed(url: &str) -> u64 {
    let c = client();
    let t0 = std::time::Instant::now();
    let resp = match c.get(url).timeout(std::time::Duration::from_secs(6)).send().await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    if !resp.status().is_success() {
        return 0;
    }
    let text = match resp.text().await {
        Ok(t) => t,
        Err(_) => return 0,
    };
    if !text.contains("#EXT") {
        // 非 HLS（直接是 ts/flv/mp4 流）：manifest 就是流本体，能连上即可用
        return u64::MAX / 2;
    }
    // HLS：按 base 拼第一个分片抓 128KB 测真实吞吐。
    let base = match url.rfind('/') {
        Some(i) => &url[..=i],
        None => return 0,
    };
    let seg = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'));
    let Some(seg) = seg else { return 0 };
    let full = if seg.starts_with("http") {
        seg.to_string()
    } else {
        format!("{base}{seg}")
    };
    let t1 = std::time::Instant::now();
    let seg_resp = match c
        .get(&full)
        .timeout(std::time::Duration::from_secs(8))
        .header("Range", "bytes=0-131071")
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return 0,
    };
    if !seg_resp.status().is_success() {
        return 0;
    }
    let bytes = match seg_resp.bytes().await {
        Ok(b) => b.len() as u64,
        Err(_) => return 0,
    };
    if bytes == 0 {
        return 0;
    }
    let secs = t1.elapsed().as_secs_f64().max(0.001);
    let _ = t0;
    (bytes as f64 / secs) as u64
}

/// 直播线路最低可用吞吐（字节/秒）：1080p 直播约 2-4Mbps，取 300KB/s 作下限。
/// 低于它的线路（实测有 43KB/s 的）选它等于让用户看幻灯片，不如继续找下一条。
const MIN_LIVE_BPS: u64 = 300 * 1024;

/// 播放 IPTV 频道：从候选线路里挑**实测最快且够用**的一条交给播放器。
/// 直链原样交 `player::launch`（direct=true 跳过 Go 代理，与飞牛直播同口径）；
/// 不落进度、不记观影历史（直播无进度概念）。
///
/// [lc-2102] `urls` = 该频道全部线路（多源合并后可达数十条）。做法：
///   · 并行探**前 8 条**（每条各自 manifest 6s + 分片 8s 超时封顶，并行不叠加）；
///   · 取吞吐最高者；全部低于 `MIN_LIVE_BPS` 时取相对最快的那条（有总比没有强）；
///   · 全都探活失败 → 退回用户点的那条，让 mpv 自己再试一次（不阻断）。
#[tauri::command]
pub async fn iptv_play(
    state: tauri::State<'_, crate::AppState>,
    app: tauri::AppHandle,
    title: String,
    url: String,
    urls: Option<Vec<String>>,
) -> Result<(), String> {
    let cfg = state.config.read().await.clone();
    crate::fnos_progress::begin("", "");
    // 候选线路：urls 非空则用它（含点中的那条），否则只有 url。
    let mut cands: Vec<String> = urls.unwrap_or_default();
    if !cands.iter().any(|u| u == &url) {
        cands.insert(0, url.clone());
    }
    cands.retain(|u| !u.trim().is_empty());
    if cands.is_empty() {
        return Err("频道地址为空".into());
    }
    let mut picked = cands[0].clone();
    if cands.len() > 1 {
        // 探活上限：太多线路并行会把本地带宽自己打满（反而全都测成慢的）。
        const PROBE_MAX: usize = 8;
        let mut set = tokio::task::JoinSet::new();
        for (i, u) in cands.iter().take(PROBE_MAX).enumerate() {
            let u = u.clone();
            set.spawn(async move { (i, probe_speed(&u).await) });
        }
        let mut best: Option<(usize, u64)> = None;
        while let Some(r) = set.join_next().await {
            if let Ok((i, bps)) = r {
                if bps == 0 {
                    continue;
                }
                match best {
                    Some((_, b)) if b >= bps => {}
                    _ => best = Some((i, bps)),
                }
            }
        }
        match best {
            Some((i, bps)) if bps >= MIN_LIVE_BPS => {
                picked = cands[i].clone();
                crate::libmpv::trace(&format!(
                    "[iptv] 优选线路 #{i}（{}KB/s）: {}",
                    bps / 1024,
                    cands[i]
                ));
            }
            Some((i, bps)) => {
                // 有能用但都偏慢：仍选最快的（比死链强），日志留证
                picked = cands[i].clone();
                crate::libmpv::trace(&format!(
                    "[iptv] 全部线路偏慢，取最快 #{i}（{}KB/s）: {}",
                    bps / 1024,
                    cands[i]
                ));
            }
            None => {
                crate::libmpv::trace("[iptv] 全部线路探活失败，回退首条交 mpv 再试");
            }
        }
    }
    crate::player::launch(
        &app, &state, &cfg, &picked, title.trim(), 0.0, 0.0, None, &[], None, true,
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
            "#EXTINF:-1 tvg-name=\"2026-11-08 18:26\" tvg-id=\"更新时间\" group-title=\"更新时间\", 2026-11-08 18:26\nhttp://junk/x.mp4\n",
            "#EXTVLCOPT:http-user-agent=ok\n",
        );
        let groups = parse_m3u(m3u);
        assert_eq!(groups.len(), 3); // 央视 / 卫视 / 其他（无 group-title 兜底；「更新时间」垃圾组已滤）
        assert_eq!(groups[0]["name"], "央视频道");
        let chs = groups[0]["channels"].as_array().unwrap();
        assert_eq!(chs.len(), 1, "同组同名合并为一个频道");
        assert_eq!(chs[0]["url"], "http://a/1.m3u8");
        // [lc-2096] 同频道第二条 EXTINF = 备用线路，收进 urls
        let urls = chs[0]["urls"].as_array().unwrap();
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[1], "http://a/2.m3u8");
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

    /// [lc-2102] 多源合并：分组名归一（emoji/空白差异视为同组）、频道名归一
    /// （CCTV-1 与 CCTV1 视为同台）、线路并进 urls 去重、台标互补。
    #[test]
    fn merge_groups_normalizes_and_merges_lines() {
        let a = parse_m3u(concat!(
            "#EXTINF:-1 tvg-id=\"c1\" tvg-logo=\"http://l/a.png\" group-title=\"📺央视频道\",CCTV-1\nhttp://a/1.m3u8\n",
            "#EXTINF:-1 group-title=\"📺央视频道\",湖南卫视\nhttp://a/hn.m3u8\n",
        ));
        let b = parse_m3u(concat!(
            "#EXTINF:-1 tvg-logo=\"http://l/b.png\" group-title=\"央视频道\",CCTV1\nhttp://b/1.m3u8\n",
            "#EXTINF:-1 group-title=\"央视频道\",CCTV1\nhttp://b/2.m3u8\n",
            "#EXTINF:-1 group-title=\"卫视频道\",浙江卫视\nhttp://b/zj.m3u8\n",
        ));
        let m = merge_groups(a, b);
        assert_eq!(m.len(), 2, "两个分组：央视（归一合并）+ 卫视");
        assert_eq!(m[0]["name"], "📺央视频道", "沿用主源的组名写法");
        let cctv = m[0]["channels"].as_array().unwrap();
        assert_eq!(cctv.len(), 2, "CCTV-1 与 CCTV1 归一为同一台");
        let c1 = cctv
            .iter()
            .find(|c| c["name"].as_str().unwrap().contains("CCTV"))
            .unwrap();
        let urls = c1["urls"].as_array().unwrap();
        assert_eq!(urls.len(), 3, "主源 1 条 + 次源 2 条全并进来");
        assert_eq!(urls[0], "http://a/1.m3u8", "主源线路排最前（探活优先）");
        assert_eq!(c1["logo"], "http://l/a.png", "已有台标不被覆盖");
        assert_eq!(cctv.len(), 2);
        assert!(cctv.iter().any(|c| c["name"] == "湖南卫视"));
        assert_eq!(m[1]["channels"][0]["name"], "浙江卫视");
    }

    /// [lc-2102] 台标互补：主源无图、次源有图 → 合并后取次源的图。
    #[test]
    fn merge_groups_fills_missing_logo() {
        let a = parse_m3u("#EXTINF:-1 group-title=\"G\",台A\nhttp://a/1.m3u8\n");
        let b = parse_m3u(
            "#EXTINF:-1 tvg-logo=\"http://l/ok.png\" group-title=\"G\",台A\nhttp://b/1.m3u8\n",
        );
        assert_eq!(a[0]["channels"][0]["logo"], "");
        let m = merge_groups(a, b);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0]["channels"][0]["logo"], "http://l/ok.png");
    }
}
