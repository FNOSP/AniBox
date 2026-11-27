/* [lc-2396] Polaris 详情页 · 横排箭头的可用态检测（剧集 / 剧照 / 主演 / 推荐 四处共用）。
 *
 * Polaris 的做法（`main.css:7150` + `09-poster-wall.js` 的箭头逻辑）：
 *   · 箭头 44×64，`left/right: -62px` 挂在容器**外面**（console 区左右各有 80px padding，外面还有空间）
 *   · 常态 `opacity: 0`，容器 hover 才显示
 *   · 滚到头 → `.disabled`（`opacity: .2` + `pointer-events: none`）
 *
 * 这里只做「还能不能往那边滚」的判定 + 一屏翻页（不接管原生滚动）：
 * 判定走 scrollLeft / scrollWidth / clientWidth，并在滚动、尺寸变化、内容变化时重算。
 */
import { useCallback, useEffect, useState, type RefObject } from "react";

/** 判定容差（亚像素布局下 scrollLeft 常带小数，差 1px 内视为到头）。 */
const EDGE = 2;

/** [lc-2403] 边缘渐隐方向 —— 「哪一侧还有滚出去的内容」。
 *   · none  = 两侧都到头（含内容不足一屏）→ **完全不淡出**
 *   · start = 只有左侧还有（已滚到底）→ 左侧淡出
 *   · end   = 只有右侧还有（在开头）→ 右侧淡出
 *   · both  = 两侧都有 → 两侧淡出
 *
 * 为什么不固定「两端都淡」：内容不足一屏时也淡，首尾两张卡会凭空缺一块，
 * 看起来像渲染坏了。渐隐只该表达「这边还有，可以滚」。
 * 也不照抄 Polaris @27600 的固定 10%/90%：我们的 scroller 带 40px 负边距补偿，
 * 固定百分比在宽屏上会淡掉将近半张卡。这里改成按**实际到头状态**着色。 */
export type RailFade = "none" | "start" | "end" | "both";

export function useRailArrows(
  ref: RefObject<HTMLElement | null>,
  deps: unknown[] = [],
): { atStart: boolean; atEnd: boolean; scrollBy: (dir: -1 | 1) => void; fade: RailFade } {
  const [atStart, setAtStart] = useState(true);
  const [atEnd, setAtEnd] = useState(true);

  const measure = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const max = el.scrollWidth - el.clientWidth;
    setAtStart(el.scrollLeft <= EDGE);
    // 内容不足一屏时 max <= 0，两个方向都算「到头」（箭头都置灰）
    setAtEnd(max <= EDGE || el.scrollLeft >= max - EDGE);
  }, [ref]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    measure();
    el.addEventListener("scroll", measure, { passive: true });
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    // 子项数量变化（集数/剧照数）也要重算：观察第一个子元素的存在性靠 MutationObserver
    const mo = new MutationObserver(measure);
    mo.observe(el, { childList: true });
    return () => {
      el.removeEventListener("scroll", measure);
      ro.disconnect();
      mo.disconnect();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [measure, ...deps]);

  const scrollBy = useCallback(
    (dir: -1 | 1) => {
      const el = ref.current;
      if (!el) return;
      // 一屏翻页（留 8% 重叠，读者能接上位置）
      el.scrollBy({ left: dir * el.clientWidth * 0.92, behavior: "smooth" });
    },
    [ref],
  );

  /* [lc-2403] 渐隐方向由「还没到头的那一侧」决定 */
  const fade: RailFade =
    atStart && atEnd
      ? "none"
      : atStart
        ? "end"
        : atEnd
          ? "start"
          : "both";

  return { atStart, atEnd, scrollBy, fade };
}
