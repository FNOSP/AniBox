/* [lc-2078] 发现页短剧栏（占位转真）—— 数据来自 guoapp「红果鉴」Go 核心（duanju_core.dll，
   上游 GPL-3.0，与本项目 AGPL-3.0 兼容；Rust 侧 FFI 桥见 src-tauri/src/duanju.rs）。
   UI：站源 chips（[lc-2180] 起动态渲染：guoapp v2 架构站源 = 随包 JS 模块，
   可用集合与 adult 声明由核心回传）+ 分类 chips + 搜索 + 竖版海报网格；
   点卡片 → 详情浮层（简介 + 选集 30/组）→ 点集 duanju_play（核心本地流直链 → mpv）。
   NOT IMPLEMENTED：推荐/榜单 Tab、弹幕、追剧、下载、多清晰度切换、Linux/macOS 核心。 */

import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { DState, Dsk } from "./shared";
import { djImg } from "../../lib/images";
import { adultSourcesEnabled, isAdultSource, onAdultSourcesChange } from "../../lib/adultSources";
import { SoraniLoginModal } from "./SoraniLoginModal";
import { DuanjuLoading } from "./DuanjuLoading";
import { Star as IcoStar, X as IcoXL } from "lucide-react";

type DuanjuDrama = {
  id: string;
  source?: string;
  sourceId?: string;
  title: string;
  description?: string;
  cover?: string;
  episodes?: number;
  category?: string;
  vip?: boolean | null;
  heat?: string;
  views?: string;
  score?: string;
  tags?: string[];
  releaseStatus?: string;
  badge?: string;
  [k: string]: unknown;
};

type DuanjuChapter = {
  id: string;
  title?: string;
  videoUrl?: string;
  currentEpisode?: unknown;
  mediaSize?: number;
  [k: string]: unknown;
};

type CatalogResp = {
  items?: DuanjuDrama[];
  hasMore?: boolean;
  page?: number;
  warning?: string;
  localSearch?: boolean;
  /** [lc-2234] 仅 `duanju_cached` 返回：缓存是否在 15 分钟有效期内 */
  fresh?: boolean;
};
type DetailResp = { drama?: DuanjuDrama; chapters?: DuanjuChapter[]; warning?: string };

/** [lc-2123] 短剧独立观看记录（存 app_data/duanju-history.json，与软件主记录完全分开）。
 *  index = 0 基集索引；pos/dur = 秒；at = 毫秒时间戳（用于排序）。 */
type DjHistory = {
  drama: { id: string; title: string; cover?: string; source?: string; episodes?: number };
  index: number;
  pos: number;
  dur: number;
  at: number;
};

/** [lc-2180] 站源能力（duanju_init 回传的 sources 列表）。
 *  guoapp v2 架构起站源 = JS 模块（随包部署到数据目录），可用集合随包变化，
 *  不再写死；adult 为上游模块 manifest 的声明，驱动 18 禁确认闸门。 */
type DjSource = {
  id: string;
  name: string;
  available?: boolean;
  adult?: boolean;
  [k: string]: unknown;
};

/** 兜底站源表：duanju_init 没回 sources 时用（老核心 / 核心缺失），id 与顺序照旧；
 *  adult 与旧 ADULT_SOURCES 口径一致（红果为公开短剧内容不拦）。 */
const FALLBACK_SOURCES: DjSource[] = [
  { id: "hongguo", name: "红果", adult: false },
  { id: "jinguo", name: "剧果", adult: true },
  { id: "jinguoduanju", name: "禁果短剧", adult: true },
  { id: "yeguo", name: "野果", adult: true },
  { id: "heitao", name: "黑桃", adult: true },
  { id: "huangdou", name: "黄豆", adult: true },
];

/** ── [lc-2189] 站源健康：目录加载失败的站点从 chips 剔除（后台探测，localStorage 持久化）。
 *   · 切标签闪旧表：sources 清单也缓存，首帧直接用上次结果（无缓存才显示骨架）；
 *   · 探测：每源无筛选目录首页拉一次，成功清零、失败计数 +1（多源并行，30 分钟 TTL，
 *     期间切标签不再重探）；
 *   · 剔除的源留「重试」入口（站点恢复后可手动拉回）。 ── */
type SrcHealth = Record<string, { fail: number; at: number }>;
const SRC_CACHE_KEY = "anibox-dj-sources-v1";
const SRC_HEALTH_KEY = "anibox-dj-src-health-v1";
const HEALTH_TTL = 30 * 60 * 1000;

function readSrcCache(): DjSource[] | null {
  try {
    const v = JSON.parse(localStorage.getItem(SRC_CACHE_KEY) || "null");
    return Array.isArray(v) && v.length ? (v as DjSource[]) : null;
  } catch {
    return null;
  }
}

function readHealth(): SrcHealth {
  try {
    const v = JSON.parse(localStorage.getItem(SRC_HEALTH_KEY) || "{}");
    return v && typeof v === "object" ? (v as SrcHealth) : {};
  } catch {
    return {};
  }
}

function writeHealth(h: SrcHealth) {
  try {
    localStorage.setItem(SRC_HEALTH_KEY, JSON.stringify(h));
  } catch {
    /* 隐私模式：本次会话内仍生效（state 驱动），只是不持久化 */
  }
}

/** 站源是否已判定不可用（探测或实际加载失败过） */
function isHidden(h: SrcHealth, id: string): boolean {
  return (h[id]?.fail || 0) > 0;
}
const EP_GROUP = 30; // 选集分组（与上游详情页同口径）

/** ── [lc-2112] 排序（纯前端）：上游 catalog 无排序参数，只对**已加载**的数据排序。
    score/heat/views 是字符串数字（后端原样透传），解析失败按 0 处理并排到末尾。 ── */
type SortMode = "default" | "score" | "heat" | "views" | "episodes";
const SORTS: { id: SortMode; label: string }[] = [
  { id: "default", label: "默认" },
  { id: "score", label: "评分" },
  { id: "heat", label: "热度" },
  { id: "views", label: "播放量" },
  { id: "episodes", label: "集数" },
];

/** [lc-2112] 数值紧凑显示：≥1 万显示「x.x万」，避免 36796836 这种长数字撑破卡片。 */
function compactNum(v: unknown): string {
  const n = numOf(v);
  if (n >= 1e8) return `${(n / 1e8).toFixed(1)}亿`;
  if (n >= 1e4) return `${(n / 1e4).toFixed(1)}万`;
  return n > 0 ? String(n) : "";
}

