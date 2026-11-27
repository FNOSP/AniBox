/* [lc-2396] Polaris 详情页 · 首屏（hero section）
 *
 * 逐条对照 `Polaris Player 5.2.8 / web/index.html:1591-1780` 与 `web/styles/main.css`：
 *   · `.pw-detail-hero-section`  min-height: calc(100vh - 180px)、justify-content: flex-end、
 *                               padding: 0 clamp(40px,4.17vw,80px) 18px —— main.css:5201
 *   · `.pw-detail-hero-lockup`   gap 50px、align-items: flex-end、
 *                               入场 pwDetailFadeUp 1.2s cb(.19,1,.22,1) 延迟 .2s —— main.css:5211
 *   · `.pw-detail-poster-container-3d` **display:none**（5.2.8 首屏是纯文字版式）—— main.css:5221
 *   · `.pw-detail-title-container` min-height 100px + padding-top 15px（防 logo 加载后跳动）—— main.css:5565
 *   · `.pw-detail-title`          clamp(43px,3.44vw,66px) / 500 字重 / 1.5px 字距 / 渐变文字 —— main.css:5538 + 33979
 *   · `.pw-detail-meta`           gap 16px / 16px / 600 / 60% 白 —— main.css:5961
 *   · `.pw-detail-overview`       3 行折叠、max-width 800px、line-height 1.6 —— main.css:6034
 *   · `.ios-play-btn.pw-detail-play-solid` 药丸（radius 9999、左 padding 10 < 右 26）—— main.css:6166
 *   · `.pw-detail-btn-icon`       54~71 × 38~48 横向胶囊 + 白玻璃 —— main.css:6309
 */
import { useEffect, useRef, useState } from "react";
import { ChevronDown as IcoChevronDownL } from "lucide-react";
import { PlayGlyph } from "../../common/PlayGlyph";
import { STATUS_CN } from "../../../lib/region";
import { useFavorite } from "../useFavorite";
import { epLabel } from "./shared";
import type { PolarisModel } from "./model";

/** 简介折叠阈值：Polaris 是 `-webkit-line-clamp: 3`，展开按钮只在**真的溢出**时出现。
 *  判断方法照 Polaris 的做法（index.html 的 toggle 默认 display:none，由 JS 量 scrollHeight）：
 *  这里用同一口径 —— 渲染后比较 scrollHeight 与 clientHeight。 */
function useOverflowCheck(text: string) {
  const ref = useRef<HTMLParagraphElement | null>(null);
  const [overflow, setOverflow] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // 折叠态下 scrollHeight 会大于 clientHeight（3 行被 clamp）
    const check = () => setOverflow(el.scrollHeight - el.clientHeight > 2);
    check();
    const ro = new ResizeObserver(check);
    ro.observe(el);
    return () => ro.disconnect();
  }, [text]);
  return { ref, overflow };
}

