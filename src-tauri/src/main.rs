// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // [lc-1781] CLI 哈希助手：mpv 侧 sub-assrt.lua 经 ANIBOX_EXE 调用
    // （迅雷 gcid / OpenSubtitles moviehash）。打印 JSON 到 stdout 即退。
    let args: Vec<String> = std::env::args().collect();
    // [lc-1804] 五源字幕搜索/下载（mpv 侧 sub-assrt.lua 经 ANIBOX_EXE 调用）
    if args.len() == 3 && args[1] == "--subtitle-search" {
        tauri::async_runtime::block_on(async {
            match anibox_lib::subtitle_sources::cli_search(&args[2]).await {
                Ok(items) => println!("{}", serde_json::json!({ "items": items })),
                Err(e) => println!("{}", serde_json::json!({ "error": e })),
            }
        });
        return;
    }
    if (args.len() == 4 || args.len() == 5) && args[1] == "--subtitle-download" {
        let name = args.get(4).cloned().unwrap_or_else(|| "字幕.srt".into());
        tauri::async_runtime::block_on(async {
            match anibox_lib::subtitle_sources::cli_download(&args[2], &args[3], &name).await {
                Ok((path, title)) => println!("{}", serde_json::json!({ "path": path, "title": title })),
                Err(e) => println!("{}", serde_json::json!({ "error": e })),
            }
        });
        return;
    }
    if args.len() == 3 && args[1] == "--subtitle-hash" {
        let p = std::path::PathBuf::from(&args[2]);
        let out = match (
            anibox_lib::subtitle_sources::gcid_hash(&p),
            anibox_lib::subtitle_sources::opensub_hash(&p),
        ) {
            (Ok(g), Ok(o)) => format!(
                "{{\"gcid\":\"{}\",\"opensub\":\"{}\",\"size\":{}}}",
                g,
                o,
                std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0)
            ),
            (Err(e), _) | (_, Err(e)) => format!("{{\"error\":\"{}\"}}", e.replace('"', "'")),
        };
        println!("{out}");
        return;
    }
    anibox_lib::run()
}
