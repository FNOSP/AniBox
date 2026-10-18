/* 手柄焦点导航（白框代替鼠标）。
 *
 * 移植自旧项目 `Fntv-Plus/src/preload/plugins/gamepadFocus.ts` 的核心逻辑，并按
 * AniBox 的 DOM 结构（.card / .side-item / .nav-cap-l / .ddet-mask / .set-*）收敛。
 *
 * 设计要点：
 *  - 候选：a[href] / button / [role=button] / .card / .poster-card / .side-item /
 *    .nav-cap-l / 设置面板控件（.set-btn / .set-sw / .set-input / input / select / label）。
 *  - 可见性：rect ≥ 最小尺寸、非 display:none/visibility:hidden/opacity:0、非 disabled。
 *  - 作用域：存在可见弹层（.ddet-mask / .zoom-mask / [role=dialog]）时**只**在该弹层内取候选。
 *  - 去重：内层候选被外层候选完全覆盖时保留外层（.card 内的按钮不再单独占一个焦点）。
 *  - 导航：同排/同列优先的最近候选；目标滚动进最近的可滚动容器。
 *  - 白框：fixed 白边 + 呼吸光晕，pointer-events:none，最高层级，平滑过渡。
 *  - 鼠标活动（mousemove/mousedown）自动隐藏，交还鼠标。
 */

export type FocusDir = "up" | "down" | "left" | "right";

const CANDIDATE_SELECTORS = [
  "a[href]",
  "button",
  "[role=button]",
  ".card",
  ".poster-card",
  ".dlib-card",
  ".dlib-gcard",
  ".lib-strip-card",
  ".hist-card",
  ".drel-card",
  ".side-item",
  ".nav-cap-l", // 首页/发现胶囊：显式列出，避免只靠 `button` 命中时被几何排序挤掉
  ".set-row button",
  ".set-sw",
  ".set-btn",
  ".set-input",
  ".set-select",
  // 语义角色：菜单项/标签页/开关/勾选等（很多是 div/span + onClick，不是原生 button）。
  "[role=option]",
  "[role=menuitem]",
  "[role=menuitemcheckbox]",
  "[role=menuitemradio]",
  "[role=tab]",
  "[role=switch]",
  "[role=checkbox]",
  "[role=radio]",
  "[role=link]",
  // 原生 onclick 属性（非 React 路径）也算可点击。
  "[onclick]",
  // 「次要/灰色样式但实际可点击」的 div/span 容器：它们没有 button/a 语义，
  // 只靠 onClick 响应，必须显式列出，否则手柄方向键永远滑不过去。
  ".sa-head", // 侧栏「收起快捷操作」
  ".bgmac-bar", // BGM 账号折叠横栏（点击展开）
  ".row-more", // 首页横排「全部 ›」
  ".dvrev-more", // 影评「更多 ›」
  ".lsc-manage", // 首页媒体库「管理」
  ".hist-lrow", // 观影记录紧凑列表 / 时间线行
  ".dlib-lrow", // 发现页紧凑列表行
  "input",
  "select",
  "label",
];

const OVERLAY_SELECTORS = [
  ".ddet-mask",
  ".zoom-mask",
  ".semi-modal",
  ".semi-modal-content",
  "[role=dialog]",
  // 设置面板下拉（portal 到 body，z-index 120）：打开时把候选限定在选项里。
  ".set-dd-pop",
];

const FRAME_ID = "fntv-focus-frame";
const STYLE_ID = "fntv-focus-style";

/** 固定在视口顶部、不随内容滚动的「壳层」：顶栏、窗口按钮层、首页/发现胶囊浮层。
 *  它们永远停在同一个 y 上；若和滚动内容一起参与几何距离比较，内容往下滚之后
 *  按方向键会总是先命中它们（表现为“跳回顶部”）。 */
const CHROME_SELECTOR = ".titlebar, .tb-close-layer, .tb-brand-layer, .navbar";

let frameEl: HTMLDivElement | null = null;
let focusedEl: HTMLElement | null = null;
let active = false;