/** [lc-2123] 秒 → mm:ss（观看进度展示用） */
function fmtTime(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, "0")}`;
}

function numOf(v: unknown): number {
  const n = Number(String(v ?? "").replace(/[^\d.]/g, ""));
  return Number.isFinite(n) ? n : 0;
}

function sortItems(list: DuanjuDrama[], mode: SortMode): DuanjuDrama[] {
  if (mode === "default") return list;
  const key = (d: DuanjuDrama): number => {
    switch (mode) {
      case "score":
        return numOf(d.score);
      case "heat":
        return numOf(d.heat);
      case "views":
        return numOf(d.views);
      case "episodes":
        return numOf(d.episodes);
      default:
        return 0;
    }
  };
  // 稳定排序：同值保持上游原序（现代 V8 的 sort 已稳定，这里显式 index 兜底）
  return list
    .map((d, i) => ({ d, i, k: key(d) }))
    .sort((a, b) => (b.k - a.k) || (a.i - b.i))
    .map((x) => x.d);
}

/** ── [lc-2099] 短剧收藏（本地持久化）：详情页右上角星标收藏，左栏「收藏」标签置顶。
    存整份 drama 摘要（id/title/cover/源/集数等）——收藏列表要能直接渲染卡片并再次
    打开详情，光存 id 得重新拉取；与 IPTV 收藏同一套 localStorage 惯例。 ── */
const FAV_KEY = "anibox.duanju.favs";

type FavDrama = {
  id: string;
  title: string;
  cover?: string;
  source?: string;
  episodes?: number;
  category?: string;
  vip?: boolean | null;
  score?: string;
  heat?: string;
  views?: string;
  releaseStatus?: string;
  tags?: string[];
};

function loadFavs(): FavDrama[] {
  try {
    const v = JSON.parse(localStorage.getItem(FAV_KEY) || "[]");
    return Array.isArray(v) ? v.filter((x) => x && typeof x.id === "string") : [];
  } catch {
    return [];
  }
}

function readFavs(): FavDrama[] {
  return loadFavs();
}

/** [lc-2240] 每页条数：用户要求「每个分类每次加载 50 个，再往下拉继续加载」。
 *  核心的 pageSize 默认 30（Config.PageSize），这里靠**前端截断首屏**实现 50 条/屏：
 *  首屏只渲染前 50 条，滚动到底追加下一批（追加时直接再取 50 条）。
 *  这样「一次性加载完」变成按需追加，减少首屏 DOM 与图片请求。 */
const PAGE_SIZE = 50;

export function DuanjuPane() {
  const [source, setSource] = useState("hongguo");
  // [lc-2180] 站源集合以核心回传为准；[lc-2189] 首帧用上次成功的缓存（null = 还没拿到过，
  // 显示骨架），核心缺失时才退兜底表 —— 避免「切到短剧标签先闪旧站源表」。
  const [sources, setSources] = useState<DjSource[] | null>(readSrcCache);
  // [lc-2189] 站源健康（失败站点从 chips 剔除；state 驱动渲染，localStorage 持久化）
  const [health, setHealth] = useState<SrcHealth>(readHealth);
  // [lc-2181] 青空账号：登录弹窗开关 + 已登录账号名（空 = 未登录）
  const [loginOpen, setLoginOpen] = useState(false);
  const [acct, setAcct] = useState("");
  // 播放因「需要登录」失败时记下待重试的播放参数，登录成功后自动重试
  const lastPlay = useRef<{ idx: number; chapter: DuanjuChapter; name: string } | null>(null);
  // [lc-2237] 用户交互（开详情/起播）标记：健康探测在此期间让路，不抢核心读名额
  const userBusyRef = useRef(false);
  // [lc-2237] 详情加载失败时 detail 为 null，重试需要记住上次要打开的那部剧
  const lastDetailRef = useRef<DuanjuDrama | null>(null);
  // [lc-2099] 收藏：favOpen = 左栏「收藏」标签激活（与站源 chips 并列置顶）
  const [favs, setFavs] = useState<FavDrama[]>(readFavs);
  const [favOpen, setFavOpen] = useState(false);
  const [sort, setSort] = useState<SortMode>("default"); // [lc-2112] 排序（纯前端）
  // [lc-2217] 18+ 站源解锁态（默认关）：没解锁时 chips 里只留红果。
  // 解锁入口在 设置 → 通用 → 短剧 18+ 站源（需手动输入确认语），这里只订阅状态。
  const [adultOn, setAdultOn] = useState(adultSourcesEnabled);
  useEffect(() => onAdultSourcesChange(() => setAdultOn(adultSourcesEnabled())), []);
  // 锁上时若当前正停在 18+ 站源（比如刚在设置里关掉）→ 退回红果
  useEffect(() => {
    if (!adultOn && isAdultSource(source)) setSource("hongguo");
  }, [adultOn, source]);
  // 解锁瞬间补探一次新冒出来的 18+ 站源（锁着时按规矩没探过）
  useEffect(() => {
    if (!adultOn) return;
    const list = (sources || []).filter((x) => x.available !== false);
    if (list.length) void probeSources(list);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [adultOn]);
  const [cats, setCats] = useState<{ id: string; name: string }[]>([]);
  const [cat, setCat] = useState("");
  const [query, setQuery] = useState("");
  const [term, setTerm] = useState(""); // 已提交的搜索词（回车触发）
  const [items, setItems] = useState<DuanjuDrama[]>([]);
  const [page, setPage] = useState(1);
  const [hasMore, setHasMore] = useState(false);
  const [busy, setBusy] = useState(false);
  const [moreBusy, setMoreBusy] = useState(false);
  /* [lc-2294] 首屏靴子阶段：目录数据还没落定之前**一律显示加载态**（进度条 + 网格骨架）。
   * 起因：lc-2235 缓存优先把「请求开始就置 busy」去掉了 —— cached/catalog 回包之前
   * busy=false 且 items=[] 会直接落进「暂无内容」空态，用户看到的就是「短剧页全空了」。
   * pct 只按真实里程碑推进（连接核心 15 → 读本地缓存 40 → 打网络 60），
   * 之后缓慢逼近上限 88，**不冲到 100**（数据没到就不该显示完成）。 */
  const [boot, setBoot] = useState<{ on: boolean; pct: number; label: string }>({
    on: true,
    pct: 15,
    label: "正在连接短剧核心…",
  });
  const bootTo = (pct: number, label: string) => setBoot({ on: true, pct, label });
  const bootDone = () => setBoot((b) => (b.on ? { ...b, on: false } : b));
  // [lc-2240] 当前渲染条数（每次 +50）；切站源/分类/搜索时归零重来
  const [visibleCount, setVisibleCount] = useState(PAGE_SIZE);
  // [lc-2240] 触底加载哨兵：滚到这个元素就追加下一批
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const [err, setErr] = useState("");
  const [nonce, setNonce] = useState(0);

  // 详情浮层
  const [detail, setDetail] = useState<DetailResp | null>(null);
  const [dBusy, setDBusy] = useState(false);
  const [dErr, setDErr] = useState("");
  const [epGroup, setEpGroup] = useState(0);
  const [playingIdx, setPlayingIdx] = useState(-1);
  // [lc-2123] 当前打开剧的观看进度（详情页展示 + 「继续观看」定位）
  const [hist, setHist] = useState<DjHistory | null>(null);
  const [playErr, setPlayErr] = useState("");

  // [lc-2189] 后台探测站源可用性：无筛选目录首页拉一次；成功清零、失败 +1（TTL 内不重探）。
  // 失败源由 chips 剔除（见 render 的 visibleSources）。
  // [lc-2234] **必须限并发**：核心的读请求名额只有 16 个（app_requests.go beginRead
  // `readCount >= 16` 直接拒绝），慢源要占满 26~60s 超时。此前 13 源全并发 →
  // 名额被死源占死，用户正在看的目录请求排队/被拒 = 界面一直「加载中」。
  // 现在：当前站源最先探、最多 3 个在飞，慢源最多拖住自己。
  // [lc-2237] 但探测仍会和用户的详情/播放抢核心读名额（实测红果 detail 0.3s → 5.1s），
  // 所以探测降为**单并发**并在等待时让路给用户操作。
  async function probeSources(list: DjSource[]) {
    const h = { ...readHealth() };
    const now = Date.now();
    const todo = list
      .filter(
        (s) =>
          // [lc-2217] 未解锁时不去探测 18+ 站点（默认态只碰红果）
          (adultSourcesEnabled() || !isAdultSource(s.id)) &&
          s.available !== false &&
          !(h[s.id] && now - h[s.id].at < HEALTH_TTL),
      )
      // 正在看的站源优先
      .sort((a, b) => Number(b.id === source) - Number(a.id === source));
    if (!todo.length) {
      setHealth(h);
      return;
    }
    const queue = [...todo];
    const worker = async () => {
      for (;;) {
        // [lc-2237] 用户正在开详情/起播 → 探测让路（等它结束再继续），
        // 避免慢源与用户的请求抢核心读名额。
        while (userBusyRef.current) await new Promise((r) => window.setTimeout(r, 400));
        const s = queue.shift();
        if (!s) return;
        try {
          await invoke("duanju_catalog", { source: s.id, category: "", page: 1, query: "" });
          h[s.id] = { fail: 0, at: Date.now() };
        } catch {
          h[s.id] = { fail: (h[s.id]?.fail || 0) + 1, at: Date.now() };
        }
        // 每完成一个就落盘并刷新 chips（死的源尽早消失，不用等全部探完）
        writeHealth(h);
        setHealth({ ...h });
      }
    };
    // 单并发：用户点开详情/播放时最多被一个慢源影响（而不是 3 个），
    // 配合下面的 userBusy 门控在交互期间直接暂停探测。
    await Promise.all(Array.from({ length: Math.min(1, todo.length) }, worker));
  }

  // [lc-2180] 站源能力清单（顺带预热核心）：guoapp v2 起站源 = 随包 JS 模块，
  // 可用集合与 adult 声明以核心回传为准。
  // [lc-2189] 成功后写缓存（下次切标签首帧就是完整列表，不再闪旧表）+ 后台健康探测。
  useEffect(() => {
    let alive = true;
    invoke<{ sources?: DjSource[] | null }>("duanju_init")
      .then((d) => {
        if (!alive) return;
        const list = Array.isArray(d?.sources)
          ? d.sources!.filter((s) => s && typeof s.id === "string" && typeof s.name === "string")
          : null;
        if (list && list.length) {
          setSources(list);
          try {
            localStorage.setItem(SRC_CACHE_KEY, JSON.stringify(list));
          } catch {
            /* 隐私模式：仅本次会话生效 */
          }
          probeSources(list);
        } else if (!readSrcCache()) {
          setSources(FALLBACK_SOURCES); // 老核心无 sources 字段
        }
      })
      .catch(() => {
        // 核心缺失：无缓存时退兜底表（chips 可见，调用报「短剧核心缺失」）
        if (!readSrcCache()) setSources(FALLBACK_SOURCES);
      });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // [lc-2181] 青空账号状态（会话在核心数据目录持久化，重启后仍有效）
  useEffect(() => {
    let alive = true;
    invoke<{ loggedIn?: boolean; name?: string }>("duanju_account_status")
      .then((r) => {
        if (alive) setAcct(r?.loggedIn && r.name ? r.name : "");
      })
      .catch(() => {
        /* 核心缺失时静默（chip 显示未登录态） */
      });
    return () => {
      alive = false;
    };
  }, []);

  function refreshAcct() {
    invoke<{ loggedIn?: boolean; name?: string }>("duanju_account_status")
      .then((r) => setAcct(r?.loggedIn && r.name ? r.name : ""))
      .catch(() => setAcct(""));
  }

  // 分类（随站源重取）
  useEffect(() => {
    let alive = true;
    setCats([]);
    setCat("");
    // [lc-2236] 核心的 categories op 返回的是 **对象** `{items: [...]}`（见
    // app_runtime.go: `return map[string]any{"items": items}`），不是数组。
    // 此前用 `Array.isArray(d)` 判断 → 永远为假 → 分类栏只剩硬编码的「全部」。
    invoke<{ items?: { id: string; name: string }[] } | { id: string; name: string }[]>(
      "duanju_categories",
      { source },
    )
      .then((d) => {
        if (!alive) return;
        const list = Array.isArray(d) ? d : d?.items;
        if (Array.isArray(list)) setCats(list);
      })
      .catch(() => {
        /* 分类拉取失败不阻塞目录（上游会回「全部」） */
      });
    return () => {
      alive = false;
    };
  }, [source]);
  // [lc-2294] 靴子进度自走：里程碑之间缓慢逼近 88（数据没到不冲 100），
  // 让慢站源的等待读起来是「在动」而不是「卡住」。
  useEffect(() => {
    if (!boot.on) return;
    const id = window.setInterval(() => {
      setBoot((b) => (b.on && b.pct < 88 ? { ...b, pct: Math.min(88, b.pct + 1) } : b));
    }, 700);
    return () => window.clearInterval(id);
  }, [boot.on]);

  // 目录第一页（站源/分类/搜索词变化即重取）
  useEffect(() => {
    if (!source) return;
    let alive = true;
    setErr("");
    setItems([]);
    setPage(1);
    setHasMore(false);
    setVisibleCount(PAGE_SIZE); // [lc-2240] 换站源/分类/搜索 → 回到第一屏 50 条
    // [lc-2294] 每次重取都回到加载态：数据没落定之前不许出现「暂无内容」
    bootTo(30, "正在读取目录…");
    // [lc-2234] 缓存优先（cached-first）：核心把目录按「站源|分类」落盘缓存（15 分钟）。
    // 从前每次进短剧页都走 catalog 全量重拉（红果实测 6.4s，用户看到的就是「加载中」）；
    // 现在先用 `cached` 这个**纯本地、不打网络**的 op 秒出内容：
    //   · 有数据 → 立刻渲染；Fresh=false（过期）再后台刷新，不阻塞、不闪白；
    //   · 没数据 → 才显示加载中并等网络的 catalog。
    const netTimeout = () => {
      if (!alive) return 0;
      return window.setTimeout(() => {
        if (!alive) return;
        setErr("目录加载超时，该站源可能暂时不可用");
        setBusy(false);
        bootDone(); // [lc-2294] 超时也必须收加载态，否则错误被骨架盖住、用户看不到原因
      }, 20000);
    };
    // 网络失败的统一处理（初载与后台刷新共用；[lc-2189] 无筛选首页失败 = 站源不可用）
    const onNetError = (e: unknown) => {
      if (!alive) return;
      setErr(String(e));
      if (!term && !cat) {
        const h = readHealth();
        h[source] = { fail: (h[source]?.fail || 0) + 1, at: Date.now() };
        writeHealth(h);
        setHealth({ ...h });
      }
    };
    // [lc-2239] 缓存优先只对「全部」有意义：核心的目录缓存按「站源」存整表
    // （catalogs.json 里的 key 就是站源名，没有分类后缀），带分类/标签筛选时
    // `cached` 恒返回空。此前切任何标签都先打一次 cached（空）再打 catalog，
    // 既多一次往返，又在 catalog 慢时把界面停在空态。现在：
    //   · 搜索 / 分类筛选 / 重试 → 直接走网络；
    //   · 仅「全部」→ cached-first。
    const forceNet = !!term || !!cat || nonce > 0;
    let tid = 0;
    if (forceNet) {
      setBusy(true);
      bootTo(60, term ? "正在搜索…" : "正在从站源加载…");
      tid = netTimeout();
      invoke<CatalogResp>("duanju_catalog", { source, category: cat, page: 1, query: term })
        .then((d) => {
          if (!alive) return;
          setItems(Array.isArray(d?.items) ? d.items : []);
          setHasMore(d?.hasMore === true);
          setPage(d?.page || 1);
          bootDone(); // [lc-2294] 数据落定才收加载态（含空结果 → 交给空态渲染）
        })
        .catch((e) => {
          onNetError(e);
          bootDone(); // 失败也要收：否则错误被加载态盖住，用户永远看不到原因
        })
        .finally(() => {
          window.clearTimeout(tid);
          if (alive) setBusy(false);
        });
    } else {
      invoke<CatalogResp>("duanju_cached", { source, category: cat })
        .then((c) => {
          if (!alive) return;
          const hit = Array.isArray(c?.items) && c.items.length > 0;
          if (hit) {
            // 命中缓存：先渲染（秒开），busy 保持 false 不显示加载中
            setItems(c.items!);
            setHasMore(c.hasMore === true);
            setPage(c.page || 1);
            bootDone(); // [lc-2294] 缓存已上屏 = 有内容可看，立刻收加载态
            if (c.fresh === true) return; // 15 分钟内新鲜：不刷新
          } else {
            setBusy(true);
            bootTo(60, "正在从站源加载…");
          }
          // 过期或未命中 → 后台刷新（不显示加载中遮罩；失败只在无缓存时提示）
          tid = netTimeout();
          invoke<CatalogResp>("duanju_catalog", { source, category: cat, page: 1, query: "" })
            .then((d) => {
              if (!alive) return;
              setItems(Array.isArray(d?.items) ? d.items : []);
              setHasMore(d?.hasMore === true);
              setPage(d?.page || 1);
              setErr("");
              bootDone();
            })
            .catch((e) => {
              bootDone();
              if (hit) return; // 有兜底数据时刷新失败不打扰
              onNetError(e);
            })
            .finally(() => {
              window.clearTimeout(tid);
              if (alive) setBusy(false);
            });
        })
        .catch(() => {
          // cached op 不可用（老核心，或 Rust 侧还没更新到含 duanju_cached 的版本）：
          // 退化为直接走网络。**这里必须用非空 query**：走 search 语义不读缓存。
          if (!alive) return;
          setBusy(true);
          bootTo(60, "正在从站源加载…");
          tid = netTimeout();
          invoke<CatalogResp>("duanju_catalog", { source, category: cat, page: 1, query: "" })
            .then((d) => {
              if (!alive) return;
              setItems(Array.isArray(d?.items) ? d.items : []);
              setHasMore(d?.hasMore === true);
              setPage(d?.page || 1);
              bootDone();
            })
            .catch((e) => {
              onNetError(e);
              bootDone();
            })
            .finally(() => {
              window.clearTimeout(tid);
              if (alive) setBusy(false);
            });
        });
    }
    return () => {
      alive = false;
      window.clearTimeout(tid);
    };
  }, [source, cat, term, nonce]);

  async function loadMore() {
    if (moreBusy) return;
    // [lc-2240] 先看内存里还有没有没渲染的条目（「全部」的 cached 一次给整表，
    // 标签分类每页只有 11~12 条）。够 50 条就直接放出来，不用打网络。
    if (items.length > visibleCount) {
      setVisibleCount((n) => n + PAGE_SIZE);
      return;
    }
    if (!hasMore) return;
    setMoreBusy(true);
    try {
      // 核心 pageSize=30、标签分类每页仅 11~12 条 → 循环取到够 50 条或到底
      const merged: DuanjuDrama[] = [];
      const seen = new Set(items.map((x) => x.id));
      let next: number = page + 1;
      let more: boolean = hasMore;
      for (let round = 0; round < 6 && merged.length < PAGE_SIZE && more; round++) {
        const d = await invoke<CatalogResp>("duanju_catalog", {
          source,
          category: cat,
          page: next,
          query: term,
        });
        const fresh = Array.isArray(d?.items) ? d.items : [];
        for (const x of fresh) {
          if (!seen.has(x.id)) {
            seen.add(x.id);
            merged.push(x);
          }
        }
        more = d?.hasMore === true;
        const got = d?.page || next;
        if (got <= next && fresh.length === 0) break; // 上游没前进：防死循环
        next = got + 1;
      }
      if (merged.length) {
        setItems((prev) => [...prev, ...merged]);
        setVisibleCount((n) => n + PAGE_SIZE);
      }
      setHasMore(more);
      setPage(next - 1);
    } catch (e) {
      setErr(String(e));
    } finally {
      setMoreBusy(false);
    }
  }

  // [lc-2240] 触底自动加载。`.content` 是滚动容器（06-content.css 的 overflow-y:auto），
  // 用哨兵 + IntersectionObserver 而不是 onScroll：列表恰好撑不满一屏时不会有滚动事件，
  // 哨兵进入视口能自动续取，直到填满或到底（与 lc-1727 的媒体库同一套做法）。
  useEffect(() => {
    if (favOpen || busy) return;
    // [lc-2240] 数据已全部呈现且没有下一页 → 不必再观察（否则哨兵一直触发空 loadMore）
    const exhausted = items.length <= visibleCount && !hasMore;
    if (exhausted) return;
    const target = sentinelRef.current;
    if (!target || typeof IntersectionObserver === "undefined") return;
    const root = target.closest(".content") as HTMLElement | null;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((en) => en.isIntersecting)) void loadMore();
      },
      // 提前 800px 触发：滚到底之前就把下一批接上，避免「到底停一下」
      { root, rootMargin: "800px 0px" },
    );
    io.observe(target);
    return () => io.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [favOpen, busy, items.length, visibleCount, moreBusy, hasMore, source, cat, term]);

  // [lc-2127] 连播事件：mpv 的 eof-reached 回调**不能**在 IPC 读线程里起播（会自己等
  // 自己的回包 = 死锁，实测第 2 集永远加载不出来）。后端只 emit 事件，这里在普通命令
  // 线程里调 duanju_auto_next 起播下一集。
  useEffect(() => {
    let alive = true;
    const un = listen("duanju-eof", () => {
      if (!alive) return;
      invoke<boolean>("duanju_auto_next")
        .then((played) => {
          if (played && alive) {
            // 连播推进后同步 UI 的集数高亮（后端已在队列里推进 index）
            setHist((prev) => (prev ? { ...prev, index: prev.index + 1 } : prev));
          }
        })
        .catch(() => {
          /* 失败后端已记日志并停止连播，不打扰用户 */
        });
    });
    return () => {
      alive = false;
      un.then((f) => f()).catch(() => {});
    };
  }, []);

  /* [lc-2295] 详情请求代次：每次「打开详情」+1，「关闭」也 +1。
   * 夹在中间的旧请求回来时代次对不上就**直接丢弃** —— 否则用户在超时提示上点了「关闭」，
   * 那个仍在跑的请求晚一步返回，会把浮层重新 setDetail 打开（表现还是「关不掉」）。 */
  const detailEpoch = useRef(0);

  /* [lc-2295] 关闭详情浮层：**必须三个状态一起清**。
   * 渲染条件是 `(dBusy || detail || dErr)`，此前「关闭」和点遮罩只做 setDetail(null) ——
   * dErr 还留着，浮层条件照样成立，于是「点关闭没反应、关不掉」（用户报障）。
   * 统一收口到一个函数，避免以后再加状态时又漏。 */
  function closeDetail() {
    detailEpoch.current += 1; // 作废在飞的请求
    setDetail(null);
    setDErr("");
    setDBusy(false);
    lastDetailRef.current = null;
    userBusyRef.current = false;
  }

  async function openDetail(d: DuanjuDrama) {
    const epoch = ++detailEpoch.current;
    setDetail(null);
    setDErr("");
    setEpGroup(0);
    setPlayErr("");
    setPlayingIdx(-1);
    lastDetailRef.current = d;
    // [lc-2237] 收藏里可能留着**已下架/已失效站源**的旧条目（上游 v2 把黄豆、禁果、
    // 黄果旧版撤了；野果/黄果视频/MissAV 等也常年超时）。对这些源调 detail 会一直挂着
    // 到上游 26~60s 超时，界面就是转圈到 20s 兜底才报错 —— 直接秒失败并说清楚。
    const src = String(d.source || (d.id.includes(":") ? d.id.split(":")[0] : ""));
    const list0 = sources || [];
    const known = list0.find((s) => s.id === src);
    const srcLabel = known?.name || src;
    if (src && (!known || known.available === false)) {
      setDErr(`该剧来自已不可用的站源（${srcLabel}），无法打开详情`);
      return;
    }
    // 已判定加载失败（健康探测剔除）的站源同样直接拦，不再等 26s 网络超时
    if (src && isHidden(health, src)) {
      setDErr(`站源「${srcLabel}」当前不可用，请点站源行的「重试」后再试`);
      return;
    }
    setDBusy(true);
    userBusyRef.current = true;
    // [lc-2234] 详情同样可能卡住（核心读名额被占满/站点超时）→ 兜底，避免永远转圈。
    // [lc-2237] 收紧到 12s：核心侧死源要 26~60s 才超时，让用户干等 20s 没有意义；
    // 12s 没回就先报错，用户可点「重试」（后台请求可能仍在跑，重试会拿新结果）。
    const tid = window.setTimeout(() => {
      if (epoch !== detailEpoch.current) return; // 已被关闭/换片：不要再动状态
      setDErr("详情加载超时，请点「重试」或换个站源");
      setDBusy(false);
      userBusyRef.current = false;
    }, 12000);
    try {
      const resp = await invoke<DetailResp>("duanju_detail", { drama: d });
      if (epoch !== detailEpoch.current) return; // 用户已关闭：丢弃这次回包，别把浮层重新打开
      setDetail({ drama: resp?.drama || d, chapters: resp?.chapters || [] });
      // [lc-2123] 读该剧的观看进度（独立存储，未看过返回 null）
      invoke<DjHistory | null>("duanju_history_one", { id: d.id })
        .then((h) => setHist(h))
        .catch(() => setHist(null));
    } catch (e) {
      if (epoch !== detailEpoch.current) return; // 已关闭：错误也不必显示
      setDErr(String(e));
    } finally {
      window.clearTimeout(tid);
      if (epoch === detailEpoch.current) {
        setDBusy(false);
        userBusyRef.current = false;
      }
    }
  }

  async function play(idx: number, chapter: DuanjuChapter, name: string) {
    if (playingIdx >= 0 || !detail?.drama) return;
    setPlayErr("");
    setPlayingIdx(idx);
    userBusyRef.current = true;
    lastPlay.current = { idx, chapter, name };
    try {
      await invoke("duanju_play", {
        drama: detail.drama,
        chapter,
        index: idx,
        quality: null,
        title: `${detail.drama.title || name} ${chapter.title || `第${idx + 1}集`}`,
        // [lc-2116] 传全部分集：后端据此在播完后自动接下一集（短剧一集只有 1~3 分钟）
        chapters,
      });
      // [lc-2123] 起播成功：本地状态立即反映「上次看到第 N 集」，并重置回写计时
      setHist((prev) => ({
        drama: {
          id: detail.drama!.id,
          title: detail.drama!.title,
          cover: detail.drama!.cover,
          source: detail.drama!.source,
          episodes: detail.drama!.episodes,
        },
        index: idx,
        pos: 0,
        dur: prev?.dur || 0,
        at: Date.now(),
      }));
    } catch (e) {
      setPlayErr(String(e));
    } finally {
      setPlayingIdx(-1);
      userBusyRef.current = false;
    }
  }

  // [lc-2181] 播放失败提示含「登录」（青空：需要登录/登录已失效）→ 提供登录入口，
  // 登录成功后自动重试上一次播放。
  const needLogin = /登录/.test(playErr);
  function retryLastPlay() {
    const lp = lastPlay.current;
    if (lp && detail?.drama) play(lp.idx, lp.chapter, lp.name);
  }

  // [lc-2189] chips 只列可用站源；失败过的剔除并留「重试」入口
  const srcList = sources || [];
  const visibleSources = srcList.filter(
    (s) =>
      s.available !== false &&
      // [lc-2217] 没解锁时 18+ 站源整条不出现（不是禁用，是不列出来）
      (adultOn || !isAdultSource(s.id)) &&
      !isHidden(health, s.id),
  );
  const hiddenCount = srcList.filter(
    (s) => s.available !== false && (adultOn || !isAdultSource(s.id)) && isHidden(health, s.id),
  ).length;
  function retryHiddenSources() {
    const h = { ...health };
    for (const s of srcList) if ((h[s.id]?.fail || 0) > 0) delete h[s.id];
    writeHealth(h);
    setHealth(h);
    probeSources(srcList.filter((s) => s.available !== false));
  }

  // [lc-2123] 观看进度**由 Rust 侧直接从 mpv 的 time-pos 落盘**（准确，无需前端估算）。
  // 这里只做「定期把后端记录读回来刷新 UI」：详情浮层开着时每 5s 拉一次该剧进度，
  // 这样集数高亮/「上次看到第 N 集」会跟着实际播放走（含连播自动切集）。
  useEffect(() => {
    const id0 = detail?.drama?.id;
    if (!id0) return;
    const id = window.setInterval(() => {
      invoke<DjHistory | null>("duanju_history_one", { id: id0 })
        .then((h) => h && setHist(h))
        .catch(() => {});
    }, 5000);
    return () => window.clearInterval(id);
  }, [detail?.drama?.id]);

  const favIds = useMemo(() => new Set(favs.map((f) => f.id)), [favs]);

  /** 收藏/取消（详情页星标）；localStorage 同步写，失败静默（下次会话丢这次勾选）。 */
  function toggleFav(d: DuanjuDrama | FavDrama) {
    setFavs((prev) => {
      const next = prev.some((f) => f.id === d.id)
        ? prev.filter((f) => f.id !== d.id)
        : [
            {
              id: d.id,
              title: d.title,
              cover: (d as DuanjuDrama).cover,
              source: (d as DuanjuDrama).source,
              episodes: (d as DuanjuDrama).episodes,
              category: (d as DuanjuDrama).category,
              vip: (d as DuanjuDrama).vip,
              score: (d as DuanjuDrama).score,
              heat: (d as DuanjuDrama).heat,
              views: (d as DuanjuDrama).views,
              releaseStatus: (d as DuanjuDrama).releaseStatus,
              tags: (d as DuanjuDrama).tags,
            },
            ...prev,
          ];
      try {
        localStorage.setItem(FAV_KEY, JSON.stringify(next));
      } catch {
        /* 存储满/被禁：不影响本次会话内的收藏态 */
      }
      return next;
    });
  }

  // [lc-2112] 目录/收藏列表按当前排序模式派生（只对已加载数据排序）
  const allShown = useMemo(() => sortItems(items, sort), [items, sort]);
  // [lc-2240] 每次只渲染前 visibleCount 条（初始 50），滚到底部再加 50。
  // 数据可能一次性就在内存里（「全部」的 cached 返回整表），但 DOM/图片按需生成，
  // 首屏不会铺几千张海报。
  const shownItems = useMemo(() => allShown.slice(0, visibleCount), [allShown, visibleCount]);
  const shownFavs = useMemo(() => sortItems(favs as DuanjuDrama[], sort), [favs, sort]) as FavDrama[];

  const chapters = detail?.chapters || [];
  const epGroups = useMemo(() => {
    const out: DuanjuChapter[][] = [];
    for (let i = 0; i < chapters.length; i += EP_GROUP) out.push(chapters.slice(i, i + EP_GROUP));
    return out;
  }, [chapters]);

  return (
    <div className="dj">
      {/* 站源行：[lc-2099] 「收藏」置顶，其次各站源 */}
      <div className="dj-sources">
        <button
          type="button"
          className={`dj-chip dj-favtab${favOpen ? " on" : ""}`}
          onClick={() => {
            setFavOpen(true);
            setTerm("");
            setQuery("");
          }}
        >
          <IcoStar size={12} strokeWidth={2} className="rating-ico" />
          收藏
          {favs.length > 0 && <span className="dj-favcount">{favs.length}</span>}
        </button>
        {/* [lc-2189] 首次运行还没有站源清单：骨架占位（不闪旧表） */}
        {sources === null && (
          <button type="button" className="dj-chip" disabled>
            加载站源…
          </button>
        )}
        {visibleSources.map((s) => (
            <button
              key={s.id}
              type="button"
              className={`dj-chip${!favOpen && source === s.id ? " on" : ""}`}
              onClick={() => {
                // [lc-2217] 18+ 站源只有在设置页手动解锁后才会出现在这里，切换不再二次弹窗
                setFavOpen(false);
                setSource(s.id);
                setTerm("");
                setQuery("");
              }}
            >
              {s.name}
            </button>
          ))}
        {/* [lc-2189] 被剔除的站源留重试入口（站点恢复后可拉回；仍在 TTL 内的成功源不重探） */}
        {hiddenCount > 0 && (
          <button
            type="button"
            className="dj-chip dj-src-retry"
            title="重新检测加载失败的站源"
            onClick={retryHiddenSources}
          >
            重试 {hiddenCount}
          </button>
        )}
        {/* [lc-2181] 青空需登录才能播放：选中该站源时行尾给账号入口（登录/账号名·退出） */}
        {source === "sorani" && (
          <button
            type="button"
            className={`dj-chip sor-acct${acct ? " on" : ""}`}
            title={acct ? `已登录 ${acct}，点击退出` : "青空播放需登录，点击登录"}
            onClick={() => {
              if (!acct) {
                setLoginOpen(true);
                return;
              }
              invoke<{ loggedIn?: boolean }>("duanju_logout")
                .then(() => setAcct(""))
                .catch(() => refreshAcct());
            }}
          >
            {acct || "登录"}
          </button>
        )}
      </div>

      {/* 分类 + 搜索 */}
      <div className="dj-row">
        <div className="dj-chips dj-cats">
          <button
            type="button"
            className={`dj-chip sm${cat === "" ? " on" : ""}`}
            onClick={() => setCat("")}
          >
            全部
          </button>
          {/* [lc-2113] 排序胶囊并入分类行（紧跟「全部」，不另起一行）：
              用户要求排序与分类同一行，省一行纵向空间。分隔用小竖线而非新行。 */}
          <span className="dj-divider" aria-hidden="true" />
          {SORTS.map((s) => (
            <button
              key={`sort-${s.id}`}
              type="button"
              className={`dj-chip sm dj-sortchip${sort === s.id ? " on" : ""}`}
              onClick={() => setSort(s.id)}
              title={`按${s.label}排序`}
            >
              {s.label}
            </button>
          ))}
          <span className="dj-divider" aria-hidden="true" />
          {cats.map((c) => (
            <button
              key={c.id}
              type="button"
              className={`dj-chip sm${cat === c.id ? " on" : ""}`}
              onClick={() => setCat(c.id)}
            >
              {c.name}
            </button>
          ))}
        </div>
        <input
          className="dj-search"
          placeholder="搜索短剧"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") setTerm(query.trim());
          }}
        />
      </div>

      {favOpen ? (
        /* [lc-2099] 收藏视图：本地即时渲染，不走目录请求 */
        favs.length === 0 ? (
          <div className="dlib-stub">
            <div className="dlib-stub-t">还没有收藏的短剧</div>
            <div className="dlib-stub-hint">在任意短剧详情页点右上角星标即可收藏</div>
          </div>
        ) : (
          <div className="dj-grid">
            {shownFavs.map((d) => (
              <button key={d.id} type="button" className="dj-card" onClick={() => openDetail(d)}>
                <span className="dj-poster">
                  {d.cover ? (
                    <img src={djImg(d.cover)} alt="" loading="lazy" draggable={false} />
                  ) : (
                    <span className="dj-ph" aria-hidden="true" />
                  )}
                  <span
                    className="dj-star on"
                    role="button"
                    tabIndex={0}
                    aria-label="取消收藏"
                    onClick={(e) => {
                      e.stopPropagation();
                      toggleFav(d);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.stopPropagation();
                        e.preventDefault();
                        toggleFav(d);
                      }
                    }}
                  >
                    <IcoStar size={13} strokeWidth={2} className="rating-ico fill" />
                  </span>
                </span>
                <span className="dj-title">{d.title}</span>
                <span className="dj-meta">
                  {[d.episodes ? `${d.episodes}集` : "", d.score ? `${d.score}分` : ""]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
              </button>
            ))}
          </div>
        )
      ) : boot.on ? (
        /* [lc-2294] 数据尚未落定 → 进度条 + 网格骨架（此前这里会落到「暂无内容」空态） */
        <DuanjuLoading progress={boot.pct} label={boot.label} />
      ) : busy ? (
        <div className="dlib-stub">
          <div className="dlib-stub-t">加载中…</div>
        </div>
      ) : err ? (
        <DState err={err} emptyText="" onRetry={() => setNonce((n) => n + 1)} />
      ) : items.length === 0 ? (
        // [lc-2239] 空态带上站源/分类，便于区分「这个分类真的没内容」和「请求没回来」
        <DState
          err=""
          emptyText={
            term
              ? `未找到与「${term}」相关的短剧`
              : cat
                ? `「${cats.find((c) => c.id === cat)?.name || cat}」分类暂无内容`
                : `${sources?.find((s) => s.id === source)?.name || source} 暂无内容`
          }
          onRetry={() => setNonce((n) => n + 1)}
        />
      ) : (
        <>
          <div className="dj-grid">
            {shownItems.map((d) => (
              <button key={d.id} type="button" className="dj-card" onClick={() => openDetail(d)}>
                <span className="dj-poster">
                  {d.cover ? (
                    <img src={djImg(d.cover)} alt="" loading="lazy" draggable={false} />
                  ) : (
                    <span className="dj-ph" aria-hidden="true" />
                  )}
                  {d.vip === true && <span className="dj-vip">VIP</span>}
                  <span
                    className={`dj-star${favIds.has(d.id) ? " on" : ""}`}
                    role="button"
                    tabIndex={0}
                    aria-label={favIds.has(d.id) ? "取消收藏" : "收藏"}
                    onClick={(e) => {
                      e.stopPropagation();
                      toggleFav(d);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.stopPropagation();
                        e.preventDefault();
                        toggleFav(d);
                      }
                    }}
                  >
                    <IcoStar
                      size={13}
                      strokeWidth={2}
                      className={`rating-ico${favIds.has(d.id) ? " fill" : ""}`}
                    />
                  </span>
                </span>
                <span className="dj-title">{d.title}</span>
                <span className="dj-meta">
                  {(() => {
                    // [lc-2112] 按当前排序模式把对应数值放最前，便于直接看出排序依据
                    const ep = d.episodes ? `${d.episodes}集` : "";
                    const sc = d.score ? `${d.score}分` : "";
                    const ht = compactNum(d.heat) ? `热度${compactNum(d.heat)}` : "";
                    const vw = compactNum(d.views) ? `播放${compactNum(d.views)}` : "";
                    const order =
                      sort === "score" ? [sc, ht, vw, ep]
                      : sort === "heat" ? [ht, sc, vw, ep]
                      : sort === "views" ? [vw, sc, ht, ep]
                      : sort === "episodes" ? [ep, sc, ht, vw]
                      : [ep, sc, ht || vw];
                    return order.filter(Boolean).slice(0, 2).join(" · ");
                  })()}
                </span>
              </button>
            ))}
          </div>
          {/* [lc-2240] 触底哨兵：滚到这里自动接下一批（50 条）。
              环形 loading 只在真正打网络时出现；内存里还有余量时瞬间放出来不闪。 */}
          <div ref={sentinelRef} className="dj-sentinel" aria-hidden="true">
            {moreBusy && <span className="dj-more-hint">加载中…</span>}
          </div>
          {items.length > 0 && items.length <= visibleCount && !hasMore && (
            <div className="dj-more-hint">已经到底了</div>
          )}
        </>
      )}

      {/* 详情浮层：**Portal 挂 body**（[lc-2091] 修「点开只有黑遮罩」——原先 absolute
          相对 .dj 网格容器定位，容器高数千像素，place-items:center 把面板居中到滚动内容
          中段，用户停在列表顶部时面板落在视口外，只剩铺满的遮罩）。fixed + Portal 后
          遮罩与面板都以视口为参照，与滚动位置无关。 */}
      {(dBusy || detail || dErr) && createPortal(
        <div className="dj-mask" onClick={() => !dBusy && closeDetail()}>
          <div className="dj-detail" onClick={(e) => e.stopPropagation()}>
            {dBusy ? (
              /* [lc-2294] 详情同样给骨架（海报位 + 文案行 + 选集格子），不再是干巴巴一行字 */
              <div className="dj-dsk" aria-busy="true">
                <div className="dj-dsk-head">
                  <div className="dj-dsk-poster">
                    <Dsk cls="dsk-poster" />
                  </div>
                  <div className="dj-dsk-lines">
                    <Dsk cls="dsk-line" />
                    <Dsk cls="dsk-line short" />
                    <Dsk cls="dsk-line" />
                    <Dsk cls="dsk-line short" />
                  </div>
                </div>
                <div className="dj-dsk-eps">
                  {Array.from({ length: 8 }, (_, i) => (
                    <Dsk key={i} cls="dj-dsk-ep" />
                  ))}
                </div>
                <div className="dj-detail-tip">加载剧集…</div>
              </div>
            ) : dErr ? (
              <>
                <div className="dj-detail-tip">{dErr}</div>
                {/* [lc-2237] 超时/失败后可直接重试（原先进来只能关掉再点一次） */}
                <button
                  type="button"
                  className="dj-morebtn"
                  onClick={() => {
                    const target = detail?.drama || lastDetailRef.current;
                    if (target) openDetail(target);
                  }}
                >
                  重试
                </button>
                <button type="button" className="dj-morebtn" onClick={closeDetail}>
                  关闭
                </button>
              </>
            ) : (
              detail && (
                <>
                  <button
                    type="button"
                    className="dj-close"
                    aria-label="关闭详情"
                    onClick={closeDetail}
                  >
                    <IcoXL size={16} strokeWidth={1.8} />
                  </button>
                  {/* [lc-2099] 收藏星标：详情页右上角（关闭按钮左侧） */}
                  {detail.drama && (
                    <button
                      type="button"
                      className={`dj-dfav${favIds.has(detail.drama.id) ? " on" : ""}`}
                      aria-label={favIds.has(detail.drama.id) ? "取消收藏" : "收藏"}
                      aria-pressed={favIds.has(detail.drama.id)}
                      onClick={() => detail.drama && toggleFav(detail.drama)}
                    >
                      <IcoStar
                        size={15}
                        strokeWidth={1.9}
                        className={`rating-ico${favIds.has(detail.drama.id) ? " fill" : ""}`}
                      />
                    </button>
                  )}
                  <div className="dj-dhead">
                    {detail.drama?.cover && (
                      <img className="dj-dposter" src={djImg(detail.drama.cover)} alt="" draggable={false} />
                    )}
                    <div className="dj-dinfo">
                      <div className="dj-dtitle">{detail.drama?.title}</div>
                      <div className="dj-dmeta">
                        {[
                          detail.drama?.episodes ? `${detail.drama.episodes}集` : "",
                          detail.drama?.releaseStatus === "finished"
                            ? "已完结"
                            : detail.drama?.releaseStatus === "ongoing"
                              ? "连载中"
                              : "",
                          detail.drama?.category || "",
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </div>
                      {(detail.drama?.tags || []).length > 0 && (
                        <div className="dj-dtags">
                          {detail.drama!.tags!.slice(0, 6).map((t) => (
                            <span key={t} className="dj-tag">
                              {t}
                            </span>
                          ))}
                        </div>
                      )}
                      {/* [lc-2123] 观看记录：独立存储（duanju-history.json），不走软件主历史 */}
                      {hist && hist.drama.id === detail.drama?.id && (
                        <div className="dj-dhist">
                          <span className="dj-dhist-t">
                            上次看到 第{hist.index + 1}集
                            {hist.pos > 0 && ` · ${fmtTime(hist.pos)}`}
                          </span>
                          <button
                            type="button"
                            className="dj-dhist-go"
                            disabled={playingIdx >= 0}
                            onClick={() => {
                              const ch = chapters[hist.index];
                              if (ch) {
                                setEpGroup(Math.floor(hist.index / EP_GROUP));
                                play(hist.index, ch, detail.drama?.title || "");
                              }
                            }}
                          >
                            继续观看
                          </button>
                        </div>
                      )}
                      {detail.drama?.description && (
                        <div className="dj-ddesc">{detail.drama.description}</div>
                      )}
                    </div>
                  </div>
                  {epGroups.length > 1 && (
                    <div className="dj-epgroups">
                      {epGroups.map((_, gi) => (
                        <button
                          key={gi}
                          type="button"
                          className={`dj-chip sm${epGroup === gi ? " on" : ""}`}
                          onClick={() => setEpGroup(gi)}
                        >
                          {gi * EP_GROUP + 1}-{Math.min((gi + 1) * EP_GROUP, chapters.length)}集
                        </button>
                      ))}
                    </div>
                  )}
                  <div className="dj-eps">
                    {(epGroups[epGroup] || []).map((ch, i) => {
                      const idx = epGroup * EP_GROUP + i;
                      // [lc-2123] 看到过（idx < 上次看到）→ 变浅；当前集 → 高亮描边
                      const watched = !!hist && hist.drama.id === detail.drama?.id && idx < hist.index;
                      const current = !!hist && hist.drama.id === detail.drama?.id && idx === hist.index;
                      return (
                        <button
                          key={ch.id || idx}
                          type="button"
                          className={`dj-ep${watched ? " watched" : ""}${current ? " current" : ""}`}
                          disabled={playingIdx >= 0}
                          onClick={() => play(idx, ch, detail.drama?.title || "")}
                          title={current ? "上次看到这里" : undefined}
                        >
                          {playingIdx === idx ? "起播中…" : ch.title || `第${idx + 1}集`}
                        </button>
                      );
                    })}
                  </div>
                  {playErr && <div className="dj-hint">{playErr}</div>}
                  {needLogin && (
                    <div className="dj-hint">
                      <button type="button" className="dj-chip sm" onClick={() => setLoginOpen(true)}>
                        登录青空
                      </button>
                    </div>
                  )}
                  <div className="dj-src">站源数据 guoapp（红果鉴，GPL-3.0）</div>
                </>
              )
            )}
          </div>
        </div>,
        document.body,
      )}

      {/* [lc-2181] 青空登录：成功后刷新账号态；若播放因未登录失败，自动重试上一集 */}
      {loginOpen && (
        <SoraniLoginModal
          onDone={(name) => {
            setLoginOpen(false);
            setAcct(name);
            retryLastPlay();
          }}
          onClose={() => setLoginOpen(false)}
        />
      )}
    </div>
  );
}
