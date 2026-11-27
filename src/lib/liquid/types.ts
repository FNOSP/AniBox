/* [lc-2431] 液态玻璃 tab 栏的类型与常量（照抄源见 spring.ts / lensFilter.ts 头注释）。 */

/** 一块"玻璃"的几何：CSS 像素 */
export interface GlassRect {
  width: number;
  height: number;
  radius: number;
}

/** 一条 SVG 位移滤镜的完整定义 */
export interface LensFilterSpec {
  id: string;
  rect: GlassRect;
  /** 折射作用带宽度 */
  refractionHeight: number;
  /** 最大位移量 */
  refractionAmount: number;
  /** 向心分量 0|1 */
  depthEffect: number;
}

/**
 * 面板（整条 tab 栏）——照抄 LiquidBottomTabs 的 `lens(24f.dp.toPx(), 24f.dp.toPx())`。
 * 我们的胶囊高 40px（内高 38），24px 的作用带会覆盖大半高度：
 * 这正是原版"整条都是一块透镜"的观感，保留。
 */
export const PANEL_REFRACTION = { height: 24, amount: 24, depth: 1 } as const;

/**
 * 滑块（选中指示器）——原版 `lens(10dp * progress, 14dp * progress, chromaticAberration = true)`：
 * **按下才出现**，progress 从 0 长到满。静止时折射量恒为 0（没有折射）。
 * 浏览器侧 scale 不能实时改（tests/probe_filter_align_2431.cjs 已证），
 * 所以只预生成"满进度"这一条，按下时把 url() 挂上去 / 摘下来。
 */
export const THUMB_PRESS_REFRACTION = { height: 10, amount: 14, depth: 1 } as const;

/** 滤镜 id（注入到常驻 <svg> 里，与 CSS 的 url(#id) 对应） */
export const FILTER_IDS = {
  panel: "anibox-lens-panel",
  thumbPress: "anibox-lens-thumb-press",
} as const;

/** 按压时滑块放大倍数 —— **逐值照抄**原版 `pressedScale = 78f / 56f`（DampedDragAnimation 构造参数）。
 *  原版滑块 56dp 恰好等于 64dp 栏体的内容高，放大后 78dp 会鼓出栏体 7dp/侧 ——
 *  这个"液体鼓包"就是该效果的签名手感，不能为了不溢出而压小。 */
export const PRESSED_SCALE = 78 / 56;

/** 拖动判定阈值（px）：小于它算点击，不进入拖拽 —— 避免点按变拖拽 */
export const DRAG_THRESHOLD = 6;

/** reduce-motion / 不支持滤镜时的降级：不用折射，只用透明度与位移 */
export const FALLBACK_CLASS = "nav-cap-plain";
