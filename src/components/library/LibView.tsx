/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useRef, useState, type UIEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { PosterCard } from "../common/PosterCard";
import { fetchLibPage, LIB_PAGE_SIZE, type LibPage } from "../../lib/api";
import { useRefresh } from "../../hooks/useRefresh";
import { Lib, MediaItem } from "../../types";

/** [lc-1727] 「还要不要继续翻页」的判停条件（与 Rust `should_load_more` 同口径）：
 *  - 已加载条数达到服务端 `total` → 到底；
 *  - 本页不足一页（loaded 不是 pageSize 的整数倍）→ 到底；
 *  - 空列表 / 非法 pageSize → 不翻页。
 *  total 缺失（0/未知）时退回「本页是否满页」判断 —— 满页就再要一页，空页自然停。 */
export function shouldLoadMore(loaded: number, total: number, pageSize: number): boolean {
  if (pageSize <= 0 || loaded <= 0) return false;
  if (total > 0 && loaded >= total) return false;
  return loaded % pageSize === 0;
}

export function LibView({
  lib,
  onBackHome,
  onOpenItem,
  initialFolder,
  onEditMeta,
  onRefreshMeta,
}: {
  lib: Lib;
  onBackHome: () => void;
  onOpenItem: (item: MediaItem) => void;
  /** [lc-1647] 从首页「按库横排」直接点进某个未识别文件夹时，把该文件夹作为起始层。
   *  个人视频/网盘类库（category=Others/Mix）根层往往就是文件夹，首页点它必须进库钻取，
   *  不能像以前那样落进详情页（详情页对文件夹无意义，表现为「打不开」）。 */
  initialFolder?: MediaItem | null;
  /** [lc-1654] 海报卡「编辑元数据 / 刷新元数据」由 App 注入（弹窗不在卡片内部） */
  onEditMeta?: (item: MediaItem) => void;
  onRefreshMeta?: (item: MediaItem) => void;
}) {
  // [lc-1647] 层级模型重写：每一层 = 「这一层显示的文件夹（库根为 null）+ 该层的条目列表」。
  // 旧模型只存父文件夹、当前层靠 `items` 单独存 —— 钻到第 3 层时面包屑标题不跟着变，
  // 返回上一层还会把**上上层**的列表贴回来（表现为「点进去内容不对/打不开」）。
  // [lc-1648] 每层带自增 id：异步取数回来时按 id 定位，层已被弹掉就丢弃结果 ——
  // 否则「点文件夹 → 立刻按返回」时，晚到的列表会写进**返回后的那一层**，
  // 看起来就是返回键没反应（内容纹丝不动）。
  // [lc-1727] 每层再记 `page`（已取到第几页）与 `total`（服务端总数）：
  // 滚动加载更多按层累加，返回上一层时已加载的部分原样保留，不用重拉。
  type Level = { id: number; folder: MediaItem | null; items: MediaItem[]; page: number; total: number };
  const nextLevelId = useRef(1);
  const [levels, setLevels] = useState<Level[]>([
    { id: 0, folder: initialFolder ?? null, items: [], page: 0, total: 0 },
  ]);
  const [err, setErr] = useState("");
  const [loading, setLoading] = useState(true);
  // [lc-1727] 加载更多：进行中 / 失败（失败给「重试」，不打断已显示的列表）
  const [moreBusy, setMoreBusy] = useState(false);
  const [moreErr, setMoreErr] = useState("");

  const levelsRef = useRef(levels);
  useEffect(() => {
    levelsRef.current = levels;
  }, [levels]);

  const top = levels[levels.length - 1];
  const items = top.items;
  const current = top.folder;
  const total = top.total;
  const hasMore = shouldLoadMore(items.length, total, LIB_PAGE_SIZE);

  // [lc-1727] 竞态防护：
  //  - `reqSeq`：每次「重置取数 / 加载更多」自增；回包时序号不匹配就整包丢弃 ——
  //    快速滚动连发、切换库/文件夹、点刷新，旧请求都不会覆盖新列表。
  //  - `loadingRef` / `moreBusyRef`：state 更新是异步的，滚动事件可能在一次渲染内连发多次，
  //    用 ref 做同步闸门，避免同一页被重复请求。
  const reqSeq = useRef(0);
  const loadingRef = useRef(true);
  const moreBusyRef = useRef(false);

  /** 把第一页结果写进指定层（层已不存在则丢弃） */
  const applyLevel = useCallback((levelId: number, list: MediaItem[], page: number, total: number) => {
    setLevels((ls) => {
      let hit = false;
      const next = ls.map((l) => {
        if (l.id !== levelId) return l;
        hit = true;
        return { ...l, items: list, page, total };
      });
      return hit ? next : ls;
    });
  }, []);

  /** [lc-1727] 按当前入口（库根 / 文件夹 / 收藏 / 分类）取第 page 页。 */
  const fetchPage = useCallback(
    async (parentGuid: string | null, page: number): Promise<LibPage> => {
      if (parentGuid) return fetchLibPage(lib.guid, parentGuid, page);
      // 虚拟库根层：收藏 / 全局分类浏览
      if (lib.guid === "_fav") {
        const d = await invoke<{ total?: number; list?: MediaItem[] | null }>("get_favorites", { page });
        return { list: d.list ?? [], total: Number(d.total ?? 0) || 0 };
      }
      if (lib.guid.startsWith("_cat:")) {
        // [lc-1805] 影视分类行走服务端 type；其余维度走 filters（item/list tags 透传）
        const types = catTypesRef.current ?? lib.guid.slice(5).split(",").filter(Boolean);
        const d = await invoke<{ list?: MediaItem[]; total?: number }>("get_items_global", {
          typeFilter: types,
          page,
          filters: serverFilters(),
        });
        return { list: d.list ?? [], total: Number(d.total ?? 0) || 0 };
      }
      return fetchLibPage(lib.guid, null, page, serverFilters() ?? undefined);
    },
    [lib.guid],
  );

  const load = useCallback(
    async (parentGuid: string | null, levelId?: number) => {
      // 记下这次取数要写进哪一层（层 id 在弹栈后不会复用）。
      // 钻取时由 openItem 直接传新层 id —— setLevels 是异步的，此刻 levelsRef 里还没有新层。
      const targetId = levelId ?? levelsRef.current[levelsRef.current.length - 1]?.id;
      const seq = ++reqSeq.current;
      loadingRef.current = true;
      moreBusyRef.current = false;
      setLoading(true);
      setErr("");
      setMoreErr("");
      setMoreBusy(false);
      try {
        // [lc-1727] 一律从第 1 页开始：刷新/切库/钻取都要重置分页游标。
        const d = await fetchPage(parentGuid, 1);
        if (seq !== reqSeq.current) return;
        applyLevel(targetId, d.list, 1, d.total);
        if (d.list.length === 0) {
          if (parentGuid) setErr("该文件夹为空");
          else if (lib.guid === "_fav") setErr("还没有收藏");
          else if (lib.guid.startsWith("_cat:")) setErr("该分类暂无已识别条目");
          else setErr("该库暂无已识别条目");
        }
      } catch (e) {
        if (seq !== reqSeq.current) return;
        // [lc-1647] 原来没有 try/catch：get_items 一旦 reject，setLoading(false) 永远走不到，
        // 页面就卡在「加载中…」—— 个人视频/网盘类库最常见的「打不开」表象。
        applyLevel(targetId, [], 0, 0);
        setErr(String(e));
      } finally {
        if (seq === reqSeq.current) {
          loadingRef.current = false;
          setLoading(false);
        }
      }
    },
    [fetchPage, applyLevel, lib.guid],
  );

  /** [lc-1727] 滚动加载下一页：只追加、不重置；层已弹掉 / 已有请求在飞 / 已到底时直接返回。 */
  const loadMore = useCallback(async () => {
    if (loadingRef.current || moreBusyRef.current) return;
    const lv = levelsRef.current[levelsRef.current.length - 1];
    if (!lv || !shouldLoadMore(lv.items.length, lv.total, LIB_PAGE_SIZE)) return;
    moreBusyRef.current = true;
    setMoreBusy(true);
    setMoreErr("");
    const seq = ++reqSeq.current;
    const levelId = lv.id;
    const nextPage = lv.page + 1;
    const parentGuid = lv.folder ? lv.folder.guid : null;
    try {
      const d = await fetchPage(parentGuid, nextPage);
      if (seq !== reqSeq.current) return;
      if (d.list.length === 0) {
        // 空页 = 服务端也没有更多了：把 total 收敛为已加载数，避免反复请求。
        setLevels((ls) =>
          ls.map((l) =>
            l.id === levelId
              ? { ...l, page: nextPage, total: l.items.length }
              : l,
          ),
        );
        return;
      }
      setLevels((ls) => {
        const i = ls.findIndex((l) => l.id === levelId);
        if (i < 0) return ls; // 层已被弹掉：丢弃
        const cur = ls[i];
        const seen = new Set(cur.items.map((it) => it.guid));
        const add = d.list.filter((it) => it.guid && !seen.has(it.guid));
        const merged = [...cur.items, ...add];
        // total 优先用服务端的；服务端没给就按已加载数记（等价于「不再翻」）。
        const nextTotal = d.total > 0 ? d.total : Math.max(merged.length, cur.total);
        const next = [...ls];
        next[i] = { ...cur, items: merged, page: nextPage, total: nextTotal };
        return next;
      });
    } catch (e) {
      if (seq === reqSeq.current) setMoreErr(String(e));
    } finally {
      moreBusyRef.current = false;
      if (seq === reqSeq.current) setMoreBusy(false);
    }
  }, [fetchPage]);

  useEffect(() => {
    // [lc-1647] 有起始文件夹就直接钻进去，否则从库根开始
    void load(initialFolder ? initialFolder.guid : null);
  }, [load, initialFolder]);

  /* [lc-1640] 顶栏 ⟳：强制刷新**当前所在层**（库根或正在钻取的文件夹）。
     飞牛 item/list 无 Rust 缓存（get_items 直连），load 本身就是强制重拉 —— 直接复用，
     不另写取数。用 ref 记住最新一层的 guid，避免 effect 依赖 current/load 引起的重复触发。 */
  const { tick: refreshTick, reportBusy } = useRefresh();
  const lastRefreshTick = useRef(refreshTick);
  const currentGuidRef = useRef<string | null>(null);
  useEffect(() => {
    currentGuidRef.current = current ? current.guid : null;
  }, [current]);
  useEffect(() => {
    if (refreshTick === lastRefreshTick.current) return;
    lastRefreshTick.current = refreshTick;
    reportBusy("lib", true);
    void load(currentGuidRef.current).finally(() => reportBusy("lib", false));
  }, [refreshTick, load, reportBusy]);

  /* [lc-1727] 滚动触底加载更多。
     `.content` 自己就是滚动容器（06-content.css：overflow-y:auto），所以监听它即可。
     除了 onScroll 兜底，再用 IntersectionObserver 观察列表末尾的哨兵：
     列表恰好撑不满一屏时不会有滚动事件，靠哨兵进入视口自动续取，直到填满或到底。 */
  const contentRef = useRef<HTMLElement | null>(null);
  const sentinelRef = useRef<HTMLDivElement | null>(null);

  function onScroll(e: UIEvent<HTMLElement>) {
    const el = e.currentTarget;
    if (el.scrollHeight - el.scrollTop - el.clientHeight > 480) return;
    void loadMore();
  }

  useEffect(() => {
    const root = contentRef.current;
    const target = sentinelRef.current;
    if (!root || !target || loading || moreErr) return;
    if (typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((en) => en.isIntersecting)) void loadMore();
      },
      // 提前 600px 触发，滚到底之前就把下一页接上，避免「滚到底停一下」。
      { root, rootMargin: "600px 0px" },
    );
    io.observe(target);
    return () => io.disconnect();
  }, [loading, loadMore, items.length, moreBusy, moreErr]);

  async function openItem(item: MediaItem) {
    // 只有未识别文件夹（Directory）继续逐层钻取；
    // TV/Movie/Episode/Video 一律进详情页 —— 已识别剧集的「季/集」由详情页走
    // season/list + episode/list 获取，item/list 白名单里根本没有 Season/Episode。
    if (item.type !== "Directory") {
      onOpenItem(item);
      return;
    }
    // 新开一层：上一层的列表原样留在 levels 里，返回时零请求还原
    const id = nextLevelId.current++;
    setLevels((ls) => [...ls, { id, folder: item, items: [], page: 0, total: 0 }]);
    await load(item.guid, id);
  }

  function pop() {
    // 返回上一层：该层列表还缓存在 levels 里，直接弹栈即可
    setLevels((ls) => (ls.length > 1 ? ls.slice(0, -1) : ls));
  }

  /* [lc-1744] 收藏页里取消收藏 → 立即把该卡从当前层移除（心形已在卡片内乐观变空，
     这里等 `favorite_toggle` 成功后收尾）。其它入口（库 / 分类）不动列表。 */
  const onFavoriteChange = useCallback(
    (item: MediaItem, favorited: boolean) => {
      if (lib.guid !== "_fav" || favorited) return;
      setLevels((ls) => {
        const i = ls.length - 1;
        if (i < 0) return ls;
        const cur = ls[i];
        const next = [...ls];
        next[i] = {
          ...cur,
          items: cur.items.filter((it) => it.guid !== item.guid),
          total: Math.max(0, cur.total - 1),
        };
        return next;
      });
    },
    [lib.guid],
  );

  /* ── [lc-1805] 筛选：照抄飞鲸影视（/v/list/all）——维度动态拉取（tag 三接口），
     全部服务端过滤（item/list tags 透传），分页继续服务端；面板样式沿用原生复刻。 ── */
  type Opt = { label: string; value: string | number | null };
  type Filters = {
    cat: string | null; // 影视类型（Movie/TV/LiveChannel…）——换服务端 type 重拉
    genres: number | null;
    resolution: string | null;
    colorRange: string | null;
    audioType: string | null;
    locate: string | null;
    decade: string | null;
    recognition: string | null;
    watched: string | null; // "1"/"0"
  };
  const EMPTY_F: Filters = { cat: null, genres: null, resolution: null, colorRange: null, audioType: null, locate: null, decade: null, recognition: null, watched: null };
  const [f, setF] = useState<Filters>(EMPTY_F);
  // [lc-1805b] 默认收起（用户要求）；点「筛选」展开
  const [collapsed, setCollapsed] = useState(true);
  const [dim, setDim] = useState<Record<string, Opt[]> | null>(null);
  const atRoot = levels.length === 1;
  const isAllCat = lib.guid.startsWith("_cat:");
  const filterActive =
    atRoot && (f.cat !== null || f.genres !== null || f.resolution !== null || f.colorRange !== null || f.audioType !== null || f.locate !== null || f.decade !== null || f.recognition !== null || f.watched !== null);

  const catTypesRef = useRef<string[] | null>(null);
  useEffect(() => {
    catTypesRef.current = null;
    setF(EMPTY_F);
    setDim(null);
  }, [lib.guid]);

  // 根层拉一次维度选项（tag/genres + tag/list + tag/iso3166）
  useEffect(() => {
    if (!atRoot || dim) return;
    const ag = lib.guid.startsWith("_cat:") || lib.guid === "_fav" ? null : lib.guid;
    invoke<Record<string, Opt[]>>("mdb_tags_get", { ancestorGuid: ag })
      .then((r) => setDim(r ?? {}))
      .catch(() => setDim({}));
  }, [atRoot, dim, lib.guid]);

  /** 把当前筛选（除 cat 外）编码成 Rust filters 参数（服务端过滤）。 */
  const serverFilters = () => {
    const o: Record<string, unknown> = {};
    if (f.genres !== null) o.genres = f.genres;
    if (f.resolution) o.resolution = f.resolution;
    if (f.colorRange) o.color_range = f.colorRange;
    if (f.audioType) o.audio_type = f.audioType;
    if (f.locate) o.locate = f.locate;
    if (f.decade) o.decade = f.decade;
    if (f.recognition) o.recognition_status = f.recognition;
    if (f.watched) o.watched = f.watched;
    return Object.keys(o).length ? o : null;
  };

  /** 任一条件变更 → 服务端重拉第 1 页（分页继续服务端）。 */
  const refetch = (next: Filters) => {
    setF(next);
    setLevels((ls) => {
      const i = ls.length - 1;
      if (i < 0) return ls;
      const nv = [...ls];
      nv[i] = { ...ls[i], items: [], page: 0, total: 0 };
      return nv;
    });
    catTypesRef.current = next.cat ? [next.cat] : null;
    void load(null);
  };

  const chip = (label: string, on: boolean, onClick: () => void) => (
    <button key={label + (on ? "*" : "")} type="button" className={`libf-chip${on ? " on" : ""}`} onClick={onClick}>
      {label}
    </button>
  );
  const row = (key: string, label: string, opts: Opt[] | undefined, cur: string | number | null, set: (v: string | number | null) => void) => {
    if (!opts || opts.length <= 1) return null;
    return (
      <div className="libf-row" key={key}>
        <span className="libf-k">{label}</span>
        {opts.map((o) => chip(String(o.label), cur === o.value, () => set(cur === o.value ? null : o.value)))}
      </div>
    );
  };

  return (
    <main className="content" ref={contentRef} onScroll={onScroll}>
      <div className="page-head">
        <button className="back" onClick={levels.length > 1 ? pop : onBackHome}>
          ‹
        </button>
        <h3>{current ? current.title : lib.title}</h3>
        <span className="muted">{total || items.length} 项</span>
      </div>
      {/* [lc-1805] 筛选面板：飞鲸同款维度（动态选项，服务端过滤） */}
      {atRoot && !collapsed && (
        <div className="libf">
          <div className="libf-scroll">
            {isAllCat &&
              row("cat", "影视分类", [
                { label: "全部", value: null },
                { label: "电影", value: "Movie" },
                { label: "电视节目", value: "TV" },
                { label: "电视直播", value: "LiveChannel" },
              ], f.cat, (v) => {
                if (typeof v === "string" || v === null) refetch({ ...f, cat: v as string | null });
              })}
            {row("genres", "类型", dim?.genres, f.genres, (v) => refetch({ ...f, genres: typeof v === "number" ? v : null }))}
            {row("resolution", "分辨率", dim?.resolutions, f.resolution, (v) => refetch({ ...f, resolution: v as string | null }))}
            {row("colorRange", "视频动态范围", dim?.colorRange, f.colorRange, (v) => refetch({ ...f, colorRange: v as string | null }))}
            {row("audioType", "音频规格", dim?.audioType, f.audioType, (v) => refetch({ ...f, audioType: v as string | null }))}
            {row("locate", "国家和地区", dim?.locate, f.locate, (v) => refetch({ ...f, locate: v as string | null }))}
            {row("decade", "发行年份", dim?.decades, f.decade, (v) => refetch({ ...f, decade: v as string | null }))}
            {row("recognition", "匹配状态", dim?.recognition, f.recognition, (v) => refetch({ ...f, recognition: v as string | null }))}
            {row("watched", "是否已观看", [
              { label: "全部", value: null },
              { label: "已观看", value: "1" },
              { label: "未观看", value: "0" },
            ], f.watched, (v) => refetch({ ...f, watched: v as string | null }))}
          </div>
          {filterActive && (
            <button type="button" className="libf-clear" onClick={() => refetch(EMPTY_F)}>
              清除筛选
            </button>
          )}
          <button type="button" className="libf-toggle" onClick={() => setCollapsed(true)}>
            收起
          </button>
        </div>
      )}
      {atRoot && collapsed && (
        <button type="button" className="libf-open" onClick={() => setCollapsed(false)}>
          筛选{filterActive ? "（生效中）" : " ▾"}
        </button>
      )}
      {err && (
        <div className="err">
          <span>{err}</span>
          {/* [lc-1727] 首屏失败给「重试」（以前只能靠顶栏 ⟳，用户不一定找得到） */}
          {!loading && (
            <button className="lib-retry" onClick={() => void load(currentGuidRef.current)}>
              重试
            </button>
          )}
        </div>
      )}
      {loading && <div className="empty">加载中…</div>}
      {!loading && (
        <div className="grid">
          {items.map((it) => (
            <PosterCard key={it.guid} item={it} onOpen={openItem} onEditMeta={onEditMeta} onRefreshMeta={onRefreshMeta} onFavoriteChange={onFavoriteChange} />
          ))}
        </div>
      )}
      {!loading && items.length === 0 && !err && (
        <div className="empty">"暂无内容"</div>
      )}
      {!loading && moreBusy && <div className="lib-more-busy">加载更多…</div>}
      {!loading && moreErr && (
        <div className="lib-more-err">
          <span>加载更多失败：{moreErr}</span>
          <button className="back" onClick={() => void loadMore()}>
            重试
          </button>
        </div>
      )}
      {!loading && hasMore && <div ref={sentinelRef} className="lib-more-sentinel" />}
    </main>
  );
}

/* ────────────────────────── 详情页（Kazumi 风：模糊海报头 + 信息卡 + 分段） ────────────────────────── */

/** [lc-118] genres 三形态归一：数组（[string] 或 [{name}]）/ 逗号串 —— 主仓库 images.ts lc-572 同款。
 *  item 详情的字段名各版本不一，按优先级多键兜底。 */
