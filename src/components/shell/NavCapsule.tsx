/* ═══════════════════ [lc-2433] 「首页 | 发现」液态玻璃 tab 栏（视觉保留 / 动画回到原版） ═══════════════════
   视觉：CoolApk 16.7.4 底栏的液态玻璃，出处 https://github.com/Kyant0/AndroidLiquidGlass
   （Apache-2.0，其 dex 内留有此 URL）。核心是「SDF 圆角矩形 + circleMap 边缘折射」的
   AGSL 着色器，我们在浏览器侧用等效通道复刻（见 src/lib/liquid/lensFilter.ts 头注释）。

   动画：**回到 lc-123-rev6 那一套**（用户明确要求），不再用弹簧/拖拽状态机：
     · 指示器 = 绝对定位、宽一个槽，translateX(0|100%) 在两槽间搬家；
     · 缓动 --ease-spring（带过冲）+ --dur-page（285ms）＝"水滴"黏滞感；
     · 切换瞬间内部 scaleX(.92)→1，两个不同 animation-name 保证每次切换都重放；
     · 选中图标 pop 一次。
   这三处与 lc-123-rev6 的 .nav-cap-ind / navBlobHome / navBlobDiscover / navIconPop 逐值一致。

   [lc-1535] 本组件最早从 App.tsx 拆出。
   [lc-2431] 首次改造成液态玻璃（当时连带换了弹簧动效）。
   [lc-2433] 用户反馈"动画改成原来那个" —— 去掉拖拽/弹簧/按压鼓包，动画回退；
            液态玻璃视觉、滤镜注册、降级路径全部保留。 */

import { useEffect, useMemo, useRef, useState } from "react";
import { House as IcoHouseL, Search as IcoSearchL } from "lucide-react";
import { upsertLensFilter, supportsBackdropFilterUrl } from "../../lib/liquid/lensRegistry";
import { FILTER_IDS, FALLBACK_CLASS, PANEL_REFRACTION } from "../../lib/liquid/types";
import { prefersReducedMotion } from "../../lib/motion";

/** 栏体高度（CSS px）：沿用旧胶囊的视觉重量 */
const BAR_HEIGHT = 40;

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

  /* 生成面板折射滤镜。依赖 barWidth：位移图尺寸必须等于元素尺寸，否则位移被拉伸。 */
  useEffect(() => {
    if (reduced || !supportsBackdropFilterUrl()) return;
    if (barWidth <= 0) return;
    /* 只建面板这一条：滑块自身不带 backdrop-filter。
       实测两者叠加时拖动帧间隔 max 37.7ms/掉 2 帧，只留面板 18.8ms/0 掉帧 ——
       滑块每帧位移，它的 backdrop-filter 要重新采样"面板已滤镜的输出"，双重采样。 */
    const ok = upsertLensFilter({
      id: FILTER_IDS.panel,
      rect: { width: barWidth, height: BAR_HEIGHT, radius: BAR_HEIGHT / 2 },
      refractionHeight: PANEL_REFRACTION.height,
      refractionAmount: PANEL_REFRACTION.amount,
      depthEffect: PANEL_REFRACTION.depth,
    });
    setLensReady(ok);
  }, [barWidth, reduced]);

  /* 量宽度（位移图按尺寸生成，尺寸变了要重做） */
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
    <div ref={barRef} className={cls} data-side={page} role="tablist" aria-label="主导航">
      {/* 整条栏的玻璃层：折射 + 模糊 + 中性底 + 高光。
          与按钮层分开，是因为 backdrop-filter 作用在元素**背后**的已绘制内容上，
          玻璃必须先画、按钮后画（否则按钮自己的背景会盖住折射效果）。 */}
      <span className="nav-cap-glass" aria-hidden="true" />

      {/* [lc-2433] 滑动指示器：绝对定位、宽一个槽，translateX(0|100%) 搬家。
          data-side 在父层，缩放在 CSS 里按 .nav-cap[data-side] 下发。 */}
      <span className="nav-cap-ind" aria-hidden="true">
        <i />
      </span>

      <button
        role="tab"
        aria-selected={page === "home"}
        aria-label="首页"
        title="首页"
        className={`nav-cap-l${page === "home" ? " on" : ""}`}
        onClick={() => onNav("home")}
      >
        <IcoHouseL size={18} strokeWidth={1.8} />
      </button>
      <button
        role="tab"
        aria-selected={page === "discover"}
        aria-label="发现"
        title="发现"
        className={`nav-cap-l${page === "discover" ? " on" : ""}`}
        onClick={() => onNav("discover")}
      >
        <IcoSearchL size={18} strokeWidth={1.8} />
      </button>
    </div>
  );
}
