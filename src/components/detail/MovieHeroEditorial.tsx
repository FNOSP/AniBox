/* [lc-2247] 电影详情页「杂志风」版式（editorial）。
 *
 * 与经典版（MovieHero.tsx）的分工：
 *   · 经典版 = 满幅头图 + 自适应反色，视觉重心是背景图；
 *   · 本版 = 左文右图双栏，视觉重心是大标题 + 层叠海报，没有满幅头图。
 * 两者由设置项 movieDetailStyle 切换（默认 classic），**目前只对电影生效**。
 *
 * 数据来源与经典版完全一致（都来自 DetailView 的同一批 props），
 * 因此切换版式不会丢数据、也不额外发请求。
 *
 * 布局（与用户的静态稿一致）：
 *   左栏  ① 艺术字大标题（kicker / 标题 / 原名）
 *         ② 数据格（评分·片长·年份·地区）+ 补充小字 + 题材标签
 *         ③ 简介（吃掉剩余高度，超长内部滚动）
 *   右栏  层叠海报轮播 + 播放/收藏/元数据 + 版本条 + 主创 + 配音
 */
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Star as IcoStarL } from "lucide-react";
import { PlayGlyph } from "../common/PlayGlyph";
import { imgUrl, tmdbImg } from "../../lib/images";
import { prefersReducedMotion } from "../../lib/motion";
// [lc-2396] 国家/语言/状态 → 中文映射提到 lib/region.ts（Polaris 详情页共用同一份，
//   两处各存一份必然漂移）。映射表内容与搬移前完全一致。
import { STATUS_CN, regionText } from "../../lib/region";
import { useFavorite } from "./useFavorite";
import { TmdbCreditPerson } from "../../types";

/** 数据格里的一项：大数字 + 小标签。 */
type StatItem = { value: string; label: string; unit?: string; rating?: boolean };

type Props = {
  title: string;
  originalTitle: string;
  /** 头图（已是完整 URL；空则退回海报） */
  backdrop: string;
  poster: string;
  rating: number | null;
  votes: number;
  year: string;
  runtimeMin: number;
  genres: string[];
  overview: string;
  cast: TmdbCreditPerson[];
  /** 4K 剧照 file_path 列表（与经典版共用，用作层叠卡片） */
  stills: string[];
  /** TMDB 详情（取 images.logos 用） */
  tmdb: Record<string, unknown> | null;
  /** 上映日期（YYYY-MM-DD，可空） */
  releaseDate?: string;
  resumeAt: boolean;
  playing: boolean;
  onPlay: () => void;
  onBack: () => void;
  onMeta?: () => void;
  onOpenPerson?: (c: TmdbCreditPerson) => void;
  versions?: { mediaGuid: string; label: string; meta?: string }[];
  selectedVersion?: string;
  onVersionChange?: (mediaGuid: string) => void;
  /** [lc-2273] 当前选中版本的文件完整路径（数据区「文件位置」行）；空 = 不渲染该行。 */
  filePath?: string;
  favItem: Record<string, unknown> | null;
  /** [lc-2255] 主创名单（DetailView 的 crew，jobLabel 后端已译成中文）：数据区下方的主创块用。 */
  crew?: TmdbCreditPerson[];
  /** [lc-2258] kicker 第二段（条目类型，如「电影」）：DetailView 从 base.type 动态传入，
   *  不在组件里写死 —— 之前硬编码的「动画长片」只对动画电影成立。 */
  typeLabel?: string;
  /** [lc-2252] 标签条（.dseg.dv2-seg 那条）与标签体（.dbody 面板容器）：
   *  由 DetailView 传入并渲染在左栏「数据区之下、简介位」——
   *  点击标签切换下方内容（经典版同款交互）。不传则退化为纯简介段落。 */
  tabsNav?: ReactNode;
  tabsBody?: ReactNode;
};

