//! [lc-2190] 发现页 IPTV 栏 —— 直播源改为自托管聚合项目 **akiralereal/iptv**（GPL-3.0）。
//!
//! 该项目部署在 NAS / 服务器上（Docker 镜像 `akiralereal/iptv`，默认 1905 端口），把内置抓取
//! 模块（央视/卫视/咪咕/地方台/体育/B站/抖音/斗鱼/虎牙…）、订阅源与自定义直链聚合成一份 M3U
//! （实测 1100+ 频道、68 个分组，`#EXTINF` 自带 tvg-logo 台标与 EPG 地址）。我们只做
//! 「取 + 解析 + 播」：
//!   · 地址：设置面板「通用 → 发现页直播源」自定义（每行一条、最多 4 条，可填部署根地址或
//!     完整 M3U 地址）；留空使用内置公共聚合源（`DEFAULT_SOURCES`，**不含任何个人服务器地址**）。
//!     上游支持标准版 302 直连、`?relay=1` 兼容版、`?relay=2` 全代理与 `?profile=xxx` 多档。
//!   · 缓存：disk_cache JSON 层，TTL 6h；**缓存键按源集合哈希**（改源不会读到别的源旧数据），
//!     全部失败时回吐同一源集合的过期缓存（旧数据比报错强）。
//!   · 播放：频道 URL 原样交 `player::launch`（`direct=true` 跳过 Go 代理 —— 与飞牛直播
//!     直链同一口径：`playvideo` 代理对直播不可用，见 lib.rs [live] 注释），不落进度、不记观影历史。
//!
//! GPL-3.0 合规：数据来源已在发现页 IPTV 栏底部注明（前端）+ 此处注释；只消费部署实例的
//! 播放列表、不分发其代码。参考 README：https://github.com/akiralereal/iptv
//!
//! NOT IMPLEMENTED（后续接入）：EPG 节目单（m3u 头部 x-tvg-url 有现成地址）、回看（catchup 参数）。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

/// [lc-2191] 内置默认播放列表：公开聚合源（**不含任何个人服务器地址**，避免把部署方
/// 的内网/隧道地址随代码公开）。
/// 单源线路质量参差（实测同一份数据里 12.7MB/s 与 43KB/s 并存、大量分片 404），所以默认
/// 全拉合并、播放时探活择优；用户可在设置里换成自己的自托管 iPTV 部署（只存本机 config）。
pub const DEFAULT_SOURCES: &[&str] = &[
    // 主源：vbskycn（每 6h 更新，频道少但央视/卫视线路实测最好）
    "https://live.zbds.top/tv/iptv4.m3u",
    // vbskycn 备用镜像（主域名偶发被运营商污染）
    "https://gh-proxy.com/raw.githubusercontent.com/vbskycn/iptv/refs/heads/master/tv/iptv4.m3u",
    // 次源：suxuang/myIPTV（1273 频道，含咪咕/移动 IPTV 线路；单条质量差但能补备线）
    "https://raw.githubusercontent.com/suxuang/myIPTV/main/ipv4.m3u",
    // 补充：iptv-org 中国区（156 条，以境外/地方台为主，补冷门台）
    "https://iptv-org.github.io/iptv/countries/cn.m3u",
];
/// 自定义源上限（与弹幕接口同一口径：多源只做故障转移/并线，不做无限堆积）。
const MAX_SOURCES: usize = 4;
/// 缓存 TTL：6 小时（上游默认更新间隔）。
const IPTV_TTL_SECS: u64 = 6 * 3600;
/// 缓存键前缀（v4：数据源改为可配置的自托管 iPTV；键尾拼源集合哈希）。
const IPTV_CACHE_PREFIX: &str = "iptv:m3u:v4";

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

