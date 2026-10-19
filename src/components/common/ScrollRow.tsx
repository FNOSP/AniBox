/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";

export function ScrollRow({
  children,
  boxClass,
  imgSel = ".poster-box",
  inSec = false,
  center = false,
}: {
  children: ReactNode;
  boxClass?: string;
  /** 量哪个元素的高度作为箭头垂直居中的基准（默认 .poster-box；剧照行传 img 选择器） */
  imgSel?: string;
  /** 这一行嵌在带内边距的卡片里（如 .tmdb-sec）→ 箭头左右内收到 14px，不压卡片边框 */
  inSec?: boolean;
  /** [lc-1714] 首页媒体库行：卡片轨道在容器内水平居中；超宽时照常横向滚动 */
  center?: boolean;
}) {
  const boxRef = useRef<HTMLDivElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const [hover, setHover] = useState(false);
  const [atStart, setAtStart] = useState(true);
  const [atEnd, setAtEnd] = useState(false);

  const sync = useCallback(() => {
    const el = boxRef.current;
    if (!el) return;
    // 1px 容差：分数缩放（125%/150%）下 scrollLeft 是小数，等于阈值时不该误判成"还能滚"
    setAtStart(el.scrollLeft <= 1);
    setAtEnd(el.scrollLeft + el.clientWidth >= el.scrollWidth - 1);
    // [lc-1409] 箭头要**相对图片区域垂直居中**（用户要求），而不是相对整行 ——
    //   整行还含图片下方的标题/副标题，按整行居中会让箭头落到图片偏下的位置。
    //   这里量出第一张卡片的图片高度写进 CSS 变量（.poster-box 有 aspect-ratio，
    //   高度只取决于宽度，不依赖图片是否加载完成，所以量得到就是准的）。
    const poster = el.querySelector<HTMLElement>(imgSel);
    const wrap = wrapRef.current;
    if (wrap) {
      // 量不到图片高度时（首帧未布局/结构变化）退化为"整行高度" ——
      // 箭头就变成相对整行居中，比固定一个猜的像素值稳（继续观看与媒体库的图片高度差很多，
      // 任何写死的兜底值都必然对其中一种不准）。
      const imgH = poster && poster.offsetHeight > 0 ? poster.offsetHeight : el.clientHeight;
      if (imgH > 0) wrap.style.setProperty("--srow-img-h", `${imgH}px`);
    }
  }, [imgSel]);

  useEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    sync();
    el.addEventListener("scroll", sync, { passive: true });
    let ro: ResizeObserver | null = null;
    if (typeof ResizeObserver !== "undefined") {
      ro = new ResizeObserver(sync);
      ro.observe(el);
    }
    window.addEventListener("resize", sync);
    return () => {
      el.removeEventListener("scroll", sync);
      ro?.disconnect();
      window.removeEventListener("resize", sync);
    };
  }, [sync, children, imgSel]);

  const page = (dir: -1 | 1) => {
    const el = boxRef.current;
    if (!el) return;
    const step = Math.max(160, el.clientWidth * 0.9);
    el.scrollBy({ left: dir * step, behavior: "smooth" });
  };

  return (
    <div
      className={`srow${inSec ? " insec" : ""}`}
      ref={wrapRef}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <div
        className={`hscroll${boxClass ? ` ${boxClass}` : ""}${center ? " srow-center" : ""}`}
        ref={boxRef}
      >
        {center ? <div className="srow-track">{children}</div> : children}
      </div>
      <button
        type="button"
        className={`srow-nav srow-prev${hover && !atStart ? " is-on" : ""}`}
        title="向左翻"
        aria-label="向左翻"
        disabled={atStart}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => page(-1)}
      >
        ‹
      </button>
      <button
        type="button"
        className={`srow-nav srow-next${hover && !atEnd ? " is-on" : ""}`}
        title="向右翻"
        aria-label="向右翻"
        disabled={atEnd}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => page(1)}
      >
        ›
      </button>
    </div>
  );
}

/** [lc-1414] 头像兜底：**拉不到图就用内置图标**，绝不显示裂图（用户明确要求）。
 *
 *  为什么需要：Bangumi 头像 URL 基本都带 `?r=…&hd=1` 查询串，历史上前端把它编成 %3F、
 *  Rust 侧没还原 → 上游 404 → 整片裂图（根因已单独修）。但上游偶发 404/超时仍可能发生，
 *  所以渲染层也要兜住：onError 后换成图标，并按用户名做**稳定的颜色**（刷新不变）。
 *
 *  用内置图标而不是灰底/首字母：与项目里其它占位（.poster-empty 用 IcoFilmL）同一套语言，
 *  视觉上"这是个占位"比"这是个坏图"清楚。
 */
