//! [lc-1780] 外置弹幕源列表：`app_data_dir/danmaku-api-sources.json`。
//!
//! 借鉴 LumiPlayer 的 danmaku-api-sources.json 架构：弹弹play 兼容源不再只有官方一个——
//! 用户可在设置面板增删/启停/排序自定义源（自建镜像、第三方兼容端点），mpv 侧
//! `uosc_danmaku/apis/dandanplay.lua` 加载同一份文件：首个 enabled 项作为默认源，
//! 传输层失败按序轮换。默认列表 = 弹弹play官方（与从前行为等价）。
//!
//! 文件形状（camelCase，与 lua 侧解析一致）：
//! `{"sources":[{"name":"弹弹play 官方","baseUrl":"https://api.dandanplay.net","enabled":true}]}`
//!
//! 注意：mpv 会话在脚本加载时读一次并缓存，改动需重启播放器（UI 有提示）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DanmakuSource {
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
}

const MAX_SOURCES: usize = 12;

fn file_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|d| d.join("danmaku-api-sources.json"))
        .map_err(|e| format!("拿不到 app_data_dir: {e}"))
}

fn official_default() -> Vec<DanmakuSource> {
    vec![DanmakuSource {
        name: "弹弹play 官方".into(),
        base_url: "https://api.dandanplay.net".into(),
        enabled: true,
    }]
}

/// 读列表（文件缺失/损坏 → 官方默认，绝不 Err——面板始终可编辑并重写修复）。
#[tauri::command]
pub async fn danmaku_sources_get(app: AppHandle) -> Result<serde_json::Value, String> {
    let p = file_path(&app)?;
    let list: Vec<DanmakuSource> = match std::fs::read_to_string(&p) {
        Ok(s) => {
            match serde_json::from_str::<Value>(&s) {
                Ok(v) => {
                    let sources = v
                        .get("sources")
                        .and_then(|x| x.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let mut out = Vec::new();
                    for s in sources {
                        // 逐项宽容解析：坏一项丢一项，不让单项拖垮整份列表
                        if let Ok(mut d) = serde_json::from_value::<DanmakuSource>(s) {
                            normalize(&mut d);
                            out.push(d);
                        }
                    }
                    out
                }
                Err(_) => {
                    // 损坏：回落默认（面板一保存即修复）
                    official_default()
                }
            }
        }
        Err(_) => official_default(),
    };
    Ok(json!({ "sources": list, "restartNeeded": false }))
}

/// 写列表（全量覆盖）。校验：非空 name、http(s) baseUrl、去尾斜杠、上限 12 条。
#[tauri::command]
pub async fn danmaku_sources_set(
    app: AppHandle,
    sources: Vec<DanmakuSource>,
) -> Result<(), String> {
    if sources.len() > MAX_SOURCES {
        return Err(format!("弹幕源最多 {} 条", MAX_SOURCES));
    }
    let mut cleaned = Vec::new();
    for (i, mut s) in sources.into_iter().enumerate() {
        normalize(&mut s);
        if s.name.is_empty() {
            return Err(format!("第 {} 个源缺少名称", i + 1));
        }
        if !(s.base_url.starts_with("http://") || s.base_url.starts_with("https://")) {
            return Err(format!("「{}」的地址必须以 http(s):// 开头", s.name));
        }
        cleaned.push(s);
    }
    let p = file_path(&app)?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let body = serde_json::to_string_pretty(&json!({ "sources": cleaned }))
        .map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&p, body).map_err(|e| format!("写入失败: {e}"))
}

fn normalize(s: &mut DanmakuSource) {
    s.name = s.name.trim().to_string();
    s.base_url = s.base_url.trim().trim_end_matches('/').to_string();
    if s.name.is_empty() {
        // 名称缺失时用 host 兜底，保证 UI/lua 日志里可读
        let host = s
            .base_url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("自定义源")
            .to_string();
        s.name = if host.is_empty() { "自定义源".into() } else { host };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_and_strips_slash() {
        let mut s = DanmakuSource {
            name: "  镜像 ".into(),
            base_url: "https://dd.example.com/".into(),
            enabled: true,
        };
        normalize(&mut s);
        assert_eq!(s.name, "镜像");
        assert_eq!(s.base_url, "https://dd.example.com");
    }

    #[test]
    fn normalize_name_falls_back_to_host() {
        let mut s = DanmakuSource {
            name: "  ".into(),
            base_url: "http://127.0.0.1:22352/tok".into(),
            enabled: false,
        };
        normalize(&mut s);
        assert_eq!(s.name, "127.0.0.1:22352");
    }
}
