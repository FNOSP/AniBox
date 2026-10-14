/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { ChromeTone } from "../types";

export const CHROME_LUMA_THRESHOLD = 150;
/** 只取图片顶部这一条（顶栏 40px 在满幅 hero 上覆盖的大致比例） */

export const CHROME_STRIP_RATIO = 0.08;
/** 采样画布的目标宽度（按比例缩，够算均值且省时间） */

export const CHROME_SAMPLE_WIDTH = 64;

export const chromeToneCache = new Map<string, ChromeTone>();

/** 采样一张图的顶部条带 → "dark"（亮底）/ "light"（暗底）；任何失败（跨域/解码/404）→ null（走兜底） */

export async function sampleChromeTone(src: string): Promise<ChromeTone | null> {
  const hit = chromeToneCache.get(src);
  if (hit) return hit;
  try {
    const img = new Image();
    img.crossOrigin = "anonymous"; // 必须：不带这个，画布被污染 → getImageData 抛 SecurityError
    img.decoding = "async";
    img.src = src;
    await img.decode();
    const sw = img.naturalWidth;
    const sh = Math.max(1, Math.round(img.naturalHeight * CHROME_STRIP_RATIO));
    if (!sw || !sh) return null;
    const dw = Math.max(1, Math.min(CHROME_SAMPLE_WIDTH, sw));
    const dh = Math.max(1, Math.round(sh * (dw / sw)));
    const cv = document.createElement("canvas");
    cv.width = dw;
    cv.height = dh;
    const ctx = cv.getContext("2d", { willReadFrequently: true });
    if (!ctx) return null;
    ctx.drawImage(img, 0, 0, sw, sh, 0, 0, dw, dh); // 源矩形 = 原图顶部 8%（宽度按比例缩到 ≤64px）
    const { data } = ctx.getImageData(0, 0, dw, dh); // ← 跨域被拦时在这里抛错 → catch → null
    let sum = 0;
    let n = 0;
    for (let i = 0; i < data.length; i += 4) {
      sum += 0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2];
      n++;
    }
    if (!n) return null;
    const tone: ChromeTone = sum / n > CHROME_LUMA_THRESHOLD ? "dark" : "light";
    chromeToneCache.set(src, tone);
    return tone;
  } catch {
    return null; // 跨域（SecurityError）/解码失败/图片 404：一律交给调用方走兜底分支
  }
}

/** item 详情图片字段取值（对齐主仓库 carousel/images.ts pickImg：字符串直取；
 *  数组取首项的 path/url/字面量——新版 fnOS 实测为字符串，数组形态留作兼容） */
