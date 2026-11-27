/* [lc-2396] Polaris 详情页 · 剧照 / 主演 / 相关推荐三条次级横排
 *
 * 逐条对照 `Polaris main.css`：
 *
 *  剧照（8357）
 *   · `.pw-detail-stills-section`   margin-top 40px；`[hidden]` 整块不显示
 *   · `.pw-detail-stills-scroller`  flex、align-items center、gap 16、padding 24px 40px、
 *                                   margin -24px -40px、overscroll-behavior-inline: contain
 *   · `.pw-detail-still-card`       clamp(260px,19.8vw,380px)、aspect-ratio 16/9.2、radius 14
 *   · 图片 opacity 0→1（0.32s）淡入；加载中 shimmer 骨架
 *
 *  主演（8407）
 *   · `.pw-detail-cast-row`   flex、gap 20、padding 20px 40px、margin -20px -40px
 *   · `.pw-detail-c-unit`     80px 宽竖排、hover scale(1.15)、z-index 10
 *   · `.pw-detail-c-img`      80px 正圆、border 1px white.08、两层阴影
 *   · `.pw-detail-c-name`     12px、白、居中、opacity .8
 *
 *  相关推荐（8533）
 *   · `.pw-detail-rec-card`   flex 0 0 280px、aspect-ratio 16/9、radius 12
 *   · 动效比别处慢：0.5s cb(.16,1,.3,1)；hover 只缩图 1.07、浮层淡入
 */
import { useRef } from "react";
import { useRailArrows } from "./RailArrows";
import { imgUrl, tmdbImg } from "../../../lib/images";
import type { TmdbCreditPerson } from "../../../types";
import type { TmdbRecommendation } from "../tabs/RelatedTab";

