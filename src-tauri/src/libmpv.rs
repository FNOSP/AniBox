//! 诊断基建：带相对时间戳的 trace 日志 + 进程内 VEH 崩溃记录器。
//!
//! 历史：本模块原本还承载「进程内 libmpv 动态 FFI」（lc-080~086 的实验路线：LoadLibrary
//! libmpv-2.dll + 10 个 GetProcAddress 函数指针，视频 `--wid` 嵌子窗、控制走 C API）。
//! 该路线在 lc-087 被放弃 —— 内嵌 wid 下 mpv 不跑自己的消息循环：不处理鼠标键盘、
//! `mouse-pos.hover` 恒 false → uosc/OSC 底栏与一切交互类 UI 全废。播放改为
//! **独立 mpv.exe 进程 + `--wid` + 命名管道 IPC**（见 mpv.rs）。
//!
//! [lc-100] 因此把那套 FFI（Api / api() / err_str / MpvEvent* 与若干常量）整体删除：它已无
//! 任何调用方，只留下 12 条 dead_code 警告。现在这里只保留三件真正在用的诊断工具：
//! `trace` / `trace_reset` / `install_crash_logger`。

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// [lc-1436] 改 pub：诊断面板「打开诊断日志」要拿到这个路径
/// （此前只有本模块内部写日志用；dev.cmd 的 tail 也盯这个文件）。
pub fn log_path() -> PathBuf {
    std::env::temp_dir().join("fntv-libmpv.log")
}

/// 诊断日志（追加一行），前缀是**相对进程启动的毫秒数**。
/// 这个时间戳很关键：今天所有"点了才生效 / 切换那一拍错位"的定位，都靠还原
/// 「哪一拍用了旧值」才成立（早期日志没时间戳，只能靠猜）。
pub fn trace(msg: &str) {
    let ms = START.get_or_init(Instant::now).elapsed().as_millis();
    let line = format!("[{:>8}ms] {}\n", ms, msg);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

/// 每轮启动重建日志（truncate）——多轮 A/B 诊断不互相污染
pub fn trace_reset(header: &str) {
    let _ = std::fs::write(log_path(), format!("=== {}\n", header));
}

/// 进程内 VEH 崩溃记录器：致命异常瞬间把 异常码/故障模块/偏移 写进诊断日志，
/// 等价于崩溃转储的关键结论，无需外部调试器（本机无 cdb/procdump）。
pub fn install_crash_logger() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW};

    unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        if info.is_null() || unsafe { (*info).ExceptionRecord.is_null() } {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let rec = unsafe { &*(*info).ExceptionRecord };
        let code = rec.ExceptionCode as u32;
        // 0xC0000005 AV / 0xC000001D 非法指令 / 0xC00000FD 栈溢出 / 0x80000003 int3 断言
        if !matches!(code, 0xC000_0005 | 0xC000_001D | 0xC000_00FD | 0x8000_0003) {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let addr = rec.ExceptionAddress as usize;
        let (mut module, mut base) = (String::from("<unknown>"), 0usize);
        let mut hmod: windows_sys::Win32::Foundation::HMODULE = std::ptr::null_mut();
        // 0x4 FROM_ADDRESS | 0x2 UNCHANGED_REFCOUNT
        if unsafe { GetModuleHandleExW(0x4 | 0x2, addr as *const u16, &mut hmod) } != 0 {
            let mut buf = [0u16; 512];
            let n = unsafe { GetModuleFileNameW(hmod, buf.as_mut_ptr(), buf.len() as u32) };
            if n > 0 {
                module = String::from_utf16_lossy(&buf[..n as usize]);
            }
            base = hmod as usize;
        }
        // AV 参数：ExceptionInformation[0] 0=read/1=write/8=exec，[1]=访问的地址
        let detail = if code == 0xC000_0005 && rec.NumberParameters >= 2 {
            let kind = match rec.ExceptionInformation[0] {
                1 => "write",
                8 => "exec",
                _ => "read",
            };
            format!(" {} {:#x}", kind, rec.ExceptionInformation[1] as usize)
        } else {
            String::new()
        };
        trace(&format!(
            "CRASH code={:#010x}{} at {:#x} in {} +{:#x}",
            code,
            detail,
            addr,
            module,
            addr.wrapping_sub(base)
        ));
        EXCEPTION_CONTINUE_SEARCH
    }

    let handle = unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
    if handle.is_null() {
        trace("CRASHLOGGER: AddVectoredExceptionHandler failed");
    }
}
