//! 自建弹幕源（danmu_api）Rust 实现 —— 模块入口。
//!
//! 默认引擎仍是 node（行为与接入前完全一致）；显式开启后 Rust 直接接管
//! 127.0.0.1:22347，Node 原 shim 退到内部端口 22348，B 站/字幕等未重写端点原样反代。
//!
//! 开关：
//!   · 配置字段 `danmu_engine`：`"node"`（默认）| `"rust"`
//!   · 环境变量 `ANIBOX_DANMAKU_ENGINE=rust` 可临时覆盖（便于测试）

pub mod builtin;
pub mod client;
pub mod matching;
pub mod memory;
pub mod server;
pub mod types;
pub mod xml;

use serde_json::{json, Value};
use types::SelfHostedOutcome;

/// Lua 硬编码的弹幕 shim 端口（Rust / Node 谁接管由引擎决定）。
pub const LISTEN_PORT: u16 = 22347;

/// 解析当前生效的弹幕引擎（环境变量优先于配置）。
pub fn resolve_engine(cfg: &crate::config::Config) -> &'static str {
    if let Ok(v) = std::env::var("ANIBOX_DANMAKU_ENGINE") {
        let v = v.trim().to_ascii_lowercase();
        if v == "rust" {
            return "rust";
        }
        if v == "node" {
            return "node";
        }
    }
    if cfg.danmu_engine.trim().eq_ignore_ascii_case("rust") {
        "rust"
    } else {
        "node"
    }
}

pub fn start_engine(upstream: u16) -> Result<(), String> {
    server::start(LISTEN_PORT, upstream)
}

pub fn stop_engine() {
    server::stop();
}

/// 用户**显式填写**的地址列表（去空/去尾斜杠/去重）。为空表示使用内置源。
pub fn user_bases(cfg: &crate::config::Config) -> Vec<String> {
    let mut list: Vec<String> = cfg
        .danmu_api_bases
        .iter()
        .map(|b| b.trim().trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty())
        .collect();
    if list.is_empty() {
        let single = cfg.danmu_api_base.trim().trim_end_matches('/').to_string();
        if !single.is_empty() {
            list.push(single);
        }
    }
    let mut seen = std::collections::HashSet::new();
    list.retain(|b| seen.insert(b.clone()));
    list
}

/// 零配置解析：用户填了就用用户的；没填就用内置实例（`127.0.0.1:22352/<TOKEN>`）。
pub fn effective_bases(cfg: &crate::config::Config) -> Vec<String> {
    let user = user_bases(cfg);
    if user.is_empty() {
        vec![builtin::base()]
    } else {
        user
    }
}

/// 首个生效地址（Node 侧 `base` 字段、Lua 闸门同步用）。
pub fn effective_base(cfg: &crate::config::Config) -> String {
    effective_bases(cfg).into_iter().next().unwrap_or_default()
}

/// 当前是否使用内置实例（= 用户没有填写任何地址）。
pub fn uses_builtin(cfg: &crate::config::Config) -> bool {
    user_bases(cfg).is_empty()
}

/// 与 shim `/danmaku` 同构的结果 JSON。
pub fn result_json(o: &SelfHostedOutcome) -> Value {
    if !o.ok {
        return json!({
            "ok": false,
            "error": o.error.clone().unwrap_or_else(|| "未命中".into()),
            "sources": o.traces,
        });
    }
    json!({
        "ok": true,
        "bvid": Value::Null,
        "title": o.title,
        "matched_title": o.matched_title.clone().unwrap_or_default(),
        "sim": 1,
        "danmaku_count": o.danmaku_count,
        "source": client::SOURCE_LABEL,
        "cid": Value::Null,
        "aggregated_from": Value::Null,
        "cookie_status": Value::Null,
        "season_id": Value::Null,
        "epid": Value::Null,
        "season_mismatch": false,
        "sources": o.traces,
        "self_count": o.danmaku_count,
        "bilibili_count": Value::Null,
    })
}

/// 调试命令：只跑自建源三跳，返回与 shim 同构的 JSON 并写 XML。
#[tauri::command]
pub async fn danmaku_fetch_selfhosted(
    state: tauri::State<'_, crate::AppState>,
    title: String,
    ep: Option<i64>,
    season: Option<i64>,
    out: String,
    threshold: Option<i64>,
) -> Result<Value, String> {
    if title.trim().is_empty() || out.trim().is_empty() {
        return Err("缺少 title 或 out 参数".into());
    }
    if !xml::is_safe_path(&out) {
        return Err("out 路径不在允许的弹幕缓存目录内".into());
    }
    let _ = threshold;
    let cfg = state.config.read().await.clone();
    if !cfg.danmu_api_enabled {
        return Err("自建弹幕接口未启用".into());
    }
    let bases = effective_bases(&cfg);
    if bases.is_empty() {
        return Err("未配置自建弹幕接口地址".into());
    }
    let client = client::SelfHostedClient::new(bases);
    let ep = ep.unwrap_or(0);
    let season = season.unwrap_or(0).max(0) as u32;
    let series = {
        let path = std::env::temp_dir().join("anibox-play-meta.json");
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| {
                v.get("parentGuid")
                    .or_else(|| v.get("episodeGuid"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default()
    };
    match client
        .auto_fetch(&title, ep, &out, season, "", &series, "")
        .await
    {
        Some(o) => Ok(result_json(&o)),
        None => Ok(result_json(&SelfHostedOutcome::miss(
            client.last_fail_reason(),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::SelfHostedOutcome;

    #[test]
    fn hit_contract_has_all_fields() {
        let o = SelfHostedOutcome::hit(123, "番名 - 第1集".into(), "番名".into());
        let v = result_json(&o);
        for key in [
            "ok",
            "bvid",
            "title",
            "matched_title",
            "sim",
            "danmaku_count",
            "source",
            "cid",
            "aggregated_from",
            "cookie_status",
            "season_id",
            "epid",
            "season_mismatch",
            "sources",
            "self_count",
            "bilibili_count",
        ] {
            assert!(v.get(key).is_some(), "契约缺少字段 {key}");
        }
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["danmaku_count"], json!(123));
        assert_eq!(v["source"], json!("自建源(danmu_api)"));
        assert_eq!(v["sources"].as_array().unwrap().len(), 3);
        assert_eq!(v["sources"][0]["id"], json!("danmu_api"));
        assert_eq!(v["sources"][0]["used"], json!(true));
    }

    #[test]
    fn miss_contract_has_error_and_sources() {
        let o = SelfHostedOutcome::miss("未命中".into());
        let v = result_json(&o);
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["error"], json!("未命中"));
        assert!(v["sources"].is_array());
    }

    #[test]
    fn node_internal_port_avoids_webrs_beacon_range() {
        // webrs::ensure_beacon 的候选范围是 [22348,22349,22350]；
        // 内部 Node 端口必须避开，否则 beacon 先占 22348 时 Node 起不来、反代全 502。
        let port = crate::danmaku_shim::NODE_INTERNAL_PORT;
        assert_eq!(port, 22351);
        assert!(![22348u16, 22349, 22350].contains(&port));
    }

    #[test]
    fn engine_resolution_defaults_to_node() {
        let mut cfg = crate::config::Config::default();
        // 环境变量若被外部设置，跳过该断言
        if std::env::var("ANIBOX_DANMAKU_ENGINE").is_err() {
            assert_eq!(resolve_engine(&cfg), "node");
            cfg.danmu_engine = "rust".into();
            assert_eq!(resolve_engine(&cfg), "rust");
        }
    }
}
