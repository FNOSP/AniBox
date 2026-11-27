/* [lc-2396] Polaris 详情页 · 剧集横排（episodes section）
 *
 * 逐条对照 `Polaris main.css`：
 *   · `.pw-episodes-section`        margin 20px 0 —— 7124
 *   · `.pw-episodes-section-header` flex 两端、margin-bottom 16px、wrap、gap 12px —— 7124
 *   · `.pw-detail-heading-title`    17px / 500 / 1.5px 字距 / rgba(235,235,245,.6) —— 7432
 *   · `.pw-episodes-rail-container` position:relative + display:flow-root（包住浮动箭头）—— 7146
 *   · `.pw-episodes-arrow`          44×64、left/right −62px、hover 才 opacity 1、38px svg 双层 drop-shadow —— 7150
 *   · `.pw-episodes-scroller`       flex、gap clamp(18,1.2vw,24)、padding 20px 40px 48px、margin 0 -40px -28px —— 8052
 *   · `.pw-episode-card`            clamp(260px,19.8vw,380px)、radius clamp(16px,1.23vw,24px)、--pw-elev —— 8124
 *   · `.pw-episode-thumb-wrap`      aspect-ratio 16/9.2、hover 只缩图 scale(1.07) —— 8140
 *   · `.pw-episode-progress`        5px、白 85%、外发光 + 内超高光、右下圆角 —— 8169
 *   · `.pw-episode-card.watching`   2px 白描边 + 16px 泛光（不是改背景）—— 8185
 *   · `.pw-episode-played-check`    四层 box-shadow 玻璃球 + cb(.34,1.56,.64,1) 回弹 —— 8298
 *   · 集号/集名文案                EPISODE N / 单行省略（strings.json idx 1a87 / 86b）
 */
import { useEffect, useRef } from "react";
import { Check as IcoCheckL } from "lucide-react";
import { useRailArrows } from "./RailArrows";
import { epLabel, progressPct } from "./shared";
import type { PolarisModel } from "./model";

