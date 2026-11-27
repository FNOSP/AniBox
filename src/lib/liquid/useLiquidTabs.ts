/* ═══════════════════ [lc-2431] 液态玻璃 tab 栏：拖拽 + 弹簧状态机 ═══════════════════
   逐条移植 Kyant0/AndroidLiquidGlass（Apache-2.0）`DampedDragAnimation.kt` +
   `LiquidBottomTabs.kt` 的行为，只是把 Compose 的 Animatable 换成 spring.ts 的数值积分。

   原实现的四件事，这里一件不少：
   ① 指示器位置是**连续值**（非整数索引），拖动时跟手，松手弹到最近槽位；
   ② 按压时 pressProgress 0→1，驱动：指示器放大、折射强度切换、高光/内外阴影出现；
   ③ 形变：scaleX/scaleY 用 damping 0.6/0.7 的弹簧，且按**速度**做挤压 ——
      速度向上时拉长、向下时压扁（原版 layerBlock 里的那三行）；
   ④ 面板整体（整条栏）在拖动时有极小的横向位移（panelOffset，最大 4px，EaseOut 衰减）。

   与 Web 的差异：
   · Compose 的 pointerInput 能拿到原始 dragAmount；这里用 Pointer Events 自己攒 delta。
   · 原版滑块拖到边界还有"橡皮筋"，我们没有对应的可拖数据（只有两个页面），
     因此索引钳在 [0,1]，不做越界回弹。

   注意：所有运动都写在 rAF 循环里**直接改 CSS 变量**，不经过 React state ——
   拖动时每帧 setState 会触发整棵树 reconciliation，60fps 下必掉帧。 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  createChannel,
  easeOut,
  setTarget,
  snapChannel,
  stepChannel,
  SPRING,
} from "./spring";
import { DRAG_THRESHOLD, PRESSED_SCALE } from "./types";

export interface LiquidTabsState {
  /** 当前页索引（0/1），供渲染选中态 */
  index: number;
  /** 面板级 pointer 事件（挂在整条栏上） */
  onPointerDown: (e: React.PointerEvent<HTMLElement>) => void;
  onPointerMove: (e: React.PointerEvent<HTMLElement>) => void;
  onPointerUp: (e: React.PointerEvent<HTMLElement>) => void;
  onPointerCancel: (e: React.PointerEvent<HTMLElement>) => void;
  /** 键盘/程序化切页（不动画拖拽，直接弹） */
  select: (index: number) => void;
  /** 拦掉指针交互后浏览器补发的 click */
  onClickCapture: (e: React.MouseEvent<HTMLElement>) => void;
  /** 每个槽自己的 click 处理器（程序化点击走这里） */
  makeClickHandler: (index: number) => (e: React.MouseEvent<HTMLElement>) => void;
  /** 滑块容器的 ref：需要知道槽位宽度 */
  trackRef: React.RefObject<HTMLDivElement | null>;
  /** 当前是否处于按压（供 CSS 切折射滤镜） */
  pressed: boolean;
}

