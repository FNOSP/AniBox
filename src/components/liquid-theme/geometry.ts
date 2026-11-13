/* [liquid-theme] 液体几何：把"液柱 + 液滴 + 飞溅 + 波浪液面"算成 SVG path。
 *
 * 纯函数、无 DOM、无副作用 —— 每帧只做浮点运算与字符串拼接，**不读布局**。
 * 输出两个 d：
 *   clip   闭合轮廓（多子路径，nonzero 填充 = 并集），给 <clipPath> 用；
 *   sheen  只描"水的上沿"的开放路径，给湿边高光用。
 *
 * 三种形态（关键，否则会画出"液柱被水面接住"的错误矩形）：
 *   A 只有水体（未开始 / 液面已漫过按钮）
 *   B 液柱已触底 → 水体与液柱合成一条"棒棒糖"轮廓（从水面左扫，绕上按钮再下来）
 *   C 液柱悬空 → 水体 + 液柱各自独立子路径（nonzero 并集），中间不连
 */

import { STAGE_BOUNDS, clamp01, easeInOutSine, gaussian, lerp, smoothstep, span } from "./timeline.ts";
import type { LiquidStage } from "./types.ts";

export type LiquidGeoInput = {
  /** 视口宽高（CSS px） */
  w: number;
  h: number;
  /** 归一化进度 0..1（真实时钟折算） */
  t: number;
  stage: LiquidStage;
  /** 按钮中心 + 等效半径 */
  ox: number;
  oy: number;
  or: number;
  /** 确定性种子（同一 seed 每帧形态一致 → 可重放、可断言） */
  seed: number;
};

export type LiquidShape = {
  clip: string;
  sheen: string;
  /** 液面基准 y（不含波形；< 0 表示已漫过顶部） */
  surfaceY: number;
  /** 覆盖率 0..1 */
  coverage: number;
  /** 0..1 水流量（驱动连续流水声与高光浓度） */
  flow: number;
  /** 湿边（水线高光）不透明度 0..1：水面贴近顶边时收到 0，避免白光扫过顶部 */
  sheenAlpha: number;
  /** 是否已经完全盖满（true 时调用方可以把 clip-path 切成 none 停止逐帧栅格化） */
  sealed: boolean;
};

/** path 数值：一位小数足够（亚像素无差），字符串更短 → 每帧写入更快 */
function f1(n: number): string {
  const v = Math.round(n * 10) / 10;
  return Object.is(v, -0) ? "0" : String(v);
}

const K = 0.5522847498;
/** 椭圆子路径（4 段三次贝塞尔，kappa 近似） */
function ellipse(cx: number, cy: number, rx: number, ry: number): string {
  const kx = rx * K;
  const ky = ry * K;
  return (
    `M${f1(cx - rx)} ${f1(cy)}` +
    `C${f1(cx - rx)} ${f1(cy - ky)} ${f1(cx - kx)} ${f1(cy - ry)} ${f1(cx)} ${f1(cy - ry)}` +
    `C${f1(cx + kx)} ${f1(cy - ry)} ${f1(cx + rx)} ${f1(cy - ky)} ${f1(cx + rx)} ${f1(cy)}` +
    `C${f1(cx + rx)} ${f1(cy + ky)} ${f1(cx + kx)} ${f1(cy + ry)} ${f1(cx)} ${f1(cy + ry)}` +
    `C${f1(cx - kx)} ${f1(cy + ry)} ${f1(cx - rx)} ${f1(cy + ky)} ${f1(cx - rx)} ${f1(cy)}Z`
  );
}

/** 液面基准：先"底部积聚"（溅落处立刻出现一层水，占 0~14% 视口高），
 *  再"整体上涨"，末段越过顶部足够多以吞掉波谷。
 *
 * 为什么积水必须"看清"：需求要求"液柱落到底部 → 飞溅 + 局部涟漪 → 积聚成波浪液面"。
 * 若积水只涨 17% 且起点在视口下方（h*1.04），impact 阶段用户根本看不到水面 ——
 * 只有一排液滴在屏外溅，观感是"液柱消失了一下"。所以：
 *   · 起点 = h（视口底边），触底即有可见水面
 *   · impact→pool 期间涨到 0.14h（约 126px @900p，足够看出波浪与涟漪）
 *   · pool→1 再从 0.14h 涨到 −0.10h（越过顶部，确保波谷也被盖住） */
