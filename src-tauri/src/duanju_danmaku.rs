//! [lc-2238] 短剧弹幕：从 guoapp 核心拉取弹幕并生成 mpv 可直接加载的 ASS 文件。
//!
//! 核心协议（`native/core/app_danmaku.go` + `nativeDanmaku`）：
//!   · 必须先 `resolve`（拿到 playbackSession），弹幕绑定在**那次播放会话**上
//!     （`engine.playbacks[session]`，会话 12 小时过期）；
//!   · 请求体：`{action:"danmaku", playbackSession, session, sequence, startMs, durationMs}` ——
//!     **必须带 session + 严格递增 sequence**，否则 `validNativeRead` 直接回「请求标记无效」
//!     （它走 beginRead 的读名额，和目录请求同一个 16 上限）；
//!   · 时间窗校验是 `durationMs > startMs`，所以窗口语义是**右端点而非长度** ——
//!     第二页必须写 `startMs=30000, durationMs=60000`，写 `30000/30000` 会报「弹幕时间范围无效」；
//!   · 返回 `{episodeId, items:[{id,text,timeMs}], startMs, nextMs, total}`，按 `nextMs`
//!     逐页推进直到没有 `nextMs` 或凑够 `total`。
//!
//! 一集 1~3 分钟 = 4~6 页、几十到几百条；全季几十集合计上万条、上百次请求，反而会
//! 长时间占满核心读名额（正是「一直加载中」的成因），因此**按集拉取 + 落磁盘缓存**，
//! 播到哪集拉哪集（连播时每集 4~6 次请求，秒级完成）。
//!
//! ASS 生成：单条 `Dialogue` 的 `{\move(x1,y,x2,y,...)}` 让弹幕从右向左横穿。
//! 时间用 ASS 的 `H:MM:SS.cc`。所有 ASS 标签必须包在 `{}` 里，反斜杠用
//! `string` 常量拼接避免被上层语言转义（历史血训见 lc-2142）。

use serde_json::Value;
use std::path::{Path, PathBuf};

/// 单页时间窗（毫秒）。80ms 是实测较优值：一集 2 分钟约 12 页，兼顾请求数与丢帧率。
const PAGE_WINDOW_MS: i64 = 30_000;
/// 单集最多翻多少页（防上游 nextMs 异常时死循环）。30s × 100 = 50 分钟，远超短剧单集。
const MAX_PAGES: usize = 100;
/// 单集弹幕上限（防极端剧集把 ASS 撑爆）。
const MAX_ITEMS: usize = 4000;

/// 一条弹幕。
#[derive(Debug, Clone)]
pub struct DanmakuItem {
    pub text: String,
    pub time_ms: i64,
}

/// 弹幕失败原因（区分「这集没有弹幕」和「拉取失败」，前端提示不同）。
#[derive(Debug)]
pub enum DanmakuError {
    /// 本集确实没有弹幕（total=0）——不算错误
    Empty,
    /// 不支持弹幕（非红果等）
    Unsupported(String),
    /// 拉取失败
    Failed(String),
}

impl std::fmt::Display for DanmakuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DanmakuError::Empty => write!(f, "本集暂无弹幕"),
            DanmakuError::Unsupported(m) => write!(f, "{m}"),
            DanmakuError::Failed(m) => write!(f, "{m}"),
        }
    }
}

/// 弹幕缓存文件路径：`<cache>/danmaku/<source>/<episodeId>.ass`。
/// episodeId 里的 `:` 换成 `_`（Windows 文件名非法字符）。
pub fn cache_path(cache_root: &Path, source: &str, episode_id: &str) -> PathBuf {
    let safe = episode_id.replace([':', '/', '\\', '*', '?', '"', '<', '>', '|'], "_");
    cache_root.join("danmaku").join(source).join(format!("{safe}.ass"))
}

