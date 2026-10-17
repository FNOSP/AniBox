//! [gamepad] 全局手柄输入（原生 gilrs 监听）。
//!
//! 为什么不用 WebView Gamepad API：WebView 只有**窗口获得焦点**时才拿得到手柄输入，
//! 而本应用的核心场景是「mpv / PotPlayer 窗口在前台播放，AniBox 在后台」——
//! 这时 WebView 的 `navigator.getGamepads()` 不会更新。原生 gilrs（Windows 上默认走
//! Windows.GameInput，支持 XInput / DS4 / Switch Pro 等）在后台也能拿到全局输入。
//!
//! 设计要点：
//! - 后台线程持有 `Gilrs`（该对象不要求 Send，创建与使用都在同一线程），8ms 轮询一次；
//! - 只向前端发**语义事件** `gamepad-input`（press/repeat/release），不发原始轴噪声；
//! - 左摇杆走「死区 → 方向阈值 → 边沿检测 → 长按延迟/连跳」状态机；
//! - 连接状态共享给 `gamepad_status` / `gamepad_detect`；
//! - `gamepadEnabled=false` 时不做输入映射、不发输入事件（仍以低频维护连接状态，
//!   否则 `gamepad_detect` 在关闭时永远返回未连接，设置面板无法「按一下手柄试出来」）；
//! - 设置修改通过 [`update_settings`] 热更新，无需重启轮询线程。

use crate::config::{
    Config, GAMEPAD_DEADZONE_MAX, GAMEPAD_DEADZONE_MIN, GAMEPAD_DIRECTION_THRESHOLD_MAX,
    GAMEPAD_DIRECTION_THRESHOLD_MIN, GAMEPAD_REPEAT_DELAY_MAX, GAMEPAD_REPEAT_DELAY_MIN,
    GAMEPAD_REPEAT_INTERVAL_MAX, GAMEPAD_REPEAT_INTERVAL_MIN,
};
use gilrs::{Button, EventType, Gilrs};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// 事件名（与前端契约一致）
pub const EVENT_INPUT: &str = "gamepad-input";
pub const EVENT_STATUS: &str = "gamepad-status";

/// 轮询周期：8ms ≈ 125Hz。XInput/WGI 的典型刷新是 4–8ms，再快只是空转。
const POLL_INTERVAL: Duration = Duration::from_millis(8);
/// 连接状态兜底刷新周期（热插拔事件偶发丢失时仍能纠正）
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

// ───────────────────────── 设置 ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GamepadSettings {
    pub enabled: bool,
    pub deadzone: f32,
    pub direction_threshold: f32,
    pub repeat_delay_ms: u32,
    pub repeat_interval_ms: u32,
}

impl Default for GamepadSettings {
    fn default() -> Self {
        GamepadSettings {
            enabled: false,
            deadzone: 0.30,
            direction_threshold: 0.40,
            repeat_delay_ms: 320,
            repeat_interval_ms: 130,
        }
    }
}

impl From<&Config> for GamepadSettings {
    fn from(cfg: &Config) -> Self {
        GamepadSettings {
            enabled: cfg.gamepad_enabled,
            // 读盘值也 clamp 一次：手改 config.json 写成 5.0 不能让摇杆永远满偏
            deadzone: cfg
                .gamepad_deadzone
                .clamp(GAMEPAD_DEADZONE_MIN, GAMEPAD_DEADZONE_MAX),
            direction_threshold: cfg.gamepad_direction_threshold.clamp(
                GAMEPAD_DIRECTION_THRESHOLD_MIN,
                GAMEPAD_DIRECTION_THRESHOLD_MAX,
            ),
            repeat_delay_ms: cfg
                .gamepad_repeat_delay
                .clamp(GAMEPAD_REPEAT_DELAY_MIN, GAMEPAD_REPEAT_DELAY_MAX),
            repeat_interval_ms: cfg
                .gamepad_repeat_interval
                .clamp(GAMEPAD_REPEAT_INTERVAL_MIN, GAMEPAD_REPEAT_INTERVAL_MAX),
        }
    }
}

