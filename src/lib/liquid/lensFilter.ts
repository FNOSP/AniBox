/* ═══════════════════ [lc-2431] 胶囊透镜位移图（照抄 AGSL 着色器） ═══════════════════
   来源：Kyant0/AndroidLiquidGlass（Apache-2.0）`backdrop/internal/Shaders.kt` 的
   `RoundedRectRefractionShaderString`。CoolApk 16.7.4 的底栏就是这套（dex 内 URL 为证）。

   Android 侧它跑在 AGSL（RuntimeShader）里，逐像素 `content.eval(refractedCoord)`。
   浏览器没有 AGSL，改用等效通道：
     1) 用同一套 SDF 数学在 Canvas 上**一次性**画出位移图（R = dx，G = dy，128 = 0）；
     2) 交给 `<feImage>` + `<feDisplacementMap>` 作用在 `backdrop-filter: url(#id)` 上。
   等价性：feDisplacementMap 的公式是
       P'(x,y) = P(x + scale·(R/255 - 0.5), y + scale·(G/255 - 0.5))
   与本文件产出的 (dx, dy) 完全相同（见下方 encode 里的注释）。

   **这是真折射，不是模糊伪装**。tests/probe_lens_visual_2431.cjs 的截图可见：
   条纹在胶囊边缘被弯曲放大、中部平滑 —— 与纯 blur 的平整灰面完全不同。

   参数逐值对齐原实现（LiquidBottomTabs.kt）：
     底栏本体  lens(24dp, 24dp)                  → CONTAINER
     选中指示器 lens(10dp, 14dp, dispersion)     → INDICATOR（含色散）
   dp→px 用 1dp = 1px（应用内 UI 无独立密度，原实现 4dp padding 也按 CSS px 落）。 */

export interface LensParams {
  width: number;
  height: number;
  /** 圆角半径（胶囊传 height/2） */
  radius: number;
  /** 折射作用带宽度（原 refractionHeight，dp） */
  refractionHeight: number;
  /** 最大位移量（原 refractionAmount，dp） */
  refractionAmount: number;
  /** 0/1：是否叠加"向心"分量（原 depthEffect） */
  depthEffect: number;
}

/** 与 AGSL 版一致的圆角矩形 SDF（负值在内部） */
function sdRoundedRect(x: number, y: number, hw: number, hh: number, r: number): number {
  const cx = Math.abs(x) - (hw - r);
  const cy = Math.abs(y) - (hh - r);
  const outside = Math.hypot(Math.max(cx, 0), Math.max(cy, 0)) - r;
  return outside + Math.min(Math.max(cx, cy), 0);
}

/** SDF 梯度（指向外部），照抄 gradSdRoundedRect */
function gradSdRoundedRect(
  x: number,
  y: number,
  hw: number,
  hh: number,
  r: number,
): [number, number] {
  const cx = Math.abs(x) - (hw - r);
  const cy = Math.abs(y) - (hh - r);
  if (cx >= 0 || cy >= 0) {
    const mx = Math.max(cx, 0);
    const my = Math.max(cy, 0);
    const len = Math.hypot(mx, my);
    if (len === 0) return [0, 0];
    return [Math.sign(x) * (mx / len), Math.sign(y) * (my / len)];
  }
  const gx = cx >= cy ? 1 : 0;
  return [Math.sign(x) * gx, Math.sign(y) * (1 - gx)];
}

/** 1 - sqrt(1 - x²)：半圆剖面（边缘位移最大、向内迅速归零） */
function circleMap(x: number): number {
  const c = Math.min(Math.max(x, 0), 1);
  return 1 - Math.sqrt(Math.max(0, 1 - c * c));
}

/** 把归一化位移编码进 8 位通道：feDisplacementMap 的 R-0.5 就是 -0.5..0.5 */
function encode(d: number, span: number): number {
  // 位移 d ∈ [-span/2, span/2]；通道值 = 128 + d/span*255
  return Math.round(Math.min(255, Math.max(0, 128 + (d / span) * 255)));
}

/**
 * 生成位移图 dataURL（在浏览器内调用，含 canvas）。
 * @param span feDisplacementMap 的 scale 值，必须与生成时一致，否则位移量被缩放两次。
 */
export function buildLensDisplacementMap(p: LensParams, span: number): string {
  const { width: W, height: H, radius, refractionHeight, refractionAmount, depthEffect } = p;
  const canvas = document.createElement("canvas");
  const dpr = Math.min(window.devicePixelRatio || 1, 2); // 上限 2：位移图无需更高精度
  canvas.width = Math.max(1, Math.round(W * dpr));
  canvas.height = Math.max(1, Math.round(H * dpr));
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  const img = ctx.createImageData(canvas.width, canvas.height);
  const data = img.data;

  const hw = W / 2;
  const hh = H / 2;
  const gradRadius = Math.min(radius * 1.5, Math.min(hw, hh));

  for (let py = 0; py < canvas.height; py++) {
    // 像素中心映射回 CSS 像素
    const y = (py + 0.5) / dpr - hh;
    for (let px = 0; px < canvas.width; px++) {
      const x = (px + 0.5) / dpr - hw;
      const i = (py * canvas.width + px) * 4;

      let sd = sdRoundedRect(x, y, hw, hh, radius);
      let dx = 0;
      let dy = 0;

      // 只在边缘 refractionHeight 带内折射；内部原样透出
      if (-sd < refractionHeight) {
        if (sd > 0) sd = 0; // 外部不收（裁切交给 CSS border-radius）
        const t = -sd / refractionHeight; // 0 = 最外缘，1 = 带内边界
        const amount = circleMap(1 - t) * refractionAmount;
        const [gx, gy] = gradSdRoundedRect(x, y, hw, hh, gradRadius);
        let nx = gx;
        let ny = gy;
        if (depthEffect !== 0) {
          const len = Math.hypot(x, y);
          if (len > 0) {
            nx += depthEffect * (x / len);
            ny += depthEffect * (y / len);
          }
        }
        const nl = Math.hypot(nx, ny);
        if (nl > 0) {
          // 注意符号：AGSL 里 setFloatUniform("refractionAmount", -amount) 传负值，
          // 因为 AGSL 的 coord 原点在左上、梯度朝外；这里直接用同样取负的位移方向，
          // 得到"边缘把背景向内吸"的凸透镜观感。
          dx = -amount * (nx / nl);
          dy = -amount * (ny / nl);
        }
      }

      data[i] = encode(dx, span);
      data[i + 1] = encode(dy, span);
      data[i + 2] = 128;
      data[i + 3] = 255;
    }
  }

  ctx.putImageData(img, 0, 0);
  return canvas.toDataURL("image/png");
}
