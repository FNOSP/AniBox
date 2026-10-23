/**
 * [ui-opacity] 界面透明度链路的**纯计算部分**（无 DOM / 无 Tauri 依赖，可直接单测）。
 *
 * **产品要求：界面/侧边栏透明度锁死 85**，不再受设置滑块影响（滑块已从设置面板移除）。
 *   · 锁定值 85（`UI_OPACITY_LOCKED`），无论入参是什么都恒用 85
 *   · 侧栏玻璃比外壳略实 4% → `--ui-sidebar-alpha: 89%`
 *   · 60–100 的常量仅为兼容旧配置/旧代码保留，不再作为可调范围
 *
 * App 只负责把这里算出的百分比写到根元素 CSS 变量；CSS 侧用
 * `rgb(R G B / var(--ui-*-alpha))` 读取。**不要**再退回 `color-mix()`：那需要
 * WebView2 ≥ 111，而本应用声明的最低 WebView2 是 109 —— 在 109/110 上
 * `@supports (color-mix)` 为假，会落到固定 alpha 的兜底值，导致「调了没反应」。
 */

/** 允许的最小透明度（百分比）。 */
export const UI_OPACITY_MIN = 60;
/** 允许的最大透明度（百分比）。 */
export const UI_OPACITY_MAX = 100;
/** 历史默认值（百分比）。 */
export const UI_OPACITY_DEFAULT = 85;
/** [产品要求] 界面/侧边栏透明度锁死值：不再受设置滑块影响。 */
export const UI_OPACITY_LOCKED = 85;
/** 侧栏玻璃相对外壳多出的不透明度（百分比）。 */
export const SIDEBAR_ALPHA_BONUS = 4;

/** 把任意来源的值归一成合法的界面透明度百分比（非法值 → 默认 85）。 */
export function normalizeUiOpacity(raw: unknown): number {
  // null / undefined / 空串都视为「没有设置」，回默认（注意 Number(null)===0、Number("")===0，
  // 不先挡掉会被当成 0 再夹到最小值 60，与「缺省用默认 85」的语义不符）。
  if (raw === null || raw === undefined || raw === "") return UI_OPACITY_DEFAULT;
  const n = Number(raw);
  if (!Number.isFinite(n)) return UI_OPACITY_DEFAULT;
  return Math.min(UI_OPACITY_MAX, Math.max(UI_OPACITY_MIN, Math.round(n)));
}

/**
 * 计算要写入 CSS 变量的两个 alpha：
 *   surface = 外壳底色（.app-shell）
 *   sidebar = 侧栏玻璃（.sidebar），比外壳略实 4%，封顶 100
 *
 * **产品要求：锁死 85** —— 入参被忽略，恒用 `UI_OPACITY_LOCKED`，
 * 这样即使 config.json / localStorage 里存了 60/100 等旧值，界面也不会变。
 */
export function computeUiAlpha(_raw?: unknown): { surface: number; sidebar: number } {
  const surface = UI_OPACITY_LOCKED;
  return {
    surface,
    sidebar: Math.min(UI_OPACITY_MAX, surface + SIDEBAR_ALPHA_BONUS),
  };
}