/** 横排箭头的公共外壳（三处结构相同，只有滚容器类名不同）。 */
function Rail({
  children,
  boxClass,
  ariaLabel,
  deps,
}: {
  children: (ref: React.RefObject<HTMLDivElement | null>) => React.ReactNode;
  boxClass: string;
  ariaLabel: string;
  deps: unknown[];
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const { atStart, atEnd, scrollBy } = useRailArrows(ref, deps);
  return (
    <div className={`pdt-rail ${boxClass}-rail`} role="region" aria-label={ariaLabel}>
      <button
        type="button"
        className={`pdt-rail-arrow left${atStart ? " disabled" : ""}`}
        onClick={() => scrollBy(-1)}
        aria-label="向左浏览"
        tabIndex={atStart ? -1 : 0}
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <polyline points="15 6 9 12 15 18" />
        </svg>
      </button>
      {children(ref)}
      <button
        type="button"
        className={`pdt-rail-arrow right${atEnd ? " disabled" : ""}`}
        onClick={() => scrollBy(1)}
        aria-label="向右浏览"
        tabIndex={atEnd ? -1 : 0}
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <polyline points="9 6 15 12 9 18" />
        </svg>
      </button>
    </div>
  );
}

/** 剧照横排：`stills` 是 TMDB file_path（形如 /abc.jpg）。 */
export function PolarisStills({
  stills,
  onZoom,
}: {
  stills: string[];
  onZoom?: (path: string) => void;
}) {
  if (stills.length === 0) return null;
  return (
    <section className="pdt-stills">
      <div className="pdt-section-head">
        <span className="pdt-heading">剧照</span>
        <span className="pdt-section-hint">{stills.length} 张</span>
      </div>
      <Rail boxClass="pdt-stills-scroller" ariaLabel="剧照" deps={[stills.length]}>
        {(ref) => (
          <div className="pdt-stills-scroller" ref={ref}>
            {stills.map((p) => (
              <button
                type="button"
                className="pdt-still-card"
                key={p}
                title="点击查看原图"
                onClick={() => onZoom?.(p)}
              >
                <img
                  className="pdt-still-img"
                  src={p.startsWith("http") ? p : tmdbImg(p, "w780")}
                  loading="lazy"
                  alt=""
                  draggable={false}
                />
              </button>
            ))}
          </div>
        )}
      </Rail>
    </section>
  );
}

/** 主演横排：80px 正圆头像 + 名字（点击进演员页）。 */
export function PolarisCast({
  cast,
  onOpenPerson,
}: {
  cast: TmdbCreditPerson[];
  onOpenPerson?: (c: TmdbCreditPerson) => void;
}) {
  const list = cast.filter((c) => String(c.name ?? "").trim()).slice(0, 24);
  if (list.length === 0) return null;
  return (
    <section className="pdt-cast">
      <div className="pdt-section-head">
        <span className="pdt-heading">主演</span>
      </div>
      <Rail boxClass="pdt-cast-scroller" ariaLabel="主演" deps={[list.length]}>
        {(ref) => (
          <div className="pdt-cast-row" ref={ref}>
            {list.map((c, i) => {
              const raw = String(c.profile ?? "").trim();
              // profile 有两种形态：TMDB 相对路径（/xx.jpg）与完整代理 URL（见 DetailView 的 castResolved）
              const src = raw ? (raw.startsWith("http") ? raw : tmdbImg(raw, "w185")) : "";
              const char = String(c.character ?? c.roles?.[0]?.character ?? "").trim();
              const clickable = !!onOpenPerson && Number(c.id ?? 0) > 0;
              return (
                <button
                  type="button"
                  className={`pdt-c-unit${clickable ? " clickable" : ""}`}
                  key={`${c.id ?? i}-${c.name ?? ""}`}
                  title={char ? `${c.name} 饰 ${char}` : String(c.name ?? "")}
                  disabled={!clickable}
                  onClick={() => clickable && onOpenPerson?.(c)}
                >
                  {src ? (
                    <img className="pdt-c-img" src={src} loading="lazy" alt="" draggable={false} />
                  ) : (
                    <span className="pdt-c-img pdt-c-empty" aria-hidden="true">
                      {String(c.name ?? "").slice(0, 1)}
                    </span>
                  )}
                  <span className="pdt-c-name">{String(c.name ?? "")}</span>
                  {char && <span className="pdt-c-char">{char}</span>}
                </button>
              );
            })}
          </div>
        )}
      </Rail>
    </section>
  );
}

/** 相关推荐横排：16:9 卡片，hover 出标题浮层。 */
export function PolarisRecommendations({
  items,
  onOpen,
}: {
  items: TmdbRecommendation[];
  onOpen?: (r: TmdbRecommendation) => void;
}) {
  const list = items.filter((r) => r.tmdbId).slice(0, 20);
  if (list.length === 0) return null;
  return (
    <section className="pdt-recs">
      <div className="pdt-section-head">
        <span className="pdt-heading">相关推荐</span>
      </div>
      <Rail boxClass="pdt-recs-scroller" ariaLabel="相关推荐" deps={[list.length]}>
        {(ref) => (
          <div className="pdt-recs-scroller" ref={ref}>
            {list.map((r) => {
              const src = r.backdrop
                ? tmdbImg(r.backdrop, "w780")
                : r.poster
                  ? imgUrl(r.poster)
                  : "";
              return (
                <button
                  type="button"
                  className="pdt-rec-card"
                  key={r.tmdbId}
                  title={r.title}
                  disabled={!onOpen}
                  onClick={() => onOpen?.(r)}
                >
                  <span className="pdt-rec-inner">
                    {src ? (
                      <img className="pdt-rec-img" src={src} loading="lazy" alt="" draggable={false} />
                    ) : (
                      <span className="pdt-rec-img pdt-rec-empty" aria-hidden="true" />
                    )}
                    <span className="pdt-rec-overlay">
                      <span className="pdt-rec-title">{r.title}</span>
                      {r.year && <span className="pdt-rec-sub">{r.year}</span>}
                    </span>
                  </span>
                </button>
              );
            })}
          </div>
        )}
      </Rail>
    </section>
  );
}
