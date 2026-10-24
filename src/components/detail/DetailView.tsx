/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { type UIEvent, useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createPortal } from "react-dom";
import {
  Check as IcoCheckL,
  Clapperboard as IcoClapL,
  Film as IcoFilmL,
  LayoutGrid as IcoGridL,
  LayoutList as IcoListL,
  Play as IcoPlayL,
  X as IcoXL,
} from "lucide-react";
import { DetailSkeleton } from "./DetailSkeleton";
// [lc-1629] 飞牛每集/条目自定义元数据操作弹层（编辑元数据 / 手动匹配 / 刷新 / 解除匹配）
import { MetaEditModal, type MetaTarget } from "./MetaEditModal";
import { BGM_PAGE, TAB_LABEL, bgmList, fnosCast, pickGenres } from "./helpers";
import { OverviewTab } from "./tabs/OverviewTab";
import { RelatedTab, type TmdbRecommendation } from "./tabs/RelatedTab";
import { CastTab } from "./tabs/CastTab";
import { ReviewsTab } from "./tabs/ReviewsTab";
import { CommentsTab } from "./tabs/CommentsTab";
import { ExtraTab } from "./tabs/ExtraTab";
import { RatingWidget } from "./RatingWidget";
// [lc-1652] 电影详情页「沉浸式」版式（用户样板）
import { MovieHero } from "./MovieHero";
// [lc-1747] 在线播放恢复挂载（lc-1627 曾因功能搁置而注释，配套 Rust 加固已就位）。
import { WebrsPanel } from "../webrs/WebrsPanel";
import { fmtDur, pick } from "../../lib/format";
import { bgmImg, imgUrl, pickLargestImg, tmdbImg } from "../../lib/images";
import { prefersReducedMotion } from "../../lib/motion";
import { useRefresh } from "../../hooks/useRefresh";
import { BgmCharacter, BgmComment, BgmEpisodeRow, BgmMatch, BgmPerson, BgmRelated, BgmReview, BgmTag, Episode, MediaItem, MediaVersion, PersonTarget, PlayItem, Season, TmdbCreditPerson, TmdbCredits } from "../../types";


/* ══════════════════ 选集显示样式（用户可切换，localStorage 持久化） ══════════════════
   · num = 纯数字（1 2 3）
   · ep  = 第 N 集（默认）
   只影响**标签文本**：点击/播放/进度/已看徽标仍绑定 ep.guid，与样式无关。 */
type EpLabelMode = "num" | "ep";
const EP_LABEL_KEY = "fntv-ep-label-mode";
const EP_LABEL_MODES: { key: EpLabelMode; label: string; hint: string }[] = [
  { key: "num", label: "1 2 3", hint: "纯数字集号（1 2 3）" },
  { key: "ep", label: "第N集", hint: "第 N 集" },
];

/** 读取已保存的集数样式；非法/首次默认「第N集」。 */
function readEpLabelMode(): EpLabelMode {
  try {
    const v = localStorage.getItem(EP_LABEL_KEY);
    if (v === "num" || v === "ep") return v;
    // 旧版本保存过 sxe：已移除该模式，回落到默认「第N集」
    if (v === "sxe") return "ep";
  } catch {
    /* 隐私模式等拿不到 localStorage：静默回落，不阻断页面 */
  }
  return "ep";
}

/* ══════════════════ 选集视图（网格 / 列表，localStorage 持久化） ══════════════════
   · list = 现有竖向列表（默认，保持老用户观感）
   · grid = 数字方块网格（用户截图样式）
   两种视图共用同一份 episodes 数据与播放逻辑。 */
type EpViewMode = "list" | "grid";
const EP_VIEW_KEY = "fntv-ep-view-mode";
/* [追更重拉] 同一季「缺剧照集号签名」变化后的最小请求间隔（60s）：
   防止接口抖动 / 集列表反复变化时来回重拉，形成请求循环。 */
const STILL_RETRY_COOLDOWN_MS = 60_000;

function readEpViewMode(): EpViewMode {
  try {
    return localStorage.getItem(EP_VIEW_KEY) === "grid" ? "grid" : "list";
  } catch {
    return "list";
  }
}

/* ══════════════════ [版本] 同一剧/集的多版本（多文件）══════════════════
   照抄 FlyNarwhal（lib/ui/features/movie_detail/movie_detail_screen.dart）：
   · 飞牛服务端把同一剧集的多个版本归到同一个 item；`stream/list` 的
     `video_streams[]` 每个文件一条（各自 media_guid），长度 > 1 即「多版本」。
   · 展示标签 = 分辨率大写 + 色域；`DolbyVision` 显示「杜比视界」。
   · 默认选中 = play/info 的 media_guid（服务端记住的上次播放版本）→ 本地记忆 → 第一条。
   · 用户选择写入 localStorage（每个条目/集 guid 一条），下次进入沿用。 */
const MEDIA_VERSION_KEY_PREFIX = "fntv-media-version:";

function readMediaVersion(guid: string): string {
  try {
    return localStorage.getItem(MEDIA_VERSION_KEY_PREFIX + guid)?.trim() ?? "";
  } catch {
    return "";
  }
}

function saveMediaVersion(guid: string, mediaGuid: string) {
  try {
    if (guid && mediaGuid) localStorage.setItem(MEDIA_VERSION_KEY_PREFIX + guid, mediaGuid);
  } catch {
    /* localStorage 不可用：仅本次会话生效 */
  }
}

/** 去掉文件扩展名（仅取文件名，不含目录；用于版本标签兜底）。 */
function stripExt(name: string): string {
  const base = String(name ?? "").trim().split(/[\\/]/).pop() ?? "";
  return base.replace(/\.[A-Za-z0-9]{1,5}$/, "").trim();
}

/** 版本盒子文案（与 FlyNarwhal `_MediaSourceBoxes` 同口径）。
 *  [版本文案] 上游 title 常为空、云端/STRM 条目分辨率与色域也空，故补回退链：
 *  分辨率+色域 → 高度P+编码+位深 → title → 文件名去扩展名 → 「默认版本」。 */
function versionLabel(v: MediaVersion): string {
  const res = String(v.resolution_type ?? "").trim().toUpperCase();
  const colorRaw = String(v.color_range_type ?? "").trim();
  const color = colorRaw === "DolbyVision" ? "杜比视界" : colorRaw;
  const primary = [res, color].filter(Boolean).join(" ").trim();
  if (primary) return primary;

  const h = Number(v.height ?? 0);
  const spec = [
    h > 0 ? `${h}P` : "",
    String(v.codec_name ?? "").trim(),
    Number(v.bit_depth ?? 0) > 0 ? `${v.bit_depth}bit` : "",
  ]
    .filter(Boolean)
    .join(" ");
  if (spec) return spec;

  const title = String(v.title ?? "").trim();
  if (title) return title;

  const file = stripExt(String(v.file_name ?? ""));
  return file || "默认版本";
}

/** 批量生成版本标签：重复时追加 ` · 文件名`，仍重复则追加 ` · 序号`，保证可区分。 */
function versionLabels(versions: MediaVersion[]): string[] {
  const base = versions.map((v) => versionLabel(v));
  const count = new Map<string, number>();
  for (const b of base) count.set(b, (count.get(b) ?? 0) + 1);

  const withFile = base.map((b, i) => {
    if ((count.get(b) ?? 0) <= 1) return b;
    const file = stripExt(String(versions[i]?.file_name ?? ""));
    return file ? `${b} · ${file}` : b;
  });
  const count2 = new Map<string, number>();
  for (const b of withFile) count2.set(b, (count2.get(b) ?? 0) + 1);
  return withFile.map((b, i) => ((count2.get(b) ?? 0) > 1 ? `${b} · ${i + 1}` : b));
}

/** 默认版本优先级：本地记忆 → play/info（服务端上次播放）→ 第一条。 */
function chooseMediaVersion(guid: string, versions: MediaVersion[], serverSelected?: string): string {
  const remembered = readMediaVersion(guid);
  if (remembered && versions.some((v) => v.media_guid === remembered)) return remembered;
  if (serverSelected && versions.some((v) => v.media_guid === serverSelected)) return serverSelected;
  return versions[0]?.media_guid ?? "";
}

/* ══════════════════ [选集定位] 上次观看集自动定位 ══════════════════
   目标：打开剧集详情页时，把右栏选集滚到「上次观看的那一集」，而不是停在顶部。
   数据来源（全部复用现有能力，不新增接口）：
     ① 继续观看 play/list（App 的 continueList）—— 条目级最近记录，含 guid/parent_guid/
        season_number/episode_number；
     ② 本机集列表自带的进度（episode.ts / duration / watched）—— 当前季兜底；
     ③ 详情页 base 自带的 season_number / episode_number —— 最后兜底。
   匹配优先级：父季 guid / 条目 guid（当前季） > 父剧 guid > 标题匹配 > 本地进度 > base。
   跨季时先切季，等该季 episodes 到位后再定位；每个条目只自动定位一次。 */

type EpisodeLocateTarget = {
  /** 目标季下标；-1 = 未知（沿用当前季） */
  seasonIdx: number;
  /** 目标集 guid（最可靠） */
  guid?: string;
  /** 目标集号（guid 找不到时按集号兜底） */
  num?: number;
};

/** play/list 字段双格式读取（snake_case / camelCase）。 */
function locateStr(it: PlayItem, keys: string[]): string {
  for (const k of keys) {
    const v = it[k];
    if (typeof v === "string" && v.trim()) return v.trim();
    if (typeof v === "number" && Number.isFinite(v)) return String(v);
  }
  return "";
}

function locateNum(it: PlayItem, keys: string[]): number | null {
  for (const k of keys) {
    const v = it[k];
    const n = typeof v === "number" ? v : typeof v === "string" && v.trim() ? Number(v) : NaN;
    if (Number.isFinite(n)) return n;
  }
  return null;
}

/** 标题归一化：忽略大小写、空白与常见分隔符（play/list 的 tv_title 与库内标题常有细微差异）。 */
function locateTitleKey(s: string): string {
  return s.trim().toLowerCase().replace(/[\s·:：\-—_]+/g, "");
}

