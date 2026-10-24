/* [lc-1615/lc-1616] 观影记录页 —— 样式照抄原项目（Electron 版 watchHistory.ts 的 .wh-* 面板：
   大标题 + 筛选胶囊 + 「观影活跃度」统计卡 + GitHub 风热力图 + 「已看完/在观看」双栏海报墙），
   配色/材质落到本仓库 tokens（深浅主题通用，见 src/styles/37-history.css）。
   数据 = get_watch_history（飞牛 play/list ∪ 本机播放历史，Rust 侧合并去重 + **按作品聚合**）。

   [lc-1616] 两处升级：
     ① 按「部」聚合：Rust 侧 aggregate_by_series 把「集」条目并成作品级卡片（一剧一卡）；
     ② 评分与评语：点卡打开详情浮层（照抄原项目观感）—— 海报 + 元数据 + TMDB/BGM 现有评分
        + 我的 5 星评分/评语（持久化到 Rust 侧 ratings.json）+ 播放记录 sessions。 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CalendarDays as IcoCalL,
  Clock as IcoClockL,
  LayoutGrid as IcoGridL,
  RefreshCw as IcoRefreshL,
  Rows3 as IcoRowsL,
  Sparkles as IcoSparklesL,
} from "lucide-react";
import { imgUrl } from "../../lib/images";
import { useRefresh } from "../../hooks/useRefresh";
import { MediaItem } from "../../types";
import { WatchReport } from "./WatchReport";

/** 一条作品播放记录（详情浮层「播放记录」用；Rust aggregate_by_series 产出） */
export type HistorySession = {
  playedAt?: number;
  episode?: string;
  progress?: number;
};

/** get_watch_history 返回的统一条目（camelCase；Rust history.rs 组装）—— [lc-1616] 作品级 */
export type HistoryItem = {
  /** 作品键（剧 guid / 电影自身 guid / 在线源 `webrs:<url>`） */
  guid: string;
  title: string;
  /** 最近一次观看的集号标签（"S2E1"；电影为空） */
  episode?: string;
  /** 看过几集（不同集号数；电影 0） */
  episodeCount?: number;
  /** 海报（NAS 相对路径；空 = 用渐变兜底） */
  poster?: string;
  tmdbId?: string;
  bgmId?: string;
  /** Movie / Episode / ...；在线源固定 "webrs" */
  type?: string;
  /** 最近观看时刻（ms；飞牛条目无时间戳时为 0 → 显示「未记录时间」） */
  playedAt?: number;
  /** "fnos"（库内）| "webrs"（在线源） */
  source?: string;
  /** 在线源 = 规则名 */
  sourceName?: string;
  parentGuid?: string;
  /** 剧 guid（富化补全；点击进详情用） */
  seriesGuid?: string;
  /** 最近一次观看进度 0-1（飞牛 play/list 有；本机记录为 0） */
  progress?: number;
  /** [lc-1631] 条目总时长（ms；0 = 未知）—— 年度报告按「进度 × 总时长」折算观看量 */
  durationMs?: number;
  /** [lc-1631] 是否看完 */
  finished?: boolean;
  /** 该作品的播放记录（时间倒序） */
  sessions?: HistorySession[];
  /** 我的评分 0-5（0/缺省 = 未评） */
  myRating?: number;
  /** 我的评语 */
  myReview?: string;
};

/** [lc-1668] 热力图的一天（count = 当天去重作品数；同一部剧一天只算一次） */
export type HistoryHeatDay = { y: number; m0: number; d: number; count: number };

type HistoryResp = {
  items?: HistoryItem[];
  fnosOk?: boolean;
  fnosError?: string;
  /** [lc-1668] 日级台账（播放过就算；在线源已搁置不计入） */
  heat?: HistoryHeatDay[];
};

/* [progress] 播放进度上报已提升为全局 hook（`src/hooks/usePlaybackProgress.ts`，
   在 App.tsx 顶层无条件挂载）——本页只负责展示/读取，不再监听播放器事件，
   避免双份上报。 */

/* [lc-1747] 在线源恢复显示（lc-1667 曾随功能搁置而隐藏）：重新提供「在线源」筛选。
   注：数据侧（get_watch_history 的 visible_in_history 过滤）如仍排除 webrs，
   该筛选会计数为 0 —— 需 Rust 侧同口径放开后才有条目。 */
type Filter = "全部" | "电影" | "剧集" | "在线源";

const FILTERS: Filter[] = ["全部", "电影", "剧集", "在线源"];

/* ── [lc-1620] 展示方式：海报网格（默认）/ 紧凑列表 / 时间线 ──
   口径照抄发现页「媒体库」左栏 TmdbPane 的三视图（timeline/grid/list）+ `.dlib-vbtn` 切换钮。
   选择记 localStorage：观影记录页是高频回访页，用户切过一次就该记住（与仓库既有
   `fntvSidebarActions.collapsed` / `anibox-guest` 等键同一套做法）。 */
type ViewMode = "grid" | "list" | "timeline";
const VIEW_KEY = "fntv-history-view";

/** 读取已保存的展示方式；非法/首次一律回落到「海报网格」（默认视图，一屏放最多）。 */
/* [lc-1770] 数据源开关（右侧两个 toggle）：在线源 / 个人视频 是否参与展示。
   独立于分类胶囊（胶囊管看哪类，开关管某类数据整块显隐），选择记 localStorage。 */
