/* [liquid-theme] 揭示层引擎（不依赖 React）。
 *
 * 层级（决定了"液体经过哪里，哪里才变主题"为什么成立）：
 *   z 1602 .lt-block    透明，动画期间吞掉指针事件（防点到过期底层内容）
 *   z 1601 .lt-sheen    只描"水的上沿"的 1.5px 湿边（让人看出那里是水面）
 *   z 1600 .lt-reveal   新主题层，clip-path: url(#id) → 液体轮廓内显示新主题界面
 *          └ .lt-backing  不透明底（液体边缘内先铺实色，防止透出旧主题）
 *          └ .lt-host     带新主题作用域；内部是与真实界面**结构与样式完全对应**的视觉快照
 *   z 1599 .lt-defs     只放 <clipPath> 的 SVG（不可见、不吃事件）
 *   底层                = 真实界面（旧主题），一个字节都没动
 *
 * 为什么用视觉快照而不是"渲染两套 React 树"：
 *   双树要复制全部业务状态（两棵树会各自分叉），或在每个组件里加"影子模式"（等于重写项目）。
 *   克隆 DOM 一次即可：克隆体不在 React 根内 → **没有任何 React 事件监听被绑定**（React 19
 *   用根容器委托，克隆节点不在树上），id/for/aria-* 全部剥离 → 不产生重复 ID，
 *   inert + aria-hidden → 不进键盘焦点与读屏顺序。
 *
 * 快照支持范围（不把静态快照说成能自动重绘）：
 *   ✅ <img>/CSS 背景图/SVG/纯 CSS 图表：连同样式一起克隆，浏览器按新主题变量重绘
 *      （这正是"液体经过处变主题"的实现方式：同一份 DOM + 另一套 CSS 变量作用域）
 *   ✅ 表单控件：复制当前 value/checked/range 状态（一次性读属性，不读布局）
 *   ⚠️ <canvas> / <video> / <iframe> / Shadow DOM：克隆体是空壳或冻结帧，不自动重绘。
 *      本仓库 UI 无这三类元素（grep 已确认），故不受影响；将来引入需按 README 自行提供快照。
 *   ✅ 滚动位置：克隆后按源容器逐个同步；resize 时重新同步。
 */

import { buildLiquidShape } from "./geometry.ts";
import { LIQUID_SHEEN } from "./palette.ts";
import { clamp01, stageAt } from "./timeline.ts";
import type {
  LiquidDirection,
  LiquidFrameSnapshot,
  LiquidOrigin,
  LiquidRevealConfig,
  LiquidStage,
  LiquidTheme,
} from "./types.ts";

const SVG_NS = "http://www.w3.org/2000/svg";

/** 不参与视觉快照的节点：脚本/样式表元信息（样式通过 class 作用于克隆体，无需重复） */
const SKIP_TAGS = new Set(["SCRIPT", "LINK", "META", "TITLE", "BASE", "NOSCRIPT", "STYLE"]);

/** 需要从克隆体中剥掉的属性类别 */
function shouldDropAttr(name: string): boolean {
  if (name.startsWith("on")) return true; // 事件属性
  if (name.startsWith("aria-")) return true; // 无障碍语义整体由 aria-hidden 接管
  if (name === "id" || name === "for" || name === "form" || name === "name") return true; // 重复 ID / 表单关联
  if (name === "autofocus" || name === "contenteditable" || name === "tabindex") return true;
  if (name === "data-tauri-drag-region") return true; // 别让克隆体被当成窗口拖拽区
  if (name === "href" || name === "xlink:href") return true; // 不留可导航引用
  return false;
}

/** 表单控件：把**当前值**从属性/property 同步成展示态（一次性读，不读布局） */
function syncControlState(src: Element, dst: HTMLElement): void {
  const tag = src.tagName;
  if (tag === "INPUT") {
    const a = src as HTMLInputElement;
    if (a.type === "checkbox" || a.type === "radio") {
      if (a.checked) dst.setAttribute("checked", "");
    } else if (a.type === "range" || a.type === "number") {
      dst.setAttribute("value", a.value);
      if (a.min) dst.setAttribute("min", a.min);
      if (a.max) dst.setAttribute("max", a.max);
      if (a.step) dst.setAttribute("step", a.step);
    } else if (a.type !== "hidden" && a.type !== "file" && a.type !== "password") {
      dst.setAttribute("value", a.value);
      if (a.placeholder) dst.setAttribute("placeholder", a.placeholder);
    }
  } else if (tag === "TEXTAREA") {
    dst.textContent = (src as HTMLTextAreaElement).value ?? "";
  } else if (tag === "SELECT") {
    const sel = src as HTMLSelectElement;
    const opt = sel.selectedOptions && sel.selectedOptions[0];
    dst.textContent = opt ? opt.textContent : "";
  }
}