function ensureStyle(): void {
  if (typeof document === "undefined") return;
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement("style");
  style.id = STYLE_ID;
  style.textContent =
    "@keyframes fntv-focus-pulse{" +
    "0%,100%{box-shadow:0 0 0 1.5px rgba(0,0,0,.55),0 0 12px rgba(255,255,255,.55),inset 0 0 8px rgba(255,255,255,.16)}" +
    "50%{box-shadow:0 0 0 1.5px rgba(0,0,0,.55),0 0 26px rgba(255,255,255,.95),inset 0 0 14px rgba(255,255,255,.32)}}";
  (document.head || document.documentElement).appendChild(style);
}

function ensureFrame(): HTMLDivElement {
  if (frameEl && frameEl.isConnected) return frameEl;
  ensureStyle();
  frameEl = document.createElement("div");
  frameEl.id = FRAME_ID;
  frameEl.style.cssText = [
    "position:fixed",
    "left:0",
    "top:0",
    "pointer-events:none",
    "z-index:2147483647",
    "border:3px solid #fff",
    "border-radius:12px",
    "transition:left .22s cubic-bezier(.22,.61,.36,1),top .22s cubic-bezier(.22,.61,.36,1),width .22s cubic-bezier(.22,.61,.36,1),height .22s cubic-bezier(.22,.61,.36,1)",
    "animation:fntv-focus-pulse 1.8s ease-in-out infinite",
    "will-change:left,top,width,height",
    "display:none",
  ].join(";");
  document.body.appendChild(frameEl);
  return frameEl;
}

function isVisible(el: HTMLElement): boolean {
  if (!el.isConnected) return false;
  const r = el.getBoundingClientRect();
  if (r.width < 12 || r.height < 12) return false;
  const st = getComputedStyle(el);
  if (st.display === "none" || st.visibility === "hidden") return false;
  // 只排除**完全透明**（opacity:0）。0.3~0.9 的“灰色/次要/禁用感”样式仍算可交互 ——
  // 用户明确这些按钮能点，不能因为颜色/次要样式把它们踢出候选。
  if (parseFloat(st.opacity) === 0) return false;
  // 真正点不到的元素：自身 pointer-events:none（父级 none、子级 auto 的常见写法不算）。
  if (st.pointerEvents === "none") return false;
  if (el.getAttribute("aria-hidden") === "true") return false;
  // 逐级检查祖先：被隐藏 / 完全透明的容器里的元素（例如 hover 才显示的操作层）不参与。
  // 注意：**不检查祖先 pointer-events**，因为 `.navbar{pointer-events:none}` 里
  // `.nav-cap{pointer-events:auto}` 这类写法下子元素仍可点。
  let p = el.parentElement;
  let depth = 0;
  while (p && p !== document.body && depth++ < 40) {
    if (p.getAttribute("aria-hidden") === "true") return false;
    const ps = getComputedStyle(p);
    if (ps.display === "none" || ps.visibility === "hidden") return false;
    if (parseFloat(ps.opacity) === 0) return false;
    p = p.parentElement;
  }
  return true;
}

/** 真正的交互控件（区别于仅作容器的卡片/行）：按钮、链接、表单控件、
 *  交互 role，以及带原生 onclick 的容器。 */
function isInteractiveControl(el: HTMLElement): boolean {
  const tag = el.tagName;
  if (tag === "BUTTON" || tag === "A" || tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA") {
    return true;
  }
  const role = el.getAttribute("role");
  if (
    role &&
    /^(button|menuitem|menuitemcheckbox|menuitemradio|tab|option|switch|checkbox|radio|link)$/.test(
      role,
    )
  ) {
    return true;
  }
  return el.hasAttribute("onclick");
}

function isDisabled(el: HTMLElement): boolean {
  const anyEl = el as HTMLElement & { disabled?: boolean };
  if (anyEl.disabled) return true;
  if (el.getAttribute("aria-disabled") === "true") return true;
  // 设置面板里禁用态的控件透明度通常为 .5，仍可见；这里显式排除。
  return false;
}

/** 是否属于固定在顶部的壳层（顶栏/窗口按钮/胶囊浮层）。 */
function isChromeCandidate(el: HTMLElement): boolean {
  try {
    if (el.closest(CHROME_SELECTOR)) return true;
    // 其它 position:fixed 的悬浮控件同样不随内容滚动。
    return getComputedStyle(el).position === "fixed";
  } catch {
    return false;
  }
}

