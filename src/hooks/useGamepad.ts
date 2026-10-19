/* 手柄操作（Xbox 布局标准映射，手柄当遥控器）。
 *
 * 原生事件版：轮询/死区/方向阈值/长按连跳全部由 Rust 侧（gilrs/XInput）完成，
 * 前端只消费语义 action，不再调用 navigator.getGamepads()。
 *
 * Tauri 事件契约：
 *  - `gamepad-input`  { action: "up"|"down"|"left"|"right"|"A"|"B"|"X"|"Y"|"LB"|"RB"|
 *                      "LT"|"RT"|"Start"|"Back", phase: "press"|"repeat"|"release" }
 *  - `gamepad-status` { connected: boolean, name: string }
 * 命令：`gamepad_status`（初始化连接状态）、`gamepad_detect`（设置面板「检测」）。
 *
 * 映射：
 *  - 非播放：方向 = 焦点导航；A = 确认；B/Back = 返回；LB/RB = 切页；
 *    Start = 打开设置；X = 收藏（点当前焦点卡片内的心形按钮）。
 *  - 播放中（监听 mpv-load / mpv-end）：A = 暂停/继续；LB/RB = ±5s；
 *    LT/RT = 倍速 ±0.25；B = 停止；Y = 下一集（操作 mpv 播放列表）。
 */

import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { focusNav } from "../lib/focusNav";

/* ────────────────────────── 按键定义 ────────────────────────── */

export type PadBtnName = "A" | "B" | "X" | "Y" | "LB" | "RB" | "LT" | "RT" | "BACK" | "START";

/** 设置面板「按键说明」表的固定映射。 */
export const GAMEPAD_KEY_TABLE: { btn: PadBtnName; label: string; action: string }[] = [
  { btn: "A", label: "A", action: "确认 / 播放暂停" },
  { btn: "B", label: "B", action: "返回 / 停止播放" },
  { btn: "X", label: "X", action: "收藏（当前卡片）" },
  { btn: "Y", label: "Y", action: "下一集" },
  { btn: "LB", label: "LB", action: "上一页 / 快退 5s" },
  { btn: "RB", label: "RB", action: "下一页 / 快进 5s" },
  { btn: "LT", label: "LT", action: "倍速 −" },
  { btn: "RT", label: "RT", action: "倍速 +" },
  { btn: "START", label: "Start", action: "打开设置" },
  { btn: "BACK", label: "Back", action: "返回 / 关闭" },
];

/* ────────────────────────── 配置默认值 ────────────────────────── */

/** 仅用于前端 UI 兜底显示；真值由 Rust `get_settings` / `set_settings` 持有，
 *  死区/阈值/长按连跳的实际处理也在 Rust 侧。 */
export const GAMEPAD_DEFAULTS = {
  enabled: true,
  stickDeadzone: 0.3,
  directionThreshold: 0.4,
  repeatDelay: 320,
  repeatInterval: 130,
} as const;

export const GAMEPAD_RANGES = {
  stickDeadzone: { min: 0.1, max: 0.5, step: 0.01 },
  directionThreshold: { min: 0.2, max: 0.8, step: 0.01 },
  repeatDelay: { min: 100, max: 800, step: 10 },
  repeatInterval: { min: 50, max: 400, step: 10 },
} as const;

/* ────────────────────────── 原生事件类型 ────────────────────────── */

export type GamepadAction =
  | "up"
  | "down"
  | "left"
  | "right"
  | "A"
  | "B"
  | "X"
  | "Y"
  | "LB"
  | "RB"
  | "LT"
  | "RT"
  | "Start"
  | "Back";

export type GamepadInputPayload = {
  action: GamepadAction;
  phase: "press" | "repeat" | "release";
};

export type GamepadStatusPayload = {
  connected: boolean;
  name?: string;
};

function normalizeStatus(raw: unknown): { connected: boolean; name: string } {
  const r = (raw ?? {}) as Record<string, unknown>;
  return {
    connected: Boolean(r.connected),
    name: String(r.name ?? r.id ?? ""),
  };
}

/** 初始化连接状态：`gamepad_status`。失败静默（非 Tauri 预览环境）。 */
export async function fetchGamepadStatus(): Promise<{ connected: boolean; name: string }> {
  try {
    return normalizeStatus(await invoke<unknown>("gamepad_status"));
  } catch {
    return { connected: false, name: "" };
  }
}

/** 「检测」按钮：优先 `gamepad_detect`，失败回退 `gamepad_status`。 */
export async function detectGamepad(): Promise<{ connected: boolean; name: string }> {
  try {
    return normalizeStatus(await invoke<unknown>("gamepad_detect"));
  } catch {
    return fetchGamepadStatus();
  }
}

/* ────────────────────────── Hook ────────────────────────── */

export type GamepadCallbacks = {
  /** Start：打开/关闭设置面板（也可由调用方改为打开侧栏）。 */
  onStart?: () => void;
  /** LB/RB：切页（-1 = 上一页，1 = 下一页）。 */
  onSwitchPage?: (dir: -1 | 1) => void;
  /** X：收藏当前焦点卡片（调用方决定具体实现）。 */
  onFavorite?: () => void;
};

