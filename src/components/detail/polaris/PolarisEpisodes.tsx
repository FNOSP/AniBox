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
import { useCallback, useEffect, useRef, useState } from "react";
import { Check as IcoCheckL } from "lucide-react";
import { useRailArrows } from "./RailArrows";
import { epDurationBadge, epLabel, progressPct } from "./shared";
import type { PolarisModel } from "./model";

/* ══════════════════ [lc-2401] 选集展示形态：封面卡片 / 纯数字网格 ══════════════════
   · cards = Polaris 原样（横排大卡：封面 + 图上日期/时长 + 图外标题/简介），默认
   · grid  = 纯数字方块网格（一屏看全一季几十集，不用横着翻）

   ⚠ 持久化键**刻意与原有版式的 `fntv-ep-view-mode` 分开**（那边是 light/极简方块）。
     共用一个键的话，在新版式里点一下切换会顺带改掉老版式的选集形态 ——
     用户明确要求「原来的不变」，所以两套记忆各存各的。 */
type PdtEpView = "cards" | "grid";
const PDT_EP_VIEW_KEY = "fntv-pdt-ep-view";

function readPdtEpView(): PdtEpView {
  try {
    return localStorage.getItem(PDT_EP_VIEW_KEY) === "grid" ? "grid" : "cards";
  } catch {
    return "cards";
  }
}