const TOGGLE_WBRS_KEY = "fntv-history-show-webrs";
const TOGGLE_PERSONAL_KEY = "fntv-history-show-personal";
function readToggle(key: string, def = true): boolean {
  try {
    const v = localStorage.getItem(key);
    if (v === "0") return false;
    if (v === "1") return true;
  } catch { /* 隐私模式：回落默认 */ }
  return def;
}

function readView(): ViewMode {
  try {
    const v = localStorage.getItem(VIEW_KEY);
    if (v === "grid" || v === "list" || v === "timeline") return v;
  } catch {
    /* 隐私模式等拿不到 localStorage：静默回落，不阻断页面 */
  }
  return "grid";
}

/** 时间线分组桶（今天 / 昨天 / 本周 / 更早；无时间戳归「更早」） */
type TimeBucket = "今天" | "昨天" | "本周" | "更早";
const BUCKET_ORDER: TimeBucket[] = ["今天", "昨天", "本周", "更早"];

function bucketOf(ts?: number): TimeBucket {
  if (!ts || ts <= 0) return "更早";
  const d = new Date(ts);
  d.setHours(0, 0, 0, 0);
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const days = Math.round((today.getTime() - d.getTime()) / 86400000);
  if (days <= 0) return "今天";
  if (days === 1) return "昨天";
  if (days < 7) return "本周";
  return "更早";
}

/** 条目 → 筛选分类（在线源按 source；其余按类型：Movie=电影，其它归剧集） */
function catOf(it: HistoryItem): Exclude<Filter, "全部"> {
  if (it.source === "webrs") return "在线源";
  return it.type === "Movie" ? "电影" : "剧集";
}

/** 相对时间（口径照抄原项目 formatAgo；无时间戳 → 未记录时间） */
function fmtAgo(ts?: number): string {
  if (!ts || ts <= 0) return "未记录时间";
  const mins = Math.floor((Date.now() - ts) / 60000);
  if (mins < 1) return "刚刚";
  if (mins < 60) return `${mins} 分钟前`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs} 小时前`;
  const days = Math.floor(hrs / 24);
  if (days < 30) return `${days} 天前`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months} 个月前`;
  return `${Math.floor(months / 12)} 年前`;
}