/** 解析「上次观看」目标集。返回 null = 没有任何可用记录（保持默认：第一集/顶部）。 */
function resolveEpisodeLocateTarget(args: {
  base: MediaItem;
  seasons: Season[];
  episodes: Episode[];
  seasonIdx: number;
  continueList: PlayItem[];
}): EpisodeLocateTarget | null {
  const { base, seasons, episodes, seasonIdx, continueList } = args;
  const seasonIdxByGuid = new Map<string, number>();
  const seasonIdxByNo = new Map<number, number>();
  seasons.forEach((s, i) => {
    if (s.guid) seasonIdxByGuid.set(s.guid, i);
    const n = Number(s.season_number);
    if (Number.isFinite(n)) seasonIdxByNo.set(n, i);
  });
  const names = new Set(
    [locateTitleKey(base.title), locateTitleKey(base.tv_title ?? "")].filter(Boolean),
  );

  // 强匹配（guid / 季 guid / 剧 guid）与弱匹配（仅标题）分开：弱匹配排在本地进度之后，
  // 避免同名作品把用户带到错误的一集。
  let best: { rank: number; at: number; order: number; target: EpisodeLocateTarget } | null = null;
  let titleOnly: EpisodeLocateTarget | null = null;

  for (let order = 0; order < continueList.length; order++) {
    const it = continueList[order];
    if (!it || typeof it !== "object") continue;
    const type = locateStr(it, ["type"]).toLowerCase();
    if (type.includes("movie")) continue;
    const guid = locateStr(it, ["guid"]);
    const parent = locateStr(it, ["parent_guid", "parentGuid"]);
    const series = locateStr(it, ["series_guid", "seriesGuid"]);
    const seasonNo = locateNum(it, ["season_number", "seasonNumber"]);
    const epNo = locateNum(it, ["episode_number", "episodeNumber"]);
    const name = locateStr(it, ["tv_title", "tvTitle"]) || locateStr(it, ["title"]);

    let rank = 0;
    let sIdx = -1;
    if (guid && episodes.some((e) => e.guid === guid)) {
      // ① 条目 guid 命中当前季集列表：最可靠
      rank = 5;
      sIdx = seasonIdx;
    } else if (parent && seasonIdxByGuid.has(parent)) {
      // ② 父季 guid 命中：跨季也能定位
      rank = 4;
      sIdx = seasonIdxByGuid.get(parent)!;
    } else if (series && series === base.guid) {
      // ③ 富化后的父剧 guid 命中
      rank = 4;
    } else if (parent && parent === base.guid) {
      // ④ 部分接口把剧 guid 直接放在 parent_guid
      rank = 3;
    } else if (name && names.has(locateTitleKey(name)) && epNo != null && epNo > 0) {
      // ⑤ 仅标题匹配：弱信号，放最后
      rank = 1;
    } else {
      continue;
    }
    if (sIdx < 0 && seasonNo != null && seasonIdxByNo.has(seasonNo)) sIdx = seasonIdxByNo.get(seasonNo)!;
    const target: EpisodeLocateTarget = { seasonIdx: sIdx, guid: guid || undefined, num: epNo ?? undefined };
    if (rank === 1) {
      if (!titleOnly) titleOnly = target;
      continue;
    }
    const at =
      locateNum(it, [
        "played_at",
        "playedAt",
        "last_played",
        "lastPlayed",
        "last_played_at",
        "lastPlayedAt",
        "watchedAt",
        "watched_at",
      ]) ?? 0;
    if (
      !best ||
      rank > best.rank ||
      (rank === best.rank && (at > best.at || (at === best.at && order < best.order)))
    ) {
      best = { rank, at, order, target };
    }
  }
  if (best) {
    const b = best as { target: EpisodeLocateTarget };
    return b.target;
  }

  // 本地进度兜底（当前季）：
  //   ① 优先「有进度且未看完」的最大集号（与「继续看」按钮同口径）；
  //   ② 若整季都看完了，则回退到「有观看痕迹（进度>0 或 watched=1）」的最大集号
  //      —— 避免看完最后一集后进详情页又跳回顶部。
  const numbered = episodes
    .map((e, i) => ({ e, num: Number(e.episode_number ?? i + 1) }))
    .filter(({ num }) => Number.isFinite(num) && num > 0);
  const inProgress = numbered.filter(
    ({ e }) => (e.ts ?? 0) > 0 && !(e.duration && e.ts! >= e.duration),
  );
  const touched = numbered.filter(({ e }) => (e.ts ?? 0) > 0 || Number(e.watched) === 1);
  const pool = inProgress.length ? inProgress : touched;
  if (pool.length) {
    const picked = pool.reduce((b, c) => (c.num > b.num ? c : b));
    return { seasonIdx, guid: picked.e.guid, num: picked.num };
  }

  if (titleOnly) return titleOnly;

  // 详情页 base 自带的当前集（例如从「继续观看」条目直接进详情）。
  const baseNum = Number(base.episode_number ?? 0);
  if (Number.isFinite(baseNum) && baseNum > 0) {
    const bSeason = Number(base.season_number ?? NaN);
    let sIdx = seasonIdx;
    if (Number.isFinite(bSeason)) {
      const i = seasons.findIndex((s) => Number(s.season_number) === bSeason);
      if (i >= 0) sIdx = i;
    }
    return { seasonIdx: sIdx, num: baseNum };
  }
  return null;
}

/** 在选集容器里找目标集元素（列表行 / 网格方块都带 data-ep-guid / data-ep-num）。 */
function findEpisodeElement(list: HTMLElement, t: EpisodeLocateTarget): HTMLElement | null {
  const nodes = Array.from(list.querySelectorAll<HTMLElement>("[data-ep-guid], [data-ep-num]"));
  if (t.guid) {
    const byGuid = nodes.find((n) => n.dataset.epGuid === t.guid);
    if (byGuid) return byGuid;
  }
  if (t.num != null) {
    const byNum = nodes.find((n) => n.dataset.epNum === String(t.num));
    if (byNum) return byNum;
  }
  return null;
}

/** 集号 → 展示文本。 */
function epLabelOf(num: number, _seasonNo: number, mode: EpLabelMode): string {  if (mode === "num") return String(num);
  return `第 ${num} 集`;
}

/** 季 → {季号文案, 可选季名}。飞牛多数把 title 直接写成「第 N 季」，此时不重复展示。 */
function seasonLabelOf(sn: Season, i: number): { no: string; name: string } {
  const rawNo = Number(sn.season_number);
  const n = Number.isFinite(rawNo) ? rawNo : i + 1;
  const no = n === 0 ? "特别篇" : `第 ${n} 季`;
  const raw = String(sn.title || "").trim();
  const name = raw && raw !== no && !new RegExp(`^第\\s*${n}\\s*季$`).test(raw) ? raw : "";
  return { no, name };
}

/** [lc-1728] 判断 Bangumi 条目名是否明确指向第 n 季（用于校验季匹配是否真的命中当前季，
 *  而不是 bgm_match 去掉季后缀后回落到的第一季）。 */
function looksLikeSeason(name: string, n: number): boolean {
  const s = name.trim().toLowerCase();
  if (!s || n <= 0) return false;
  const zh = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十"][n] ?? "";
  if (s.includes(`第${n}季`) || s.includes(`第${n}期`)) return true;
  if (zh && (s.includes(`第${zh}季`) || s.includes(`第${zh}期`))) return true;
  if (s.includes(`season ${n}`) || s.includes(`season${n}`)) return true;
  if (s.includes(`${n}nd season`) || s.includes(`${n}rd season`) || s.includes(`${n}th season`)) return true;
  return false;
}