/** 从 tmdb 详情里取作品标题 logo（后端 [lc-2247] 已按「中文优先 + 面积最大」挑好）。
 *  返回 TMDB file_path（形如 /abc.png）与一张备选。
 *
 *  为什么要两张：TMDB 的 zh logo 常见是**白色字**（配深色海报用的），
 *  直接放浅色底上等于隐形。所以：
 *    · 浅色主题 → 用 alt（通常是英文深色版）
 *    · 深色主题 → 用 primary（中文白版）
 *  两者都缺时返回空串，区块不渲染。 */
function pickLogos(tmdb: Record<string, unknown> | null): { light: string; dark: string } {
  const p = String(tmdb?.titleLogo ?? "").trim();
  const a = String(tmdb?.titleLogoAlt ?? "").trim();
  const ok = (s: string) => (s.startsWith("/") ? s : "");
  return { light: ok(a) || ok(p), dark: ok(p) || ok(a) };
}

/** 数据格的构造：缺项的条目自动跳过（不占位、不留空 label）。 */
function buildStats(
  rating: number | null,
  votes: number,
  runtimeMin: number,
  year: string,
  regionLabel?: string,
): StatItem[] {
  const out: StatItem[] = [];
  if (rating != null && rating > 0) {
    out.push({ value: rating.toFixed(1), label: votes > 0 ? `${votes} 人评分` : "评分", rating: true });
  }
  if (runtimeMin > 0) out.push({ value: String(runtimeMin), unit: " 分钟", label: "片长" });
  if (year) out.push({ value: year, label: "上映年份" });
  const region = (regionLabel ?? "").split(" · ")[0]?.trim();
  if (region) out.push({ value: region, label: regionLabel ?? "" });
  return out;
}

/** 主创信息块的构造（[lc-2255]）：按后端 jobLabel（已译中文）/ 原始 job 双口径匹配，
 *  每行最多 2 人；四行都没数据时返回空数组（整块不渲染，不留空框）。 */
function buildFacts(crew?: TmdbCreditPerson[]): { label: string; value: string }[] {
  if (!crew?.length) return [];
  const pickNames = (labels: string[]): string =>
    crew
      .filter(
        (c) => labels.includes(String(c.jobLabel ?? "")) || labels.includes(String(c.job ?? "")),
      )
      .map((c) => String(c.name ?? "").trim())
      .filter(Boolean)
      .slice(0, 2)
      .join(" · ");
  const rows: [string, string[]][] = [
    ["导演", ["导演", "Director"]],
    ["编剧", ["编剧", "Screenplay", "Writer"]],
    ["音乐", ["音乐", "Original Music Composer", "Music"]],
    ["角色设计", ["角色设计", "Character Designer"]],
  ];
  const out: { label: string; value: string }[] = [];
  for (const [label, keys] of rows) {
    const v = pickNames(keys);
    if (v) out.push({ label, value: v });
  }
  return out;
}

