// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // [lc-1781] CLI 哈希助手：mpv 侧 sub-assrt.lua 经 ANIBOX_EXE 调用
    // （迅雷 gcid / OpenSubtitles moviehash）。打印 JSON 到 stdout 即退。
    let args: Vec<String> = std::env::args().collect();
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
