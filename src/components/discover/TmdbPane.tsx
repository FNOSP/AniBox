/* [lc-1577] 发现页左栏 TMDB 面板 —— 按用户设计稿（`媒体库_动画版.html`）整体重做。
   结构照设计稿三段：
     ① 顶行：TMDB 品牌胶囊 + 概览（共 N 部作品 / 平均分 / 最高分 / 更新于）+ 搜索框
     ② 次行：主标签（节目/电影）+ 子标签（当季/热门/即将播出）+ 视图切换（时间线/网格/紧凑列表）+ 排序（按时间/按评分）
     ③ 内容区三视图（时间线按月分组、按评分时=排行榜 / 海报网格 / 紧凑列表）
   数据全部真实接 TMDB：`tmdb_discover`（当季/热门/即将播出）+ `tmdb_search`（搜索框）。
   设计稿里的假数据一律不要；颜色一律走主题变量（**不照抄设计稿的白色底/浅灰渐变**，那会破深色主题）。
   类名一律 `dlib-` 前缀（根容器 `dlib`）—— 本项目踩过 `.dmore` 撞名的坑（用户看到莫名的大椭圆）。 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CalendarDays as IcoCalendarL,
  Clock as IcoClockL,
  Film as IcoFilmL,
  Flame as IcoFlameL,
  LayoutGrid as IcoGridL,
  LayoutList as IcoListL,
  Rows3 as IcoRowsL,
  Search as IcoSearchL,
  Star as IcoStarL,
  Tv as IcoTvL,
  X as IcoXL,
} from "lucide-react";
import { DState, Dsk, UP_MAX_PAGE } from "./shared";
import { tmdbImg } from "../../lib/images";
import { TmdbDiscoverItem } from "../../types";

type MediaKind = "tv" | "movie";
type SubTab = "season" | "popular" | "upcoming";
type ViewMode = "timeline" | "grid" | "list";
type SortMode = "time" | "rating";

/** `tmdb_discover` 的返回（items + 分页/窗口元信息；`changed===false` = 缓存命中/304，**一个 state 都不许碰**） */
type DiscoverResp = {
  items?: TmdbDiscoverItem[];
  total?: number;
  totalPages?: number;
  changed?: boolean;
};

/** `tmdb_search` 的返回（与 discover 同一套 item 形状） */
type SearchResp = {
  items?: TmdbDiscoverItem[];
  total?: number;
  totalPages?: number;
};

const WEEKDAYS_CN = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

/** 子标签 → 徽章文案/配色（当季=绿系、热门=主色系、即将播出=橙系） */
const SUB_LABEL: Record<SubTab, string> = { season: "当季", popular: "热门", upcoming: "即将播出" };
const SUB_TONE: Record<SubTab, string> = { season: "airing", popular: "hot", upcoming: "soon" };

/** 子标签 → 概览标题（照设计稿：当季新剧/当季新片、热门剧集/热门电影、即将开播/即将上映） */
const SUB_TITLE: Record<MediaKind, Record<SubTab, string>> = {
  tv: { season: "当季新剧", popular: "热门剧集", upcoming: "即将开播" },
  movie: { season: "当季新片", popular: "热门电影", upcoming: "即将上映" },
};
/** 概览标题的括号说明（「当季」那格按**当天星期几**现算，见 JSX；与设计稿一致） */
const SUB_PAREN: Record<SubTab, string> = { season: "", popular: "（全时段）", upcoming: "（待播）" };

