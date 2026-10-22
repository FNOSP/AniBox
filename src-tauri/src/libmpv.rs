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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// 组件级日志开关的键（与前端固定契约一致）。缺省视为全开。
pub const MODULE_KEYS: [&str; 7] = ["mpv", "bgm", "mirror", "feedback", "history", "webrs", "other"];

/// 日志运行态设置（来自 config.json，启动时注入 + 设置面板实时更新）。
///
/// 分流规则：
///   · `debug = true` 且该组件开关为 true（缺省 true）→ 写 `anibox.log`；
///   · 任意一条 WARN/ERROR 级别行（见 [`is_error_level`]）→ **无论 debug/组件开关**都写
///     `anibox-error.log`（独立报错日志，排查时不能被调试开关关掉）。
#[derive(Clone, Debug)]
pub struct LogSettings {
    pub debug: bool,
    pub modules: BTreeMap<String, bool>,
}

impl Default for LogSettings {
    fn default() -> Self {
        LogSettings {
            // 默认保持历史行为：诊断日志常开。
            debug: true,
            modules: BTreeMap::new(),
        }
    }
}

fn settings_cell() -> &'static RwLock<LogSettings> {
    static S: OnceLock<RwLock<LogSettings>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(LogSettings::default()))
}

/// 启动时 / 设置面板保存后调用：把配置注入日志运行态（立即生效，无需重启）。
pub fn apply_settings(debug: bool, modules: BTreeMap<String, bool>) {
    if let Ok(mut g) = settings_cell().write() {
        g.debug = debug;
        g.modules = modules;
    }
}

/// 当前日志设置快照（供诊断面板回显）。
pub fn snapshot_settings() -> LogSettings {
    settings_cell()
        .read()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// [lc-1436] 改 pub：诊断面板「打开诊断日志」要拿到这个路径
/// （此前只有本模块内部写日志用；dev.cmd 的 tail 也盯这个文件）。
pub fn log_path() -> PathBuf {
    std::env::temp_dir().join("anibox.log")
}

/// 独立报错日志（与 anibox.log 同目录）：只收 WARN/ERROR 级关键行。
pub fn error_log_path() -> PathBuf {
    std::env::temp_dir().join("anibox-error.log")
}

/// 组件归属：按消息前缀归类（低侵入 —— 不要求改所有调用点）。
/// 无前缀/未知前缀归到 `other`。需要精确归属时用 [`trace_mod`]。
pub fn module_of(msg: &str) -> &'static str {
    let m = msg.trim_start();
    if m.starts_with("[bgm") {
        "bgm"
    } else if m.starts_with("[cache") || m.starts_with("[mirror") {
        "mirror"
    } else if m.starts_with("[feedback") {
        "feedback"
    } else if m.starts_with("[history") {
        "history"
    } else if m.starts_with("[webrs") {
        "webrs"
    } else if m.starts_with("[mpv")
        || m.starts_with("mpv ")
        || m.starts_with("mpv.")
        || m.starts_with("mpv/")
        || m.starts_with("IPC ")
        || m.starts_with("launch(")
        || m.starts_with("player_")
    {
        "mpv"
    } else {
        "other"
    }
}

/// 是否 WARN/ERROR 级：中文「失败/错误/异常/崩溃」或英文 warn/panic/crash/fatal/exception/error。
/// 对 `error=` 这种占位形式做一次豁免（mpv 的 `error=` 字段在无错误时也会出现）。
pub fn is_error_level(msg: &str) -> bool {
    const CN: [&str; 4] = ["失败", "错误", "异常", "崩溃"];
    if CN.iter().any(|k| msg.contains(k)) {
        return true;
    }
    let low = msg.to_ascii_lowercase();
    const EN: [&str; 6] = ["warn", "panic", "crash", "fatal", "exception", "error"];
    for kw in EN {
        let mut from = 0usize;
        while let Some(rel) = low[from..].find(kw) {
            let idx = from + rel;
            let after = low[idx + kw.len()..].chars().next();
            // `error=` 视为字段占位，不算错误（真实失败一般会带「失败」或非空错误描述）。
            if kw == "error" && after == Some('=') {
                from = idx + kw.len();
                continue;
            }
            return true;
        }
    }
    false
}

fn module_enabled(settings: &LogSettings, module: &str) -> bool {
    settings.modules.get(module).copied().unwrap_or(true)
}

fn append_line(path: &Path, line: &str) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

