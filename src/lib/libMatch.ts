/* [lc-1862] 发现页「已在库」判定 / 「点开进库内条目」解析的公共标题匹配。
 *
 * 为什么单独成模块：`useLibIndex` 里带 React/Tauri 依赖，纯匹配逻辑抽出来才能被
 * `tests/verify_discover_libmatch.mjs` 直接跑（Node 24 原生类型剥离），也方便与后端
 * `src-tauri/src/libmatch.rs` 的 `norm_title` 保持同一口径。
 *
 * 根因（用户报障「库里确实有的剧集，发现页显示不在库」）：
 *   旧实现只做「归一化后全等 → 前缀兜底」，而库标题与 BGM 日历标题经常只差中间一两个字：
 *     · 身份 / 身分（繁简异体）        —— 后端 libmatch 已映射，前端漏了
 *     · 世界最强魔女 / 世界最强的魔女   —— 多一个「的」，既非全等也非前缀
 *     · 赛博朋克：边缘行者 / 赛博朋克：边缘行者 2 —— 前缀能过，但标点/装饰变体不能
 *   结果在库条目被归进「不在库」分组，点开也走 discover-* 占位详情（无源）。
 *
 * 口径（与后端 libmatch 对齐 + 前端额外的相似度兜底）：
 *   ① 归一化：去【…】装饰、去年份括号与 `from xxx` 后缀、NFKC、繁简/异体确定映射、
 *      小写、只留字母数字与汉字；
 *   ② 判定：全等 → 互相包含（季/篇后缀差异）→ LCS 相似度（中段个别字差异）。
 * 取向：**库里存在就判在库**；宁可极少数误标，也不要把在库条目丢进「不在库」。
 */
import type { MediaItem } from "../types";

/** 繁简/异体字确定等价映射（与后端 `libmatch.rs::variant_char` 同口径）。
 *  只做无歧义的等价替换，不做繁简整表转换（整表转换会引入新的误判）。 */
const TITLE_VARIANTS: Record<string, string> = {
  份: "分",
  裏: "里",
  裡: "里",
  臺: "台",
  檯: "台",
  為: "为",
  與: "与",
  這: "这",
  說: "说",
  們: "们",
  個: "个",
  隻: "只",
  著: "着",
  麼: "么",
  處: "处",
  衛: "卫",
  髮: "发",
  鬥: "斗",
  啟: "启",
  歷: "历",
};

/** 归一化标题：NFKC + 去装饰/年份/来源后缀 + 异体映射 + 小写 + 只留字母数字与汉字。 */
export function normTitle(raw?: string | null): string {
  let s = String(raw ?? "");
  // 库标题偶尔带【完结】【1080P】等标签 —— 先整体去掉（与后端 strip_decor 一致）
  s = s.replace(/【[^】]*】/g, "");
  // 年份括号：「某某 (2023)」/「某某 [2023]」
  s = s.replace(/[[(（]\s*(?:19|20)\d{2}\s*[\])）]/g, "");
  // 采集/字幕组来源后缀：「某某 from bilibili」
  s = s.replace(/\s*from\s+[a-z0-9]+\s*$/i, "");
  s = s.normalize("NFKC").toLowerCase();
  let out = "";
  for (const ch of s) {
    const c = TITLE_VARIANTS[ch] ?? ch;
    // 只保留字母/数字（汉字属于 \p{L}）；标点、符号、空白全部丢弃
    if (/[\p{L}\p{N}]/u.test(c)) out += c;
  }
  return out;
}

/** 最长公共子序列长度（标题都很短，O(n·m) 足够）。 */
function lcsLen(a: string, b: string): number {
  if (!a.length || !b.length) return 0;
  // 让 b 更短，滚动数组只留两行
  if (a.length < b.length) {
    const t = a;
    a = b;
    b = t;
  }
  let prev = new Array<number>(b.length + 1).fill(0);
  let cur = new Array<number>(b.length + 1).fill(0);
  for (let i = 0; i < a.length; i++) {
    cur[0] = 0;
    const ca = a[i];
    for (let j = 0; j < b.length; j++) {
      cur[j + 1] = ca === b[j] ? prev[j] + 1 : Math.max(prev[j + 1], cur[j]);
    }
    const tmp = prev;
    prev = cur;
    cur = tmp;
  }
  return prev[b.length];
}

/** 相似度阈值：0.8 时「世界最强魔女，就此开始」↔「世界最强的魔女，就此开始」= 1.0，
 *  而实测无关标题（CCTV/录屏/黄石 等）最高仅 0.56，留足安全边界。 */
export const TITLE_MATCH_SCORE = 0.8;

/** 归一化标题相似度：1 = 全等/包含；0~1 = LCS 占比；0 = 明显不同。
 *  两个标题都太短（<3 字）时只认全等，避免「迷宫」误配「迷宫饭」之外的两字标题。 */
export function titleScore(a: string, b: string): number {
  if (!a || !b) return 0;
  if (a === b) return 1;
  const min = Math.min(a.length, b.length);
  if (min < 3) return 0;
  if (a.includes(b) || b.includes(a)) return 1;
  if (min < 4) return 0;
  return lcsLen(a, b) / min;
}

/** 标题是否命中库索引：全等 → 包含（季/篇后缀）→ 相似度兜底。 */
export function libHasTitle(titles: Set<string>, raw?: string | null): boolean {
  const n = normTitle(raw);
  if (n.length < 2) return false;
  if (titles.has(n)) return true;
  for (const t of titles) {
    if (titleScore(n, t) >= TITLE_MATCH_SCORE) return true;
  }
  return false;
}

/** 解析到库内条目（点开用）：与 libHasTitle 同口径。
 *  相似度命中多个时取「归一化标题长度与查询串最接近」的一个，避免季/篇后缀挑错。
 *  未命中返回 null —— 调用方维持原链路（Bangumi 占位详情）。 */
export function resolveLibItem(byTitle: Map<string, MediaItem>, raw?: string | null): MediaItem | null {
  // [lc-1564] 旧形状索引（热更残留）没有 byTitle —— 视为未命中、绝不抛错（点击路径的底线）
  if (!byTitle) return null;
  const n = normTitle(raw);
  if (n.length < 2) return null;
  const exact = byTitle.get(n);
  if (exact) return exact;
  let best: MediaItem | null = null;
  let bestScore = 0;
  let bestDist = Number.POSITIVE_INFINITY;
  for (const [t, it] of byTitle) {
    const score = titleScore(n, t);
    if (score < TITLE_MATCH_SCORE) continue;
    const dist = Math.abs(t.length - n.length);
    if (score > bestScore || (score === bestScore && dist < bestDist)) {
      best = it;
      bestScore = score;
      bestDist = dist;
    }
  }
  return best;
}
