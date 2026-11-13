/* [liquid-theme] React 接入层：一个 hook，包住不受控的引擎。
 *
 * 核心设计：**动画期间不动 React 主题状态**
 *   底层真实界面保持旧主题（一个字节不动），揭示层里是新主题的视觉快照。
 *   动画走完的那一帧，先同步提交主题（applyDomTheme + onChange），再拆揭示层 ——
 *   两者在同一个任务里完成，浏览器只画终态一帧，所以零闪帧。
 *   好处：
 *     · 不存在两套 React 树 → 不会重复绑定事件、不会状态分叉
 *     · rAF 里不 setState → 组件树一帧都不重渲染
 *     · 业务状态（表单、筛选、滚动）全程只有一份，从未被复制或重置
 */

import { useCallback, useEffect, useRef } from "react";
import { createLiquidAudio, type LiquidAudio } from "./audio.ts";
import { LIQUID_BACKING } from "./palette.ts";
import { createLiquidReveal, supportsDynamicClipPath, type LiquidRevealHandle } from "./reveal.ts";
import { directionOf } from "./timeline.ts";
import type {
  LiquidAudioAdapter,
  LiquidDirection,
  LiquidOrigin,
  LiquidOriginSource,
  LiquidRevealConfig,
  LiquidStage,
  LiquidTheme,
  UseLiquidThemeOptions,
} from "./types.ts";

export const DEFAULT_LIQUID_DURATION = 1050;

/** 主题作用域的属性名。宿主项目若用 `html.dark` class 方案，
 *  只需在 applyThemeScope / applyDomTheme 里改写（见 README）。 */
export const LIQUID_THEME_ATTR = "data-theme";

let clipSeq = 0;
function nextClipId(): string {
  clipSeq += 1;
  return `lt-clip-${clipSeq}`;
}

/** 从"元素 / React 事件 / 裸坐标"取起点。事件优先用 currentTarget：
 *  键盘触发或点在元素边缘时，比 clientX/Y 稳定。 */
export function resolveOrigin(source: LiquidOriginSource): LiquidOrigin | null {
  if (!source) return null;
  if (typeof source === "object" && "x" in source && "y" in source && "radius" in source) {
    return source as LiquidOrigin;
  }
  const el: Element | null =
    source instanceof Element
      ? source
      : typeof (source as { currentTarget?: unknown }).currentTarget === "object"
        ? ((source as { currentTarget: Element | null }).currentTarget ?? null)
        : null;
  if (!el) return null;
  const r = el.getBoundingClientRect();
  const vw = document.documentElement.clientWidth || window.innerWidth;
  const vh = document.documentElement.clientHeight || window.innerHeight;
  // 夹到视口内：元素被滚出视口/变形时不至于把液柱起点甩到屏幕外
  return {
    x: Math.min(vw, Math.max(0, r.left + r.width / 2)),
    y: Math.min(vh, Math.max(0, r.top + r.height / 2)),
    radius: Math.max(4, Math.min(r.width, r.height) / 2),
  };
}

export type LiquidThemeApi = {
  /** 触发切换。传按钮元素或 React 事件（用它的 rect 算液体起点） */
  toggle: (source: LiquidOriginSource) => void;
  /** 中断当前动画（切视图 / 卸载前调用） */
  cancel: () => void;
  /** 是否正在播（非响应式：别在 render 里拿来做视觉判断） */
  isAnimating: () => boolean;
  /** 试听某个阶段（设置面板用；不启动动画） */
  audition: (stage: LiquidStage, direction?: LiquidDirection) => void;
  /** 内置音频实例（不存在时返回 null） */
  audio: LiquidAudio | null;
};

export type LiquidThemeHookOptions = UseLiquidThemeOptions & {
  /** 内置音效开关（只有没传 audio 适配器时生效）。默认 false */
  audioEnabled?: boolean;
  /** 内置音效音量 0..1 */
  audioVolume?: number;
};

