/* ═══════════════════ [lc-2431] 弹簧物理：逐值移植 Compose `spring()` ═══════════════════
   来源：CoolApk 16.7.4 的液态玻璃底栏出自 https://github.com/Kyant0/AndroidLiquidGlass
   （Apache-2.0；其 dex 里留了这行 URL 作为出处声明）。它的动效全部由 Compose 的
   `Animatable.animateTo(target, spring(dampingRatio, stiffness))` 驱动，
   `DampedDragAnimation.kt` 里逐条写死了参数。

   浏览器侧没有 Compose，这里把同一条运动方程搬过来：
     x'' = -stiffness·(x - target) - c·x'      c = 2·dampingRatio·√stiffness
   质量恒为 1（Compose 的 spring 也是 mass=1），所以 stiffness 就是 ω²、单位 s⁻²。

   数值积分用**半隐式欧拉 + 固定子步长 1/240s**：最高刚度 1000 时 ω≈31.6 rad/s，
   ω·h≈0.13，远小于稳定上限 2，不会发散也不会有能量漂移。帧间隔用真实时间累加，
   掉帧时会补多步子步（上限 64ms，防止切后台回来一次跳完动画）。

   停表判据对齐 Compose 的 visibilityThreshold：位移与速度同时进入阈值才落位。
   速度阈值取 threshold·20 —— Compose 内部也是按阈值推速度阈，这里取一个经验倍率，
   实测（0.001 阈值）落位干净、不会出现「差 0.3px 永远在跑」的空转。 */

export interface SpringConfig {
  /** 0 = 无阻尼，1 = 临界阻尼（不过冲）；>1 过阻尼 */
  dampingRatio: number;
  /** 刚度 (s⁻²)：ω = √stiffness */
  stiffness: number;
}

export interface SpringChannel {
  value: number;
  velocity: number;
  target: number;
  config: SpringConfig;
}

/** 子步长：1/240s */
const SUBSTEP = 1 / 240;
/** 单帧最多推进 64ms（切后台/掉帧保护） */
const MAX_DT = 0.064;

export function createChannel(value: number, config: SpringConfig): SpringChannel {
  return { value, velocity: 0, target: value, config };
}

/** 推进一帧。返回 true = 仍在运动（调用方据此决定是否继续 rAF）。 */
export function stepChannel(ch: SpringChannel, dt: number, restDelta = 0.0005): boolean {
  const { config } = ch;
  const k = config.stiffness;
  const c = 2 * config.dampingRatio * Math.sqrt(k);
  let { value, velocity } = ch;
  let remaining = Math.min(Math.max(dt, 0), MAX_DT);

  while (remaining > 0) {
    const h = Math.min(SUBSTEP, remaining);
    const accel = -k * (value - ch.target) - c * velocity;
    velocity += accel * h;
    value += velocity * h;
    remaining -= h;
  }

  // 落位：位移与速度同时进入阈值
  if (Math.abs(value - ch.target) < restDelta && Math.abs(velocity) < restDelta * 20) {
    ch.value = ch.target;
    ch.velocity = 0;
    return false;
  }
  ch.value = value;
  ch.velocity = velocity;
  return true;
}

/** 直接落位（reduced-motion / 初始化） */
export function snapChannel(ch: SpringChannel, value: number) {
  ch.value = value;
  ch.velocity = 0;
  ch.target = value;
}

export function setTarget(ch: SpringChannel, target: number, config?: SpringConfig) {
  if (config) ch.config = config;
  ch.target = target;
}

/* ── 参数表：**逐个**对应 LiquidBottomTabs.kt / DampedDragAnimation.kt ──
   DampedDragAnimation 里五个 spec：
     valueAnimationSpec        spring(1f, 1000f, visibilityThreshold)
     velocityAnimationSpec     spring(0.5f, 300f, visibilityThreshold*10f)
     pressProgressAnimationSpec spring(1f, 1000f, 0.001f)
     scaleXAnimationSpec       spring(0.6f, 250f, 0.001f)
     scaleYAnimationSpec       spring(0.7f, 250f, 0.001f)
   另有 LiquidBottomTabs 里拖动结束的整条位移：spring(1f, 300f, 0.5f)。 */
export const SPRING = {
  /** 指示器位置（damper 1.0：过冲 0，贴手但不"甩"） */
  position: { dampingRatio: 1, stiffness: 1000 },
  /** 拖拽速度（0.5 会过冲，用来做挤压回弹） */
  velocity: { dampingRatio: 0.5, stiffness: 300 },
  /** 按压进度 */
  press: { dampingRatio: 1, stiffness: 1000 },
  /** 形变 X（0.6 轻微回弹） */
  scaleX: { dampingRatio: 0.6, stiffness: 250 },
  /** 形变 Y（0.7 更硬一点，避免竖向抖） */
  scaleY: { dampingRatio: 0.7, stiffness: 250 },
  /** 整条 tab 栏拖动时的位移回弹 */
  panel: { dampingRatio: 1, stiffness: 300 },
} as const satisfies Record<string, SpringConfig>;

/** Compose 的 EaseOut = CubicBezierEasing(0f, 0f, 0.58f, 1f)（面板位移曲线用） */
export function easeOut(t: number): number {
  // 该贝塞尔在给定 x 上取 y：用牛顿迭代反解参数，3~4 次即收敛到 1e-4
  const x = Math.min(Math.max(t, 0), 1);
  const bx = (s: number) => 3 * (1 - s) * (1 - s) * s * 0 + 3 * (1 - s) * s * s * 0.58 + s * s * s;
  const by = (s: number) => 3 * (1 - s) * s * s * 1 + s * s * s;
  let s = x;
  for (let i = 0; i < 6; i++) {
    const d = bx(s) - x;
    if (Math.abs(d) < 1e-5) break;
    const slope = 3 * (1 - s) * (1 - s) * 0 + 6 * (1 - s) * s * 0.58 + 3 * s * s;
    if (slope < 1e-6) break;
    s = Math.min(Math.max(s - d / slope, 0), 1);
  }
  return by(s);
}
