/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { invoke } from "@tauri-apps/api/core";
import { MediaItem } from "../types";

export async function fetchLibItems(libGuid: string, parentGuid: string | null): Promise<MediaItem[]> {
  const d = await invoke<{ list: MediaItem[]; total: number }>("get_items", {
    ancestorGuid: parentGuid === null ? libGuid : null,
    parentGuid,
    page: 1,
  });
  return d.list ?? [];
}

/* ────────────────────────── 标题栏 ────────────────────────── */

/** [lc-123-rev5] 用户是否要求"少动效"：三处动效的 **JS 侧守卫**
 *  （CSS 侧另有 @media (prefers-reduced-motion: reduce) 守卫，双保险）。 */