// ───────────────────────── 共享状态 ─────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Status {
    connected: bool,
    name: String,
}

struct Shared {
    settings: RwLock<GamepadSettings>,
    status: Mutex<Status>,
    stop: AtomicBool,
    /// 轮询线程是否已启动（避免重复 spawn）
    started: AtomicBool,
    /// `gamepad_detect` 请求立即刷新一次连接状态
    detect_requested: AtomicBool,
}

impl Shared {
    fn new(settings: GamepadSettings) -> Self {
        Shared {
            settings: RwLock::new(settings),
            status: Mutex::new(Status::default()),
            stop: AtomicBool::new(false),
            started: AtomicBool::new(false),
            detect_requested: AtomicBool::new(false),
        }
    }

    fn settings(&self) -> GamepadSettings {
        *self.settings.read().unwrap_or_else(|e| e.into_inner())
    }

    fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set_status(&self, status: Status) -> bool {
        let mut cur = self.status.lock().unwrap_or_else(|e| e.into_inner());
        if *cur == status {
            false
        } else {
            *cur = status;
            true
        }
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn shared() -> Option<&'static Arc<Shared>> {
    SHARED.get()
}

// ───────────────────────── 生命周期 ─────────────────────────

/// 启动后台轮询线程（幂等）。`settings` 用启动时读到的配置初始化。
pub fn spawn(app: AppHandle, settings: GamepadSettings) {
    let shared = SHARED.get_or_init(|| Arc::new(Shared::new(settings))).clone();
    *shared.settings.write().unwrap_or_else(|e| e.into_inner()) = settings;
    shared.stop.store(false, Ordering::SeqCst);

    // 只允许一个轮询线程
    if shared
        .started
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let thread_shared = shared.clone();
    let result = std::thread::Builder::new()
        .name("gamepad-poll".into())
        .spawn(move || run(app, thread_shared));
    if let Err(e) = result {
        shared.started.store(false, Ordering::SeqCst);
        crate::libmpv::trace(&format!("[gamepad] 启动轮询线程失败: {e}"));
    }
}

/// 请求轮询线程退出（应用退出时调用；线程在 ≤8ms 内自行结束）。
pub fn stop() {
    if let Some(s) = shared() {
        s.stop.store(true, Ordering::SeqCst);
    }
}

/// 设置热更新（set_settings / save_config 之后调用）。
pub fn update_settings(cfg: &Config) {
    if let Some(s) = shared() {
        *s.settings.write().unwrap_or_else(|e| e.into_inner()) = GamepadSettings::from(cfg);
    }
}

// ───────────────────────── 命令 ─────────────────────────

/// 当前连接状态（不触发额外扫描）。
#[tauri::command]
pub fn gamepad_status() -> Value {
    status_json()
}

/// 触发一次检测并返回当前状态（与 `gamepad_status` 同形；额外唤醒轮询线程刷新一次）。
#[tauri::command]
pub fn gamepad_detect() -> Value {
    if let Some(s) = shared() {
        s.detect_requested.store(true, Ordering::SeqCst);
    }
    status_json()
}

fn status_json() -> Value {
    let st = shared().map(|s| s.status()).unwrap_or_default();
    json!({ "connected": st.connected, "name": st.name })
}

// ───────────────────────── 轮询主循环 ─────────────────────────

fn run(app: AppHandle, shared: Arc<Shared>) {
    // gilrs 初始化失败（无 HID 权限 / 系统组件缺失）时每 5s 重试，不 panic。
    let mut gilrs = loop {
        if shared.stop.load(Ordering::SeqCst) {
            shared.started.store(false, Ordering::SeqCst);
            return;
        }
        match Gilrs::new() {
            Ok(g) => break g,
            Err(e) => {
                crate::libmpv::trace(&format!("[gamepad] 初始化失败（5s 后重试）: {e}"));
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    };
    crate::libmpv::trace("[gamepad] 轮询线程已启动（gilrs）");

    let start = Instant::now();
    let mut settings = shared.settings();
    let mut enabled_prev = settings.enabled;
    let mut engine = DirectionEngine::new(settings.repeat_delay_ms, settings.repeat_interval_ms);
    let mut dpad = [false; 4];
    let mut last_x = 0.0_f32;
    let mut last_y = 0.0_f32;
    let mut buttons_down: Vec<&'static str> = Vec::new();
    let mut last_status_check = Instant::now();

    // 启动时先同步一次真实连接状态（应用启动前手柄就已插入时没有 Connected 事件）
    let st = current_status(&gilrs);
    shared.set_status(st.clone());
    if settings.enabled {
        emit_status(&app, &st);
    }

    loop {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }

        let new_settings = shared.settings();
        if new_settings != settings {
            settings = new_settings;
            engine.set_timing(settings.repeat_delay_ms, settings.repeat_interval_ms);
        }

        // 开关切换：关闭时把按下态清干净并补发 release，避免前端卡在「一直按住」；
        // 重新打开时立即刷新并广播一次连接状态。
        if enabled_prev && !settings.enabled {
            for (action, phase) in engine.reset() {
                emit_input(&app, action, phase);
            }
            for action in buttons_down.drain(..) {
                emit_input(&app, action, "release");
            }
            dpad = [false; 4];
            last_x = 0.0;
            last_y = 0.0;
        } else if !enabled_prev && settings.enabled {
            let st = current_status(&gilrs);
            shared.set_status(st.clone());
            emit_status(&app, &st);
        }
        enabled_prev = settings.enabled;

        // 事件队列：无论是否启用都要 drain（否则关闭期间队列无限增长）；
        // 关闭时只处理连接/断开，不做输入映射。
        let mut want_status_refresh = false;
        while let Some(ev) = gilrs.next_event() {
            match ev.event {
                EventType::Connected | EventType::Disconnected => {
                    want_status_refresh = true;
                }
                EventType::ButtonPressed(btn, _) if settings.enabled => {
                    if let Some(action) = map_button(btn) {
                        if !buttons_down.contains(&action) {
                            buttons_down.push(action);
                            emit_input(&app, action, "press");
                        }
                    } else if let Some(idx) = dpad_index(btn) {
                        dpad[idx] = true;
                    }
                }
                EventType::ButtonReleased(btn, _) if settings.enabled => {
                    if let Some(action) = map_button(btn) {
                        if let Some(pos) = buttons_down.iter().position(|a| *a == action) {
                            buttons_down.remove(pos);
                            emit_input(&app, action, "release");
                        }
                    } else if let Some(idx) = dpad_index(btn) {
                        dpad[idx] = false;
                    }
                }
                EventType::AxisChanged(axis, value, _) if settings.enabled => match axis {
                    gilrs::Axis::LeftStickX => last_x = value,
                    gilrs::Axis::LeftStickY => last_y = value,
                    _ => {}
                },
                _ => {}
            }
        }

        // detect 请求 / 热插拔 / 定期兜底：刷新连接状态
        let detect = shared.detect_requested.swap(false, Ordering::SeqCst);
        if detect
            || want_status_refresh
            || last_status_check.elapsed() >= STATUS_REFRESH_INTERVAL
        {
            last_status_check = Instant::now();
            let st = current_status(&gilrs);
            let changed = shared.set_status(st.clone());
            if settings.enabled || detect {
                if changed || detect {
                    emit_status(&app, &st);
                }
            }
        }

        // 由当前设置重算摇杆方向（死区/阈值热更新立即生效）
        let stick = stick_direction(
            last_x,
            last_y,
            settings.deadzone,
            settings.direction_threshold,
        );

        if settings.enabled {
            let now_ms = start.elapsed().as_millis() as u64;
            let active = [
                dpad[0] || stick[0],
                dpad[1] || stick[1],
                dpad[2] || stick[2],
                dpad[3] || stick[3],
            ];
            for (action, phase) in engine.update(now_ms, active) {
                emit_input(&app, action, phase);
            }
        } else {
            // 关闭状态：清空方向按下态（release 已在切换瞬间补发）
            dpad = [false; 4];
            last_x = 0.0;
            last_y = 0.0;
            engine.reset();
            buttons_down.clear();
        }

        // 启用时高频轮询（8ms）；关闭时降到 250ms —— 只维护连接状态，
        // 不把「手柄关闭」的用户也拖进 125Hz 的 HID 轮询。
        let idle = if settings.enabled {
            POLL_INTERVAL
        } else {
            Duration::from_millis(250)
        };
        std::thread::sleep(idle);
    }

    shared.started.store(false, Ordering::SeqCst);
    crate::libmpv::trace("[gamepad] 轮询线程已退出");
}

fn current_status(gilrs: &Gilrs) -> Status {
    match gilrs.gamepads().find(|(_, g)| g.is_connected()) {
        Some((_, g)) => Status {
            connected: true,
            name: g.name().to_string(),
        },
        None => Status::default(),
    }
}

fn emit_input(app: &AppHandle, action: &str, phase: &str) {
    let _ = app.emit(EVENT_INPUT, json!({ "action": action, "phase": phase }));
}

fn emit_status(app: &AppHandle, st: &Status) {
    let _ = app.emit(
        EVENT_STATUS,
        json!({ "connected": st.connected, "name": st.name }),
    );
}

// ───────────────────────── 映射（纯函数） ─────────────────────────

/// 标准按钮 → 契约动作。未列出的按钮（摇杆按压 / Mode / C / Z）不映射。
pub fn map_button(button: Button) -> Option<&'static str> {
    use Button::*;
    Some(match button {
        South => "A",
        East => "B",
        North => "Y",
        West => "X",
        // gilrs 的命名：`*Trigger2` 是肩键（LB/RB），`*Trigger` 是扳机（LT/RT）
        LeftTrigger2 => "LB",
        RightTrigger2 => "RB",
        LeftTrigger => "LT",
        RightTrigger => "RT",
        Start => "Start",
        Select => "Back",
        _ => return None,
    })
}

fn dpad_index(button: Button) -> Option<usize> {
    match button {
        Button::DPadUp => Some(0),
        Button::DPadDown => Some(1),
        Button::DPadLeft => Some(2),
        Button::DPadRight => Some(3),
        _ => None,
    }
}

const DIR_ACTIONS: [&str; 4] = ["up", "down", "left", "right"];

/// 左摇杆 → 四方向布尔。先按**模长**过死区，再按单轴阈值判定；
/// 两轴同时过阈值时取绝对值更大的轴（避免斜推时一次发两个方向）。
pub fn stick_direction(x: f32, y: f32, deadzone: f32, threshold: f32) -> [bool; 4] {
    if (x * x + y * y).sqrt() < deadzone {
        return [false; 4];
    }
    let ax = x.abs();
    let ay = y.abs();
    let mut up = y >= threshold;
    let mut down = y <= -threshold;
    let mut left = x <= -threshold;
    let mut right = x >= threshold;
    if (up || down) && (left || right) {
        if ay >= ax {
            left = false;
            right = false;
        } else {
            up = false;
            down = false;
        }
    }
    [up, down, left, right]
}

#[derive(Clone, Copy, Default)]
struct DirState {
    pressed: bool,
    next_due_ms: u64,
}

/// 方向键状态机：边沿 press → 长按 delay 后首次 repeat → 每 interval 一次 repeat → release。
pub struct DirectionEngine {
    dirs: [DirState; 4],
    repeat_delay_ms: u64,
    repeat_interval_ms: u64,
}

impl DirectionEngine {
    pub fn new(repeat_delay_ms: u32, repeat_interval_ms: u32) -> Self {
        let mut e = DirectionEngine {
            dirs: [DirState::default(); 4],
            repeat_delay_ms: 1,
            repeat_interval_ms: 1,
        };
        e.set_timing(repeat_delay_ms, repeat_interval_ms);
        e
    }

