/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Copy as IcoCopy, Menu as IcoMenuL, Minus as IcoMinus, Square as IcoSquare, X as IcoXL } from "lucide-react";
import { ChromeTone } from "../../types";

/* ────────────────────────── 顶栏二值色调采样 ──────────────────────────
 * [lc-1703] 用户否掉了上一版 `mix-blend-mode: difference`：反相结果是彩色/半透明/灰，
 * 要求图标/文字**要么实心纯白、要么实心纯黑**（完全不透明），按背景亮度自动二选一。
 *
 * 策略（优先级从高到低）：
 *   1) 在顶栏 40px 高度的一条横带上取若干采样点，用 `elementsFromPoint` 穿透顶栏浮层，
 *      找到**实际**压在下面的背景元素：
 *        · <img> / background-image:url(...) → 用 canvas 画它"顶栏那一条"再算平均亮度；
 *        · 纯色 background-color → 直接按颜色算亮度（alpha<1 时与主题底色合成）。
 *   2) 图片跨域污染 canvas / 加载失败 / 根本没找到背景 → 回退到主题：
 *      浅色主题 → 纯黑图标；深色主题 → 纯白图标。
 *
 * 亮度阈值 150（0–255）：高 → data-tone="light"（背景亮，图标纯黑）；
 * 低 → data-tone="dark"（背景暗，图标纯白）。
 * 采样成本：1s 轮询，但只有"背景签名"（图 URL / 颜色 / 尺寸 / 主题）变化时才真正读像素；
 * resize 200ms 防抖、主题 class 变化、页面切换都会强制重采。
 * ───────────────────────────────────────────────────────────────────── */

type TbTone = "light" | "dark";

const TONE_LUMA_THRESHOLD = 150;
/** 顶栏高度（与 CSS .titlebar height 保持一致） */
const SAMPLE_BAND = 40;
/** 顶栏自身的浮层：采样时跳过，穿透到真实背景 */
const OVERLAY_SELECTOR = ".titlebar, .tb-brand-layer, .tb-close-layer";

type CssColor = { r: number; g: number; b: number; a: number };
type BackdropCand = { el: Element; kind: "image" | "color" };

function parseCssColor(input: string): CssColor | null {
  const m = input.match(
    /rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:[,\s/]+([\d.]+))?\s*\)/i,
  );
  if (!m) return null;
  return {
    r: Number(m[1]),
    g: Number(m[2]),
    b: Number(m[3]),
    a: m[4] === undefined ? 1 : Number(m[4]),
  };
}

