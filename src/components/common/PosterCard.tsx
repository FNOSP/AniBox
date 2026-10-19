/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
/* [lc-1654] 海报卡四项操作（对齐飞牛网页端卡片）：
 *   中间大圆「播放」 / 左下「眼睛带斜杠（已看切换）」 / 中下「心形（本地收藏）」 / 右下「⋯ 更多」。
 *   · 播放 → invoke("play_item")
 *   · 已看 → invoke("set_item_watched")（飞牛 item/watched，失败自动降级本地 watched.json）
 *   · 收藏 → 本地收藏 favorite_get / favorite_toggle（飞牛 favorite 接口未实现，沿用本地）
 *   · 更多 → 查看详情 / 编辑元数据 / 刷新元数据（编辑弹窗由父组件注入回调，不塞进卡片）
 *   只对真实媒体条目显示（Movie/TV/Episode/Video）；Directory、discover-* 占位条目保持原样。 */
import { useEffect, useRef, useState, type MouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Check as IcoCheckL,
  EyeOff as IcoEyeOffL,
  Film as IcoFilmL,
  Heart as IcoHeartL,
  MoreHorizontal as IcoMoreL,
  Play as IcoPlayL,
  Star as IcoStarL,
} from "lucide-react";
import { imgUrl } from "../../lib/images";
import { loadWatchedOverrides, setWatchedOverride } from "../../lib/watched";
import { MediaItem } from "../../types";

/** 真实媒体条目才显示四项操作（Directory / discover-* 占位不要） */
const REAL_MEDIA_TYPES = new Set(["Movie", "TV", "Episode", "Video"]);

const numOf = (v: unknown): number => {
  const n = Number(v);
  return Number.isFinite(n) ? n : 0;
};

/** 入库时间紧凑展示：支持秒/毫秒时间戳与 "YYYY-MM-DD" 字符串 → "M/D"。 */
const fmtShortDate = (v: unknown): string => {
  const asDate = (n: number): string => {
    if (!Number.isFinite(n) || n <= 0) return "";
    const d = new Date(n > 1e12 ? n : n * 1000);
    return Number.isNaN(d.getTime()) ? "" : `${d.getMonth() + 1}/${d.getDate()}`;
  };
  if (typeof v === "number") return asDate(v);
  const s = String(v ?? "").trim();
  if (/^\d{9,}$/.test(s)) return asDate(Number(s));
  const m = s.match(/(\d{4})-(\d{2})-(\d{2})/);
  return m ? `${Number(m[2])}/${Number(m[3])}` : "";
};

/** [lc-1714] 卡片元信息行：字段全部来自 item/list，缺失项直接隐藏（不占位）。
 *  参考飞牛原生卡片：年份 · 季/集 · 更新日期，继续观看剩余时长。 */
function buildCardMeta(item: MediaItem): { line: string; remain: string } {
  const dateStr = String(item.release_date || item.air_date || "");
  const year = /^\d{4}/.test(dateStr) ? dateStr.slice(0, 4) : "";
  const seasons = numOf(item.number_of_seasons) || numOf(item.local_number_of_seasons);
  const episodes = numOf(item.number_of_episodes) || numOf(item.local_number_of_episodes);
  const updated = fmtShortDate(item.create_time);
  const parts = [
    year,
    seasons > 0 ? `${seasons} 季` : "",
    episodes > 0 ? `共 ${episodes} 集` : "",
    updated ? `更新 ${updated}` : "",
  ].filter(Boolean);

  const ts = numOf(item.ts);
  const duration = numOf(item.duration);
  const remain =
    duration > 0 && ts > 0 && ts < duration
      ? `剩余 ${Math.max(1, Math.round((duration - ts) / 60))} 分钟`
      : "";

  return { line: parts.join(" · "), remain };
}


