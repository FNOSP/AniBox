/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { Check as IcoCheckL, Film as IcoFilmL, Star as IcoStarL } from "lucide-react";
import { imgUrl } from "../../lib/images";
import { MediaItem } from "../../types";


export function PosterCard({
  item,
  onOpen,
  subtitle,
}: {
  item: MediaItem;
  onOpen: (item: MediaItem) => void;
  subtitle?: string;
}) {
  const isPlayable = item.type === "Movie" || item.type === "Episode";
  const progress =
    item.ts && item.duration && item.duration > 0 ? Math.min(100, (item.ts / item.duration) * 100) : 0;
  return (
    <div className="card" onClick={() => onOpen(item)} title={item.title}>
      <div className="poster-box">
        {item.poster ? (
          <>
            {/* [lc-114] 骨架屏放在图片**下层**（纯 CSS 层叠）。此前用 opacity 切换且依赖
                onLoad，而图片命中 WebView 缓存时常在 React 挂监听前就 complete ⇒ 回调不触发、
                图片永久 opacity:0 ⇒ "图片出不来"。层叠写法没有这个竞态。 */}
            <div className="poster-skeleton" />
            <img src={imgUrl(item.poster)} loading="lazy" alt={item.title} />
          </>
        ) : (
          <div className="poster-empty"><IcoFilmL size={22} strokeWidth={1.8} /></div>
        )}
        <div className="card-hover">
          <span className="play-pill">{isPlayable ? "▶ 播放" : "打开"}</span>
        </div>
        {item.vote_average && item.vote_average !== "0" && (
          <span className="rating"><IcoStarL size={12} strokeWidth={2} className="rating-ico fill" /> {item.vote_average}</span>
        )}
        {progress > 0 && (
          <div className="progress">
            <div style={{ width: `${progress}%` }} />
          </div>
        )}
        {item.type === "Episode" && item.season_number != null && (
          <span className="ep-badge">
            S{item.season_number}E{item.episode_number ?? ""}
          </span>
        )}
        {item.watched === 1 && <span className="watched-badge"><IcoCheckL size={11} strokeWidth={2.2} /></span>}
      </div>
      <div className="card-title">{subtitle || item.title}</div>
      {subtitle && item.title !== subtitle && <div className="card-sub">{item.title}</div>}
    </div>
  );
}

/** [lc-1407] 可横向翻页的一行（首页「继续观看」与各媒体库横排共用）。
 *
 *  为什么需要它：这两条横排是 `overflow-x: auto`，但本项目**全局隐藏了滚动条**
 *  （`* { scrollbar-width: none }`，WebView2 的 Fluent 滚动条无视 ::-webkit-scrollbar 才这么干的），
 *  而鼠标滚轮默认只滚纵向 —— 于是横排实际上"没有任何可用的滚动入口"
 *  （用户反馈：无法横向滑动）。这里改成**轮播同款雪佛龙箭头**翻页（用户指定）。
 *
 *  实现要点：
 *   - 翻页距离 = **可视宽度 × 0.9**（留一点重叠，让用户看得出是接着翻的），按 smooth 滚；
 *   - 到两端后对应箭头**自动淡出并禁用**（与轮播的 near 透明逻辑同一套观感）；
 *   - 箭头只在鼠标进入这一行时才显形（否则首页一屏里到处是箭头，很吵）；
 *   - 用 ResizeObserver 跟踪可视宽度，窗口缩放后翻页距离自动跟着变。
 */