/// 诊断日志（追加一行），前缀是**相对进程启动的毫秒数**。
/// 这个时间戳很关键：今天所有"点了才生效 / 切换那一拍错位"的定位，都靠还原
/// 「哪一拍用了旧值」才成立（早期日志没时间戳，只能靠猜）。
///
/// 组件按消息前缀自动归类（见 [`module_of`]）；需要明确归属时用 [`trace_mod`]。
pub fn trace(msg: &str) {
    trace_mod(module_of(msg), msg);
}

/// 带显式组件名的日志入口（低侵入：只在关键模块替换，其余调用点继续用 [`trace`]）。
pub fn trace_mod(module: &str, msg: &str) {
    let settings = snapshot_settings();
    let ms = START.get_or_init(Instant::now).elapsed().as_millis();
    let line = format!("[{:>8}ms] {}\n", ms, msg);

    // 普通诊断日志：debug 开 且 该组件开
    if settings.debug && module_enabled(&settings, module) {
        append_line(&log_path(), &line);
    }
    // 独立报错日志：WARN/ERROR 级一律落盘（不受 debug / 组件开关影响）
    if is_error_level(msg) {
        append_line(&error_log_path(), &line);
    }
}

/// 每轮启动重建诊断日志（truncate）——多轮 A/B 诊断不互相污染。
/// 调试日志关闭时只清空、不写头部（保证 `debug=false` 下 anibox.log 不产生新内容）。
/// 注意：**不动** `anibox-error.log`（独立报错日志要跨启动保留线索）。
pub fn trace_reset(header: &str) {
    let debug = snapshot_settings().debug;
    let body = if debug {
        format!("=== {}\n", header)
    } else {
        String::new()
    };
    let _ = std::fs::write(log_path(), body);
}

/// 进程内 VEH 崩溃记录器：致命异常瞬间把 异常码/故障模块/偏移 写进诊断日志，
/// 等价于崩溃转储的关键结论，无需外部调试器（本机无 cdb/procdump）。
/// VEH 是 Win32 专有机制，仅 Windows 实现；其它平台为空操作。
#[cfg(target_os = "windows")]
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

/// 非 Windows 平台的空实现（保持 `lib.rs` 里的调用点无需 cfg）。
#[cfg(not(target_os = "windows"))]
pub fn install_crash_logger() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_classification_by_prefix() {
        assert_eq!(module_of("[bgm] 镜像不可用"), "bgm");
        assert_eq!(module_of("[cache] LRU 淘汰"), "mirror");
        assert_eq!(module_of("[feedback] 工单已提交"), "feedback");
        assert_eq!(module_of("[history] 合并"), "history");
        assert_eq!(module_of("[webrs] resolve"), "webrs");
        assert_eq!(module_of("mpv file-loaded"), "mpv");
        assert_eq!(module_of("mpv end-file"), "mpv");
        assert_eq!(module_of("IPC 事件流已通"), "mpv");
        assert_eq!(module_of("JS: something"), "other");
        assert_eq!(module_of("[tmdb] GET"), "other");
    }

    #[test]
    fn error_level_detection() {
        assert!(is_error_level("播放失败：HTTP 500"));
        assert!(is_error_level("解析错误"));
        assert!(is_error_level("WARN something"));
        assert!(is_error_level("panic at ..."));
        assert!(is_error_level("CRASH code=0xc0000005"));
        assert!(is_error_level("connection error: refused"));
        // mpv 的空 error 字段占位不算错误
        assert!(!is_error_level("mpv end-file reason=eof error="));
        // 普通信息
        assert!(!is_error_level("[bgm] cache HIT subject 1"));
    }

    #[test]
    fn default_settings_all_enabled() {
        let s = LogSettings::default();
        assert!(s.debug);
        for k in MODULE_KEYS {
            assert!(module_enabled(&s, k), "缺省全开: {k}");
        }
    }

    #[test]
    fn error_log_path_is_sibling() {
        assert_eq!(error_log_path().parent(), log_path().parent());
        assert!(error_log_path().ends_with("anibox-error.log"));
    }

    /// 行为验证：debug=false 时 info 不写普通日志，但 ERROR 级仍写独立报错日志。
    #[test]
    fn routing_debug_off_still_writes_error_log() {
        let size = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let log_before = size(&log_path());
        let err_before = size(&error_log_path());

        apply_settings(false, BTreeMap::new());
        trace_mod("other", "UNIT_TEST_INFO_MARKER_1");
        trace_mod("other", "UNIT_TEST_ERROR_MARKER_1 失败");
        apply_settings(true, BTreeMap::new());

        assert_eq!(
            size(&log_path()),
            log_before,
            "debug=false 时普通日志不应增长"
        );
        assert!(
            size(&error_log_path()) > err_before,
            "WARN/ERROR 行必须写入独立报错日志"
        );
    }
}
