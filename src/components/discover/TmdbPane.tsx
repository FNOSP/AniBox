/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Film as IcoFilmL, Star as IcoStarL } from "lucide-react";
import { DState, Dsk, TmdbUpResp, UP_MAX_PAGE } from "./shared";
import { tmdbImg } from "../../lib/images";
import { TmdbDiscoverItem } from "../../types";

export function TmdbFeatured({
  it,
  isMovie,
  onOpenDetail,
}: {
  it: TmdbDiscoverItem;
  isMovie: boolean;
  onOpenDetail: (it: TmdbDiscoverItem) => void;
}) {
  const state = it.state === "upcoming" ? "未开播" : it.state === "airing" ? "已开播" : "";
  return (
    <div className="dfeat">
      <div className="dfeat-poster">
        <Dsk cls="dsk-poster" />
        {it.poster ? (
          <img src={tmdbImg(it.poster, "w500")} alt="" draggable={false} />
        ) : (
          <span className="dfeat-poster-empty" aria-hidden="true">
            <IcoFilmL size={28} strokeWidth={1.8} />
          </span>
        )}
      </div>
      <div className="dfeat-info">
        <div className="dfeat-title">{it.title}</div>
        <div className="dfeat-meta">
          {it.rating > 0 && (
            <span className="dfeat-score">
              <IcoStarL size={14} strokeWidth={2} className="rating-ico fill" /> {it.rating.toFixed(1)}
            </span>
          )}
          <span>{it.year || "—"}</span>
          {state && <span>· {state}</span>}
          {it.votes > 0 && <span>· {it.votes.toLocaleString("zh-CN")} 人评分</span>}
          <span className="dfeat-kind">{isMovie ? "电影" : "剧集"}</span>
        </div>
        {it.overview ? <p className="dfeat-desc">{it.overview}</p> : null}
        <button className="dfeat-btn" onClick={() => onOpenDetail(it)}>
          查看详情
        </button>
      </div>
    </div>
  );
}

/** [lc-123-rev9] 标准海报卡（设计稿 .tmdb-card）：**2:3 竖版海报**（不再裁 16:9）+
 *  右上评分角标 + 下方两行（标题 / 年份·状态 pill）。`minmax(140px,1fr)` 自适应 2~4 列。
 *  缺 poster → 占位块（Film 图标），不破格。 */

export function TmdbPC({ it, onOpen }: { it: TmdbDiscoverItem; onOpen: (it: TmdbDiscoverItem) => void }) {
  const tip = [it.title, it.overview].filter(Boolean).join("\n\n");
  const state = it.state === "upcoming" ? "未开播" : it.state === "airing" ? "已开播" : "";
  return (
    <div className="dpc" title={tip} onClick={() => onOpen(it)}>
      <div className="dpc-img">
        <Dsk cls="dsk-poster" />
        {it.poster ? (
          <img src={tmdbImg(it.poster, "w342")} alt="" loading="lazy" draggable={false} />
        ) : (
          <span className="dpc-empty" aria-hidden="true">
            <IcoFilmL size={22} strokeWidth={1.8} />
          </span>
        )}
        {it.rating > 0 && <span className="dpc-rate">{it.rating.toFixed(1)}</span>}
      </div>
      <div className="dpc-bd">
        <div className="dpc-t">{it.title}</div>
        <div className="dpc-s">
          <span>{it.year || "—"}</span>
          {state && <span className="dpill">{state}</span>}
        </div>
      </div>
    </div>
  );
}

/** [lc-123-rev9] 新形状骨架：主推分栏 + 海报网格卡（与真内容一致，避免加载完成跳动） */

export function TmdbSeasonSkeleton() {
  return (
    <>
      <div className="dfeat dfeat-sk">
        <div className="dfeat-poster">
          <Dsk cls="dsk-poster" />
        </div>
        <div className="dfeat-info">
          <Dsk cls="dsk-line" />
          <Dsk cls="dsk-line short" />
          <Dsk cls="dsk-line" />
          <Dsk cls="dsk-line short" />
        </div>
      </div>
      <div className="dpcgrid">
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <div key={i} className="dpc">
            <div className="dpc-img">
              <Dsk cls="dsk-poster" />
            </div>
            <div className="dpc-bd">
              <Dsk cls="dsk-line" />
              <Dsk cls="dsk-line short" />
            </div>
          </div>
        ))}
      </div>
    </>
  );
}

/** [lc-123-rev] 「即将播出」紧凑行：日期徽章 + 一行标题 + 类型小标签（纯展示，无跳转）。 */

