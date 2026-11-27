/* [lc-2431] 把透镜位移图注册成常驻 SVG 滤镜，供 CSS `backdrop-filter: url(#id)` 引用。
   为什么必须常驻（不能每次渲染新建）：backdrop-filter 引用的滤镜在**引用它的元素重绘时**
   才重新求值；若滤镜节点被 React 卸载/替换，Chromium 会短暂丢掉滤镜 → 闪一帧。
   所以只在首次挂载时建一次，之后只改 <feImage href>。 */

import { buildLensDisplacementMap, type LensParams } from "./lensFilter";
import type { LensFilterSpec } from "./types";

const SVG_NS = "http://www.w3.org/2000/svg";
const CONTAINER_ID = "anibox-lens-filters";

/** feDisplacementMap 的 scale：位移图按这个跨度编码，两端必须一致 */
const SPAN = 48;

function ensureContainer(): SVGSVGElement {
  let svg = document.getElementById(CONTAINER_ID) as SVGSVGElement | null;
  if (svg) return svg;
  svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("id", CONTAINER_ID);
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("width", "0");
  svg.setAttribute("height", "0");
  // 不能用 display:none / visibility:hidden —— 部分实现会跳过隐藏滤镜的求值
  svg.style.cssText =
    "position:absolute;width:0;height:0;overflow:hidden;pointer-events:none;inset:0;";
  document.body.appendChild(svg);
  return svg;
}

/** 建/更新一条滤镜。同 id 已存在则只换位移图，避免重建导致闪帧。 */
export function upsertLensFilter(spec: LensFilterSpec): boolean {
  const params: LensParams = {
    width: spec.rect.width,
    height: spec.rect.height,
    radius: spec.rect.radius,
    refractionHeight: spec.refractionHeight,
    refractionAmount: spec.refractionAmount,
    depthEffect: spec.depthEffect,
  };
  const dataUrl = buildLensDisplacementMap(params, SPAN);
  if (!dataUrl) return false;

  const svg = ensureContainer();
  let filter = svg.querySelector<SVGFilterElement>(`filter[id="${spec.id}"]`);

  if (!filter) {
    filter = document.createElementNS(SVG_NS, "filter");
    filter.setAttribute("id", spec.id);
    filter.setAttribute("filterUnits", "userSpaceOnUse");
    filter.setAttribute("color-interpolation-filters", "sRGB");

    const feImage = document.createElementNS(SVG_NS, "feImage");
    feImage.setAttribute("preserveAspectRatio", "none");
    feImage.setAttribute("result", "disp");
    feImage.dataset.role = "map";
    feImage.setAttribute("href", dataUrl);

    const feDisp = document.createElementNS(SVG_NS, "feDisplacementMap");
    feDisp.setAttribute("in", "SourceGraphic");
    feDisp.setAttribute("in2", "disp");
    feDisp.setAttribute("scale", String(SPAN));
    feDisp.setAttribute("xChannelSelector", "R");
    feDisp.setAttribute("yChannelSelector", "G");

    filter.append(feImage, feDisp);
    svg.appendChild(filter);
  } else {
    const feImage = filter.querySelector<SVGFEImageElement>('[data-role="map"]');
    if (feImage && feImage.getAttribute("href") !== dataUrl) {
      feImage.setAttribute("href", dataUrl);
    }
  }

  // userSpaceOnUse 的原点 = 元素 border-box 左上角（已实测，见 probe_filter_align_2431）
  filter.setAttribute("x", "0");
  filter.setAttribute("y", "0");
  filter.setAttribute("width", String(spec.rect.width));
  filter.setAttribute("height", String(spec.rect.height));
  return true;
}


/** 运行时能力探测：不支持 SVG 滤镜折射时走降级外观 */
export function supportsBackdropFilterUrl(): boolean {
  if (typeof CSS === "undefined" || !CSS.supports) return false;
  if (!CSS.supports("backdrop-filter", "blur(1px)")) return false;
  // Chromium 对 url() 的支持无法用 CSS.supports 判定（它会一律返回 true），
  // 因此这里只做粗筛；真实能力由首次挂载后的 `data-lens="on"` 标记 + 视觉验证保证。
  return true;
}