export function PolarisEpisodes({ m }: { m: PolarisModel }) {
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const [view, setView] = useState<PdtEpView>(readPdtEpView);
  /* 切换时视图容器会被换掉（横排 scroller ↔ 网格），拖到 deps 里让 RailArrows
     重新挂监听 —— 否则它听的是已经 unmount 的旧节点，atStart/atEnd 永远是旧值。 */
  const { atStart, atEnd, scrollBy } = useRailArrows(scrollerRef, [
    m.episodes.length,
    m.seasonIdx,
    view,
  ]);
  const toggleView = useCallback(() => {
    setView((v) => {
      const next: PdtEpView = v === "grid" ? "cards" : "grid";
      try {
        localStorage.setItem(PDT_EP_VIEW_KEY, next);
      } catch {
        /* localStorage 不可用：仅本次会话生效 */
      }
      return next;
    });
  }, []);

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
          {/* [lc-2399] 季/集数摘要（「第 1 季 · 共 8 集 · 无缺失」）从右端挪到「选集」右边
              （用户要求）。它描述的是**当前这一季**的内容，紧贴标题读起来才成立；
              原来甩到整行最右端，隔着几百像素和标题分家，扫视时对不上。 */}
          {m.seasonEpInfo && (
            <span className="pdt-episodes-meta" title={m.seasonEpInfo.title || undefined}>
              {m.seasonEpInfo.text}
            </span>
          )}
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

        {/* [lc-2400] 翻页圆钮：从「雪佛兰箭头挂轨道两侧」改成**右上角一对圆形按钮**
            （用户指定：原来那种去掉，改成图片样式，位置放到头部右边 —— 也就是上一版
            季/集摘要腾出来的那个位置）。
            可用态仍由 useRailArrows 判定：到头置灰且不可点，不是隐藏 ——
            隐藏会让整行在滚动时左右跳动。
            [lc-2401] 圆钮**左边**再插一个切换键（用户给的那个上下双箭头图标）：
            在「封面卡片 / 纯数字网格」之间切换下面的剧集区。放同一组里是为了让它
            和翻页键共享同一套视觉语言（36px 同尺寸、同底色、同投影），
            读起来是「一组控件」而不是三个孤立按钮。
            网格态下方两个翻页键自然失效（没有横向滚动），置灰即可。 */}
        <div className="pdt-ep-nav">
          <button
            type="button"
            className="pdt-ep-nav-btn pdt-ep-view-btn"
            onClick={toggleView}
            aria-pressed={view === "grid"}
            aria-label={view === "grid" ? "切换为封面卡片" : "切换为数字网格"}
            title={view === "grid" ? "切换为封面卡片" : "切换为数字网格"}
          >
            {/* 用户给的图标：上箭头朝右、下箭头朝左的双向切换符号（竖排堆叠） */}
            <svg viewBox="0 0 1024 1024" aria-hidden="true">
              <path
                d="M921.207 328.929l0.128 0.146-176.96 159.263-0.127-0.146c-9.41 8.473-21.732 13.772-35.387 13.772-29.32 0-53.088-23.767-53.088-53.087 0-15.666 6.913-29.6 17.702-39.317l-0.128-0.143 74.13-66.718h-609.3c-29.32 0-53.087-23.772-53.087-53.089 0-29.32 23.768-53.089 53.088-53.089h138.348l-0.002 0.006H266.46v-0.006h230.049v0.006h250.969l-74.13-66.72 0.129-0.144c-10.791-9.717-17.702-23.651-17.702-39.315 0-29.317 23.768-53.089 53.087-53.089 13.655 0 25.978 5.302 35.387 13.772l0.127-0.146L921.338 250.15l-0.129 0.143c10.79 9.722 17.705 23.654 17.705 39.32-0.003 15.663-6.918 29.595-17.707 39.316zM85.089 732.47c0-15.666 6.915-29.598 17.704-39.32l-0.128-0.143 176.961-159.265 0.127 0.146c9.41-8.47 21.732-13.772 35.387-13.772 29.32 0 53.088 23.772 53.088 53.09 0 15.663-6.912 29.597-17.702 39.314l0.128 0.143-74.129 66.72h250.969v-0.005h230.049v0.005h-10.064l-0.003-0.005h138.35c29.32 0 53.088 23.77 53.088 53.09 0 29.316-23.768 53.088-53.088 53.088H276.522l74.13 66.718-0.129 0.143c10.791 9.717 17.702 23.651 17.702 39.317 0 29.32-23.768 53.087-53.087 53.087-13.655 0-25.978-5.3-35.387-13.772l-0.127 0.146-176.96-159.262 0.128-0.146c-10.788-9.722-17.703-23.654-17.703-39.317z"
                fill="currentColor"
              />
            </svg>
          </button>
          <button
            type="button"
            className="pdt-ep-nav-btn"
            onClick={() => scrollBy(-1)}
            disabled={atStart || view === "grid"}
            aria-label="向左浏览剧集"
            title="上一页"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <polyline points="15 6 9 12 15 18" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </button>
          <button
            type="button"
            className="pdt-ep-nav-btn"
            onClick={() => scrollBy(1)}
            disabled={atEnd || view === "grid"}
            aria-label="向右浏览剧集"
            title="下一页"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <polyline points="9 6 15 12 9 18" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </button>
        </div>
      </div>

      {view === "grid" ? (
        /* ── [lc-2401] 纯数字网格：一屏看全一季 ──
           交互与卡片态完全一致（点播放、右上勾标记观看、⋯ 单集元数据），
           只把"封面 + 标题 + 简介"这一层信息换成集号本身。
           视觉参数取原版式 .dv2e-grid 的同一档（1:1 方块 / auto-fill 64px / gap 8px），
           但配色改用影院版的深色口径（白 8% 底 + 白描边），不引入浅色主题变量 ——
           详情页恒为深色底，套 var(--bg-3) 会在深色下变浅、在白底上反白。 */
        <div className="pdt-ep-grid-wrap">
          <div className="pdt-ep-grid" role="list" aria-label="选集（数字）">
            {m.epsLoading && m.episodes.length === 0
              ? Array.from({ length: 12 }, (_, i) => (
                  <div className="pdt-ep-cell pdt-ep-cell-sk" key={`sk${i}`} aria-hidden="true" />
                ))
              : m.episodes.map((ep, i) => {
                  const num = Number(ep.episode_number ?? i + 1);
                  const pct = progressPct(ep.ts, ep.duration);
                  const seen = m.isEpSeen(ep);
                  const playing = m.playingGuid === ep.guid;
                  return (
                    <div
                      key={ep.guid}
                      role="listitem"
                      tabIndex={0}
                      data-ep-guid={ep.guid}
                      data-ep-num={String(num)}
                      className={`pdt-ep-cell${playing ? " playing" : ""}${seen ? " seen" : ""}`}
                      title={`播放 ${epLabel(num)}${ep.title ? `：${ep.title}` : ""}`}
                      onClick={() => m.onPlayEpisode(ep.guid)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          m.onPlayEpisode(ep.guid);
                        }
                      }}
                    >
                      <span className="pdt-ep-cell-no">{num}</span>
                      {/* 已看开关：与卡片态同一个语义（右上角圆钮），尺寸缩到 18px 适配方块 */}
                      <button
                        type="button"
                        className={`pdt-ep-cell-mark${seen ? " on" : ""}`}
                        title={seen ? "已看（点击取消标记）" : "标记为已看"}
                        aria-pressed={seen}
                        onClick={(e) => {
                          e.stopPropagation();
                          m.onToggleEpWatched(ep, seen);
                        }}
                      >
                        <IcoCheckL size={10} strokeWidth={3} />
                      </button>
                      {pct > 0 && (
                        <span className="pdt-ep-cell-prog" aria-hidden="true">
                          <i style={{ width: `${pct}%` }} />
                        </span>
                      )}
                      {m.onEpMeta && (
                        <button
                          type="button"
                          className="pdt-ep-cell-meta"
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
                  );
                })}
          </div>
        </div>
      ) : (
        <div className="pdt-episodes-rail">
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
                  /* [lc-2397] 图外标题两段（对照截图「S1E1 - 自动贩卖机，移动吧」的形态）：
                       epNoLabel = S1E1（季号取本集自带，缺省用当前季）
                       epTitle   = 集名；没有集名时退回「第 N 集」，不留空行 */
                  const seasonNo =
                    Number(ep.season_number ?? m.seasons[m.seasonIdx]?.season_number ?? m.seasonIdx + 1) || 1;
                  const epNoLabel = `S${seasonNo}E${num}`;
                  const epTitle =
                    String(ep.title ?? "").trim() || `第 ${num} 集`;
                  /* 图上角标：首播日期（左上）与时长（左下），与截图一致；取不到就不渲染 */
                  const airDate = String(ep.air_date ?? "").trim().slice(0, 10);
                  const durText = epDurationBadge(ep.duration);
                  const desc = m.epDesc(ep, num);
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
                        {/* [lc-2397] 图上角标：左上首播日期、左下时长（与截图的浮字位置一致），
                            集号与集名挪到图外下方（图内只保留这两条不会挤占画面的小字）。 */}
                        {airDate && <span className="pdt-ep-date">{airDate}</span>}
                        {durText && <span className="pdt-ep-dur">{durText}</span>}
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
                        {/* [lc-2398] 「已看」标记 —— **唯一入口**（用户要求：两个对勾只留一个）。
                            改前有两套：这里有只读的状态指示圆钮、左下角另有一个可点的开关，
                            两者视觉重复，且左下角那个还压住了「24m」时长角标（截图实锤）。
                            现在合成一个：右上角圆钮直接就是开关（写飞牛 watched），
                            已看 = 亮起实心；未看 = 暗态描边，悬停才明显。
                            stopPropagation 防止点标记时误触播放。 */}
                        <button
                          type="button"
                          className={`pdt-ep-check${seen ? " on" : ""}`}
                          title={seen ? "已看（点击取消标记）" : "标记为已看"}
                          aria-pressed={seen}
                          onClick={(e) => {
                            e.stopPropagation();
                            m.onToggleEpWatched(ep, seen);
                          }}
                        >
                          <IcoCheckL size={14} strokeWidth={3.2} />
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

                      {/* ── 图外下方：集号 + 集名 / 简介（[lc-2397] 用户指定版式，
                             对照截图：封面在上，下面为标题和简介） ── */}
                      <div className="pdt-ep-body">
                        <div className="pdt-ep-line">
                          <span className="pdt-ep-no">{epNoLabel}</span>
                          <span className="pdt-ep-title">{epTitle}</span>
                        </div>
                        {desc && <p className="pdt-ep-desc">{desc}</p>}
                      </div>
                    </div>
                  );
                })}
          </div>
        </div>
      )}

      {/* 空态：与 Polaris `.pw-episodes-empty` 同文案口径（padding 40px 20px / 40% 白） */}
      {!m.epsLoading && m.episodes.length === 0 && (
        <div className="pdt-episodes-empty">
          {m.noFnOS ? "未查询到资源" : "本季暂无剧集"}
        </div>
      )}
    </section>
  );
}