function relLuma(r: number, g: number, b: number): number {
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function isDarkTheme(): boolean {
  return document.documentElement.classList.contains("dark");
}

/** 主题兜底：浅色主题背景亮 → 纯黑图标；深色主题背景暗 → 纯白图标 */
function themeTone(): TbTone {
  return isDarkTheme() ? "dark" : "light";
}

/** 主题底色（用于把半透明背景色合成出不透明结果） */
function themeBase(): [number, number, number] {
  return isDarkTheme() ? [13, 15, 22] : [246, 248, 252];
}

function extractCssUrl(bgImage: string): string | null {
  const m = bgImage.match(/url\((['"]?)(.*?)\1\)/i);
  return m ? m[2] : null;
}

function classifyBackdrop(el: Element): "image" | "color" | null {
  if (el instanceof HTMLImageElement) {
    return (el.currentSrc || el.src) && el.naturalWidth > 0 ? "image" : null;
  }
  const cs = getComputedStyle(el);
  const bi = cs.backgroundImage;
  if (bi && bi !== "none" && bi.includes("url(")) return "image";
  const c = parseCssColor(cs.backgroundColor);
  if (c && c.a > 0.05) return "color";
  return null;
}

/** 沿顶栏横带取采样点，穿透浮层拿到真实背景元素（每个点取最上层可采样者） */
function pickBackdrops(): BackdropCand[] {
  const w = window.innerWidth;
  const h = window.innerHeight;
  if (w <= 0 || h <= 0) return [];
  const y = Math.min(SAMPLE_BAND / 2, h - 1);
  const xs = [0.05, 0.2, 0.35, 0.5, 0.65, 0.8, 0.95].map((r) =>
    Math.min(w - 1, Math.round(w * r)),
  );
  const seen = new Set<Element>();
  const out: BackdropCand[] = [];
  for (const x of xs) {
    const stack = document.elementsFromPoint(x, y);
    for (const el of stack) {
      if (el.closest(OVERLAY_SELECTOR)) continue;
      const kind = classifyBackdrop(el);
      if (!kind) continue;
      if (!seen.has(el)) {
        seen.add(el);
        out.push({ el, kind });
      }
      break;
    }
  }
  return out;
}

/** 背景签名：用于避免每秒重复读像素 */
function backdropSignature(): string {
  const parts = pickBackdrops().map(({ el, kind }) => {
    const r = el.getBoundingClientRect();
    const box = `${kind}:${Math.round(r.left)},${Math.round(r.top)},${Math.round(r.width)},${Math.round(r.height)}`;
    if (el instanceof HTMLImageElement) return `${box}:${el.currentSrc || el.src}`;
    const cs = getComputedStyle(el);
    return `${box}:${cs.backgroundImage}:${cs.backgroundColor}`;
  });
  return `${parts.join(";")}|${window.innerWidth}x${window.innerHeight}|${isDarkTheme() ? "d" : "l"}`;
}

/** 纯色背景亮度：alpha < 1 时与主题底色合成 */
function colorLuma(el: Element): number | null {
  const c = parseCssColor(getComputedStyle(el).backgroundColor);
  if (!c) return null;
  const [br, bg, bb] = themeBase();
  const r = c.r * c.a + br * (1 - c.a);
  const g = c.g * c.a + bg * (1 - c.a);
  const b = c.b * c.a + bb * (1 - c.a);
  return relLuma(r, g, b);
}

const imageCache = new Map<string, Promise<HTMLImageElement | null>>();

function loadImage(src: string): Promise<HTMLImageElement | null> {
  const hit = imageCache.get(src);
  if (hit) return hit;
  const p = new Promise<HTMLImageElement | null>((resolve) => {
    const img = new Image();
    let settled = false;
    let timer = 0;
    const done = (v: HTMLImageElement | null) => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timer);
      resolve(v);
    };
    img.crossOrigin = "anonymous"; // 不带会被画布污染 → getImageData 抛 SecurityError
    img.decoding = "async";
    img.onload = () => done(img);
    img.onerror = () => {
      imageCache.delete(src); // 失败不永久缓存，后续轮询可重试
      done(null);
    };
    // 兜底：网络挂起时别让 busy 永远卡住
    timer = window.setTimeout(() => {
      imageCache.delete(src);
      done(null);
    }, 4000);
    img.src = src;
  });
  imageCache.set(src, p);
  return p;
}

/**
 * 取"元素在顶栏那一条"的平均亮度。
 * <img> 走 object-fit 的 cover/contain/fill 映射；CSS background-image 用 background-size 近似。
 * 跨域污染 / 图片加载失败 → null（调用方走主题兜底）。
 */
async function imageLuma(el: Element): Promise<number | null> {
  const rect = el.getBoundingClientRect();
  if (rect.width < 1 || rect.height < 1) return null;

  let src: string | null = null;
  let natW = 0;
  let natH = 0;
  let fit: "cover" | "contain" | "fill" = "fill";

  if (el instanceof HTMLImageElement) {
    src = el.currentSrc || el.src;
    natW = el.naturalWidth;
    natH = el.naturalHeight;
    const of = getComputedStyle(el).objectFit;
    fit = of === "cover" || of === "contain" ? of : "fill";
  } else {
    const cs = getComputedStyle(el);
    src = extractCssUrl(cs.backgroundImage);
    const bs = cs.backgroundSize;
    fit = bs === "cover" ? "cover" : bs === "contain" ? "contain" : "fill";
  }
  if (!src) return null;

  const img = await loadImage(src);
  if (!img) return null;
  if (!natW || !natH) {
    natW = img.naturalWidth;
    natH = img.naturalHeight;
  }
  if (!natW || !natH) return null;

  // 顶栏横带 [0, SAMPLE_BAND] 与该元素可见矩形的交集（元素局部坐标）
  const localTop = Math.max(0, -rect.top);
  const localBottom = Math.min(SAMPLE_BAND, rect.bottom) - rect.top;
  if (localBottom <= localTop) return null;

  let sx = 0;
  let sy = 0;
  let sw = natW;
  let sh = natH;
  if (fit === "cover" || fit === "contain") {
    const scale =
      fit === "cover"
        ? Math.max(rect.width / natW, rect.height / natH)
        : Math.min(rect.width / natW, rect.height / natH);
    const dispW = natW * scale;
    const dispH = natH * scale;
    const offX = (dispW - rect.width) / 2; // cover 时为裁掉的量；contain 时可能为负（留白）
    const offY = (dispH - rect.height) / 2;
    sx = Math.max(0, offX) / scale;
    sy = Math.max(0, offY) / scale + localTop / scale;
    sw = Math.min(natW - sx, rect.width / scale);
    sh = Math.min(natH - sy, (localBottom - localTop) / scale);
  } else {
    sy = (localTop / rect.height) * natH;
    sh = ((localBottom - localTop) / rect.height) * natH;
  }
  if (sw <= 0 || sh <= 0) return null;

  const dw = 64;
  const dh = Math.max(1, Math.min(16, Math.round(dw * (sh / sw))));
  const cv = document.createElement("canvas");
  cv.width = dw;
  cv.height = dh;
  const ctx = cv.getContext("2d", { willReadFrequently: true });
  if (!ctx) return null;
  try {
    ctx.drawImage(img, sx, sy, sw, sh, 0, 0, dw, dh);
    const { data } = ctx.getImageData(0, 0, dw, dh);
    let sum = 0;
    let n = 0;
    for (let i = 0; i < data.length; i += 4) {
      const a = data[i + 3] / 255;
      if (a === 0) continue;
      sum += relLuma(data[i], data[i + 1], data[i + 2]) * a;
      n += a;
    }
    return n > 0 ? sum / n : null;
  } catch {
    return null; // 跨域污染（SecurityError）→ 兜底主题
  }
}

async function sampleTone(): Promise<TbTone> {
  const cands = pickBackdrops();
  const lumas: number[] = [];
  for (const c of cands) {
    const l = c.kind === "image" ? await imageLuma(c.el) : colorLuma(c.el);
    if (l != null) lumas.push(l);
  }
  if (!lumas.length) return themeTone();
  const avg = lumas.reduce((a, b) => a + b, 0) / lumas.length;
  return avg > TONE_LUMA_THRESHOLD ? "light" : "dark";
}

/** 顶栏色调：初始值直接取主题，避免首帧闪白/闪黑 */
function useTitlebarTone(page?: string, nav?: boolean): TbTone {
  const [tone, setTone] = useState<TbTone>(() => themeTone());
  const busyRef = useRef(false);
  const sigRef = useRef("");

  useEffect(() => {
    let alive = true;
    let timer = 0;

    const run = async (force: boolean) => {
      if (!alive || busyRef.current) return;
      busyRef.current = true;
      try {
        const sig = backdropSignature();
        if (!force && sig === sigRef.current) return;
        sigRef.current = sig;
        const next = await sampleTone();
        if (alive) setTone((prev) => (prev === next ? prev : next));
      } catch {
        // 采样失败静默：保留上一次 tone（首帧则保留主题兜底）
      } finally {
        busyRef.current = false;
      }
    };
    const schedule = (delay: number) => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => void run(true), delay);
    };

    const raf = requestAnimationFrame(() => void run(true));
    const poll = window.setInterval(() => void run(false), 1000);
    const onResize = () => schedule(200);
    window.addEventListener("resize", onResize);
    // 主题切换 = <html> class 变化；页面/轮播内容变化由轮询签名兜住
    const mo = new MutationObserver(() => schedule(0));
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });

    return () => {
      alive = false;
      cancelAnimationFrame(raf);
      window.clearInterval(poll);
      window.clearTimeout(timer);
      window.removeEventListener("resize", onResize);
      mo.disconnect();
    };
  }, [page, nav]);

  return tone;
}

