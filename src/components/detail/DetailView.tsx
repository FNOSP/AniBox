/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { type UIEvent, useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createPortal } from "react-dom";
import { Check as IcoCheckL, Clapperboard as IcoClapL, Film as IcoFilmL, Play as IcoPlayL, X as IcoXL } from "lucide-react";
import { DetailSkeleton } from "./DetailSkeleton";
// [lc-1629] 飞牛每集/条目自定义元数据操作弹层（编辑元数据 / 手动匹配 / 刷新 / 解除匹配）
import { MetaEditModal, type MetaTarget } from "./MetaEditModal";
import { BGM_PAGE, TAB_LABEL, bgmList, fnosCast, pickGenres } from "./helpers";
import { OverviewTab } from "./tabs/OverviewTab";
import { RelatedTab } from "./tabs/RelatedTab";
import { CastTab } from "./tabs/CastTab";
import { ReviewsTab } from "./tabs/ReviewsTab";
import { CommentsTab } from "./tabs/CommentsTab";
import { ExtraTab } from "./tabs/ExtraTab";
import { RatingWidget } from "./RatingWidget";
// [lc-1627] 在线播放先搁置：WebrsPanel 暂不挂载（无源条目改为显示「未查询到资源」）。
// 恢复时把下行 import 与 1271 行附近的分支一起放回来即可。
// import { WebrsPanel } from "../webrs/WebrsPanel";
import { fmtDur, pick } from "../../lib/format";
import { bgmImg, imgUrl, pickLargestImg, tmdbImg } from "../../lib/images";
import { prefersReducedMotion } from "../../lib/motion";
import { useRefresh } from "../../hooks/useRefresh";
import { BgmCharacter, BgmComment, BgmEpisodeRow, BgmMatch, BgmPerson, BgmRelated, BgmReview, BgmTag, Episode, MediaItem, Season, TmdbCreditPerson, TmdbCredits } from "../../types";