/** 顶栏/窗口按钮层（比胶囊更“高”的一层）：从内容向上移动时让胶囊优先，
 *  这样「首页 / 发现」两个按钮不会被顶栏两侧的图标按钮抢走。 */
function isTopChrome(el: HTMLElement): boolean {
  return !!el.closest(".titlebar, .tb-close-layer, .tb-brand-layer");
}

/** 当前最高可见弹层（作为焦点作用域）；没有则 null = 整页。 */
function findActiveOverlay(): HTMLElement | null {
  if (typeof document === "undefined") return null;
  const nodes = document.querySelectorAll(OVERLAY_SELECTORS.join(","));
  let best: HTMLElement | null = null;
  let bestZ = -Infinity;
  nodes.forEach((n) => {
    const el = n as HTMLElement;
    const st = getComputedStyle(el);
    if (st.display === "none" || st.visibility === "hidden" || parseFloat(st.opacity) === 0) return;
    const pos = st.position;
    if (pos !== "fixed" && pos !== "absolute") return;
    const r = el.getBoundingClientRect();
    // 下拉选项层可能只有一两项高，不套用弹层的 80x60 下限。
    const minW = el.classList.contains("set-dd-pop") ? 40 : 80;
    const minH = el.classList.contains("set-dd-pop") ? 20 : 60;
    if (r.width < minW || r.height < minH) return;
    const z = parseInt(st.zIndex, 10);
    if (Number.isFinite(z) && z > bestZ) {
      bestZ = z;
      best = el;
    }
  });
  return best;
}

function collectCandidates(scope?: HTMLElement | null): HTMLElement[] {
  const root: Document | HTMLElement = scope ?? findActiveOverlay() ?? document;
  const map = new Set<HTMLElement>();
  for (const sel of CANDIDATE_SELECTORS) {
    let nodes: NodeListOf<Element> | null = null;
    try {
      nodes = root.querySelectorAll(sel);
    } catch {
      continue;
    }
    nodes.forEach((n) => {
      const el = n as HTMLElement;
      if (!el || !el.isConnected) return;
      if (!isVisible(el) || isDisabled(el)) return;
      // label 只有包裹了可交互控件时才作为整行开关；纯文字 label 跳过。
      if (el.tagName === "LABEL" && !el.querySelector("input,select,textarea")) return;
      map.add(el);
    });
  }
  // 去重：内层被外层完全覆盖 → 保留外层（.card / .side-item / label 整行）。
  const list = Array.from(map);
  const dropped = new Set<HTMLElement>();
  for (const c of list) {
    if (dropped.has(c)) continue;
    for (const a of list) {
      if (a === c) continue;
      if (!a.contains(c) || c.contains(a)) continue;
      const ra = a.getBoundingClientRect();
      const rc = c.getBoundingClientRect();
      const covers =
        ra.left - 2 <= rc.left &&
        ra.top - 2 <= rc.top &&
        ra.right + 2 >= rc.right &&
        ra.bottom + 2 >= rc.bottom;
      if (covers) {
        // 关键：内层是真正的交互控件（按钮 / 链接 / role 按钮 / onclick 容器），
        // 而外层只是卡片/行容器时，**保留内层**。否则卡片里的次级（灰色）操作按钮
        // 会被外层整卡覆盖而永远无法被手柄选中。
        // label 视为「交互包装器」，其内部 input 仍交给 label 统一处理。
        const outerInteractive = isInteractiveControl(a) || a.tagName === "LABEL";
        if (isInteractiveControl(c) && !outerInteractive) continue;
        dropped.add(c);
        break;
      }
    }
  }
  return list.filter((el) => !dropped.has(el));
}

