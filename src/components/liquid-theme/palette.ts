/* [liquid-theme] 配色与主题基色。
 *
 * 需求给定的调色板（新项目直接用它；本仓库是既有项目，只在演示页与"液体高光"里用，
 * 不去改 AniBox 自己的 token —— 需求明确要求"已有项目优先兼容现有主题变量"）。
 */

export const LIQUID_PALETTE = {
  paper: "#F1F0ED",
  ink: "#141319",
  inkDeep: "#060609",
  lilac: "#F4F2F8",
  ember: "#E07A2E",
  azure: "#2F6BFF",
  teal: "#14A3A8",
} as const;

/** 揭示层的**不透明**底：液体盖住的地方必须是实色，否则会透出旧主题。
 *  这是模块默认值；AniBox 接入时传自己 token 对应的值（#f5f5f7 / #1c1c1e）。 */
export const LIQUID_BACKING = {
  light: LIQUID_PALETTE.paper,
  dark: LIQUID_PALETTE.inkDeep,
} as const;

/** 液面高光（湿边）：跟随方向，浅色揭示用深一点的墨色会更"水"，
 *  深色揭示用白色低 alpha。都不引入新色相。 */
export const LIQUID_SHEEN = {
  // 湿边是"水的边缘"，必须比背景本身更亮（浅色揭示层上是白色高光，
  // 深色揭示层上是低 alpha 白）。0.10 在浅色背景上完全看不见（实测），
  // 所以浅色用白色高光 + 深色用更高 alpha 的白。
  light: "rgba(255, 255, 255, 0.85)",
  dark: "rgba(255, 255, 255, 0.22)",
} as const;