function surfaceBase(t: number, h: number): number {
  /* pour 阶段（0 → 0.2）液面必须停在视口**下方**：液柱要独占画面往下流，
     否则液面一出现就把液柱吞掉，用户只看到"底部一坨黑的"（实测截图就是这个毛病）。
     触底（pour 之后）才让水面升起。 */
  // 起点 h*1.005：pour 末段水面刚好探进视口底边一两像素的波浪，触底瞬间可见"水"，
  // 又不会提前把液柱吞掉（1.06 时整段 pour 完全看不到水，观感像液柱掉进虚空）。
  const bottom = h * 1.005;
  const accumulate = smoothstep(STAGE_BOUNDS.pour, STAGE_BOUNDS.pool, t) * h * 0.20;
  /* [lc-2155] 上涨行程从 1.24h 收到 0.87h + 缓动改 easeInOutSine。
     原来 1.24h 远超"盖满"所需的 0.805h，盖满落在缓动曲线的 65% 处 ——
     正好压上 easeInOutCubic 的 3× 峰速，实测 0.70→0.85（157ms）盖完 81% 屏幕，
     观感就是"前 80% 慢慢磨、到 80 多突然冲上去"（用户反馈）。
     现在行程与所需距离对齐、峰速 1.57×，盖满落在 t≈0.92，上涨段速度均匀；
     最后 8% 是水面已盖满后的收束。 */
  /* 行程 0.82h 是实测调出来的：水面起于 0.805h 下方（1.005h 起点 − 0.20h 积水），
     正好在缓动曲线尾部（t≈0.97）才走到顶边 —— 最后 3% 留给 sealed 收束，
     不再出现"84% 就盖满、之后十来帧空转"的空档（截图实测）。 */
  const rise = easeInOutSine(span(t, STAGE_BOUNDS.pool, 1)) * (h * 0.82);
  return bottom - accumulate - rise;
}

/** 水体振幅：撞击瞬间最大；接近视口顶边时**必须压平到 0**。
 *
 *  [lc-2155] 这是「90% 左右页面元素闪一下」的真因：
 *    水面基准越过顶边（surface ≈ 0）时波浪仍有 ±29px 振幅 ——
 *    于是**波峰先越过顶边、波谷还差几十像素**，顶边残留一撮撮锯齿状旧主题竖条，
 *    且它们随波相位左右移动，看起来就是顶部元素在闪。
 *    把振幅随 surface 趋近 0 压平后，整条水线一次越过顶边，切得干净。 */
function amplitudeAt(t: number, h: number, surface: number): number {
  const boom = gaussian(span(t, STAGE_BOUNDS.pour, STAGE_BOUNDS.impact + 0.06), 0.16);
  const flowing = smoothstep(STAGE_BOUNDS.impact, STAGE_BOUNDS.pool, t);
  const fade = 1 - 0.5 * smoothstep(0.9, 1, t);
  const base = Math.min(30, Math.max(9, h * 0.024));
  /* 压平窗口从 **0.45h** 开始（原来 0.20h 太晚 —— 实测水面到 y=145（顶部 18%）时
     振幅还有 142px，顶边残留一大片锯齿，正是用户说的"90 左右闪一下"）。
     0.45h 起随水位线性收到 0，上升段前半仍保留明显波浪，后半平滑压平。 */
  const flatten = smoothstep(0, h * 0.45, surface);
  return base * (0.3 + flowing * 0.8 + boom * 0.9) * fade * flatten;
}

/** 湿边（水线高光）可见度：水面接近顶部时收掉。
 *  7px 白色柔光扫过顶部会像"闪一下白光"，所以让它比振幅更早淡出：
 *  实测水面在 y=145（顶部 18%）时 sheenAlpha 还是 1.00，顶部确实会闪；
 *  改成在 0.06h→0.40h 之间淡完，进入顶部 40% 时已经完全不可见。 */
function sheenAlphaAt(h: number, surface: number): number {
  return smoothstep(h * 0.06, h * 0.4, surface);
}

/** 液面 y：多正弦叠加 + 撞击点局部涟漪（涟漪是水面几何本身，不是额外画的圈） */
function waveY(x: number, t: number, surface: number, amp: number, ox: number, w: number): number {
  const u = x / Math.max(1, w);
  let y = surface;
  y += Math.sin(u * 26 + t * 7.2) * amp * 0.5;
  y += Math.sin(u * 61 - t * 10.5 + 1.7) * amp * 0.26;
  y += Math.sin(u * 11 + t * 4.1 + 0.6) * amp * 0.62;
  const since = span(t, STAGE_BOUNDS.pour, 1);
  const spread = 70 + since * w * 0.5;
  const ring = Math.sin(since * 34) * Math.exp(-since * 3.2) * 2.1;
  y -= gaussian(x - ox, spread) * amp * ring;
  return y;
}