export function TitleBar({
  title,
  nav,
  page,
  sidebarOpen,
  onToggleSidebar,
  onRefresh,
  refreshing,
  onToggleTheme,
  onHome,
}: {
  title: string;
  nav?: boolean;
  /** 当前"页"（首页/发现）：左上角那行字跟着它切（胶囊已挪到内容区浮层，见 NavCapsule） */
  page?: "home" | "discover";
  sidebarOpen?: boolean;
  onToggleSidebar?: () => void;
  onRefresh?: () => void;
  refreshing?: boolean;
  /** [lc-123-rev6] 传事件：深浅切换的圆形扩散要拿**按钮中心**做原点（getBoundingClientRect） */
  onToggleTheme?: (e: React.MouseEvent<HTMLButtonElement>) => void;
  /** [lc-123-rev4] 保留该 prop 以兼容调用方；[lc-1703] 顶栏已改为 JS 二值采样，不再消费它。 */
  chrome?: ChromeTone;
  /** [lc-1661] 点击左上角页名快速回首页；不传则仍渲染为纯文本（向后兼容） */
  onHome?: () => void;
}) {
  const win = getCurrentWindow();
  const tone = useTitlebarTone(page, nav);
  const crumb = page === "discover" ? "发现" : "首页";
  // [fix] 最大化状态：用于把"最大化/还原"按钮的图标在 Square（最大化）与 Copy（还原）之间切换。
  // 优先用 Tauri 的 isMaximized() 初始化，并监听窗口 resize（拖动改变大小/双击标题栏等都会触发）持续同步。
  const [isMaximized, setIsMaximized] = useState(false);
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    const sync = () => {
      win
        .isMaximized()
        .then((v) => {
          if (alive) setIsMaximized(v);
        })
        .catch(() => {
          /* 非 Tauri 环境/查询失败：保留上一次状态 */
        });
    };
    sync();
    win
      .onResized(sync)
      .then((fn) => {
        if (alive) unlisten = fn;
        else fn(); // 组件已卸载：立刻注销，避免监听泄漏
      })
      .catch(() => {
        /* 注册失败：仅失去实时同步，按钮点击仍可用 */
      });
    return () => {
      alive = false;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <>
      {/* [lc-1703] 二值色调：data-tone 决定图标/文字用**实心纯黑**还是**实心纯白**。
          采样逻辑见文件顶部 useTitlebarTone（背景图片/纯色 + 主题兜底）。
          品牌 logo 与关闭键不在 .titlebar 内部，各自由独立图层承载（关闭键同步 tone）。 */}
      <div className="titlebar" data-tone={tone} data-tauri-drag-region>
        <div className="tb-left" data-tauri-drag-region>
          {nav && onToggleSidebar && (
            <button
              className="tb-icon"
              title={sidebarOpen ? "收起侧栏" : "展开侧栏"}
              onClick={onToggleSidebar}
            >
              <IcoMenuL size={16} strokeWidth={1.8} />
            </button>
          )}
          {/* [lc-123] 左上角这行字 = **当前页名**（首页/发现），随胶囊切换（不再是"回首页"按钮） */}
          {/* [lc-1661] 用户要求：点这行字快速回首页。传了 onHome 才渲染成按钮（可点）；
              没传仍是纯文本，保持向后兼容。button 不是 data-tauri-drag-region 的命中目标，
              所以点击不会被拖拽区吞掉（同 .tb-icon 的既有行为）。 */}
          {nav &&
            (onHome ? (
              <button
                type="button"
                className="tb-crumb tb-crumb-btn"
                title="返回首页"
                aria-label="返回首页"
                onClick={onHome}
              >
                {crumb}
              </button>
            ) : (
              <span className="tb-crumb">{crumb}</span>
            ))}
          {nav && onRefresh && (
            <button
              className={`tb-icon${refreshing ? " spinning" : ""}`}
              // [lc-1640] 语义由"刷新首页数据"改为"按当前页强制刷新"（各视图分派到自己的 force 刷新）。
              title="刷新当前页数据"
              onClick={onRefresh}
              disabled={refreshing}
            >
              <span className="tb-spin">⟳</span>
            </button>
          )}
          {onToggleTheme && (
            <button className="tb-icon" title="切换深浅模式" onClick={(e) => onToggleTheme(e)}>
              ◐
            </button>
          )}
          {!nav && <span className="tb-page">{title}</span>}
        </div>
        {/* [lc-1702] 中部只保留窗口按钮里的最小化/最大化（关闭键独立成层）。
            整条顶栏仍是拖拽区：中间的空白不拦截拖拽。 */}
        <div className="tb-right" data-tauri-drag-region>
          <div className="tb-buttons">
            <button className="tb-btn" title="最小化" onClick={() => win.minimize()}>
              <IcoMinus size={16} strokeWidth={1.8} />
            </button>
            <button
              className="tb-btn"
              title={isMaximized ? "还原" : "最大化"}
              onClick={() => win.toggleMaximize()}
            >
              {isMaximized ? (
                <IcoCopy size={15} strokeWidth={1.8} />
              ) : (
                <IcoSquare size={15} strokeWidth={1.8} />
              )}
            </button>
          </div>
        </div>
      </div>

      {/* [lc-1703] 品牌 logo：独立图层（不参与色调切换，保持原渐变/浮动）。
          pointer-events:none —— 不吃鼠标事件，拖拽仍由下方 .titlebar 接管。 */}
      <div className="tb-brand-layer">
        <svg
          className="tb-brand tb-brand-logo"
          viewBox="0 0 800 240"
          xmlns="http://www.w3.org/2000/svg"
          role="img"
          aria-label="AniBox"
        >
          <defs>
            {/* 动漫感多色相渐变（青 → 蓝紫 → 紫 → 粉） */}
            <linearGradient id="aniboxLogoInk" x1="0" y1="0" x2="1" y2="0">
              <stop offset="0" stopColor="#5EC8E8" />
              <stop offset="0.33" stopColor="#7B8FE8" />
              <stop offset="0.66" stopColor="#C77BD8" />
              <stop offset="1" stopColor="#F58EB5" />
            </linearGradient>
          </defs>
          <g className="mark">
            <text
              x="400"
              y="162"
              textAnchor="middle"
              fontFamily="'Baloo 2', system-ui, sans-serif"
              fontWeight="800"
              fontSize="150"
              letterSpacing="1"
              fill="url(#aniboxLogoInk)"
            >AniBox</text>
          </g>
        </svg>
      </div>

      {/* [lc-1703] 关闭键独立图层：可点击（在 .titlebar 之上），同步 data-tone
          使用同一套实心纯黑/纯白；hover 红色保留。 */}
      <div className="tb-close-layer" data-tone={tone}>
        <button className="tb-btn tb-close" title="关闭" onClick={() => win.close()}>
          <IcoXL size={16} strokeWidth={1.8} />
        </button>
      </div>
    </>
  );
}

/* ────────────────────────── 登录页 ────────────────────────── */