    pub fn set_timing(&mut self, repeat_delay_ms: u32, repeat_interval_ms: u32) {
        self.repeat_delay_ms = u64::from(repeat_delay_ms.max(1));
        self.repeat_interval_ms = u64::from(repeat_interval_ms.max(1));
    }

    /// 用当前时刻与四方向按下态推进状态机，返回本 tick 需要发出的事件（有序）。
    pub fn update(
        &mut self,
        now_ms: u64,
        active: [bool; 4],
    ) -> Vec<(&'static str, &'static str)> {
        let mut out = Vec::new();
        for i in 0..4 {
            if active[i] {
                if !self.dirs[i].pressed {
                    self.dirs[i].pressed = true;
                    self.dirs[i].next_due_ms = now_ms + self.repeat_delay_ms;
                    out.push((DIR_ACTIONS[i], "press"));
                } else if now_ms >= self.dirs[i].next_due_ms {
                    out.push((DIR_ACTIONS[i], "repeat"));
                    self.dirs[i].next_due_ms = now_ms + self.repeat_interval_ms;
                }
            } else if self.dirs[i].pressed {
                self.dirs[i].pressed = false;
                out.push((DIR_ACTIONS[i], "release"));
            }
        }
        out
    }

    /// 清空按下态，返回需要补发的 release（关闭手柄开关时用）。
    pub fn reset(&mut self) -> Vec<(&'static str, &'static str)> {
        let mut out = Vec::new();
        for i in 0..4 {
            if self.dirs[i].pressed {
                self.dirs[i].pressed = false;
                out.push((DIR_ACTIONS[i], "release"));
            }
        }
        out
    }
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // 死区：模长小于 deadzone → 全中立（哪怕单轴已经不小）
    #[test]
    fn stick_deadzone_neutralizes_small_magnitude() {
        // 0.20 模长 < 0.30 死区
        assert_eq!(
            stick_direction(0.20, 0.0, 0.30, 0.20),
            [false; 4],
            "死区内必须回中"
        );
        // 斜推：模长 0.283 < 0.30，单轴 0.20 虽到阈值也必须中立
        assert_eq!(stick_direction(0.20, 0.20, 0.30, 0.20), [false; 4]);
    }

