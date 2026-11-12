/* [lc-2078] 发现页短剧栏（占位转真）—— 数据来自 guoapp「红果鉴」Go 核心（duanju_core.dll，
   GPL-3.0 与本仓库同许可；Rust 侧 FFI 桥见 src-tauri/src/duanju.rs）。
   UI：站源 chips（红果/剧果/禁果短剧/野果/黑桃/黄豆）+ 分类 chips + 搜索 + 竖版海报网格；
   点卡片 → 详情浮层（简介 + 选集 30/组）→ 点集 duanju_play（核心本地流直链 → mpv）。
   NOT IMPLEMENTED：推荐/榜单 Tab、弹幕、追剧、下载、多清晰度切换、Linux/macOS 核心。 */

import { useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { DState } from "./shared";
import { djImg } from "../../lib/images";
import { AdultGateModal, ADULT_SOURCES, adultAckDone, markAdultAck } from "./AdultGateModal";
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

/** 站源（id 与上游 Go sourceXxx 常量一致；顺序即展示顺序） */
const SOURCES: { id: string; label: string }[] = [
  { id: "hongguo", label: "红果" },
  { id: "jinguo", label: "剧果" },
  { id: "jinguoduanju", label: "禁果短剧" },
  { id: "yeguo", label: "野果" },
  { id: "heitao", label: "黑桃" },
  { id: "huangdou", label: "黄豆" },
];
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

export function DuanjuPane() {
  const [source, setSource] = useState("hongguo");
  // [lc-2099] 收藏：favOpen = 左栏「收藏」标签激活（与站源 chips 并列置顶）
  const [favs, setFavs] = useState<FavDrama[]>(readFavs);
  const [favOpen, setFavOpen] = useState(false);
  const [sort, setSort] = useState<SortMode>("default"); // [lc-2112] 排序（纯前端）
  // [lc-2115] 成人内容闸门：pendingAdult = 等着确认的目标站源（null = 不弹）
  const [pendingAdult, setPendingAdult] = useState<string | null>(null);
  const [cats, setCats] = useState<{ id: string; name: string }[]>([]);
  const [cat, setCat] = useState("");
  const [query, setQuery] = useState("");
  const [term, setTerm] = useState(""); // 已提交的搜索词（回车触发）
  const [items, setItems] = useState<DuanjuDrama[]>([]);
  const [page, setPage] = useState(1);
  const [hasMore, setHasMore] = useState(false);
  const [busy, setBusy] = useState(false);
  const [moreBusy, setMoreBusy] = useState(false);
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

  // 分类（随站源重取）
  useEffect(() => {
    let alive = true;
    setCats([]);
    setCat("");
    invoke<{ id: string; name: string }[]>("duanju_categories", { source })
      .then((d) => {
        if (alive && Array.isArray(d)) setCats(d);
      })
      .catch(() => {
        /* 分类拉取失败不阻塞目录（上游会回「全部」） */
      });
    return () => {
      alive = false;
    };
  }, [source]);

  // 目录第一页（站源/分类/搜索词变化即重取）
  useEffect(() => {
    if (!source) return;
    let alive = true;
    setBusy(true);
    setErr("");
    setItems([]);
    setPage(1);
    setHasMore(false);
    invoke<CatalogResp>("duanju_catalog", {
      source,
      category: cat,
      page: 1,
      query: term,
    })
      .then((d) => {
        if (!alive) return;
        setItems(Array.isArray(d?.items) ? d.items : []);
        setHasMore(d?.hasMore === true);
        setPage(d?.page || 1);
      })
      .catch((e) => {
        if (alive) setErr(String(e));
      })
      .finally(() => {
        if (alive) setBusy(false);
      });
    return () => {
      alive = false;
    };
  }, [source, cat, term, nonce]);

  async function loadMore() {
    if (moreBusy || !hasMore) return;
    setMoreBusy(true);
    try {
      const next = page + 1;
      const d = await invoke<CatalogResp>("duanju_catalog", {
        source,
        category: cat,
        page: next,
        query: term,
      });
      const fresh = Array.isArray(d?.items) ? d.items : [];
      const seen = new Set(items.map((x) => x.id));
      setItems((prev) => [...prev, ...fresh.filter((x) => !seen.has(x.id))]);
      setHasMore(d?.hasMore === true);
      setPage(d?.page || next);
    } catch (e) {
      setErr(String(e));
    } finally {
      setMoreBusy(false);
    }
  }

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

  async function openDetail(d: DuanjuDrama) {
    setDetail(null);
    setDBusy(true);
    setDErr("");
    setEpGroup(0);
    setPlayErr("");
    setPlayingIdx(-1);
    try {
      const resp = await invoke<DetailResp>("duanju_detail", { drama: d });
      setDetail({ drama: resp?.drama || d, chapters: resp?.chapters || [] });
      // [lc-2123] 读该剧的观看进度（独立存储，未看过返回 null）
      invoke<DjHistory | null>("duanju_history_one", { id: d.id })
        .then((h) => setHist(h))
        .catch(() => setHist(null));
    } catch (e) {
      setDErr(String(e));
    } finally {
      setDBusy(false);
    }
  }

  async function play(idx: number, chapter: DuanjuChapter, name: string) {
    if (playingIdx >= 0 || !detail?.drama) return;
    setPlayErr("");
    setPlayingIdx(idx);
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
    }
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
  const shownItems = useMemo(() => sortItems(items, sort), [items, sort]);
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
        {SOURCES.map((s) => (
          <button
            key={s.id}
            type="button"
            className={`dj-chip${!favOpen && source === s.id ? " on" : ""}`}
            onClick={() => {
              // [lc-2115] 红果之后的站源含成人内容：首次切换弹完整条款确认，
              // 同意后写 localStorage 不再打扰（红果自身不拦）。
              if (ADULT_SOURCES.has(s.id) && !adultAckDone()) {
                setPendingAdult(s.id);
                return;
              }
              setFavOpen(false);
              setSource(s.id);
              setTerm("");
              setQuery("");
            }}
          >
            {s.label}
          </button>
        ))}
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
      ) : busy ? (
        <div className="dlib-stub">
          <div className="dlib-stub-t">加载中…</div>
        </div>
      ) : err ? (
        <DState err={err} emptyText="" onRetry={() => setNonce((n) => n + 1)} />
      ) : items.length === 0 ? (
        <DState
          err=""
          emptyText={term ? `未找到与「${term}」相关的短剧` : "暂无内容"}
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
          {hasMore && (
            <div className="dj-more">
              <button type="button" className="dj-morebtn" disabled={moreBusy} onClick={loadMore}>
                {moreBusy ? "加载中…" : "加载更多"}
              </button>
            </div>
          )}
        </>
      )}

      {/* 详情浮层：**Portal 挂 body**（[lc-2091] 修「点开只有黑遮罩」——原先 absolute
          相对 .dj 网格容器定位，容器高数千像素，place-items:center 把面板居中到滚动内容
          中段，用户停在列表顶部时面板落在视口外，只剩铺满的遮罩）。fixed + Portal 后
          遮罩与面板都以视口为参照，与滚动位置无关。 */}
      {(dBusy || detail || dErr) && createPortal(
        <div className="dj-mask" onClick={() => !dBusy && setDetail(null)}>
          <div className="dj-detail" onClick={(e) => e.stopPropagation()}>
            {dBusy ? (
              <div className="dj-detail-tip">加载中…</div>
            ) : dErr ? (
              <>
                <div className="dj-detail-tip">{dErr}</div>
                <button type="button" className="dj-morebtn" onClick={() => setDetail(null)}>
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
                    onClick={() => setDetail(null)}
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
                  <div className="dj-src">站源数据 guoapp（红果鉴，GPL-3.0）</div>
                </>
              )
            )}
          </div>
        </div>,
        document.body,
      )}

      {/* [lc-2115] 成人内容闸门：确认后写入 localStorage 并切到该站源 */}
      {pendingAdult && (
        <AdultGateModal
          onAgree={() => {
            const target = pendingAdult;
            markAdultAck();
            setPendingAdult(null);
            setFavOpen(false);
            setSource(target);
            setTerm("");
            setQuery("");
          }}
          onBack={() => setPendingAdult(null)}
        />
      )}
    </div>
  );
}
