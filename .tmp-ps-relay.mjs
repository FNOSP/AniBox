// 一次性：spawn 经 powershell.exe Start-Process 中转（绕开 Rust 直接 spawn 与 mpv 的不兼容）
import fs from "node:fs";

let s = fs.readFileSync("src-tauri/src/mpv.rs", "utf8");

const f = `        // bInheritHandles 假说验证：无 File 重定向 spawn（诊断靠 mpv --log-file）
        let mut child = tokio::process::Command::new(exe);
        let mut child = child.args(&args)`;
const r = `        // ⚠ Rust(std/tokio) 直接 spawn 的 mpv 必死（exit 1 零输出；参数/环境/Job/句柄/中转全部排查无果，
        // 同参经 PowerShell Start-Process 则存活——bInheritHandles=TRUE 继承应用句柄表为头号嫌疑）。
        // 工程解：经系统自带 powershell.exe Start-Process 中转（其 CreateProcess 不继承句柄表）。
        // mpv 由 powershell 脱离管理（Child=powershell 自身，秒退），死活判定靠 IPC 管道与 --log-file。
        let ps_args = args
            .iter()
            .map(|a| format!("'{}'", a.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(",");
        let ps_cmd = format!(
            "Start-Process -FilePath '{}' -ArgumentList @({}) -WindowStyle Hidden",
            exe.replace('\'', "''"),
            ps_args
        );
        let _ = exe; // exe 已内嵌进 ps_cmd
        let mut child = tokio::process::Command::new("powershell");
        child.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps_cmd]);
        let mut child = child.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())`;
if (!s.includes(f)) { console.error("miss1"); process.exit(1); }
s = s.replace(f, r);

fs.writeFileSync("src-tauri/src/mpv.rs", s, "utf8");
console.log("powershell relay wired");