/** HTML 元素：新建同名标签，只搬"影响外观"的属性 */
function cloneHtmlElement(src: Element, doc: Document, focusable: string[]): HTMLElement {
  const tag = src.tagName.toLowerCase();
  const dst = doc.createElement(tag);
  for (const { name, value } of Array.from(src.attributes)) {
    if (shouldDropAttr(name)) continue;
    if (name.startsWith("data-")) {
      dst.setAttribute(name, value);
      continue;
    }
    if (name === "class" || name === "style") {
      dst.setAttribute(name, value);
      continue;
    }
    if (name === "src" || name === "srcset" || name === "alt" || name === "type") {
      dst.setAttribute(name, value);
      continue;
    }
    if (name === "disabled" || name === "readonly") {
      dst.setAttribute(name, value);
      continue;
    }
    // 其余（placeholder/title/role/value/…）不复制：placeholder 会露出提示语，
    // value 由 syncControlState 按当前值重写，role 会重复进无障碍树
  }
  syncControlState(src, dst);
  const t = src.tagName;
  if (t === "BUTTON" || t === "INPUT" || t === "TEXTAREA" || t === "SELECT" || t === "A") {
    // 克隆体内的控件一律移出 Tab 序列（宿主已 inert，这里再加一道显式保险）
    focusable.push(t);
    dst.setAttribute("tabindex", "-1");
  }
  return dst;
}

/** SVG 子树：整体 import（命名空间/大量表现属性都对），但剥掉 id 与事件 */
function cloneSvgTree(src: Element, doc: Document): Node {
  const copy = doc.importNode(src, true) as Element;
  const all: Element[] = [copy, ...Array.from(copy.querySelectorAll("*"))];
  for (const el of all) {
    for (const a of Array.from(el.attributes)) {
      const n = a.name;
      // SVG 里的 id 与 url(#…) 引用：剥掉 id 后，引用仍会解析到**原文档里那一个**
      // （原元素还在，没被删）→ 渐变/裁剪依然生效，且不会出现重复 id
      if (n === "id" || n.startsWith("on")) el.removeAttribute(n);
    }
  }
  return copy;
}

function cloneNodeShallowAware(src: Node, doc: Document, focusable: string[]): Node | null {
  if (src.nodeType === Node.TEXT_NODE) return doc.createTextNode(src.textContent ?? "");
  if (src.nodeType !== Node.ELEMENT_NODE) return null;
  const el = src as Element;
  if (SKIP_TAGS.has(el.tagName)) return null;
  if (el.tagName.toLowerCase() === "svg" || el.namespaceURI === SVG_NS) {
    return cloneSvgTree(el, doc);
  }
  const dst = cloneHtmlElement(el, doc, focusable);
  const kids = el.childNodes;
  for (let i = 0; i < kids.length; i += 1) {
    const c = cloneNodeShallowAware(kids[i], doc, focusable);
    if (c) dst.appendChild(c);
  }
  return dst;
}

export type LiquidRevealHandle = {
  /** 开始播放（幂等：重复调用无效） */
  start: () => void;
  /** 中断并清理（音效收到 interrupted，不播完成音） */
  interrupt: () => void;
  /** 立即完成到终态（重复点击时"快进"，不会锁死交互） */
  finishNow: () => void;
  /** 彻底销毁（卸载/切视图） */
  destroy: () => void;
  /** 最近一帧（调试/测试） */
  readonly frame: LiquidFrameSnapshot | null;
  /** 是否还在播放 */
  readonly active: boolean;
};

