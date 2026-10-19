/* [lc-1652] 电影详情页「沉浸式」版式（用户提供的样板）：
   全幅头图 + 大标题 + 元数据行 + 播放/收藏按钮 + 玻璃拟态简介/演职员卡。
   [lc-1655] 底部剧照横排已删除，改为「剧照」按钮打开全屏轮播（StillLightbox）。
   [lc-1662] 「剧照」按钮删除：剧照全部并入头图背景自动轮播（backdrop 第一张 + 4K 剧照），
     5.5s 交叉淡入淡出；StillLightbox 组件保留但不再由本页打开。
   只有**电影**（!isSeries）走这套；剧集详情页保持原版式。下方仍接原有标签页。 */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Star as IcoStarL } from "lucide-react";
import { imgUrl, tmdbImg } from "../../lib/images";
import { prefersReducedMotion } from "../../lib/motion";
import { useAdaptiveTone } from "../../lib/adaptiveTone";
import { TmdbCreditPerson } from "../../types";

/* [lc-1709] 详情页头图的采样区域 = 正文区（标题/元信息/简介）矩形。
   在该矩形内布点，配合 elementsFromPoint 穿透文字/玻璃层，拿到背后真正显示的头图，
   与顶栏按钮共用 src/lib/adaptiveTone.ts 的同一套亮度判定。 */
function pointsInRect(el: HTMLElement | null): { x: number; y: number }[] {
  if (!el) return [];
  const r = el.getBoundingClientRect();
  if (r.width < 1 || r.height < 1) return [];
  const xs = [0.08, 0.25, 0.5, 0.75, 0.92];
  const ys = [0.15, 0.45, 0.8];
  const out: { x: number; y: number }[] = [];
  for (const ry of ys) for (const rx of xs) out.push({ x: r.left + r.width * rx, y: r.top + r.height * ry });
  return out;
}

type Props = {
  title: string;
  originalTitle: string;
  /** 头图（已是完整 URL；空则退回海报） */
  backdrop: string;
  poster: string;
  rating: number | null;
  votes: number;
  year: string;
  runtimeMin: number;
  genres: string[];
  overview: string;
  cast: TmdbCreditPerson[];
  /** [lc-1655] 4K 剧照 file_path 列表（[lc-1662] 全部并入头图背景轮播） */
  stills: string[];
  resumeAt: boolean;
  playing: boolean;
  onPlay: () => void;
  onBack: () => void;
  onMeta?: () => void;
  /** [lc-1700] 点演职员 → 打开演员页（TMDB id > 0 才可点） */
  onOpenPerson?: (c: TmdbCreditPerson) => void;
  /** [版本] 同一电影的多版本（多文件）盒子；长度 > 1 才显示（照抄 FlyNarwhal） */
  versions?: { mediaGuid: string; label: string }[];
  selectedVersion?: string;
  onVersionChange?: (mediaGuid: string) => void;
  favItem: Record<string, unknown> | null;
};