    // 阈值：过了死区但单轴不足 threshold → 仍中立
    #[test]
    fn stick_below_threshold_is_neutral() {
        assert_eq!(stick_direction(0.35, 0.0, 0.30, 0.40), [false; 4]);
        assert_eq!(stick_direction(0.0, -0.35, 0.30, 0.40), [false; 4]);
    }

    // 四方向基本判定（Y 正 = 上，X 正 = 右）
    #[test]
    fn stick_maps_four_directions() {
        assert_eq!(stick_direction(0.0, 0.9, 0.30, 0.40), [true, false, false, false]);
        assert_eq!(stick_direction(0.0, -0.9, 0.30, 0.40), [false, true, false, false]);
        assert_eq!(stick_direction(-0.9, 0.0, 0.30, 0.40), [false, false, true, false]);
        assert_eq!(stick_direction(0.9, 0.0, 0.30, 0.40), [false, false, false, true]);
    }

    // 斜推：只取主导轴，避免同时发两个方向
    #[test]
    fn stick_diagonal_picks_dominant_axis() {
        // 右更强 → 只有 right
        assert_eq!(stick_direction(0.9, 0.5, 0.30, 0.40), [false, false, false, true]);
        // 上更强 → 只有 up
        assert_eq!(stick_direction(0.5, 0.9, 0.30, 0.40), [true, false, false, false]);
        // 相等时按实现取 Y 轴（ay >= ax）
        assert_eq!(stick_direction(0.7, 0.7, 0.30, 0.40), [true, false, false, false]);
    }

