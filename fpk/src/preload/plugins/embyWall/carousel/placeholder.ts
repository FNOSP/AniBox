// embyWall/carousel/placeholder.ts — 轮播图片骨架占位 + 主题自适应底色 + 加载完成淡入
// ─────────────────────────────────────────────────────────────────────────────
// 背景（本次优化）：
//   1. 轮播海报在「图片尚未拉取/解码完成」期间，容器底图此前是写死的近黑渐变
//      （#0b1219 / #10141c），在浅色主题下显得突兀，且加载失败会留下黑块。
//   2. 这里统一提供：主题自适应占位渐变 + 灰阶微光骨架 + 图片层淡入。
//      - 底色按页面实际明暗（isSurfaceDark）取色，不再写死黑色；
//      - 图片就绪后追加一层 .fntv-bg-layer 淡入，骨架微光随之移除；
//      - 拉取失败/无图时移除微光、保留主题占位色（绝不留黑块）。
// 只被 carousel 内部使用，不依赖任何 DOM 之外的运行时。

import { isSurfaceDark } from '../theme';

const SKEL_STYLE_ID = 'fntv-carousel-img-skel-style';
const LAYER_CLASS = 'fntv-bg-layer';
const LOADING_CLASS = 'fntv-img-loading';

/** 注入骨架/图片层样式（幂等）。 */
export function ensurePlaceholderCss(): void {
  if (document.getElementById(SKEL_STYLE_ID)) return;
  const st = document.createElement('style');
  st.id = SKEL_STYLE_ID;
  st.textContent = `
@keyframes fntv-img-shimmer{0%{transform:translateX(-120%)}100%{transform:translateX(120%)}}
/* 图片加载中的骨架：主题自适应底色由内联 background 给出，这里只负责微光扫过 */
.${LOADING_CLASS}{position:relative;overflow:hidden}
.${LOADING_CLASS}::after{
  content:'';position:absolute;inset:0;z-index:1;pointer-events:none;
  transform:translateX(-120%);animation:fntv-img-shimmer 1.4s ease-in-out infinite;
  background:linear-gradient(90deg,transparent,rgba(255,255,255,.16),transparent);
}
.${LOADING_CLASS}[data-fntv-skel="light"]::after{
  background:linear-gradient(90deg,transparent,rgba(255,255,255,.72),transparent);
}
/* 真实图片层：叠在占位底色之上，加载完成后淡入 */
.${LAYER_CLASS}{
  position:absolute;inset:0;z-index:0;
  background-size:cover;background-position:center 25%;
  opacity:0;transition:opacity .5s ease;pointer-events:none;
}
@media (prefers-reduced-motion: reduce){
  .${LOADING_CLASS}::after{animation:none;background:transparent}
  .${LAYER_CLASS}{transition:none}
}
`;
  (document.head || document.documentElement).appendChild(st);
}

/** 主题自适应占位渐变（替代写死的 #0b1219 / #10141c）。
 *  accent 为可选的每片主题色（如 '#6eb5ff'），用于保留原有五色循环观感。 */
export function carouselPlaceholderGradient(accent?: string, dark?: boolean): string {
  const isDark = dark === undefined ? isSurfaceDark() : dark;
  const base = isDark ? 'rgba(30,34,50,.92)' : 'rgba(232,237,246,.96)';
  if (accent) return `linear-gradient(160deg, ${accent}55, ${base})`;
  return `linear-gradient(160deg, ${isDark ? 'rgba(58,64,90,.55)' : 'rgba(210,218,232,.9)'}, ${base})`;
}

/** 标记目标进入「加载中」：加骨架微光 + 若尚无底色则铺主题占位渐变。 */
export function markBackdropLoading(target: HTMLElement, dark?: boolean): void {
  ensurePlaceholderCss();
  const isDark = dark === undefined ? isSurfaceDark() : dark;
  target.classList.add(LOADING_CLASS);
  target.setAttribute('data-fntv-skel', isDark ? 'dark' : 'light');
  const cur = target.style.backgroundImage;
  if (!cur || cur === 'none') target.style.backgroundImage = carouselPlaceholderGradient(undefined, isDark);
}

/** 停止骨架微光，保留（或恢复）主题占位底色。 */
export function clearBackdropLoading(target: HTMLElement, dark?: boolean): void {
  target.classList.remove(LOADING_CLASS);
  target.removeAttribute('data-fntv-skel');
  const cur = target.style.backgroundImage;
  if (!cur || cur === 'none') target.style.backgroundImage = carouselPlaceholderGradient(undefined, dark);
}

/** 目标是否已经有真实背景图（用于样式5 的竖版兜底判断）。 */
export function hasBackdropImage(target: HTMLElement): boolean {
  const layer = target.querySelector(':scope > .' + LAYER_CLASS) as HTMLElement | null;
  return !!(layer && layer.style.backgroundImage && layer.style.backgroundImage !== 'none');
}

function getImageLayer(target: HTMLElement): HTMLElement {
  ensurePlaceholderCss();
  let layer = target.querySelector(':scope > .' + LAYER_CLASS) as HTMLElement | null;
  if (!layer) {
    if (getComputedStyle(target).position === 'static') target.style.position = 'relative';
    layer = document.createElement('div');
    layer.className = LAYER_CLASS;
    target.appendChild(layer);
  }
  return layer;
}

function preloadImage(url: string): Promise<boolean> {
  return new Promise((resolve) => {
    try {
      const im = new Image();
      let settled = false;
      const done = (ok: boolean): void => { if (!settled) { settled = true; resolve(ok); } };
      im.onload = () => done(true);
      im.onerror = () => done(false);
      im.src = url;
      if (im.complete && im.naturalWidth > 0) done(true);
    } catch (e) {
      resolve(false);
    }
  });
}

/** 把真实背景图铺到目标上并淡入；解码成功后移除骨架微光。
 *  position 可选（样式5 竖版兜底用 'center 20%'）。 */
export function applyBackdropImage(
  target: HTMLElement,
  url: string,
  opts?: { position?: string }
): void {
  if (!url) return;
  const layer = getImageLayer(target);
  if (opts && opts.position) layer.style.backgroundPosition = opts.position;
  preloadImage(url).then((ok) => {
    if (!ok) return; // 解码失败：保留主题占位，不淡入（绝不出现黑块/空白）
    layer.style.backgroundImage = `url("${url}")`;
    const show = (): void => { layer.style.opacity = '1'; };
    requestAnimationFrame(show);
    // [fix-carousel-regress] 兜底：后台标签页 / 合成器节流时 rAF 可能迟迟不回调，
    //   图片已就绪却停在 opacity:0（表现为「海报永远不显示」）。定时器保证一定揭示。
    window.setTimeout(show, 80);
    target.classList.remove(LOADING_CLASS);
    target.removeAttribute('data-fntv-skel');
  });
}
