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
}: {
  lib: Lib;
  onBackHome: () => void;
  onOpenItem: (item: MediaItem) => void;
}) {
  const [items, setItems] = useState<MediaItem[]>([]);
  const [total, setTotal] = useState(0);
  const [stack, setStack] = useState<{ folder: MediaItem; items: MediaItem[] }[]>([]);
  const [err, setErr] = useState("");
  const [loading, setLoading] = useState(true);

  const current = stack.length > 0 ? stack[stack.length - 1].folder : null;

  const load = useCallback(
    async (parentGuid: string | null) => {
      setLoading(true);
      setErr("");
      // 虚拟库：收藏 / 全局分类浏览
      if (lib.guid === "_fav") {
        try {
          const d = await invoke<{ total: number; list: MediaItem[] | null }>("get_favorites", {
            page: 1,
          });
          setItems(d.list ?? []);
          setTotal(d.total ?? 0);
          if (!d.list?.length) setErr("还没有收藏");
        } catch (e) {
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
          setItems(d.list ?? []);
          setTotal(d.total ?? 0);
          if (!d.list?.length) setErr("该分类暂无已识别条目");
        } catch (e) {
          setErr(String(e));
        }
        setLoading(false);
        return;
      }
      const list = await fetchLibItems(lib.guid, parentGuid);
      setItems(list);
      setTotal(list.length);
      if (list.length === 0) setErr("该库暂无已识别条目");
      setLoading(false);
    },
    [lib.guid],
  );

  useEffect(() => {
    load(null);
  }, [load]);

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
    if (current) setStack((s) => [...s.slice(0, -1), { folder: current, items }]);
    else setStack((s) => [...s, { folder: item, items }]);
    await load(item.guid);
  }

  async function pop() {
    const prev = stack[stack.length - 1];
    setStack((s) => s.slice(0, -1));
    setItems(prev.items);
  }

  return (
    <main className="content">
      <div className="page-head">
        <button className="back" onClick={stack.length > 0 ? pop : onBackHome}>
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
            <PosterCard key={it.guid} item={it} onOpen={openItem} />
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