/** 绝对时间（详情浮层「播放记录」用；无时间戳 → 未记录时间） */
function fmtAbs(ts?: number): string {
  if (!ts || ts <= 0) return "未记录时间";
  const d = new Date(ts);
  const p = (n: number) => (n < 10 ? `0${n}` : `${n}`);
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 无海报时的渐变兜底（照抄原项目 ART_PALETTE：按名称 hash 稳定取色） */
const ART_PALETTE = [
  "#3a2a1a,#120c08", "#3a1a2a,#140810", "#1a2a3a,#081018",
  "#2a1a3a,#100818", "#2a2a1a,#101008", "#10283a,#04101c",
  "#2a2410,#100c04", "#3a2010,#140a04", "#3a1010,#140404",
  "#1a2a2a,#080810", "#2a1a1a,#100808", "#102a2a,#04100c",
];
function artForName(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = ((h << 5) - h + name.charCodeAt(i)) | 0;
  const [a, b] = ART_PALETTE[Math.abs(h) % ART_PALETTE.length].split(",");
  return `linear-gradient(145deg, ${a}, ${b})`;
}

/* ── 星星（照抄原项目 STAR_PATH + 金色渐变） ── */
const STAR_PATH =
  "M12 17.27L18.18 21l-1.64-7.03L22 9.24l-7.19-.61L12 2 9.19 8.63 2 9.24l5.46 4.73L5.82 21z";
function Stars({ value, size = 13 }: { value: number; size?: number }) {
  return (
    <span className="hist-stars" style={{ ["--hist-star" as string]: `${size}px` }}>
      {[1, 2, 3, 4, 5].map((i) => (
        <svg key={i} viewBox="0 0 24 24" className={i <= value ? "on" : "off"}>
          <path d={STAR_PATH} />
        </svg>
      ))}
    </span>
  );
}

/* ── 热力图（GitHub 贡献图口径，照抄原项目：列=周、行=星期，53 周 = 过去一年） ── */

type HeatCell = { date: Date; count: number; future: boolean };

const HEAT_CELL = 12; // 与 37-history.css 的 --hist-hcell 同步
const HEAT_GAP = 4;
const HEAT_WEEKS = 53;

function localKey(d: Date): string {
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}
const pad2 = (n: number) => (n < 10 ? `0${n}` : `${n}`);
const fmtDay = (d: Date) => `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
/** 颜色档位（照抄原项目）：0=空 1=1部 2=2~3 3=4~5 4=6+ */
const heatLevel = (c: number) => (c <= 0 ? 0 : c === 1 ? 1 : c <= 3 ? 2 : c <= 5 ? 3 : 4);

function buildHeat(days: HistoryHeatDay[]) {
  const counts = new Map<string, number>();
  for (const b of days) {
    if (b.count > 0) counts.set(`${b.y}-${b.m0}-${b.d}`, b.count);
  }
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const end = new Date(today);
  end.setDate(end.getDate() + (6 - end.getDay())); // 对齐到周六（本周尾）
  const start = new Date(end);
  start.setDate(start.getDate() - (HEAT_WEEKS * 7 - 1)); // 起点 = 周日
  const weeks: HeatCell[][] = [];
  const cursor = new Date(start);
  for (let w = 0; w < HEAT_WEEKS; w++) {
    const col: HeatCell[] = [];
    for (let dow = 0; dow < 7; dow++) {
      const future = cursor > today;
      col.push({ date: new Date(cursor), count: future ? 0 : counts.get(localKey(cursor)) ?? 0, future });
      cursor.setDate(cursor.getDate() + 1);
    }
    weeks.push(col);
  }
  // 月份标签：月份变化时打一个，隔一个打（避免 53 周挤满，照抄原项目口径）
  const months: { left: number; text: string }[] = [];
  let lastMonth = -1;
  let skip = 0;
  weeks.forEach((wk, wi) => {
    const m = wk[0].date.getMonth();
    if (m !== lastMonth) {
      if (skip % 2 === 0) months.push({ left: wi * (HEAT_CELL + HEAT_GAP), text: `${m + 1}月` });
      lastMonth = m;
      skip++;
    }
  });
  const total = [...counts.values()].reduce((s, n) => s + n, 0);
  return { weeks, months, total, activeDays: counts.size };
}

const WEEKDAY_LABELS = ["日", "一", "二", "三", "四", "五", "六"];

export function WatchHistoryView({
  onBack,
  onOpenItem,
}: {
  onBack: () => void;
  /** 库内条目点击 → 走现有详情链路（App.tsx 负责 setDetailItem + view 切换） */
  onOpenItem: (item: MediaItem) => void;
}) {
  const [data, setData] = useState<{
    items: HistoryItem[];
    fnosOk: boolean;
    fnosError: string;
    heat: HistoryHeatDay[];
  } | null>(null);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const [filter, setFilter] = useState<Filter>("全部");
  /** [lc-1620] 展示方式（默认海报网格；从 localStorage 恢复） */
  const [view, setView] = useState<ViewMode>(readView);
  /** [lc-1770] 数据源开关：在线源 / 个人视频 是否展示（默认都展示） */
  const [showWebrs, setShowWebrs] = useState(() => readToggle(TOGGLE_WBRS_KEY));
  const [showPersonal, setShowPersonal] = useState(() => readToggle(TOGGLE_PERSONAL_KEY));
  const [note, setNote] = useState("");
  /** 我的评分/评语（作品键 → 值）；从 items 初始化，保存后就地更新，卡片星星随之刷新 */
  const [myRatings, setMyRatings] = useState<Record<string, { score: number; review: string }>>({});
  /** 详情浮层当前作品（null = 关闭） */
  const [detail, setDetail] = useState<HistoryItem | null>(null);
  /** [lc-1631] 年度报告浮层是否打开 */
  const [reportOpen, setReportOpen] = useState(false);
  const noteTimer = useRef<number | null>(null);
  const heatWrapRef = useRef<HTMLDivElement | null>(null);
  const heatScrollRef = useRef<HTMLDivElement | null>(null);
  const [tip, setTip] = useState<{ x: number; y: number; text: string } | null>(null);

  function showNote(msg: string) {
    setNote(msg);
    if (noteTimer.current) window.clearTimeout(noteTimer.current);
    noteTimer.current = window.setTimeout(() => setNote(""), 2600);
  }

  const load = useCallback(async (isRefresh = false) => {
    if (isRefresh) setBusy(true);
    setErr("");
    try {
      const r = await invoke<HistoryResp>("get_watch_history");
      const list = Array.isArray(r?.items) ? r.items : [];
      setData({
        items: list,
        fnosOk: !!r?.fnosOk,
        fnosError: String(r?.fnosError ?? ""),
        heat: Array.isArray(r?.heat) ? r.heat : [],
      });
      // 用后端带出的评分初始化本地表（避免逐条 get_my_rating 往返）
      const map: Record<string, { score: number; review: string }> = {};
      for (const it of list) {
        if (it.guid && (it.myRating || it.myReview)) {
          map[it.guid] = { score: it.myRating ?? 0, review: it.myReview ?? "" };
        }
      }
      setMyRatings(map);
      if (isRefresh) showNote(`已刷新：共 ${list.length} 部作品`);
    } catch (e) {
      setErr(String(e));
    } finally {
      if (isRefresh) setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  /* [lc-1640] 顶栏 ⟳：复用本页已有的「立即同步」路径（load(true)）—— get_watch_history 无 Rust 缓存，
     本机记录 + 飞牛 play/list 每次都是实拉，天然就是强制刷新；load(true) 还会给页内 toast 与自身按钮收尾。 */
  const { tick: refreshTick, reportBusy } = useRefresh();
  const lastRefreshTick = useRef(refreshTick);
  useEffect(() => {
    if (refreshTick === lastRefreshTick.current) return;
    lastRefreshTick.current = refreshTick;
    reportBusy("history", true);
    void load(true).finally(() => reportBusy("history", false));
  }, [refreshTick, load, reportBusy]);

  // 卸载时清掉 toast 定时器（防泄漏）
  useEffect(() => () => {
    if (noteTimer.current) window.clearTimeout(noteTimer.current);
  }, []);

  const items = data?.items ?? [];
  const heatDays = data?.heat;
  const counts = useMemo(() => {
    const c: Record<Filter, number> = { 全部: items.length, 电影: 0, 剧集: 0, 在线源: 0 };
    for (const it of items) c[catOf(it)]++;
    return c;
  }, [items]);

  const filtered = useMemo(() => {
    let out = filter === "全部" ? items : items.filter((it) => catOf(it) === filter);
    // [lc-1770] 数据源开关：独立于分类胶囊，整块显隐（判定口径与 Rust 侧个人视频一致）
    if (!showWebrs) out = out.filter((it) => it.source !== "webrs");
    if (!showPersonal)
      out = out.filter((it) => it.type !== "Video" && it.type !== "Directory");
    return out;
  }, [items, filter, showWebrs, showPersonal]);

  // 双栏拆分（照抄原项目：已看完 = 进度≥1；其余 = 在观看）
  const done = useMemo(() => filtered.filter((it) => (it.progress ?? 0) >= 1), [filtered]);
  const watching = useMemo(() => filtered.filter((it) => (it.progress ?? 0) < 1), [filtered]);

  /** [lc-1620] 时间线视图的分组（今天/昨天/本周/更早；空桶不渲染） */
  const groups = useMemo(() => {
    const map: Record<TimeBucket, HistoryItem[]> = { 今天: [], 昨天: [], 本周: [], 更早: [] };
    for (const it of filtered) map[bucketOf(it.playedAt)].push(it);
    return BUCKET_ORDER.map((k) => ({ key: k, rows: map[k] })).filter((g) => g.rows.length > 0);
  }, [filtered]);

  /** 切换展示方式并落盘（写失败不影响当前会话） */
  const pickView = useCallback((v: ViewMode) => {
    setView(v);
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      /* 忽略写入失败 */
    }
  }, []);

  const stats = useMemo(() => {
    const now = new Date();
    const monthStart = new Date(now.getFullYear(), now.getMonth(), 1).getTime();
    const dayMs = 86400000;
    const thirtyAgo = now.getTime() - 30 * dayMs;
    // [lc-1668] 近30天活跃天数改看**播放活动台账**：只要播放过就算一天，
    // 不再受「作品清单」过滤（个人视频/在线源）与「只保留最近一次」的影响。
    const activeDays = new Set<string>();
    for (const b of heatDays ?? []) {
      const d = new Date(b.y, b.m0, b.d);
      if (d.getTime() >= thirtyAgo) activeDays.add(`${b.y}-${b.m0}-${b.d}`);
    }
    let monthCount = 0;
    for (const it of items) {
      const ts = it.playedAt ?? 0;
      if (ts <= 0) continue;
      if (ts >= monthStart) monthCount++;
    }
    const total = items.length;
    const doneN = items.filter((it) => (it.progress ?? 0) >= 1).length;
    return {
      activeDays: activeDays.size,
      monthCount,
      total,
      doneRate: total ? Math.round((doneN / total) * 100) : 0,
      doneN,
      watchingN: total - doneN,
    };
  }, [items, heatDays]);

  const heat = useMemo(() => buildHeat(heatDays ?? []), [heatDays]);

  /* 热力图默认滚到最右（最近日期）。卡片宽度通常放不下 53 周，不滚的话
     用户看到的是去年的月份，会误以为数据/时间截止到过去。 */
  useEffect(() => {
    const el = heatScrollRef.current;
    if (!el) return;
    const id = requestAnimationFrame(() => {
      el.scrollLeft = el.scrollWidth;
    });
    return () => cancelAnimationFrame(id);
  }, [heat]);

  /** 卡片点击 → 打开详情浮层（[lc-1616] 不再直接跳详情；浮层内「查看详情」再走详情链路） */
  const handleOpen = (it: HistoryItem) => setDetail(it);

  /** 浮层「查看详情」：库内 → 详情链路；在线源 → 提示 */
  const handleViewDetail = (it: HistoryItem) => {
    if (it.source === "webrs") {
      showNote("在线源条目暂无可打开的详情页 —— 可在对应作品的「在线资源」面板中继续播放");
      return;
    }
    setDetail(null);
    onOpenItem({
      guid: it.seriesGuid || it.guid,
      title: it.title,
      poster: it.poster || undefined,
      // 剧集有剧 guid → TV（完整季/集详情）；缺剧 guid 时退化为集条目形态（仍能打开，不会空等选集）
      type: it.type === "Movie" ? "Movie" : it.seriesGuid ? "TV" : "Episode",
    });
  };

  const loading = data === null && !err;

  return (
    <main className="content hist-full">
      <div className="page-head hist-head">
        <button className="back" onClick={onBack} title="返回首页">
          ‹
        </button>
        <h3 className="hist-title">观影记录</h3>
        <span className="muted">{data ? `${items.length} 部` : ""}</span>
        {/* [lc-1631] 年度报告入口（照原项目：面板顶栏一个按钮） */}
        <button className="hist-report-btn" onClick={() => setReportOpen(true)} title="生成本年度观影报告">
          <IcoSparklesL size={14} strokeWidth={2.2} />
          年度报告
        </button>
        <button className="hist-sync" onClick={() => void load(true)} disabled={busy || loading}>
          <IcoRefreshL size={13} strokeWidth={2.2} className={busy ? "hist-sync-spin" : ""} />
          {busy ? "同步中…" : "立即同步"}
        </button>
      </div>

      {err && (
        <div className="err">
          加载失败：{err}
          <button className="hist-retry" onClick={() => void load()}>
            重试
          </button>
        </div>
      )}

      {loading && (
        <>
          <div className="hist-skel-stats" />
          <div className="hist-grid hist-skel-grid">
            {Array.from({ length: 10 }).map((_, i) => (
              <div className="hist-skel" key={i} />
            ))}
          </div>
        </>
      )}

      {!loading && data && (
        <>
          {/* 筛选胶囊（照抄原项目右上角「全部/电影/剧集/动漫」→ 本仓库分类：+在线源；计数 = 作品数）
              + [lc-1620] 右侧展示方式切换（照抄发现页 .dlib-vbtn 口径） */}
          <div className="hist-pills">
            {FILTERS.map((f) => (
              <button
                key={f}
                className={`hist-pill${filter === f ? " active" : ""}`}
                onClick={() => setFilter(f)}
              >
                {f} ({counts[f]})
              </button>
            ))}
            {/* [lc-1770] 数据源开关：在线源 / 个人视频 是否展示（on=展示，点击切换；持久化） */}
            <button
              className={`hist-pill hist-toggle${showWebrs ? "" : " off"}`}
              aria-pressed={showWebrs}
              title="点击切换：观影记录里是否展示在线源（第三方源）播放的条目"
              onClick={() => {
                const v = !showWebrs;
                setShowWebrs(v);
                try { localStorage.setItem(TOGGLE_WBRS_KEY, v ? "1" : "0"); } catch { /* ignore */ }
              }}
            >
              在线源
            </button>
            <button
              className={`hist-pill hist-toggle${showPersonal ? "" : " off"}`}
              aria-pressed={showPersonal}
              title="点击切换：观影记录里是否展示个人视频（本地 Video/目录条目）"
              onClick={() => {
                const v = !showPersonal;
                setShowPersonal(v);
                try { localStorage.setItem(TOGGLE_PERSONAL_KEY, v ? "1" : "0"); } catch { /* ignore */ }
              }}
            >
              个人视频
            </button>
            <div className="hist-views" role="group" aria-label="展示方式">
              <button
                className={`hist-vbtn${view === "grid" ? " on" : ""}`}
                title="海报网格"
                onClick={() => pickView("grid")}
              >
                <IcoGridL size={16} strokeWidth={2} />
              </button>
              <button
                className={`hist-vbtn${view === "list" ? " on" : ""}`}
                title="紧凑列表"
                onClick={() => pickView("list")}
              >
                <IcoRowsL size={16} strokeWidth={2} />
              </button>
              <button
                className={`hist-vbtn${view === "timeline" ? " on" : ""}`}
                title="时间线"
                onClick={() => pickView("timeline")}
              >
                <IcoCalL size={16} strokeWidth={2} />
              </button>
            </div>
          </div>

          {!data.fnosOk && (
            <div className="hist-warn">
              飞牛记录暂不可用{data.fnosError ? `（${data.fnosError}）` : ""} —— 当前仅显示本机播放记录
            </div>
          )}

          {items.length === 0 && (heatDays?.length ?? 0) === 0 ? (
            <div className="hist-empty">
              <IcoClockL size={26} strokeWidth={1.6} />
              <div className="hist-empty-t">暂无观影记录</div>
              <div className="hist-empty-s">
                {data.fnosOk ? "播放过的内容会自动记录在这里（库内 + 在线源）" : "登录后还会并入飞牛影视的观看记录"}
              </div>
            </div>
          ) : (
            <>
              {/* 顶部两栏：左=观影活跃度统计卡，右=热力图卡（照抄原项目 wh-chart-split 版式） */}
              <section className="hist-chart-split">
                <div className="hist-stats">
                  <div className="hist-stats-title">观影活跃度</div>
                  <div className="hist-stats-sub">
                    共 {stats.total} 部 · 已看完 {stats.doneN} · 在看 {stats.watchingN} · {stats.activeDays} 天/30天活跃
                  </div>
                  <div className="hist-stat-row">
                    <div>
                      <div className="n"><b>{stats.activeDays}</b><i>天</i></div>
                      <span>近30天</span>
                    </div>
                    <div>
                      <div className="n"><b>{stats.monthCount}</b><i>部</i></div>
                      <span>本月</span>
                    </div>
                    <div>
                      <div className="n"><b>{stats.total}</b><i>部</i></div>
                      <span>累计</span>
                    </div>
                    <div>
                      <div className="n"><b>{stats.doneRate}</b><i>%</i></div>
                      <span>看完率</span>
                    </div>
                  </div>
                </div>

                <div className="hist-heat">
                  <div className="hist-heat-head">
                    <div className="hist-heat-title-wrap">
                      <div className="hist-heat-title">观影活动热力图</div>
                      <div className="hist-heat-range">过去一年 · {heat.activeDays} 天有观看</div>
                    </div>
                    <div className="hist-heat-legend">
                      少
                      <span className="hist-hcell l1" />
                      <span className="hist-hcell l2" />
                      <span className="hist-hcell l3" />
                      <span className="hist-hcell l4" />
                      多
                    </div>
                  </div>
                  <div className="hist-heat-body" ref={heatWrapRef}>
                    <div className="hist-heat-days">
                      {WEEKDAY_LABELS.map((d, i) => (
                        <span key={d} className={i === 1 || i === 3 || i === 5 ? "show" : ""}>
                          {d}
                        </span>
                      ))}
                    </div>
                    <div className="hist-heat-scroll" ref={heatScrollRef}>
                      <div className="hist-heat-inner">
                        <div className="hist-heat-months">
                          {heat.months.map((m) => (
                            <span key={m.left} style={{ left: m.left }}>
                              {m.text}
                            </span>
                          ))}
                        </div>
                        <div className="hist-heat-cols">
                          {heat.weeks.map((wk, wi) => (
                            <div className="hist-heat-week" key={wi}>
                              {wk.map((cell) => {
                                const lv = cell.future ? -1 : heatLevel(cell.count);
                                return (
                                  <div
                                    key={fmtDay(cell.date)}
                                    className={`hist-hcell${cell.future ? " future" : lv > 0 ? ` l${lv}` : ""}`}
                                    data-cnt={cell.count}
                                    onMouseEnter={(e) => {
                                      const wrap = heatWrapRef.current;
                                      if (!wrap) return;
                                      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                                      const wr = wrap.getBoundingClientRect();
                                      setTip({
                                        x: r.left - wr.left + r.width / 2,
                                        y: r.top - wr.top - 6,
                                        text: `${fmtDay(cell.date)} · ${cell.count > 0 ? `${cell.count} 部作品` : "未观看"}`,
                                      });
                                    }}
                                    onMouseLeave={() => setTip(null)}
                                  />
                                );
                              })}
                            </div>
                          ))}
                        </div>
                      </div>
                    </div>
                    {tip && (
                      <div className="hist-heat-tip show" style={{ left: tip.x, top: tip.y }}>
                        {tip.text}
                      </div>
                    )}
                  </div>
                </div>
              </section>

              {/* 影视清单：三种展示方式（[lc-1620] 替换原「已看完/在观看」双栏大卡）
                  —— 默认海报网格（一屏放最多）；切换后换 key 重挂载 ⇒ 条目依次入场（照抄 TmdbPane contentKey 手法）。 */}
              <section className="hist-section">
                <div className="hist-section-head">
                  <div className="hist-section-title">影视清单</div>
                  <div className="hist-section-hint">
                    {done.length} 部已看完 · {watching.length} 部在看 · 点击查看详情并评分
                  </div>
                </div>

                {filtered.length === 0 ? (
                  <div className="hist-col-empty">该分类下暂无作品</div>
                ) : (
                  <div className="hist-content" key={`${filter}|${view}`}>
                    {view === "grid" ? (
                      <div className="hist-grid">
                        {filtered.map((it, i) => (
                          <HistoryCard
                            key={it.guid}
                            it={it}
                            rating={myRatings[it.guid]?.score ?? 0}
                            onOpen={handleOpen}
                            delay={i}
                          />
                        ))}
                      </div>
                    ) : view === "list" ? (
                      <div className="hist-lv">
                        {filtered.map((it, i) => (
                          <HistoryRow
                            key={it.guid}
                            it={it}
                            rating={myRatings[it.guid]?.score ?? 0}
                            onOpen={handleOpen}
                            delay={i}
                          />
                        ))}
                      </div>
                    ) : (
                      <div className="hist-tl">
                        {groups.map((g, gi) => (
                          <div className="hist-grp" key={g.key} style={{ animationDelay: `${Math.min(gi, 6) * 0.04}s` }}>
                            <div className="hist-ghd">
                              <span className="hist-gpill">
                                <IcoCalL size={13} strokeWidth={2.2} />
                                {g.key}
                              </span>
                              <span className="hist-gcnt">{g.rows.length} 部作品</span>
                            </div>
                            <div className="hist-rows">
                              {g.rows.map((it, i) => (
                                <HistoryRow
                                  key={it.guid}
                                  it={it}
                                  rating={myRatings[it.guid]?.score ?? 0}
                                  onOpen={handleOpen}
                                  delay={i}
                                />
                              ))}
                            </div>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </section>
            </>
          )}
        </>
      )}

      {detail && (
        <HistoryDetail
          it={detail}
          rating={myRatings[detail.guid] ?? { score: 0, review: "" }}
          onClose={() => setDetail(null)}
          onViewDetail={() => handleViewDetail(detail)}
          onSaved={(v) => {
            setMyRatings((m) => ({ ...m, [detail.guid]: v }));
            // 同步回 data（卡片/统计若读取 items 上的评分也保持一致）
            setData((d) =>
              d
                ? {
                    ...d,
                    items: d.items.map((x) =>
                      x.guid === detail.guid ? { ...x, myRating: v.score, myReview: v.review } : x,
                    ),
                  }
                : d,
            );
          }}
          onNote={showNote}
        />
      )}

      {/* [lc-1631] 年度报告浮层（作品级事实复用已加载的 items，避免再拉一次飞牛） */}
      {reportOpen && <WatchReport items={items} onClose={() => setReportOpen(false)} />}

      {note && <div className="hist-toast show">{note}</div>}

      {/* 我的评分金星渐变（供 CSS fill: url(#hist-gold) 引用；照抄原项目金色渐变） */}
      <svg width="0" height="0" style={{ position: "absolute" }} aria-hidden="true">
        <defs>
          <linearGradient id="hist-gold" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stopColor="#ffe27a" />
            <stop offset="1" stopColor="#ffb300" />
          </linearGradient>
        </defs>
      </svg>
    </main>
  );
}

/* ── 海报卡（照抄原项目 .wh-card：2:3 封面 + 底部渐隐 + 星星/名称/副行 + 进度条 + 来源角标） ── */

function HistoryCard({
  it,
  rating,
  onOpen,
  delay = 0,
}: {
  it: HistoryItem;
  rating: number;
  onOpen: (it: HistoryItem) => void;
  /** [lc-1620] 入场错峰延迟（按索引；与发现页网格一致，最多封顶 11 档） */
  delay?: number;
}) {
  const pct = Math.round(Math.min(1, Math.max(0, it.progress ?? 0)) * 100);
  // [lc-1616] 副行 = 最新一集 / 看过几集 · 进度 · 最近观看时间
  const epPart = it.episode || (it.episodeCount && it.episodeCount > 0 ? `看过 ${it.episodeCount} 集` : "");
  const sub = [epPart, pct > 0 && pct < 100 ? `${pct}%` : "", fmtAgo(it.playedAt)]
    .filter(Boolean)
    .join(" · ");
  const isWebrs = it.source === "webrs";
  return (
    <div
      className="hist-card"
      onClick={() => onOpen(it)}
      title={isWebrs && it.sourceName ? `在线源：${it.sourceName}` : it.title}
      style={{ animationDelay: `${Math.min(delay, 11) * 0.03}s` }}
    >
      <div
        className="hist-art"
        style={
          it.poster
            ? { backgroundImage: `url(${imgUrl(it.poster)})` }
            : { background: artForName(it.title || it.guid) }
        }
      />
      <div className="hist-scrim" />
      <span className={`hist-badge${isWebrs ? " webrs" : ""}`}>{isWebrs ? "在线源" : "飞牛"}</span>
      {pct > 0 && pct < 100 && (
        <div className="hist-pbar">
          <div className="hist-pfill" style={{ width: `${pct}%` }} />
        </div>
      )}
      <div className="hist-meta">
        {rating > 0 && <Stars value={rating} />}
        <div className="hist-name">{it.title || "未知作品"}</div>
        <div className="hist-sub">{sub}</div>
      </div>
    </div>
  );
}

/* ── [lc-1620] 紧凑行（紧凑列表 / 时间线视图共用：小封面 + 标题/进度 + 最近时间 + 评分） ──
   一屏能扫十几条，是对「一横展示太少」的第二种解法（网格是横向多放，行是纵向多扫）。 */

function HistoryRow({
  it,
  rating,
  onOpen,
  delay = 0,
}: {
  it: HistoryItem;
  rating: number;
  onOpen: (it: HistoryItem) => void;
  delay?: number;
}) {
  const pct = Math.round(Math.min(1, Math.max(0, it.progress ?? 0)) * 100);
  const epPart = it.episode || (it.episodeCount && it.episodeCount > 0 ? `看过 ${it.episodeCount} 集` : "");
  const sub = [epPart, pct > 0 ? `${pct}%` : ""].filter(Boolean).join(" · ") || "暂无进度";
  const isWebrs = it.source === "webrs";
  return (
    <div
      className="hist-lrow"
      onClick={() => onOpen(it)}
      title={isWebrs && it.sourceName ? `在线源：${it.sourceName}` : it.title}
      style={{ animationDelay: `${Math.min(delay, 9) * 0.035}s` }}
    >
      <div
        className="hist-lposter"
        style={
          it.poster
            ? { backgroundImage: `url(${imgUrl(it.poster)})` }
            : { background: artForName(it.title || it.guid) }
        }
      />
      <div className="hist-linfo">
        <div className="hist-lhead">
          <span className="hist-ltitle">{it.title || "未知作品"}</span>
          <span className={`hist-lbadge${isWebrs ? " webrs" : ""}`}>{isWebrs ? "在线源" : "飞牛"}</span>
        </div>
        <div className="hist-lsub">{sub}</div>
      </div>
      <div className="hist-lright">
        {rating > 0 ? <Stars value={rating} size={12} /> : <span className="hist-lnone">未评</span>}
        <span className="hist-ltime">{fmtAgo(it.playedAt)}</span>
      </div>
    </div>
  );
}

/* ── 详情浮层（照抄原项目 .wh-detail：左海报 6fr / 右内容 4fr） ── */

type TmdbInfo = {
  rating?: number;
  votes?: number;
  year?: string;
  genres?: string[];
  overview?: string;
  episodes?: number;
};
type BgmInfo = {
  rating?: { score?: number; total?: number };
  date?: string;
  eps?: number;
  name_cn?: string;
  name?: string;
};

function HistoryDetail({
  it,
  rating,
  onClose,
  onViewDetail,
  onSaved,
  onNote,
}: {
  it: HistoryItem;
  rating: { score: number; review: string };
  onClose: () => void;
  onViewDetail: () => void;
  onSaved: (v: { score: number; review: string }) => void;
  onNote: (msg: string) => void;
}) {
  const [score, setScore] = useState(rating.score);
  const [review, setReview] = useState(rating.review);
  const [saving, setSaving] = useState(false);
  // [lc-1616] 可选：顺带把「看过」状态同步到 BGM（默认关闭；仓库无写 BGM 评分的命令，
  //   只能借已有的 bgm_collect_set 标状态 —— 绝不擅自改用户 BGM 数据）。
  const [syncBgm, setSyncBgm] = useState(false);
  const [tmdb, setTmdb] = useState<TmdbInfo | null>(null);
  const [bgm, setBgm] = useState<BgmInfo | null>(null);

  // 打开时按需取「现有评分」：TMDB（tmdb_show_details，落盘缓存 120 天）+ Bangumi（bgm_subject）。
  // 都是仓库已有命令，不新增接口；失败静默（对应栏显示「暂无」）。
  useEffect(() => {
    let alive = true;
    if (it.tmdbId) {
      invoke<TmdbInfo>("tmdb_show_details", {
        tmdbId: Number(it.tmdbId),
        mediaType: it.type === "Movie" ? "movie" : "tv",
        title: it.title,
      })
        .then((r) => alive && setTmdb(r ?? null))
        .catch(() => {});
    }
    if (it.bgmId) {
      invoke<BgmInfo>("bgm_subject", { id: Number(it.bgmId) })
        .then((r) => alive && setBgm(r ?? null))
        .catch(() => {});
    }
    return () => {
      alive = false;
    };
  }, [it.tmdbId, it.bgmId, it.type, it.title]);

  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const save = async () => {
    setSaving(true);
    try {
      await invoke("set_my_rating", { key: it.guid, score, review });
      // 可选的 BGM 同步（默认关）：仅当用户显式勾选且该作品有 BGM 条目时执行。
      if (syncBgm && it.bgmId) {
        try {
          // 评分意味着看过 → 借已有 bgm_collect_set 标记为「看过」(kind=2)；已绑定则顺手同步。
          await invoke("bgm_collect_set", { subjectId: Number(it.bgmId), kind: 2 });
          onNote("已保存并同步到 Bangumi（标记为看过）");
        } catch {
          onNote("已保存；Bangumi 同步失败（可稍后重试）");
        }
      } else {
        onNote("已保存你的评价");
      }
      onSaved({ score, review });
    } catch (e) {
      onNote(`保存失败：${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  const typeLabel = it.type === "Movie" ? "电影" : it.source === "webrs" ? "在线源" : "剧集";
  const year = String(tmdb?.year ?? "").trim() || String(bgm?.date ?? "").slice(0, 4);
  const genres = tmdb?.genres ?? [];
  const totalEps = Number(tmdb?.episodes ?? 0) || Number(bgm?.eps ?? 0) || 0;
  const tmdbScore = Number(tmdb?.rating ?? 0);
  const tmdbVotes = Number(tmdb?.votes ?? 0);
  const bgmScore = Number(bgm?.rating?.score ?? 0);
  const bgmTotal = Number(bgm?.rating?.total ?? 0);
  const sessions = it.sessions ?? [];

  const fmtVotes = (v: number) => (v <= 0 ? "" : v >= 10000 ? `${(v / 10000).toFixed(1)} 万人评` : `${v} 人评`);

  return (
    <div className="hist-overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="hist-detail">
        <div className="hist-detail-hero">
          {it.poster ? (
            <img className="hist-detail-poster" src={imgUrl(it.poster)} alt="" />
          ) : (
            <div className="hist-detail-poster fallback" style={{ background: artForName(it.title || it.guid) }} />
          )}
          <div className="hist-detail-scrim" />
        </div>
        <div className="hist-detail-body">
          <button className="hist-detail-close" onClick={onClose} title="关闭（Esc）">
            ✕
          </button>
          <div className="hist-detail-name">{it.title || "未知作品"}</div>
          <div className="hist-detail-meta">
            <span>{typeLabel}</span>
            {year && <span>{year}</span>}
            {totalEps > 0 && <span>共 {totalEps} 集</span>}
            {it.episodeCount && it.episodeCount > 0 ? <span>已看 {it.episodeCount} 集</span> : null}
          </div>
          {genres.length > 0 && (
            <div className="hist-detail-chips">
              {genres.map((g) => (
                <span className="hist-detail-chip" key={g}>
                  {g}
                </span>
              ))}
            </div>
          )}

          {/* 三栏评分：TMDB / Bangumi / 我的（互不覆盖） */}
          <div className="hist-ratings">
            <div className="hist-rating">
              <div className="hist-rating-label">TMDB</div>
              <div className="hist-rating-score">{tmdbScore > 0 ? tmdbScore.toFixed(1) : "暂无"}</div>
              <div className="hist-rating-count">{fmtVotes(tmdbVotes) || "—"}</div>
            </div>
            <div className="hist-rating">
              <div className="hist-rating-label">Bangumi</div>
              <div className="hist-rating-score">{bgmScore > 0 ? bgmScore.toFixed(1) : "暂无"}</div>
              <div className="hist-rating-count">{fmtVotes(bgmTotal) || "—"}</div>
            </div>
            <div className="hist-rating mine">
              <div className="hist-rating-label">我的评分</div>
              <div className="hist-rating-score mine-score">{score > 0 ? `${score}.0` : "未评"}</div>
              <div className="hist-rating-count">{score > 0 ? "★".repeat(score) : "—"}</div>
            </div>
          </div>

          {tmdb?.overview && <div className="hist-detail-overview">{tmdb.overview}</div>}

          <div className="hist-detail-divider" />
          <div className="hist-detail-label">我的评分</div>
          <div className="hist-rate">
            {[1, 2, 3, 4, 5].map((i) => (
              <button
                key={i}
                className={`hist-rate-star${i <= score ? " on" : ""}`}
                onClick={() => setScore(score === i ? 0 : i)}
                title={`${i} 星`}
                type="button"
              >
                <svg viewBox="0 0 24 24">
                  <path d={STAR_PATH} />
                </svg>
              </button>
            ))}
          </div>
          <textarea
            className="hist-review"
            value={review}
            placeholder="写下你对这部作品的看法…"
            onChange={(e) => setReview(e.target.value)}
          />
          {it.bgmId && (
            <label className="hist-sync-bgm">
              <input type="checkbox" checked={syncBgm} onChange={(e) => setSyncBgm(e.target.checked)} />
              同步到 Bangumi（标记为看过）
            </label>
          )}
          <div className="hist-detail-actions">
            <button className="hist-btn primary" onClick={() => void save()} disabled={saving}>
              {saving ? "保存中…" : "保存我的评价"}
            </button>
            <button className="hist-btn ghost" onClick={onViewDetail}>
              查看详情
            </button>
          </div>

          <div className="hist-detail-divider" />
          <div className="hist-sessions">
            <div className="hist-detail-label">播放记录</div>
            {sessions.length > 0 ? (
              sessions.slice(0, 30).map((s, i) => {
                const p = Math.round(Math.min(1, Math.max(0, s.progress ?? 0)) * 100);
                return (
                  <div className="hist-sess" key={i}>
                    <span>{fmtAbs(s.playedAt)}</span>
                    <span className="pos">
                      {[s.episode, p > 0 ? `${p}%` : ""].filter(Boolean).join(" · ") || "—"}
                    </span>
                  </div>
                );
              })
            ) : (
              <div className="hist-sess">
                <span>暂无分段记录</span>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
