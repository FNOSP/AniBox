/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { invoke } from "@tauri-apps/api/core";
import { MediaItem, SearchResponse } from "../types";

/** [lc-1727] item/list 单页条数：与 Rust 侧 `ITEM_LIST_PAGE_SIZE` 默认值一致。
 *  库页「滚动加载更多」按这个粒度翻页；后端支持传 `pageSize`，这里固定 60 不传，保持老版本 NAS 行为一致。 */
export const LIB_PAGE_SIZE = 60;

/** [lc-1727] 一页库条目：`total` 是**服务端总数**（不是本页条数），用于判断还有没有下一页。 */
export type LibPage = { list: MediaItem[]; total: number };

/** [lc-1727] 取库/文件夹的第 `page` 页（1 起）。`total` 缺失时回落 0（前端按「本页是否满页」兜底判停）。 */
export async function fetchLibPage(
  libGuid: string,
  parentGuid: string | null,
  page = 1,
  filters?: Record<string, unknown> | null,
): Promise<LibPage> {
  const d = await invoke<{ list?: MediaItem[] | null; total?: number }>("get_items", {
    ancestorGuid: parentGuid === null ? libGuid : null,
    parentGuid,
    page,
    filters: filters ?? null,
  });
  return { list: d.list ?? [], total: Number(d.total ?? 0) || 0 };
}

/** 首页「按库横排」只取第一页（刻意保持 60 条上限，滚动加载只在库页生效）。 */
export async function fetchLibItems(libGuid: string, parentGuid: string | null): Promise<MediaItem[]> {
  return (await fetchLibPage(libGuid, parentGuid, 1)).list;
}

/* ────────────────────────── 全局搜索 ────────────────────────── */

/** [lc-1725] 飞牛全局聚合搜索（GET /v/api/v1/search/list?q=）。
 *  `category` 仅作契约透传：上游无分类参数，分类过滤在前端结果上完成。 */
export async function searchItems(query: string, category?: string, page?: number): Promise<SearchResponse> {
  return invoke<SearchResponse>("search_items", {
    query,
    category: category ?? null,
    page: page ?? null,
  });
}

/* ────────────────────────── 标题栏 ────────────────────────── */

/** [lc-123-rev5] 用户是否要求"少动效"：三处动效的 **JS 侧守卫**
 *  （CSS 侧另有 @media (prefers-reduced-motion: reduce) 守卫，双保险）。 */
