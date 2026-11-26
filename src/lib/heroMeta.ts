/* [lc-2390] 首页 hero 标签行（放映厅样式）：年份 / 标签 / 状态 / 集数 / 评分。
 *
 * 为什么单独成文件：这五项各有各的口径陷阱，散在组件里必然写错
 * （详见下面 TMDB_STATUS / episodes 的注释）。做成纯函数才能离线断言，
 * 不必起浏览器、也不必等真机拉 TMDB。
 *
 * 数据来源两层（与 lc-2385 的元数据口径一致）：
 *   · 飞牛条目本体（MediaItem）：年份、评分 —— 首屏立刻就有，不留空；
 *   · 我们自建 TMDB 的 `tmdb_show_details`：题材、状态、已播集数 —— 拉到后覆盖。
 * 也就是说 TMDB 未到/失败时，标签行退化成「年份 · 电影/剧集 · 评分」，绝不空着。
 */

import type { MediaItem } from "../types";

/** 标签行要展示的五项（都可缺省：缺的那项直接不出现在行里）。 */
export type HeroMeta = {
  year?: string;
  /** 题材标签：最多 2 个（TMDB 中文名），由调用方截断 */
  genres?: string[];
  /** 状态文案（已完结 / 连载中 / 已上映 …） */
  status?: string;
  /** 集数文案（全 12 集 / 更新至 7 集） */
  episodes?: string;
  /** 评分（1 位小数，不含星号） */
  rating?: string;
  /** 兜底类型文案（电影 / 剧集）：TMDB 题材拿不到时补位，不留空洞 */
  kind?: string;
};

/** TMDB 剧集状态 → 中文。**只分「已完结 / 连载中」两类**（用户明确要求）：
 *  Ended / Canceled 都算已完结（被砍的剧同样不会再更新，对用户是同一件事）；
 *  其余（Returning Series / In Production / Planned / Pilot）一律连载中 ——
 *  未开播的剧显示「连载中」比显示「更新至 0 集」诚实，且不占集数标签。 */
const TV_STATUS: Record<string, string> = {
  Ended: "已完结",
  Canceled: "已完结",
  "Returning Series": "连载中",
  "In Production": "连载中",
  Planned: "连载中",
  Pilot: "连载中",
};

/** TMDB 电影状态 → 中文（与详情页 MovieHeroEditorial 的 STATUS_CN 同口径）。 */
const MOVIE_STATUS: Record<string, string> = {
  Released: "已上映",
  "Post Production": "后期制作",
  "In Production": "制作中",
  Planned: "计划中",
  Rumored: "传闻中",
  Canceled: "已取消",
};

/** 题材标签最多展示几个：再多就把标签行挤成两行、压掉简介的位置。 */
export const MAX_GENRES = 2;

const str = (v: unknown): string => (v == null ? "" : String(v).trim());
const num = (v: unknown): number => {
  const n = Number(v);
  return Number.isFinite(n) ? n : 0;
};

/**
 * TMDB `tmdb_show_details` 载荷 → 标签行的补充数据（纯函数，零副作用）。
 *
 * 集数口径（这里是最容易写错的地方，两条都踩过）：
 *   · **已完结** → `全 N 集`，N 取 `episodes`（number_of_episodes，全剧总集数）；
 *   · **连载中** → `更新至 N 集`，N 取 `airedEpisodes`（Rust 侧按
 *     `last_episode_to_air` 累加前面各季算出，见 tmdb.rs::aired_episodes）。
 *   绝不能拿 number_of_episodes 当「已播」用 —— 连载中会把没播的说成已播，
 *   与「更新至」的语义正好相反；也不能只取 last_episode_to_air.episode_number，
 *   那是**本季**集号，多季剧会少一整季。
 *
 * 拿不到就说拿不到：`airedEpisodes = 0`（未开播/字段缺失）时**不产出集数文案**，
 * 宁可这项不显示，也不显示「更新至 0 集」。
 */
export function tmdbHeroMeta(d: Record<string, unknown> | null | undefined): HeroMeta {
  if (!d) return {};
  const isMovie = str(d.mediaType) === "movie";
  const out: HeroMeta = {};

  const year = str(d.year);
  if (/^\d{4}$/.test(year)) out.year = year;

  const genres = Array.isArray(d.genres)
    ? d.genres.map((g) => str(g)).filter(Boolean).slice(0, MAX_GENRES)
    : [];
  if (genres.length) out.genres = genres;

  const status =
    (isMovie ? MOVIE_STATUS : TV_STATUS)[str(d.status)] ?? "";
  if (status) out.status = status;

  const total = num(d.episodes);
  const aired = num(d.airedEpisodes);
  if (!isMovie) {
    if (status === "已完结" && total > 0) out.episodes = `全 ${total} 集`;
    else if (status === "连载中" && aired > 0) out.episodes = `更新至 ${aired} 集`;
  }

  const rating = num(d.rating);
  if (rating > 0) out.rating = rating.toFixed(1);

  return out;
}

/**
 * 合成最终展示值：飞牛条目本体打底，TMDB 那份覆盖（缺的字段各自回落）。
 *
 * 为什么打底用飞牛：TMDB 请求是异步的（且可能失败/被限流），首屏不能等它 ——
 * 年份与评分飞牛本来就有，先用着；题材/状态/集数只有 TMDB 有，到了再补。
 */
export function resolveHeroMeta(it: MediaItem, patch?: HeroMeta): HeroMeta {
  const isMovie = (it.type || "").toLowerCase() === "movie";
  const out: HeroMeta = { kind: isMovie ? "电影" : "剧集" };

  const year = patch?.year || str(it.release_date || it.air_date).slice(0, 4);
  if (/^\d{4}$/.test(year)) out.year = year;

  if (patch?.genres?.length) out.genres = patch.genres.slice(0, MAX_GENRES);
  if (patch?.status) out.status = patch.status;
  if (patch?.episodes) out.episodes = patch.episodes;

  const raw = str(it.vote_average);
  const rating =
    patch?.rating || (raw && raw !== "0" && num(raw) > 0 ? num(raw).toFixed(1) : "");
  if (rating) out.rating = rating;

  return out;
}

/** 标签行的渲染段：`text` 是文案，`kind` 决定着色（评分单独用金色）。 */
export type HeroMetaSegment = { text: string; kind: "plain" | "rating" };

/**
 * 按用户指定顺序拼出标签行：**年份 → 标签 → 状态 → 集数 → 评分**。
 * 标签为多个题材时逐个成段（圆点分隔），与其余项同一套排版。
 * 题材为空时用「电影/剧集」补位 —— 那一格不留空洞。
 */
export function heroMetaSegments(m: HeroMeta): HeroMetaSegment[] {
  const out: HeroMetaSegment[] = [];
  if (m.year) out.push({ text: m.year, kind: "plain" });
  if (m.genres?.length) {
    for (const g of m.genres) out.push({ text: g, kind: "plain" });
  } else if (m.kind) {
    out.push({ text: m.kind, kind: "plain" });
  }
  if (m.status) out.push({ text: m.status, kind: "plain" });
  if (m.episodes) out.push({ text: m.episodes, kind: "plain" });
  if (m.rating) out.push({ text: `★ ${m.rating}`, kind: "rating" });
  return out;
}