export function PolarisEpisodes({ m }: { m: PolarisModel }) {
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const { atStart, atEnd, scrollBy } = useRailArrows(scrollerRef, [
    m.episodes.length,
    m.seasonIdx,
  ]);

  /* 打开时把「续播目标集」滚到可视区中部（与经典版 .dv2e-list 的自动定位同目的，
     只是这边是横排：改 scrollLeft）。用户手动滚动过就不再接管。 */
  const touchedRef = useRef(false);
  useEffect(() => {
    touchedRef.current = false;
  }, [m.seasonIdx]);
  useEffect(() => {
    if (touchedRef.current) return;
    const box = scrollerRef.current;
    const target = m.resumeTarget;
    if (!box || !target) return;
    const el = box.querySelector<HTMLElement>(`[data-ep-guid="${CSS.escape(target.guid)}"]`);
    if (!el) return;
    const rel = el.getBoundingClientRect().left - box.getBoundingClientRect().left;
    const next = box.scrollLeft + rel - (box.clientWidth - el.offsetWidth) / 2;
    box.scrollTo({ left: Math.max(0, next), behavior: "auto" });
  }, [m.resumeTarget, m.episodes.length, m.seasonIdx]);

  if (!m.isSeries) return null;

  const heading = m.noFnOS ? "剧集" : "选集";

  return (
    <section className="pdt-episodes" aria-label={heading}>
      <div className="pdt-episodes-head">
        <div className="pdt-episodes-left">
          <span className="pdt-heading">{heading}</span>
          {/* 季选择胶囊：只有多季才出现（Polaris 是服务器下拉，我们这里换季直接铺开更少一次点击） */}
          {m.seasons.length > 1 && (
            <div className="pdt-season-pills" role="group" aria-label="选择季">
              {m.seasons.map((sn, i) => {
                const no = Number(sn.season_number ?? i + 1);
                const label = no === 0 ? "特别篇" : `第 ${no} 季`;
                return (
                  <button
                    key={sn.guid || `s${i}`}
                    type="button"
                    className={`pdt-season-pill${i === m.seasonIdx ? " active" : ""}`}
                    aria-pressed={i === m.seasonIdx}
                    title={String(sn.title ?? label)}
                    onClick={() => m.onSeasonChange(i)}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          )}
        </div>
        <div className="pdt-episodes-right">
          {m.seasonEpInfo && (
            <span className="pdt-episodes-meta" title={m.seasonEpInfo.title || undefined}>
              {m.seasonEpInfo.text}
            </span>
          )}
        </div>
      </div>

      <div className="pdt-episodes-rail">
          <button
            type="button"
            className={`pdt-rail-arrow left${atStart ? " disabled" : ""}`}
            onClick={() => scrollBy(-1)}
            aria-label="向左浏览剧集"
            tabIndex={atStart ? -1 : 0}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <polyline points="15 6 9 12 15 18" />
            </svg>
          </button>

          <div
            className="pdt-episodes-scroller"
            ref={scrollerRef}
            onWheel={() => {
              touchedRef.current = true;
            }}
            onTouchStart={() => {
              touchedRef.current = true;
            }}
            onPointerDown={() => {
              touchedRef.current = true;
            }}
          >
            {m.epsLoading && m.episodes.length === 0
              ? Array.from({ length: 6 }, (_, i) => (
                  <div className="pdt-ep-card pdt-ep-card-sk" key={`sk${i}`} aria-hidden="true">
                    <div className="pdt-ep-thumb" />
                  </div>
                ))
              : m.episodes.map((ep, i) => {
                  const num = Number(ep.episode_number ?? i + 1);
                  const pct = progressPct(ep.ts, ep.duration);
                  const seen = m.isEpSeen(ep);
                  const playing = m.playingGuid === ep.guid;
                  const resumed =
                    !!m.resumeTarget && m.resumeTarget.guid === ep.guid && pct > 0 && pct < 100;
                  const thumb = m.epThumb(ep, num);
                  return (
                    <div
                      key={ep.guid}
                      data-ep-guid={ep.guid}
                      data-ep-num={String(num)}
                      className={`pdt-ep-card${playing ? " watching" : ""}${seen ? " played" : ""}${resumed ? " watching" : ""}`}
                      role="button"
                      tabIndex={0}
                      title={`播放 ${epLabel(num)}${ep.title ? `：${ep.title}` : ""}`}
                      onClick={() => m.onPlayEpisode(ep.guid)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          m.onPlayEpisode(ep.guid);
                        }
                      }}
                    >
                      <div className="pdt-ep-thumb">
                        {thumb ? (
                          <img className="pdt-ep-img" src={thumb} loading="lazy" alt="" draggable={false} />
                        ) : (
                          <div className="pdt-ep-img pdt-ep-empty" aria-hidden="true" />
                        )}
                        {/* 底部渐变 + 集号/集名（Polaris .pw-episode-overlay） */}
                        <div className="pdt-ep-overlay">
                          <span className="pdt-ep-num">{epLabel(num).toUpperCase()}</span>
                          <span className="pdt-ep-name">
                            {String(ep.title ?? "").trim() || `第 ${num} 集`}
                          </span>
                        </div>
                        {/* 中央播放钮：hover 才显（Polaris .pw-play-indicator） */}
                        <span className="pdt-ep-play" aria-hidden="true">
                          <svg viewBox="0 0 24 24" fill="currentColor">
                            <path d="M9.9 4.5c-1.2-.7-2.7.2-2.7 1.6v11.8c0 1.4 1.5 2.3 2.7 1.6l9.9-5.9c1.2-.7 1.2-2.5 0-3.2z" />
                          </svg>
                        </span>
                        {/* 进度条：5px 白（Polaris .pw-episode-progress） */}
                        {pct > 0 && (
                          <span className="pdt-ep-progress" aria-hidden="true">
                            <i style={{ width: `${pct}%` }} />
                          </span>
                        )}
                        {/* 已看完勾：四层玻璃球，回弹入场（Polaris .pw-episode-played-check） */}
                        <span className="pdt-ep-check" aria-hidden="true">
                          <IcoCheckL size={14} strokeWidth={3.2} />
                        </span>
                        {/* 手动「已看」开关：常显在左下，与勾标分工不同
                            （勾标=状态指示，此键=可点的开关，写飞牛 watched） */}
                        <button
                          type="button"
                          className={`pdt-ep-mark${seen ? " on" : ""}`}
                          title={seen ? "已看（点击取消标记）" : "标记为已看"}
                          aria-pressed={seen}
                          onClick={(e) => {
                            e.stopPropagation();
                            m.onToggleEpWatched(ep, seen);
                          }}
                        >
                          <IcoCheckL size={11} strokeWidth={2.6} />
                        </button>
                        {m.onEpMeta && (
                          <button
                            type="button"
                            className="pdt-ep-meta"
                            title="本集元数据（编辑 / 手动匹配 / 解除匹配 / 刷新）"
                            onClick={(e) => {
                              e.stopPropagation();
                              m.onEpMeta?.(ep, num);
                            }}
                          >
                            ⋯
                          </button>
                        )}
                      </div>
                    </div>
                  );
                })}
          </div>

          <button
            type="button"
            className={`pdt-rail-arrow right${atEnd ? " disabled" : ""}`}
            onClick={() => scrollBy(1)}
            aria-label="向右浏览剧集"
            tabIndex={atEnd ? -1 : 0}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <polyline points="9 6 15 12 9 18" />
            </svg>
          </button>
        </div>

      {/* 空态：与 Polaris `.pw-episodes-empty` 同文案口径（padding 40px 20px / 40% 白） */}
      {!m.epsLoading && m.episodes.length === 0 && (
        <div className="pdt-episodes-empty">
          {m.noFnOS ? "未查询到资源" : "本季暂无剧集"}
        </div>
      )}
    </section>
  );
}
