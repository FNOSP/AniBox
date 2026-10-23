/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { PosterCard } from "../common/PosterCard";
import { fetchLibItems } from "../../lib/api";
import { useRefresh } from "../../hooks/useRefresh";
import { Lib, MediaItem } from "../../types";


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
  type Level = { id: number; folder: MediaItem | null; items: MediaItem[] };
  const nextLevelId = useRef(1);
  const [levels, setLevels] = useState<Level[]>([{ id: 0, folder: initialFolder ?? null, items: [] }]);
  const [total, setTotal] = useState(0);
  const [err, setErr] = useState("");
  const [loading, setLoading] = useState(true);

  const levelsRef = useRef(levels);
  useEffect(() => {
    levelsRef.current = levels;
  }, [levels]);

  const top = levels[levels.length - 1];
  const items = top.items;
  const current = top.folder;

  /** 把取回的列表写进指定层（层已不存在则丢弃） */
  const applyItems = useCallback((list: MediaItem[], levelId?: number) => {
    setLevels((ls) => {
      const id = levelId ?? ls[ls.length - 1]?.id;
      let hit = false;
      const next = ls.map((l) => {
        if (l.id !== id) return l;
        hit = true;
        return { ...l, items: list };
      });
      return hit ? next : ls;
    });
  }, []);

  const load = useCallback(
    async (parentGuid: string | null, levelId?: number) => {
      // 记下这次取数要写进哪一层（层 id 在弹栈后不会复用）。
      // 钻取时由 openItem 直接传新层 id —— setLevels 是异步的，此刻 levelsRef 里还没有新层。
      const targetId = levelId ?? levelsRef.current[levelsRef.current.length - 1]?.id;
      setLoading(true);
      setErr("");
      // [lc-1648] **钻取优先**：parentGuid 非空 = 已进入某个文件夹，必须按 parent_guid 拉子项。
      // 之前这段判断排在「收藏/全部分类」之后，_cat/_fav 入口下点文件夹会落回根查询 ——
      // 列表内容不变（点一下闪一下、看起来进不去），面包屑却变了，用户只能干瞪眼。
      if (parentGuid) {
        try {
          const list = await fetchLibItems(lib.guid, parentGuid);
          applyItems(list, targetId);
          setTotal(list.length);
          if (list.length === 0) setErr("该文件夹为空");
        } catch (e) {
          applyItems([], targetId);
          setTotal(0);
          setErr(String(e));
        }
        setLoading(false);
        return;
      }
      // 虚拟库根层：收藏 / 全局分类浏览
      if (lib.guid === "_fav") {
        try {
          const d = await invoke<{ total: number; list: MediaItem[] | null }>("get_favorites", {
            page: 1,
          });
          applyItems(d.list ?? [], targetId);
          setTotal(d.total ?? 0);
          if (!d.list?.length) setErr("还没有收藏");
        } catch (e) {
          applyItems([], targetId);
          setErr(String(e));
        }
        setLoading(false);
        return;
      }
      if (lib.guid.startsWith("_cat:")) {
        try {
          const types = lib.guid.slice(5).split(",").filter(Boolean);
          const d = await invoke<{ list: MediaItem[]; total: number }>("get_items_global", {
            typeFilter: types,
            page: 1,
          });
          applyItems(d.list ?? [], targetId);
          setTotal(d.total ?? 0);
          if (!d.list?.length) setErr("该分类暂无已识别条目");
        } catch (e) {
          applyItems([], targetId);
          setErr(String(e));
        }
        setLoading(false);
        return;
      }
      try {
        const list = await fetchLibItems(lib.guid, null);
        applyItems(list, targetId);
        setTotal(list.length);
        if (list.length === 0) setErr("该库暂无已识别条目");
      } catch (e) {
        // [lc-1647] 原来没有 try/catch：get_items 一旦 reject，setLoading(false) 永远走不到，
        // 页面就卡在「加载中…」—— 个人视频/网盘类库最常见的「打不开」表象。
        applyItems([], targetId);
        setTotal(0);
        setErr(String(e));
      }
      setLoading(false);
    },
    [lib.guid, applyItems],
  );

  useEffect(() => {
    // [lc-1647] 有起始文件夹就直接钻进去，否则从库根开始
    load(initialFolder ? initialFolder.guid : null);
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
    setLevels((ls) => [...ls, { id, folder: item, items: [] }]);
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

  return (
    <main className="content">
      <div className="page-head">
        <button className="back" onClick={levels.length > 1 ? pop : onBackHome}>
          ‹
        </button>
        <h3>{current ? current.title : lib.title}</h3>
        <span className="muted">{total || items.length} 项</span>
      </div>
      {err && <div className="err">{err}</div>}
      {loading && <div className="empty">加载中…</div>}
      {!loading && (
        <div className="grid">
          {items.map((it) => (
            <PosterCard key={it.guid} item={it} onOpen={openItem} onEditMeta={onEditMeta} onRefreshMeta={onRefreshMeta} onFavoriteChange={onFavoriteChange} />
          ))}
        </div>
      )}
      {!loading && items.length === 0 && !err && <div className="empty">暂无内容</div>}
    </main>
  );
}

/* ────────────────────────── 详情页（Kazumi 风：模糊海报头 + 信息卡 + 分段） ────────────────────────── */

/** [lc-118] genres 三形态归一：数组（[string] 或 [{name}]）/ 逗号串 —— 主仓库 images.ts lc-572 同款。
 *  item 详情的字段名各版本不一，按优先级多键兜底。 */