export function DetailView({
  base,
  onBack,
  onOpenTmdb,
  onOpenPerson,
  continueList = [],
  webrsOn = false,
}: {
  base: MediaItem;
  onBack: () => void;
  /** [lc-1658] 关联标签页 TMDB 推荐卡片点击 → 打开对应 TMDB 条目详情（与发现页同一链路） */
  onOpenTmdb?: (tmdbId: number, title: string, year: string, mediaType: "tv" | "movie") => void;
  /** [lc-1700] 演职人员点击 → 打开演员页 */
  onOpenPerson?: (t: PersonTarget) => void;
  /** [选集定位] 继续观看 play/list（App 已加载）：用于定位「上次观看集」 */
  continueList?: PlayItem[];
  /** [lc-1753] 在线资源搜索开关（App 的 settings.webrsPanelEnabled）：开启后才在
   *  无资源的剧集/动漫详情页挂 WebrsPanel；关闭时维持「未查询到资源/暂无剧集」占位。 */
  webrsOn?: boolean;
}) {
  const [data, setData] = useState<Record<string, unknown> | null>(null);
  const [seasons, setSeasons] = useState<Season[]>([]);
  const [seasonIdx, setSeasonIdx] = useState(0);
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  // [lc-1727] 切季时清空旧季集列表后，右栏先显示「加载中」而不是「暂无剧集」，避免误判。
  const [epsLoading, setEpsLoading] = useState(false);
  /* [版本] 当前展示的版本列表（属于 versionGuid 这个条目/集）。
     多版本判定与展示照抄 FlyNarwhal：video_streams.length > 1 才显示版本盒子。 */
  const [mediaVersions, setMediaVersions] = useState<MediaVersion[]>([]);
  const [versionGuid, setVersionGuid] = useState("");
  const [selectedMediaGuid, setSelectedMediaGuid] = useState("");
  /* [版本] 每个 guid 的版本列表缓存 + 请求令牌（快速切集时只接受最后一次结果）。 */
  const versionCacheRef = useRef<Map<string, { versions: MediaVersion[]; selected: string }>>(new Map());
  const versionReqRef = useRef(0);
  /* [版本] 点「多版本」的某一集时弹出的版本选择器（选完即播）。 */
  const [versionPicker, setVersionPicker] = useState<{
    guid: string;
    label: string;
    versions: MediaVersion[];
    selected: string;
  } | null>(null);
  /* [单集封面兜底] TMDB 季度剧照映射：key = `${tmdbId}:${seasonNumber}`，
     value = { 集号 → still 文件路径（/abc.jpg） }。飞牛缺单集海报时按季拉一次，切季按需再拉。 */
  const [seasonStills, setSeasonStills] = useState<Record<string, Record<number, string>>>({});
  /* [追更重拉] 每季最近一次请求的「缺剧照集号签名」+ 时间戳：
       · 同一签名不重复请求（去重，避免每次 render 都打网络）；
       · 签名变化（新集加入 / 集列表更新）→ 带 force 重拉一次；
       · 换签名后 60s 冷却，防止接口抖动 / 集列表来回变化导致请求循环。
     失败也静默降级，不阻塞页面。 */
  const stillReqRef = useRef<Map<string, { sig: string; at: number }>>(new Map());
  /* [选集样式] 集号显示模式：纯数字 / 第N集 / SxxExx。选择写入 localStorage，
     下次进任何剧集详情页都沿用（与 fntv-history-view 等键同一套做法）。 */
  const [epLabelMode, setEpLabelMode] = useState<EpLabelMode>(readEpLabelMode);
  useEffect(() => {
    try {
      localStorage.setItem(EP_LABEL_KEY, epLabelMode);
    } catch {
      /* localStorage 不可用：仅本次会话生效 */
    }
  }, [epLabelMode]);
  /* [选集视图] 网格 / 列表：与集数样式一样持久化，下次进任何剧集详情页都沿用。 */
  const [epView, setEpView] = useState<EpViewMode>(readEpViewMode);
  useEffect(() => {
    try {
      localStorage.setItem(EP_VIEW_KEY, epView);
    } catch {
      /* localStorage 不可用：仅本次会话生效 */
    }
  }, [epView]);
  /* [选季控件] 横向滚动轨道的 ref + 两端是否还有内容（决定箭头可用态）。 */
  const seasonRailRef = useRef<HTMLDivElement | null>(null);
  /* 集列表请求令牌：快速连点不同季时，只接受最后一次请求的结果，
     避免先发的旧季响应晚到、把新季列表覆盖掉（"点了后面季却还是第一季"的另一种成因）。 */
  const epReqRef = useRef(0);
  /* [选集定位] 右栏选集滚动容器 + 自动定位状态。
     · done：本条目只自动定位一次（数据晚到/重渲染不再把用户拉回）；
     · userTouched：用户手动滚动/点击过 → 放弃尚未执行的自动定位；
     · skipOnce：切条目后的第一轮 effect 跳过（此时 seasons/episodes 还是上一条目的旧值）。 */
  const epListRef = useRef<HTMLDivElement | null>(null);
  /* 当前 episodes 实际属于哪一季（在 get_episodes 成功回调里写入）。
     切季瞬间 episodes 还是上一季的旧数组，定位 effect 必须等它换成目标季才能滚，
     否则会按旧季的集号滚到错误位置。 */
  const loadedSeasonGuidRef = useRef("");
  const autoLocateRef = useRef<{ guid: string; done: boolean; userTouched: boolean }>({
    guid: "",
    done: true,
    userTouched: false,
  });
  const autoLocateSkipRef = useRef(false);
  // [lc-1595] 播放结束 → 强制刷新观看进度：`ts`/`watched` 随每次播放变化，而本页原先只在
  //   挂载/切条目/切季时取数 ⇒ 用户「看完第 3 集退出来，状态还是之前的」。
  //   事件来自 Rust（mpv.rs）：正常看完/退出 = `mpv-load{phase:"ended"}`；播放失败 = `mpv-end`。
  //   ⚠ 必须**带 force** 取数：季/集有 10 分钟缓存（lc-1578），不带 force 会拿回旧进度。
  const [progTick, setProgTick] = useState(0);
  const forceSeasonRef = useRef(false);
  const forceEpsRef = useRef(false);
  const lastEpGuidRef = useRef("");
  // [lc-1727] 上一次渲染的条目 guid：区分「切条目」与「播放结束/刷新重取」，
  //   后者不能重置 seasonIdx（否则看完当前季会被打回第 1 季）。
  const lastBaseGuidRef = useRef("");
  /* [lc-1728] 首次进入某条目时，把默认季落在「第 1 季」而不是排在最前的特别篇（season 0）。 */
  const seasonInitRef = useRef("");
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
  // [lc-1759→1761] Switch 独立按钮（不占标签页）：切换右列「本地选集 ↔ 在线资源搜索」
  const [onlineMode, setOnlineMode] = useState(false);
  // [lc-1423] 重新引入简介展开态：true = 弹窗打开（看完整简介）。
  //   注意语义与 lc-1410 之前不同 —— 那时是"就地铺开"（会撑高容器、拉长海报），
  //   现在是"**弹出弹窗**看全文"，容器高度不受影响。
  const [introFull, setIntroFull] = useState(false);
  // [lc-119] 吐槽（Bangumi 评论）：懒加载 —— 用户切到该分段时才匹配+拉评论（避免拖慢首屏）
  const [bgm, setBgm] = useState<BgmMatch | null>(null);
  const bgmRef = useRef<BgmMatch | null>(null); // 供 legacy 回填任务/翻页读最新值（不进依赖数组）
  /* [lc-1728] 本次 BGM 匹配是否**命中了当前季**（多季时按「剧名 + 季名/季号」重搜）。
     只有命中当前季，才允许用 BGM 集号映射单集标题/简介；否则（回落剧根/第一季）必须禁用，
     避免把第一季内容串到其它季。 */
  const bgmSeasonMatchedRef = useRef(false);
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
  // [lc-1652] 电影（非剧集、且有飞牛条目）走「沉浸式」样板版式；发现页占位条目保持原样
  const isMovie = !isSeries && !/^discover-/.test(base.guid);
  // [lc-1444] 发现页条目形态：guid 是 `discover-<tmdbId>` 占位标识（不是真飞牛 guid），
  //   没有飞牛条目可查 —— 详情主体靠 TMDB（条目自带 tmdb_id 直接定位）+ Bangumi（标题匹配），
  //   选集区显示占位。dataReady = 主体可以渲染（真条目等 data 到位；发现页条目立即渲染）。
  const noFnOS = /^discover-/.test(base.guid);
  const dataReady = !!data || noFnOS;

  /* [lc-1728] 当前选中季 = 所有按季数据的**唯一来源**。
     · multiSeason：是否多季（单季条目沿用原有链路，不做额外按季处理）；
     · seasonGuid / seasonNo：当前季的 guid / 季号；
     · bgmSeasonKey：按季派生状态（BGM / 演员表 / 评论…）的重置与请求依赖键。
     ⚠ 这里放在 resolveBgm 之前：BGM 匹配、演员表缓存键都要带当前季。 */
  const multiSeason = isSeries && seasons.length > 1;
  const seasonGuid = isSeries ? String(seasons[seasonIdx]?.guid ?? "") : "";
  // 保留 0（特别篇）；缺失时才用下标+1 兜底。
  const seasonNo = (() => {
    if (!isSeries) return 0;
    const raw = Number(seasons[seasonIdx]?.season_number);
    return Number.isFinite(raw) ? raw : seasonIdx + 1;
  })();
  const bgmSeasonKey = multiSeason ? seasonGuid || `idx${seasonIdx}` : "";
  /* [lc-1728] 是否允许用 Bangumi 集号映射单集标题/简介：
     · 单季、多季第一季 → 允许（剧根 subject 就是这一季）；
     · 多季其它季 → 仅当本次 BGM 匹配确实命中了「当前季」且与剧根 subject 不同才允许，
       否则（回落剧根/第一季）一律禁用，避免把第一季的集标题/简介串过来。 */
  const bgmSeriesSubjectId = Number(base.bgm_id ?? 0) || 0;
  const bgmCurrentSubjectId = Number(bgm?.subject_id ?? 0) || 0;
  const bgmMappingAllowed =
    !multiSeason ||
    seasonNo === 1 ||
    (bgmSeasonMatchedRef.current &&
      bgmCurrentSubjectId > 0 &&
      bgmCurrentSubjectId !== bgmSeriesSubjectId);

  // [lc-1444] Bangumi 定位统一入口：发现页右栏条目自带 bgm_id → **直连**（省一次标题匹配，
  //   也不会匹配错同名番）；否则 bgm_match(标题+年)。结果写 bgmRef + setBgm，供
  //   首屏标签框 / 影评 / 吐槽 / 更多 / 关联五个懒加载点共用同一条路径。
  // [lc-1728] 多季条目：Bangumi 的 subject 是「按季」的（第二季是独立 subject）。原来只用剧根
  //   标题匹配 → 永远命中第一季，于是切到第 2 季时标签/评分/角色/评论/关联全是第一季的数据。
  //   现在第 2 季起用「剧名 + 季名/季号」重搜一次（cacheKey = 季 guid，独立缓存）；搜不到
  //   才回落到剧根匹配（此时禁用按集号映射，避免串季）。
  const resolveBgm = useCallback(async (): Promise<BgmMatch | null> => {
    const cached = bgmRef.current;
    if (cached) return cached;
    const d = data ?? {};
    const seriesTitle = String(pick(d, ["tvTitle", "title"]) ?? base.title).trim() || base.title;
    let m: BgmMatch | null = null;
    let seasonMatched = false;
    if (multiSeason && seasonNo > 1) {
      const sn = seasons[seasonIdx];
      const rawSeasonTitle = String(sn?.title ?? "").trim();
      const label = seasonLabelOf(sn, seasonIdx).no;
      // 季名若是「第 N 季」这类泛称，就拼成「剧名 第N季」；否则若已含剧名直接用，避免重复。
      const isGeneric = !rawSeasonTitle || /^第\s*\d+\s*季$/.test(rawSeasonTitle) || rawSeasonTitle === label;
      const query = isGeneric
        ? `${seriesTitle} ${label}`
        : rawSeasonTitle.includes(seriesTitle)
          ? rawSeasonTitle
          : `${seriesTitle} ${rawSeasonTitle}`;
      const year =
        String(sn?.release_date || sn?.air_date || "").slice(0, 4) ||
        String(base.release_date || "").slice(0, 4) ||
        null;
      try {
        const r = await invoke<BgmMatch | null>("bgm_match", {
          query,
          year,
          mediaType: base.type ?? null,
          cacheKey: seasonGuid || `season-${seasonNo}`,
        });
        if (r && r.subject_id) {
          // 校验确实命中「当前季」：与剧根 subject 不同，或条目名里带明确季标记。
          // 否则视为 bgm_match 去掉季后缀后的回落（第一季），不标记 seasonMatched，
          // 从而禁用按集号映射，避免把第一季内容串过来。
          const nm = `${r.name ?? ""} ${r.name_cn ?? ""}`;
          const distinctFromSeries = bgmSeriesSubjectId > 0 && r.subject_id !== bgmSeriesSubjectId;
          seasonMatched = distinctFromSeries || looksLikeSeason(nm, seasonNo);
          m = r;
        }
      } catch {
        /* 季匹配失败 → 回落剧根匹配（下面） */
      }
    }
    if (!m) {
      if (base.bgm_id && (!multiSeason || seasonNo <= 1)) {
        m = { subject_id: base.bgm_id };
      } else {
        const r = await invoke<BgmMatch | null>("bgm_match", {
          query: seriesTitle,
          year: String(base.release_date || "").slice(0, 4) || null,
          mediaType: base.type ?? null,
          cacheKey: base.guid,
        });
        m = r && r.subject_id ? r : null;
      }
    }
    bgmSeasonMatchedRef.current = seasonMatched;
    bgmRef.current = m;
    setBgm(m);
    return m;
  }, [base.bgm_id, base.guid, base.title, base.type, base.release_date, data, multiSeason, seasonIdx, seasons, seasonGuid, seasonNo, bgmSeriesSubjectId]);

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

  /* [单集封面兜底 / 追更重拉] 当前季有集缺 `poster` 时，向 TMDB 拉该季剧照（`tmdb_season_stills`），
     按 episode_number 建映射。规则：
       · 需要有效 tmdbId（发现页占位条目没有飞牛选集，直接跳过）；
       · 用「缺剧照的集号集合」做签名 `s{season}:{nums}`：
         - 同一签名只请求一次（去重）；
         - 签名变化（新集加入 / 集列表更新）→ 带 `force: true` 重拉，避免后端旧缓存挡住新集；
         - 换签名后 60s 内不再发第二次（防接口抖动 / 集列表来回变化造成循环）；
       · 切季时 cacheKey 与签名都变，自然按新季请求；
       · 该季所有集都有剧照 → 签名不存在，不请求；
       · 请求失败静默降级 —— 不影响页面，缩略图回落到剧集横图/竖版海报/空占位。 */
  useEffect(() => {
    if (!isSeries || noFnOS || episodes.length === 0) return;
    const missingNums = episodes
      .map((ep, i) => ({ ep, num: Number(ep.episode_number ?? i + 1) }))
      .filter(({ ep, num }) => !ep.poster && Number.isFinite(num) && num > 0)
      .map(({ num }) => num)
      .sort((a, b) => a - b);
    if (missingNums.length === 0) return;
    const tmdbId = Number(tmdbAnchor.tmdbId);
    if (!Number.isFinite(tmdbId) || tmdbId <= 0) return;
    const seasonNo =
      Number(seasons[seasonIdx]?.season_number ?? seasonIdx + 1) || 1;
    const cacheKey = `${tmdbId}:${seasonNo}`;
    const sig = `s${seasonNo}:${missingNums.join(",")}`;
    const prev = stillReqRef.current.get(cacheKey);
    const now = Date.now();
    if (prev) {
      if (prev.sig === sig) return; // 同一签名：已请求过，去重
      if (now - prev.at < STILL_RETRY_COOLDOWN_MS) return; // 换签名：短冷却
    }
    stillReqRef.current.set(cacheKey, { sig, at: now });
    invoke<{ episodes?: { episodeNumber?: number; still?: string }[] }>("tmdb_season_stills", {
      arg: { tmdbId, seasonNumber: seasonNo, force: true },
    })
      .then((r) => {
        const map: Record<number, string> = {};
        for (const e of r?.episodes ?? []) {
          const n = Number(e?.episodeNumber);
          const s = String(e?.still ?? "").trim();
          if (Number.isFinite(n) && s) map[n] = s;
        }
        setSeasonStills((prev) => ({ ...prev, [cacheKey]: map }));
      })
      .catch(() => {
        /* 静默降级：不弹错、不影响选集与播放 */
      });
  }, [isSeries, noFnOS, episodes, tmdbAnchor.tmdbId, seasons, seasonIdx]);

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
    // [lc-1727] 只有**切条目**才清空数据并回到第 1 季；播放结束 / 顶栏刷新（progTick 变化）只重取，
    //   绝不能把用户正在看的季重置回第 1 季 —— 否则「看完第 2 季又跳回第 1 季」。
    const baseChanged = lastBaseGuidRef.current !== base.guid;
    if (baseChanged) {
      lastBaseGuidRef.current = base.guid;
      setData(null);
      setSeasons([]);
      setSeasonIdx(0);
      setEpisodes([]);
      setEpsLoading(false);
      // 新条目第一季即使 guid 与旧条目相同也要重新拉取并进入加载态。
      lastEpGuidRef.current = "";
      // 让上一条目的在途集请求失效，避免它晚到后把新条目的选集覆盖掉。
      epReqRef.current++;
    }
    if (noFnOS) {
      // 发现页条目：无飞牛条目，跳过详情/季/集三条飞牛请求；清掉可能残留的详情错误
      setErr("");
      return;
    }
    // 切条目后旧请求的响应一律丢弃（快速连点不同条目时尤其明显）。
    let alive = true;
    // [lc-1629] 读一次 force 并消费：播放结束/元数据写操作成功后置真 → 详情也跳过 120 天缓存，
    //   否则「编辑元数据」改完标题/简介看不到变化。
    const forceItem = forceSeasonRef.current;
    forceSeasonRef.current = false;
    invoke<Record<string, unknown>>("get_item_detail", { guid: base.guid, force: forceItem })
      .then((d) => {
        if (alive) setData(d);
      })
      .catch((e) => {
        if (alive) setErr(String(e));
      });
    // 层级：剧 → 季 → 集。episode/list 的入参是季 guid（传剧 guid 实测 data:null），
    // 所以先 season/list/{剧guid}，选中季后再 episode/list/{季guid}。
    if (isSeries) {
      invoke<Season[]>("get_seasons", { guid: base.guid, force: forceItem })
        .then((ss) => {
          if (!alive) return;
          const list = (Array.isArray(ss) ? ss : []).slice();
          /* 按季号升序：飞牛返回顺序不保证（特别篇 season_number=0 排最前），
             否则「第 3 季」可能夹在第 1、2 季中间，用户更难找到。缺季号保持原序。 */
          list.sort((a, b) => {
            const an = Number(a.season_number);
            const bn = Number(b.season_number);
            if (Number.isFinite(an) && Number.isFinite(bn)) return an - bn;
            return 0;
          });
          setSeasons(list);
        })
        .catch(() => {});
    }
    return () => {
      alive = false;
    };
  }, [base.guid, isSeries, noFnOS, progTick]);

  // 季列表刷新后索引可能越界（例如缓存回填成更短的列表）：回到第一季，避免右侧空白。
  useEffect(() => {
    if (seasonIdx > 0 && seasonIdx >= seasons.length) setSeasonIdx(0);
  }, [seasons, seasonIdx]);

  /* [lc-1728] 首次进入条目：默认选中「第 1 季」——飞牛季列表按 season_number 升序，特别篇(0)
     排在最前，直接 seasonIdx=0 会默认展示特别篇。每个条目只初始化一次；刷新/播放结束不改。 */
  useEffect(() => {
    if (!isSeries || seasons.length === 0) return;
    if (seasonInitRef.current === base.guid) return;
    seasonInitRef.current = base.guid;
    let idx = seasons.findIndex((s) => Number(s.season_number) === 1);
    if (idx < 0) idx = seasons.findIndex((s) => Number(s.season_number) > 0);
    if (idx < 0) idx = 0;
    setSeasonIdx(idx);
  }, [isSeries, seasons, base.guid]);

  /* [选季控件] 当前季始终滚到可视区中间：切季/首次渲染都能看到高亮项。 */
  useEffect(() => {
    const el = seasonRailRef.current;
    if (!el) return;
    const btn = el.querySelector<HTMLElement>(".season-tab.active");
    if (!btn) return;
    const railRect = el.getBoundingClientRect();
    const btnRect = btn.getBoundingClientRect();
    const target = el.scrollLeft + (btnRect.left - railRect.left) - (el.clientWidth - btnRect.width) / 2;
    el.scrollTo({
      left: Math.max(0, target),
      behavior: prefersReducedMotion() ? "auto" : "smooth",
    });
  }, [seasonIdx, seasons.length]);

  useEffect(() => {
    const s = seasons[seasonIdx];
    if (!s) return;
    const force = forceEpsRef.current;
    forceEpsRef.current = false;
    // 只在**换季**时清空列表：播放结束的刷新保留现有行（否则右栏会闪一下空白）
    const seasonChanged = s.guid !== lastEpGuidRef.current;
    if (seasonChanged) {
      lastEpGuidRef.current = s.guid;
      setEpisodes([]);
      setEpsLoading(true);
    }
    // 请求令牌：快速连点季时丢弃旧响应（见 epReqRef 声明处）。
    const req = ++epReqRef.current;
    invoke<Episode[]>("get_episodes", { guid: s.guid, force, seriesGuid: base.guid })
      .then((eps) => {
        if (req !== epReqRef.current) return;
        loadedSeasonGuidRef.current = s.guid;
        setEpisodes(
          (Array.isArray(eps) ? eps : []).map((e) => ({
            ...e,
            quality:
              Array.isArray(e?.media_stream?.resolutions) && e.media_stream.resolutions[0]
                ? String(e.media_stream.resolutions[0]).replace(/p$/i, "")
                : undefined,
          })),
        );
        setEpsLoading(false);
      })
      .catch(() => {
        if (req === epReqRef.current) setEpsLoading(false);
      });
  }, [seasons, seasonIdx, progTick]);

  /* [选集定位] 切换条目：重置自动定位状态。skipOnce 让同一轮 commit 的定位 effect 先跳过，
     避免拿上一条目的 seasons/episodes 去定位新条目。 */
  useEffect(() => {
    autoLocateRef.current = { guid: base.guid, done: false, userTouched: false };
    autoLocateSkipRef.current = true;
    loadedSeasonGuidRef.current = "";
  }, [base.guid]);

  /* [选集定位] 数据就绪后定位「上次观看集」：
     · 等 seasons + 当前季 episodes 都到位才执行（列表/进度常常晚于组件挂载）；
     · 目标在别的季 → 先 setSeasonIdx，等该季集列表回来后再滚；
     · 只滚动 .dv2e-list 容器（scrollTop 计算），不带动整页；
     · 每个条目只执行一次；用户手动滚动/点击后放弃。 */
  useEffect(() => {
    if (!isSeries || noFnOS) return;
    if (autoLocateSkipRef.current) {
      autoLocateSkipRef.current = false;
      return;
    }
    const st = autoLocateRef.current;
    if (st.guid !== base.guid || st.done || st.userTouched) return;
    if (!seasons.length || !episodes.length) return;
    // 切季瞬间 episodes 仍是上一季的旧数组：等目标季的集列表真正到位（loadedSeasonGuidRef 对齐）再定位。
    const curSeasonGuid = seasons[seasonIdx]?.guid ?? "";
    if (curSeasonGuid && loadedSeasonGuidRef.current !== curSeasonGuid) return;
    const list = epListRef.current;
    if (!list) return;

    const target = resolveEpisodeLocateTarget({ base, seasons, episodes, seasonIdx, continueList });
    // 没有可用记录时**不标记 done**：continueList 常常晚于季/集到达，
    // 等它到达后本 effect 会随依赖变化再跑一次；仍无记录就保持默认（第一集/顶部）。
    if (!target) return;
    // 跨季：先切季；等目标季的 episodes 到位后本 effect 会再跑（done 仍为 false）。
    if (target.seasonIdx >= 0 && target.seasonIdx !== seasonIdx) {
      setSeasonIdx(target.seasonIdx);
      return;
    }
    const el = findEpisodeElement(list, target);
    // 目标元素还没渲染出来（极端时序）：不标记 done，留给下一次依赖变化重试。
    if (!el) return;
    st.done = true;
    // 只算容器内偏移：不调用 scrollIntoView，避免带动整页/左列滚动。
    // 注意：这里**不注册 cleanup 取消 rAF** —— effect 若因 continueList/episodes 更新重跑，
    // 取消会让这次唯一的自动定位丢失；改为在回调里校验元素仍挂载、用户未接管。
    requestAnimationFrame(() => {
      if (autoLocateRef.current.userTouched) return;
      if (!el.isConnected || !list.isConnected) return;
      const lr = list.getBoundingClientRect();
      const rr = el.getBoundingClientRect();
      const top = list.scrollTop + (rr.top - lr.top) - (list.clientHeight - rr.height) / 2;
      list.scrollTo({ top: Math.max(0, top), behavior: "auto" });
    });
  }, [isSeries, noFnOS, base, seasons, seasonIdx, episodes, continueList]);

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

  /* [lc-1728] 切季（多季条目）：把所有**按季派生**的状态清空，避免 React state 复用上一季的
     BGM 匹配 / 标签 / 评分 / 角色 / 评论 / 关联 / 演员表。清空后，下面依赖 bgmSeasonKey 的
     effect 会用「当前季」重新请求（BGM 匹配 cacheKey=季 guid，演员表 cacheKey=季 guid）。
     单季条目没有「切季」概念，直接跳过。 */
  useEffect(() => {
    if (!multiSeason) return;
    bgmRef.current = null;
    bgmSeasonMatchedRef.current = false;
    setBgm(null);
    setBgmProbed(false);
    setBgmSub(null);
    setBgmEps([]);
    commentsOffset.current = 0;
    commentsDoneRef.current = false;
    commentsBusyRef.current = false;
    setComments([]);
    setCommentsErr("");
    setCommentsBusy(false);
    setCommentsMoreBusy(false);
    setCommentsDone(false);
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
  }, [bgmSeasonKey, multiSeason]);

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
  }, [base.guid, data, bgmSeasonKey]);

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
  }, [tab, reviewsNonce, base.guid, bgmSeasonKey]);

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
  }, [tab, commentsNonce, base.guid, bgmSeasonKey]);

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
  }, [tab, moreNonce, base.guid, bgmSeasonKey]);

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
  }, [tab, relatedNonce, base.guid, bgmSeasonKey]);

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
            // [lc-1728] 多季：按当前季取演员表（TMDB season credits），缓存键用季 guid；
            //   单季保持剧级 aggregate credits 不变。
            seasonNumber: multiSeason ? seasonNo : undefined,
            cacheKey: multiSeason ? seasonGuid || base.guid : base.guid,
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
  }, [tab, creditsNonce, base.guid, data, bgmSeasonKey, seasonNo, tmdbAnchor.trimId, tmdbAnchor.tmdbId, tmdbAnchor.imdbId]);

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
    // [lc-1727/1728] 多季条目：Bangumi subject 只覆盖一季，按集号回填会把第一季的简介串到其它季。
    //   现在只有「BGM 命中当前季」（bgmMappingAllowed）才用 BGM desc 按集号回填；否则
    //   **仍然保留**按集取详情（get_item_detail，并发上限 3）—— 这是本季每一集自己的简介，
    //   不是第一季数据。旧版在这里直接 `return` 导致多季选集永远没有集简介，一并修掉。
    let alive = true;

    if (bgm?.subject_id && bgmMappingAllowed) {
      invoke<{ data?: BgmEpisodeRow[] }>("bgm_episodes", {
        id: bgm.subject_id,
        limit: 200,
      })
        .then((r) => {
          if (!alive) return;
          const rows = Array.isArray(r?.data) ? r.data : [];
          setBgmEps(rows); // [lc-1401] 右栏选集用它拿「第N话 小标题 + 单集简介」
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

    // 非「命中当前季」的 BGM：不要用上一季的 bgmEps 显示小标题/简介。
    setBgmEps([]);
    const missing = episodes.filter((ep) => !ep.overview);
    if (!missing.length) return;
    const queue = [...missing];
    const worker = async () => {
      while (alive && queue.length) {
        // 若本轮期间 BGM 恰好命中当前季，交给 BGM desc（避免重复请求）。
        if (bgmRef.current && bgmSeasonMatchedRef.current) return;
        const ep = queue.shift()!;
        try {
          const d = await invoke<Record<string, unknown>>("get_item_detail", {
            guid: ep.guid,
            seriesGuid: base.guid,
          });
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
  }, [episodes.length, bgm?.subject_id, bgmMappingAllowed, bgmSeasonKey, base.guid]);

  // [lc-1727] 当前选中季的季级元数据：飞牛 `season/list` 已随季返回 overview / poster /
  //   vote_average / release_date / local_number_of_episodes。多季剧集切季时，头部（简介 /
  //   封面 / 评分 / 首播日期）必须换成该季，而不是永远用剧根条目（飞牛把剧根当第一季聚合）。
  //   单季剧集 selSeason 也在，但值通常与剧根一致，不产生副作用。
  const selSeason = isSeries ? seasons[seasonIdx] : undefined;
  const seasonLabel = selSeason ? seasonLabelOf(selSeason, seasonIdx).no : "";
  const seasonOverview = String(selSeason?.overview ?? "").trim();
  const seasonPosterPath = String(selSeason?.poster ?? "").trim();
  const seasonDate = String(selSeason?.release_date || selSeason?.air_date || "").trim();

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
    // [lc-1727] 多季剧集：当前选中季的飞牛季封面优先（每季封面不同，切季必须换），
    //   其次才是 TMDB 剧级海报 / 剧根 posters / 列表页 base.poster。
    //   单季剧集保持原有优先级（TMDB 高清优先），避免无谓改变既有展示。
    (isSeries && seasons.length > 1 && seasonPosterPath ? imgUrl(seasonPosterPath) : "") ||
    (tmdbHeroPoster ? tmdbImg(tmdbHeroPoster, "w500") : "") ||
    pickLargestImg((data ?? {}).posters) ||
    (pick(data ?? {}, ["posters", "poster"]) as string) ||
    base.poster ||
    "";
  /* [单集封面兜底] 当前季 TMDB 剧照映射（集号 → still 文件路径）；没有就是空对象。 */
  const currentStillMap =
    seasonStills[
      `${Number(tmdbAnchor.tmdbId) || 0}:${Number(seasons[seasonIdx]?.season_number ?? seasonIdx + 1) || 1}`
    ] ?? {};
  /* [单集封面兜底] 剧集横图：TMDB heroBackdrop（面积最大）→ 飞牛 backdrops → 竖版 poster → 空。
     只用于「单集既无飞牛海报、TMDB 也没有该集剧照」时的缩略图兜底。 */
  const episodeThumbFallback = (() => {
    const tb = String((tmdb ?? {}).heroBackdrop ?? "");
    if (tb) return tmdbImg(tb, "w780");
    const fnb =
      pickLargestImg((data ?? {}).backdrops) ||
      String(pick(data ?? {}, ["backdrops", "backdrop"]) ?? "");
    if (fnb) return imgUrl(fnb);
    return poster ? imgUrl(poster) : "";
  })();
  // [lc-1507] 简介兜底链：飞牛详情 → 飞牛条目自带 → **Bangumi subject.summary**（发现页
  //   Bangumi 条目 TMDB 匹配不上时，简介不至于空白 —— bgmSub 进页面就拉了）
  const bgmSummary = String(pick(bgmSub ?? {}, ["summary"]) ?? "").trim();
  const overview = (() => {
    // [lc-1727] 多季剧集：优先当前季的季简介（飞牛 season/list 自带），切季即换；
    //   没有季简介才回退剧根详情 → 列表页 → Bangumi。单季保持原有链路。
    if (isSeries && seasons.length > 1 && seasonOverview) return seasonOverview;
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
  // [lc-1652] 电影版式重新需要通栏头图：TMDB heroBackdrop（面积最大）→ 飞牛 backdrops → 海报兜底
  // [lc-1662] 模糊根因：全屏背景曾回退到 **w500 竖版海报**（500px 拉到整屏必糊）。
  //   现在兜底链改为：TMDB 原图竖版海报 → BGM 大图 → 最后才用传入的小图；绝不拿 w500 当整屏底。
  const movieBackdrop = (() => {
    const tb = String((tmdb ?? {}).heroBackdrop ?? "");
    if (tb) return tmdbImg(tb, "original");
    const fnb =
      pickLargestImg((data ?? {}).backdrops) ||
      String(pick(data ?? {}, ["backdrops", "backdrop"]) ?? "");
    if (fnb) return imgUrl(fnb);
    if (tmdbHeroPoster) return tmdbImg(tmdbHeroPoster, "original");
    if (bgmCover) return bgmCover;
    return poster ? imgUrl(poster) : "";
  })();
  // [lc-1655] 电影剧照：只取后端 `stills4k`（已过滤 width>=3840 并按面积降序）。
  // [lc-1662] 不再截断为 6 张 —— 全部并入头图背景轮播（详情页「剧照」按钮已删除）。
  const movieStills: string[] = Array.isArray((tmdb ?? {}).stills4k)
    ? ((tmdb ?? {}).stills4k as unknown[])
        .map((p) => String(p ?? ""))
        .filter((p) => p.length > 0)
    : [];
  // 片长：飞牛详情 duration 是秒
  const runtimeMin = Math.round(Number(pick(data ?? {}, ["duration"]) ?? 0) / 60) || 0;
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
  //   [lc-1657] 「评论」「吐槽」只在**拿到 BGM 条目 id** 时才出现（用户要求：无 BGM id 的剧集一律隐藏）。
  //     判据与 ratingSubjectId 同源：bgm.subject_id 或 base.bgm_id，任一为有效正整数即视为有 BGM id；
  //     0 / undefined / null 一律视为无效。其余四个标签（详情/关联/角色/更多）顺序与行为不变。
  const hasBgmId =
    Number(bgm?.subject_id ?? 0) > 0 || Number(base.bgm_id ?? 0) > 0;
  const tabIds: string[] = hasBgmId
    ? ["overview", "reviews", "comments", "related", "more", "extra"]
    : ["overview", "related", "more", "extra"];
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
  // [lc-1658] TMDB 相似推荐（后端 recommendations，含 poster/backdrop/rating）→ 前端类型，
  //   供「关联」标签在 BGM 无关联时兜底渲染。
  const tmdbRecs: TmdbRecommendation[] = Array.isArray(tmdb?.recommendations)
    ? (tmdb!.recommendations as unknown[]).flatMap((r) => {
        if (!r || typeof r !== "object") return [];
        const o = r as Record<string, unknown>;
        const id = Number(o.tmdbId);
        if (!id) return [];
        return [{
          tmdbId: id,
          title: String(o.title ?? ""),
          year: String(o.year ?? ""),
          poster: String(o.poster ?? ""),
          backdrop: String(o.backdrop ?? ""),
          rating: Number(o.rating ?? 0) || 0,
        }];
      })
    : [];
  // [lc-1720] 本地手动元数据覆盖：评分/首播日期优先用用户选定的值（Rust 侧打 `_metadata_override` 标记）。
  const overrideActive = Boolean(pick(data ?? {}, ["_metadata_override"]));
  const overrideRating = Number(pick(data ?? {}, ["_override_rating"]) ?? 0) || 0;
  const overrideDate = String(pick(data ?? {}, ["_override_date"]) ?? "").trim();
  // [lc-1728] 多季：当前季自带的评分（飞牛 season/list 的 vote_average，来源即 TMDB 季评分）。
  //   BGM 未命中当前季时用它兜底，避免整页评分一直显示第一季/整部剧的分。
  const seasonVote = Number(selSeason?.vote_average ?? 0) || 0;
  const ratingScore =
    overrideActive && overrideRating > 0
      ? overrideRating
      : bgmScore > 0
        ? bgmScore
        : multiSeason && seasonVote > 0
          ? seasonVote
          : tmdbScore > 0
            ? tmdbScore
            : null;
  const ratingSource: "bgm" | "tmdb" | "none" =
    overrideActive && overrideRating > 0
      ? "tmdb"
      : bgmScore > 0
        ? "bgm"
        : multiSeason && seasonVote > 0
          ? "tmdb"
          : tmdbScore > 0
            ? "tmdb"
            : "none";
  const ratingTotal = bgmScore > 0 ? bgmTotal : tmdbVotes;
  const airDate =
    // [lc-1727] 多季剧集：首播日期随当前季变化（飞牛季条目的 release_date/air_date）。
    (isSeries && seasons.length > 1 && seasonDate ? seasonDate : "") ||
    (overrideActive && overrideDate ? overrideDate : "") ||
    String(pick(bgmSub ?? {}, ["date"]) ?? "").trim() ||
    String(pick(tmdb ?? {}, ["airDate"]) ?? "").trim();
  const ratingSubjectId = bgm?.subject_id ?? base.bgm_id ?? null;
  // [lc-1598] 「有观看记录」判据（给评分组件的状态按钮用）：条目级 watched/ts 有值，或任意一集有进度/已看。
  //   为 true 时 RatingWidget 会把「未追 / 想看」默认改成「在看」（用户要求）。
  // [lc-1728] 多季：剧根的 watched/ts 是「整部/第一季」口径，不能代表当前季；只认本季各集进度。
  const hasWatchRecord =
    (!multiSeason &&
      (Number(pick(data ?? {}, ["watched"]) ?? 0) === 1 ||
        Number(pick(data ?? {}, ["ts"]) ?? 0) > 0)) ||
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
  // [lc-1728] 多季：本季没有集播出日期时，回退当前季的首播日期，而不是剧根/Bangumi（第一季）日期。
  const lastAir = airDates.length
    ? airDates[airDates.length - 1]
    : multiSeason && seasonDate
      ? seasonDate
      : String(pick(bgmSub ?? {}, ["date"]) ?? "");
  // [lc-1728] 「已入库 X / 总数」的分母：多季时用**当前季**的集数，而不是 Bangumi 剧根 subject 的
  //   集数（那是第一季的数字，会把第 2 季显示成 "12 / 12" 之类的第一季口径）。
  const seasonLibraryTotal = (() => {
    if (!multiSeason) return totalEps;
    const local = Number(selSeason?.local_number_of_episodes ?? 0) || 0;
    const remote = Number(selSeason?.number_of_episodes ?? 0) || 0;
    // 拿不到本季集数时不回退 Bangumi 剧根（第一季）数字，宁可不显示分母。
    return local > 0 ? local : remote > 0 ? remote : 0;
  })();

  // [季/集与缺集检测] 「已入库」同一行的季/集补充信息。
  //   · 季号：飞牛 season_number（0 = 特别篇）；季总数 = seasons.length。
  //   · 本季应有集数：优先 season.number_of_episodes（飞牛季级元数据）；否则用 Bangumi
  //     subject 的 eps（totalEps）——Bangumi 一个 subject 通常只对应**一季**，多季条目里
  //     无法确定它描述的是哪一季，故仅在单季条目上采用；再否则用已入库集号的最大值
  //     （此时只能发现 1..最大值 之间的空档，不硬报最大值之后的缺集）。
  //   · 缺集：以本季应有集数为范围对比 episode_number 集合；小数集（如 1.5 的拆分/特别集）
  //     不参与整数集号判断，避免把 1 或 2 误报为缺失；0 起的连续编号识别为 0-based；
  //     特别篇（season_number=0）只报集数、不做缺集判断。
  const seasonEpInfo = (() => {
    if (!isSeries || noFnOS || episodes.length === 0) return null;
    const cur = seasons[seasonIdx];
    const nums = new Set<number>();
    for (const ep of episodes) {
      if (ep.episode_number == null || String(ep.episode_number).trim() === "") continue;
      const n = Number(ep.episode_number);
      // 只认整数集号；小数集不参与缺集比较（宁可少报，也不误报）。
      if (!Number.isInteger(n) || n < 0) continue;
      nums.add(n);
    }
    const seen = nums.size;
    const maxNum = seen ? Math.max(...nums) : 0;
    // 季级 total 与 Bangumi total 都必须不小于已见最大集号，否则是别季/整部的数字，不可用。
    // [lc-1727] 与卡片口径一致：优先 local_number_of_episodes（实际入库），再回退 number_of_episodes。
    const seasonTotalLocal = Number(cur?.local_number_of_episodes);
    const seasonTotalRemote = Number(cur?.number_of_episodes);
    const seasonTotalRaw =
      Number.isFinite(seasonTotalLocal) && seasonTotalLocal > 0 ? seasonTotalLocal : seasonTotalRemote;
    const seasonTotal =
      Number.isFinite(seasonTotalRaw) && seasonTotalRaw > 0 && (maxNum === 0 || seasonTotalRaw >= maxNum)
        ? Math.floor(seasonTotalRaw)
        : 0;
    // 多季条目的 Bangumi subject 只匹配其中一季，拿它当本季总集会误报 → 只信单季条目。
    const bgmTotal =
      seasons.length <= 1 && totalEps > 0 && (maxNum === 0 || totalEps >= maxNum) ? totalEps : 0;
    const expected = seasonTotal || bgmTotal || maxNum;
    const seasonNoRaw = Number(cur?.season_number);
    const special = Number.isFinite(seasonNoRaw) && seasonNoRaw === 0;

    const parts: string[] = [];
    if (cur) {
      const label = seasonLabelOf(cur, seasonIdx).no;
      parts.push(seasons.length > 1 ? `${label} / 共 ${seasons.length} 季` : label);
    }
    // 一个集号都拿不到（字段缺失/全是异常值）：不猜缺集，只标注未知。
    if (seen === 0) {
      if (expected > 0) parts.push(`共 ${expected} 集`);
      parts.push("集数未知");
      return { text: parts.join(" · "), title: "" };
    }
    if (expected <= 0) {
      parts.push("集数未知");
      return { text: parts.join(" · "), title: "" };
    }
    parts.push(`共 ${expected} 集`);
    // 特别篇没有 1..N 的语义：只报季/集数，不判断缺集。
    if (special) return { text: parts.join(" · "), title: "" };
    // 0 起编号：仅在「有 0、没有 expected、集号都 < expected」时按 0..expected-1 处理，
    // 避免把「特别篇 0 + 正片 1..N」误判成 0-based（歧义时宁可少报，不误报）。
    const zeroBased = nums.has(0) && !nums.has(expected) && maxNum < expected;
    const start = zeroBased ? 0 : 1;
    const end = zeroBased ? expected - 1 : expected;
    const missing: number[] = [];
    for (let n = start; n <= end; n++) if (!nums.has(n)) missing.push(n);
    if (!missing.length) {
      parts.push("无缺失");
      return { text: parts.join(" · "), title: "" };
    }
    const short = missing.length <= 5;
    parts.push(short ? `缺 ${missing.length} 集（第 ${missing.join("、")} 集）` : `缺 ${missing.length} 集`);
    return {
      text: parts.join(" · "),
      // 截断时用 title 给出完整列表；短列表也带上，方便悬停确认。
      title: `缺失集号：第 ${missing.join("、")} 集`,
    };
  })();
  /* [lc-1728] 多季条目的完结判定：Bangumi subject（第一季）的「放送结束」不代表本季。
     当本季集数齐全（seasonEpInfo 判为「无缺失」）时，也视为已完结。 */
  const seasonComplete = multiSeason && !!seasonEpInfo && seasonEpInfo.text.includes("无缺失");
  const seriesFinished = finished || seasonComplete;

  // [lc-1419] 续播目标：有播放记录（ts>0 且未看完）就跳那一集并带进度，
  //   否则从第 1 集开始。多集都有进度时取**集号最大**的那一集（最近在看的），
  //   不能按数组顺序取第一条，否则看完第 2 集后仍会显示「继续看第 1 集」。
  const resumeEp = (() => {
    // 集号可能缺失（飞牛部分条目只有顺序），用「episode_number ?? 数组下标+1」
    // 作为比较口径，否则第 2 集会按 0 参与比较，永远输给第 1 集。
    const candidates = episodes
      .map((e, i) => ({ e, num: e.episode_number ?? i + 1 }))
      .filter(({ e }) => (e.ts ?? 0) > 0 && !(e.duration && e.ts! >= e.duration));
    if (!candidates.length) return episodes[0];
    return candidates.reduce((best, cur) => (cur.num > best.num ? cur : best)).e;
  })();
  const resumeTarget = resumeEp ?? null;
  // 进度百分比（给按钮上的小字用）
  const resumePct =
    resumeTarget && resumeTarget.ts && resumeTarget.duration && resumeTarget.duration > 0
      ? Math.min(100, Math.round((resumeTarget.ts / resumeTarget.duration) * 100))
      : 0;

  // [lc-1401] 右栏选集：本机集号 → Bangumi 那集的 {小标题, 简介}（集号对齐 sort 优先，与回填逻辑一致）
  // [lc-1727/1728] 只有 BGM 命中当前季（bgmMappingAllowed）时才按集号映射；否则只用飞牛本季自带的
  //   集标题/简介，避免把第一季内容串到其它季。
  const bgmEpByNum = new Map<number, BgmEpisodeRow>();
  if (bgmMappingAllowed) {
    for (const e of bgmEps) {
      const num = Number.isInteger(e?.sort) ? (e.sort as number) : Number.isInteger(e?.ep) ? (e.ep as number) : NaN;
      if (Number.isFinite(num) && !bgmEpByNum.has(num)) bgmEpByNum.set(num, e);
    }
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

  /* [版本] 取某条目/集的版本列表（带内存缓存）：调 Rust `get_media_versions`，
     返回 video_streams + play/info 记住的 media_guid（与 FlyNarwhal 同口径）。 */
  const fetchMediaVersions = useCallback(async (guid: string) => {
    const cached = versionCacheRef.current.get(guid);
    if (cached) return cached;
    const r = await invoke<{ versions?: MediaVersion[]; selected_media_guid?: string }>(
      "get_media_versions",
      { guid },
    );
    const versions = (r?.versions ?? []).filter((v) => v && v.media_guid);
    const selected = chooseMediaVersion(guid, versions, r?.selected_media_guid);
    const out = { versions, selected };
    versionCacheRef.current.set(guid, out);
    return out;
  }, []);

  /** [版本] 用户切换版本：写本地记忆 + 更新缓存/当前选中（不立即播放）。 */
  const selectVersion = (guid: string, mediaGuid: string) => {
    if (!guid || !mediaGuid) return;
    saveMediaVersion(guid, mediaGuid);
    const cached = versionCacheRef.current.get(guid);
    versionCacheRef.current.set(guid, {
      versions: cached?.versions ?? (versionGuid === guid ? mediaVersions : []),
      selected: mediaGuid,
    });
    if (versionGuid === guid) setSelectedMediaGuid(mediaGuid);
  };

  /** [版本] 用指定版本起播（mediaGuid 为空 → 交给服务端默认版本）。 */
  const playWithVersion = (guid: string, mediaGuid?: string) => {
    setPlayingGuid(guid);
    invoke("play_item", { guid, mediaGuid: mediaGuid || undefined }).catch((e) => setErr(String(e)));
  };

  /** 选集（列表行 / 网格方块共用）：先查版本；多版本弹选择器，单版本直接播。 */
  const playEpisode = async (guid: string) => {
    // [选集定位] 用户已手动选集 → 放弃尚未执行的自动定位，不再把列表拉回去。
    autoLocateRef.current.userTouched = true;
    try {
      const { versions, selected } = await fetchMediaVersions(guid);
      if (versions.length > 1) {
        const ep = episodes.find((e) => e.guid === guid);
        const num = ep?.episode_number ?? (ep ? episodes.indexOf(ep) + 1 : 0);
        setVersionPicker({
          guid,
          label: ep ? `第 ${num} 集${ep.title ? ` · ${ep.title}` : ""}` : "本集",
          versions,
          selected,
        });
        return;
      }
      playWithVersion(guid, versions.length === 1 ? selected : undefined);
    } catch {
      // 版本接口失败不阻断播放：交给服务端默认版本。
      playWithVersion(guid);
    }
  };
  const startPlay = async () => {
    const ep = episodes[0];
    const guid = isSeries && ep ? ep.guid : base.guid;
    setPlaying(true);
    setPlayNote("正在打开独立播放器…");
    try {
      let mediaGuid: string | undefined;
      try {
        const r = await fetchMediaVersions(guid);
        mediaGuid = r.selected || undefined;
      } catch {
        mediaGuid = undefined;
      }
      await invoke("play_item", { guid, mediaGuid });
      setPlayNote("已在独立播放器窗口播放（可回到本页继续浏览）");
    } catch (e) {
      setPlayNote(String(e));
    } finally {
      setPlaying(false);
    }
  };

  /* [版本] 当前要展示版本盒子的目标：
     · 电影 = 条目本身；
     · 剧集 = 续播目标集（没有则第一集）—— 对齐 FlyNarwhal「剧集详情页按单集展示版本」。 */
  const versionTargetGuid = isMovie
    ? (noFnOS ? "" : base.guid)
    : (resumeTarget?.guid ?? episodes[0]?.guid ?? "");
  useEffect(() => {
    if (!dataReady || noFnOS || !versionTargetGuid) {
      setMediaVersions([]);
      setVersionGuid("");
      setSelectedMediaGuid("");
      return;
    }
    let alive = true;
    const req = ++versionReqRef.current;
    fetchMediaVersions(versionTargetGuid)
      .then(({ versions, selected }) => {
        if (!alive || req !== versionReqRef.current) return;
        setMediaVersions(versions);
        setVersionGuid(versionTargetGuid);
        setSelectedMediaGuid(selected);
      })
      .catch(() => {
        if (!alive || req !== versionReqRef.current) return;
        setMediaVersions([]);
        setVersionGuid(versionTargetGuid);
        setSelectedMediaGuid("");
      });
    return () => {
      alive = false;
    };
  }, [dataReady, noFnOS, versionTargetGuid, fetchMediaVersions]);


  return (
    <main className={`content detail-full${isMovie ? " mv-detail" : ""}`} onScroll={onDetailScroll}>
      {/* [lc-1401] 详情页按**用户定稿版式**重排（两栏）：
            左列：竖版封面 → 标签页 → 该页内容（[lc-1567] 标签/简介从首屏移入「详情」页）
            右列：集数据（本机集数 × Bangumi 集号对齐）的**整条竖向列表**
          ⚠ 上一版的模糊大头图（.dhero）**已按新稿删除**：稿里首屏就是"封面 + 两块信息框"，没有通栏头图。
          数据 effect 全部留在本组件顶层（不下沉到面板里，否则单集简介回填失效）。 */}
      <div className="dv2-rail" style={isMovie ? { display: "none" } : undefined}>
        <button className="back" onClick={onBack} title="返回上一页">
          ‹ 返回
        </button>
        {/* [lc-1403] 标题挪到返回键后面（用户要求）；原名（日文/英文）放标题后的括号里。
            原名优先 TMDB 的 original_name/original_title，其次 Bangumi 的 name —— 与当前显示名
            完全相同（或为空）时不显示括号，避免出现「进击的巨人（进击的巨人）」。 */}
        <h1 className="dv2-rail-title">
          {title || "（无标题）"}
          {/* [lc-1727] 多季剧集：标题后显示当前季，切季即时变化，明确"现在看的是第几季"。 */}
          {isSeries && seasons.length > 1 && seasonLabel && (
            <span className="dv2-rail-season"> · {seasonLabel}</span>
          )}
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
              setMetaTarget({
                guid: base.guid,
                title: title || base.title,
                type: isSeries ? "TV" : "Movie",
                seriesGuid: base.guid,
              })
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

      {/* [lc-1652] 电影：沉浸式头图 + 简介/演职员 + 剧照（用户样板） */}
      {dataReady && isMovie && (
        <MovieHero
          title={title}
          originalTitle={originalTitle}
          backdrop={movieBackdrop}
          poster={poster}
          rating={ratingScore}
          votes={ratingTotal}
          year={String(pick(tmdb ?? {}, ["year"]) ?? "").trim()}
          runtimeMin={runtimeMin}
          genres={genres}
          overview={overview}
          cast={cast}
          stills={movieStills}
          resumeAt={!!resumeAt}
          playing={playing}
          onPlay={startPlay}
          onBack={onBack}
          onMeta={() => setMetaTarget({ guid: base.guid, title: title || base.title, type: "Movie", seriesGuid: base.guid })}
          onOpenPerson={
            onOpenPerson
              ? (c) =>
                  onOpenPerson({
                    source: "tmdb",
                    id: Number(c.id ?? 0),
                    name: String(c.name ?? ""),
                    profile: c.profile ? String(c.profile) : undefined,
                  })
              : undefined
          }
          favItem={{
            guid: base.guid,
            title,
            tvTitle: "",
            poster: String(poster ?? ""),
            type: String(pick(data ?? {}, ["type"]) ?? base.type ?? ""),
            seasonNumber: 0,
            episodeNumber: 0,
          }}
          versions={
            versionGuid === base.guid
              ? mediaVersions.map((v, i) => ({
                  mediaGuid: v.media_guid,
                  label: versionLabels(mediaVersions)[i],
                }))
              : undefined
          }
          selectedVersion={versionGuid === base.guid ? selectedMediaGuid : undefined}
          onVersionChange={(mg) => selectVersion(base.guid, mg)}
        />
      )}

      {/* ══ 两栏主体（用户版式）══ */}
      {dataReady && (
      <div className={`dv2${isMovie ? " dv2-movie" : ""}`}>
        {/* ── 左列：封面 / 标签框 / 简介框 / 标签页 / 内容 ── */}
        <div className="dv2-left">
          {/* 竖版封面 + 标题（海报来自本机条目，无则用 Bangumi 的） */}
          <div className="dv2-top" style={isMovie ? { display: "none" } : undefined}>
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
            {/* [lc-1761] Switch 是独立开关按钮（不切标签页）：选中字体变色，右列切在线搜索 */}
            <button
              role="button"
              aria-pressed={onlineMode}
              className={`dseg-btn dv2-switch${onlineMode ? " switch-on" : ""}`}
              title="切换右列：本地选集 ↔ 在线资源搜索"
              onClick={() => setOnlineMode((v) => !v)}
            >
              Switch
            </button>
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
            /* [lc-1655] 电影页不再渲染剧照块（剧照改全屏轮播）；剧集页保持原行为 */
            showStills={!isMovie}
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
            tmdbRecs={tmdbRecs}
            tmdbDone={tmdbDone}
            mediaType={isSeries ? "tv" : "movie"}
            onOpenTmdb={onOpenTmdb}
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
            onOpenPerson={onOpenPerson}
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
        <aside className="dv2-right" style={isMovie ? { display: "none" } : undefined}>
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
          {/* [选季/更新时间重做] 头部不再是一整排 nowrap + overflow-x（旧版把季切换和
              「更新至」挤到右边界外，横向滚动条在 44px 高的头部里几乎看不见，于是
              "后面几季点不了"、"更新至被截断"）。
              现在分两层：第一层是两行信息（上行胶囊/按钮、下行纯文本，各自 flex-wrap），
              第二层是独立的选季轨道 —— 季多时横向滚动 + 左右箭头，窄窗自动换成原生下拉，永不裁切。 */}
          <div className="dv2e-hd">
            <div className="dv2e-hd-top">
              {/* [头部两行重排] 上面一行只放胶囊 / 按钮类控件：
                  播放胶囊 · 连载中/已完结状态胶囊 · 集数样式分段控件 · 网格/列表切换。
                  下面一行只放纯文本：已入库 6/7 集、更新至 YYYY-MM-DD。
                  两行各自 flex-wrap，窄窗自然换行，日期整体不拆。 */}
              {isSeries && (
                <div className="dv2e-hd-pills">
              {/* 播放入口（原来在右侧，现移到最左）。语义：
                  有播放记录 → 「继续看 第 N 集 · N%」；没有记录 → 「播放 第 1 集」。 */}
              {isSeries && resumeTarget && (
                <button
                  className="dv2e-play-btn"
                  disabled={playing}
                  onClick={() => {
                    // [版本] 续播沿用当前选中版本（未加载则交给服务端默认版本）。
                    const mg =
                      versionGuid === resumeTarget.guid
                        ? selectedMediaGuid
                        : versionCacheRef.current.get(resumeTarget.guid)?.selected;
                    playWithVersion(resumeTarget.guid, mg || undefined);
                  }}
                  title={
                    resumePct > 0
                      ? `从第 ${resumeTarget.episode_number ?? 1} 集的 ${resumePct}% 继续看`
                      : `继续看 第 ${resumeTarget.episode_number ?? 1} 集`
                  }
                >
                  <IcoPlayL size={14} strokeWidth={2.4} />
                  <span>
                    {playing
                      ? "打开中…"
                      : `继续看 第 ${resumeTarget.episode_number ?? 1} 集`}
                  </span>
                  {resumePct > 0 && <i className="dv2e-play-pct">{resumePct}%</i>}
                </button>
              )}
              {/* 状态胶囊属于控件行：与播放按钮、分段控件同高（30px）。 */}
              {isSeries && (
                <span className={`dv2e-state${seriesFinished ? " done" : " airing"}`}>
                  {seriesFinished ? "已完结" : "连载中"}
                </span>
              )}
              {/* 集数显示样式 + 视图切换（网格 / 列表）：都只影响展示，不影响点击/进度/已看逻辑。 */}
              {isSeries && (
                <div className="dv2e-tools">
                  <div className="ep-mode" role="group" aria-label="集数显示样式">
                    {EP_LABEL_MODES.map((m) => (
                      <button
                        key={m.key}
                        type="button"
                        className={`ep-mode-btn${epLabelMode === m.key ? " active" : ""}`}
                        title={m.hint}
                        aria-pressed={epLabelMode === m.key}
                        onClick={() => setEpLabelMode(m.key)}
                      >
                        {m.label}
                      </button>
                    ))}
                  </div>
                  <div className="dv2e-view" role="group" aria-label="选集视图">
                    <button
                      type="button"
                      className={`dv2e-view-btn${epView === "grid" ? " active" : ""}`}
                      title="网格视图"
                      aria-label="网格视图"
                      aria-pressed={epView === "grid"}
                      onClick={() => setEpView("grid")}
                    >
                      <IcoGridL size={15} strokeWidth={2} />
                    </button>
                    <button
                      type="button"
                      className={`dv2e-view-btn${epView === "list" ? " active" : ""}`}
                      title="列表视图"
                      aria-label="列表视图"
                      aria-pressed={epView === "list"}
                      onClick={() => setEpView("list")}
                    >
                      <IcoListL size={15} strokeWidth={2} />
                    </button>
                  </div>
                </div>
              )}
                </div>
              )}
              {/* 下面一行：纯文本信息（不参与控件行的高度与对齐）。 */}
              {isSeries ? (
                <div className="dv2e-hd-meta">
                  {/* [已入库] 本机集数 = 已入库集数，Bangumi 总集数仍作为分母。
                      · 发现页条目（noFnOS）没有本机库，标签回退为「总集数」，计数口径不变。 */}
                  <h3 className="dv2e-hd-h">{noFnOS ? "总集数" : "已入库"}</h3>
                  <span className="dv2e-meta-n">
                    {/* [lc-1444] 发现页条目没有本机集数：显示 Bangumi 总集数（若有），否则 "—" */}
                    {noFnOS
                      ? totalEps > 0
                        ? totalEps
                        : "—"
                      : `${episodes.length}${seasonLibraryTotal > 0 && seasonLibraryTotal !== episodes.length ? ` / ${seasonLibraryTotal}` : ""}`}{" "}
                    集
                  </span>
                  {/* [季/集与缺集检测] 季号 / 本季集数 / 缺集结论（纯文本，与「更新至」同档，
                      不抢上方胶囊控件的视觉重量；窄窗由 CSS 允许换行）。 */}
                  {seasonEpInfo && (
                    <span className="dv2e-meta-season" title={seasonEpInfo.title || undefined}>
                      {seasonEpInfo.text}
                    </span>
                  )}
                  {lastAir && <span className="dv2e-meta-d">更新至 {String(lastAir).slice(0, 10)}</span>}
                </div>
              ) : (
                /* 非剧集（发现页电影等）没有集数可入库，保留原「选集」标题占位 */
                <h3 className="dv2e-hd-h">选集</h3>
              )}
            </div>
            {/* [版本] 同一剧集多版本：照抄 FlyNarwhal 的版本盒子（仅 video_streams > 1 显示）。
                默认选中 = play/info 记住的版本；点选即写本地记忆，播放时按选中版本拉流。 */}
            {isSeries && versionGuid && mediaVersions.length > 1 && (
              <div className="dv2e-versions" role="group" aria-label="选择版本">
                <span className="dv2e-versions-h">版本</span>
                {mediaVersions.map((v, i) => {
                  const label = versionLabels(mediaVersions)[i];
                  return (
                  <button
                    key={v.media_guid}
                    type="button"
                    className={`dv2e-version${v.media_guid === selectedMediaGuid ? " active" : ""}`}
                    title={label}
                    aria-pressed={v.media_guid === selectedMediaGuid}
                    onClick={() => selectVersion(versionGuid, v.media_guid)}
                  >
                    {label}
                  </button>
                  );
                })}
              </div>
            )}
            {isSeries && seasons.length > 1 && (
              <div className="dv2e-seasons">
                <div className="season-tabs" ref={seasonRailRef} role="tablist" aria-label="选择季">
                  {seasons.map((sn, i) => {
                    const sl = seasonLabelOf(sn, i);
                    return (
                      <button
                        key={sn.guid}
                        type="button"
                        role="tab"
                        aria-selected={i === seasonIdx}
                        className={`season-tab ${i === seasonIdx ? "active" : ""}`}
                        title={`${sl.no}${sl.name ? ` · ${sl.name}` : ""}`}
                        onClick={() => {
                          // [选集定位] 用户手动切季 → 不再自动定位（尊重用户选择）。
                          autoLocateRef.current.userTouched = true;
                          setSeasonIdx(i);
                        }}
                      >
                        <span className="season-no">{sl.no}</span>
                        {sl.name && <span className="season-name">{sl.name}</span>}
                      </button>
                    );
                  })}
                </div>
                {/* 窄窗兜底：原生下拉，季再多也能完整选到（CSS 在 ≤720px 时替换轨道）。 */}
                <select
                  className="season-select"
                  aria-label="选择季"
                  value={seasonIdx}
                  onChange={(e) => {
                    // [选集定位] 用户手动切季 → 不再自动定位。
                    autoLocateRef.current.userTouched = true;
                    setSeasonIdx(Number(e.target.value));
                  }}
                >
                  {seasons.map((sn, i) => {
                    const sl = seasonLabelOf(sn, i);
                    return (
                      <option key={sn.guid} value={i}>
                        {sl.no}
                        {sl.name ? ` · ${sl.name}` : ""}
                      </option>
                    );
                  })}
                </select>
              </div>
            )}
          </div>
          <div
            ref={epListRef}
            className={`dv2e-list${isSeries && epView === "grid" ? " dv2e-list-grid" : ""}`}
            /* [选集定位] 用户一旦手动滚动/触摸/点击，就放弃尚未执行的自动定位。 */
            onWheel={() => {
              autoLocateRef.current.userTouched = true;
            }}
            onTouchStart={() => {
              autoLocateRef.current.userTouched = true;
            }}
            onPointerDown={() => {
              autoLocateRef.current.userTouched = true;
            }}
          >
            {/* [lc-1759/1761] Switch 模式（独立按钮触发）：右列整体切为在线资源搜索
                （本地有资源也可切换到在线）。开关未开时给指引而不是硬挂面板。 */}
            {onlineMode ? (
              webrsOn ? (
                <WebrsPanel
                  title={title || base.title}
                  kind={
                    Number(base.bgm_id ?? 0) > 0 || Number(bgm?.subject_id ?? 0) > 0
                      ? undefined
                      : "maccms"
                  }
                />
              ) : (
                <div className="empty">
                  在线资源搜索未开启 —— 到「设置 → 播放器」打开「在线资源搜索」开关后，再用 Switch 切换
                </div>
              )
            ) : !isSeries ? (
              <div className="empty">本条目不是剧集</div>
            ) : episodes.length === 0 && seasons.length === 0 && !noFnOS ? (
              /* [lc-1420] **分块骨架**：条目详情（data）到得很快，所以整页骨架一闪就没了；
                 但季/集是另外两条请求 —— 它们没回来之前右栏是空的，用户看着就是"还在等数据"。
                 这里单独给右栏摆骨架（用真实行高，数据到位时不跳）。 */
              epView === "grid" ? (
                <div className="dv2e-grid" aria-hidden="true">
                  {Array.from({ length: 12 }, (_, i) => (
                    <div className="dv2e-cell dv2e-cell-sk" key={`skg${i}`} />
                  ))}
                </div>
              ) : (
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
              )
            ) : episodes.length === 0 ? (
              /* [lc-1444] 发现页条目没有飞牛选集数据。
                 [lc-1500] 无源条目的"看"法 = 在线资源面板（Kazumi 规则源 → mpv）。
                 [lc-1747] lc-1627 曾把这里换成「未查询到资源」占位，现随功能恢复换回面板。
                 [lc-1753] 面板改由设置开关控制（默认关闭，声明「不存储不分发资源」后开启）：
                 开启时无资源的条目（发现页无源 / 库内暂无剧集）都挂面板；关闭维持占位。 */
              epsLoading ? (
                <div className="empty">加载中…</div>
              ) : webrsOn ? (
                /* [lc-1754→1758] 按编号分源：无编号（TMDB 剧集/电影）→ 只挂采集源；
                   有 BGM 编号（番剧）→ 不过滤 = Kazumi 规则源 + 采集源都挂
                   （采集站也收录动漫，多一路兜底；规则源只有番剧没有剧集）。 */
                <WebrsPanel
                  title={title || base.title}
                  kind={
                    Number(base.bgm_id ?? 0) > 0 || Number(bgm?.subject_id ?? 0) > 0
                      ? undefined
                      : "maccms"
                  }
                />
              ) : noFnOS ? (
                <div className="empty">未查询到资源</div>
              ) : (
                <div className="empty">暂无剧集</div>
              )
            ) : epView === "grid" ? (
              /* 数字方块网格（用户截图样式）：每格一个集号，当前集 accent 高亮。
                 集数样式（1 2 3 / 第N集 / S01E02）作用于格内文本；已看弱化 + 勾标，
                 有进度时底部画细进度条。右键元数据入口与列表模式一致。 */
              <div className="dv2e-grid" data-label={epLabelMode} role="list" aria-label="选集">
                {episodes.map((ep, i) => {
                  const num = ep.episode_number ?? i + 1;
                  const seasonNo =
                    Number(ep.season_number ?? seasons[seasonIdx]?.season_number ?? seasonIdx + 1) || 1;
                  const epLabel = epLabelOf(num, seasonNo, epLabelMode);
                  const bgmEp = bgmEpByNum.get(num);
                  const sub = String(bgmEp?.name_cn || bgmEp?.name || "").trim();
                  const progress =
                    ep.ts && ep.duration && ep.duration > 0 ? Math.min(100, (ep.ts / ep.duration) * 100) : 0;
                  const seen = Number(ep.watched) === 1 || progress >= 80;
                  const isPlaying = playingGuid === ep.guid;
                  return (
                    <div
                      key={ep.guid}
                      role="listitem"
                      tabIndex={0}
                      data-ep-guid={ep.guid}
                      data-ep-num={String(num)}
                      className={`dv2e-cell${isPlaying ? " playing" : ""}${seen ? " seen" : ""}`}
                      title={`播放 ${epLabel}`}
                      onClick={() => playEpisode(ep.guid)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          playEpisode(ep.guid);
                        }
                      }}
                    >
                      <span className="dv2e-cell-no">{epLabel}</span>
                      {seen && (
                        <span className="dv2e-cell-mark" title="已看">
                          <IcoCheckL size={10} strokeWidth={2.4} />
                        </span>
                      )}
                      {progress > 0 && (
                        <span className="dv2e-cell-prog" aria-hidden="true">
                          <i style={{ width: `${progress}%` }} />
                        </span>
                      )}
                      <button
                        className="dv2e-cell-meta"
                        title="本集元数据（编辑 / 手动匹配 / 解除匹配 / 刷新）"
                        onClick={(e) => {
                          e.stopPropagation();
                          setMetaTarget({
                            guid: ep.guid,
                            title: `第 ${num} 集${ep.title ? `. ${ep.title}` : sub ? `. ${sub}` : ""}`,
                            type: "TV",
                            seriesGuid: base.guid,
                            seasonNumber: ep.season_number,
                            episodeNumber: ep.episode_number ?? num,
                            isEpisode: true,
                          });
                        }}
                      >
                        ⋯
                      </button>
                    </div>
                  );
                })}
              </div>
            ) : (
              episodes.map((ep, i) => {
                const num = ep.episode_number ?? i + 1;
                // 集号样式只影响展示文本；seasonNo 优先取本集自带的季号。
                const seasonNo = Number(ep.season_number ?? seasons[seasonIdx]?.season_number ?? seasonIdx + 1) || 1;
                const epLabel = epLabelOf(num, seasonNo, epLabelMode);
                const bgmEp = bgmEpByNum.get(num);
                const sub = String(bgmEp?.name_cn || bgmEp?.name || "").trim();
                const desc = String(ep.overview || bgmEp?.desc || "").trim();
                const progress =
                  ep.ts && ep.duration && ep.duration > 0 ? Math.min(100, (ep.ts / ep.duration) * 100) : 0;
                // [lc-1596] 「已看」门槛：本地进度 **≥80%** 即视为已观看（原为 95% —— 用户要求
                //   「观看进度超过 80 后打上已观看的标记」）。飞牛自己的 `watched=1` 仍然优先。
                const seen = Number(ep.watched) === 1 || progress >= 80;
                const playing = playingGuid === ep.guid;
                /* [单集封面兜底] 优先级：飞牛单集海报 → TMDB 该集剧照 → 剧集横图 → 剧集竖版海报 → 空占位。
                   整条链路都走 imgUrl（tmdbImg 内部已走 fntvimg 代理，imgUrl 幂等直通）。 */
                const thumb = ep.poster
                  ? imgUrl(ep.poster)
                  : currentStillMap[num]
                    ? imgUrl(tmdbImg(currentStillMap[num], "w300"))
                    : episodeThumbFallback;
                return (
                  <div
                    key={ep.guid}
                    data-ep-guid={ep.guid}
                    data-ep-num={String(num)}
                    className={`dv2e-row${playing ? " playing" : ""}${seen ? " seen" : ""}`}
                    onClick={() => playEpisode(ep.guid)}
                    title={`播放 ${epLabel}`}
                  >
                    <div className="dv2e-thumb">
                      {thumb ? (
                        <img src={thumb} loading="lazy" alt="" draggable={false} />
                      ) : (
                        <div className="poster-empty">
                          <IcoClapL size={18} strokeWidth={1.8} />
                        </div>
                      )}
                      {/* 左下角操作区：播放按钮 + 「已看」徽标同一行、垂直居中。
                          徽标从右上角移到这里，紧跟播放按钮右侧。 */}
                      <div className="dv2e-actions">
                        <span className="dv2e-play">{playing ? "◉" : "▶"}</span>
                        {seen && <span className="ep-mark seen"><IcoCheckL size={11} strokeWidth={2.2} /> 已看</span>}
                        {!seen && progress > 0 && <span className="ep-mark now">看到 {Math.round(progress)}%</span>}
                      </div>
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
                            seriesGuid: base.guid,
                            seasonNumber: ep.season_number,
                            episodeNumber: ep.episode_number ?? num,
                            isEpisode: true,
                          });
                        }}
                      >
                        ⋯
                      </button>
                      {progress > 0 && (
                        <div className="progress">
                          <div style={{ width: `${progress}%` }} />
                        </div>
                      )}
                    </div>
                    <div className="dv2e-main">
                      <div className="dv2e-t">
                        <span className="dv2e-name"><span className="dv2e-epno">{epLabel}</span>{ep.title ? `. ${ep.title}` : sub ? `. ${sub}` : ""}</span>
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

      {/* [版本] 点「多版本」的某一集：先选版本再播（照抄飞鲸详情页的版本盒子）。
          单版本的集不会弹这个层，直接播放。 */}
      {versionPicker &&
        createPortal(
          <div className="ddet-mask" onClick={() => setVersionPicker(null)}>
            <div className="sum-modal version-picker" onClick={(e) => e.stopPropagation()}>
              <div className="sum-modal-h">
                <h3>选择版本{versionPicker.label ? ` · ${versionPicker.label}` : ""}</h3>
                <button
                  className="ddet-close sum-modal-x"
                  title="关闭（Esc）"
                  onClick={() => setVersionPicker(null)}
                >
                  <IcoXL size={15} strokeWidth={1.8} />
                </button>
              </div>
              <div className="sum-modal-b">
                <div className="dv2e-versions version-picker-boxes" role="group" aria-label="选择版本">
                  {versionPicker.versions.map((v, i) => (
                    <button
                      key={v.media_guid}
                      type="button"
                      className={`dv2e-version${v.media_guid === versionPicker.selected ? " active" : ""}`}
                      title={versionLabels(versionPicker.versions)[i]}
                      aria-pressed={v.media_guid === versionPicker.selected}
                      onClick={() => {
                        const guid = versionPicker.guid;
                        selectVersion(guid, v.media_guid);
                        setVersionPicker(null);
                        playWithVersion(guid, v.media_guid);
                      }}
                    >
                      <span className="dv2e-version-label">{versionLabels(versionPicker.versions)[i]}</span>
                      {(v.width ?? 0) > 0 && (v.height ?? 0) > 0 && (
                        <span className="dv2e-version-sub">
                          {v.width}×{v.height}
                          {(v.bit_depth ?? 0) > 0 ? ` · ${v.bit_depth}bit` : ""}
                          {v.codec_name ? ` · ${v.codec_name}` : ""}
                        </span>
                      )}
                    </button>
                  ))}
                </div>
              </div>
            </div>
          </div>,
          document.body,
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