export type LiquidRevealDeps = {
  doc?: Document;
  raf?: (cb: (ts: number) => void) => number;
  cancelRaf?: (id: number) => void;
  /** 用宿主的真实尺寸（默认 documentElement.clientWidth/Height） */
  viewport?: () => { w: number; h: number };
  /** 给宿主挂新主题作用域（AniBox 里 = data-theme 属性） */
  applyThemeScope: (el: HTMLElement, theme: LiquidTheme) => void;
};

export type CreatedLiquidReveal = { handle: LiquidRevealHandle; root: HTMLElement };

function emptyFrame(direction: LiquidDirection): LiquidFrameSnapshot {
  return { progress: 0, stage: "idle", direction, flow: 0, surfaceY: 0, coverage: 0 };
}

export function createLiquidReveal(
  cfg: LiquidRevealConfig,
  deps: LiquidRevealDeps,
): CreatedLiquidReveal {
  const doc = deps.doc ?? document;
  const raf = deps.raf ?? ((cb: (ts: number) => void) => window.requestAnimationFrame(cb));
  const cancelRaf = deps.cancelRaf ?? ((id: number) => window.cancelAnimationFrame(id));
  const viewport =
    deps.viewport ??
    (() => ({
      w: doc.documentElement.clientWidth || window.innerWidth,
      h: doc.documentElement.clientHeight || window.innerHeight,
    }));

  const direction: LiquidDirection = cfg.direction;
  const targetTheme: LiquidTheme = direction === "darken" ? "dark" : "light";
  const origin: LiquidOrigin = { ...cfg.origin };

  /* ── DOM 骨架 ── */
  const root = doc.createElement("div");
  root.className = "lt-stage";
  root.setAttribute("data-lt-direction", direction);
  root.setAttribute("aria-hidden", "true");

  const defsSvg = doc.createElementNS(SVG_NS, "svg");
  defsSvg.setAttribute("class", "lt-defs");
  defsSvg.setAttribute("aria-hidden", "true");
  defsSvg.setAttribute("focusable", "false");
  const clipPath = doc.createElementNS(SVG_NS, "clipPath");
  clipPath.setAttribute("id", cfg.clipId);
  clipPath.setAttribute("clipPathUnits", "userSpaceOnUse");
  const clipShape = doc.createElementNS(SVG_NS, "path");
  clipShape.setAttribute("d", "");
  clipPath.appendChild(clipShape);
  defsSvg.appendChild(clipPath);

  const reveal = doc.createElement("div");
  reveal.className = "lt-reveal";
  reveal.setAttribute("aria-hidden", "true");
  reveal.style.clipPath = `url(#${cfg.clipId})`;
  (reveal.style as CSSStyleDeclaration & { webkitClipPath?: string }).webkitClipPath = `url(#${cfg.clipId})`;

  const backing = doc.createElement("div");
  backing.className = "lt-backing";
  // 先用调用方给的值，挂载后再用宿主上**新主题作用域**算出的真实实底覆盖（见 syncBackingColor）
  backing.style.background = cfg.backingColor;

  const host = doc.createElement("div");
  host.className = "lt-host";
  host.setAttribute("aria-hidden", "true");
  host.setAttribute("inert", "");
  deps.applyThemeScope(host, targetTheme);

  reveal.appendChild(backing);
  reveal.appendChild(host);

  /* 湿边：两层描边同形叠在一起 —— 外层宽而淡（水面张力/反光的柔光边），
     内层 1.5px 实线（明确的水线）。单层 1.5px 在浅色主题下几乎看不见（实测截图）。 */
  const sheenSvg = doc.createElementNS(SVG_NS, "svg");
  sheenSvg.setAttribute("class", "lt-sheen");
  sheenSvg.setAttribute("aria-hidden", "true");
  const haloPath = doc.createElementNS(SVG_NS, "path");
  haloPath.setAttribute("fill", "none");
  haloPath.setAttribute("stroke", LIQUID_SHEEN[targetTheme]);
  haloPath.setAttribute("stroke-width", "7");
  haloPath.setAttribute("stroke-linecap", "round");
  haloPath.setAttribute("stroke-linejoin", "round");
  haloPath.setAttribute("opacity", "0.55");
  const sheenPath = doc.createElementNS(SVG_NS, "path");
  sheenPath.setAttribute("fill", "none");
  sheenPath.setAttribute("stroke", LIQUID_SHEEN[targetTheme]);
  sheenPath.setAttribute("stroke-width", "1.5");
  sheenPath.setAttribute("stroke-linecap", "round");
  sheenPath.setAttribute("stroke-linejoin", "round");
  sheenSvg.appendChild(haloPath);
  sheenSvg.appendChild(sheenPath);

  const block = doc.createElement("div");
  block.className = "lt-block";
  block.setAttribute("aria-hidden", "true");

  root.appendChild(defsSvg);
  root.appendChild(reveal);
  root.appendChild(sheenSvg);
  if (cfg.blockInteraction !== false) root.appendChild(block);

  /* ── 状态 ── */
  let started = false;
  let done = false;
  let rafId: number | null = null;
  let t0 = 0;
  let lastStage: LiquidStage = "idle";
  let frame: LiquidFrameSnapshot | null = null;
  /** [lc-2155] 是否已把 clip-path 切成 none（完全盖满后省掉每帧整屏栅格化） */
  let sealedApplied = false;
  /** [lc-2157] 完成后的淡出任务（卸载时要立刻执行，不能留一层在屏上） */
  let detachFade: (() => void) | null = null;
  let scrollSnaps: { el: Element; top: number; left: number }[] = [];
  let detachResize: (() => void) | null = null;
  let detachScroll: (() => void) | null = null;
  /** 克隆内容的内层包裹（整页滚动时整体平移，见 syncViewport） */
  let cloneInner: HTMLElement = doc.createElement("div");
  /** 克隆体内被改写成 relative 的 sticky 元素（需要按 scrollY 反向补偿） */
  let stickyClones: HTMLElement[] = [];

  const sourceEl = (): Element => {
    const t = cfg.snapshotTarget ? cfg.snapshotTarget() : doc.getElementById("root");
    return t ?? doc.body;
  };

  /* ── 快照：一次性克隆 + 滚动同步 ── */
  function captureScroll(src: Element): { el: Element; top: number; left: number }[] {
    const out: { el: Element; top: number; left: number }[] = [];
    const walk = (el: Element): void => {
      const h = el as HTMLElement;
      if (h.scrollTop || h.scrollLeft) out.push({ el, top: h.scrollTop, left: h.scrollLeft });
      const kids = el.children;
      for (let i = 0; i < kids.length; i += 1) walk(kids[i]);
    };
    walk(src);
    return out;
  }

  function childIndexPath(el: Element, rootEl: Element): number[] | null {
    const path: number[] = [];
    let cur: Element | null = el;
    while (cur && cur !== rootEl) {
      const parent: Element | null = cur.parentElement;
      if (!parent) return null;
      path.unshift(Array.prototype.indexOf.call(parent.children, cur));
      cur = parent;
    }
    return cur === rootEl ? path : null;
  }

  function syncScroll(): void {
    const src = sourceEl();
    for (const s of scrollSnaps) {
      const path = childIndexPath(s.el, src);
      if (!path || !path.length) continue;
      let target: Element = cloneInner;
      let ok = true;
      for (const idx of path) {
        const next: Element | undefined = target.children[idx];
        if (!next) {
          ok = false;
          break;
        }
        target = next;
      }
      if (!ok) continue;
      const h = target as HTMLElement;
      h.scrollTop = s.top;
      h.scrollLeft = s.left;
    }
  }

  /** 文档级滚动（window.scrollY）对齐。
   *
   *  两种宿主形态都必须支持：
   *    · 应用自己满高、内部容器滚动（AniBox：#root 100vh + overflow:hidden）→ scrollY 恒为 0，无操作
   *    · 普通网页整页滚动（演示页 / 可嵌入页面）→ 克隆体在 fixed 容器里不会跟着滚，
   *      必须把内容整体平移 -scrollY，否则揭示层显示的是"页面顶部"而底层已经滚到别处。
   *  sticky 元素额外处理：它们在真实页面里吸附在视口上，平移后要反向补偿回真实位置
   *  （用 relative + top:scrollY，而不是 fixed —— fixed 会被父级 transform 当作包含块）。 */
  function syncViewport(): void {
    const y = window.scrollY || doc.documentElement.scrollTop || 0;
    cloneInner.style.transform = y ? `translateY(${-y}px)` : "";
    for (let i = 0; i < stickyClones.length; i += 1) {
      stickyClones[i].style.top = y ? `${y}px` : "";
    }
  }

  function buildSnapshot(): void {
    while (host.firstChild) host.removeChild(host.firstChild);
    const src = sourceEl();
    const focusable: string[] = [];
    cloneInner = doc.createElement("div");
    cloneInner.style.cssText = "position:absolute;left:0;top:0;width:100%";
    stickyClones = [];
    const kids = Array.from(src.children);
    for (const k of kids) {
      if (k === root) continue;
      const c = cloneNodeShallowAware(k, doc, focusable);
      if (c) cloneInner.appendChild(c);
    }
    host.appendChild(cloneInner);
    // sticky / fixed 元素在克隆里不再吸附（容器不滚）：改成 relative，由 syncViewport 补偿
    const realAll = Array.from(src.querySelectorAll("*"));
    const cloneAll = Array.from(cloneInner.querySelectorAll("*"));
    for (let i = 0; i < realAll.length && i < cloneAll.length; i += 1) {
      const pos = (doc.defaultView?.getComputedStyle(realAll[i]) as CSSStyleDeclaration | null)?.position;
      if (pos === "sticky" || pos === "fixed") {
        const c = cloneAll[i] as HTMLElement;
        if (pos === "sticky") {
          c.style.position = "relative";
          stickyClones.push(c);
        } else {
          // fixed：相对视口定位，直接固定到它当前的视口位置
          const r = realAll[i].getBoundingClientRect();
          c.style.position = "fixed";
          c.style.top = `${r.top}px`;
          c.style.left = `${r.left}px`;
          if (r.width) c.style.width = `${r.width}px`;
        }
      }
    }
    host.style.width = `${doc.documentElement.clientWidth || src.clientWidth}px`;
    host.style.height = `${doc.documentElement.clientHeight || src.clientHeight}px`;
    scrollSnaps = captureScroll(src);
    syncScroll();
    syncViewport();
    syncBackingColor();
    freezeAnimations();
  }

  /** 把克隆体里**所有正在运行的 CSS 动画直接跳到终态并停住**。
   *
   *  [lc-2156] 这是「整体控件和按钮会闪一下才切换」的真因：
   *    node.cloneNode 出来的副本会被浏览器当成**刚插入的新元素**，于是把
   *    `animation` 声明**从头重播一遍**。实测克隆体的 `.app-shell` 上：
   *      animationName = "appIn"，opacity = 0.808，transform = scale(1.00154)
   *    —— 也就是液体揭开的区域里，那套界面正在"淡入"，观感就是控件先半透明、
   *    再闪一下才变成新主题。同理 `.view-pane.view-in`（viewIn，透明 + 位移）、
   *    卡片入场、骨架屏 shimmer、spin 都会重播。
   *
   *  为什么不能简单写 `animation: none`：
   *    appIn / viewIn / setPaneIn 等都用 `both` 或 `forwards` 填充，直接去掉动画会
   *    让这些"终态填充值"一起消失（回到无动画的基础样式），反而更闪。
   *  正确做法：拿到每个 animation 对象，`finish()` 让它立刻落到终态，
   *    再 `pause()` 冻住（无限动画如 spin / shimmer 的 finish() 会抛错，忽略即可，
   *    它们暂停在当前位置不影响静态观感）。 */
  function freezeAnimations(): void {
    const anims = host.getAnimations({ subtree: true });
    for (const a of anims) {
      /* 无限动画（骨架 shimmer / spin / 脉冲）必须**继续跑**，而且要和真实界面同相位 ——
         否则克隆体里的骨架和底下的骨架错开，揭开的瞬间会看到"骨架跳了一下"。
         做法：把它的 currentTime 对齐到源元素的对应动画（源也在跑，用同一个时间基准）。
         这类动画不能 finish()（会抛 InvalidStateError）。 */
      const infinite =
        (a.effect as KeyframeEffect | null)?.getTiming?.().iterations === Infinity ||
        a.playState === "running" && isInfiniteName(a);
      if (infinite) {
        alignPhase(a);
        continue;
      }
      try {
        // 带 delay 且尚未开始（idle）的入场动画：先 play 再 finish，否则 finish 可能不生效
        if (a.playState === "idle") a.play();
        a.finish();
      } catch {
        /* 其它不能 finish 的情况：至少冻在当前帧 */
      }
      try {
        a.pause();
      } catch {
        /* noop */
      }
    }
    freezeSvgAnimations();
    /* 兜底：finish() 在部分动画上不会立刻把样式落到终态（需要一次样式重算）。
       这里主动读一个几何属性强制同步，避免"冻结了但第一帧还是半透明"。 */
    void host.offsetHeight;
  }

  /** 把揭示层的不透明底对齐到**新主题的真实实底**（[lc-2157]）。
   *
   *  为什么必须读 token 而不是用硬编码色：
   *    真实界面是 `.app-shell { background: rgb(28 28 30 / 85%) }` 叠在 DWM 亚克力/桌面上；
   *    揭示层底下是 `.lt-backing`。若 backing 用一个跟真实基色不同的值（例如 #060609 vs #1c1c1e），
   *    那么透过 85% 半透明的卡片/侧栏/文字底混出来的颜色就不相等 ——
   *    切换瞬间这个色差一次性暴露，看起来就是"闪一下"。
   *  做法：宿主已经带好新主题作用域，直接读它上面解析出的 `--panel` / `--bg-3`
   *    （AniBox 里两者都是该主题的**不透明**实底）。读不到就保留调用方给的值。
   *  注意：必须 `getComputedStyle(host)` —— 在 host 上取才带新主题的变量值。 */
  function syncBackingColor(): void {
    try {
      const cs = (doc.defaultView ?? window).getComputedStyle(host);
      const panel = cs.getPropertyValue("--panel").trim();
      const surface = cs.getPropertyValue("--bg-3").trim();
      // 优先 --panel（明确的不透明实底），其次 --bg-3，最后回退调用方传入值
      const picked = panel || surface;
      if (picked) {
        backing.style.background = picked;
        return;
      }
    } catch {
      /* noop */
    }
    backing.style.background = cfg.backingColor;
  }

  /** 已知的无限循环动画名（见各 styles/*.css 的 `infinite`） */
  const INFINITE_NAMES = new Set([
    "shimmer",
    "posterShimmer",
    "dskShimmer",
    "arcShimmer",
    "bgmcShimmer",
    "tb-spin",
    "diconSpin",
    "dweekDot",
    "dlibDotPulse",
    "webrsPulse",
  ]);

  function isInfiniteName(a: Animation): boolean {
    const n = (a as Animation & { animationName?: string }).animationName;
    return !!n && INFINITE_NAMES.has(n);
  }

  /** 把克隆体的无限动画相位对齐到源元素（避免揭开时骨架/转圈跳动） */
  function alignPhase(a: Animation): void {
    const n = (a as Animation & { animationName?: string }).animationName;
    if (!n) return;
    const src = sourceEl();
    let srcAnim: Animation | undefined;
    try {
      srcAnim = src.getAnimations({ subtree: true }).find((x) => {
        const xn = (x as Animation & { animationName?: string }).animationName;
        return xn === n && x.playState === "running";
      });
    } catch {
      /* noop */
    }
    try {
      if (srcAnim && typeof srcAnim.currentTime === "number") {
        a.currentTime = srcAnim.currentTime;
      }
    } catch {
      /* noop */
    }
  }

  /** SVG 内的 SMIL（<animate>/<animateTransform>）也要停：SMIL 不受 getAnimations 管。
   *  AniBox 目前没有 SMIL（已 grep），但模块要可复用，顺手处理掉。 */
  function freezeSvgAnimations(): void {
    try {
      const smils = host.querySelectorAll("animate, animateTransform, animateMotion, set");
      for (const s of Array.from(smils)) {
        const el = s as SVGAnimationElement & { endElement?: () => void };
        // 用 setTimeout 结束整条时间线：SMIL 没有 pause，但可以先 end 再冻结
        if (typeof el.endElement === "function") {
          // 跳到文档末尾时间点（让 fill 生效）后再冻结
          try {
            el.endElement();
          } catch {
            /* noop */
          }
        }
      }
    } catch {
      /* noop */
    }
  }

  /* ── 每帧：只写必要的几个属性（clip 的 d、湿边的 d / opacity）── */
  function applyFrame(progress: number): void {
    const p = clamp01(progress);
    const { w, h } = viewport();
    const stage = stageAt(p);
    const shape = buildLiquidShape({
      w,
      h,
      t: p,
      stage,
      ox: origin.x,
      oy: origin.y,
      or: origin.radius,
      seed: 20261113,
    });

    /* [lc-2155] 两个"90% 闪一下"的修复：
       ① 湿边跟随水位淡出（sheenAlpha）：7px 白色柔光扫过顶部会像闪一下白光。
       ② 完全盖满后把 clip-path 切成 none：水面已越过顶边、振幅已压平，
          clip 再算下去只是一条出屏的直线 —— 切成 none 既省掉每帧整屏栅格化，
          也彻底消除"顶部残留锯齿/旧主题竖条"的可能。
          注意必须**先**判断 sealed 才能跳过 d 写入，否则会先画一帧残形。 */
    if (shape.sealed) {
      if (!sealedApplied) {
        sealedApplied = true;
        reveal.style.clipPath = "none";
        (reveal.style as CSSStyleDeclaration & { webkitClipPath?: string }).webkitClipPath = "none";
        sheenSvg.style.opacity = "0";
      }
    } else {
      if (sealedApplied) {
        // 重复点击/中断后重新开一轮时恢复（正常情况下 sealed 只发生在最后一帧）
        sealedApplied = false;
        reveal.style.clipPath = `url(#${cfg.clipId})`;
        (reveal.style as CSSStyleDeclaration & { webkitClipPath?: string }).webkitClipPath = `url(#${cfg.clipId})`;
        sheenSvg.style.opacity = "1";
      }
      clipShape.setAttribute("d", shape.clip);
      sheenPath.setAttribute("d", shape.sheen);
      haloPath.setAttribute("d", shape.sheen);
      // 湿边不透明度：水面贴到顶边时收到 0（避免白光扫过顶部）
      haloPath.setAttribute("opacity", (0.55 * shape.sheenAlpha).toFixed(3));
      sheenPath.setAttribute("opacity", shape.sheenAlpha.toFixed(3));
    }

    frame = {
      progress: p,
      stage,
      direction,
      flow: shape.flow,
      surfaceY: shape.surfaceY,
      coverage: shape.coverage,
    };
    cfg.onFrame?.(frame);
    if (stage !== lastStage) {
      lastStage = stage;
      cfg.onStage?.(stage, frame);
    }
  }

  /* ── 清理 ── */
  function detach(): void {
    if (rafId !== null) {
      cancelRaf(rafId);
      rafId = null;
    }
    detachResize?.();
    detachResize = null;
    detachScroll?.();
    detachScroll = null;
  }

  function remove(): void {
    detach();
    if (root.parentNode) root.parentNode.removeChild(root);
    scrollSnaps = [];
    detachFade = null;
  }

  /** 收尾：**先**回调（调用方在此同步提交主题，此刻液体仍满屏盖着 → 零闪帧），
   *  再让揭示层淡出后拆除。
   *
   *  [lc-2157] 为什么不能直接 remove()：
   *    用户反馈「液体满了他要闪一下才能切换深浅」。实测平面采样差值为 0，
   *    说明"闪"不在平坦底色，而在**半透明面**：
   *      · 揭示层底下是 `.lt-backing`（我给的不透明实色）
   *      · 真实界面底下是 DWM 亚克力/桌面
   *    两者透过 85% 的 `.app-shell` / `--bg-*` / 卡片混出来的颜色并不完全相等。
   *    以前结束瞬间直接删层 = 这个色差一次性暴露 → 硬切（看起来"闪一下"）。
   *    现在：提交主题后让揭示层在 180ms 内淡出，色差被溶解掉；
   *    因为底层此时**已经是同一个新主题**，淡出过程不会看到任何旧内容。 */
  const FADE_OUT_MS = 180;

  function finish(result: "complete" | "interrupted"): void {
    if (done) return;
    done = true;
    detach(); // 先停 rAF / 监听，避免淡出期间还在算几何
    try {
      cfg.onEnd?.(result);
    } catch {
      /* 调用方异常不能阻止清理 */
    }

    const teardown = () => {
      clipShape.setAttribute("d", "");
      sheenPath.setAttribute("d", "");
      haloPath.setAttribute("d", "");
      remove();
    };

    if (result !== "complete" || FADE_OUT_MS <= 0) {
      teardown();
      return;
    }

    /* 正常完成才淡出：中断（切视图/卸载）要立刻消失，不能残留一层 */
    try {
      reveal.style.transition = `opacity ${FADE_OUT_MS}ms linear`;
      // 强制样式重算，保证 transition 从当前值开始（否则可能被合并成一帧）
      void reveal.offsetHeight;
      reveal.style.opacity = "0";
      sheenSvg.style.transition = `opacity ${FADE_OUT_MS}ms linear`;
      sheenSvg.style.opacity = "0";
    } catch {
      teardown();
      return;
    }
    // 不依赖 transitionend（后台标签页/失焦时可能不触发），用定时器兜底
    const timer = window.setTimeout(teardown, FADE_OUT_MS + 40);
    // 卸载时立刻清掉定时器并直接拆（destroy 会调 remove）
    detachFade = () => {
      window.clearTimeout(timer);
      teardown();
    };
  }

  function tick(ts: number): void {
    if (done) return;
    rafId = null;
    if (!t0) t0 = ts;
    const p = cfg.duration > 0 ? (ts - t0) / cfg.duration : 1;
    applyFrame(p);
    if (p >= 1) {
      finish("complete");
      return;
    }
    rafId = raf(guard);
  }

  /** 单帧几何异常不能把整页锁死 */
  function guard(ts: number): void {
    try {
      tick(ts);
    } catch {
      finish("interrupted");
    }
  }

  const handle: LiquidRevealHandle = {
    get frame() {
      return frame;
    },
    get active() {
      return started && !done;
    },
    start(): void {
      if (started || done) return;
      started = true;
      frame = emptyFrame(direction);
      (doc.body ?? doc.documentElement).appendChild(root);

      // 后台标签页 rAF 会被暂停 → 直接不播（调用方按"立即切换"收尾）
      if (doc.visibilityState === "hidden") {
        finish("interrupted");
        return;
      }

      buildSnapshot();
      // 视口变化：不重建 DOM（CSS 会随宿主宽度重新排版），只重新对齐尺寸与滚动
      const onResize = () => {
        if (done) return;
        const src = sourceEl();
        host.style.width = `${src.clientWidth}px`;
        host.style.height = `${src.clientHeight}px`;
        scrollSnaps = captureScroll(src);
        syncScroll();
        syncViewport();
        freezeAnimations();
        const p = t0 ? (performance.now() - t0) / (cfg.duration || 1) : 0;
        applyFrame(p);
      };
      window.addEventListener("resize", onResize);
      detachResize = () => window.removeEventListener("resize", onResize);
      // 整页滚动的宿主：动画期间用户仍可能滚动 → 持续对齐（passive，不阻塞滚动）
      window.addEventListener("scroll", syncViewport, { passive: true });
      detachScroll = () => window.removeEventListener("scroll", syncViewport);

      applyFrame(0);
      rafId = raf(guard);
    },
    interrupt(): void {
      if (!started) {
        remove();
        return;
      }
      finish("interrupted");
    },
    finishNow(): void {
      if (done) return;
      if (!started) {
        remove();
        return;
      }
      applyFrame(1);
      finish("complete");
    },
    destroy(): void {
      done = true;
      // 正在淡出的层要立刻拆（卸载场景不能等 180ms）
      if (detachFade) {
        detachFade();
        return;
      }
      remove();
    },
  };

  return { handle, root };
}

/** 动态 SVG clipPath 支持探测：不支持就直接切主题（需求要求） */
export function supportsDynamicClipPath(doc: Document = document): boolean {
  if (typeof CSS === "undefined" || typeof CSS.supports !== "function") return false;
  try {
    // 用真实值探测：空串在部分老 Chromium 上恒为 false
    if (!CSS.supports("clip-path", "url(#lt-probe)")) return false;
    const probe = doc.createElementNS(SVG_NS, "clipPath");
    return !!probe && typeof (probe as Element).setAttribute === "function";
  } catch {
    return false;
  }
}