/** 液柱中心线：顶端钉在按钮上，越往下摆动越大 */
function columnCenter(y: number, t: number, ox: number, oy: number, h: number, or: number): number {
  const k = clamp01((y - oy) / Math.max(1, h - oy));
  // 摆动：低频大幅（像被拉长的水柱在晃）+ 高频小幅（表面张力抖动）。
  // 幅度随高度 k 增长（顶端钉在按钮上），0.9 的幅度在 1280 宽下约 40px 摆幅，看得出"自然摆动"。
  const swing = Math.sin((y - oy) * 0.017 - t * 14) * 26 * k;
  const jitter = Math.sin((y - oy) * 0.062 - t * 33) * 4.5 * k;
  return ox + swing * (0.55 + or * 0.03) + jitter;
}

/** 液柱半径：向下渐细 + 沿柱身脉动（制造"颈缩/分离"的观感）+ 收尾变细 */
function columnRadius(y: number, t: number, oy: number, h: number, or: number): number {
  // 液柱主体宽度：按钮半径 × 0.95。截图实测 0.62 在 1280 宽下只有 ~9px，偏"线"而非"柱"；
  // 0.95 给出约 13px 的视觉宽度（按钮 28px 高时），既细长又看得出体积。
  const top = Math.max(6.5, or * 0.95);
  const k = clamp01((y - oy) / Math.max(1, h - oy));
  const taper = 1 - 0.28 * k;
  const endThin = 1 - 0.5 * smoothstep(STAGE_BOUNDS.pool, 0.95, t);
  const pulse = 0.90 + 0.18 * Math.sin((y - oy) * 0.035 - t * 26);
  return Math.max(1.1, top * taper * endThin * pulse);
}

/** 液柱下端 y：0..pour 期间加速落到视口底 */
function columnTipY(t: number, oy: number, h: number, or: number): number {
  const k = span(t, 0, STAGE_BOUNDS.pour);
  const eased = k * k * (0.55 + 0.45 * k);
  return lerp(oy + or * 0.9, h * 1.02, eased);
}

type Droplet = { t0: number; x0: number; vx: number; r: number; vy0: number };
type Speck = { a: number; v: number; r: number; lag: number };

/** 线性同余伪随机：同一 seed 结果固定（不依赖 Math.random，动画可重放） */
function rngOf(seed: number): () => number {
  let s = (seed | 0) || 7;
  return () => {
    s = (s * 1103515245 + 12345) & 0x7fffffff;
    return (s % 100000) / 100000;
  };
}

/** 液柱下落途中分离出的液滴 */
function droplets(seed: number, or: number): Droplet[] {
  const rnd = rngOf(seed ^ 0x5f3a);
  const out: Droplet[] = [];
  for (let i = 0; i < 5; i += 1) {
    out.push({
      t0: 0.035 + i * 0.031 + rnd() * 0.012,
      x0: (rnd() - 0.5) * or * 1.9,
      vx: (rnd() - 0.5) * 26,
      r: 1.9 + rnd() * 2.6,
      vy0: 40 + rnd() * 90,
    });
  }
  return out;
}

/** 撞底飞溅的小水珠（向上半圆抛洒） */
function splash(seed: number): Speck[] {
  const rnd = rngOf(seed ^ 0x1c9e);
  const out: Speck[] = [];
  for (let i = 0; i < 9; i += 1) {
    out.push({
      a: -Math.PI * (0.12 + rnd() * 0.76),
      v: 180 + rnd() * 320,
      r: 1.6 + rnd() * 3.4,
      lag: rnd() * 0.055,
    });
  }
  return out;
}

const DROP_G = 2600; // px/s²，视觉重力（不是物理真值）