/** 只按当前 rect 重画白框（不触发滚动，避免打断进行中的平滑滚动）。 */
function placeFrame(el: HTMLElement): void {
  const r = el.getBoundingClientRect();
  const f = ensureFrame();
  // 弹层遮罩可能与白框同为最高层级，绘制顺序决定谁在上面：始终把白框放到 body 末尾。
  if (f.parentNode === document.body) document.body.appendChild(f);
  f.style.left = `${r.left}px`;
  f.style.top = `${r.top}px`;
  f.style.width = `${r.width}px`;
  f.style.height = `${r.height}px`;
  f.style.display = "block";
}

let settleTimer = 0;

/** 把目标平滑滚入最近的可滚动容器（首页 .content / 发现页 .dscroll 等），
 *  滚动结束后再校准一次白框位置。 */
function scrollToEl(el: HTMLElement): void {
  try {
    el.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  } catch {
    try {
      el.scrollIntoView({ block: "nearest", inline: "nearest" });
    } catch {
      /* ignore */
    }
  }
  window.clearTimeout(settleTimer);
  settleTimer = window.setTimeout(() => {
    if (active && focusedEl === el && el.isConnected) placeFrame(el);
  }, 380);
}

function focusEl(el: HTMLElement): void {
  focusedEl = el;
  active = true;
  scrollToEl(el);
  placeFrame(el);
}

function hideFrame(): void {
  active = false;
  focusedEl = null;
  if (frameEl) frameEl.style.display = "none";
}

function centerOf(el: HTMLElement): { cx: number; cy: number } {
  const r = el.getBoundingClientRect();
  return { cx: r.left + r.width / 2, cy: r.top + r.height / 2 };
}

/** 「返回」文案：中文“返回”，或独立的英文 back（避免误中 feedback 之类）。 */
const BACK_LABEL_RE = /返回|\bback\b/i;

/** 是否是“返回”控件：aria-label / title / 文本，或只有 ‹ 图标的 .back / .mv-back / .webrs-back。 */
function isBackControl(el: HTMLElement): boolean {
  const label = `${el.getAttribute("aria-label") ?? ""} ${el.getAttribute("title") ?? ""} ${el.textContent ?? ""}`;
  if (BACK_LABEL_RE.test(label)) return true;
  for (const cls of Array.from(el.classList)) {
    if (cls === "back" || cls.startsWith("back-") || cls.endsWith("-back")) return true;
  }
  return false;
}

/** 找当前可见的「返回」控件；有弹层时只在弹层内找，且永远排除顶栏/胶囊壳层
 *  （避免 B 键误点顶栏的「返回首页」或窗口按钮）。 */
function findBackControl(scope: HTMLElement | null): HTMLElement | null {
  if (typeof document === "undefined") return null;
  const root: ParentNode = scope ?? document;
  const nodes = root.querySelectorAll<HTMLElement>('button, [role="button"], a[href]');
  for (const el of Array.from(nodes)) {
    if (!scope && el.closest(CHROME_SELECTOR)) continue;
    if (!isVisible(el) || isDisabled(el)) continue;
    if (isBackControl(el)) return el;
  }
  return null;
}

