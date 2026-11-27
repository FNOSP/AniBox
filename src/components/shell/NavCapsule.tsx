/* ═══════════════════ [lc-2431] 「首页 | 发现」液态玻璃 tab 栏 ═══════════════════
   效果来源：CoolApk 16.7.4 底栏。反查确认其实现在
   https://github.com/Kyant0/AndroidLiquidGlass （Apache-2.0，dex 内留有此 URL），
   核心是「SDF 圆角矩形 + circleMap 边缘折射 + 7 采样色散」的 AGSL 着色器。
   我们在浏览器侧用等效通道复刻（见 src/lib/liquid/*.ts 各文件头注释）：
     · 面板：折射（lens 24/24）—— 整条栏是一块透镜
     · 滑块：折射（静止 8/10，按下 10/14）+ 色散 + 按下放大 1.2
     · 动效：Compose spring 逐值搬运（1.0/1000 位置，0.6/0.7-250 形变）
     · 交互：可**直接拖动**滑块，松手弹到最近槽位（原版 DampedDragAnimation）

   与旧版（lc-123-rev6 / lc-2111 分段控件）的关系：定位、aria 语义、点击切页保留，
   视觉与交互升级；旧的 .nav-cap-ind 水滴指示器被液态滑块取代。

   [lc-1535] 本组件最早从 App.tsx 拆出。 */

import { useEffect, useMemo, useRef, useState } from "react";
import { House as IcoHouseL, Search as IcoSearchL } from "lucide-react";
import { useLiquidTabs } from "../../lib/liquid/useLiquidTabs";
import { upsertLensFilter, supportsBackdropFilterUrl } from "../../lib/liquid/lensRegistry";
import {
  FILTER_IDS,
  FALLBACK_CLASS,
  PANEL_REFRACTION,
  THUMB_PRESS_REFRACTION,
} from "../../lib/liquid/types";
import { prefersReducedMotion } from "../../lib/motion";

/** 栏体高度（CSS px）：沿用旧胶囊的视觉重量 */
const BAR_HEIGHT = 40;
/** 滑块相对栏体的内缩 */
const THUMB_INSET = 4;

export function NavCapsule({
  page,
  onNav,
}: {
  page: "home" | "discover";
  onNav: (p: "home" | "discover") => void;
}) {
  const [reduced] = useState(() => prefersReducedMotion());
  const [lensReady, setLensReady] = useState(false);
  const barRef = useRef<HTMLDivElement | null>(null);
  const [barWidth, setBarWidth] = useState(0);

  const index = page === "home" ? 0 : 1;
  const tabs = useLiquidTabs(2, index, (i) => onNav(i === 0 ? "home" : "discover"), reduced);

  /* 生成三条滤镜（面板 / 滑块静止 / 滑块按下）。
     依赖 barWidth：位移图尺寸必须等于元素尺寸，否则位移量被拉伸。
     首帧 barWidth=0 时跳过，等量到真实宽度再建。 */
  useEffect(() => {
    if (reduced || !supportsBackdropFilterUrl()) return;
    if (barWidth <= 0) return;
    // 滑块 = 一个槽 = (栏宽 − 左右 padding) / 2；高度 = 栏高 − 上下 padding。
    // 必须与 CSS 里 .nav-cap-thumb 的 50% / 100% 完全一致，否则位移图被拉伸。
    const thumbW = (barWidth - THUMB_INSET * 2) / 2;
    const thumbH = BAR_HEIGHT - THUMB_INSET * 2;
    // 滑块只建"按下"那一条：静止态在原版里没有折射（progress=0 ⇒ lens(0,0)），
    // 详见 CSS 里 .nav-cap-thumb-lens 的注释。
    const ok =
      upsertLensFilter({
        id: FILTER_IDS.panel,
        rect: { width: barWidth, height: BAR_HEIGHT, radius: BAR_HEIGHT / 2 },
        refractionHeight: PANEL_REFRACTION.height,
        refractionAmount: PANEL_REFRACTION.amount,
        depthEffect: PANEL_REFRACTION.depth,
      }) &&
      upsertLensFilter({
        id: FILTER_IDS.thumbPress,
        rect: { width: thumbW, height: thumbH, radius: thumbH / 2 },
        refractionHeight: THUMB_PRESS_REFRACTION.height,
        refractionAmount: THUMB_PRESS_REFRACTION.amount,
        depthEffect: THUMB_PRESS_REFRACTION.depth,
      });
    setLensReady(ok);
  }, [barWidth, reduced]);

  /* 量宽度 */
  useEffect(() => {
    const el = barRef.current;
    if (!el) return;
    const measure = () => setBarWidth(el.clientWidth);
    measure();
    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver(measure);
      ro.observe(el);
      return () => ro.disconnect();
    }
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, []);

  const cls = useMemo(() => `nav-cap${lensReady ? "" : ` ${FALLBACK_CLASS}`}`, [lensReady]);

  return (
    <div
      ref={barRef}
      className={cls}
      data-side={page}
      data-pressed={tabs.pressed ? "on" : "off"}
      role="tablist"
      aria-label="主导航"
      onPointerDown={tabs.onPointerDown}
      onPointerMove={tabs.onPointerMove}
      onPointerUp={tabs.onPointerUp}
      onPointerCancel={tabs.onPointerCancel}
      onClickCapture={tabs.onClickCapture}
    >
      {/* 整条栏的玻璃层：折射 + 模糊 + 中性底 + 高光。
          与按钮层分开，是因为 backdrop-filter 作用在元素**背后**的已绘制内容上，
          玻璃必须先画、按钮后画（否则按钮自己的背景会盖住折射效果）。 */}
      <span className="nav-cap-glass" aria-hidden="true" />

      {/* 滑块 + 按钮层：--lx / --lpress / --lscale* 由 JS 每帧写入（不经过 React） */}
      <div className="nav-cap-track" ref={tabs.trackRef}>
        <span className="nav-cap-thumb" aria-hidden="true">
          <i className="nav-cap-thumb-lens" />
          <i className="nav-cap-thumb-spec" />
        </span>
        <button
          role="tab"
          aria-selected={page === "home"}
          aria-label="首页"
          title="首页"
          className={`nav-cap-l${page === "home" ? " on" : ""}`}
          onClick={tabs.makeClickHandler(0)}
        >
          <IcoHouseL size={18} strokeWidth={1.8} />
        </button>
        <button
          role="tab"
          aria-selected={page === "discover"}
          aria-label="发现"
          title="发现"
          className={`nav-cap-l${page === "discover" ? " on" : ""}`}
          onClick={tabs.makeClickHandler(1)}
        >
          <IcoSearchL size={18} strokeWidth={1.8} />
        </button>
      </div>
    </div>
  );
}