/** 「更新于」：形如「周三 下午 9:12」（本机时区、12 小时制；中文上午/下午，非设计稿的 AM/PM） */
function nowLabel(ts: number): string {
  const d = new Date(ts);
  const h = d.getHours();
  const h12 = h % 12 || 12;
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${WEEKDAYS_CN[d.getDay()]} ${h >= 12 ? "下午" : "上午"} ${h12}:${mm}`;
}

/** "YYYY-MM-DD" → {y,m,d}；空/非法日期返回 null（发现页偶有缺日期的条目） */
function parseYmd(s: string): { y: number; m: number; d: number } | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec((s || "").trim());
  if (!m) return null;
  return { y: Number(m[1]), m: Number(m[2]), d: Number(m[3]) };
}

function weekdayOf(s: string): string {
  const p = parseYmd(s);
  if (!p) return "";
  return WEEKDAYS_CN[new Date(p.y, p.m - 1, p.d).getDay()] || "";
}

/** 评分色阶：≥8 绿 / 6–8 黄 / <6 红（与既有 .dsc 同一套口径） */
function scoreClass(r: number): "hi" | "mid" | "lo" {
  return r >= 8 ? "hi" : r >= 6 ? "mid" : "lo";
}

/** 副标题：**真实数据里没有「副标题」字段**（设计稿那条是编的）——
 *  originalTitle 非空且与 title 不同就用它，否则回退「YYYY 年」。 */
function subtitleOf(it: TmdbDiscoverItem): string {
  const o = (it.originalTitle || "").trim();
  if (o && o !== (it.title || "").trim()) return o;
  return it.year ? `${it.year} 年` : "—";
}

/** 原题（**仅在与 title 不同时**返回）：网格卡的「年份 · 原题」用，避免出现「2024 · 2024 年」这种重复 */
function altTitleOf(it: TmdbDiscoverItem): string {
  const o = (it.originalTitle || "").trim();
  return o && o !== (it.title || "").trim() ? o : "";
}

/** 列表去重/React key：tmdbId + 首播日（同一部剧跨页可能出现，日期一起当键更稳） */
function keyOf(it: TmdbDiscoverItem): string {
  return `${it.tmdbId}-${it.date}`;
}

function tipOf(it: TmdbDiscoverItem): string {
  return [it.title, it.date, it.overview].filter(Boolean).join("\n\n");
}

/** 海报块：**缺 poster 一律走占位块（Film 图标），不留破图** */
function DlibPoster({ it, size, cls }: { it: TmdbDiscoverItem; size: string; cls: string }) {
  return (
    <div className={cls}>
      <Dsk cls="dsk-poster" />
      {it.poster ? (
        <img src={tmdbImg(it.poster, size)} alt="" loading="lazy" draggable={false} />
      ) : (
        <span className="dlib-ph" aria-hidden="true">
          <IcoFilmL size={20} strokeWidth={1.7} />
        </span>
      )}
    </div>
  );
}

/** 子标签徽章（当季/热门/即将播出）：文案与配色由**当前子标签**决定 */
function DlibBadge({ sub }: { sub: SubTab }) {
  return <span className={`dlib-badge dlib-badge-${SUB_TONE[sub]}`}>{SUB_LABEL[sub]}</span>;
}

/** 时间线行里的横卡：竖版海报 + 标题/徽章 + 副标题 + 题材 + 2 行简介 + 右侧大号评分 */
function TimelineCard({ it, sub, onOpen }: { it: TmdbDiscoverItem; sub: SubTab; onOpen: () => void }) {
  const genres = (it.genres || []).slice(0, 4);
  return (
    <div className="dlib-card" title={tipOf(it)} onClick={onOpen}>
      <DlibPoster it={it} size="w185" cls="dlib-poster" />
      <div className="dlib-cinfo">
        <div className="dlib-chead">
          <span className="dlib-ctitle">{it.title}</span>
          <DlibBadge sub={sub} />
        </div>
        <div className="dlib-csub">{subtitleOf(it)}</div>
        {genres.length > 0 && (
          <div className="dlib-genres">
            {genres.map((g) => (
              <span key={g} className="dlib-genre">
                {g}
              </span>
            ))}
          </div>
        )}
        {it.overview ? <div className="dlib-cov">{it.overview}</div> : null}
      </div>
      <div className="dlib-cscore">
        {it.rating > 0 ? (
          <>
            <b className={`dlib-scorenum ${scoreClass(it.rating)}`}>{it.rating.toFixed(1)}</b>
            <span className="dlib-scorelab">评分</span>
          </>
        ) : (
          <span className="dlib-scorelab">暂无评分</span>
        )}
      </div>
    </div>
  );
}

/** 骨架：**按当前视图摆形状**（贴近真实内容，避免加载完跳动） */
function DlibSkeleton({ view }: { view: ViewMode }) {
  if (view === "grid") {
    return (
      <div className="dlib-skgrid">
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <div key={i} className="dlib-skg">
            <div className="dlib-skgp">
              <Dsk cls="dsk-poster" />
            </div>
            <div className="dlib-sklines">
              <Dsk cls="dsk-line" />
              <Dsk cls="dsk-line short" />
            </div>
          </div>
        ))}
      </div>
    );
  }
  if (view === "list") {
    return (
      <div className="dlib-sklist">
        {[0, 1, 2, 3, 4].map((i) => (
          <div key={i} className="dlib-skl">
            <div className="dlib-sklp">
              <Dsk cls="dsk-poster" />
            </div>
            <div className="dlib-sklines">
              <Dsk cls="dsk-line" />
              <Dsk cls="dsk-line short" />
            </div>
          </div>
        ))}
      </div>
    );
  }
  return (
    <div className="dlib-sktl">
      {[0, 1, 2, 3].map((i) => (
        <div key={i} className="dlib-skrow">
          <div className="dlib-skdate">
            <Dsk cls="dsk-line" />
          </div>
          <div className="dlib-skdot" />
          <div className="dlib-skcard">
            <div className="dlib-skp">
              <Dsk cls="dsk-poster" />
            </div>
            <div className="dlib-sklines">
              <Dsk cls="dsk-line" />
              <Dsk cls="dsk-line short" />
              <Dsk cls="dsk-line" />
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}

/** 左栏：TMDB。`active` = 首次激活闸门（懒加载）。导出签名与 props 名保持不变（DiscoverView 依赖）。 */
export function TmdbPane({
  active,
  onOpenDetail,
}: {
  active: boolean;
  /** [lc-1444] 点条目 → 直接进主详情页（复用 DetailView；父级合成 MediaItem） */
  onOpenDetail: (it: TmdbDiscoverItem, mediaType: "tv" | "movie") => void;
}) {
  const [mediaType, setMediaType] = useState<MediaKind>("tv");
  const [subTab, setSubTab] = useState<SubTab>("season");
  const [view, setView] = useState<ViewMode>("timeline");
  const [sort, setSort] = useState<SortMode>("time");

  // ── 当前子标签的列表（当季/热门/即将播出共用一份；切标签即清空重取） ──
  const [items, setItems] = useState<TmdbDiscoverItem[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [page, setPage] = useState(1); // 「即将播出」已载入页数（1..UP_MAX_PAGE）
  const [total, setTotal] = useState(0); // 上游总条数（0 = 未知）
  const [totalPages, setTotalPages] = useState(0);
  const [moreBusy, setMoreBusy] = useState(false);
  const [moreErr, setMoreErr] = useState(""); // 追加页失败：只提示，不动已显示的列表

  // ── 搜索（回车触发；结果与标签列表共用同一套三视图） ──
  const [query, setQuery] = useState("");
  const [searchTerm, setSearchTerm] = useState("");
  const [sItems, setSItems] = useState<TmdbDiscoverItem[]>([]);
  const [sBusy, setSBusy] = useState(false);
  const [sErr, setSErr] = useState("");

  const [now, setNow] = useState(() => Date.now()); // 「更新于」每分钟刷新一次
  const [nonce, setNonce] = useState(0); // 失败重试触发器（force + noEtag：强制全量）
  const [probe, setProbe] = useState(0); // [lc-123-rev5] 10 分钟后台探测（force=false：命令层先读缓存判 TTL）

  // 失败重试与"到期探测"走**不同**的 force 语义：重试 = force + noEtag（强制全量，别拿坏缓存反复撞）
  const retryRef = useRef(false);
  const hardRetry = useCallback(() => {
    retryRef.current = true;
    setNonce((n) => n + 1);
  }, []);

  // changed=false（缓存命中/304/内容未变）时**不碰 UI**：用 ref 记"已经渲染过哪一份数据"
  const appliedKeyRef = useRef("");
  const keyRef = useRef(""); // 当前列表对应的 (mediaType|subTab)
  const itemsRef = useRef<TmdbDiscoverItem[]>([]);
  useEffect(() => {
    itemsRef.current = items;
  }, [items]);

  // 「更新于」：每分钟重算一次（只影响栏头文本，内容区不重挂 ⇒ 不重播入场动画）
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(t);
  }, []);

  // 取数：当季/热门各 1 页；即将播出先取第 1 页（后续页由「加载更多」追）
  useEffect(() => {
    if (!active) return;
    const key = `${mediaType}|${subTab}`;
    const switched = keyRef.current !== key;
    keyRef.current = key;
    if (switched) {
      // 切标签：清空对应状态 + 回骨架（骨架形状贴内容，避免加载完跳动）
      setItems([]);
      setPage(1);
      setTotal(0);
      setTotalPages(0);
      setMoreErr("");
    }
    let alive = true;
    setBusy(true);
    setErr(""); // 每次取数（含重试/探测）都先清掉上一次的错误
    const hard = retryRef.current; // 失败重试：绕过本地缓存且**不带 ETag**（强制全量）
    retryRef.current = false;
    invoke<DiscoverResp>("tmdb_discover", {
      // 常规（进页面/10 分钟探测）：force=false ⇒ 命令层「先读磁盘缓存 → 未过期直接返回（零网络）
      // → 过期才带 ETag 探测（304 就只顺延时间戳）」
      arg: { mode: subTab, mediaType, season: "current", page: 1, force: hard, noEtag: hard },
    })
      .then((d) => {
        if (!alive) return;
        // changed=false（缓存命中 / 304 / 200 但内容未变）且这份数据已经渲染过 → **一个 state 都不碰**
        // （零 re-render、滚动位置不动）。只有 内容变了 / 首次 / 换了 key 才落 state。
        if (d?.changed === false && appliedKeyRef.current === key && itemsRef.current.length) return;
        appliedKeyRef.current = key;
        setItems(Array.isArray(d?.items) ? d.items : []);
        setTotal(typeof d?.total === "number" ? d.total : 0);
        setTotalPages(typeof d?.totalPages === "number" ? d.totalPages : 0);
        setPage(1);
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
  }, [active, mediaType, subTab, nonce, probe]);

  // [lc-123-rev5] 后台探测：每 10 分钟一次 —— **304 极便宜**（未变只传几十字节），
  // 且命令层只在 TTL 过期时才真发请求；未变时 changed=false ⇒ 上面那个分支一个 state 都不碰。
  // 只探**当前活动标签**：即将播出是"加载更多"累加出来的列表（用户翻过页就不该被后台缩回去）。
  useEffect(() => {
    if (!active || subTab === "upcoming") return;
    const t = window.setInterval(() => setProbe((n) => n + 1), 10 * 60 * 1000);
    return () => window.clearInterval(t);
  }, [active, subTab]);

  const upLastPage = Math.min(totalPages > 0 ? totalPages : UP_MAX_PAGE, UP_MAX_PAGE);
  const moreAvail =
    subTab === "upcoming" &&
    !searchTerm &&
    items.length > 0 &&
    page < upLastPage &&
    (total === 0 || items.length < total);

  // 「加载更多」：**只追加**、不重置已显示内容（先出列表，追加失败只在尾部提示）
  async function loadMore() {
    if (moreBusy) return;
    const next = page + 1;
    setMoreBusy(true);
    setMoreErr("");
    try {
      const d = await invoke<DiscoverResp>("tmdb_discover", {
        arg: { mode: "upcoming", mediaType, page: next, force: false },
      });
      const more = Array.isArray(d?.items) ? d.items : [];
      setItems((prev) => {
        const seen = new Set(prev.map(keyOf));
        return [...prev, ...more.filter((x) => !seen.has(keyOf(x)))];
      });
      setPage(next);
      if (typeof d?.total === "number" && d.total > 0) setTotal(d.total);
      if (typeof d?.totalPages === "number" && d.totalPages > 0) setTotalPages(d.totalPages);
    } catch (e) {
      setMoreErr(String(e));
    } finally {
      setMoreBusy(false);
    }
  }

  // ── 搜索 ──
  const searchSeqRef = useRef(0); // 竞态护栏：只认最后一次搜索的响应
  const doSearch = useCallback((q: string, mt: MediaKind) => {
    const term = q.trim();
    const seq = ++searchSeqRef.current;
    if (!term) {
      setSearchTerm("");
      setSItems([]);
      setSErr("");
      setSBusy(false);
      return;
    }
    setSearchTerm(term);
    setSItems([]);
    setSErr("");
    setSBusy(true);
    invoke<SearchResp>("tmdb_search", { arg: { query: term, mediaType: mt, page: 1 } })
      .then((d) => {
        if (seq !== searchSeqRef.current) return;
        setSItems(Array.isArray(d?.items) ? d.items : []);
      })
      .catch((e) => {
        if (seq !== searchSeqRef.current) return;
        setSErr(String(e));
      })
      .finally(() => {
        if (seq === searchSeqRef.current) setSBusy(false);
      });
  }, []);

  const clearSearch = useCallback(() => {
    searchSeqRef.current++; // 作废在途响应
    setQuery("");
    setSearchTerm("");
    setSItems([]);
    setSErr("");
    setSBusy(false);
  }, []);

  // 切主标签：子标签重置为「当季」+ 退出搜索（搜索用的是**主标签的 mediaType**）
  function pickMedia(mt: MediaKind) {
    if (mt === mediaType) return;
    setMediaType(mt);
    setSubTab("season");
    clearSearch();
  }
  // 切子标签：退出搜索，回到该标签自己的列表
  function pickSub(s: SubTab) {
    if (s === subTab) return;
    setSubTab(s);
    clearSearch();
  }

  const searching = searchTerm !== "";
  const list = searching ? sItems : items;

  // 概览数字按**当前显示的列表**实时算（搜索时按搜索结果算）；rating=0 视为「无评分」，不参与均值（否则会把均分拉低到失真）
  const rated = list.filter((x) => x.rating > 0);
  const avg = rated.length ? rated.reduce((s, x) => s + x.rating, 0) / rated.length : 0;
  const max = rated.length ? Math.max(...rated.map((x) => x.rating)) : 0;

  const sorted = useMemo(() => {
    const arr = list.slice();
    if (sort === "rating") arr.sort((a, b) => b.rating - a.rating || a.tmdbId - b.tmdbId);
    else arr.sort((a, b) => (b.date || "").localeCompare(a.date || "") || a.tmdbId - b.tmdbId);
    return arr;
  }, [list, sort]);

  // 时间线（按时间）：按 `YYYY-MM` 分月分组（组头 = 「YYYY 年 M 月」+ N 部作品）
  const groups = useMemo(() => {
    if (sort !== "time") return null;
    const out: { key: string; label: string; rows: TmdbDiscoverItem[] }[] = [];
    const at = new Map<string, number>();
    for (const it of sorted) {
      const p = parseYmd(it.date);
      const k = p ? `${p.y}-${String(p.m).padStart(2, "0")}` : "__none__";
      let i = at.get(k);
      if (i === undefined) {
        i = out.length;
        at.set(k, i);
        out.push({ key: k, label: p ? `${p.y} 年 ${p.m} 月` : "日期待定", rows: [] });
      }
      out[i].rows.push(it);
    }
    return out;
  }, [sorted, sort]);

  const loadingNow = searching ? sBusy && sItems.length === 0 : busy && items.length === 0;
  const errNow = searching ? sErr : items.length ? "" : err;
  // 内容切换（标签/视图/排序/搜索词）时换 key ⇒ 重新挂载 ⇒ 条目依次入场（不靠每次 render 乱触发）
  const contentKey = `${mediaType}|${subTab}|${view}|${sort}|${searchTerm}`;
  const retryNow = searching ? () => doSearch(searchTerm, mediaType) : hardRetry;

  function renderTimeline() {
    if (sort === "rating") {
      // 按评分 = 「评分排行榜」：行首 #1/#2/#3…（前三名配色不同），**不再按月分组**
      return (
        <div className="dlib-tl">
          <div className="dlib-grp">
            <div className="dlib-ghd">
              <span className="dlib-gpill">
                <IcoStarL size={13} strokeWidth={2.2} />
                评分排行榜
              </span>
              <span className="dlib-gcnt">共 {sorted.length} 部作品</span>
            </div>
            <div className="dlib-rows">
              <span className="dlib-line" aria-hidden="true" />
              {sorted.map((it, i) => (
                <div
                  key={keyOf(it)}
                  className={`dlib-row${i < 3 ? ` rm${i + 1}` : ""}`}
                  style={{ animationDelay: `${Math.min(i, 8) * 0.04}s` }}
                >
                  <div className={`dlib-rank r${i + 1}`}>{`#${i + 1}`}</div>
                  <div className="dlib-dotcol">
                    <span className="dlib-dot" />
                  </div>
                  <TimelineCard it={it} sub={subTab} onOpen={() => onOpenDetail(it, mediaType)} />
                </div>
              ))}
            </div>
          </div>
        </div>
      );
    }
    return (
      <div className="dlib-tl">
        {(groups ?? []).map((g, gi) => (
          <div key={g.key} className="dlib-grp" style={{ animationDelay: `${Math.min(gi, 6) * 0.04}s` }}>
            <div className="dlib-ghd">
              <span className="dlib-gpill">
                <IcoCalendarL size={13} strokeWidth={2.2} />
                {g.label}
              </span>
              <span className="dlib-gcnt">{g.rows.length} 部作品</span>
            </div>
            <div className="dlib-rows">
              <span className="dlib-line" aria-hidden="true" />
              {g.rows.map((it, i) => {
                const p = parseYmd(it.date);
                return (
                  <div key={keyOf(it)} className="dlib-row" style={{ animationDelay: `${Math.min(i, 8) * 0.04}s` }}>
                    <div className="dlib-date">
                      <span className="dlib-day">{p ? p.d : "—"}</span>
                      <span className="dlib-wd">{p ? weekdayOf(it.date) : ""}</span>
                    </div>
                    <div className="dlib-dotcol">
                      <span className="dlib-dot" />
                    </div>
                    <TimelineCard it={it} sub={subTab} onOpen={() => onOpenDetail(it, mediaType)} />
                  </div>
                );
              })}
            </div>
          </div>
        ))}
      </div>
    );
  }

  function renderGrid() {
    return (
      <div className="dlib-gv">
        {sorted.map((it, i) => {
          const meta = [it.year, altTitleOf(it)].filter(Boolean).join(" · ");
          const genres = (it.genres || []).slice(0, 2);
          return (
            <div
              key={keyOf(it)}
              className="dlib-gcard"
              title={tipOf(it)}
              onClick={() => onOpenDetail(it, mediaType)}
              style={{ animationDelay: `${Math.min(i, 11) * 0.03}s` }}
            >
              <div className="dlib-gposter">
                <Dsk cls="dsk-poster" />
                {it.poster ? (
                  <img src={tmdbImg(it.poster, "w342")} alt="" loading="lazy" draggable={false} />
                ) : (
                  <span className="dlib-ph" aria-hidden="true">
                    <IcoFilmL size={20} strokeWidth={1.7} />
                  </span>
                )}
                <DlibBadge sub={subTab} />
                {it.rating > 0 && (
                  <span className={`dlib-gscore ${scoreClass(it.rating)}`}>
                    <IcoStarL size={10} strokeWidth={2} className="rating-ico fill" />
                    {it.rating.toFixed(1)}
                  </span>
                )}
              </div>
              <div className="dlib-ginfo">
                <div className="dlib-gtitle">{it.title}</div>
                {meta ? <div className="dlib-gmeta">{meta}</div> : null}
                {genres.length > 0 && (
                  <div className="dlib-genres">
                    {genres.map((g) => (
                      <span key={g} className="dlib-genre">
                        {g}
                      </span>
                    ))}
                  </div>
                )}
              </div>
            </div>
          );
        })}
      </div>
    );
  }

  function renderList() {
    return (
      <div className="dlib-lv">
        {sorted.map((it, i) => {
          const meta = [it.year, (it.genres || []).slice(0, 3).join(" / ")].filter(Boolean).join(" · ");
          return (
            <div
              key={keyOf(it)}
              className="dlib-lrow"
              title={tipOf(it)}
              onClick={() => onOpenDetail(it, mediaType)}
              style={{ animationDelay: `${Math.min(i, 9) * 0.035}s` }}
            >
              <DlibPoster it={it} size="w185" cls="dlib-lposter" />
              <div className="dlib-linfo">
                <div className="dlib-lhead">
                  <span className="dlib-ltitle">{it.title}</span>
                  <DlibBadge sub={subTab} />
                </div>
                <div className="dlib-lsub">{subtitleOf(it)}</div>
                {meta ? <div className="dlib-lmeta">{meta}</div> : null}
              </div>
              <div className={`dlib-lscore ${it.rating > 0 ? scoreClass(it.rating) : ""}`}>
                {it.rating > 0 ? it.rating.toFixed(1) : "—"}
              </div>
            </div>
          );
        })}
      </div>
    );
  }

  return (
    <section className="dpane">
      <div className="dlib-pane">
        {/* ① 顶行：品牌胶囊 + 概览标题 + 概览 + 搜索框（窄栏下 flex-wrap 换行堆叠，绝不横向溢出） */}
        <header className="dpane-hd dlib-hd">
          <div className="dlib-row1">
            <span className="dlib-brand">
              <IcoFilmL size={13} strokeWidth={2.4} />
              TMDB
            </span>
            {/* 概览标题（设计稿的 ov-tag）：当季新剧（周三）/ 热门剧集（全时段）/ 即将开播（待播）；
                搜索态改显示「搜索结果（N 条）」—— 它就是"当前这份列表是什么"的唯一说明 */}
            <span className="dlib-ovtag">
              {searching ? "搜索结果" : SUB_TITLE[mediaType][subTab]}
              <span className="dlib-paren">
                {searching
                  ? `（${sItems.length} 条）`
                  : subTab === "season"
                    ? `（${WEEKDAYS_CN[new Date().getDay()]}）`
                    : SUB_PAREN[subTab]}
              </span>
            </span>
            <div className="dlib-ov">
              <span className="dlib-ov-i">
                <span className="dlib-ov-k">共</span>
                <b className="dlib-ov-v accent">{list.length}</b>
                <span className="dlib-ov-k">部作品</span>
              </span>
              <span className="dlib-ov-i">
                <span className="dlib-ov-k">平均分</span>
                <b className="dlib-ov-v orange">{avg.toFixed(1)}</b>
              </span>
              <span className="dlib-ov-i">
                <span className="dlib-ov-k">最高分</span>
                <b className="dlib-ov-v green">{max.toFixed(1)}</b>
              </span>
              <span className="dlib-ov-i">
                <span className="dlib-ov-k">更新于</span>
                <b className="dlib-ov-v">{nowLabel(now)}</b>
              </span>
            </div>
            <form
              className="dlib-search"
              onSubmit={(e) => {
                e.preventDefault();
                doSearch(query, mediaType);
              }}
            >
              <IcoSearchL size={15} strokeWidth={2} className="dlib-search-ico" />
              <input
                className="dlib-search-in"
                value={query}
                placeholder="搜索电影、剧集…"
                onChange={(e) => {
                  const v = e.target.value;
                  setQuery(v);
                  // 输入清空 = 退出搜索（否则列表会停在旧结果上，看着像"卡住"）
                  if (!v.trim()) clearSearch();
                }}
              />
              {query ? (
                <button type="button" className="dlib-search-x" title="清除" onClick={clearSearch}>
                  <IcoXL size={13} strokeWidth={2.4} />
                </button>
              ) : null}
            </form>
          </div>

          {/* ② 次行：主/子标签 + 视图切换 + 排序切换 */}
          <div className="dlib-row2">
            <div className="dlib-tabs">
              <div className="dlib-tgroup">
                <button className={`dlib-tab${mediaType === "tv" ? " on" : ""}`} onClick={() => pickMedia("tv")}>
                  <IcoTvL size={14} strokeWidth={2} />
                  <span>节目</span>
                </button>
                <button className={`dlib-tab${mediaType === "movie" ? " on" : ""}`} onClick={() => pickMedia("movie")}>
                  <IcoFilmL size={14} strokeWidth={2} />
                  <span>电影</span>
                </button>
              </div>
              <span className="dlib-tdiv" aria-hidden="true" />
              <div className="dlib-tgroup">
                <button className={`dlib-tab${subTab === "season" ? " on" : ""}`} onClick={() => pickSub("season")}>
                  <IcoCalendarL size={14} strokeWidth={2} />
                  <span>当季</span>
                </button>
                <button className={`dlib-tab${subTab === "popular" ? " on" : ""}`} onClick={() => pickSub("popular")}>
                  <IcoFlameL size={14} strokeWidth={2} />
                  <span>热门</span>
                </button>
                <button className={`dlib-tab${subTab === "upcoming" ? " on" : ""}`} onClick={() => pickSub("upcoming")}>
                  <IcoClockL size={14} strokeWidth={2} />
                  <span>即将播出</span>
                </button>
              </div>
            </div>
            <div className="dlib-switch">
              <button
                className={`dlib-vbtn${view === "timeline" ? " on" : ""}`}
                title="时间线视图"
                onClick={() => setView("timeline")}
              >
                <IcoListL size={16} strokeWidth={2} />
              </button>
              <button className={`dlib-vbtn${view === "grid" ? " on" : ""}`} title="网格视图" onClick={() => setView("grid")}>
                <IcoGridL size={16} strokeWidth={2} />
              </button>
              <button className={`dlib-vbtn${view === "list" ? " on" : ""}`} title="紧凑列表" onClick={() => setView("list")}>
                <IcoRowsL size={16} strokeWidth={2} />
              </button>
            </div>
            <div className="dlib-switch dlib-sort">
              <button className={`dlib-sbtn${sort === "time" ? " on" : ""}`} onClick={() => setSort("time")}>
                <IcoClockL size={14} strokeWidth={2} />
                <span>按时间</span>
              </button>
              <button className={`dlib-sbtn${sort === "rating" ? " on" : ""}`} onClick={() => setSort("rating")}>
                <IcoStarL size={14} strokeWidth={2} />
                <span>按评分</span>
              </button>
            </div>
          </div>
        </header>

        {/* ③ 内容区（独立滚动；头部固定） */}
        <div className="dscroll dlib-scroll">
          {loadingNow ? (
            <DlibSkeleton view={view} />
          ) : errNow ? (
            <DState err={errNow} emptyText="" onRetry={retryNow} />
          ) : !list.length ? (
            <DState
              err=""
              emptyText={searching ? `未找到与「${searchTerm}」相关的条目` : `${SUB_LABEL[subTab]}暂无数据`}
              onRetry={retryNow}
            />
          ) : (
            <div className="dlib-content" key={contentKey}>
              {view === "grid" ? renderGrid() : view === "list" ? renderList() : renderTimeline()}
            </div>
          )}
          {/* 「加载更多」按钮类名用既有的 .dmore-btn（**.dmore** 是详情页容器，不能撞） */}
          {moreAvail && (
            <button className="dmore-btn" onClick={loadMore} disabled={moreBusy}>
              {moreBusy ? "加载中…" : "加载更多"}
            </button>
          )}
          {/* 上游页数被后端夹在上限内：如实说明"只显示到第 N 部"，不谎称"已全部加载" */}
          {!searching && subTab === "upcoming" && items.length > 0 && !moreAvail && page >= upLastPage && total > items.length && (
            <div className="dup-hint">
              已显示前 {items.length} 部 · 共 {total} 部
            </div>
          )}
          {!searching && subTab === "upcoming" && moreErr && <div className="dup-hint">加载失败：{moreErr}</div>}
        </div>
      </div>
    </section>
  );
}
