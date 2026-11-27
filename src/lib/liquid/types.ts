/* [lc-2433] 液态玻璃 tab 栏的类型与常量（折射实现见 lensFilter.ts 头注释）。
   拖拽 / 弹簧相关的常量已随动效回退一并删除。 */

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

/** 滤镜 id（注入到常驻 <svg> 里，与 CSS 的 url(#id) 对应） */
export const FILTER_IDS = {
  panel: "anibox-lens-panel",
} as const;

/** reduce-motion / 不支持滤镜时的降级：不用折射，只用透明度与位移 */
export const FALLBACK_CLASS = "nav-cap-plain";