/// [lc-2190] 归一化单条 iPTV 地址（设置面板 / 手工改 config 都走这里）：
///   · 没有协议头 → 补 `http://`（家里 NAS 端口直填最省事）；
///   · 路径已是 `.m3u` / `.m3u8` → 原样保留（`?relay=2`、`?profile=tv` 等查询串不动）；
///   · 路径以 `interface` 结尾（少打后缀）→ 补 `.m3u`；
///   · 路径以 `/txt` 结尾（TXT 订阅入口）→ 换成 `/interface.m3u`（解析器只认 M3U）；
///   · 其余（裸主机 / 根路径 / 密码路径）→ 视为部署根地址，补 `/interface.m3u`。
///
/// 只接受 http(s)；非法地址返回 Err —— 设置面板立即报错，不静默存坏值。
pub fn normalize_source(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("地址为空".into());
    }
    let with_scheme = if s.contains("://") {
        s.to_string()
    } else {
        format!("http://{s}")
    };
    let mut url = reqwest::Url::parse(&with_scheme).map_err(|e| format!("{s}: 地址无效（{e}）"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("{s}: 只支持 http/https"));
    }
    if url.host_str().unwrap_or("").is_empty() {
        return Err(format!("{s}: 缺少主机名"));
    }
    let path = url.path().trim_end_matches('/').to_string();
    let low = path.to_ascii_lowercase();
    if low.ends_with(".m3u") || low.ends_with(".m3u8") {
        return Ok(url.to_string());
    }
    let tail = low.rsplit('/').next().unwrap_or("");
    if tail == "interface" {
        url.set_path(&format!("{path}.m3u"));
        return Ok(url.to_string());
    }
    if tail == "txt" {
        // `/txt` 与 `/interface.m3u` 是同一份数据的两种出口，换成 M3U
        let base = path[..path.len() - 3].trim_end_matches('/');
        url.set_path(&format!("{base}/interface.m3u"));
        return Ok(url.to_string());
    }
    url.set_path(&format!("{path}/interface.m3u"));
    Ok(url.to_string())
}

/// [lc-2190] 归一化地址列表：逐行解析、去重、上限 4 条。
/// 每项内部也按行拆分（前端按行发，config 手工编辑时可能塞成一整串）。
pub fn normalize_sources(list: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in list {
        for line in raw.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            let n = normalize_source(t)?;
            if !out.iter().any(|x| x == &n) {
                out.push(n);
            }
        }
    }
    out.truncate(MAX_SOURCES);
    Ok(out)
}

/// 内置默认源列表（给设置面板回显/占位用；`DEFAULT_SOURCES` 的 owned 形式）。
pub fn default_sources() -> Vec<String> {
    DEFAULT_SOURCES.iter().map(|s| s.to_string()).collect()
}

/// 当前生效的源集合 → `(sources, custom)`：
///   · 配置为空 → 内置公共聚合源（custom=false）；
///   · 否则用归一化后的自定义列表（custom=true）。老 config / 手工编辑的裸地址在这里
///     兜底归一化；**单条坏地址只跳过它**，不连坐整个列表。
pub fn resolve_sources(cfg: &crate::config::Config) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    for raw in &cfg.iptv_sources {
        for line in raw.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(n) = normalize_source(t) {
                if !out.iter().any(|x| x == &n) {
                    out.push(n);
                }
            }
        }
    }
    out.truncate(MAX_SOURCES);
    if out.is_empty() {
        (default_sources(), false)
    } else {
        (out, true)
    }
}

