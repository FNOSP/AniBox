//! 自建弹幕源（danmu_api）的数据结构。
//!
//! 字段口径对齐 `assets/danmu-api.cjs` / `bili-runner.cjs`，但只保留 Rust 侧真正用到的
//! 部分；对外 JSON 契约在 `mod.rs::result_json` 统一组装。

use serde::{Deserialize, Serialize};

/// `%TEMP%/anibox-danmu-config.json`（Rust 写、Node/Rust 读）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DanmuConfigFile {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub bases: Vec<String>,
}

impl DanmuConfigFile {
    /// 与 Node `getDanmuApiBases()` 同口径：bases 优先、去空/去尾斜杠/去重，否则退回单地址。
    pub fn effective_bases(&self) -> Vec<String> {
        let clean = |b: &String| b.trim().trim_end_matches('/').to_string();
        let mut list: Vec<String> = self
            .bases
            .iter()
            .map(clean)
            .filter(|b| !b.is_empty())
            .collect();
        if list.is_empty() {
            let single = self.base.trim().trim_end_matches('/').to_string();
            if !single.is_empty() {
                list.push(single);
            }
        }
        // 去重（保留首次出现顺序）
        let mut seen = std::collections::HashSet::new();
        list.retain(|b| seen.insert(b.clone()));
        list
    }
}

/// 搜索命中条目（`/api/v2/search/anime` 的 animes[]）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchHit {
    pub anime_id: i64,
    pub anime_title: String,
    pub episode_count: i64,
    pub source: String,
    /// 季号档位：0=精确一致，1=播放侧无季号而候选标第一季，2=候选未标季，
    /// 3=候选未标季但播放侧要第 N 季（需分集标题核验）。
    pub season_tier: u8,
}

/// 分集（`/api/v2/bangumi/{id}` 的 bangumi.episodes[]）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Episode {
    pub episode_id: i64,
    pub episode_title: String,
    pub episode_number: Option<f64>,
}

/// 定位到的分集。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PickedEpisode {
    pub episode_id: i64,
    pub episode_title: String,
}

/// 逐源结论（回给 Lua「B站弹幕配置」面板的三行）。
#[derive(Debug, Clone, Serialize)]
pub struct SourceTrace {
    pub id: String,
    pub attempted: bool,
    pub used: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "skippedReason")]
    pub skipped_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
}

impl SourceTrace {
    pub fn used_self(detail: String, count: i64) -> Self {
        SourceTrace {
            id: "danmu_api".into(),
            attempted: true,
            used: true,
            detail: Some(detail),
            error: None,
            skipped_reason: None,
            count: Some(count),
        }
    }
    pub fn skipped(id: &str, reason: &str) -> Self {
        SourceTrace {
            id: id.into(),
            attempted: false,
            used: false,
            detail: None,
            error: None,
            skipped_reason: Some(reason.into()),
            count: None,
        }
    }
}

/// 自建源单次取弹幕的结果。
#[derive(Debug, Clone)]
pub struct SelfHostedOutcome {
    pub ok: bool,
    pub danmaku_count: i64,
    pub title: String,
    pub matched_title: Option<String>,
    pub error: Option<String>,
    pub traces: Vec<SourceTrace>,
}

impl SelfHostedOutcome {
    pub fn hit(count: i64, title: String, matched_title: String) -> Self {
        let traces = vec![
            SourceTrace::used_self(
                format!("命中《{matched_title}》（精确匹配）"),
                count,
            ),
            SourceTrace::skipped("bilibili", "自建源精确命中，未参与"),
            SourceTrace::skipped(
                "dandanplay",
                "弹弹play 兜底由 MPV 弹幕脚本按需触发（应用侧不含第三方凭证）",
            ),
        ];
        SelfHostedOutcome {
            ok: true,
            danmaku_count: count,
            title,
            matched_title: Some(matched_title),
            error: None,
            traces,
        }
    }

    pub fn miss(error: String) -> Self {
        SelfHostedOutcome {
            ok: false,
            danmaku_count: 0,
            title: String::new(),
            matched_title: None,
            error: Some(error),
            traces: vec![
                SourceTrace {
                    id: "danmu_api".into(),
                    attempted: true,
                    used: false,
                    detail: None,
                    error: Some("未命中".into()),
                    skipped_reason: None,
                    count: None,
                },
                SourceTrace::skipped("bilibili", "自建源未命中，已降级"),
                SourceTrace::skipped(
                    "dandanplay",
                    "弹弹play 兜底由 MPV 弹幕脚本按需触发（应用侧不含第三方凭证）",
                ),
            ],
        }
    }
}