export function PosterCard({
  item,
  onOpen,
  subtitle,
  showMeta,
  onEditMeta,
  onRefreshMeta,
  onShowDetail,
  onWatchedChange,
  onFavoriteChange,
}: {
  item: MediaItem;
  onOpen: (item: MediaItem) => void;
  subtitle?: string;
  /** [lc-1714] 首页媒体库行：标题下方补充年份/季集/更新时间/续看剩余（缺字段自动隐藏） */
  showMeta?: boolean;
  /** [lc-1654] 父组件注入：打开元数据编辑弹窗 */
  onEditMeta?: (item: MediaItem) => void;
  /** [lc-1654] 父组件注入：刷新元数据（缺省时卡片自己调 refresh_item） */
  onRefreshMeta?: (item: MediaItem) => void;
  /** [lc-1654] 父组件注入：查看详情（缺省 = onOpen） */
  onShowDetail?: (item: MediaItem) => void;
  onWatchedChange?: (item: MediaItem, watched: boolean) => void;
  onFavoriteChange?: (item: MediaItem, favorited: boolean) => void;
}) {
  const isRealMedia = REAL_MEDIA_TYPES.has(item.type ?? "");
  const isPlayable = item.type === "Movie" || item.type === "Episode";
  const progress =
    item.ts && item.duration && item.duration > 0 ? Math.min(100, (item.ts / item.duration) * 100) : 0;
  const meta = showMeta ? buildCardMeta(item) : null;

  const [watched, setWatched] = useState(item.watched === 1);
  const [favorited, setFavorited] = useState(item.is_favorite === 1);
  const [busyWatched, setBusyWatched] = useState(false);
  const [busyFavorite, setBusyFavorite] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement | null>(null);

  // 换条目（列表复用节点）时重置本地状态
  useEffect(() => {
    setWatched(item.watched === 1);
    setFavorited(item.is_favorite === 1);
    setMenuOpen(false);
  }, [item.guid, item.watched, item.is_favorite]);

  // 首屏：合并本地降级已看标记 + 本地收藏状态
  useEffect(() => {
    if (!isRealMedia) return;
    let alive = true;
    void loadWatchedOverrides().then((set) => {
      if (alive && set.has(item.guid)) setWatched(true);
    });
    void invoke<boolean>("favorite_get", { guid: item.guid })
      .then((v) => {
        if (alive) setFavorited(!!v);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [item.guid, isRealMedia]);

  // 菜单点外部关闭
  useEffect(() => {
    if (!menuOpen) return;
    const onDoc = (e: globalThis.MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [menuOpen]);

  const stop = (e: MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
  };

  const onPlay = (e: MouseEvent) => {
    stop(e);
    void invoke("play_item", { guid: item.guid }).catch(() => {});
  };

  const onToggleWatched = async (e: MouseEvent) => {
    stop(e);
    if (busyWatched) return;
    setBusyWatched(true);
    try {
      const r = await invoke<{ watched: boolean; local: boolean }>("set_item_watched", {
        guid: item.guid,
        watched: !watched,
      });
      setWatched(!!r.watched);
      // 降级到本地时才需要覆盖集合；远程成功则清掉旧的本地痕迹
      setWatchedOverride(item.guid, !!r.local && !!r.watched);
      onWatchedChange?.(item, !!r.watched);
    } catch {
      /* 失败保持原状态 */
    } finally {
      setBusyWatched(false);
    }
  };

  const onToggleFavorite = async (e: MouseEvent) => {
    stop(e);
    if (busyFavorite) return;
    setBusyFavorite(true);
    try {
      const r = await invoke<{ favorited: boolean }>("favorite_toggle", {
        item: { ...item, tvTitle: item.tv_title ?? "" },
      });
      setFavorited(!!r.favorited);
      onFavoriteChange?.(item, !!r.favorited);
      // [lc-1660] 通知 App 刷新侧栏收藏计数（收藏/取消都派发）
      window.dispatchEvent(new Event("anibox-favorites-changed"));
    } catch {
      /* 失败保持原状态 */
    } finally {
      setBusyFavorite(false);
    }
  };

  const doShowDetail = (e: MouseEvent) => {
    stop(e);
    setMenuOpen(false);
    (onShowDetail ?? onOpen)(item);
  };
  const doEditMeta = (e: MouseEvent) => {
    stop(e);
    setMenuOpen(false);
    onEditMeta?.(item);
  };
  const doRefreshMeta = (e: MouseEvent) => {
    stop(e);
    setMenuOpen(false);
    // [lc-1720] 「刷新元数据」已改为手动获取（弹窗由父组件注入）；未注入时不做任何事，
    // 不再直接调用飞牛的 refresh_item 重刮。
    onRefreshMeta?.(item);
  };

  return (
    <div
      className={`card${showMeta ? " card-meta-center" : ""}`}
      onClick={() => onOpen(item)}
      title={item.title}
    >
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
        {isRealMedia ? (
          /* [lc-1654] 飞牛同款四按钮覆盖层：点击一律 stopPropagation，不触发卡片 onOpen */
          <div className="poster-actions">
            <button className="pa-play" onClick={onPlay} title="播放" aria-label="播放">
              <IcoPlayL size={26} strokeWidth={1.6} className="fill" />
            </button>
            <div className="pa-row">
              <button
                className={`pa-btn${watched ? " is-on" : ""}`}
                onClick={onToggleWatched}
                title={watched ? "标记为未观看" : "标记为已观看"}
                aria-label={watched ? "标记为未观看" : "标记为已观看"}
                disabled={busyWatched}
              >
                <IcoEyeOffL size={20} strokeWidth={1.8} />
              </button>
              <button
                className={`pa-btn pa-heart${favorited ? " is-on" : ""}`}
                onClick={onToggleFavorite}
                title={favorited ? "取消收藏" : "收藏"}
                aria-label={favorited ? "取消收藏" : "收藏"}
                disabled={busyFavorite}
              >
                <IcoHeartL size={20} strokeWidth={1.8} className={favorited ? "fill" : undefined} />
              </button>
              <div className="pa-more-wrap" ref={menuRef}>
                <button
                  className={`pa-btn${menuOpen ? " is-on" : ""}`}
                  onClick={(e) => {
                    stop(e);
                    setMenuOpen((v) => !v);
                  }}
                  title="更多"
                  aria-label="更多"
                  aria-haspopup="true"
                  aria-expanded={menuOpen}
                >
                  <IcoMoreL size={20} strokeWidth={2} />
                </button>
                {menuOpen && (
                  <div className="pa-menu" role="menu" onClick={stop}>
                    <button role="menuitem" onClick={doShowDetail}>查看详情</button>
                    <button role="menuitem" onClick={doEditMeta}>编辑元数据</button>
                    <button role="menuitem" onClick={doRefreshMeta}>刷新元数据</button>
                  </div>
                )}
              </div>
            </div>
          </div>
        ) : (
          <div className="card-hover">
            <span className="play-pill">{isPlayable ? "▶ 播放" : "打开"}</span>
          </div>
        )}
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
        {watched && <span className="watched-badge"><IcoCheckL size={11} strokeWidth={2.2} /></span>}
      </div>
      <div className="card-title">{subtitle || item.title}</div>
      {subtitle && item.title !== subtitle && <div className="card-sub">{item.title}</div>}
      {meta && (meta.line || meta.remain) && (
        <div className="card-meta">
          {meta.line && <div className="card-meta-line">{meta.line}</div>}
          {meta.remain && <div className="card-meta-cont">{meta.remain}</div>}
        </div>
      )}
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