/** 主入口 */
export function buildLiquidShape(input: LiquidGeoInput): LiquidShape {
  const { w, h, t, ox, oy, or, seed } = input;
  const surface = surfaceBase(t, h);
  // 振幅需要 surface（越接近顶边越压平，见 amplitudeAt 注释）
  const amp = amplitudeAt(t, h, surface);
  const coverage = clamp01((h - surface) / Math.max(1, h));
  const sheenAlpha = sheenAlphaAt(h, surface);
  /* 完全盖满：水面已越过顶边且振幅压到 0（此时水线是一条直线、已经出屏），
     可以把 clip 直接切成 none —— 少一次每帧的整屏 clip-path 栅格化，
     也彻底消除"顶部残留锯齿"的可能。 */
  const sealed = surface + amp * 1.2 <= 0;

  // 水量 = 上升速度 + 撞击瞬时能量（供连续流水声与高光浓度使用）
  // 注意 t=0 必须为 0：动画尚未开始就报 0.62 的水量，音效会在第一帧就误起流水。
  const next = surfaceBase(Math.min(1, t + 0.012), h);
  const riseRate = clamp01((surface - next) / (h * 0.018));
  // 撞击能量包：中心落在 pour 边界（液柱触底那一刻），前后各约 0.14 宽
  const impactEnergy = gaussian(t - STAGE_BOUNDS.pour, 0.11) * smoothstep(0.02, STAGE_BOUNDS.pour, t);
  const flow = clamp01(
    0.14 * smoothstep(STAGE_BOUNDS.pour * 0.5, STAGE_BOUNDS.pool, t) + 0.42 * riseRate + 0.62 * impactEnergy,
  );

  /* 液柱的生命周期：从动画第一帧一直存在，**直到上涨的水面漫过出水口**（按钮下缘）。
     两个容易写错的地方：
       · 不能拿"水面是否在视口外"判断 —— pour 期间水面本就在视口下，那会把液柱整段判掉
         （实测截图：只剩底部一条水，没有柱子，动画完全看不出"往下流"）。
       · 液柱下端不必止于视口底：触底后它继续"喂"水面，所以下端直接接到水面，
         两者合成一个轮廓（棒棒糖形态）；按钮被淹后液柱才消失。 */
  const hasColumn = t > 0 && t < 1 && surface > oy + or * 1.2;

  const tipRaw = columnTipY(t, oy, h, or);
  // 触底后液柱下端跟随水面（低于水面时取水面），这样水体与液柱天然连成一个轮廓
  const tip = Math.min(tipRaw, Math.max(surface, oy + or));
  const topY = oy + Math.max(0, or * 0.55);
  // 液柱下端是否已经触到水面（决定用"棒棒糖"合并轮廓还是两个独立子路径）
  const connected = hasColumn && tipRaw >= surface - 2;

  // 液面采样
  const N = Math.max(40, Math.min(140, Math.round(w / 13)));
  const wx: number[] = [];
  const wy: number[] = [];
  for (let i = 0; i <= N; i += 1) {
    const x = (w * i) / N;
    wx.push(x);
    wy.push(waveY(x, t, surface, amp, ox, w));
  }

  let clip = "";
  let sheen = "";

  /* ── 液柱两侧采样（从下往上） ── */
  const M = Math.max(14, Math.min(56, Math.round((h - oy) / 24)));
  const lx: number[] = [];
  const ly: number[] = [];
  const rx: number[] = [];
  const ry: number[] = [];
  if (hasColumn) {
    for (let i = 0; i <= M; i += 1) {
      const y = lerp(tip, topY, i / M);
      const cx = columnCenter(y, t, ox, oy, h, or);
      const rr = columnRadius(y, t, oy, h, or);
      lx.push(cx - rr);
      ly.push(y);
      rx.push(cx + rr);
      ry.push(y);
    }
    // 下端张开成小喇叭口（与水面/自身平滑衔接）
    const foot = Math.max(5, or * 0.8);
    lx[0] -= foot * 0.5;
    rx[0] += foot * 0.5;
  }

  /** 水体部分（从 x0 扫到 x1，含底部闭合） */
  const waterRun = (from: number, to: number): string => {
    let d = "";
    let started = false;
    for (let i = 0; i <= N; i += 1) {
      if (wx[i] < from) continue;
      if (wx[i] > to) break;
      d += `L${f1(wx[i])} ${f1(wy[i])}`;
      started = true;
    }
    if (!started) d += `L${f1(from)} ${f1(waveY(from, t, surface, amp, ox, w))}`;
    return d;
  };

  if (!hasColumn) {
    /* A：只有水体 */
    clip = `M${f1(-2)} ${f1(h + 2)}L${f1(-2)} ${f1(wy[0])}${waterRun(-2, w + 2)}L${f1(w + 2)} ${f1(h + 2)}Z`;
    sheen = `M${f1(-2)} ${f1(wy[0])}${waterRun(-2, w + 2)}`;
  } else if (connected) {
    /* B：棒棒糖 —— 水面左段 → 沿左缘上按钮 → 越过按钮 → 沿右缘回水面 → 水面右段 */
    const xEnter = lx[0];
    const xExit = rx[0];
    clip = `M${f1(-2)} ${f1(h + 2)}L${f1(-2)} ${f1(wy[0])}`;
    clip += waterRun(-2, xEnter);
    for (let i = 0; i < lx.length; i += 1) clip += `L${f1(lx[i])} ${f1(ly[i])}`;
    // 出水口：按钮下方一段圆弧（液体自按钮下缘渗出）
    clip += `Q${f1(ox)} ${f1(oy - or * 0.5)} ${f1(rx[rx.length - 1])} ${f1(ry[ry.length - 1])}`;
    for (let i = rx.length - 1; i >= 0; i -= 1) clip += `L${f1(rx[i])} ${f1(ry[i])}`;
    clip += waterRun(xExit, w + 2);
    clip += `L${f1(w + 2)} ${f1(h + 2)}Z`;

    sheen = `M${f1(-2)} ${f1(wy[0])}${waterRun(-2, xEnter)}`;
    for (let i = 0; i < lx.length; i += 1) sheen += `L${f1(lx[i])} ${f1(ly[i])}`;
    sheen += `M${f1(rx[rx.length - 1])} ${f1(ry[ry.length - 1])}`;
    for (let i = rx.length - 1; i >= 0; i -= 1) sheen += `L${f1(rx[i])} ${f1(ry[i])}`;
    sheen += waterRun(xExit, w + 2);
  } else {
    /* C：液柱悬空 —— 水体与液柱是两个独立子路径（nonzero 并集），中间不连 */
    clip = `M${f1(-2)} ${f1(h + 2)}L${f1(-2)} ${f1(wy[0])}${waterRun(-2, w + 2)}L${f1(w + 2)} ${f1(h + 2)}Z`;
    // 液柱封闭轮廓：左缘下→上，兜过按钮，右缘上→下，再兜过柱底
    clip += `M${f1(lx[0])} ${f1(ly[0])}`;
    for (let i = 1; i < lx.length; i += 1) clip += `L${f1(lx[i])} ${f1(ly[i])}`;
    clip += `Q${f1(ox)} ${f1(oy - or * 0.5)} ${f1(rx[rx.length - 1])} ${f1(ry[ry.length - 1])}`;
    for (let i = rx.length - 2; i >= 0; i -= 1) clip += `L${f1(rx[i])} ${f1(ry[i])}`;
    clip += `Z`; // 柱底由 lx[0] → rx[0] 的闭合边构成（喇叭口）

    sheen = `M${f1(-2)} ${f1(wy[0])}${waterRun(-2, w + 2)}`;
    sheen += `M${f1(lx[0])} ${f1(ly[0])}`;
    for (let i = 1; i < lx.length; i += 1) sheen += `L${f1(lx[i])} ${f1(ly[i])}`;
    sheen += `M${f1(rx[rx.length - 1])} ${f1(ry[ry.length - 1])}`;
    for (let i = rx.length - 2; i >= 0; i -= 1) sheen += `L${f1(rx[i])} ${f1(ry[i])}`;
  }

  /* ── 液柱上分离的液滴 ── */
  if (hasColumn && t > 0.02 && t < 0.42) {
    for (const d of droplets(seed, or)) {
      const dt = t - d.t0;
      if (dt <= 0) continue;
      const y = oy + or + d.vy0 * dt + 0.5 * DROP_G * dt * dt * 0.55;
      if (y > surface + 3) continue; // 已入水
      const x = ox + d.x0 + d.vx * dt;
      const stretch = 1 + Math.min(0.9, dt * 4.5);
      clip += ellipse(x, y, d.r / stretch, d.r * stretch);
    }
  }

  /* ── 撞底飞溅 ── */
  const sinceImpact = span(t, STAGE_BOUNDS.pour, STAGE_BOUNDS.pour + 0.1);
  if (t >= STAGE_BOUNDS.pour && sinceImpact < 1) {
    for (const s of splash(seed)) {
      const dt = (sinceImpact - s.lag) * 0.62;
      if (dt <= 0) continue;
      const x = ox + Math.cos(s.a) * s.v * dt;
      const y = h + Math.sin(s.a) * s.v * dt + 0.5 * DROP_G * dt * dt;
      const r = s.r * (1 - sinceImpact * 0.55);
      if (r <= 0.5 || y > h + 4) continue;
      clip += ellipse(x, y, r, r * 1.15);
    }
  }

  return {
    clip,
    // sealed 之后水线已出屏，清空 sheen 省一次整屏描边
    sheen: sealed ? "" : sheen,
    surfaceY: surface,
    coverage,
    flow,
    sheenAlpha,
    sealed,
  };
}