export const focusNav = {
  isActive(): boolean {
    return active;
  },

  getFocused(): HTMLElement | null {
    return focusedEl;
  },

  hasOpenOverlay(): boolean {
    return !!findActiveOverlay();
  },

  /** 激活白框。已有焦点时只刷新位置；没有候选则保持不激活。 */
  activate(): boolean {
    const overlay = findActiveOverlay();
    if (active && focusedEl && focusedEl.isConnected && (!overlay || overlay.contains(focusedEl))) {
      scrollToEl(focusedEl);
      placeFrame(focusedEl);
      return true;
    }
    // 作用域变了（弹层打开/关闭）或旧焦点已失效：清掉旧焦点，重新在正确作用域里选。
    if (active) hideFrame();
    const cands = collectCandidates(overlay);
    if (!cands.length) return false;
    // 初始落点避开顶栏/胶囊等固定壳层：内容往下滚后重新激活时不该先停在顶部。
    // 若整页只有壳层可聚焦（极少见），再退回全部候选。
    const pool = overlay ? cands : cands.filter((el) => !isChromeCandidate(el));
    const usable = pool.length ? pool : cands;
    // 初始焦点：视口中心最近者（弹层内同理，取弹层中心更自然）。
    const cx = window.innerWidth / 2;
    const cy = window.innerHeight / 2;
    let best: HTMLElement | null = null;
    let bestD = Infinity;
    for (const el of usable) {
      const c = centerOf(el);
      const d = Math.abs(c.cx - cx) + Math.abs(c.cy - cy);
      if (d < bestD) {
        bestD = d;
        best = el;
      }
    }
    if (!best) return false;
    focusEl(best);
    return true;
  },

  move(dir: FocusDir): boolean {
    if (!active) {
      if (!this.activate()) return false;
      return true;
    }
    // 弹层作用域变化（打开/关闭）或旧焦点失效：先在新作用域里重新落点。
    const scope = findActiveOverlay();
    if (focusedEl && (!focusedEl.isConnected || (scope && !scope.contains(focusedEl)))) {
      hideFrame();
      if (!this.activate()) return false;
    }
    // 滑杆（设置面板的死区/阈值等）：左右直接调值，而不是把焦点移走。
    if (
      focusedEl instanceof HTMLInputElement &&
      focusedEl.type === "range" &&
      (dir === "left" || dir === "right")
    ) {
      const step = Number(focusedEl.step) || 1;
      const min = Number(focusedEl.min);
      const max = Number(focusedEl.max);
      const curVal = Number(focusedEl.value);
      const next = Math.min(max, Math.max(min, curVal + (dir === "right" ? step : -step)));
      if (next !== curVal) {
        // React 受控 input：必须走原生 setter，再派发 input 事件，onChange 才会生效。
        const setter = Object.getOwnPropertyDescriptor(
          HTMLInputElement.prototype,
          "value",
        )?.set;
        setter?.call(focusedEl, String(next));
        focusedEl.dispatchEvent(new Event("input", { bubbles: true }));
        focusedEl.dispatchEvent(new Event("change", { bubbles: true }));
      }
      return true;
    }
    const cands = collectCandidates();
    if (!cands.length) return false;
    const cur = focusedEl && focusedEl.isConnected ? focusedEl : null;
    const cr = cur
      ? cur.getBoundingClientRect()
      : {
          left: window.innerWidth / 2 - 5,
          top: window.innerHeight / 2 - 5,
          right: window.innerWidth / 2 + 5,
          bottom: window.innerHeight / 2 + 5,
          width: 10,
          height: 10,
        };
    const curTop = cr.top;
    const curBottom = cr.bottom;
    const curLeft = cr.left;
    const curRight = cr.right;
    const curCx = cr.left + cr.width / 2;
    const curCy = cr.top + cr.height / 2;

    // 固定壳层（顶栏 / 胶囊）只在“没有弹层”时做特殊排序：
    //  - 从内容里向下/水平移动：直接排除（否则滚动后会被吸回顶部）；
    //  - 从内容里向上移动：降权为兜底（先把内容往上滚完，再落到胶囊/顶栏）；
    //  - 焦点本来就在壳层里：水平移动只在壳层内走（首页 ↔ 发现）。
    const chromeEnabled = !scope;
    const curIsChrome = chromeEnabled && !!cur && isChromeCandidate(cur);

    let best: HTMLElement | null = null;
    let bestScore = Infinity;

    for (const el of cands) {
      if (cur && el === cur) continue;
      const chrome = chromeEnabled && isChromeCandidate(el);
      if (chromeEnabled) {
        if (!curIsChrome) {
          // 从内容里向下/水平移动：固定壳层不参与（否则滚动后会被吸回顶部）。
          if (dir === "down" || dir === "left" || dir === "right") continue;
        } else if (dir === "left" || dir === "right") {
          // 焦点在壳层内：水平移动只在同一层里走（首页 ↔ 发现 / 顶栏图标之间）。
          if (!chrome || isTopChrome(el) !== isTopChrome(cur as HTMLElement)) continue;
        }
      }
      const r = el.getBoundingClientRect();
      if (dir === "left" || dir === "right") {
        const inDir = dir === "right" ? r.left > curRight + 2 : r.right < curLeft - 2;
        if (!inDir) continue;
        const sameRow = r.top < curBottom && curTop < r.bottom;
        const gap = dir === "right" ? r.left - curRight : curLeft - r.right;
        const dy = Math.abs((r.top + r.bottom) / 2 - curCy);
        let score = sameRow ? gap : dy * 20 + gap;
        if (chromeEnabled && chrome && !curIsChrome) score += isTopChrome(el) ? 2e6 : 1e6;
        if (score < bestScore) {
          bestScore = score;
          best = el;
        }
      } else {
        const inDir = dir === "down" ? r.top > curBottom + 2 : r.bottom < curTop - 2;
        if (!inDir) continue;
        const sameCol = r.left < curRight && curLeft < r.right;
        const gap = dir === "down" ? r.top - curBottom : curTop - r.bottom;
        const dx = Math.abs((r.left + r.right) / 2 - curCx);
        let score = sameCol ? gap : dx * 20 + gap;
        // 向上：先走内容（含屏幕外的候选，靠平滑滚动带入视口）；
        // 内容走完后才落到胶囊（1e6），最后才是顶栏图标/窗口按钮（2e6）。
        if (chromeEnabled && chrome && !curIsChrome) score += isTopChrome(el) ? 2e6 : 1e6;
        if (score < bestScore) {
          bestScore = score;
          best = el;
        }
      }
    }
    if (!best) return false;
    focusEl(best);
    return true;
  },

  /** A 键确认：模拟鼠标点击当前焦点；卡片内含链接时点链接。 */
  select(): boolean {
    if (!active) return false;
    const overlay = findActiveOverlay();
    if (!focusedEl || !focusedEl.isConnected || (overlay && !overlay.contains(focusedEl))) {
      if (!this.activate()) return false;
    }
    const el = focusedEl;
    if (!el) return false;
    // 自身就是交互控件（button / a / role=button / onclick 容器）→ 直接点自己，
    // React 的 onClick 由事件冒泡到根节点触发，适用于原生按钮，也适用于 div/span 按钮。
    // 否则（卡片/行容器）优先点内部链接，保持原有「卡片内含链接时点链接」的行为。
    const target: HTMLElement = isInteractiveControl(el)
      ? el
      : ((el.querySelector("a[href]") as HTMLElement | null) ?? el);
    try {
      target.dispatchEvent(
        new MouseEvent("click", {
          bubbles: true,
          cancelable: true,
          view: window,
          button: 0,
        }),
      );
      return true;
    } catch {
      return false;
    }
  },

  /** B 键优先动作：若当前页面有可见的「返回」控件就点它，返回是否点到了。 */
  clickBack(): boolean {
    const overlay = findActiveOverlay();
    const btn = findBackControl(overlay);
    if (!btn) return false;
    try {
      btn.dispatchEvent(
        new MouseEvent("click", { bubbles: true, cancelable: true, view: window }),
      );
      return true;
    } catch {
      return false;
    }
  },

  /** B 键兜底：隐藏白框（页面关闭由调用方派发 Escape 完成）。 */
  back(): boolean {
    if (!active) return false;
    hideFrame();
    return true;
  },

  dismiss(): void {
    if (active) hideFrame();
  },

  /** 重贴白框（滚动/尺寸变化时用）。 */
  refresh(): void {
    if (active && focusedEl && focusedEl.isConnected) placeFrame(focusedEl);
  },
};

/** 焦点在输入控件上时，方向键应留给输入本身（不做焦点导航）。 */
export function isEditableFocused(): boolean {
  const el = document.activeElement as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || el.isContentEditable;
}

/** 鼠标活动交还鼠标；滚动/尺寸变化时重贴白框。模块导入时安装一次。 */
let installed = false;
export function installFocusNav(): void {
  if (installed || typeof window === "undefined") return;
  installed = true;
  window.addEventListener("mousemove", () => focusNav.dismiss(), { passive: true });
  window.addEventListener("mousedown", () => focusNav.dismiss(), { passive: true });
  let timer = 0;
  window.addEventListener(
    "scroll",
    () => {
      if (!active || !focusedEl) return;
      window.clearTimeout(timer);
      timer = window.setTimeout(() => focusNav.refresh(), 80);
    },
    { passive: true, capture: true },
  );
  window.addEventListener("resize", () => focusNav.refresh());
}

installFocusNav();
