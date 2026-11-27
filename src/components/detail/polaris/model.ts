/* [lc-2396] Polaris 详情页的入参模型（纯数据描述，不含任何请求）。
 *
 * 为什么要单独立一个 model：
 *   `DetailView.tsx` 里所有取数/状态都在顶层（季、集、版本、BGM、TMDB、进度…），
 *   Polaris 版式只是**换一套渲染**，绝不能自己再发一遍请求（会双倍打 NAS，
 *   也会出现「两个版式看到的进度不一样」）。所以这里只声明"要画什么"，
 *   由 DetailView 用已有派生值组装，组件本身保持无副作用。
 *
 * 与 `MovieHeroEditorial` 的分工完全一样（它也是纯展示）：
 *   切换版式不丢数据、不额外请求，只换 DOM 结构与类名。
 */
import type { ReactNode } from "react";
import type {
  BgmTag,
  Episode,
  MediaItem,
  MediaSpecs,
  MediaVersion,
  PersonTarget,
  Season,
  TmdbCreditPerson,
} from "../../../types";
import type { TmdbRecommendation } from "../tabs/RelatedTab";

/** 一部作品在 Polaris 版式下需要的全部展示数据。 */
export type PolarisModel = {
  /* ── 条目基本信息 ── */
  base: MediaItem;
  title: string;
  originalTitle: string;
  isSeries: boolean;
  isMovie: boolean;
  /** 发现页占位条目（无飞牛 guid）：没有本地资源，播放入口要让位给在线搜索 */
  noFnOS: boolean;

  /* ── 首屏视觉 ── */
  /** 满屏背景图（已是完整 URL；空则纯黑） */
  backdrop: string;
  /** 竖版海报（首屏不渲染海报卡 —— Polaris 5.2.8 的 poster-container-3d 是 display:none，
   *  只在剧集卡/相关推荐的图上用到） */
  poster: string;
  /** 标题 Logo（TMDB images.logos；空则用文字标题）。两版用于深浅主题。 */
  logoLight: string;
  logoDark: string;
  rating: number | null;
  votes: number;
  year: string;
  runtimeMin: number;
  genres: string[];
  overview: string;
  /** 首播/上映日期（YYYY-MM-DD，可空） */
  releaseDate: string;

  /* ── 剧集（isSeries 时有效）── */
  seasons: Season[];
  seasonIdx: number;
  episodes: Episode[];
  epsLoading: boolean;
  /** 换季（Polaris 的季选择胶囊） */
  onSeasonChange: (idx: number) => void;
  /** 当前季的集数合计（「已入库 N / M 集」的 M） */
  seasonLibraryTotal: number;
  seriesFinished: boolean;
  lastAir: string;
  seasonEpInfo: { text: string; title: string } | null;

  /* ── 播放与选集 ── */
  playing: boolean;
  playingGuid: string | null;
  /** 首屏播放药丸（剧集=续播目标集 / 电影=条目本身） */
  onPlay: () => void;
  /** 点某一集（含多版本选择器链路） */
  onPlayEpisode: (guid: string) => void;
  /** 续播目标集（用于按钮副标题「第 3 集 12:34」与卡片「正在看」标） */
  resumeTarget: Episode | null;
  resumePct: number;
  /** 已看判定（含手动覆盖/localWatched，与经典版同一口径） */
  isEpSeen: (ep: Episode) => boolean;
  /** 手动切换某集「已看」 */
  onToggleEpWatched: (ep: Episode, current: boolean) => void;
  /** 单集缩略图（含 TMDB 剧照兜底链，DetailView 已算好） */
  epThumb: (ep: Episode, num: number) => string;
  /** 单集简介（飞牛 ep.overview → Bangumi 该集 desc 兜底，DetailView 已按集号对齐算好）。
   *  卡片图下方标题下面的那一段，空则整段不渲染。 */
  epDesc: (ep: Episode, num: number) => string;

  /* ── 版本 ── */
  versions: MediaVersion[];
  versionsGuid: string;
  selectedMediaGuid: string;
  onVersionChange: (mediaGuid: string) => void;
  /** 当前选中版本的完整路径（空则不渲染该行） */
  filePath: string;
  /** 当前版本的技术规格（音频流/字幕数/容器） */
  specs: MediaSpecs | null;

  /* ── 次级内容 ── */
  /** 剧照（TMDB file_path，前端自己拼 size） */
  stills: string[];
  cast: TmdbCreditPerson[];
  /** 主创（导演/编剧/音乐…） */
  crew: TmdbCreditPerson[];
  /** TMDB 相似推荐（关联区的卡片来源） */
  recommendations: TmdbRecommendation[];
  /** Bangumi 主题标签 */
  tags: BgmTag[];

  /* ── 标签页（沿用 DetailView 的七个面板，不重写）── */
  tabsNav: ReactNode;
  tabsBody: ReactNode;
  activeTab: string;

  /* ── 回调 ── */
  onBack: () => void;
  onOpenPerson?: (t: PersonTarget) => void;
  onOpenTmdb?: (tmdbId: number, title: string, year: string, mediaType: "tv" | "movie") => void;
  /** 条目级元数据操作（编辑 / 手动匹配 / 解除匹配 / 刷新） */
  onMeta?: () => void;
  /** 单集元数据操作 */
  onEpMeta?: (ep: Episode, num: number) => void;
  /** 下载字幕 */
  onSubtitle?: () => void;
  /** 收藏按钮的条目信息 */
  favItem: Record<string, unknown> | null;
};
