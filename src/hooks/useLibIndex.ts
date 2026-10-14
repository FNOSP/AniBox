/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { MediaItem } from "../types";

export const LIB_INDEX_PAGE = 60; // get_items_global 的 page_size 在 Rust 侧固定 60

export const LIB_INDEX_MAX_PAGES = 8; // 上限 480 部；不足一页才算"到底"

export const LIB_WHITELIST_TYPES = ["Movie", "TV"]; // 只要已识别的电影/剧集（排除文件夹/未识别视频）

/** 归一化标题：NFKC（全角→半角）+ 小写 + 去掉空白/标点/符号（只留字母数字与汉字） */

export function normTitle(raw?: string | null): string {
  return String(raw ?? "")
    .normalize("NFKC")
    .toLowerCase()
    .replace(/[\s\p{P}\p{S}]/gu, "");
}

/** 标题是否命中库索引：先全等，再前缀兜底（「我推的孩子」↔「我推的孩子 第三季」这类季/篇后缀差异） */

export function libHasTitle(titles: Set<string>, raw?: string | null): boolean {
  const n = normTitle(raw);
  if (n.length < 2) return false;
  if (titles.has(n)) return true;
  for (const t of titles) {
    if (t.length >= 3 && n.length >= 3 && (t.startsWith(n) || n.startsWith(t))) return true;
  }
  return false;
}


export type LibIndexState = "loading" | "ok" | "partial" | "none";

/** 会话级缓存：拉到的索引切页回来不再重拉（最多 8 次请求只花一次；失败不缓存，下次进来可重试） */

export let libIndexCache: { state: LibIndexState; titles: Set<string> } | null = null;

/** 库标题索引（active 即拉，与日历并行；不阻塞列表渲染） */

export function useLibIndex(active: boolean): { state: LibIndexState; titles: Set<string> } {
  const [idx, setIdx] = useState<{ state: LibIndexState; titles: Set<string> }>(
    () => libIndexCache ?? { state: "loading", titles: new Set() },
  );
  useEffect(() => {
    if (!active || libIndexCache) return;
    let alive = true;
    (async () => {
      const titles = new Set<string>();
      let complete = false;
      try {
        for (let p = 1; p <= LIB_INDEX_MAX_PAGES; p++) {
          const d = await invoke<{ list?: MediaItem[]; total?: number }>("get_items_global", {
            typeFilter: LIB_WHITELIST_TYPES,
            page: p,
          });
          const list = Array.isArray(d?.list) ? d.list : [];
          for (const it of list) {
            const n = normTitle(it?.title);
            if (n) titles.add(n);
          }
          if (list.length < LIB_INDEX_PAGE) {
            complete = true; // 不足一页 = 已到底（空页也是）
            break;
          }
        }
      } catch {
        if (alive) setIdx({ state: "none", titles: new Set() }); // 失败不写缓存（下次进来重试）
        return;
      }
      if (titles.size === 0) {
        if (alive) setIdx({ state: "none", titles: new Set() });
        return;
      }
      const next = { state: (complete ? "ok" : "partial") as LibIndexState, titles };
      libIndexCache = next;
      if (alive) setIdx(next);
    })();
    return () => {
      alive = false;
    };
  }, [active]);
  return idx;
}

/** [lc-123-rev] 单个更新条目行：左侧强调竖条 + 封面(46×62，左上角该天序号) + 标题 +
 *  评分色阶标签(≥8 绿 / 7–8 黄 / <7 灰；无评分不显示) + 年份 + 今日更新标。
 *  `/calendar` **没有集号与播出时刻**（只有 air_date 首播日 + air_weekday），所以这里
 *  刻意**不显示**「第 N 话 / 今晚 22:30」——那两列无数据可依，宁可不要也不编。 */