export function DetailView({ base, onBack }: { base: MediaItem; onBack: () => void }) {
  const [data, setData] = useState<Record<string, unknown> | null>(null);
  const [seasons, setSeasons] = useState<Season[]>([]);
  const [seasonIdx, setSeasonIdx] = useState(0);
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  // [lc-1595] 播放结束 → 强制刷新观看进度：`ts`/`watched` 随每次播放变化，而本页原先只在
  //   挂载/切条目/切季时取数 ⇒ 用户「看完第 3 集退出来，状态还是之前的」。
  //   事件来自 Rust（mpv.rs）：正常看完/退出 = `mpv-load{phase:"ended"}`；播放失败 = `mpv-end`。
  //   ⚠ 必须**带 force** 取数：季/集有 10 分钟缓存（lc-1578），不带 force 会拿回旧进度。
  const [progTick, setProgTick] = useState(0);
  const forceSeasonRef = useRef(false);
  const forceEpsRef = useRef(false);
  const lastEpGuidRef = useRef("");
  // [lc-1597] 播放结束后是否要跑一次「BGM 分集看过」同步（见下方 syncBgmRef 的 effect）
  const syncBgmRef = useRef(false);
  // TMDB 完整详情（剧照/相似/更多）：「详情」「更多」两个标签页的 TMDB 侧数据源
  const [tmdb, setTmdb] = useState<Record<string, unknown> | null>(null);
  // [lc-1507] TMDB 请求是否已 settle（成败都置位）：区分「加载中」（骨架）与「已失败」
  // （不再转骨架，改显示 Bangumi 简介兜底 / 空态）—— 否则失败后骨架永远转
  const [tmdbDone, setTmdbDone] = useState(false);
  const [err, setErr] = useState("");
  // [lc-118] 详情页分段栏当前项
  const [tab, setTab] = useState("overview");
  // [lc-1423] 重新引入简介展开态：true = 弹窗打开（看完整简介）。
  //   注意语义与 lc-1410 之前不同 —— 那时是"就地铺开"（会撑高容器、拉长海报），
  //   现在是"**弹出弹窗**看全文"，容器高度不受影响。
  const [introFull, setIntroFull] = useState(false);
  // [lc-119] 吐槽（Bangumi 评论）：懒加载 —— 用户切到该分段时才匹配+拉评论（避免拖慢首屏）
  const [bgm, setBgm] = useState<BgmMatch | null>(null);
  const bgmRef = useRef<BgmMatch | null>(null); // 供 legacy 回填任务/翻页读最新值（不进依赖数组）
  // [lc-1401] 匹配是否已尝试（成败都置位）。旧版用它给「吐槽」分段做入口闸门；
  //   新版式标签条恒在（不增删项），这个标志只作诊断用途 —— 保留 setter 以沿用同一条匹配路径。
  const [, setBgmProbed] = useState(false);
  const [comments, setComments] = useState<BgmComment[]>([]);
  const [commentsErr, setCommentsErr] = useState("");
  const [commentsBusy, setCommentsBusy] = useState(false); // 首屏拉取（骨架屏）
  const [commentsMoreBusy, setCommentsMoreBusy] = useState(false); // 翻页中（列表尾部提示）
  const [commentsDone, setCommentsDone] = useState(false); // 已翻到底
  const [commentsNonce, setCommentsNonce] = useState(0); // 失败重试触发器
  const commentsOffset = useRef(0); // 下一页 offset = 已累计的**实际返回条数**
  // [lc-1412] 评论（长影评）状态：与吐槽同款一套（懒加载 / 翻页 / 到底 / 重试），但数据源是 /reviews
  const [reviews, setReviews] = useState<BgmReview[]>([]);
  const [reviewsErr, setReviewsErr] = useState("");
  const [reviewsBusy, setReviewsBusy] = useState(false);
  const [reviewsMoreBusy, setReviewsMoreBusy] = useState(false);
  const [reviewsDone, setReviewsDone] = useState(false);
  const [reviewsNonce, setReviewsNonce] = useState(0);
  const reviewsOffset = useRef(0);
  const reviewsBusyRef = useRef(false);
  const reviewsDoneRef = useRef(false);
  const commentsBusyRef = useRef(false);
  const commentsDoneRef = useRef(false);
  // 更多分段（Bangumi 角色/制作人员）：懒加载 —— 与吐槽同套路（骨架屏 / 失败重试 / 空态）
  const [chars, setChars] = useState<BgmCharacter[]>([]);
  const [persons, setPersons] = useState<BgmPerson[]>([]);
  const [moreErr, setMoreErr] = useState("");
  const [moreBusy, setMoreBusy] = useState(false); // 首屏拉取（骨架屏）
  const [moreNonce, setMoreNonce] = useState(0); // 失败重试触发器
  const [moreProbed, setMoreProbed] = useState(false); // 拉取是否已尝试（成败都置位；空壳分段靠它退场）
  const moreLoadedRef = useRef(false); // 成功拉到过 → 不重复拉（切走再回来不重打请求）
  // [lc-128] 演员表（**TMDB 主源**）：与「更多」同套路懒加载；Rust 侧按 guid + tmdb id 落盘缓存，
  // 命中本地就零请求。Bangumi/飞牛侧只在 TMDB 无数据时兜底展示。
  const [credits, setCredits] = useState<TmdbCredits | null>(null);
  const [creditsErr, setCreditsErr] = useState("");
  const [creditsBusy, setCreditsBusy] = useState(false);
  const [creditsNonce, setCreditsNonce] = useState(0);
  const creditsLoadedRef = useRef(false);
  // [lc-128] 剧照点击放大（original 尺寸：走同一套三级缓存，拉了即落盘）
  const [zoom, setZoom] = useState("");
  // [lc-1401] 关联条目（Bangumi /subjects/{id}/subjects）：与「更多」同套懒加载骨架
  const [related, setRelated] = useState<BgmRelated[]>([]);
  const [relatedErr, setRelatedErr] = useState("");
  const [relatedBusy, setRelatedBusy] = useState(false);
  const [relatedNonce, setRelatedNonce] = useState(0);
  const relatedLoadedRef = useRef(false);
  // [lc-1401] 右栏选集：本机条目集数 + Bangumi 集号对齐表（desc/标题回填）
  const [bgmEps, setBgmEps] = useState<BgmEpisodeRow[]>([]);
  // [lc-1401] Bangumi 条目详情（顶部标签框用它的 tags[]；与 bgm 匹配分开存，语义不同）
  const [bgmSub, setBgmSub] = useState<Record<string, unknown> | null>(null);
  // 注意：与吐槽一样**不做 busy 重入闸门** —— StrictMode 的双调用会「首跑 abort + 二跑早退」，
  // 骨架屏会永久卡住；靠 alive 标志丢弃过期结果即可（最坏情况是 dev 下多发一次请求）。

  // [lc-1513] 标签页切换淡入（详情/评论/吐槽/关联/角色/更多）：class 加一下即触发
  const [tabAnim, setTabAnim] = useState(false);
  useEffect(() => {
    if (prefersReducedMotion()) return;
    setTabAnim(true);
    const t = window.setTimeout(() => setTabAnim(false), 260);
    return () => window.clearTimeout(t);
  }, [tab]);

  const isSeries = base.type === "TvSeries" || base.type === "TV";
  // [lc-1444] 发现页条目形态：guid 是 `discover-<tmdbId>` 占位标识（不是真飞牛 guid），
  //   没有飞牛条目可查 —— 详情主体靠 TMDB（条目自带 tmdb_id 直接定位）+ Bangumi（标题匹配），
  //   选集区显示占位。dataReady = 主体可以渲染（真条目等 data 到位；发现页条目立即渲染）。
  const noFnOS = /^discover-/.test(base.guid);
  const dataReady = !!data || noFnOS;

  // [lc-1444] Bangumi 定位统一入口：发现页右栏条目自带 bgm_id → **直连**（省一次标题匹配，
  //   也不会匹配错同名番）；否则 bgm_match(标题+年)。结果写 bgmRef + setBgm，供
  //   首屏标签框 / 影评 / 吐槽 / 更多 / 关联五个懒加载点共用同一条路径。
  const resolveBgm = useCallback(async (): Promise<BgmMatch | null> => {
    const cached = bgmRef.current;
    if (cached) return cached;
    let m: BgmMatch | null = null;
    if (base.bgm_id) {
      m = { subject_id: base.bgm_id };
    } else {
      const title = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
      const r = await invoke<BgmMatch | null>("bgm_match", {
        query: title,
        year: String(base.release_date || "").slice(0, 4) || null,
        mediaType: base.type ?? null,
        cacheKey: base.guid,
      });
      m = r && r.subject_id ? r : null;
    }
    bgmRef.current = m;
    setBgm(m);
    return m;
  }, [base.bgm_id, base.guid, base.title, base.type, base.release_date, data]);

  /** [lc-128] TMDB 精确锚点：飞牛条目 `trim_id` 形如 `tm63578`（TMDB id）/ `tt325158`（IMDb id）/
   *  `bg456080`（Bangumi，无 TMDB id）；另有显式 `tmdb_id`/`imdb_id` 字段时优先。有锚点就**直接定位**，
   *  没有才回落标题搜索（Rust 侧搜索带相似度门槛，并回传 matched_by 说明"这条是谁给的 id"）。 */
  const tmdbAnchor = (() => {
    const d = data ?? {};
    const str = (v: unknown): string => (v == null ? "" : String(v).trim());
    const directTmdb = str(pick(d, ["tmdbId", "tmdb_id"]));
    const imdb = str(pick(d, ["imdb_id", "imdbId"]));
    const trim = str(pick(d, ["trim_id", "trimId"]));
    let tmdbId = /^\d+$/.test(directTmdb) ? directTmdb : "";
    if (!tmdbId) {
      const m = /^(?:tm)(\d+)$/i.exec(trim);
      if (m) tmdbId = m[1];
      else if (/^\d+$/.test(trim)) tmdbId = trim; // 纯数字：老口径当 TMDB id
    }
    const imdbId = imdb || (/^tt\d+$/i.test(trim) ? trim.toLowerCase() : "");
    return { trimId: trim, tmdbId, imdbId };
  })();

  // [lc-1629] 统一的「带 force 重取」：置三个 force ref + bump progTick，让条目详情/季/集都跳过缓存。
  //   播放结束（lc-1595）与元数据写操作成功后（编辑/匹配/刷新/解除）共用同一条路径。
  const refreshAll = useCallback(() => {
    forceSeasonRef.current = true;
    forceEpsRef.current = true;
    syncBgmRef.current = true;
    setProgTick((n) => n + 1);
  }, []);

  /* ── [lc-1640] 顶栏 ⟳：复用 refreshAll（带 force 重取条目详情/季/集，跳过 120 天/10 分钟缓存）──
     完成信号：详情主体请求会把 data（先被清空、再回填）——用 dataReady 的 false→true 收尾；
     noFnOS 详情（发现页占位条目）没有飞牛主体可刷，dataReady 恒真，故另设 1.5s 兜底，避免 ⟳ 长亮。 */
  const { tick: refreshTick, reportBusy } = useRefresh();
  const lastRefreshTick = useRef(refreshTick);
  const [extRefreshing, setExtRefreshing] = useState(false);
  const extSawPendingRef = useRef(false);
  useEffect(() => {
    if (refreshTick === lastRefreshTick.current) return;
    lastRefreshTick.current = refreshTick;
    extSawPendingRef.current = false; // 每次刷新重新等待"详情被清空再回填"
    setExtRefreshing(true);
    refreshAll();
  }, [refreshTick, refreshAll]);
  useEffect(() => {
    if (!extRefreshing) return;
    reportBusy("detail", true);
    const t = window.setTimeout(() => setExtRefreshing(false), 1500);
    return () => {
      window.clearTimeout(t);
      reportBusy("detail", false);
    };
  }, [extRefreshing, reportBusy]);
  useEffect(() => {
    if (!extRefreshing) return;
    if (!dataReady) {
      extSawPendingRef.current = true;
      return;
    }
    if (extSawPendingRef.current) {
      extSawPendingRef.current = false;
      setExtRefreshing(false);
    }
  }, [extRefreshing, dataReady]);

  // [lc-1629] 元数据操作弹层的目标（null = 关闭）。每集行的「⋯」与条目「⋯」都设它。
  const [metaTarget, setMetaTarget] = useState<MetaTarget | null>(null);

  // [lc-1595] 监听播放结束事件 → 触发一次带 force 的重取（见上）。
  useEffect(() => {
    const bump = () => refreshAll();
    const un1 = listen<{ phase?: string }>("mpv-load", (e) => {
      if (e.payload?.phase === "ended") bump();
    });
    const un2 = listen("mpv-end", bump);
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, [refreshAll]);

  useEffect(() => {
    setData(null);
    setSeasons([]);
    setSeasonIdx(0);
    setEpisodes([]);
    if (noFnOS) {
      // 发现页条目：无飞牛条目，跳过详情/季/集三条飞牛请求；清掉可能残留的详情错误
      setErr("");
      return;
    }
    // [lc-1629] 读一次 force 并消费：播放结束/元数据写操作成功后置真 → 详情也跳过 120 天缓存，
    //   否则「编辑元数据」改完标题/简介看不到变化。
    const forceItem = forceSeasonRef.current;
    forceSeasonRef.current = false;
    invoke<Record<string, unknown>>("get_item_detail", { guid: base.guid, force: forceItem })
      .then((d) => setData(d))
      .catch((e) => setErr(String(e)));
    // 层级：剧 → 季 → 集。episode/list 的入参是季 guid（传剧 guid 实测 data:null），
    // 所以先 season/list/{剧guid}，选中季后再 episode/list/{季guid}。
    if (isSeries) {
      invoke<Season[]>("get_seasons", { guid: base.guid, force: forceItem })
        .then((ss) => setSeasons(Array.isArray(ss) ? ss : []))
        .catch(() => {});
    }
  }, [base.guid, isSeries, noFnOS, progTick]);

  useEffect(() => {
    const s = seasons[seasonIdx];
    if (!s) return;
    const force = forceEpsRef.current;
    forceEpsRef.current = false;
    // 只在**换季**时清空列表：播放结束的刷新保留现有行（否则右栏会闪一下空白）
    if (s.guid !== lastEpGuidRef.current) {
      lastEpGuidRef.current = s.guid;
      setEpisodes([]);
    }
    invoke<Episode[]>("get_episodes", { guid: s.guid, force })
      .then((eps) =>
        setEpisodes(
          (Array.isArray(eps) ? eps : []).map((e) => ({
            ...e,
            quality:
              Array.isArray(e?.media_stream?.resolutions) && e.media_stream.resolutions[0]
                ? String(e.media_stream.resolutions[0]).replace(/p$/i, "")
                : undefined,
          })),
        ),
      )
      .catch(() => {});
  }, [seasons, seasonIdx, progTick]);

  // TMDB 完整详情（剧照/相似/更多/演职人员）：主仓库 fetchShowDetails 同款链路，失败静默降级
  // [lc-128] 额外带上 trim_id / tmdb_id / imdb_id 锚点：能精确定位就不靠标题搜索（matchedBy 见日志）
  // [lc-1507] 发现页 Bangumi 条目（无 tmdb 锚点）的标题搜索失败率很高 —— TMDB 对新番/第二季
  //   经常没有条目或没有中文译名（实测「转生就是剑 第二季」sim=0.17 只有第一季 0.22）。
  //   两级兜底：① 首搜失败 → 用 Bangumi **日文原名**（bgmSub.name）重搜一次（TMDB 收录日文
  //   原名远多于中文译名，且不带年份防错标）；② 日文重搜也失败 → 详情信息 tab 显示
  //   Bangumi 简介而不是永久骨架。
  const tmdbTitle = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
  const bgmNameJp = String(pick(bgmSub ?? {}, ["name"]) ?? "").trim();
  // [lc-1573] tmdb 重置只在**切换条目**时做一次：本 effect 的依赖里有 bgmNameJp（来自懒加载的
  //   bgmSub，晚于 data 到达）—— lc-1507 把它加进依赖后，bgmSub 一到就会重跑本段；若每次都
  //   setTmdb(null)，已渲染的 TMDB 海报会被先清后取：封面在「TMDB 图 → 飞牛图 → TMDB 图」
  //   之间连跳（用户反馈「进入详情页后封面会跳动一下」的根因）。用 ref 认条目 guid。
  const tmdbGuidRef = useRef("");
  useEffect(() => {
    if (tmdbGuidRef.current !== base.guid) {
      tmdbGuidRef.current = base.guid;
      setTmdb(null);
      setTmdbDone(false);
    }
    // 等条目详情到位再发：锚点（trim_id 等）就来自它，先发一次纯标题搜索再重发纯属浪费。
    // [lc-1444] 发现页条目例外：条目自带 tmdb_id，直接精确定位，不等（也没有）飞牛详情。
    if (!dataReady || !tmdbTitle) return;
    let alive = true;
    const fetchDetails = (title: string, year: string) =>
      invoke<Record<string, unknown>>("tmdb_show_details", {
        arg: {
          title,
          year,
          mediaType: base.type === "Movie" ? "movie" : "tv",
          trimId: tmdbAnchor.trimId || undefined,
          tmdbId: noFnOS
            ? base.tmdb_id
              ? String(base.tmdb_id)
              : undefined // 右栏新番条目没有 TMDB id → 走标题搜索
            : tmdbAnchor.tmdbId || undefined,
          imdbId: tmdbAnchor.imdbId || undefined,
        },
      });
    (async () => {
      let d: Record<string, unknown> | null = null;
      try {
        const r = await fetchDetails(tmdbTitle, String(base.release_date || "").slice(0, 4));
        if (r && !r.error) d = r;
      } catch {
        /* 静默：走日文重搜 */
      }
      if (!alive) return;
      // ① 首搜失败（或没搜到）→ 日文原名重搜（不带年份：TMDB/飞牛的年份错标很常见）
      if (!d && bgmNameJp && bgmNameJp !== tmdbTitle) {
        try {
          const r2 = await fetchDetails(bgmNameJp, "");
          if (r2 && !r2.error) d = r2;
        } catch {
          /* 静默：兜底空态 */
        }
      }
      if (!alive) return;
      if (d) setTmdb(d);
      setTmdbDone(true);
    })();
    return () => {
      alive = false;
    };
  }, [
    base.guid,
    tmdbTitle,
    bgmNameJp,
    dataReady,
    noFnOS,
    base.tmdb_id,
    base.release_date,
    base.type,
    tmdbAnchor.trimId,
    tmdbAnchor.tmdbId,
    tmdbAnchor.imdbId,
  ]);

  // [lc-119] 切换条目（本组件不随 base 重挂载）时清空 Bangumi 状态，避免把上一部的吐槽/匹配带过来
  useEffect(() => {
    bgmRef.current = null;
    commentsOffset.current = 0;
    commentsDoneRef.current = false;
    commentsBusyRef.current = false;
    setBgm(null);
    setBgmProbed(false);
    setComments([]);
    setCommentsErr("");
    setCommentsBusy(false);
    setCommentsMoreBusy(false);
    setCommentsDone(false);
    // [lc-1412] 评论（长影评）同样重置
    reviewsOffset.current = 0;
    reviewsDoneRef.current = false;
    reviewsBusyRef.current = false;
    setReviews([]);
    setReviewsErr("");
    setReviewsBusy(false);
    setReviewsMoreBusy(false);
    setReviewsDone(false);
    moreLoadedRef.current = false;
    setChars([]);
    setPersons([]);
    setMoreErr("");
    setMoreBusy(false);
    setMoreProbed(false);
    creditsLoadedRef.current = false;
    setCredits(null);
    setCreditsErr("");
    setCreditsBusy(false);
    relatedLoadedRef.current = false;
    setRelated([]);
    setRelatedErr("");
    setRelatedBusy(false);
    setBgmEps([]);
    setBgmSub(null);
    setZoom("");
  }, [base.guid]);

  // [lc-1401] 条目详情（bgm_subject）：新版式顶部的「标签框」用它（tags[]），**必须一开始就有**，
  // 所以不再等用户点某个标签页 —— 匹配 + 详情两次请求在进页面时就发（都走镜像 + 落盘缓存：
  // match 30 天、subject 6h），首屏不等它们（标签框自己显示骨架屏）。
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        let m = bgmRef.current;
        if (!m) {
          m = await resolveBgm();
          if (!alive) return;
          setBgmProbed(true);
        }
        if (!m) return;
        const s = await invoke<Record<string, unknown>>("bgm_subject", { id: m.subject_id });
        if (alive && s && !(s as { error?: unknown }).error) setBgmSub(s);
      } catch {
        /* 标签框拉不到就空着（下面的各标签页有各自的失败重试），绝不阻塞首屏 */
      }
    })();
    return () => {
      alive = false;
    };
  }, [base.guid, data]);

  // [lc-1412] 评论（长影评）懒加载：与吐槽同款（切到该页才发、复用 bgmRef 匹配、nonce 重试），
  // 数据源换成 /p1/.../reviews。与吐槽**互相独立**：各自有自己的 offset/到底/错误态。
  useEffect(() => {
    if (tab !== "reviews") return;
    if (reviewsOffset.current > 0) return;
    let alive = true;
    (async () => {
      reviewsBusyRef.current = true;
      setReviewsBusy(true);
      setReviewsErr("");
      try {
        let m = bgmRef.current;
        if (!m) {
          m = await resolveBgm();
          if (!alive) return;
          setBgmProbed(true);
        }
        if (!m) throw new Error("未匹配到 Bangumi 条目");
        const r = await invoke<{ data?: BgmReview[] }>("bgm_reviews", {
          id: m.subject_id,
          offset: 0,
          limit: BGM_PAGE,
        });
        const list = Array.isArray(r?.data) ? r.data : [];
        if (!alive) return;
        reviewsOffset.current = list.length;
        setReviews(list);
        if (list.length === 0) {
          reviewsDoneRef.current = true;
          setReviewsDone(true);
        }
      } catch (e) {
        if (alive) {
          setBgmProbed(true);
          setReviewsErr(String(e));
        }
      } finally {
        reviewsBusyRef.current = false;
        if (alive) setReviewsBusy(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [tab, reviewsNonce, base.guid]);

  // [lc-119] 吐槽懒加载（只在切到「吐槽」分段时才开始）：先 bgm_match（结果按 base.guid 落盘缓存），
  // 再拉第一页 /p1/.../comments。重试按钮通过 commentsNonce 重新走这段。已有数据则不重复拉。
  useEffect(() => {
    if (tab !== "comments") return;
    if (commentsOffset.current > 0) return; // 已拉到过数据（含翻页）→ 不重拉
    let alive = true;
    (async () => {
      commentsBusyRef.current = true;
      setCommentsBusy(true);
      setCommentsErr("");
      try {
        let m = bgmRef.current;
        if (!m) {
          m = await resolveBgm();
          if (!alive) return;
          setBgmProbed(true);
        }
        if (!m) throw new Error("未匹配到 Bangumi 条目");
        const r = await invoke<{ data?: BgmComment[] }>("bgm_comments", {
          id: m.subject_id,
          offset: 0,
          limit: BGM_PAGE,
        });
        const list = Array.isArray(r?.data) ? r.data : [];
        if (!alive) return;
        commentsOffset.current = list.length; // ← 实际返回条数（不假定 limit 生效）
        setComments(list);
        if (list.length === 0) {
          commentsDoneRef.current = true;
          setCommentsDone(true);
        }
      } catch (e) {
        if (alive) {
          setBgmProbed(true);
          setCommentsErr(String(e));
        }
      } finally {
        commentsBusyRef.current = false;
        if (alive) setCommentsBusy(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [tab, commentsNonce, base.guid]);

  // 更多分段懒加载（只在切到「更多」时才开始）：复用 bgm 匹配结果（bgmRef）——
  // 吐槽先点过就直接用，两边都没匹配过才发 bgm_match（同 title/年份/type/cacheKey 参数）。
  // 注意：这里**不**置 bgmProbed —— 那个标志的语义是「吐槽已探测」，是「吐槽」分段的入口闸门。
  // 角色/制作人员一次拉两条（同 subject）：任一失败整体失败，重试按钮走 moreNonce 重走本段。
  useEffect(() => {
    if (tab !== "more") return;
    if (moreLoadedRef.current) return; // 已拉到过 → 不重拉
    let alive = true;
    (async () => {
      setMoreBusy(true);
      setMoreErr("");
      try {
        let m = bgmRef.current;
        if (!m) {
          m = await resolveBgm();
          if (!alive) return;
        }
        if (!m) throw new Error("未匹配到 Bangumi 条目");
        const [cr, pr] = await Promise.all([
          invoke<BgmCharacter[]>("bgm_characters", { id: m.subject_id }),
          invoke<BgmPerson[]>("bgm_persons", { id: m.subject_id }),
        ]);
        if (!alive) return;
        setChars(bgmList<BgmCharacter>(cr));
        setPersons(bgmList<BgmPerson>(pr));
        moreLoadedRef.current = true;
      } catch (e) {
        if (alive) setMoreErr(String(e));
      } finally {
        if (alive) {
          setMoreBusy(false);
          setMoreProbed(true);
        }
      }
    })();
    return () => {
      alive = false;
    };
  }, [tab, moreNonce, base.guid]);

  /** 失败重试：清掉错误从头发一次（数据成功前 moreLoadedRef 恒为 false，无需额外重置） */
  const retryMore = () => setMoreNonce((n) => n + 1);

  // [lc-1401] 关联条目懒加载：切到「关联条目」才发。复用 bgmRef（吐槽/更多已匹配过就直接用），
  // 都没匹配过才发 bgm_match —— 三条懒加载路径共用同一份匹配结果的模式，与「更多」一致。
  useEffect(() => {
    if (tab !== "related") return;
    if (relatedLoadedRef.current) return;
    let alive = true;
    (async () => {
      setRelatedBusy(true);
      setRelatedErr("");
      try {
        let m = bgmRef.current;
        if (!m) {
          m = await resolveBgm();
          if (!alive) return;
        }
        if (!m) throw new Error("未匹配到 Bangumi 条目");
        const r = await invoke<BgmRelated[]>("bgm_related", { id: m.subject_id });
        if (!alive) return;
        setRelated(bgmList<BgmRelated>(r));
        relatedLoadedRef.current = true;
      } catch (e) {
        if (alive) setRelatedErr(String(e));
      } finally {
        if (alive) setRelatedBusy(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [tab, relatedNonce, base.guid]);

  /** 失败重试（关联条目）：与 retryMore 同款 —— 成功前 relatedLoadedRef 恒为 false */
  const retryRelated = () => setRelatedNonce((n) => n + 1);

  // [lc-128] 演员表（**TMDB 主源**）：切到「更多」分段时懒加载一次；锚点优先（trim_id/tmdbId/imdbId），
  // 锚不到由 Rust 侧回落标题搜索（带相似度门槛）。结果在 Rust 侧落盘缓存 7 天 → 重复进详情页零请求。
  // 失败不影响 Bangumi/飞牛兜底展示（各自独立状态机）。
  useEffect(() => {
    if (tab !== "more") return;
    if (creditsLoadedRef.current) return;
    let alive = true;
    (async () => {
      setCreditsBusy(true);
      setCreditsErr("");
      try {
        const title = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
        const r = await invoke<TmdbCredits>("tmdb_credits", {
          arg: {
            title,
            year: String(base.release_date || "").slice(0, 4),
            mediaType: base.type === "Movie" ? "movie" : "tv",
            trimId: tmdbAnchor.trimId || undefined,
            tmdbId: noFnOS
              ? base.tmdb_id
                ? String(base.tmdb_id)
                : undefined // 右栏新番条目没有 TMDB id → 走标题搜索
              : tmdbAnchor.tmdbId || undefined,
            imdbId: tmdbAnchor.imdbId || undefined,
            cacheKey: base.guid,
          },
        });
        if (!alive) return;
        setCredits(r);
        creditsLoadedRef.current = true;
      } catch (e) {
        if (alive) setCreditsErr(String(e));
      } finally {
        if (alive) setCreditsBusy(false);
      }
    })();
    return () => {
      alive = false;
    };
    // data 到位后才拿得到 trim_id 锚点：进「更多」早于详情返回时，数据一到会带着锚点重跑一次
  }, [tab, creditsNonce, base.guid, data, tmdbAnchor.trimId, tmdbAnchor.tmdbId, tmdbAnchor.imdbId]);

  /** [lc-128] 失败重试：清掉错误从头发一次（成功前 creditsLoadedRef 恒为 false） */
  const retryCredits = () => setCreditsNonce((n) => n + 1);

  /** [lc-1412] 评论翻页：与吐槽同款（offset += 实际返回条数，0 条即到底） */
  const loadMoreReviews = async () => {
    const m = bgmRef.current;
    if (!m || reviewsBusyRef.current || reviewsDoneRef.current) return;
    reviewsBusyRef.current = true;
    setReviewsMoreBusy(true);
    setReviewsErr("");
    try {
      const r = await invoke<{ data?: BgmReview[] }>("bgm_reviews", {
        id: m.subject_id,
        offset: reviewsOffset.current,
        limit: BGM_PAGE,
      });
      const list = Array.isArray(r?.data) ? r.data : [];
      reviewsOffset.current += list.length;
      setReviews((prev) => [...prev, ...list]);
      if (list.length === 0) {
        reviewsDoneRef.current = true;
        setReviewsDone(true);
      }
    } catch (e) {
      setReviewsErr(String(e));
    } finally {
      reviewsBusyRef.current = false;
      setReviewsMoreBusy(false);
    }
  };

  /** [lc-1412] 评论重试：已有列表续拉，空列表从头（含重新匹配） */
  const retryReviews = () => {
    if (reviews.length > 0) {
      setReviewsErr("");
      void loadMoreReviews();
    } else {
      setReviewsNonce((n) => n + 1);
    }
  };

  /** [lc-119] 翻下一页：offset += 实际返回条数；返回 0 条即到底（不再假定 limit/总数） */
  const loadMoreComments = async () => {
    const m = bgmRef.current;
    if (!m || commentsBusyRef.current || commentsDoneRef.current) return;
    commentsBusyRef.current = true;
    setCommentsMoreBusy(true);
    setCommentsErr("");
    try {
      const r = await invoke<{ data?: BgmComment[] }>("bgm_comments", {
        id: m.subject_id,
        offset: commentsOffset.current,
        limit: BGM_PAGE,
      });
      const list = Array.isArray(r?.data) ? r.data : [];
      commentsOffset.current += list.length; // ← 实际返回条数
      setComments((prev) => [...prev, ...list]);
      if (list.length === 0) {
        commentsDoneRef.current = true;
        setCommentsDone(true);
      }
    } catch (e) {
      setCommentsErr(String(e));
    } finally {
      commentsBusyRef.current = false;
      setCommentsMoreBusy(false);
    }
  };

  /** [lc-119] 失败重试：已有列表 → 从当前 offset 续拉；空列表 → 重新从头（含重新匹配） */
  const retryComments = () => {
    if (comments.length > 0) {
      setCommentsErr("");
      void loadMoreComments();
    } else {
      setCommentsNonce((n) => n + 1);
    }
  };

  /** [lc-119] 滚动到底自动加载下一页：extentAfter = scrollHeight - scrollTop - clientHeight < 200 */
  const onDetailScroll = (e: UIEvent<HTMLElement>) => {
    // [lc-1412] 评论 / 吐槽两页各自触底翻页（互不影响）
    const el = e.currentTarget;
    if (el.scrollHeight - el.scrollTop - el.clientHeight >= 200) return;
    if (activeTab === "comments") {
      if (!bgm || commentsDone || commentsErr || commentsMoreBusy) return;
      void loadMoreComments();
    } else if (activeTab === "reviews") {
      if (!bgm || reviewsDone || reviewsErr || reviewsMoreBusy) return;
      void loadMoreReviews();
    }
  };

  // [lc-119] 单集简介回填（替换旧版 —— 唯一允许改动的既有逻辑）：
  // 旧版对整季每集 **无并发上限** 地并发 invoke("get_item_detail")（Promise.allSettled(episodes.map(...))），
  // 长篇（几十集）会瞬间打出几十个请求把 NAS/代理打爆 —— 与本项目 lc-114 图片并发风暴同源。
  // 新逻辑（按规格）：
  //   (1) 已匹配到 Bangumi（bgm 就绪）→ 用一次 bgm_episodes 的 desc 按 episode_number 对齐回填（单请求）；
  //   (2) 未匹配/尚未匹配（吐槽是懒加载，可能一直没匹配）→ 保留原 item 详情回填，但加 **并发上限 3**（worker 池）；
  //      worker 每轮先看 bgmRef：匹配一到位就退场，把补齐工作交给 (1) 的 desc（不再堆 NAS 请求）。
  useEffect(() => {
    if (!episodes.length) return;
    let alive = true;
    const missing = episodes.filter((ep) => !ep.overview);
    if (!missing.length) return;

    if (bgm?.subject_id) {
      invoke<{ data?: BgmEpisodeRow[] }>("bgm_episodes", {
        id: bgm.subject_id,
        limit: 200,
      })
        .then((r) => {
          if (!alive) return;
          const rows = Array.isArray(r?.data) ? r.data : [];
          if (rows.length) setBgmEps(rows); // [lc-1401] 右栏选集用它拿「第N话 小标题 + 单集简介」
          const byNum = new Map<number, string>();
          for (const e of rows) {
            // Bangumi 集号：sort 是数字序号（ep 是展示号，可能带小数/字母），优先 sort
            const num = Number.isInteger(e?.sort)
              ? (e.sort as number)
              : Number.isInteger(e?.ep)
                ? (e.ep as number)
                : NaN;
            const desc = String(e?.desc ?? "").trim();
            if (Number.isFinite(num) && desc) byNum.set(num, desc);
          }
          if (!byNum.size) return;
          setEpisodes((prev) =>
            prev.map((ep) => {
              if (ep.overview || ep.episode_number == null) return ep;
              const d = byNum.get(ep.episode_number);
              return d ? { ...ep, overview: d } : ep;
            }),
          );
        })
        .catch(() => {});
      return () => {
        alive = false;
      };
    }

    const queue = [...missing];
    const worker = async () => {
      while (alive && queue.length) {
        if (bgmRef.current) return; // (1) 就绪 → legacy 任务立即退场
        const ep = queue.shift()!;
        try {
          const d = await invoke<Record<string, unknown>>("get_item_detail", { guid: ep.guid });
          const ov = pick(d, ["overview"]);
          if (alive && ov) {
            setEpisodes((prev) =>
              prev.map((x) => (x.guid === ep.guid && !x.overview ? { ...x, overview: String(ov) } : x)),
            );
          }
        } catch {
          /* 单集失败忽略（与原 allSettled 语义一致） */
        }
      }
    };
    void Promise.all([worker(), worker(), worker()]); // 并发上限 3
    return () => {
      alive = false;
    };
  }, [episodes.length, bgm?.subject_id]);

  const title = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
  // [lc-1433] 竖版海报：**TMDB 高清优先**（用户要求"直接取 tmdb 的或者 bgm 的高清大图"）。
  //   取值顺序：TMDB images.posters 面积最大的那张（Rust 的 heroPoster，拼 t/p/original）
  //   → 飞牛 posters（数组取最大 / 字符串直取）→ 列表页带来的 base.poster → 空（再兜 BGM 封面）。
  //   为什么换源：飞牛的 posters 常是**单个字符串**（它自己抓的小 webp），选无可选、放大必糊。
  //   图片仍走 fntvimg 代理，**拉了即落盘**，不会因为换档位而重复下载。
  const tmdbHeroPoster = String((tmdb ?? {}).heroPoster ?? "");
  // 档位选 w500 而不是 original：本海报的**显示尺寸只有 132×198**（.dv2-poster），
  //   w500 已是 3.8 倍密度、放大到极限也看不出差别，而 original（2000×3000，实测 ~260KB）
  //   纯属浪费 —— 多出的解码时间与内存都花在把 2000px 缩到 132px 上。
  //   （轮播 hero 那边是满屏宽度，才必须用 original。）
  const poster =
    (tmdbHeroPoster ? tmdbImg(tmdbHeroPoster, "w500") : "") ||
    pickLargestImg((data ?? {}).posters) ||
    (pick(data ?? {}, ["posters", "poster"]) as string) ||
    base.poster ||
    "";
  // [lc-1507] 简介兜底链：飞牛详情 → 飞牛条目自带 → **Bangumi subject.summary**（发现页
  //   Bangumi 条目 TMDB 匹配不上时，简介不至于空白 —— bgmSub 进页面就拉了）
  const bgmSummary = String(pick(bgmSub ?? {}, ["summary"]) ?? "").trim();
  const overview = (() => {
    const a = String(pick(data ?? {}, ["overview"]) ?? "").trim();
    if (a) return a;
    const b = String(base.overview ?? "").trim();
    if (b) return b;
    return bgmSummary;
  })();
  // [lc-1401] 原题名（有就别名不显示）：优先条目详情，其次 Bangumi 匹配结果
  const originalTitle = String(
    pick(data ?? {}, ["originalTitle", "original_title", "originalName"]) ?? bgm?.name ?? "",
  ).trim();
  // [lc-1401] Bangumi 封面（本机没海报时的兜底；走 fntvimg 的 /__bgm 代理，拉了即落盘）
  const bgmCover = bgmImg(
    bgm?.images?.large || bgm?.images?.common || bgm?.images?.medium || bgm?.images?.grid || "",
  );
  const resumeAt = base.ts && base.duration && base.ts > 0 && base.ts < base.duration;

  // [lc-1401] 新版式删掉了通栏头图（backdrop 不再用于详情页背景），
  //   但「剧照」一节仍取 TMDB 的 backdrops —— 那是 tmdb.images.backdrops，与本机条目的 backdrop 不是一回事。
  // [lc-1408] 原「年份 / 季数 / 集数 / 类型 / 评分 / 播放按钮」那组派生值随红框一起删掉了
  //   （用户定稿：那块的原有内容全部不要，改放标签与简介）。播放按钮挪到海报上，仍用 resumeAt。
  const tmdbGenres: string[] = Array.isArray(tmdb?.genres)
    ? (tmdb?.genres as unknown[]).map((x) => String(x))
    : [];
  const genres = Array.from(
    new Set((tmdbGenres.length ? tmdbGenres : pickGenres(data)).filter(Boolean)),
  ).slice(0, 6);
  // [lc-1404] 剧照列表不再在这里派生：它只被 TmdbExtra 内部用（那边自己从 tmdb.backdrops 取），
  //   留着一份"渲染过一次的剧照数组"正是上版**两组剧照**的来源。
  // [lc-128] 演员/制作人员三级来源（**TMDB 主源**）：
  //   (1) tmdb_credits（专用命令，锚点优先 + 落盘缓存 7 天）
  //   (2) tmdb_show_details 里同源的 cast/crew（(1) 失败时兜底，已在内存里，零额外请求）
  //   (3) 飞牛 item 详情里的 actors/cast（有则用、没有不报错）
  const creditsCast = Array.isArray(credits?.cast) ? (credits?.cast as TmdbCreditPerson[]) : [];
  const creditsCrew = Array.isArray(credits?.crew) ? (credits?.crew as TmdbCreditPerson[]) : [];
  const showCast: TmdbCreditPerson[] = Array.isArray(tmdb?.cast)
    ? (tmdb?.cast as TmdbCreditPerson[])
    : [];
  const showCrew: TmdbCreditPerson[] = Array.isArray(tmdb?.crew)
    ? (tmdb?.crew as TmdbCreditPerson[])
    : [];
  const cast = creditsCast.length ? creditsCast : showCast;
  const crew = creditsCrew.length ? creditsCrew : showCrew;
  const castFrom: "credits" | "details" | "none" = creditsCast.length
    ? "credits"
    : showCast.length
      ? "details"
      : "none";
  const fbCast = fnosCast(data);
  // [lc-1401] 分段栏（用户定稿版式：顺序固定）
  //   详情 / 评论 / 吐槽 / 关联 / 角色 / 更多 —— 全部**恒在**：
  //   版式要求标签条是固定的一条（不是按"有没有数据"增删），所以不再自适应增删项；
  //   无数据的分段自己显示空态（各自都有骨架屏/失败重试/空态三态）。
  //   「选集」已移出标签条 —— 按版式常驻右栏（每集小标题 + 简介的竖向列表）。
  //   [lc-1565] 用户改名：详情信息→详情、关联条目→关联、演职人员→角色；新增「更多」页。
  const tabIds: string[] = ["overview", "reviews", "comments", "related", "more", "extra"];
  // [lc-1423] 简介是否需要"展开"：与之前同一套字数近似（>96 字≈4~5 行）
  const longIntro = overview.length > 96;
  // Esc 关简介弹窗（与项目其它弹层同一套键盘约定）
  useEffect(() => {
    if (!introFull) return;
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") setIntroFull(false);
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [introFull]);
  const activeTab = tabIds.includes(tab) ? tab : "overview";
  // [lc-1405] 标签框数据源：**Bangumi tags[] 优先，TMDB genres 兜底**（用户要求"直接取 bgm 或 tmdb 的"）。
  //   ⚠ 不再用飞牛 item 的 tags —— 那是刮削元信息字典（含"话数"这类），不是题材标签，
  //   之前它经 pickGenres 混进来，就把 "13"（话数）当标签显示了（用户反馈的现象）。
  const bgmRawTags: BgmTag[] = Array.isArray((bgmSub as Record<string, unknown> | null)?.tags)
    ? ((bgmSub as Record<string, unknown>).tags as BgmTag[])
    : [];
  // TMDB 的题材（tmdb.genres 已在上面解析成 genres: string[]）；两边都空 → 标签框整块不渲染
  const bgmTags: BgmTag[] = bgmRawTags.length
    ? bgmRawTags
    : genres.map((g) => ({ name: g }));
  // [lc-1570] 评分组件数据：BGM 优先（date / rating.rank·total·score·count），TMDB 兜底（airDate / rating·votes）
  const bgmRating = ((bgmSub as Record<string, unknown> | null)?.rating ?? {}) as Record<string, unknown>;
  const bgmScore = Number(bgmRating.score ?? 0) || 0;
  const bgmTotal = Number(bgmRating.total ?? 0) || 0;
  const bgmRank = Number(bgmRating.rank ?? 0) || 0;
  const bgmCounts = (bgmRating.count ?? null) as Record<string, number> | null;
  const tmdbScore = Number((tmdb as Record<string, unknown> | null)?.rating ?? 0) || 0;
  const tmdbVotes = Number((tmdb as Record<string, unknown> | null)?.votes ?? 0) || 0;
  // [lc-1575] TMDB 编号（字符串，可能为空）：BGM 无排名时，评分组件的 Ranked 块改用它兜底
  const tmdbIdStr = String((tmdb as Record<string, unknown> | null)?.tmdbId ?? "").trim();
  const ratingScore = bgmScore > 0 ? bgmScore : tmdbScore > 0 ? tmdbScore : null;
  const ratingSource: "bgm" | "tmdb" | "none" = bgmScore > 0 ? "bgm" : tmdbScore > 0 ? "tmdb" : "none";
  const ratingTotal = bgmScore > 0 ? bgmTotal : tmdbVotes;
  const airDate =
    String(pick(bgmSub ?? {}, ["date"]) ?? "").trim() || String(pick(tmdb ?? {}, ["airDate"]) ?? "").trim();
  const ratingSubjectId = bgm?.subject_id ?? base.bgm_id ?? null;
  // [lc-1598] 「有观看记录」判据（给评分组件的状态按钮用）：条目级 watched/ts 有值，或任意一集有进度/已看。
  //   为 true 时 RatingWidget 会把「未追 / 想看」默认改成「在看」（用户要求）。
  const hasWatchRecord =
    Number(pick(data ?? {}, ["watched"]) ?? 0) === 1 ||
    Number(pick(data ?? {}, ["ts"]) ?? 0) > 0 ||
    episodes.some((e) => Number(e.watched) === 1 || (e.ts ?? 0) > 0);
  // [lc-1576] 评分透视兜底：**BGM 没有评分分布（rating.count）时改用 TMDB 的分布**（镜像签名路由
  //   /rating-details/...；Rust 侧落盘缓存 24h）。转成与 BGM 同形的 {"1":count,...} 供组件直接用；
  //   拉取失败/空分布保持 null（柱状图照常画空态矮柱）。切换条目（base.guid）时重置。
  const [tmdbHist, setTmdbHist] = useState<Record<string, number> | null>(null);
  // 重置只在切换条目时做一次（ref 认 guid，同 [lc-1573] tmdb 段的理由：依赖里的 bgmSub/tmdb
  // 晚到会重跑本段，每次都清会让已画好的柱形"先清后取"闪一下）。
  const histGuidRef = useRef("");
  useEffect(() => {
    if (histGuidRef.current !== base.guid) {
      histGuidRef.current = base.guid;
      setTmdbHist(null);
    }
    if (bgmScore > 0 && bgmCounts) return; // BGM 有分布 → 用 BGM 的，不请求
    if (!tmdbIdStr) return; // 没定位到 TMDB 条目 → 保持空态
    let alive = true;
    invoke<{ histogram?: { key?: unknown; count?: unknown }[] }>("tmdb_rating_details", {
      id: Number(tmdbIdStr),
      mediaType: base.type === "Movie" ? "movie" : "tv",
    })
      .then((r) => {
        if (!alive) return;
        const hist = Array.isArray(r?.histogram) ? r.histogram : [];
        const m: Record<string, number> = {};
        for (const h of hist) {
          const k = Number(h?.key);
          const c = Number(h?.count);
          if (Number.isInteger(k) && k >= 1 && k <= 10 && Number.isFinite(c)) m[String(k)] = c;
        }
        if (Object.keys(m).length) setTmdbHist(m); // 空分布视为无数据，保持空态
      })
      .catch(() => {
        /* 静默：保持空态（柱状图照常画矮柱） */
      });
    return () => {
      alive = false;
    };
  }, [base.guid, base.type, bgmScore, bgmCounts, tmdbIdStr]);
  // [lc-1401] 关联条目按 relation 分组（上游原序即热度序，分组内不重排）
  const relatedGroups: { relation: string; items: BgmRelated[] }[] = (() => {
    const map = new Map<string, BgmRelated[]>();
    for (const r of related) {
      const k = String(r.relation || "其它").trim() || "其它";
      const arr = map.get(k);
      if (arr) arr.push(r);
      else map.set(k, [r]);
    }
    return [...map.entries()].map(([relation, items]) => ({ relation, items }));
  })();
  // [lc-1419] 选集头部的「集数 / 完结状态 / 上次更新」三项（用户要求）。
  //   数据源与判据：
  //   · 总集数：Bangumi subject 的 eps/total_episodes（实测 13），本机集数作为回退；
  //   · 是否完结：infobox 里**有「放送结束」即视为已完结**（实测已完结条目有该键、在播没有）——
  //     比"集数是否相等"稳（有些番本机只有部分集，拿本机数判会误报"未完结"）；
  //   · 上次更新：本机集里最新的一集播出日期（air_date），没有就回退 Bangumi subject.date。
  const subjectEps = Number(
    (bgmSub as Record<string, unknown> | null)?.eps ??
      (bgmSub as Record<string, unknown> | null)?.total_episodes ??
      0,
  );
  const totalEps = subjectEps > 0 ? subjectEps : episodes.length;
  const infoBox = ((bgmSub as Record<string, unknown> | null)?.infobox ?? []) as {
    key?: string;
    value?: unknown;
  }[];
  const finished = infoBox.some((b) => String(b?.key ?? "").includes("放送结束"));
  const airDates = episodes.map((e) => String(e.air_date ?? "")).filter(Boolean).sort();
  const lastAir = airDates.length ? airDates[airDates.length - 1] : String(pick(bgmSub ?? {}, ["date"]) ?? "");

  // [lc-1419] 续播目标：有播放记录（ts>0 且未看完）就跳那一集并带进度，
  //   否则从第 1 集开始。原「海报上的播放按钮」用的就是 episodes[0]，这里升级为"上次的位置"。
  const resumeEp =
    episodes.find((e) => (e.ts ?? 0) > 0 && !(e.duration && e.ts! >= e.duration)) ?? episodes[0];
  const resumeTarget = resumeEp ?? null;
  // 进度百分比（给按钮上的小字用）
  const resumePct =
    resumeTarget && resumeTarget.ts && resumeTarget.duration && resumeTarget.duration > 0
      ? Math.min(100, Math.round((resumeTarget.ts / resumeTarget.duration) * 100))
      : 0;

  // [lc-1401] 右栏选集：本机集号 → Bangumi 那集的 {小标题, 简介}（集号对齐 sort 优先，与回填逻辑一致）
  const bgmEpByNum = new Map<number, BgmEpisodeRow>();
  for (const e of bgmEps) {
    const num = Number.isInteger(e?.sort) ? (e.sort as number) : Number.isInteger(e?.ep) ? (e.ep as number) : NaN;
    if (Number.isFinite(num) && !bgmEpByNum.has(num)) bgmEpByNum.set(num, e);
  }

  // [lc-1597] 播放结束后：把**本地进度 ≥80%**（或飞牛已标 watched）的集在 BGM 上标「看过」。
  //   对齐原 Electron 项目 bangumiSync.ts：`PUT /v0/users/-/collections/-/episodes/{id}` body `{type:2}`。
  //   前提：① 已绑定 BGM ② 条目已匹配到 BGM（才有 episode_id）。上游还要求**条目已在收藏里**，
  //   否则 400 `subject not collected` —— 静默失败（不打扰用户，也**不替用户改收藏状态**）。
  //   条件不满足时**不消费标志**（等 BGM 匹配/集列表晚到后再跑）。
  useEffect(() => {
    if (!syncBgmRef.current) return;
    const sid = Number((bgm as Record<string, unknown> | null)?.subject_id ?? 0);
    if (!sid || !episodes.length) return;
    syncBgmRef.current = false;
    for (const ep of episodes) {
      const progress =
        ep.ts && ep.duration && ep.duration > 0 ? Math.min(100, (ep.ts / ep.duration) * 100) : 0;
      if (!(Number(ep.watched) === 1 || progress >= 80)) continue;
      const num = Number(ep.episode_number ?? 0);
      const id = Number(num > 0 ? bgmEpByNum.get(num)?.id ?? 0 : 0);
      if (id > 0) invoke("bgm_episode_watched", { episodeId: id }).catch(() => {});
    }
  }, [episodes, bgm]);


  // [lc-106] 独立 mpv 窗口模式：点播放直接拉起独立 mpv（与主仓库一致），页面留在详情页
  const [playNote, setPlayNote] = useState("");
  const [playing, setPlaying] = useState(false);
  const [playingGuid, setPlayingGuid] = useState<string | null>(null);
  const startPlay = async () => {
    const ep = episodes[0];
    const guid = isSeries && ep ? ep.guid : base.guid;
    setPlaying(true);
    setPlayNote("正在打开独立播放器…");
    try {
      await invoke("play_item", { guid });
      setPlayNote("已在独立播放器窗口播放（可回到本页继续浏览）");
    } catch (e) {
      setPlayNote(String(e));
    } finally {
      setPlaying(false);
    }
  };

  return (
    <main className="content detail-full" onScroll={onDetailScroll}>
      {/* [lc-1401] 详情页按**用户定稿版式**重排（两栏）：
            左列：竖版封面 → 标签页 → 该页内容（[lc-1567] 标签/简介从首屏移入「详情」页）
            右列：集数据（本机集数 × Bangumi 集号对齐）的**整条竖向列表**
          ⚠ 上一版的模糊大头图（.dhero）**已按新稿删除**：稿里首屏就是"封面 + 两块信息框"，没有通栏头图。
          数据 effect 全部留在本组件顶层（不下沉到面板里，否则单集简介回填失效）。 */}
      <div className="dv2-rail">
        <button className="back" onClick={onBack} title="返回上一页">
          ‹ 返回
        </button>
        {/* [lc-1403] 标题挪到返回键后面（用户要求）；原名（日文/英文）放标题后的括号里。
            原名优先 TMDB 的 original_name/original_title，其次 Bangumi 的 name —— 与当前显示名
            完全相同（或为空）时不显示括号，避免出现「进击的巨人（进击的巨人）」。 */}
        <h1 className="dv2-rail-title">
          {title || "（无标题）"}
          {originalTitle && originalTitle !== title && (
            <span className="dv2-rail-orig">（{originalTitle}）</span>
          )}
        </h1>
        {/* [lc-1629] 条目级「元数据」入口（对齐飞牛网页端条目的右键菜单）：
            编辑元数据 / 手动匹配影片 / 解除匹配 / 刷新元数据。无飞牛条目（发现页）时不显示。 */}
        {!noFnOS && (
          <button
            className="dv2-meta-btn"
            title="元数据操作（编辑 / 手动匹配 / 解除匹配 / 刷新）"
            onClick={() =>
              setMetaTarget({ guid: base.guid, title: title || base.title, type: isSeries ? "TV" : "Movie" })
            }
          >
            ⋯
          </button>
        )}
      </div>

      {err && <div className="err">{err}</div>}

      {/* [lc-1417] 首屏骨架屏：条目详情（data）没到位之前先摆骨架，别让用户对着空白等。
          判据用 `!dataReady && !err`：出错时走上面的 .err，已有数据时立即换成真内容。
          [lc-1444] 发现页条目没有飞牛详情，dataReady 立即为真 —— 不等、不摆骨架。
          ⚠ 只包住"两栏主体"—— 返回键与标题（rail）是从 base 直接来的，不需要等网络。 */}
      {!dataReady && !err && <DetailSkeleton />}

      {/* ══ 两栏主体（用户版式）══ */}
      {dataReady && (
      <div className="dv2">
        {/* ── 左列：封面 / 标签框 / 简介框 / 标签页 / 内容 ── */}
        <div className="dv2-left">
          {/* 竖版封面 + 标题（海报来自本机条目，无则用 Bangumi 的） */}
          <div className="dv2-top">
            <div className="dv2-poster">
              {poster || bgmCover ? (
                <img src={poster ? imgUrl(poster) : bgmCover} alt={title} draggable={false} />
              ) : (
                <div className="poster-empty"><IcoFilmL size={22} strokeWidth={1.8} /></div>
              )}
              {/* [lc-1420] 海报上的播放按钮已删（用户要求）。
                  播放入口现在统一在**选集头部**（剧集），或右栏为空时的下方按钮（电影）——
                  见 aside 里的 .dv2e-play-btn / .dv2-solo-play。 */}
            </div>

            {/* [lc-1567] 标签与简介已移入「详情」页（用户要求：标签→简介→剧照，同一容器、浅线分隔）。
                [lc-1570] 海报右侧空位挂「评分组件」（用户设计稿接入：放送开始/评分/Ranked/收藏/评分透视）。 */}
            <div className="dv2-topinfo">
              <RatingWidget
                subjectId={ratingSubjectId}
                date={airDate}
                score={ratingScore}
                scoreSource={ratingSource}
                total={ratingTotal}
                rank={bgmRank > 0 ? bgmRank : null}
                tmdbId={tmdbIdStr}
                counts={bgmScore > 0 && bgmCounts ? bgmCounts : tmdbHist}
                countsSource={bgmScore > 0 && bgmCounts ? "bgm" : tmdbHist ? "tmdb" : "none"}
                autoWatching={hasWatchRecord}
                /* [lc-1651] 无 BGM 条目时按钮变「收藏」：这里给足收藏卡片渲染字段 */
                favItem={{
                  guid: base.guid,
                  title:
                    String(pick(data ?? {}, ["tv_title", "tvTitle"]) ?? "").trim() ||
                    String(pick(data ?? {}, ["title"]) ?? base.title ?? ""),
                  tvTitle: String(pick(data ?? {}, ["tv_title", "tvTitle"]) ?? "").trim(),
                  poster: String(poster ?? ""),
                  type: String(pick(data ?? {}, ["type"]) ?? base.type ?? ""),
                  seasonNumber: Number(pick(data ?? {}, ["season_number", "seasonNumber"]) ?? 0) || 0,
                  episodeNumber: Number(pick(data ?? {}, ["episode_number", "episodeNumber"]) ?? 0) || 0,
                }}
              />
              {playNote && <div className="play-note">{playNote}</div>}
            </div>
          </div>

          {/* 四标签页（固定一条，不随数据增删） */}
          <nav className="dseg dv2-seg" role="tablist">
            {tabIds.map((id) => (
              <button
                key={id}
                role="tab"
                aria-selected={activeTab === id}
                className={`dseg-btn${activeTab === id ? " active" : ""}`}
                onClick={() => setTab(id)}
              >
                {TAB_LABEL[id]}
              </button>
            ))}
          </nav>

          {/* 标签页内容 */}
          <div className={`dbody dv2-body${tabAnim ? " view-in" : ""}`}>
        {activeTab === "overview" && (
          <OverviewTab
            bgmTags={bgmTags}
            overview={overview}
            longIntro={longIntro}
            introFull={introFull}
            onExpand={() => setIntroFull(true)}
            setZoom={setZoom}
            tmdb={tmdb}
            tmdbDone={tmdbDone}
          />
        )}

        {/* [lc-1401] 关联条目（Bangumi /subjects/{id}/subjects）：**按 relation 分组**渲染
            （上游没有分组，只有每条的 relation 词）。组序 = 上游首次出现顺序（热度序）。 */}
        {activeTab === "related" && (
          <RelatedTab
            related={related}
            relatedGroups={relatedGroups}
            relatedBusy={relatedBusy}
            relatedErr={relatedErr}
            retryRelated={retryRelated}
          />
        )}

        {/* 演职人员：**TMDB 主源**（tmdb_credits 落盘缓存）→ Bangumi 角色/制作人员 → 飞牛演员字段兜底。
            [lc-1404] 本页**不含** TmdbExtra（分级/在线观看/相似/别名那些属于「剧照剧集详情信息」）——
            用户反馈"演职人员下面又接了一个 tmdb 数据卡"，就是它原来挂在这里造成的重复。 */}
        {activeTab === "more" && (
          <CastTab
            cast={cast}
            castFrom={castFrom}
            showCast={showCast}
            chars={chars}
            credits={credits}
            creditsBusy={creditsBusy}
            creditsCast={creditsCast}
            creditsErr={creditsErr}
            crew={crew}
            fbCast={fbCast}
            moreBusy={moreBusy}
            moreErr={moreErr}
            moreProbed={moreProbed}
            persons={persons}
            retryCredits={retryCredits}
            retryMore={retryMore}
          />

        )}

        {/* [lc-1565] 「更多」页：原「详情」页底部的 更多 块（别名/关键词/热度/编号/链接）独立成页 */}
        {activeTab === "extra" && <ExtraTab tmdb={tmdb} tmdbDone={tmdbDone} />}

        {/* [lc-1412] 评论（长影评，用户设计稿版式）：
              封面 + 标题（链接色）+ 摘要（3 行截断）+ 作者 · 日期 · N 回复。
              数据源 /p1/.../reviews；封面在 entry.icon —— **多数是 no_photo.png 占位图**，
              所以碰到 no_photo 就退成图标（否则整列都是灰方块，很难看）。 */}
        {activeTab === "reviews" && (
          <ReviewsTab
            loadMoreReviews={loadMoreReviews}
            retryReviews={retryReviews}
            reviews={reviews}
            reviewsBusy={reviewsBusy}
            reviewsDone={reviewsDone}
            reviewsErr={reviewsErr}
            reviewsMoreBusy={reviewsMoreBusy}
          />
        )}

        {/* [lc-119] 吐槽（Bangumi 短评，走自建镜像）：骨架屏 / 失败重试 / 空态 / 滚到底翻页
            [lc-1412] 版式按用户设计稿改成**气泡流**（原为头像+昵称+正文的列表行）：
            每条是一张带边框的气泡卡，右上角心形（reactions 有数据时点亮并显示人数），
            昵称/星级/时间一行在气泡上方。
            ⚠ **做不出嵌套回复**：Bangumi 的 /comments 不提供回复链（50 条样本无 parent 字段，
              parent_id 参数无效、三个候选回复端点全 404），所以这里只有平铺的评论流。 */}
        {activeTab === "comments" && (
          <CommentsTab
            comments={comments}
            commentsBusy={commentsBusy}
            commentsDone={commentsDone}
            commentsErr={commentsErr}
            commentsMoreBusy={commentsMoreBusy}
            retryComments={retryComments}
          />

        )}
          </div>
        </div>

        {/* ── 右列：集数据（整条竖向列表）──
            每集 = 集号徽标 + 小标题（本机集名 / Bangumi name_cn 兜底）+ 该集简介 + 时长。
            「本机集数 × Bangumi 集号」对齐：本机集按 episode_number 取 Bangumi 同号那条的
            小标题与 desc（Bangumi 只有一季时 ep 是**全季连号**，所以按号取，不按季内序号）。 */}
        <aside className="dv2-right">
          {/* [lc-1420] 非剧集（电影等）没有选集可点，海报上的播放按钮又已删 —— 
              这里给一个独立的播放入口，避免"能看不能播"。
              [lc-1444] 发现页条目没有飞牛 guid，播不了 —— 不显示播放入口。 */}
          {!isSeries && !noFnOS && (
            <button
              className="dv2-solo-play"
              disabled={playing}
              onClick={playing ? undefined : startPlay}
            >
              <IcoPlayL size={16} strokeWidth={2.4} />
              <span>{playing ? "打开中…" : resumeAt ? "继续播放" : "播放"}</span>
            </button>
          )}
          {/* [lc-1421] 头部改成**一整行**（用户要求："做成一排显示，播放按钮在左边"）。
              顺序 = 播放按钮 → 选集标题 → 集数/完结/更新 → 季切换。
              原来按钮独占右侧、标题与信息在上面一行，一窄就分成两行（用户截图就是那样）。
              `.dv2e-hd` 用 flex + nowrap + 允许横向溢出，保证"一排"；窄到实在放不下时才换行。 */}
          <div className="dv2e-hd">
            {/* 播放入口（原来在右侧，现移到最左）。语义：
                有播放记录 → 「观看至 第 N 集 · N%」（[lc-1424] 文案由"继续观看"改为"观看至"：
                更贴合"已经看到第 N 集的第 N%"这个事实）；没有记录 → 「播放 第 1 集」。 */}
            {isSeries && resumeTarget && (
              <button
                className="dv2e-play-btn"
                disabled={playing}
                onClick={() => {
                  setPlayingGuid(resumeTarget.guid);
                  invoke("play_item", { guid: resumeTarget.guid }).catch((e) => setErr(String(e)));
                }}
                title={
                  resumePct > 0
                    ? `从第 ${resumeTarget.episode_number ?? 1} 集的 ${resumePct}% 继续播放`
                    : `播放 第 ${resumeTarget.episode_number ?? 1} 集`
                }
              >
                <IcoPlayL size={14} strokeWidth={2.4} />
                <span>
                  {playing
                    ? "打开中…"
                    : resumePct > 0
                      ? `观看至 第 ${resumeTarget.episode_number ?? 1} 集`
                      : `播放 第 ${resumeTarget.episode_number ?? 1} 集`}
                </span>
                {resumePct > 0 && <i className="dv2e-play-pct">{resumePct}%</i>}
              </button>
            )}
            <h3 className="dv2e-hd-h">选集</h3>
            {/* [lc-1419] 集数/完结/上次更新（"一眼确认状态"） */}
            {isSeries && (
              <span className="dv2e-meta">
                <span className="dv2e-meta-n">
                  {/* [lc-1444] 发现页条目没有本机集数：显示 Bangumi 总集数（若有），否则 "—" */}
                  {noFnOS
                    ? totalEps > 0
                      ? totalEps
                      : "—"
                    : `${episodes.length}${totalEps > 0 && totalEps !== episodes.length ? ` / ${totalEps}` : ""}`}{" "}
                  集
                </span>
                <span className={`dv2e-state${finished ? " done" : " airing"}`}>
                  {finished ? "已完结" : "连载中"}
                </span>
                {lastAir && <span className="dv2e-meta-d">更新至 {String(lastAir).slice(0, 10)}</span>}
              </span>
            )}
            {seasons.length > 1 && (
              <div className="season-tabs">
                {seasons.map((sn, i) => (
                  <button
                    key={sn.guid}
                    className={`season-tab ${i === seasonIdx ? "active" : ""}`}
                    onClick={() => setSeasonIdx(i)}
                  >
                    {sn.title || `第 ${sn.season_number ?? i + 1} 季`}
                  </button>
                ))}
              </div>
            )}
          </div>
          <div className="dv2e-list">
            {!isSeries ? (
              <div className="empty">本条目不是剧集</div>
            ) : episodes.length === 0 && seasons.length === 0 && !noFnOS ? (
              /* [lc-1420] **分块骨架**：条目详情（data）到得很快，所以整页骨架一闪就没了；
                 但季/集是另外两条请求 —— 它们没回来之前右栏是空的，用户看着就是"还在等数据"。
                 这里单独给右栏摆骨架（用真实行高，数据到位时不跳）。 */
              [0, 1, 2, 3].map((i) => (
                <div className="dv2e-row" key={`sk${i}`}>
                  <div className="dv2e-thumb"><div className="dsk dsk-dthumb" /></div>
                  <div className="dv2e-main">
                    <div className="dsk dsk-sline" />
                    <div className={`dsk dsk-sline${i % 2 ? " short" : ""}`} />
                    <div className="dsk dsk-sline short" />
                  </div>
                </div>
              ))
            ) : episodes.length === 0 ? (
              /* [lc-1444] 发现页条目没有飞牛选集数据。
                 [lc-1627] 用户要求：**在线播放先搁置** —— 无源条目不再挂在线资源面板，
                 直接显示「未查询到资源」。（WebrsPanel 代码保留，恢复时把下面换回
                 `<WebrsPanel title={title || base.title} />` 并放开文件头部的 import 即可。） */
              noFnOS ? (
                <div className="empty">未查询到资源</div>
              ) : (
                <div className="empty">暂无剧集</div>
              )
            ) : (
              episodes.map((ep, i) => {
                const num = ep.episode_number ?? i + 1;
                const bgmEp = bgmEpByNum.get(num);
                const sub = String(bgmEp?.name_cn || bgmEp?.name || "").trim();
                const desc = String(ep.overview || bgmEp?.desc || "").trim();
                const progress =
                  ep.ts && ep.duration && ep.duration > 0 ? Math.min(100, (ep.ts / ep.duration) * 100) : 0;
                // [lc-1596] 「已看」门槛：本地进度 **≥80%** 即视为已观看（原为 95% —— 用户要求
                //   「观看进度超过 80 后打上已观看的标记」）。飞牛自己的 `watched=1` 仍然优先。
                const seen = Number(ep.watched) === 1 || progress >= 80;
                const playing = playingGuid === ep.guid;
                return (
                  <div
                    key={ep.guid}
                    className={`dv2e-row${playing ? " playing" : ""}${seen ? " seen" : ""}`}
                    onClick={() => {
                      setPlayingGuid(ep.guid);
                      invoke("play_item", { guid: ep.guid }).catch((e) => setErr(String(e)));
                    }}
                    title={`播放 第 ${num} 集`}
                  >
                    <div className="dv2e-thumb">
                      {ep.poster ? (
                        <img src={imgUrl(ep.poster)} loading="lazy" alt="" draggable={false} />
                      ) : (
                        <div className="poster-empty">
                          <IcoClapL size={18} strokeWidth={1.8} />
                        </div>
                      )}
                      <span className="dv2e-play">{playing ? "◉" : "▶"}</span>
                      {/* [lc-1629] 每集「元数据」入口（对齐飞牛网页端每集海报的右键菜单）：
                          编辑元数据 / 手动匹配影片 / 解除匹配 / 刷新元数据。stopPropagation 防止误触播放。 */}
                      <button
                        className="dv2e-meta-btn"
                        title="本集元数据（编辑 / 手动匹配 / 解除匹配 / 刷新）"
                        onClick={(e) => {
                          e.stopPropagation();
                          setMetaTarget({
                            guid: ep.guid,
                            title: `第 ${num} 集${ep.title ? `. ${ep.title}` : sub ? `. ${sub}` : ""}`,
                            type: "TV",
                            seasonNumber: ep.season_number,
                            episodeNumber: ep.episode_number ?? num,
                            isEpisode: true,
                          });
                        }}
                      >
                        ⋯
                      </button>
                      {seen && <span className="ep-mark seen"><IcoCheckL size={11} strokeWidth={2.2} /> 已看</span>}
                      {!seen && progress > 0 && <span className="ep-mark now">看到 {Math.round(progress)}%</span>}
                      {progress > 0 && (
                        <div className="progress">
                          <div style={{ width: `${progress}%` }} />
                        </div>
                      )}
                    </div>
                    <div className="dv2e-main">
                      <div className="dv2e-t">
                        <span className="dv2e-name">第 {num} 集{ep.title ? `. ${ep.title}` : sub ? `. ${sub}` : ""}</span>
                        {ep.quality && <span className="ep-quality">{ep.quality}</span>}
                      </div>
                      {desc && <p className="dv2e-desc">{desc}</p>}
                      {fmtDur(ep.duration) && <div className="dv2e-dur">{fmtDur(ep.duration)}</div>}
                    </div>
                  </div>
                );
              })
            )}
          </div>
        </aside>
      </div>
      )}

      {/* [lc-1423] 简介弹窗：完整正文（容器固定高度 + 截断后，全文在这里看）。
          Esc / 点背景关闭；正文可滚动（长简介比一屏更长）。 */}
      {introFull &&
        createPortal(
          <div className="ddet-mask" onClick={() => setIntroFull(false)}>
          <div className="sum-modal" onClick={(e) => e.stopPropagation()}>
            <div className="sum-modal-h">
              <h3>简介</h3>
              <button
                className="ddet-close sum-modal-x"
                title="关闭（Esc）"
                onClick={() => setIntroFull(false)}
              >
                <IcoXL size={15} strokeWidth={1.8} />
              </button>
            </div>
            <div className="sum-modal-b">
              <p className="dfold">{overview}</p>
            </div>
          </div>
        </div>,
          document.body,
        )}

      {/* [lc-128] 剧照原图（`original` 尺寸）：点击任意剧照放大。走同一套三级缓存
          （拉了即落盘，下次点开零网络），点背景/Esc… 这里只需点背景关闭。 */}
      {zoom &&
        createPortal(
          <div className="ddet-mask zoom-mask" onClick={() => setZoom("")}>
            <img className="zoom-img" src={tmdbImg(zoom, "original")} alt="" />
          </div>,
          document.body,
        )}

      {/* [lc-1629] 飞牛每集/条目自定义元数据操作弹层（编辑 / 手动匹配 / 解除匹配 / 刷新）。
          写成功后 onDone → refreshAll()：带 force 重取详情/季/集，立刻反映飞牛侧的新数据。 */}
      {metaTarget && (
        <MetaEditModal
          target={metaTarget}
          onClose={() => setMetaTarget(null)}
          onDone={() => refreshAll()}
        />
      )}
    </main>
  );
}


/* ────────────────────────── TMDB 扩展信息（剧照） ──────────────────────────
   剧照/头像走 fntvimg 代理的 TMDB 图片域（Rust 反代镜像 + **落盘三级缓存**，见 tmdbImg() 与 imgproto.rs）。
   [lc-128] 演职人员一节搬去「角色」页（TMDB 主源 + 落盘缓存 + 飞牛兜底），这里不再重复渲染。
   [lc-1565] 「更多」块（别名/关键词/热度/编号/链接）独立成「更多」页（tabs/ExtraTab）。
   [lc-1566] 「相似剧集」也移入「更多」页 —— 本段只剩「剧照」（详情页）。
   [lc-1413] 按用户要求**删掉「预告片」与「分级 / 在线观看」两节**：
     前者是 YouTube 外链（国内打不开，点了也是白点），后者对国内用户基本是空数据；
     连带删掉 TmdbRegions 组件、videos 派生变量与相关类型用法。 */
