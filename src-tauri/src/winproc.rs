//! [lc-1900] 子进程探活 / 终止：**纯 Win32 API，不再 spawn tasklist / taskkill**。
//!
//! 背景（2026-11-01 用户报告）：Windows 关机时先弹「ANIBOX 程序阻止关机」，紧接着桌面
//! 弹出 `tasklist.exe - Application Error (0xc0000142)` 硬错误框。根因是退出/轮询路径
//! 仍在 `Command::new("tasklist")` 探活：会话拆除阶段 CreateProcess 初始化失败
//! （STATUS_DLL_INIT_FAILED = 0xc0000142），系统弹出的模态错误框反而把关机继续拖住。
//!
//! 因此这里统一换成本进程内的 API 调用（零外部进程、微秒级）：
//!   · 探活：`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetExitCodeProcess`
//!   · 终止：`OpenProcess(PROCESS_TERMINATE)` + `TerminateProcess`（替代 `taskkill /F /PID`）
//!   · 按镜像名终止：`CreateToolhelp32Snapshot` 枚举 PID 后逐个终止（替代 `taskkill /IM`）
//!   · 句柄保险：把 mpv 放进 KILL_ON_JOB_CLOSE 的 job 对象，应用进程无论怎么退出
//!     （包括 tao 在 `WM_ENDSESSION` 里直接 `process::exit(0)` 的路径）都不会留下孤儿 mpv。
//!
//! 非 Windows 平台保留原来的 `kill` 命令实现（macOS/Linux 桌面端没有关机广播那套问题）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// 本次运行中需要在退出/会话结束时收掉的子进程 PID（mpv / PotPlayer）。
static TRACKED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// 会话结束（`WM_QUERYENDSESSION` / `WM_ENDSESSION`）已到达：此后禁止任何 spawn 慢路径。
static SESSION_ENDING: AtomicBool = AtomicBool::new(false);

/// 登记一个「退出时必须收掉」的子进程 PID。
pub fn track(pid: u32) {
    let mut list = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    if !list.contains(&pid) {
        list.push(pid);
    }
}

/// 注销已主动收掉的子进程 PID。
pub fn untrack(pid: u32) {
    let mut list = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
    list.retain(|p| *p != pid);
}

/// 会话是否已进入结束流程（关机/重启/注销）。
pub fn session_ending() -> bool {
    SESSION_ENDING.load(Ordering::SeqCst)
}

/// 标记会话结束（由 [`crate::winshutdown`] 的窗口过程在收到关机广播时置位）。
pub fn mark_session_ending() {
    SESSION_ENDING.store(true, Ordering::SeqCst);
}

/// 仅供单测复位（生产路径不需要「关机取消」复位：标记只用于本次进程的收尾）。
#[cfg(test)]
fn reset_session_ending_for_test() {
    SESSION_ENDING.store(false, Ordering::SeqCst);
}

/// 硬杀所有已登记的子进程（会话结束路径；终止进程是微秒级，不 spawn 任何东西）。
pub fn kill_tracked() {
    let list: Vec<u32> = {
        let mut guard = TRACKED.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    };
    for pid in list {
        terminate(pid);
    }
}

/// 进程是否还活着（替代 `tasklist /FI "PID eq <pid>"`）。
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // 打不开（进程已消失 / 无权限）：按「不在」处理，与原 tasklist 语义一致。
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        ok != 0 && code == STILL_ACTIVE as u32
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::process::{Command, Stdio};
        /// `kill -0`：进程存在 → 退出码 0；不存在 → 非 0。
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// 硬终止进程（替代 `taskkill /F /PID <pid>`）。返回是否调用成功。
pub fn terminate(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1);
        CloseHandle(h);
        ok != 0
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::process::{Command, Stdio};
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// 按映像名硬终止（替代 `taskkill /F /IM <exe>`），返回命中并终止的进程数。
#[cfg(target_os = "windows")]
pub fn terminate_by_image_name(exe: &str) -> usize {
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap == INVALID_HANDLE_VALUE {
            return 0;
        }
        let mut count = 0usize;
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry);
        while ok != 0 {
            let name: String = entry
                .szExeFile
                .iter()
                .take_while(|c| **c != 0)
                .map(|c| char::from_u32(*c as u32).unwrap_or('\u{fffd}'))
                .collect();
            if name.eq_ignore_ascii_case(exe) && terminate(entry.th32ProcessID) {
                count += 1;
            }
            ok = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
        count
    }
}

#[cfg(not(target_os = "windows"))]
pub fn terminate_by_image_name(_exe: &str) -> usize {
    0
}

/// [lc-1900] 把指定 PID 放进「句柄关闭即杀」的 job 对象（Windows）。
///
/// 目的：tao 在 `WM_ENDSESSION` 里直接 `std::process::exit(0)`，本应用的
/// `RunEvent::Exit` 清理钩子根本不会跑；只要把 mpv 挂进这个 job，进程退出时内核
/// 回收 job 句柄 → 连同 mpv 一起终止，彻底不留孤儿（比在任何退出钩子里补杀更可靠）。
#[cfg(target_os = "windows")]
pub fn assign_kill_on_close_job(pid: u32) {
    use windows_sys::Win32::Foundation::HANDLE;

    /// 已创建的 job 句柄（0 = 未创建）。句柄在本进程生命周期内保持打开，
    /// 进程结束时由内核关闭 → 触发 KILL_ON_JOB_CLOSE。
    static JOB: Mutex<usize> = Mutex::new(0);

    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        let mut slot = JOB.lock().unwrap_or_else(|e| e.into_inner());
        if *slot == 0 {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            *slot = job as usize;
        }
        let job: HANDLE = *slot as HANDLE;
        let proc = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if !proc.is_null() {
            // 失败（如父进程已在受限 job 内）不影响播放，TerminateProcess / 登记的 PID 仍是兜底。
            AssignProcessToJobObject(job, proc);
            CloseHandle(proc);
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn assign_kill_on_close_job(_pid: u32) {}

// ───────────────────────── 单测（纯逻辑，不起外部进程） ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 登记名单必须去重、注销后不再残留（否则退出时会反复 TerminateProcess 同一个 PID）。
    #[test]
    fn track_dedup_and_untrack() {
        track(4242);
        track(4242);
        assert_eq!(
            TRACKED
                .lock()
                .unwrap()
                .iter()
                .filter(|p| **p == 4242)
                .count(),
            1
        );
        untrack(4242);
        assert!(!TRACKED.lock().unwrap().contains(&4242));
    }

    /// 探活对自己必须返回 true、对不存在的 PID 返回 false（替代 tasklist 的核心语义）。
    #[test]
    fn pid_alive_on_self_and_missing() {
        assert!(pid_alive(std::process::id()), "本进程必须是活的");
        assert!(!pid_alive(0xFFFF_FFFE), "不存在的 PID 必须判为已退出");
    }

    /// 会话结束标记只升不降地生效（关机路径据此跳过慢操作）。
    #[test]
    fn session_ending_flag_roundtrip() {
        assert!(!session_ending(), "初始状态不应处于会话结束");
        mark_session_ending();
        assert!(session_ending());
        reset_session_ending_for_test();
    }
}
