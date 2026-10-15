/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { BgmPane } from "./BgmPane";
import { TmdbPane } from "./TmdbPane";
import { BgmCalendarItem, MediaItem, TmdbDiscoverItem } from "../../types";


export function DiscoverView({
  active,
  onOpenDetail,
  onOpenBgm,
  onOpenLib,
}: {
  active: boolean;
  /** [lc-1444] 左栏 TMDB 条目 → 主详情页（父级合成 MediaItem 并切 view） */
  onOpenDetail: (it: TmdbDiscoverItem, mediaType: "tv" | "movie") => void;
  /** [lc-1444] 右栏新番表条目 → 主详情页（条目自带 Bangumi subject id） */
  onOpenBgm: (it: BgmCalendarItem) => void;
  /** [lc-1563] 右栏「已在库」条目 → 库内条目（真 guid；父级 openDiscoverLib） */
  onOpenLib: (item: MediaItem) => void;
}) {
  return (
    <div className="discover">
      <TmdbPane active={active} onOpenDetail={onOpenDetail} />
      <BgmPane active={active} onOpen={onOpenBgm} onOpenLib={onOpenLib} />
    </div>
  );
}

/* ════════════ [lc-1502] 在线资源面板（发现页无源条目 · Kazumi「我的规则」式）════════════
   源由**用户勾选启用**（⚙ 设置面板，localStorage 持久化），搜索只跑勾选的源：
   · ⚙ 源管理：复选框启用/停用、全选/清空、＋粘贴 Kazumi 规则 JSON 添加、× 删除
   · 结果按源分组：N 个结果（绿）/ 无结果 / 检索失败（红），点条目 → 线路/选集 → mpv
   · 解析直链：HTTP 正则直解 → 隐藏 WebView 嗅探兜底 */