export function useLiquidThemeReveal(options: LiquidThemeHookOptions): LiquidThemeApi {
  const {
    theme,
    onChange,
    applyDomTheme,
    duration = DEFAULT_LIQUID_DURATION,
    instant = false,
    audio: audioOverride,
    snapshotTarget,
    backing,
    onStage,
    audioEnabled = false,
    audioVolume = 0.45,
  } = options;

  const handleRef = useRef<LiquidRevealHandle | null>(null);
  const themeRef = useRef(theme);
  themeRef.current = theme;
  const busyRef = useRef(false);

  /** 内置音频实例：**懒创建**，且必须在用户手势里创建（Autoplay 政策） */
  const builtinRef = useRef<LiquidAudio | null>(null);
  const audioEnabledRef = useRef(audioEnabled);
  const audioVolumeRef = useRef(audioVolume);

  audioEnabledRef.current = audioEnabled;
  audioVolumeRef.current = audioVolume;

  // 音量/静音变化实时同步（不重建 instance，避免 AudioContext 反复开关）
  useEffect(() => {
    const a = builtinRef.current;
    if (!a) return;
    a.setVolume(audioVolume);
    a.setMuted(!audioEnabled || audioVolume <= 0);
  }, [audioEnabled, audioVolume]);

  // 关闭音效：立即停掉正在发声的源
  useEffect(() => {
    if (!audioEnabled) builtinRef.current?.stopAll();
  }, [audioEnabled]);

  const getAdapter = useCallback((): LiquidAudioAdapter | null => {
    if (audioOverride) return audioOverride;
    if (!audioEnabledRef.current || audioVolumeRef.current <= 0) return null;
    if (!builtinRef.current) {
      builtinRef.current = createLiquidAudio({ volume: audioVolumeRef.current });
    }
    return builtinRef.current;
  }, [audioOverride]);

  const applyThemeScope = useCallback((el: HTMLElement, t: LiquidTheme) => {
    el.setAttribute(LIQUID_THEME_ATTR, t);
  }, []);

  const commit = useCallback(
    (next: LiquidTheme) => {
      // 先同步改 DOM（此刻液体仍满屏），再通知调用方 setState
      applyDomTheme?.(next);
      onChange(next);
    },
    [applyDomTheme, onChange],
  );

  const cancel = useCallback(() => {
    busyRef.current = false;
    const h = handleRef.current;
    handleRef.current = null;
    h?.interrupt();
    builtinRef.current?.stopAll();
  }, []);

  const toggle = useCallback(
    (source: LiquidOriginSource) => {
      const from = themeRef.current;
      const to: LiquidTheme = from === "dark" ? "light" : "dark";
      const direction = directionOf(from, to);

      // 重复点击：先把上一次"快进到终态"（提交它承诺的主题），再开新的。
      // 不排队、不锁死、两次液体也不会叠加。
      if (busyRef.current && handleRef.current) {
        const prev = handleRef.current;
        handleRef.current = null;
        busyRef.current = false;
        prev.finishNow();
      }

      const origin = resolveOrigin(source);
      // 降级：减少动效 / 性能模式 / 时长 0 / 不支持动态 clip-path / 拿不到起点
      if (instant || duration <= 0 || !origin || !supportsDynamicClipPath()) {
        commit(to);
        return;
      }

      const audio = getAdapter();
      audio?.begin?.(direction);

      const cfg: LiquidRevealConfig = {
        direction,
        origin,
        duration,
        snapshotTarget: snapshotTarget ?? (() => document.getElementById("root")),
        backingColor: backing?.[to] ?? LIQUID_BACKING[to],
        clipId: nextClipId(),
        // 每帧直接喂音效：这是"音效由真实进度驱动"的唯一通路（不经过 React）
        onFrame: (snap) => audio?.update?.(snap),
        onStage: (stage, snap) => onStage?.(stage, snap),
        onEnd: (result) => {
          busyRef.current = false;
          handleRef.current = null;
          audio?.end?.(result);
          if (result === "complete") commit(to);
        },
      };

      const { handle } = createLiquidReveal(cfg, { applyThemeScope });
      handleRef.current = handle;
      busyRef.current = true;
      handle.start();
    },
    [applyThemeScope, backing, commit, duration, getAdapter, instant, onStage, snapshotTarget],
  );

  const audition = useCallback(
    (stage: LiquidStage, direction: LiquidDirection = "lighten") => {
      const a = getAdapter();
      if (!a) return;
      a.begin?.(direction);
      a.update({
        progress: stage === "pour" ? 0.1 : stage === "impact" ? 0.24 : stage === "rise" ? 0.7 : 0.94,
        stage,
        direction,
        flow: 0.72,
        surfaceY: 0,
        coverage: 0,
      });
      // 试听收尾：把流水关掉（与真实结束一致）
      window.setTimeout(() => a.end?.("complete"), stage === "rise" ? 900 : 420);
    },
    [getAdapter],
  );

  // 外部把 theme 改掉（别处 setTheme）→ 正在播的动画立即作废，避免提交出双重主题
  const lastSeen = useRef(theme);
  useEffect(() => {
    if (lastSeen.current !== theme) {
      lastSeen.current = theme;
      if (busyRef.current) cancel();
    }
  }, [theme, cancel]);

  // 页面进后台停声；卸载时清帧、清监听、释放 AudioContext
  useEffect(() => {
    const onHide = () => {
      if (document.visibilityState === "hidden") builtinRef.current?.stopAll();
    };
    document.addEventListener("visibilitychange", onHide);
    return () => {
      document.removeEventListener("visibilitychange", onHide);
      handleRef.current?.destroy();
      handleRef.current = null;
      busyRef.current = false;
      builtinRef.current?.dispose();
      builtinRef.current = null;
    };
  }, []);

  const isAnimating = useCallback(() => busyRef.current, []);

  return { toggle, cancel, isAnimating, audition, audio: builtinRef.current };
}
