/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { TmdbDiscoverItem } from "../../types";

export const WEEKDAYS: { id: number; cn: string }[] = [
  { id: 1, cn: "周一" },
  { id: 2, cn: "周二" },
  { id: 3, cn: "周三" },
  { id: 4, cn: "周四" },
  { id: 5, cn: "周五" },
  { id: 6, cn: "周六" },
  { id: 7, cn: "周日" },
];

/** 本机时区的今天是周几（1..7）；右栏默认选中它、并在胶囊上打"今天"点 */

export function localWeekdayId(): number {
  const d = new Date().getDay(); // 0=周日
  return d === 0 ? 7 : d;
}

/** [lc-123-rev5] 本机日期 YYYY-MM-DD：日历缓存的**跨天分桶键**
 *  （本地零点一过就换键 ⇒ 必然重取；不会把昨天的表当今天用）。 */

export function localDay(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** 发现页骨架块（两栏共用；shimmer 语言对齐已有的 .poster-skeleton / .bgmc-sk-*） */

export function Dsk({ cls }: { cls: string }) {
  return <div className={`dsk ${cls}`} aria-hidden="true" />;
}

/* ───────────────────────── 左栏：TMDB 当季热门 + 即将播出 ───────────────────────── */

/** [lc-123-rev] 左栏「即将播出」的追加页响应（Rust tmdb_discover 带 total/totalPages 回传）。 */

export type TmdbUpResp = { items?: TmdbDiscoverItem[]; total?: number; totalPages?: number };

/** 「加载更多」的页数上限：与 Rust `tmdb_discover` 的 `page.clamp(1,25)` 对齐（20 条/页 × 25 = 500） */

export const UP_MAX_PAGE = 25;

/** 失败重试 / 空态（两块共用；骨架由各块自己的形状渲染） */

export function DState({ err, emptyText, onRetry }: { err: string; emptyText: string; onRetry: () => void }) {
  if (err) {
    return (
      <div className="dstate">
        <div className="dstate-msg">{err}</div>
        <button className="dretry" onClick={onRetry}>
          重试
        </button>
      </div>
    );
  }
  return <div className="dstate">{emptyText}</div>;
}

/** [lc-123-rev9] 主推区（**用户权威设计稿**：紧凑左右分栏 —— 左竖版海报 140×210，右信息列：
 *  大标题 / 评分徽章+年份·状态·N 人评分一行 / 3 行简介 / 「查看详情」按钮）。**没有大 backdrop、
 *  没有文字压图** —— 文字永远在实底信息列里。主推数据 = 当季第 1 条（热度第一），**不轮播不翻页**。
 *  海报 = poster（2:3，w500）；缺图走占位块（Film 图标），不破格。
 *  「查看详情」：TMDB 条目没有飞牛 guid —— [lc-1444] 改为**直接进主详情页**
 *  （复用 DetailView，选集信息占位），不再走 bgm_match + 轻量弹层的旧链路。 */
