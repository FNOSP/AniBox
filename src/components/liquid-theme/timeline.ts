/* [liquid-theme] 时间轴与纯数学（无副作用、可单测）。
 *
 * 阶段划分按"液体实际发生了什么"切，而不是按好看的百分比切：
 *   pour   液柱从按钮向下延伸（含中途分离的液滴）
 *   impact 液柱触底：飞溅 + 局部涟漪 + 池底开始积水
 *   pool   液面在底部积聚、起伏
 *   rise   液面整体上涨，最终漫过视口
 *   settle 完全覆盖后的一小段收束（液面继续微幅晃动 → 收尾音）
 * 音效的 4 个提示音严格挂在这些边界上（见 stageAt / STAGE_START）。
 */

import type { LiquidDirection, LiquidStage } from "./types.ts";

/** 各阶段的**结束**进度（0..1，严格递增）。改这里 = 改整条动画与音效的节奏。 */
export const STAGE_BOUNDS = {
  pour: 0.2,
  impact: 0.3,
  pool: 0.56,
  rise: 0.88,
  settle: 1,
} as const;

/** 各阶段**开始**进度（音效触发点用它，避免各写一遍魔法数） */
export const STAGE_START: Record<LiquidStage, number> = {
  idle: 0,
  pour: 0,
  impact: STAGE_BOUNDS.pour,
  pool: STAGE_BOUNDS.impact,
  rise: STAGE_BOUNDS.pool,
  settle: STAGE_BOUNDS.rise,
  done: 1,
};

export function clamp01(v: number): number {
  return v < 0 ? 0 : v > 1 ? 1 : v;
}

export function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

/** 区间内归一化到 0..1（区间外夹紧） */
export function span(v: number, from: number, to: number): number {
  if (to <= from) return v >= to ? 1 : 0;
  return clamp01((v - from) / (to - from));
}

export function smoothstep(from: number, to: number, v: number): number {
  const t = span(v, from, to);
  return t * t * (3 - 2 * t);
}

export function easeOutCubic(t: number): number {
  const x = clamp01(t);
  return 1 - Math.pow(1 - x, 3);
}

export function easeInOutCubic(t: number): number {
  const x = clamp01(t);
  return x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2;
}

/** 上涨段专用缓动：峰值速度只有 1.57×（cubic 是 3×）。
 *  原来用 cubic，峰值落在 t≈0.78 那一截 —— 观感就是"到 80 多突然冲上去"（用户实测反馈）。
 *  sine 的加速度分布更均匀，中段匀速推进、收尾自然减速。 */
export function easeInOutSine(t: number): number {
  const x = clamp01(t);
  return -(Math.cos(Math.PI * x) - 1) / 2;
}

/** 高斯包（涟漪、局部起伏用） */
export function gaussian(d: number, sigma: number): number {
  const s = Math.abs(sigma) < 1e-6 ? 1e-6 : sigma;
  return Math.exp(-(d * d) / (2 * s * s));
}

export function stageAt(progress: number): LiquidStage {
  if (progress <= 0) return "idle";
  if (progress < STAGE_BOUNDS.pour) return "pour";
  if (progress < STAGE_BOUNDS.impact) return "impact";
  if (progress < STAGE_BOUNDS.pool) return "pool";
  if (progress < STAGE_BOUNDS.rise) return "rise";
  if (progress < 1) return "settle";
  return "done";
}

/** 阶段内的局部进度 0..1（例如 rise 阶段 0.56→0.88 映射到 0→1） */
export function stageProgress(progress: number, stage: LiquidStage): number {
  const start = STAGE_START[stage];
  const end =
    stage === "pour"
      ? STAGE_BOUNDS.pour
      : stage === "impact"
        ? STAGE_BOUNDS.impact
        : stage === "pool"
          ? STAGE_BOUNDS.pool
          : stage === "rise"
            ? STAGE_BOUNDS.rise
            : stage === "settle"
              ? 1
              : 0;
  return span(progress, start, end);
}

/** 由当前主题与下一主题推出液体方向（同色视为 lighten：只用于兜底） */
export function directionOf(from: "light" | "dark", to: "light" | "dark"): LiquidDirection {
  if (from === "dark" && to === "light") return "lighten";
  if (from === "light" && to === "dark") return "darken";
  return to === "dark" ? "darken" : "lighten";
}

/** 方向对音高/滤波的影响系数（深沉 = 更低，清亮 = 更高） */
export function directionPitch(d: LiquidDirection): number {
  return d === "darken" ? 0.78 : 1.28;
}

/** 由 seed 生成确定性伪随机（同一个 seed 每一帧结果一致 → 可断点重放、可测试） */
export function seeded(n: number): () => number {
  let s = (n | 0) || 1;
  return () => {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    return ((s >>> 0) % 100000) / 100000;
  };
}
