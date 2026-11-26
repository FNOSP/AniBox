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
 * ⚠ [lc-2390 修正] **状态与集数以飞牛为准，TMDB 只补题材**（见 resolveHeroMeta）。
 *   原因（用户实测报障）：TMDB 的剧集进度是社区投稿维护，**会滞后**。实测
 *   「冷淡的佐藤同学」TMDB 只到 S1E2（2026-10-13），而飞牛库里已有 7 集、最新 11-18；
 *   拿 TMDB 算「更新至」会比详情页的「已入库 7 / 12 集」少 5 集，用户一眼就看出对不上。
 *   所以本函数产出的 status/episodes/rating 现在只作**兜底**（飞牛字段缺失时才用）。
 *
 * TMDB 侧的集数口径（兜底时用）：
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
 * 飞牛条目本体 → 状态与集数（**首选口径**，与详情页「已入库 X / Y 集」同源）。
 *
 * 为什么以飞牛为准（用户实测报障）：英雄标签原来用 TMDB 的 `last_episode_to_air`
 * 算「更新至」，而 TMDB 的剧集进度是社区投稿、**会滞后**。实测「冷淡的佐藤同学」
 * TMDB 只到 S1E2（10-13），飞牛库里已有 7 集（最新 11-18）—— 于是 hero 写「更新至 2 集」
 * 而详情页写着「已入库 7 / 12 集」，同一个界面两个数字互相矛盾。
 * 详情页那个数字读的就是本函数用的 `local_number_of_episodes`，同源即一致。
 *
 * ⚠ **两个集数字段不同级**（探针实测，2026-11-26）：
 *   · `local_number_of_episodes` = **当前季**已入库集数；
 *   · `number_of_episodes`       = **全剧**总集数。
 *   多季番会因此出现 local < total 甚至差很多（「药屋少女的呢喃」第 3 季：local=7、total=60）。
 *   所以多季条目必须**带上季号**（`第 3 季 · 更新至 7 集`），否则用户会读成「全剧只更新到 7 集」。
 *   单季条目（season_number ≤ 1 且只有一季）才用裸文案。
 *
 * 口径：
 *   · 连载中 → `更新至 {local} 集`（= 用户当前能看的集数）；
 *   · 已完结 → `全 {total} 集`；
 *   · 两个数字都没有 → 不产出（不显示），绝不显示「更新至 0 集」。
 */
export function fnosHeroMeta(it: MediaItem): HeroMeta {
  const isMovie = (it.type || "").toLowerCase() === "movie";
  const out: HeroMeta = {};
  if (isMovie) return out;

  const status = TV_STATUS[str(it.status)] ?? "";
  if (status) out.status = status;

  // [lc-1727] 与海报卡/详情页同一套回退：local_* 优先，缺失才用 number_*
  const local = Math.max(num(it.local_number_of_episodes), 0); // 当前季已入库
  const total = Math.max(num(it.number_of_episodes), 0); // 全剧总集数
  const seasonNo = num(it.season_number);
  const seasonCount = Math.max(num(it.local_number_of_seasons), num(it.number_of_seasons), 0);

  // 多季判定：季号 > 1，或本地有多季 —— 此时 local 与 total 不同级，必须标季号
  const multiSeason = seasonNo > 1 || seasonCount > 1;
  const seasonTag =
    multiSeason && seasonNo > 0 ? `第 ${seasonNo} 季 · ` : "";

  if (status === "已完结") {
    // 已完结优先报全剧总集数（那才是「全 N 集」的语义）；缺 total 才退到本季已入库
    const n = total > 0 ? total : local;
    if (n > 0) out.episodes = `全 ${n} 集`;
  } else if (status === "连载中") {
    if (local > 0) out.episodes = `${seasonTag}更新至 ${local} 集`;
  } else if (local > 0 && total > local) {
    // 状态缺失（老 NAS / 部分库）：已入库 < 总集数 ⇒ 在连载
    out.episodes = `${seasonTag}更新至 ${local} 集`;
  } else if (total > 0) {
    out.episodes = `全 ${total} 集`;
  } else if (local > 0) {
    out.episodes = `${seasonTag}全 ${local} 集`;
  }

  return out;
}

/**
 * 合成最终展示值。**优先级：飞牛本体（状态/集数）→ TMDB 兜底（仅飞牛缺该字段时）。**
 *
 * 为什么这条顺序不能反：TMDB 的剧集进度是社区投稿，实测会滞后（某番 TMDB 停在
 * S1E2 而库里已有 7 集）。用它覆盖飞牛会让标签与详情页对不上 —— 用户已报障。
 * 题材则相反：飞牛 item/list 不带 genres，只有 TMDB 有，所以那一项永远是 TMDB 补的。
 */
export function resolveHeroMeta(it: MediaItem, patch?: HeroMeta): HeroMeta {
  const isMovie = (it.type || "").toLowerCase() === "movie";
  const out: HeroMeta = { kind: isMovie ? "电影" : "剧集" };

  // 年份：TMDB 优先（与 lc-2385「元数据换成我们自建 TMDB」一致）。
  //   年份不存在"滞后"问题（一部剧的首播年不会变），所以不必像状态/集数那样反过来。
  const year = patch?.year || str(it.release_date || it.air_date).slice(0, 4);
  if (/^\d{4}$/.test(year)) out.year = year;

  if (patch?.genres?.length) out.genres = patch.genres.slice(0, MAX_GENRES);

  // 状态/集数：飞牛优先，TMDB 只补空
  const fnos = fnosHeroMeta(it);
  const status = fnos.status || patch?.status;
  if (status) out.status = status;
  const episodes = fnos.episodes || patch?.episodes;
  if (episodes) out.episodes = episodes;

  // 评分：TMDB 覆盖飞牛（与 lc-2385 的元数据口径一致，且 TMDB 分数更准）
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
