/* [lc-2396] Polaris 详情页主容器（+ 底部玻璃控制台）
 *
 * 逐条对照 `Polaris Player 5.2.8 / web/index.html:1570-1920` 与 `web/styles/main.css`：
 *
 *  覆盖层（5046）
 *   · `.pw-detail-overlay`  position absolute inset 0、background #000、overflow hidden
 *   · `.active`             pwPageEnter 1.31s cb(.32,.72,0,1)（只淡入）
 *  背景（5076）
 *   · `.pw-detail-backdrop-fixed` inset 0、z-index 0、pointer-events none、contain layout paint style
 *   · `.pw-detail-backdrop-img`   object-fit cover、初始 scale(1.1) + opacity 0；
 *                                 `.is-loaded` → opacity .82 + saturate(1.08) contrast(1.03)
 *                                 + ken-burns 40s ease-in-out infinite alternate
 *   · ken-burns 关键帧           scale 1.10→1.12、translate 0→(-1.5%, -1%)
 *   · 滚动时暂停                 `.is-detail-scrolling .pw-detail-backdrop-img { animation-play-state: paused }`
 *   · `.pw-detail-backdrop-gradient` **十段**渐变（40% 前全透明，50%→100% 八档到 #0a0a0e 实色）—— 5110
 *  返回（5130）
 *   · `.pw-detail-close`  44~54 × 40、radius 999、bg rgba(0,0,0,.16) + blur(20px) saturate(110%)；
 *                         hover 整体反白、图标 svg 20px stroke 4、translateX(-3.5%) 光学居中
 *  滚动容器（5187）
 *   · `.pw-detail-scroll-container` inset 0、overflow-y auto、scroll-behavior smooth、
 *                                   padding-bottom 100px、滚动条 display:none
 *  控制台（6395）
 *   · `.pw-detail-console-section`  padding 40px 80px
 *   · `::before`  底色 rgba(10,10,14,.55) + feTurbulence 噪点(alpha .015, 128×128) +
 *                 blur(24px) saturate(180%) brightness(.85) + **19 段** mask 渐入
 *   · `stair1` 100px blur(2px)  / `stair2` 140px blur(5px) / `::after` 180px blur(10px)
 *                 —— 三级渐进模糊（不做一次大模糊）
 *   · `.pw-detail-console-inner` 入场 pwDetailFadeUp 1.2s cb(.19,1,.22,1) **延迟 .6s**
 *
 * 与 AniBox 外壳的适配（本版必须处理，Polaris 没有这些前提）：
 *   AniBox 有 40px 悬浮标题栏 + 侧栏浮层，所以 overlay 用 absolute 铺满 .view-pane，
 *   返回键 top 让开标题栏（54px）；其余数值一律照抄。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { PolarisHero } from "./PolarisHero";
import { PolarisEpisodes } from "./PolarisEpisodes";
import { PolarisVersions } from "./PolarisVersions";
import { PolarisSpecs } from "./PolarisSpecs";
import { PolarisCast, PolarisRecommendations, PolarisStills } from "./PolarisRails";
import type { PolarisModel } from "./model";

export function PolarisDetail({
  m,
  onZoom,
}: {
  m: PolarisModel;
  onZoom?: (path: string) => void;
}) {
  /** 背景图淡入：图 onLoad 后才加 .is-loaded（Polaris 同款，避免半透明底图闪一下）。
   *
   *  ⚠ 不能用 `useEffect(() => setBgLoaded(false), [src])` 来重置 —— 那是个真 bug：
   *  本项目图片走 `fntvimg` 自定义协议且**拉了即落盘**，命中磁盘缓存时图片解码极快，
   *  `onLoad` 可能在 effect 执行**之前**就触发；随后 effect 把标记重置成 false，
   *  而 `onLoad` 不会再触发第二次 → 图片永久停在 `opacity: 0`（表现为「详情页封面没了」，
   *  只剩黑底 + 文字）。
   *
   *  改法：状态里记住「哪个 src 已经加载完成」。src 变化时状态自然不再匹配 → 自动回到
   *  未加载态（效果与重置相同，但没有竞态）；并且用 ref 回调在**挂载那一刻**补一次
   *  `img.complete` 检查，兜住「节点复用 + 图片已在缓存里」的情况。 */
  const [loadedSrc, setLoadedSrc] = useState("");
  const bgLoaded = !!m.backdrop && loadedSrc === m.backdrop;
  /** 挂载/换 src 时立刻检查一次是否已经加载完成（缓存命中时 onLoad 可能早于 effect）。 */
  const bgRefCb = useCallback(
    (el: HTMLImageElement | null) => {
      if (!el) return;
      if (el.complete && el.naturalWidth > 0 && m.backdrop) setLoadedSrc(m.backdrop);
    },
    [m.backdrop],
  );

  /** 滚动时暂停 Ken Burns（Polaris `.is-detail-scrolling`）：省 GPU，也避免 transform 与滚动合成打架 */
  const [scrolling, setScrolling] = useState(false);
  const scrollTimer = useRef<number | null>(null);
  const onScroll = () => {
    setScrolling(true);
    if (scrollTimer.current) window.clearTimeout(scrollTimer.current);
    scrollTimer.current = window.setTimeout(() => setScrolling(false), 160);
  };
  useEffect(
    () => () => {
      if (scrollTimer.current) window.clearTimeout(scrollTimer.current);
    },
    [],
  );

  /** 首屏让位常量：Polaris 是裁 180px（min-height: calc(100vh - 180px)），
   *  它没有顶栏。我们顶部有 40px 悬浮标题栏，所以换成 var(--pdt-hero-offset) 由 CSS 叠加。 */
  const selectedVersion =
    m.versions.find((v) => v.media_guid === m.selectedMediaGuid) ?? m.versions[0] ?? null;

  return (
    <div
      className={`pdt${scrolling ? " is-scrolling" : ""}`}
      /* [lc-2396] 诊断用：背景大图有三种状态，截图/日志一眼能分清是哪一种，
         不必再猜「到底是没数据还是没加载」：
           no-src = backdrop 为空串（TMDB 与飞牛都没有横图）
           loading = 有 src 但还没 onLoad
           loaded  = 已加 .is-loaded（opacity .82 可见） */
      data-bg={!m.backdrop ? "no-src" : bgLoaded ? "loaded" : "loading"}
    >
      {/* ── 背景层：固定不滚（inset 0 + position absolute），Ken Burns 呼吸 —— */}
      <div className="pdt-backdrop" aria-hidden="true">
        {m.backdrop && (
          <img
            className={`pdt-backdrop-img${bgLoaded ? " is-loaded" : ""}`}
            ref={bgRefCb}
            src={m.backdrop}
            alt=""
            decoding="async"
            onLoad={() => setLoadedSrc(m.backdrop)}
            onError={() => setLoadedSrc("")}
            draggable={false}
          />
        )}
        <div className="pdt-backdrop-gradient" />
      </div>

      {/* ── 返回胶囊：Polaris 的 .pw-detail-close（‹ + hover 反白） ── */}
      <button className="pdt-close" onClick={m.onBack} title="返回上一页" aria-label="返回上一页">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <polyline points="15 6 9 12 15 18" fill="none" stroke="currentColor" strokeWidth="4" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>

      {/* ── 滚动容器：隐藏滚动条、底部 100px 气口 ── */}
      <div className="pdt-scroll" onScroll={onScroll}>
        <PolarisHero m={m} />

        {/* ── 玻璃控制台：三级渐进模糊 + 噪点 + 19 段遮罩（全部在 CSS 里照抄） ── */}
        <section className="pdt-console">
          <div className="pdt-glass-stair1" aria-hidden="true" />
          <div className="pdt-glass-stair2" aria-hidden="true" />

          <div className="pdt-console-inner">
            {/* 版本条（原 Polaris 的「版本选择」+ 资源卡横排；这里放 video_streams） */}
            <PolarisVersions
              versions={m.versions}
              selected={m.selectedMediaGuid}
              onSelect={(mg) => m.onVersionChange(mg)}
              filePath={m.filePath}
            />

            {/* 剧集横排（剧集限定） */}
            <PolarisEpisodes m={m} />

            {/* [lc-2405] 文件位置条（取代原 4 列规格网格 + 展开区；用户要求只留长条，
                内容改为文件位置） */}
            <PolarisSpecs version={selectedVersion} filePath={m.filePath} />

            {/* 剧照横排 */}
            <PolarisStills stills={m.stills} onZoom={onZoom} />

            {/* 主演横排 */}
            <PolarisCast
              cast={m.cast}
              onOpenPerson={
                m.onOpenPerson
                  ? (c) =>
                      m.onOpenPerson?.({
                        source: "tmdb",
                        id: Number(c.id ?? 0),
                        name: String(c.name ?? ""),
                        profile: c.profile ? String(c.profile) : undefined,
                      })
                  : undefined
              }
            />

            {/* 详情标签页（明细/评论/吐槽/关联/角色/更多/讨论）—— 直接复用 DetailView 的面板 */}
            <section className="pdt-tabs">
              {m.tabsNav}
              {m.tabsBody}
            </section>

            {/* 相关推荐（Polaris 的 .pw-detail-more-content 区 → 这里用 TMDB 推荐） */}
            <PolarisRecommendations
              items={m.recommendations}
              onOpen={
                m.onOpenTmdb
                  ? (r) => m.onOpenTmdb?.(r.tmdbId, r.title, r.year, m.isSeries ? "tv" : "movie")
                  : undefined
              }
            />
          </div>
        </section>
      </div>
    </div>
  );
}