export function MovieHero({
  title,
  originalTitle,
  backdrop,
  poster,
  rating,
  votes,
  year,
  runtimeMin,
  genres,
  overview,
  cast,
  stills,
  resumeAt,
  playing,
  onPlay,
  onBack,
  onMeta,
  onOpenPerson,
  versions,
  selectedVersion,
  onVersionChange,
  favItem,
}: Props) {
  const favGuid = String((favItem?.guid as string) ?? "").trim();
  const [fav, setFav] = useState(false);
  const [favBusy, setFavBusy] = useState(false);
  const [note, setNote] = useState("");

  useEffect(() => {
    if (!favGuid) return;
    let alive = true;
    invoke<boolean>("favorite_get", { guid: favGuid })
      .then((v) => alive && setFav(!!v))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [favGuid]);

  const toggleFav = () => {
    if (!favGuid || favBusy) return;
    setFavBusy(true);
    invoke<{ favorited?: boolean }>("favorite_toggle", { item: favItem })
      .then((r) => {
        const on = !!r?.favorited;
        setFav(on);
        setNote(on ? "已收藏，可在侧边栏「收藏」里找到" : "已取消收藏");
        // [lc-1660] 通知 App 刷新侧栏收藏计数（收藏/取消都派发）
        window.dispatchEvent(new Event("anibox-favorites-changed"));
      })
      .catch((e) => setNote(String(e)))
      .finally(() => setFavBusy(false));
  };

  const bg = backdrop || (poster ? imgUrl(poster) : "");
  const topCast = cast.filter((c) => c?.name).slice(0, 4);

  // [lc-1662] 头图轮播：backdrop 第一张，4K 剧照依次加入（stills 是 TMDB file_path，
  //   这里统一拼成 original 代理 URL）。去重 + 去空，保证不会出现空白帧。
  const stillUrls = stills
    .map((p) => (p.startsWith("http") ? p : tmdbImg(p, "original")))
    .filter(Boolean);
  const slides = Array.from(new Set([backdrop, ...stillUrls].filter(Boolean)));
  const slideKey = slides.join("|");
  const [slideIdx, setSlideIdx] = useState(0);

  // [lc-1709] 与顶栏按钮同款的自适应反色：按头图（当前可见那一层）的亮度，
  //   在 data-tone="light"（亮底→深色文字）/ "dark"（暗底→浅色文字）之间切换。
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const tone = useAdaptiveTone(
    {
      points: () => pointsInRect(bodyRef.current),
      overlaySelector: ".mv-hero-body, .mv-topbar, .mv-hero .mist-nav",
      band: () => {
        const r = bodyRef.current?.getBoundingClientRect();
        return r ? { top: r.top, bottom: r.bottom } : null;
      },
      extraKey: `${slideKey}|${slideIdx}`,
    },
    [slideKey, slideIdx],
  );

  // 换片/剧照列表变化 → 回到第一张
  useEffect(() => {
    setSlideIdx(0);
  }, [slideKey]);

  // 自动轮播：约 5.5s 一张；尊重系统「减弱动态效果」（不自动轮换）。
  useEffect(() => {
    if (slides.length <= 1 || prefersReducedMotion()) return;
    const t = window.setInterval(() => setSlideIdx((i) => (i + 1) % slides.length), 5500);
    return () => window.clearInterval(t);
  }, [slides.length, slideKey]);

  return (
    <>
      <section className="mv-hero" data-tone={tone}>
        {/* 基础层：backdrop / poster 常驻。轮播的剧照层叠在它上面 —— 某张剧照加载失败时
            会自然露出这一层，不会出现空白。 */}
        {bg && <div className="mv-hero-bg" style={{ backgroundImage: `url("${bg}")` }} />}
        {slides.map((url, i) =>
          url === bg ? null : (
            <div
              key={url}
              className={`mv-hero-bg mv-hero-bg-layer${slideIdx === i ? " on" : ""}`}
              style={{ backgroundImage: `url("${url}")` }}
            />
          ),
        )}
        {/* 手动切换头图：复用首页轮播的 .mist-nav 样式（无背景色，悬停出现） */}
        {slides.length > 1 && (
          <>
            <button
              type="button"
              className="mist-nav mist-nav-prev"
              title="上一张"
              aria-label="上一张"
              onClick={() => setSlideIdx((i) => (i - 1 + slides.length) % slides.length)}
            >
              ‹
            </button>
            <button
              type="button"
              className="mist-nav mist-nav-next"
              title="下一张"
              aria-label="下一张"
              onClick={() => setSlideIdx((i) => (i + 1) % slides.length)}
            >
              ›
            </button>
          </>
        )}
        {/* [lc-1697] 顶部薄雾 .mv-hero-topscrim 与底部/整体暗化遮罩 .mv-hero-shade
            已按用户要求删除；头图只保留背景图与内容本身，不再叠加任何装饰层。 */}

        <div className="mv-topbar">
          <button className="mv-back" onClick={onBack} title="返回上一页">
            ‹ 返回
          </button>
        </div>

        <div className="mv-hero-body" ref={bodyRef}>
          <h1 className="mv-title">{title || "（无标题）"}</h1>
          {originalTitle && originalTitle !== title && <div className="mv-subtitle">{originalTitle}</div>}

          <div className="mv-meta-row">
            {rating != null && rating > 0 && (
              <span className="mv-rating">
                <IcoStarL size={13} strokeWidth={0} className="mv-rating-star" aria-hidden="true" />{" "}
                {rating.toFixed(1)}
              </span>
            )}
            {year && <span>{year}</span>}
            {runtimeMin > 0 && <span>{runtimeMin} 分钟</span>}
            {genres.length > 0 && <span>{genres.slice(0, 3).join(" / ")}</span>}
            {votes > 0 && <span className="mv-votes">{votes} 人评分</span>}
          </div>

          <div className="mv-actions">
            <button className="mv-btn-primary" onClick={onPlay} disabled={playing}>
              ▶ {playing ? "打开中…" : resumeAt ? "继续播放" : "立即播放"}
            </button>
            <button className={`mv-btn-ghost${fav ? " on" : ""}`} onClick={toggleFav} disabled={!favGuid || favBusy}>
              <svg viewBox="0 0 24 24" fill={fav ? "currentColor" : "none"} stroke="currentColor" strokeWidth={fav ? 0 : 1.8}>
                <path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" />
              </svg>
              {favBusy ? "处理中…" : fav ? "已收藏" : "收藏"}
            </button>
            {/* 元数据入口：小正方形按钮，紧挨「收藏」右侧 */}
            {onMeta && (
              <button
                className="mv-btn-square"
                onClick={(e) => {
                  e.stopPropagation();
                  onMeta();
                }}
                title="元数据操作（编辑 / 手动匹配 / 刷新）"
                aria-label="元数据操作"
              >
                ⋯
              </button>
            )}
          </div>
          {/* [版本] 同一电影多版本：与剧集详情页同一套版本盒子（仅 > 1 显示）。 */}
          {versions && versions.length > 1 && (
            <div className="mv-versions" role="group" aria-label="选择版本">
              {versions.map((v) => (
                <button
                  key={v.mediaGuid}
                  type="button"
                  className={`mv-version${v.mediaGuid === selectedVersion ? " active" : ""}`}
                  aria-pressed={v.mediaGuid === selectedVersion}
                  onClick={() => onVersionChange?.(v.mediaGuid)}
                >
                  {v.label}
                </button>
              ))}
            </div>
          )}
          {note && <div className="mv-note">{note}</div>}

          {(overview || topCast.length > 0) && (
            <div className="mv-glass">
              {overview && <p className="mv-desc">{overview}</p>}
              {topCast.length > 0 && (
                <div className="mv-cast">
                  {topCast.map((c, i) => {
                    const av = c.profile ? (c.profile.startsWith("/") ? tmdbImg(c.profile, "w185") : imgUrl(c.profile)) : "";
                    const clickable = !!onOpenPerson && Number(c.id ?? 0) > 0;
                    return (
                      <div
                        className={`mv-cast-item${clickable ? " clickable" : ""}`}
                        key={`${c.id ?? i}`}
                        role={clickable ? "button" : undefined}
                        tabIndex={clickable ? 0 : undefined}
                        onClick={clickable ? () => onOpenPerson?.(c) : undefined}
                        onKeyDown={
                          clickable
                            ? (e) => {
                                if (e.key === "Enter" || e.key === " ") {
                                  e.preventDefault();
                                  onOpenPerson?.(c);
                                }
                              }
                            : undefined
                        }
                      >
                        <div
                          className="mv-cast-avatar"
                          style={av ? { backgroundImage: `url("${av}")` } : undefined}
                        />
                        <div className="mv-cast-info">
                          <h4>{c.name}</h4>
                          <p>{c.character ? `饰 ${c.character}` : c.jobLabel || c.job || ""}</p>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          )}
        </div>
      </section>
    </>
  );
}