    // 按钮映射（含肩键/扳机的 gilrs 命名差异）
    #[test]
    fn button_mapping_matches_contract() {
        use Button::*;
        assert_eq!(map_button(South), Some("A"));
        assert_eq!(map_button(East), Some("B"));
        assert_eq!(map_button(North), Some("Y"));
        assert_eq!(map_button(West), Some("X"));
        assert_eq!(map_button(LeftTrigger2), Some("LB"));
        assert_eq!(map_button(RightTrigger2), Some("RB"));
        assert_eq!(map_button(LeftTrigger), Some("LT"));
        assert_eq!(map_button(RightTrigger), Some("RT"));
        assert_eq!(map_button(Start), Some("Start"));
        assert_eq!(map_button(Select), Some("Back"));
        // 不映射的按键
        assert_eq!(map_button(Mode), None);
        assert_eq!(map_button(LeftThumb), None);
        assert_eq!(map_button(DPadUp), None, "DPad 由方向状态机处理，不直接映射");
    }

    // 边沿 + 长按延迟 + 连跳间隔
    #[test]
    fn direction_engine_press_repeat_release_timing() {
        let mut e = DirectionEngine::new(320, 130);

        // t=0 进入：press
        assert_eq!(e.update(0, [true, false, false, false]), vec![("up", "press")]);
        // 保持但未到延迟：无事件
        assert!(e.update(100, [true, false, false, false]).is_empty());
        assert!(e.update(319, [true, false, false, false]).is_empty());
        // 到 320：首次 repeat
        assert_eq!(e.update(320, [true, false, false, false]), vec![("up", "repeat")]);
        // 未到 +130：无事件
        assert!(e.update(400, [true, false, false, false]).is_empty());
        // 450 = 320 + 130：再次 repeat
        assert_eq!(e.update(450, [true, false, false, false]), vec![("up", "repeat")]);
        // 松开：release
        assert_eq!(e.update(500, [false; 4]), vec![("up", "release")]);
        // 已松开后再 tick：无事件
        assert!(e.update(600, [false; 4]).is_empty());
    }