export function UpRow({ it, isMovie }: { it: TmdbDiscoverItem; isMovie: boolean }) {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(it.date || "");
  const tip = [it.title, it.date, it.overview].filter(Boolean).join("\n\n");
  return (
    <li className="dup" title={tip}>
      <span className="dup-d">
        {m ? (
          <>
            <b>{String(Number(m[3]))}</b>
            {String(Number(m[2]))} 月
          </>
        ) : (
          "待定"
        )}
      </span>
      <span className="dup-t">{it.title}</span>
      <span className="dpill">{isMovie ? "电影" : "剧集"}</span>
    </li>
  );
}

/** 左栏：TMDB。`active` = 首次激活闸门（懒加载）。取数走 tmdb_discover（一条命令带 mode 参数）。 */

export function TmdbPane({
  active,
  onOpenDetail,
}: {
  active: boolean;
  /** [lc-1444] 「查看详情」→ 直接进主详情页（复用 DetailView；父级合成 MediaItem） */
  onOpenDetail: (it: TmdbDiscoverItem, mediaType: "tv" | "movie") => void;
}) {
  const [mediaType, setMediaType] = useState<"tv" | "movie">("tv");
  const [season, setSeason] = useState<"current" | "next">("current");
  const [items, setItems] = useState<TmdbDiscoverItem[]>([]);
  const [range, setRange] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [upcoming, setUpcoming] = useState<TmdbDiscoverItem[]>([]);
  const [upBusy, setUpBusy] = useState(false);
  const [upErr, setUpErr] = useState("");
  const [upMoreErr, setUpMoreErr] = useState(""); // 追加页失败：只提示，不动已显示的列表
  const [upPage, setUpPage] = useState(1); // 已载入页数（1..UP_MAX_PAGE）
  const [upTotal, setUpTotal] = useState(0); // 上游总条数（0=未知）
  const [upTotalPages, setUpTotalPages] = useState(0);
  const [upMoreBusy, setUpMoreBusy] = useState(false);
  const [nonce, setNonce] = useState(0); // 两块共用的**失败重试**触发器（force + noEtag：强制全量）
  const [probe, setProbe] = useState(0); // [lc-123-rev5] 10 分钟后台探测（force=false：命令层先读缓存判 TTL）
  // 失败重试与"到期探测"走**不同**的 force 语义：重试 = force + noEtag（强制全量，别拿坏缓存反复撞）
  const retryRef = useRef(false);
  const hardRetry = useCallback(() => {
    retryRef.current = true;
    setNonce((n) => n + 1);
  }, []);
  // changed=false（缓存命中/304/内容未变）时**不碰 UI**：用 ref 记"已经渲染过哪一份数据"
  const appliedKeyRef = useRef("");
  const itemsRef = useRef<TmdbDiscoverItem[]>([]);
  const upCountRef = useRef(0);
  useEffect(() => {
    itemsRef.current = items;
  }, [items]);
  useEffect(() => {
    upCountRef.current = upcoming.length;
  }, [upcoming]);

  // 当季/下季（切换 剧集/电影 或 本季/下季 → 重取；active 才取）
  useEffect(() => {
    if (!active) return;
    let alive = true;
    setBusy(true);
    setErr("");
    const hard = retryRef.current; // 失败重试：绕过本地缓存且**不带 ETag**（强制全量）
    retryRef.current = false;
    invoke<{ items?: TmdbDiscoverItem[]; gte?: string; lte?: string; changed?: boolean }>(
      "tmdb_discover",
      // 常规（进页面/10 分钟探测）：force=false ⇒ 命令层「先读 12h 磁盘缓存 → 未过期直接返回（零网络）
      // → 过期才带 ETag 探测（304 就只顺延时间戳）」
      { arg: { mode: "season", mediaType, season, force: hard, noEtag: hard } },
    )
      .then((d) => {
        if (!alive) return;
        const key = `${mediaType}|${season}`;
        // changed=false（缓存命中 / 304 / 200 但内容未变）且这份数据已经渲染过 → **一个 state 都不碰**
        // （零 re-render、滚动位置不动）。只有 内容变了 / 首次 / 换了 key 才落 state。
        if (d?.changed === false && appliedKeyRef.current === key && itemsRef.current.length) return;
        appliedKeyRef.current = key;
        setItems(Array.isArray(d?.items) ? d.items : []);
        setRange(d?.gte && d?.lte ? `${d.gte} ~ ${d.lte}` : "");
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
  }, [active, mediaType, season, nonce, probe]);

  // [lc-123-rev5] 后台探测：每 10 分钟一次 —— **304 极便宜**（未变只传几十字节），
  // 且命令层只在 TTL 过期时才真发请求；未变时 changed=false ⇒ 上面那个分支一个 state 都不碰。
  // 只探当季列表：即将播出是"加载更多"累加出来的列表（用户翻过页就不该被后台缩回去）。
  useEffect(() => {
    if (!active) return;
    const t = window.setInterval(() => setProbe((n) => n + 1), 10 * 60 * 1000);
    return () => window.clearInterval(t);
  }, [active]);

  // 即将播出（同一 mediaType；**不带季度窗口**）：先取第一页，后续页由「加载更多」追
  useEffect(() => {
    if (!active) return;
    let alive = true;
    setUpBusy(true);
    setUpErr("");
    setUpMoreErr("");
    setUpPage(1);
    const hard = retryRef.current;
    retryRef.current = false;
    invoke<TmdbUpResp & { changed?: boolean }>("tmdb_discover", {
      // 常规 force=false：先读磁盘缓存（即将播出 TTL 24h）；失败重试才 force+noEtag 强制全量
      arg: { mode: "upcoming", mediaType, page: 1, force: hard, noEtag: hard },
    })
      .then((d) => {
        if (!alive) return;
        if (d?.changed === false && upCountRef.current > 0) return; // 内容没变 → 不碰 UI
        setUpcoming(Array.isArray(d?.items) ? d.items : []);
        setUpTotal(typeof d?.total === "number" ? d.total : 0);
        setUpTotalPages(typeof d?.totalPages === "number" ? d.totalPages : 0);
      })
      .catch((e) => {
        if (alive) setUpErr(String(e));
      })
      .finally(() => {
        if (alive) setUpBusy(false);
      });
    return () => {
      alive = false;
    };
  }, [active, mediaType, nonce]);

  const featured = items[0]; // 设计稿：主推 = 热度第一，无轮播
  const gridItems = items.slice(1);
  const upLastPage = Math.min(upTotalPages > 0 ? upTotalPages : UP_MAX_PAGE, UP_MAX_PAGE);
  const upMoreAvail =
    upcoming.length > 0 && upPage < upLastPage && (upTotal === 0 || upcoming.length < upTotal);

  // 「加载更多」：**只追加**、不重置已显示内容（先出列表，追加失败只在尾部提示）
  async function loadMoreUpcoming() {
    if (upMoreBusy) return;
    const next = upPage + 1;
    setUpMoreBusy(true);
    setUpMoreErr("");
    try {
      const d = await invoke<TmdbUpResp>("tmdb_discover", {
        arg: { mode: "upcoming", mediaType, page: next, force: false },
      });
      const more = Array.isArray(d?.items) ? d.items : [];
      setUpcoming((prev) => {
        const seen = new Set(prev.map((x) => `${x.tmdbId}-${x.date}`));
        return [...prev, ...more.filter((x) => !seen.has(`${x.tmdbId}-${x.date}`))];
      });
      setUpPage(next);
      if (typeof d?.total === "number" && d.total > 0) setUpTotal(d.total);
      if (typeof d?.totalPages === "number" && d.totalPages > 0) setUpTotalPages(d.totalPages);
    } catch (e) {
      setUpMoreErr(String(e));
    } finally {
      setUpMoreBusy(false);
    }
  }

  // [lc-1444] 「查看详情」：**直接进主详情页**（复用 DetailView，选集信息占位）。
  //   TMDB 条目没有飞牛 guid —— 由父级合成 MediaItem（guid 用 discover-<tmdbId> 占位标识）。
  function openDetail(it: TmdbDiscoverItem) {
    onOpenDetail(it, mediaType);
  }

  const isMovie = mediaType === "movie";
  return (
    <section className="dpane">
      <header className="dpane-hd">
        <h2>{isMovie ? "当季热门电影" : "当季热门剧集"}</h2>
        <span className="dn">{items.length} 部</span>
        <span className="dsp" />
        <div className="dchips">
          <button className={`dchip${!isMovie ? " on" : ""}`} onClick={() => setMediaType("tv")}>
            剧集
          </button>
          <button className={`dchip${isMovie ? " on" : ""}`} onClick={() => setMediaType("movie")}>
            电影
          </button>
          <button
            className={`dchip${season === "current" ? " on" : ""}`}
            onClick={() => setSeason("current")}
          >
            本季
          </button>
          <button className={`dchip${season === "next" ? " on" : ""}`} onClick={() => setSeason("next")}>
            下季
          </button>
        </div>
      </header>
      <div className="dscroll">
        {range && (
          <div className="dcap">
            {season === "next" ? "下季" : "本季"} {range} · 共 {items.length} 部
          </div>
        )}
        {busy && !items.length ? (
          <TmdbSeasonSkeleton />
        ) : err && !items.length ? (
          <DState err={err} emptyText="" onRetry={hardRetry} />
        ) : !items.length ? (
          <DState
            err=""
            emptyText={`${season === "next" ? "下季" : "本季"}暂无数据`}
            onRetry={hardRetry}
          />
        ) : (
          <>
            {featured && (
              <TmdbFeatured
                it={featured}
                isMovie={isMovie}
                onOpenDetail={openDetail}
              />
            )}
            {gridItems.length > 0 && (
              <>
                <h3 className="dsub-h">
                  {season === "next" ? "下季看点" : "本季新剧"}
                  <span>按热度排序</span>
                </h3>
                <div className="dpcgrid">
                  {gridItems.map((it) => (
                    <TmdbPC key={`${it.tmdbId}-${it.date}`} it={it} onOpen={openDetail} />
                  ))}
                </div>
              </>
            )}
          </>
        )}
        <h3 className="dsub-h">
          {isMovie ? "即将上映" : "即将播出"}
          <span>按首播日排序</span>
        </h3>
        {upBusy && !upcoming.length ? (
          <ul className="duplist">
            {[0, 1, 2, 3].map((i) => (
              <li key={i} className="dup">
                <Dsk cls="dsk-date" />
                <div className="dup-t">
                  <Dsk cls="dsk-line" />
                </div>
              </li>
            ))}
          </ul>
        ) : upErr && !upcoming.length ? (
          <DState err={upErr} emptyText="" onRetry={hardRetry} />
        ) : !upcoming.length ? (
          <div className="dstate">暂无即将播出的条目</div>
        ) : (
          <>
            <ul className="duplist">
              {upcoming.map((it) => (
                <UpRow key={`${it.tmdbId}-${it.date}`} it={it} isMovie={isMovie} />
              ))}
            </ul>
            {/* [lc-1416] 按钮类名 .dmore → **.dmore-btn**：原来它与「演职人员」页的**容器** .dmore 撞名，
                而 `.dmore` 那条选择器同样命中那个 block 级、撑满宽度的 div →
                容器套上了胶囊样式（border-radius:999px：上下半圆 + 中间矩形）与 :hover 高亮。
                用户看到的"背景有个大椭圆、鼠标放上去还高亮"就是这个**类名冲突**，不是几何绘制。 */}
            {upMoreAvail && (
              <button className="dmore-btn" onClick={loadMoreUpcoming} disabled={upMoreBusy}>
                {upMoreBusy ? "加载中…" : "加载更多"}
              </button>
            )}
            {/* 上游页数被后端夹在上限内：这里如实说明"只显示到第 N 部"，不谎称"已全部加载" */}
            {!upMoreAvail && upPage >= upLastPage && upTotal > upcoming.length && (
              <div className="dup-hint">
                已显示前 {upcoming.length} 部 · 共 {upTotal} 部
              </div>
            )}
            {upMoreErr && <div className="dup-hint">加载失败：{upMoreErr}</div>}
          </>
        )}
      </div>
    </section>
  );
}

/* ───────────────────────── 右栏：Bangumi 每日新番 ───────────────────────── */

/* ── [lc-123-rev] 在库匹配（best-effort；宁可少标，绝不谎报） ──
   取数方式：复用**已有的** get_items_global（typeFilter 白名单 Movie/TV，与主仓库「库索引」同一口径），
   按页拉标题集合。为什么建索引：本仓库**没有**"按标题查库"的命令（fnapi.rs 只有 item/list 等既有端点），
   逐条网络匹配（一天 10~26 条、一周 105 条）会把 NAS 打爆 —— 一次性分页索引是唯一省的方式。
   诚实边界：索引**分页到底**才算完整 ⇒ "ok"（可分组：在库/不在库）；
   拉满上限还没到底 ⇒ "partial"（只给命中项打「已在库」标，**不分组、不标"不在库"**）；
   拉失败或空库 ⇒ "none"（整个分组退场）。三种状态下列表都照常渲染（匹配在后台补，不阻塞）。 */