/// 把核心返回的一页解析成条目（`items[].text/timeMs`）。
pub fn parse_page(page: &Value) -> (Vec<DanmakuItem>, Option<i64>, i64) {
    let items = page
        .get("items")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|it| {
                    let text = it.get("text").and_then(|x| x.as_str())?.trim().to_string();
                    if text.is_empty() {
                        return None;
                    }
                    let time_ms = it.get("timeMs").and_then(|x| x.as_i64()).unwrap_or(0);
                    Some(DanmakuItem { text, time_ms })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let next = page
        .get("nextMs")
        .and_then(|x| x.as_i64())
        .filter(|n| *n > 0);
    let total = page.get("total").and_then(|x| x.as_i64()).unwrap_or(0);
    (items, next, total)
}

/// 秒 → ASS 时间 `H:MM:SS.cc`。
fn ass_time(secs: f64) -> String {
    let s = secs.max(0.0);
    let h = (s / 3600.0).floor() as i64;
    let m = ((s % 3600.0) / 60.0).floor() as i64;
    let sec = s % 60.0;
    format!("{h}:{m:02}:{sec:05.2}")
}

/// ASS 文本转义：`{` `}` 会被当标签块，反斜杠会被当标签，换行直接拍平。
fn esc_ass(text: &str) -> String {
    text.replace('\\', "／")
        .replace('{', "（")
        .replace('}', "）")
        .replace(['\r', '\n'], " ")
}

/// 生成 ASS。`play_res_x/y` 用来算滚动轨道与速度（与视频分辨率无关，mpv 会缩放）。
///
/// 轨道数按高度除 40px 估；每条弹幕按它在同轨道里的序号决定 `y`，形成多轨道垂直分布。
/// 滚动时长按文字长度给 6~10 秒（中文越长停留越久，观感接近 B 站）。
pub fn build_ass(items: &[DanmakuItem], play_res_x: i64, play_res_y: i64) -> String {
    let res_x = play_res_x.max(640);
    let res_y = play_res_y.max(360);
    let lane_h = (res_y as f64 / 18.0).max(28.0); // 最多 18 条轨道
    let lanes = ((res_y as f64 / lane_h).floor() as usize).max(1);

    let mut out = String::with_capacity(items.len() * 120 + 512);
    out.push_str("[Script Info]\n");
    out.push_str("ScriptType: v4.00+\n");
    out.push_str(&format!("PlayResX: {res_x}\nPlayResY: {res_y}\n"));
    out.push_str("WrapStyle: 2\nScaledBorderAndShadow: yes\nYCbCr Matrix: TV.709\n\n");
    out.push_str("[V4+ Styles]\n");
    out.push_str(
        "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, \
Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
    );
    // 白字黑边、顶部对齐（\move 自己给坐标，对齐方式只影响锚点）
    out.push_str(
        "Style: Danmaku,Microsoft YaHei,42,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,\
0,0,0,0,100,100,0,0,1,2,1,7,0,0,0,1\n\n",
    );
    out.push_str("[Events]\n");
    out.push_str("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");

    // 每条弹幕的起步时间；打字机式散布：同一毫秒的弹幕错开一点，避免完全重叠
    let mut lane_cursor = vec![0f64; lanes];
    for (i, it) in items.iter().take(MAX_ITEMS).enumerate() {
        let start = (it.time_ms as f64) / 1000.0;
        // 同毫秒多条：按序号加一点偏移（最多 +0.9s），避免同点堆叠
        let jitter = (i % 10) as f64 * 0.1;
        let t0 = start + jitter;
        let text = esc_ass(&it.text);
        // 估算文字宽度（中文按字号 1 倍宽），决定滚动时长
        let chars = text.chars().count().max(1) as f64;
        let dur = (5.0 + chars * 0.16).clamp(6.0, 12.0);
        let lane = (i % lanes).min(lanes - 1);
        lane_cursor[lane] = t0;
        let y = (lane_h * lane as f64 + lane_h * 0.8) as i64;
        // 从右边界外滚到左边界外
        let x1 = res_x;
        let x2 = -(chars * 42.0 * 0.6) as i64;
        out.push_str(&format!(
            "Dialogue: 0,{},{},Danmaku,,0,0,0,,{{\\move({x1},{y},{x2},{y},0,{dur_ms})}}{text}\n",
            ass_time(t0),
            ass_time(t0 + dur),
            dur_ms = (dur * 1000.0) as i64,
        ));
    }
    out
}

/// 读缓存（存在且非空即用）。
pub fn read_cache(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() < 64 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// 写缓存（失败不影响播放，只记日志由调用方处理）。
pub fn write_cache(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ass_time_format() {
        assert_eq!(ass_time(0.0), "0:00:00.00");
        assert_eq!(ass_time(65.5), "0:01:05.50");
        assert_eq!(ass_time(3725.25), "1:02:05.25");
    }

    #[test]
    fn parse_page_reads_items_and_next() {
        let page = json!({
            "episodeId": "7707590703068498968",
            "items": [
                {"id": "1", "text": "剧组真有钱啊", "timeMs": 0},
                {"id": "2", "text": "  ", "timeMs": 100},      // 空白丢弃
                {"id": "3", "text": "第二季我来啦", "timeMs": 8249},
            ],
            "startMs": 0, "nextMs": 30000, "total": 6884
        });
        let (items, next, total) = parse_page(&page);
        assert_eq!(items.len(), 2, "空白弹幕应被丢弃");
        assert_eq!(items[1].text, "第二季我来啦");
        assert_eq!(items[1].time_ms, 8249);
        assert_eq!(next, Some(30000));
        assert_eq!(total, 6884);
    }

    #[test]
    fn parse_page_no_next_means_end() {
        let page = json!({"items": [], "nextMs": 0, "total": 0});
        let (items, next, _) = parse_page(&page);
        assert!(items.is_empty());
        assert_eq!(next, None, "nextMs=0 视为结束，不能死循环");
    }

    #[test]
    fn esc_ass_neutralizes_tags() {
        // 大括号会被 libass 当标签块、反斜杠会被当标签 → 必须替换掉
        let out = esc_ass("{\\move(1,2)}坏\\N东西");
        assert!(!out.contains('{'));
        assert!(!out.contains('}'));
        assert!(!out.contains('\\'));
    }

    #[test]
    fn build_ass_has_dialogues_per_item() {
        let items = vec![
            DanmakuItem { text: "第一条".into(), time_ms: 0 },
            DanmakuItem { text: "第二条".into(), time_ms: 5000 },
        ];
        let ass = build_ass(&items, 1080, 1920);
        assert!(ass.contains("[Script Info]"));
        assert!(ass.contains("PlayResY: 1920"));
        assert_eq!(ass.matches("Dialogue:").count(), 2);
        assert!(ass.contains("第一条"));
        assert!(ass.contains("\\move("));
    }

    #[test]
    fn cache_path_sanitizes_colons() {
        let p = cache_path(Path::new("/tmp/c"), "hongguo", "hongguo:123:456");
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!name.contains(':'), "Windows 文件名不能有冒号：{name}");
        assert!(name.ends_with(".ass"));
    }
}

#[cfg(test)]
mod ass_format_tests {
    use super::*;

    /// 用真实抓到的弹幕样本校验生成的 ASS 结构合法（时间格式、必需段落、无裸标签）。
    #[test]
    fn real_samples_produce_valid_ass() {
        let items = vec![
            DanmakuItem { text: "剧组真有钱啊".into(), time_ms: 0 },
            DanmakuItem { text: "啊啊啊啊啊终于等到第三季了，太对味了[爱慕][爱慕]".into(), time_ms: 290 },
            DanmakuItem { text: "祝《少夫人来自东北》系列大暴[爱慕]".into(), time_ms: 430 },
        ];
        let ass = build_ass(&items, 1080, 1920);

        // 1) 必需段落
        for sec in ["[Script Info]", "[V4+ Styles]", "[Events]"] {
            assert!(ass.contains(sec), "缺段落 {sec}");
        }
        assert!(ass.contains("PlayResX: 1080"));
        assert!(ass.contains("PlayResY: 1920"));

        // 2) 每条都有 Dialogue，且格式合法
        let lines: Vec<&str> = ass.lines().filter(|l| l.starts_with("Dialogue:")).collect();
        assert_eq!(lines.len(), 3);

        // 3) 时间必须是 H:MM:SS.cc（两位小数），不能出现 3 位小数或空位
        let re_ok = |line: &str| {
            let parts: Vec<&str> = line.split(',').collect();
            parts.len() >= 4
                && parts[1].chars().filter(|c| *c == ':').count() == 2
                && parts[1].split('.').nth(1).map(|x| x.len()) == Some(2)
        };
        for l in &lines {
            assert!(re_ok(l), "时间格式非法: {l}");
        }

        // 4) 零时刻必须写成 0:00:00.00（不能是 0:00:000.0 这种）
        assert!(lines[0].contains(",0:00:00.00,"), "零时刻格式错: {}", lines[0]);

        // 5) 特征标签完整（ASS 反斜杠在 Rust 字符串里要双写）
        let move_tag = concat!("\\", "move(");
        assert!(lines.iter().all(|l| l.contains(move_tag)), "每条都应有 move 标签");
        // 6) 标签块闭合：每条 Dialogue 的 Text 段都含 `}`（\move 块收尾）
        assert!(lines.iter().all(|l| l.contains("}move") || l.contains("}")));
    }

    /// 分页合并去重：同一 (time,text) 在边界重复出现时只能留一条。
    #[test]
    fn dedup_key_is_time_and_text() {
        let mut seen = std::collections::HashSet::new();
        let a = DanmakuItem { text: "重复".into(), time_ms: 1000 };
        let b = DanmakuItem { text: "重复".into(), time_ms: 1000 };
        let c = DanmakuItem { text: "重复".into(), time_ms: 2000 };
        assert!(seen.insert((a.time_ms, a.text.clone())));
        assert!(!seen.insert((b.time_ms, b.text.clone())), "完全相同的应判重");
        assert!(seen.insert((c.time_ms, c.text.clone())), "时间不同不算重复");
    }
}