/// FNV-1a（跨进程稳定，不能为了哈希缓存键引入随机种子）。
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 缓存键 = 前缀 + 源集合哈希：换源（含 relay/profile/密码路径变化）即换键。
fn cache_key(sources: &[String]) -> String {
    format!("{IPTV_CACHE_PREFIX}:{:016x}", fnv1a(&sources.join("\n")))
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
            // 上游把「更新时间」元数据、以及部署方自己的「公告 / 教程」短视频都伪装成频道分组 ——
            // 不是电视频道，整组滤掉（akiralereal/iptv 的公告组里是入群/登录教程 mp4）。
            if group == "更新时间" || group == "公告" {
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

/// 发现页 IPTV 栏频道列表（多源合并，缓存 6h；全部失败回吐同一源集合的过期缓存）。
///
/// [lc-2190] 源集合来自 config（设置面板「通用 → 发现页直播源」，空 = 内置默认）；
/// 缓存键按源集合哈希，换源不会读到旧源的缓存。
/// [lc-2102] 仍保留**并行拉全部源并合并**：多源合并后每个频道能攒到多条备线，
/// 播放时 `iptv_play` 探活择优；任一源成功即返回（不因某个源挂掉就整体失败）。
#[tauri::command]
pub async fn iptv_channels_get(state: tauri::State<'_, crate::AppState>) -> Result<Value, String> {
    // [lc-2190] 源集合来自配置（设置面板可自定义；空 = 内置公共聚合源）
    let (sources, custom) = {
        let cfg = state.config.read().await;
        resolve_sources(&cfg)
    };
    let key = cache_key(&sources);
    if let Some(v) = crate::disk_cache::json_get(&key, IPTV_TTL_SECS) {
        return Ok(v);
    }
    // 并行拉取（各源互不阻塞；总耗时 ≈ 最慢的那个）
    let mut set = tokio::task::JoinSet::new();
    for (i, src) in sources.iter().enumerate() {
        let src = src.clone();
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
            "custom": custom,
        });
        crate::disk_cache::json_put(&key, &v);
        return Ok(v);
    }
    // 全部失败：同一源集合的过期缓存也回吐（标记 stale 让前端提示「数据不是最新的」）。
    if let Some(mut v) = crate::disk_cache::json_get(&key, 0) {
        if let Some(o) = v.as_object_mut() {
            o.insert("stale".into(), json!(true));
            o.insert("custom".into(), json!(custom));
        }
        return Ok(v);
    }
    Err(format!(
        "直播源拉取失败：{}",
        if errs.is_empty() {
            "没有可用的源".to_string()
        } else {
            errs.join("；")
        }
    ))
}

/// [lc-2190] 设置面板「发现页直播源」的连通测试：按当前输入（空 = 内置默认）并行拉取，
/// 汇总可用源数与频道/分组数；不写缓存。逐条失败原因原样带回，便于用户改地址。
#[tauri::command]
pub async fn iptv_sources_test(sources: Vec<String>) -> Result<String, String> {
    let mut list = normalize_sources(&sources)?;
    if list.is_empty() {
        list = default_sources();
    }
    let mut set = tokio::task::JoinSet::new();
    for (i, src) in list.iter().enumerate() {
        let src = src.clone();
        set.spawn(async move { (i, fetch_one(&src).await) });
    }
    let mut got: Vec<(usize, Result<Vec<Value>, String>)> = Vec::new();
    while let Some(r) = set.join_next().await {
        match r {
            Ok(v) => got.push(v),
            Err(e) => return Err(format!("测试任务失败：{e}")),
        }
    }
    got.sort_by_key(|(i, _)| *i);
    let mut ok_count = 0usize;
    let mut total_chans = 0usize;
    let mut group_count = 0usize;
    let mut errs = Vec::new();
    for (_, res) in got {
        match res {
            Ok(groups) => {
                ok_count += 1;
                group_count += groups.len();
                total_chans += groups
                    .iter()
                    .map(|g| {
                        g.get("channels")
                            .and_then(|v| v.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0)
                    })
                    .sum::<usize>();
            }
            Err(e) => errs.push(e),
        }
    }
    if ok_count == 0 {
        return Err(format!("全部源拉取失败：{}", errs.join("；")));
    }
    let mut msg = format!("{ok_count} 个源可用：{total_chans} 个频道，{group_count} 个分组");
    if !errs.is_empty() {
        msg.push_str(&format!("（{} 个源失败：{}）", errs.len(), errs.join("；")));
    }
    Ok(msg)
}