export function useGamepad(callbacks: GamepadCallbacks = {}): void {
  const cbRef = useRef(callbacks);
  cbRef.current = callbacks;

  useEffect(() => {
    let stopped = false;
    let playing = false;
    let rate = 1;
    let lastRealEscapeAt = 0;

    const unlisteners: (() => void)[] = [];
    const addUnlisten = (p: Promise<() => void>) => {
      p.then((f) => {
        if (stopped) f();
        else unlisteners.push(f);
      }).catch(() => {});
    };

    // 真实键盘 Escape 与合成 Escape 去重：只记录非合成的按键。
    const onRealKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !(e as KeyboardEvent & { __fntvSynthetic?: boolean }).__fntvSynthetic) {
        lastRealEscapeAt = Date.now();
      }
    };
    window.addEventListener("keydown", onRealKey, true);

    addUnlisten(
      listen<{ phase?: string }>("mpv-load", (e) => {
        const phase = e.payload?.phase;
        if (phase === "start" || phase === "loaded") {
          playing = true;
          rate = 1;
        } else if (phase === "ended") {
          playing = false;
        }
      }),
    );
    addUnlisten(
      listen("mpv-end", () => {
        playing = false;
      }),
    );

    const cmd = (name: string, args?: Record<string, unknown>) => {
      void invoke(name, args).catch(() => {});
    };

    const dispatchEscape = () => {
      if (Date.now() - lastRealEscapeAt < 300) return;
      const ae = document.activeElement as HTMLElement | null;
      const target = ae && ae.isConnected ? ae : document.body;
      const opts: KeyboardEventInit = {
        key: "Escape",
        code: "Escape",
        bubbles: true,
        cancelable: true,
      };
      const down = new KeyboardEvent("keydown", opts);
      (down as KeyboardEvent & { __fntvSynthetic?: boolean }).__fntvSynthetic = true;
      target.dispatchEvent(down);
      window.setTimeout(() => {
        const up = new KeyboardEvent("keyup", opts);
        (up as KeyboardEvent & { __fntvSynthetic?: boolean }).__fntvSynthetic = true;
        target.dispatchEvent(up);
      }, 30);
    };

    const onPlaybackButton = (btn: PadBtnName) => {
      switch (btn) {
        case "A":
          cmd("mpv_toggle_pause");
          break;
        case "B":
        case "BACK":
          cmd("player_stop");
          break;
        case "LB":
          cmd("mpv_seek", { seconds: -5, absolute: false });
          break;
        case "RB":
          cmd("mpv_seek", { seconds: 5, absolute: false });
          break;
        case "LT":
          rate = Math.max(0.5, Math.round((rate - 0.25) * 100) / 100);
          cmd("mpv_set_rate", { rate });
          break;
        case "RT":
          rate = Math.min(2, Math.round((rate + 0.25) * 100) / 100);
          cmd("mpv_set_rate", { rate });
          break;
        case "Y":
          // [playlist] 下一集：直接操作 mpv 播放列表（整季列表由 play_item 一次性写入）。
          cmd("mpv_next_episode");
          break;
        case "START":
          cbRef.current.onStart?.();
          break;
        default:
          break;
      }
    };

    /** 手柄接管导航前的焦点处理（只在手柄路径调用，不影响键盘输入）：
     *  - `input[type=range]`：保留焦点，放行给 focusNav.move()（其左右键会调值）；
     *  - 文本类输入（input / textarea / select / contentEditable）：先 blur，
     *    让手柄重新接管焦点导航；
     *  - 其他元素：不动。 */
    const releaseEditableFocus = () => {
      const ae = document.activeElement as HTMLElement | null;
      if (!ae || ae === document.body) return;
      if (ae instanceof HTMLInputElement && ae.type === "range") return;
      const tag = ae.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || ae.isContentEditable) {
        ae.blur();
      }
    };

    const onNavButton = (btn: PadBtnName) => {
      switch (btn) {
        case "A":
          releaseEditableFocus();
          if (focusNav.isActive()) focusNav.select();
          else focusNav.activate();
          break;
        case "B":
        case "BACK":
          // 优先级：页面「返回」按钮 → Escape（关弹层）→ 隐藏白框兜底。
          if (!focusNav.clickBack()) {
            dispatchEscape();
            focusNav.back();
          }
          break;
        case "LB":
          if (!focusNav.hasOpenOverlay()) cbRef.current.onSwitchPage?.(-1);
          break;
        case "RB":
          if (!focusNav.hasOpenOverlay()) cbRef.current.onSwitchPage?.(1);
          break;
        case "X":
          cbRef.current.onFavorite?.();
          break;
        case "START":
          cbRef.current.onStart?.();
          break;
        default:
          break;
      }
    };

    const moveFocus = (dir: "up" | "down" | "left" | "right") => {
      releaseEditableFocus();
      focusNav.move(dir);
    };

    const onInput = (payload: GamepadInputPayload | undefined) => {
      if (!payload || !payload.action) return;
      const { action, phase } = payload;
      const dirAction = String(action).toLowerCase();

      if (
        dirAction === "up" ||
        dirAction === "down" ||
        dirAction === "left" ||
        dirAction === "right"
      ) {
        if (playing) return;
        // press = 首次进入；repeat = Rust 侧按长按延迟/间隔产生的连跳。
        if (phase === "press" || phase === "repeat") moveFocus(dirAction);
        return;
      }

      // 按钮只在 press 触发；repeat / release 忽略，避免连发。
      if (phase !== "press") return;
      if (dirAction === "start") {
        cbRef.current.onStart?.();
        return;
      }
      if (dirAction === "back") {
        if (playing) {
          cmd("player_stop");
        } else if (!focusNav.clickBack()) {
          // 无可见「返回」按钮：派发 Escape 关弹层，再隐藏白框兜底。
          dispatchEscape();
          focusNav.back();
        }
        return;
      }
      const btn = String(action).toUpperCase() as PadBtnName;
      if (playing) onPlaybackButton(btn);
      else onNavButton(btn);
    };

    addUnlisten(listen<GamepadInputPayload>("gamepad-input", (e) => onInput(e.payload)));

    return () => {
      stopped = true;
      window.removeEventListener("keydown", onRealKey, true);
      unlisteners.forEach((f) => f());
    };
  }, []);
}