export function useLiquidTabs(
  count: number,
  currentIndex: number,
  onSelect: (index: number) => void,
  reducedMotion: boolean,
): LiquidTabsState {
  const trackRef = useRef<HTMLDivElement | null>(null);
  const [pressed, setPressedState] = useState(false);

  // 全部运动通道（值域：position 为"槽位索引"，0..count-1）
  const ch = useMemo(
    () => ({
      position: createChannel(currentIndex, SPRING.position),
      panel: createChannel(0, SPRING.panel),
      press: createChannel(0, SPRING.press),
      scaleX: createChannel(1, SPRING.scaleX),
      scaleY: createChannel(1, SPRING.scaleY),
      velocity: createChannel(0, SPRING.velocity),
    }),
    // 只在 count 变化时重建（几乎不会）
    [count],
  );

  const rafRef = useRef<number | null>(null);
  const lastTsRef = useRef(0);
  const dragRef = useRef({
    active: false,
    moved: false,
    startX: 0,
    startY: 0,
    origin: 0,
    slotWidth: 0,
    /** 按下时命中的槽位（点击用） */
    hitIndex: -1,
    /** 本次交互已由指针逻辑处理完毕 —— 用来吞掉随后浏览器补发的 click，
     *  避免「指针切页」+「onClick 再切一次」。程序化 .click()（手柄/键盘/
     *  focusNav）不会经过 pointerdown，所以 suppress 为 false，正常走 onClick。 */
    suppressClick: false,
  });

  /** 把当前物理量写到 DOM（CSS 变量 + 数据属性），不触发 React 渲染 */
  const paint = useCallback(() => {
    const track = trackRef.current;
    if (!track) return;
    const slot = dragRef.current.slotWidth || track.clientWidth / count;
    const x = ch.position.value * slot;
    track.style.setProperty("--lx", `${x.toFixed(2)}px`);
    track.style.setProperty("--lpanel", `${ch.panel.value.toFixed(2)}px`);
    track.style.setProperty("--lpress", ch.press.value.toFixed(4));
    const v = Math.max(-1, Math.min(1, ch.velocity.value / 10));
    // 原版：scaleX /= 1 - clamp(v*0.75, ±0.2)；scaleY *= 1 - clamp(v*0.25, ±0.2)
    const sx = ch.scaleX.value / (1 - Math.max(-0.2, Math.min(0.2, v * 0.75)));
    const sy = ch.scaleY.value * (1 - Math.max(-0.2, Math.min(0.2, v * 0.25)));
    track.style.setProperty("--lscaleX", sx.toFixed(4));
    track.style.setProperty("--lscaleY", sy.toFixed(4));
  }, [ch, count]);

  /** 单例 rAF：任何通道还在动就继续跑 */
  const ensureLoop = useCallback(() => {
    if (rafRef.current != null) return;
    lastTsRef.current = 0;
    const tick = (ts: number) => {
      const dt = lastTsRef.current ? (ts - lastTsRef.current) / 1000 : 1 / 60;
      lastTsRef.current = ts;
      let alive = false;
      for (const c of Object.values(ch)) {
        if (stepChannel(c, dt)) alive = true;
      }
      paint();
      if (alive) {
        rafRef.current = requestAnimationFrame(tick);
      } else {
        rafRef.current = null;
        lastTsRef.current = 0;
      }
    };
    rafRef.current = requestAnimationFrame(tick);
  }, [ch, paint]);

  const setPressed = useCallback(
    (v: boolean) => {
      setPressedState((prev) => (prev === v ? prev : v));
    },
    [],
  );

  const press = useCallback(() => {
    setTarget(ch.press, 1);
    setTarget(ch.scaleX, PRESSED_SCALE);
    setTarget(ch.scaleY, PRESSED_SCALE);
    ensureLoop();
  }, [ch, ensureLoop]);

  const release = useCallback(() => {
    setTarget(ch.press, 0);
    setTarget(ch.scaleX, 1);
    setTarget(ch.scaleY, 1);
    setTarget(ch.velocity, 0);
    setTarget(ch.panel, 0);
    ensureLoop();
  }, [ch, ensureLoop]);

  /** 弹到某个槽位（原版 animateToValue：press → 位移 → release） */
  const settleTo = useCallback(
    (index: number) => {
      setTarget(ch.position, Math.max(0, Math.min(count - 1, index)));
      setTarget(ch.velocity, 0);
      setTarget(ch.panel, 0);
      ensureLoop();
    },
    [ch, count, ensureLoop],
  );

  const select = useCallback(
    (index: number) => {
      setTarget(ch.position, Math.max(0, Math.min(count - 1, index)));
      ensureLoop();
    },
    [ch, count, ensureLoop],
  );

  /* 外部（键盘/手柄/侧栏）改页 → 指示器跟上 */
  useEffect(() => {
    if (dragRef.current.active) return; // 拖动中不抢
    if (reducedMotion) {
      snapChannel(ch.position, currentIndex);
      paint();
      return;
    }
    if (ch.position.target !== currentIndex || Math.abs(ch.position.value - currentIndex) > 0.001) {
      setTarget(ch.position, currentIndex);
      ensureLoop();
    }
  }, [currentIndex, ch, ensureLoop, paint, reducedMotion]);

  /* 卸载清理 */
  useEffect(() => {
    return () => {
      if (rafRef.current != null) cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    };
  }, []);

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLElement>) => {
      if (e.button !== 0) return;
      const track = trackRef.current;
      if (!track) return;
      const rect = track.getBoundingClientRect();
      const slotWidth = rect.width / count;
      const relX = e.clientX - rect.left;
      const hit = Math.max(0, Math.min(count - 1, Math.floor(relX / slotWidth)));

      dragRef.current = {
        active: true,
        moved: false,
        startX: e.clientX,
        startY: e.clientY,
        origin: ch.position.target,
        slotWidth,
        hitIndex: hit,
        suppressClick: true,
      };
      // 拖动起点 = 当前稳定索引（而非目标值），避免连点叠加速度
      dragRef.current.origin = Math.round(ch.position.value);
      e.currentTarget.setPointerCapture?.(e.pointerId);
      if (!reducedMotion) {
        press();
        setPressed(true);
      }
    },
    [ch, count, press, reducedMotion, setPressed],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLElement>) => {
      const d = dragRef.current;
      if (!d.active) return;
      const dx = e.clientX - d.startX;
      const dy = e.clientY - d.startY;
      if (!d.moved) {
        if (Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
        d.moved = true;
      }
      if (reducedMotion) return;

      const raw = d.origin + dx / d.slotWidth;
      const clamped = Math.max(0, Math.min(count - 1, raw));
      // 位置直接跟随指针（拖拽期间不走弹簧）
      const prev = ch.position.value;
      ch.position.value = clamped;
      ch.position.target = clamped;
      ch.position.velocity = 0;
      // 面板位移：原版 = 4px * sign * EaseOut(|lateral / width|)
      const frac = Math.max(-1, Math.min(1, dx / (d.slotWidth * count)));
      ch.panel.value = 4 * Math.sign(frac) * easeOut(Math.abs(frac));
      // 速度通道（驱动挤压）：原版是 VelocityTracker 对 value 求导 ÷ 值域，
      // 这里用本帧位置增量 / 值域近似，再加一点平滑避免抖动。
      const vNow = Math.max(-1, Math.min(1, (clamped - prev) * 0.8));
      ch.velocity.value = ch.velocity.value * 0.6 + vNow * 0.4;
      paint();
    },
    [ch, count, paint, reducedMotion],
  );

  const finishDrag = useCallback(
    (e: React.PointerEvent<HTMLElement>, cancelled: boolean) => {
      const d = dragRef.current;
      if (!d.active) return;
      d.active = false;
      e.currentTarget.releasePointerCapture?.(e.pointerId);

      if (!d.moved && !cancelled) {
        // 点击：弹到被点的槽位
        select(d.hitIndex);
        if (d.hitIndex !== currentIndex) onSelect(d.hitIndex);
      } else if (!reducedMotion) {
        const nearest = Math.round(ch.position.value);
        const target = Math.max(0, Math.min(count - 1, nearest));
        settleTo(target);
        if (target !== currentIndex) onSelect(target);
      }

      if (reducedMotion) {
        snapChannel(ch.press, 0);
        snapChannel(ch.scaleX, 1);
        snapChannel(ch.scaleY, 1);
        snapChannel(ch.panel, 0);
        paint();
      } else {
        release();
      }
      setPressed(false);
      // 浏览器在 pointerup 之后紧跟着派发 click；等这一轮事件结束再放开，
      // 这样后续的程序化 .click()（手柄/键盘）又能正常触发。
      window.setTimeout(() => {
        dragRef.current.suppressClick = false;
      }, 0);
    },
    [ch, count, currentIndex, onSelect, paint, reducedMotion, release, select, setPressed, settleTo],
  );

  const onPointerUp = useCallback(
    (e: React.PointerEvent<HTMLElement>) => finishDrag(e, false),
    [finishDrag],
  );

  /** 程序化点击（手柄 Start/LB-RB、键盘 Enter、focusNav 的 el.click()）。
   *  指针路径已在 finishDrag 里切过页，这里只吞掉浏览器补发的 click。
   *  suppressClick 在下一次 pointerdown 前一直为 true，正好覆盖"指针交互后紧跟的那次 click"；
   *  真正的程序化点击从不经过 pointerdown，那时 suppressClick 是 false（初始化值），
   *  但为了不误伤"先拖过一次、再用手柄点"，finishDrag 结束时会延时复位。 */
  const onClickCapture = useCallback((e: React.MouseEvent<HTMLElement>) => {
    if (dragRef.current.suppressClick) {
      e.stopPropagation();
      e.preventDefault();
    }
  }, []);

  /** 按钮自身的 onClick：只有在"非指针路径"下才真正执行切页 */
  const makeClickHandler = useCallback(
    (index: number) => (e: React.MouseEvent<HTMLElement>) => {
      // 指针路径已处理 → 交给 onClickCapture 拦掉（这里做兜底判断）
      if (dragRef.current.suppressClick) return;
      e.stopPropagation();
      select(index);
      onSelect(index);
    },
    [onSelect, select],
  );
  const onPointerCancel = useCallback(
    (e: React.PointerEvent<HTMLElement>) => finishDrag(e, true),
    [finishDrag],
  );

  /* 窗口尺寸变化后槽位宽度变了：重算一次并落位 */
  useEffect(() => {
    const onResize = () => {
      const track = trackRef.current;
      if (!track) return;
      dragRef.current.slotWidth = track.clientWidth / count;
      paint();
    };
    window.addEventListener("resize", onResize);
    onResize();
    return () => window.removeEventListener("resize", onResize);
  }, [count, paint]);

  return {
    index: currentIndex,
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel,
    select,
    onClickCapture,
    makeClickHandler,
    trackRef,
    pressed,
  };
}
