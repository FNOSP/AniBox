/**
 * [ui-opacity] 界面透明度链路的**纯计算部分**（无 DOM / 无 Tauri 依赖，可直接单测）。
 *
 * 设置面板「外观 → 界面透明度」的取值口径集中在这里：
 *   · 取值范围 60–100（与 Rust `config.rs` 的 UI_OPACITY_MIN/MAX、`set_settings` 的 clamp 同口径）
 *   · 默认 85（历史默认值，见项目记忆「透明度默认 85%」）
 *   · 侧栏玻璃比外壳略实 4%（保证导航文字可读），但不超过 100%
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
 */
export function computeUiAlpha(raw: unknown): { surface: number; sidebar: number } {
  const surface = normalizeUiOpacity(raw);
  return {
    surface,
    sidebar: Math.min(UI_OPACITY_MAX, surface + SIDEBAR_ALPHA_BONUS),
  };
}
