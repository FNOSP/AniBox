/* 自适应反色（二值色调）—— 顶栏与详情页头图共用的唯一实现。
 *
 * 原实现是 TitleBar 内部的 `useTitlebarTone`（[lc-1703]）：在顶栏 40px 横带上取采样点，
 * 用 `elementsFromPoint` 穿透浮层找到**真实背景**（<img> / background-image / 纯色），
 * 用 canvas 算平均亮度；阈值 150 → data-tone="light"（背景亮，前景纯黑）或
 * data-tone="dark"（背景暗，前景纯白）。采样失败（跨域/解码/404）回退主题。
 *
 * [lc-1709] 抽成共享模块，供详情页头图复用 —— 顶栏与详情页**不再各写一套**。
 * 差异只有采样区域：顶栏固定顶部 40px 横带；详情页传入正文区的矩形（backdrop 大图）。
 * ───────────────────────────────────────────────────────────────────── */

import { useEffect, useRef, useState } from "react";

export type Tone = "light" | "dark";

export const TONE_LUMA_THRESHOLD = 150;

type CssColor = { r: number; g: number; b: number; a: number };
type BackdropCand = { el: Element; kind: "image" | "color" };
export type ToneBand = { top: number; bottom: number };

export type ToneSampleConfig = {
  /** 采样点（窗口坐标）。为空 → 走主题兜底。 */
  points: () => { x: number; y: number }[];
  /** 命中这些浮层时跳过，继续向下找真实背景。 */
  overlaySelector?: string;
  /** 图片只取这条横带（窗口坐标）；省略 = 元素整个可见矩形。 */
  band?: () => ToneBand | null;
  /** 额外签名：轮播/内容变化时强制重采。 */
  extraKey?: string;
};

export function parseCssColor(input: string): CssColor | null {
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

export function relLuma(r: number, g: number, b: number): number {
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function isDarkTheme(): boolean {
  return document.documentElement.classList.contains("dark");
}

/** 主题兜底：浅色主题背景亮 → "light"（前景纯黑）；深色主题背景暗 → "dark"（前景纯白）。 */
export function themeTone(): Tone {
  return isDarkTheme() ? "dark" : "light";
}

/** 主题底色（用于把半透明背景色合成出不透明结果）。 */
export function themeBase(): [number, number, number] {
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
  // 轮播里未激活的层 opacity:0：视觉上不可见，不能拿来算色调。
  if (cs.visibility === "hidden" || Number(cs.opacity) < 0.05) return null;
  const bi = cs.backgroundImage;
  if (bi && bi !== "none" && bi.includes("url(")) return "image";
  const c = parseCssColor(cs.backgroundColor);
  if (c && c.a > 0.05) return "color";
  return null;
}

/** 沿采样点穿透浮层拿到真实背景元素（每个点取最上层可采样者）。 */
function pickBackdrops(cfg: ToneSampleConfig): BackdropCand[] {
  const w = window.innerWidth;
  const h = window.innerHeight;
  if (w <= 0 || h <= 0) return [];
  const seen = new Set<Element>();
  const out: BackdropCand[] = [];
  for (const p of cfg.points()) {
    const x = Math.min(w - 1, Math.max(0, Math.round(p.x)));
    const y = Math.min(h - 1, Math.max(0, Math.round(p.y)));
    const stack = document.elementsFromPoint(x, y);
    for (const el of stack) {
      if (cfg.overlaySelector && el.closest(cfg.overlaySelector)) continue;
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

/** 背景签名：用于避免每秒重复读像素。 */
function backdropSignature(cfg: ToneSampleConfig): string {
  const parts = pickBackdrops(cfg).map(({ el, kind }) => {
    const r = el.getBoundingClientRect();
    const box = `${kind}:${Math.round(r.left)},${Math.round(r.top)},${Math.round(r.width)},${Math.round(r.height)}`;
    if (el instanceof HTMLImageElement) return `${box}:${el.currentSrc || el.src}`;
    const cs = getComputedStyle(el);
    return `${box}:${cs.backgroundImage}:${cs.backgroundColor}`;
  });
  const band = cfg.band?.() ?? null;
  const bandKey = band ? `${Math.round(band.top)}-${Math.round(band.bottom)}` : "-";
  return `${parts.join(";")}|${bandKey}|${cfg.extraKey ?? ""}|${window.innerWidth}x${window.innerHeight}|${
    isDarkTheme() ? "d" : "l"
  }`;
}

/** 纯色背景亮度：alpha < 1 时与主题底色合成。 */
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
 * 取"元素在指定横带内"的平均亮度（横带为窗口坐标）。
 * <img> 走 object-fit 的 cover/contain/fill 映射；CSS background-image 用 background-size 近似。
 * 跨域污染 / 图片加载失败 → null（调用方走主题兜底）。
 */
async function imageLuma(el: Element, band: ToneBand | null): Promise<number | null> {
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

  // 横带 [band.top, band.bottom] 与该元素可见矩形的交集（元素局部坐标）。
  const bandTop = band ? band.top : rect.top;
  const bandBottom = band ? band.bottom : rect.bottom;
  const localTop = Math.max(0, bandTop - rect.top);
  const localBottom = Math.min(rect.height, bandBottom - rect.top);
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

/** 采样一次，返回二值色调。 */
export async function sampleAdaptiveTone(cfg: ToneSampleConfig): Promise<Tone> {
  const band = cfg.band?.() ?? null;
  const lumas: number[] = [];
  for (const c of pickBackdrops(cfg)) {
    const l = c.kind === "image" ? await imageLuma(c.el, band) : colorLuma(c.el);
    if (l != null) lumas.push(l);
  }
  if (!lumas.length) return themeTone();
  const avg = lumas.reduce((a, b) => a + b, 0) / lumas.length;
  return avg > TONE_LUMA_THRESHOLD ? "light" : "dark";
}

/**
 * 自适应色调 hook：1s 轮询 + 背景签名去重；resize 200ms 防抖；
 * 主题 class 变化、页面/内容变化（deps）都会强制重采。首帧直接取主题兜底。
 */
export function useAdaptiveTone(cfg: ToneSampleConfig, deps: unknown[] = []): Tone {
  const [tone, setTone] = useState<Tone>(() => themeTone());
  const cfgRef = useRef(cfg);
  cfgRef.current = cfg;
  const busyRef = useRef(false);
  const sigRef = useRef("");

  useEffect(() => {
    let alive = true;
    let timer = 0;

    const run = async (force: boolean) => {
      if (!alive || busyRef.current) return;
      busyRef.current = true;
      try {
        const sig = backdropSignature(cfgRef.current);
        if (!force && sig === sigRef.current) return;
        sigRef.current = sig;
        const next = await sampleAdaptiveTone(cfgRef.current);
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return tone;
}