    // 方向切换：旧方向 release、新方向 press 同一 tick 都发出
    #[test]
    fn direction_engine_switches_direction() {
        let mut e = DirectionEngine::new(320, 130);
        assert_eq!(e.update(0, [true, false, false, false]), vec![("up", "press")]);
        let ev = e.update(50, [false, false, false, true]);
        assert_eq!(ev, vec![("up", "release"), ("right", "press")]);
    }

    // reset 补发 release（关闭开关时避免前端卡键）
    #[test]
    fn direction_engine_reset_releases_held() {
        let mut e = DirectionEngine::new(320, 130);
        e.update(0, [true, false, true, false]);
        let rel = e.reset();
        assert_eq!(rel, vec![("up", "release"), ("left", "release")]);
        assert!(e.reset().is_empty());
    }

    // 设置默认值与 clamp
    #[test]
    fn settings_from_config_clamps_out_of_range() {
        let mut cfg = Config::default();
        assert!(!cfg.gamepad_enabled);
        assert_eq!(cfg.gamepad_deadzone, 0.30);
        assert_eq!(cfg.gamepad_direction_threshold, 0.40);
        assert_eq!(cfg.gamepad_repeat_delay, 320);
        assert_eq!(cfg.gamepad_repeat_interval, 130);

        cfg.gamepad_deadzone = 9.0;
        cfg.gamepad_direction_threshold = 0.0;
        cfg.gamepad_repeat_delay = 1;
        cfg.gamepad_repeat_interval = 9999;
        let s = GamepadSettings::from(&cfg);
        assert_eq!(s.deadzone, GAMEPAD_DEADZONE_MAX);
        assert_eq!(s.direction_threshold, GAMEPAD_DIRECTION_THRESHOLD_MIN);
        assert_eq!(s.repeat_delay_ms, GAMEPAD_REPEAT_DELAY_MIN);
        assert_eq!(s.repeat_interval_ms, GAMEPAD_REPEAT_INTERVAL_MAX);
    }
}