export function MovieHeroEditorial({
  title,
  originalTitle,
  backdrop,
  poster,
  rating,
  votes,
  year,
  runtimeMin,
  genres,
  overview,
  cast,
  stills,
  tmdb,
  releaseDate,
  resumeAt,
  playing,
  onPlay,
  onBack,
  onMeta,
  onOpenPerson,
  versions,
  selectedVersion,
  onVersionChange,
  filePath,
  favItem,
  crew,
  typeLabel,
  tabsNav,
  tabsBody,
}: Props) {
  /* [lc-2396] 收藏状态/写盘提到 useFavorite（Polaris 详情页共用同一实现）：
     本组件内的调用点与变量名保持不变，行为与搬移前一致。 */
  const { fav, favGuid, busy: favBusy, note, toggle: toggleFav } = useFavorite(favItem);

  /* ── 层叠海报：backdrop / 海报 / 剧照 一起去重后叠放 ──
     经典版用同一批图做背景轮播，这里改成卡片层叠（位置 0/1/2，其余隐藏）。 */
  /* ── 层叠海报：卡片是 **16:9 横版**（[lc-2248] 用户要求），
     所以优先用横图：背板 → 剧照（都是 16:9/2.35:1 原生横图），
     竖版海报（2:3）排最后兜底 —— 它被 cover 裁进 16:9 会损失大半画面，
     只在完全没有横图时才用它，避免空白。 */
  const stillUrls = stills.map((p) => (p.startsWith("http") ? p : tmdbImg(p, "original"))).filter(Boolean);
  const posterUrl = poster ? imgUrl(poster) : "";
  const slides = Array.from(new Set([backdrop, ...stillUrls, posterUrl].filter(Boolean)));
  const slideKey = slides.join("|");
  const [slideIdx, setSlideIdx] = useState(0);
  const [hovered, setHovered] = useState(false);
  const autoRef = useRef<number | null>(null);
  const reduce = prefersReducedMotion();
  /* [lc-2249] 入场动画开关：挂载后下一帧才置真，
     这样 `.is-in` 是从"无 → 有"的**状态变化**，动画必然触发（同一帧加上不会播）。 */
  const [fanIn, setFanIn] = useState(false);
  useEffect(() => {
    if (reduce) return;
    const id = requestAnimationFrame(() => setFanIn(true));
    return () => cancelAnimationFrame(id);
  }, [reduce]);

  useEffect(() => setSlideIdx(0), [slideKey]);

  /* ── [lc-2264] 版本条交互 ──
     · 滚轮横滚：悬停在版本条上时，纵向滚轮直接转为横向滚动。
       只在条真正溢出时接管（preventDefault），没溢出时页面照常滚；
       触控板的横向手势（deltaX 为主）交给浏览器原生处理。
       必须挂原生非 passive 监听 —— React 的 onWheel 是 passive，preventDefault 无效。
     · 活动版本自动滚进可视区：版本多时切换/载入后把选中项滚到条中间
       （只改版本条自身 scrollLeft，不牵动页面）。 */
  const versionsRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const el = versionsRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (el.scrollWidth <= el.clientWidth) return;
      if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;
      e.preventDefault();
      el.scrollLeft += e.deltaY;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);
  useEffect(() => {
    const bar = versionsRef.current;
    const active = bar?.querySelector<HTMLButtonElement>(".mved-version.active");
    if (!bar || !active) return;
    const rel = active.getBoundingClientRect().left - bar.getBoundingClientRect().left;
    const target = bar.scrollLeft + rel - (bar.clientWidth - active.offsetWidth) / 2;
    bar.scrollLeft = Math.max(0, Math.min(target, bar.scrollWidth - bar.clientWidth));
    /* 依赖 versions?.length：列表条数变化（载入/换条目）后重新定位；
       数组每次渲染都是新引用，不能直接当依赖，否则会打断用户手动滚动。 */
  }, [selectedVersion, versions?.length]);

  /* ── [lc-2267/2268] 左右对齐：左栏标签条顶边 = 右栏按钮行顶边 ──
     做法：把左栏标题+数据区（.mved-left-top）拉伸到按钮行顶边的高度——
     按钮行紧跟海报（海报 16:9 随宽度变），所以用 ResizeObserver 盯着
     海报和左栏自身重算；≤1000px 单列布局（海报提到文字上方）时跳过。
     [lc-2268] 内容自然高超过目标（长标题/logo 图撑高）时 minHeight 拉不动，
     标签条会被顶下去——改为压缩 wrapper 内部 gap 吸收差值（下限 4px）；
     gap 只压不回弹，否则 RO 触发链上会振荡。
     [lc-2270] 对齐目标扣除衬线 inset（border-top + padding-top）。
     [lc-2271] 吸收链两级化：wrapper gap（下限4px）→ 衬线间距（下限4px）。 */
  const leftTopRef = useRef<HTMLDivElement | null>(null);
  const stackRef = useRef<HTMLDivElement | null>(null);
  const actionsRef = useRef<HTMLDivElement | null>(null);
  const tabsRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const top = leftTopRef.current;
    const stack = stackRef.current;
    const actions = actionsRef.current;
    const tabs = tabsRef.current;
    if (!top || !stack || !actions) return;
    const left = top.parentElement as HTMLElement;
    const header = left.parentElement as HTMLElement;
    const align = () => {
      if (getComputedStyle(header).gridTemplateColumns.split(" ").length < 2) {
        top.style.minHeight = "";
        top.style.gap = "";
        if (tabs) tabs.style.paddingTop = "";
        return;
      }
      const rowGap = parseFloat(getComputedStyle(left).rowGap) || 0;
      /* [lc-2271] 吸收链两级：wrapper 内 gap（下限 4px）→ 衬线间距 padding-top
         （下限 4px）。tab 卡加高后内容超量更大，一级不够时压二级；
         超量超过两级上限时放弃对齐保排版。每轮先重置再测量，RO 收敛。 */
      if (tabs) tabs.style.paddingTop = "";
      const tabsCs = getComputedStyle(tabs!);
      const padTop = parseFloat(tabsCs.paddingTop) || 0;
      const inset = parseFloat(tabsCs.borderTopWidth) + padTop;
      const base = actions.getBoundingClientRect().top - top.getBoundingClientRect().top - rowGap;
      let target = base - inset;
      top.style.minHeight = "";
      let natural = top.getBoundingClientRect().height;
      let over = natural - target;
      if (over > 0) {
        const innerGap = parseFloat(getComputedStyle(top).rowGap) || 0;
        const x = Math.min(over, innerGap - 4);
        if (x > 0) {
          top.style.gap = `${Math.round(innerGap - x)}px`;
          natural -= x;
          over -= x;
        }
      }
      if (over > 0 && tabs && padTop > 4) {
        const y = Math.min(over, padTop - 4);
        if (y > 0) {
          tabs.style.paddingTop = `${Math.round(padTop - y)}px`;
          target = base - (parseFloat(tabsCs.borderTopWidth) + padTop - y);
        }
      }
      top.style.minHeight = `${Math.max(0, Math.round(target))}px`;
    };
    align();
    const ro = new ResizeObserver(align);
    ro.observe(stack);
    ro.observe(top);
    window.addEventListener("resize", align);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", align);
      top.style.minHeight = "";
      top.style.gap = "";
      if (tabs) tabs.style.paddingTop = "";
    };
  }, []);

  // 自动轮换：4s 一张；悬停暂停；尊重「减弱动态效果」
  useEffect(() => {
    if (slides.length <= 1 || reduce) return;
    if (hovered) return;
    /* fpk style3 AUTO_DELAY = 4500（照抄） */
    const t = window.setInterval(() => setSlideIdx((i) => (i + 1) % slides.length), 4500);
    autoRef.current = t;
    return () => window.clearInterval(t);
  }, [slides.length, slideKey, hovered, reduce]);

  /* [lc-2257] 演员阵容扩到 8 人 —— 单排 4 人填不满右栏（用户反馈）。
     [lc-2280] 改 3 列整排：一行三个（用户），9 人 = 3×3 满排不空位。
     cast 是完整演员表（TMDB credits），多取不亏；没有头像的也在列（占位底色）。 */
  const topCast = cast.filter((c) => c?.name).slice(0, 9);

  /* ── 数据格：评分 / 片长 / 年份 / 地区（缺项自动跳过） ──
     地区与语言从 tmdb 的原始代码取，组件内映射中文（后端 [lc-2247] 新增字段）。 */
  const regionLabel = regionText(
    String(tmdb?.originCountry ?? ""),
    String(tmdb?.originalLanguage ?? ""),
  );
  const stats = buildStats(rating, votes, runtimeMin, year, regionLabel);
  const facts = buildFacts(crew);
  /* [lc-2259] 宣传语（TMDB tagline，中英随条目语言）：空时整行不渲染 */
  const tagline = String(tmdb?.tagline ?? "").trim();
  /* [lc-2257] 状态（Released → 已上映 等）：数据区补充小字用 */
  const statusLabel = STATUS_CN[String(tmdb?.status ?? "")] ?? "";

  // 作品 logo：两张（浅色主题用 light、深色主题用 dark），见 pickLogos 注释
  const logos = pickLogos(tmdb);
  const logoLight = logos.light ? tmdbImg(logos.light, "w500") : "";
  const logoDark = logos.dark ? tmdbImg(logos.dark, "w500") : "";

  return (
    <div className="mved">
      <div className="mved-inner">
        <button className="mved-back" onClick={onBack} title="返回上一页">
          ‹ 返回
        </button>

        <header className="mved-header">
          {/* ══ 左栏：标题 / 数据 / 简介 ══
              [lc-2267] 标题+数据两块包进 .mved-left-top：JS 把它拉伸到与右栏
              海报区同高，标签条顶边随即与右栏按钮行顶边对齐（用户要求）。 */}
          <div className="mved-left">
            <div className="mved-left-top" ref={leftTopRef}>
              <div className="mved-block mved-block-title">
                <h1 className="mved-title">{title || "（无标题）"}</h1>
                {originalTitle && originalTitle !== title && (
                  <div className="mved-subtitle">{originalTitle}</div>
                )}
                {/* [lc-2259] 宣传语（TMDB tagline）：常为空 —— 空时整行不渲染 */}
                {tagline && <div className="mved-tagline">{tagline}</div>}
              </div>

              <div className="mved-block mved-block-data">
                <div className="mved-data-main">
                  {stats.length > 0 && (
                    <div className="mved-stats">
                      {stats.map((s, i) => (
                        <div className="mved-stat" key={`${s.label}-${i}`}>
                          <span className={`mved-stat-v${s.rating ? " is-rating" : ""}`}>
                            {s.rating && <IcoStarL size={14} strokeWidth={0} aria-hidden="true" />}
                            {s.value}
                            {s.unit && <span className="mved-stat-unit">{s.unit}</span>}
                          </span>
                          <span className="mved-stat-k">{s.label}</span>
                        </div>
                      ))}
                    </div>
                  )}

                  {/* [lc-2267] 条目类型（电影/剧集）挪进补充行最前：原 kicker「2026 · 电影」
                      删除——年份与上方数据格重复，类型是唯一有信息量的部分。 */}
                  {(typeLabel || releaseDate || genres.length > 0) && (
                    <div className="mved-meta-row">
                      {typeLabel && <span>{typeLabel}</span>}
                      {releaseDate && <span>{String(releaseDate).slice(0, 10)} 上映</span>}
                      {statusLabel && <span>{statusLabel}</span>}
                      {genres.slice(0, 3).map((g) => (
                        <span key={g}>{g}</span>
                      ))}
                    </div>
                  )}

                  {genres.length > 0 && (
                    <div className="mved-itags" role="list" aria-label="标签">
                      {genres.slice(0, 12).map((g, i) => (
                        <span className="mved-tag" role="listitem" key={`${g}-${i}`}>
                          {g}
                        </span>
                      ))}
                    </div>
                  )}

                  {/* [lc-2273] 文件位置：当前选中版本的完整路径（红框位置展示）。
                      上游没给路径时整行不渲染；[lc-2275] 换行显示最多两行；
                      [lc-2276] 按 `/` 拆成 inline-block 段渲染 —— 段整体不可拆，
                      换行只发生在段间，段内永远完整（零宽空格/keep-all 都压不住
                      「字母→CJK」边界的隐式断点，夹具实测 1080p.|潮 仍断）。 */}
                  {filePath && (
                    <div className="mved-file-path" title={filePath}>
                      <span className="mved-file-path-k">文件位置</span>
                      <span className="mved-file-path-v">
                        {filePath
                          .split("/")
                          .filter((seg) => seg !== "")
                          .map((seg, i) => (
                            <span className="mved-path-seg" key={i}>
                              /{seg}
                            </span>
                          ))}
                      </span>
                    </div>
                  )}
                </div>

                {/* 作品 logo：两张按主题切换（见 pickLogos），都没有则整块不渲染 */}
                {(logoLight || logoDark) && (
                  <div className={`mved-data-logo${logoLight && logoDark ? "" : " mved-data-logo--single"}`}>
                    {logoLight && <img className="mved-logo-light" src={logoLight} alt="" aria-hidden="true" />}
                    {logoDark && <img className="mved-logo-dark" src={logoDark} alt="" aria-hidden="true" />}
                  </div>
                )}
              </div>
            </div>

            {/* [lc-2252] 标签条 + 标签体：挪到简介位（用户要求「挪到简介上面，
                改成之前那种便签切换」）。详情面板 = 纯简介；关联/角色/更多/评论/吐槽
                面板由 DetailView 通过 tabsNav/tabsBody 传入，交互与经典版一致。 */}
            {tabsNav && (
              <div className="mved-block mved-block-tabs" ref={tabsRef}>{tabsNav}</div>
            )}
            <div className={`mved-block mved-block-desc${tabsBody ? " has-tabs" : ""}`}>
              {tabsBody ?? (overview ? <p className="mved-lead">{overview}</p> : null)}
            </div>
          </div>

          {/* ══ 右栏：海报 / 操作 / 版本 / 主创 / 配音 ══ */}
          <div className="mved-media">
            <div
              className={`mved-stack${fanIn ? " is-in" : ""}`}
              ref={stackRef}
              role="group"
              aria-label="海报轮播"
              onMouseEnter={() => setHovered(true)}
              onMouseLeave={() => setHovered(false)}
              onClick={(e) => {
                const el = (e.target as HTMLElement).closest<HTMLElement>(".mved-card");
                if (el?.dataset.state === "active") return; // 点最前那张不切换
                if (slides.length > 1) setSlideIdx((i) => (i + 1) % slides.length);
              }}
            >
              {/* [lc-2253] fpk style3：卡直接挂进 stack（无 track 层），状态分配照抄
                  styles.ts updatePositions() —— diff 0=active / 1=next / n-1=prev /
                  2=behind1 / 3=behind2 / 其余=deep。 */}
              {slides.map((url, i) => {
                const n = slides.length;
                const diff = (i - slideIdx + n) % n;
                let state = "deep";
                if (diff === 0) state = "active";
                else if (diff === n - 1) state = "prev";
                else if (diff === 1) state = "next";
                else if (diff === 2) state = "behind1";
                else if (diff === 3) state = "behind2";
                return (
                  <div
                    key={url}
                    className="mved-card"
                    data-state={state}
                    /* [lc-2249] --i 给入场动画做错峰延迟 */
                    style={{ ["--i" as string]: String(diff) }}
                  >
                    {/* fpk 结构：卡内一层 bg 承载图片 —— Ken Burns 缩放作用在它身上 */}
                    <div className="mved-card-bg" style={{ backgroundImage: `url("${url}")` }} />
                  </div>
                );
              })}
            </div>

            <div className="mved-actions" ref={actionsRef}>
              <button className="mved-btn-primary" onClick={onPlay} disabled={playing}>
                <PlayGlyph size={16} />
                {playing ? "打开中…" : resumeAt ? "继续播放" : "立即播放"}
              </button>
              <button
                className={`mved-btn-ghost${fav ? " on" : ""}`}
                onClick={toggleFav}
                disabled={!favGuid || favBusy}
              >
                <svg viewBox="0 0 24 24" fill={fav ? "currentColor" : "none"} stroke="currentColor" strokeWidth={fav ? 0 : 1.8}>
                  <path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" />
                </svg>
                {favBusy ? "处理中…" : fav ? "已收藏" : "收藏"}
              </button>
              {onMeta && (
                <button className="mved-btn-square" onClick={onMeta} title="元数据操作（编辑 / 手动匹配 / 刷新）" aria-label="元数据操作">
                  ⋯
                </button>
              )}
            </div>
            {note && <div className="mved-note">{note}</div>}

            {/* ── 版本条：常驻占位（[lc-2264] 没有版本也渲染一行，给下方主创/配音留位）。
                没取到版本 / 确实没有多版本时显示「默认版本」占位（不可点）；
                版本多时整条横向滚动，滚轮悬停在条上直接横滚（上方 wheel 监听）。 ── */}
            <div className="mved-versions" role="group" aria-label="选择版本" ref={versionsRef}>
              {!versions || versions.length === 0 ? (
                <button
                  type="button"
                  className="mved-version"
                  disabled
                  title="当前条目没有多版本，将直接播放默认版本"
                >
                  <span className="ver-name">默认版本</span>
                </button>
              ) : (
                versions.map((v) => (
                  <button
                    key={v.mediaGuid}
                    type="button"
                    className={`mved-version${v.mediaGuid === selectedVersion ? " active" : ""}`}
                    title={v.label}
                    aria-pressed={v.mediaGuid === selectedVersion}
                    onClick={() => onVersionChange?.(v.mediaGuid)}
                  >
                    {/* [lc-2265] 两行信息卡（照抄经典版 mv-version 结构）：
                        上行名称、下行元信息（体积·编码·位深，DetailView 已传 meta）。 */}
                    <span className="ver-name">{v.label}</span>
                    {v.meta && <span className="ver-meta">{v.meta}</span>}
                  </button>
                ))
              )}
            </div>

            {/* [lc-2255] 主创信息（导演/编剧/音乐/角色设计）：填充右栏中部空位。
                crew 没有匹配职务时整块不渲染，不留空框。 */}
            {facts.length > 0 && (
              <div className="mved-facts">
                {facts.map((f) => (
                  <div className="mved-fact-row" key={f.label}>
                    <span className="mved-fact-k">{f.label}</span>
                    <span className="mved-fact-v">{f.value}</span>
                  </div>
                ))}
              </div>
            )}

            {/* 配音阵容：4 等分，人多时换行；没有演职员数据时整块不渲染 */}
            {topCast.length > 0 && (
              <div className="mved-cast-row" role="list" aria-label="主要配音">
                {topCast.map((c, i) => {
                  /* [lc-2285→2287] 头像两级：TMDB 方形人脸裁剪 w180_and_h180_face
                     （实测 200、TMDB 网页自用；弃用的只是 2:3 的 w185_and_h278_face），
                     加载失败回退 w185 原图缩放（个别图没有 face 生成版），再失败露占位底。
                     profile 可能是完整代理 URL（飞牛头像兜底，lc-2287）—— 以 "/" 开头才走 TMDB。 */
                  const profile = c.profile || "";
                  const faceAv = profile.startsWith("/")
                    ? tmdbImg(profile, "w180_and_h180_face")
                    : profile; /* 完整 URL（飞牛兜底头像）直接当 face 源 */
                  const fullAv = profile.startsWith("/") ? tmdbImg(profile, "w185") : profile;
                  /* [lc-2288] 无图占位显示姓氏首字（中文取首字、拉丁取首字母大写）：
                     img 两级回退都失败隐藏后，这层文字自然露出。 */
                  const initial = (String(c.name || "").trim().slice(0, 1) || "?").toUpperCase();
                  const clickable = !!onOpenPerson && Number(c.id ?? 0) > 0;
                  const role = c.character ? `饰 ${c.character}` : c.jobLabel || c.job || "";
                  return (
                    <div
                      className={`mved-cast${clickable ? " clickable" : ""}`}
                      key={`${c.id ?? i}`}
                      role="listitem"
                      tabIndex={clickable ? 0 : undefined}
                      onClick={clickable ? () => onOpenPerson?.(c) : undefined}
                      onKeyDown={
                        clickable
                          ? (e) => {
                              if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                onOpenPerson?.(c);
                              }
                            }
                          : undefined
                      }
                    >
                      <div
                        className="mved-cast-avatar"
                        style={!faceAv && fullAv ? { backgroundImage: `url("${fullAv}")` } : undefined}
                      >
                        <span className="mved-cast-initial" aria-hidden="true">
                          {initial}
                        </span>
                        {faceAv ? (
                          <img
                            src={faceAv}
                            alt=""
                            loading="lazy"
                            onError={(e) => {
                              const el = e.currentTarget;
                              if (!el.dataset.fb) {
                                el.dataset.fb = "1";
                                el.src = fullAv;
                              } else {
                                el.style.display = "none"; /* 两个尺寸都失败 → 露占位底色 */
                              }
                            }}
                          />
                        ) : null}
                      </div>
                      <div className="mved-cast-meta">
                        <div className="mved-cast-name">{c.name}</div>
                        {role && <div className="mved-cast-role">{role}</div>}
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </header>
      </div>
    </div>
  );
}