export function PolarisHero({
  m,
}: {
  m: PolarisModel;
}) {
  const [expanded, setExpanded] = useState(false);
  const { ref: overviewRef, overflow } = useOverflowCheck(m.overview);
  /* 收藏键：与 MovieHeroEditorial 共用同一 hook（同一套 favorite_get / favorite_toggle 与事件广播） */
  const { fav, favGuid, busy: favBusy, toggle: toggleFav } = useFavorite(m.favItem);

  /* 题材串：Polaris 是一个 .pw-detail-meta-item 纯文本。
     分隔符用「/」而不是中点 —— 项目规范禁止用「·」当分隔符（AGENTS.md UI 第 1 条）。 */
  const genresText = m.genres.slice(0, 3).join(" / ");
  /** 播放药丸副标题：剧集显示「第 N 集」，电影显示片长。与 Polaris 的 text-stack 双行结构一致。 */
  const playSub = m.isSeries && m.resumeTarget
    ? `${epLabel(Number(m.resumeTarget.episode_number ?? 1))}${m.resumePct > 0 ? ` · ${m.resumePct}%` : ""}`
    : m.runtimeMin > 0
      ? `${m.runtimeMin} 分钟`
      : "";
  const statusLabel = STATUS_CN[String(m.base.status ?? "")] ?? "";
  const ratingVotes = m.votes > 0 ? `${m.votes} 人评分` : "";

  return (
    <section className="pdt-hero">
      <div className="pdt-hero-lockup">
        {/* Polaris 5.2.8 的 .pw-detail-poster-container-3d 是 display:none —— 首屏纯文字。
            这里保留同名占位元素以对照源码结构，但不渲染任何内容（CSS 也置 none）。 */}
        <div className="pdt-poster-slot" aria-hidden="true" />

        <div className="pdt-hero-info">
          {/* ── 标题区：logo 优先，无 logo 用文字（.has-logo 互斥，同 Polaris main.css:5565） ── */}
          <div className={`pdt-title-wrap${m.logoLight || m.logoDark ? " has-logo" : ""}`}>
            {(m.logoLight || m.logoDark) && (
              <>
                {m.logoLight && <img className="pdt-logo pdt-logo-light" src={m.logoLight} alt="" aria-hidden="true" />}
                {m.logoDark && <img className="pdt-logo pdt-logo-dark" src={m.logoDark} alt="" aria-hidden="true" />}
              </>
            )}
            <h1 className="pdt-title">
              {m.title || "（无标题）"}
              {m.isSeries && m.seasons.length > 1 && (
                <span className="pdt-title-season">{` 第 ${Number(m.seasons[m.seasonIdx]?.season_number ?? m.seasonIdx + 1)} 季`}</span>
              )}
            </h1>
          </div>
          {m.originalTitle && m.originalTitle !== m.title && (
            <div className="pdt-original">{m.originalTitle}</div>
          )}

          {/* ── 元信息行：评分 · 年份 · 时长 · 题材 · 画质标签（Polaris main.css:5961） ── */}
          <div className="pdt-meta">
            {m.rating != null && m.rating > 0 && (
              <span className="pdt-rating" title={ratingVotes}>
                <svg className="pdt-tmdb-logo" viewBox="0 0 24 24" aria-label="TMDB" role="img">
                  <rect x="1" y="6" width="22" height="12" rx="3" fill="currentColor" opacity="0.18" />
                  <text x="12" y="15.4" textAnchor="middle" fontSize="7.4" fontWeight="800" fill="currentColor">
                    TMDB
                  </text>
                </svg>
                <span className="pdt-score">{m.rating.toFixed(1)}</span>
              </span>
            )}
            {m.year && <span className="pdt-meta-item">{m.year}</span>}
            {m.isSeries
              ? m.episodes.length > 0 && (
                  <span className="pdt-meta-item">
                    {m.noFnOS
                      ? `共 ${m.seasonLibraryTotal} 集`
                      : `${m.episodes.length}${m.seasonLibraryTotal > 0 && m.seasonLibraryTotal !== m.episodes.length ? ` / ${m.seasonLibraryTotal}` : ""} 集`}
                  </span>
                )
              : m.runtimeMin > 0 && <span className="pdt-meta-item">{m.runtimeMin} 分钟</span>}
            {genresText && <span className="pdt-meta-item">{genresText}</span>}
            {(statusLabel || m.isSeries) && (
              <span className={`pdt-state${m.isSeries ? (m.seriesFinished ? " done" : " airing") : ""}`}>
                {m.isSeries ? (m.seriesFinished ? "已完结" : "连载中") : statusLabel}
              </span>
            )}
            {m.isSeries && m.lastAir && (
              <span className="pdt-meta-item">更新至 {String(m.lastAir).slice(0, 10)}</span>
            )}
          </div>

          {/* ── 简介：3 行折叠 + 展开圆钮（Polaris main.css:6034 / 6057） ── */}
          {m.overview && (
            <div className="pdt-overview-wrap">
              <p ref={overviewRef} className={`pdt-overview${expanded ? " expanded" : ""}`}>
                {m.overview}
              </p>
              {overflow && (
                <button
                  className={`pdt-overview-toggle${expanded ? " expanded" : ""}`}
                  onClick={() => setExpanded((v) => !v)}
                  title={expanded ? "收起简介" : "展开简介"}
                  aria-label={expanded ? "收起简介" : "展开简介"}
                  aria-expanded={expanded}
                >
                  <IcoChevronDownL size={12} strokeWidth={3} />
                </button>
              )}
            </div>
          )}

          {/* ── 动作区：播放药丸 + 一排玻璃胶囊图标键（Polaris main.css:6096 / 6166 / 6309） ── */}
          <div className="pdt-actions">
            <button
              className="pdt-play"
              onClick={m.onPlay}
              disabled={m.playing}
              aria-label={m.isSeries && m.resumePct > 0 ? "继续播放" : "播放"}
            >
              <span className="pdt-play-circle">
                <PlayGlyph size={17} />
              </span>
              <span className="pdt-play-stack">
                <span className="pdt-play-title">
                  {m.playing ? "打开中…" : m.resumePct > 0 ? "继续播放" : "播放"}
                </span>
                {playSub && <span className="pdt-play-sub">{playSub}</span>}
              </span>
              {/* 3px 进度条贴药丸底边（Polaris main.css:6135 的 progress-track） */}
              {m.resumePct > 0 && (
                <span className="pdt-play-progress" aria-hidden="true">
                  <i style={{ width: `${m.resumePct}%` }} />
                </span>
              )}
            </button>

            {favGuid && (
              <button
                className={`pdt-icon-btn${fav ? " is-fav" : ""}`}
                onClick={toggleFav}
                disabled={favBusy}
                title={fav ? "取消收藏" : "收藏"}
                aria-pressed={fav}
              >
                <svg viewBox="0 0 24 24" width="18" height="18" fill={fav ? "currentColor" : "none"} stroke="currentColor" strokeWidth="3.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M19.51 5.72a4.68 4.68 0 0 0-6.61 0L12 6.62l-.9-.9a4.68 4.68 0 0 0-6.61 6.61l.9.9L12 19.85l6.61-6.61.9-.9a4.68 4.68 0 0 0 0-6.61z" />
                </svg>
              </button>
            )}

            {m.onMeta && (
              <button className="pdt-icon-btn" onClick={m.onMeta} title="元数据操作（编辑 / 手动匹配 / 刷新）">
                <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor" aria-hidden="true">
                  <circle cx="5" cy="12" r="1.9" />
                  <circle cx="12" cy="12" r="1.9" />
                  <circle cx="19" cy="12" r="1.9" />
                </svg>
              </button>
            )}

            {m.onSubtitle && (
              <button className="pdt-icon-btn" onClick={m.onSubtitle} title="下载字幕（迅雷 / assrt / OpenSubtitles）">
                <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <rect x="3" y="5" width="18" height="14" rx="2.6" />
                  <path d="M7.4 15h3.4M13.6 15h3" />
                </svg>
              </button>
            )}

            {m.onOpenTmdb && Number(m.base.tmdb_id ?? 0) > 0 && (
              <button
                className="pdt-icon-btn"
                onClick={() =>
                  m.onOpenTmdb?.(
                    Number(m.base.tmdb_id),
                    m.title,
                    m.year,
                    m.isSeries ? "tv" : "movie",
                  )
                }
                title="在 TMDB 里查看"
              >
                <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M14 4h6v6" />
                  <path d="M20 4 11 13" />
                  <path d="M18 14v4.4A1.6 1.6 0 0 1 16.4 20H5.6A1.6 1.6 0 0 1 4 18.4V7.6A1.6 1.6 0 0 1 5.6 6H10" />
                </svg>
              </button>
            )}
          </div>
        </div>
      </div>
    </section>
  );
}