/// 单条线路探活 + 测速：拉 HLS manifest + 抓第一个分片 128KB，返回吞吐（字节/秒）。
/// 0 = 不可用（manifest 失败 / 无分片 / 分片非 2xx）。
///
/// [lc-2102] 从「能不能播」升级为「**跑多快**」：合并多源后一个频道常有 2～20 条备线，
/// 只挑「第一条能通」可能挑到 43KB/s 的死链（实测 CCTV8 就是），必须按实测速度择优。
/// 直播至少需要 ~300KB/s（1080p 约 2-4Mbps），低于阈值的线路不选。
async fn probe_speed(url: &str) -> u64 {
    let c = client();
    let resp = match c.get(url).timeout(std::time::Duration::from_secs(6)).send().await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    if !resp.status().is_success() {
        return 0;
    }
    // [lc-2190] akiralereal/iptv 标准版是「网关 302 → 官方 CDN」：清单里的相对分片必须
    // 以**跳转后的最终地址**为 base 解析；用原网关地址拼会 404，整批线路会被误判不可用。
    let final_url = resp.url().clone();
    let text = match resp.text().await {
        Ok(t) => t,
        Err(_) => return 0,
    };
    if !text.contains("#EXT") {
        // 非 HLS（直接是 ts/flv/mp4 流）：manifest 就是流本体，能连上即可用。
        // 但 302 到 HTML 错误页（网关鉴权失败 / 源已失效）不能当可用。
        let head = text.trim_start().to_ascii_lowercase();
        if head.starts_with("<!doctype") || head.starts_with("<html") {
            return 0;
        }
        return u64::MAX / 2;
    }
    // HLS：取清单里第一个分片地址，按最终地址解析（相对 / 根路径 / 绝对地址都吃）。
    let seg = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'));
    let Some(seg) = seg else { return 0 };
    let Ok(full) = final_url.join(seg) else { return 0 };
    let t1 = std::time::Instant::now();
    let seg_resp = match c
        .get(full.as_str())
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

    /// [lc-2190] 公告组（部署方的教程/入群短视频）不是电视频道，整组滤掉。
    #[test]
    fn parse_m3u_drops_announcement_group() {
        let m3u = concat!(
            "#EXTINF:-1 tvg-id=\"iptv-announcement\" group-title=\"公告\",欢迎使用 iPTV\n",
            "http://h:1905/assets/announcement.mp4\n",
            "#EXTINF:-1 group-title=\"央视\",CCTV1\nhttp://h:1905/1\n",
        );
        let groups = parse_m3u(m3u);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["name"], "央视");
        assert_eq!(groups[0]["channels"].as_array().unwrap().len(), 1);
    }

    /// [lc-2190] 地址归一化：裸主机补协议、根地址补 /interface.m3u、完整地址/查询串不动、
    /// /txt 换 M3U、密码路径保留、非 http(s) 报错。
    #[test]
    fn normalize_source_accepts_deploy_forms() {
        assert_eq!(
            normalize_source("192.168.1.10:1905").unwrap(),
            "http://192.168.1.10:1905/interface.m3u"
        );
        assert_eq!(
            normalize_source("http://h:1905/").unwrap(),
            "http://h:1905/interface.m3u"
        );
        assert_eq!(
            normalize_source("http://h:1905/pass").unwrap(),
            "http://h:1905/pass/interface.m3u"
        );
        assert_eq!(
            normalize_source("http://h:1905/interface.m3u?relay=2&profile=tv").unwrap(),
            "http://h:1905/interface.m3u?relay=2&profile=tv"
        );
        assert_eq!(
            normalize_source("http://h:1905/interface").unwrap(),
            "http://h:1905/interface.m3u"
        );
        assert_eq!(
            normalize_source("http://h:1905/txt").unwrap(),
            "http://h:1905/interface.m3u"
        );
        assert_eq!(
            normalize_source("http://h:1905/pass/txt").unwrap(),
            "http://h:1905/pass/interface.m3u"
        );
        assert!(normalize_source("ftp://h/x").is_err());
        assert!(normalize_source("").is_err());
    }

    /// [lc-2190] 多源归一化：逐行拆分、去重、上限 4 条；空列表返回空（由调用方补默认）。
    #[test]
    fn normalize_sources_splits_dedups_and_caps() {
        let list = vec![
            "http://a:1905\nhttp://b:1905\n\n".to_string(),
            "http://a:1905/interface.m3u".to_string(),
        ];
        let out = normalize_sources(&list).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], "http://a:1905/interface.m3u");
        assert_eq!(out[1], "http://b:1905/interface.m3u");
        assert!(normalize_sources(&[]).unwrap().is_empty());

        let many: Vec<String> = (1..=6).map(|i| format!("http://h{i}:1905")).collect();
        assert_eq!(normalize_sources(&many).unwrap().len(), MAX_SOURCES);
    }

    /// [lc-2190] 缓存键按源集合区分：同集合稳定、改源/改参数必换键。
    #[test]
    fn cache_key_depends_on_source_set() {
        let a = vec!["http://h:1905/interface.m3u".to_string()];
        let b = vec!["http://h:1905/interface.m3u?relay=2".to_string()];
        let a2 = a.clone();
        assert_eq!(cache_key(&a), cache_key(&a2));
        assert_ne!(cache_key(&a), cache_key(&b));
        assert!(cache_key(&a).starts_with(IPTV_CACHE_PREFIX));
    }

    /// [lc-2190/lc-2191] 生效源集合：空配置 → 内置公共聚合源；单条坏地址只跳过它，
    /// 其余照常生效（并标记为自定义源）。
    #[test]
    fn resolve_sources_falls_back_and_skips_bad_entries() {
        let mut cfg = crate::config::Config::default();
        let (def, custom) = resolve_sources(&cfg);
        assert_eq!(def, default_sources());
        assert!(!custom, "空配置 = 内置源");
        cfg.iptv_sources = vec![
            "ftp://bad/x".to_string(),
            "192.168.1.10:1905".to_string(),
        ];
        let (list, custom) = resolve_sources(&cfg);
        assert_eq!(list, vec!["http://192.168.1.10:1905/interface.m3u".to_string()]);
        assert!(custom);
    }

    /// [lc-2190] 真实部署联调（设 `ANIBOX_IPTV_TEST_SOURCE` 指向 M3U 地址；默认跳过，手动运行）：
    /// 拉取 + 解析非空，且线路探活可用（验证 302 网关的最终地址解析）。
    /// 运行：`ANIBOX_IPTV_TEST_SOURCE=http://…/interface.m3u cargo test --lib real_source -- --ignored --nocapture`
    #[test]
    #[ignore = "需要 ANIBOX_IPTV_TEST_SOURCE 指向可访问的 M3U"]
    fn real_source_parses() {
        let Ok(src) = std::env::var("ANIBOX_IPTV_TEST_SOURCE") else {
            println!("[lc-2190] 未设置 ANIBOX_IPTV_TEST_SOURCE，跳过");
            return;
        };
        let src = match normalize_source(&src) {
            Ok(s) => s,
            Err(e) => {
                println!("[lc-2190] 测试地址无效：{e}");
                return;
            }
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let groups = fetch_one(&src).await.expect("真实源应拉取成功");
            let total: usize = groups
                .iter()
                .map(|g| {
                    g.get("channels")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0)
                })
                .sum();
            println!("[lc-2190] {src}：{} 分组 / {total} 频道", groups.len());
            assert!(total > 100, "频道数过少：{total}");
            assert!(groups.len() > 3, "分组数过少：{}", groups.len());
            // 302 网关线路探活（用清单里第一条频道地址）。
            let first = groups
                .iter()
                .filter_map(|g| g.get("channels").and_then(|v| v.as_array()))
                .flatten()
                .filter_map(|c| c.get("url").and_then(|v| v.as_str()))
                .find(|u| u.starts_with("http"))
                .expect("至少有一条频道地址")
                .to_string();
            let bps = probe_speed(&first).await;
            println!("[lc-2190] 探活 {first} → {}KB/s", bps / 1024);
            assert!(bps > 0, "302 网关线路探活失败：{first}");
        });
    }
}
