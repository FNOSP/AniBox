/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */

export function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** [lc-2135] 与 01-tokens.css 的 --dur-* 一一对应的 JS 侧时长（ms）。
 *  弹层「先播退场动画、再真正卸载」的那类定时器必须用这里的值 —— 否则 CSS 调快了，
 *  JS 还在原地多等一截，观感就是"点了关闭没反应"。改档位时两处一起改。
 *  [lc-2135] 第二版整体放慢（130/170/230 → 165/210/285）：第一版压得太快，
 *  用户实测「整体太快、轮播海报尤其太急」。hero 档对应 --dur-hero，
 *  目前只有首页轮播（CSS 横移/文案 + JS 克隆位兜底）在消费。 */
export const DUR = {
  /** 高频交互 / 弹层关闭 */
  tap: 165,
  /** 常规运动 / 页窗退场 */
  move: 210,
  /** 大位移 / 整页转场 */
  page: 285,
  /** 首页全屏轮播（--dur-hero）：CSS 侧海报横移用时；JS 侧克隆位复位的兜底
   *  定时器用它推导（必须 > CSS 过渡时长，否则会在动画中途跳一下） */
  hero: 650,
} as const;

/** [lc-123-rev2] 「首页 | 发现」胶囊：**从顶栏挪到内容区顶部的浮层**（.navbar = absolute，
 *  不占布局高度，直接浮在首页海报上；见 App.css）。顶栏恢复干净：左边只留当前页名
 *  （+侧栏/刷新/主题按钮），中间弱化品牌小字，右边窗口按钮。
 *  [lc-123-rev6] 文字 → **内联 SVG 图标**（房子/放大镜）；高亮不再是"直接跳过去"，而是
 *  一块会滑动的**水滴指示器**（等宽两槽 translateX(0|100%) + 过冲回弹缓动），见 App.css。
 *  图标按钮必须带 aria-label + title（无文字时读屏与悬停都要认得出）。
 *  胶囊仍带 `-webkit-app-region: no-drag`（见 App.css .nav-cap）：虽已不贴拖拽区，
 *  这道声明是"胶囊一定点得动"的保险丝（顶栏整条仍是 data-tauri-drag-region 拖拽区）。 */
