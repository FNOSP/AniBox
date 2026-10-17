/* [lc-1652] 电影详情页「沉浸式」版式（用户提供的样板）：
   全幅头图 + 大标题 + 元数据行 + 播放/收藏按钮 + 玻璃拟态简介/演职员卡 + 底部剧照横排。
   只有**电影**（!isSeries）走这套；剧集详情页保持原版式。下方仍接原有标签页。 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Star as IcoStarL } from "lucide-react";
import { ScrollRow } from "../common/ScrollRow";
import { imgUrl, tmdbImg } from "../../lib/images";
import { TmdbCreditPerson } from "../../types";

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
  stills: { file_path?: string }[];
  resumeAt: boolean;
  playing: boolean;
  onPlay: () => void;
  onBack: () => void;
  onMeta?: () => void;
  onZoom?: (path: string) => void;
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
  onZoom,
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
      })
      .catch((e) => setNote(String(e)))
      .finally(() => setFavBusy(false));
  };

  const bg = backdrop || (poster ? imgUrl(poster) : "");
  const topCast = cast.filter((c) => c?.name).slice(0, 4);

  return (
    <>
      <section className="mv-hero">
        {bg && <div className="mv-hero-bg" style={{ backgroundImage: `url("${bg}")` }} />}
        <div className="mv-hero-shade" />

        <div className="mv-topbar">
          <button className="mv-back" onClick={onBack} title="返回上一页">
            ‹ 返回
          </button>
          {onMeta && (
            <button className="mv-meta" onClick={onMeta} title="元数据操作（编辑 / 手动匹配 / 刷新）">
              ⋯
            </button>
          )}
        </div>

        <div className="mv-hero-body">
          <h1 className="mv-title">{title || "（无标题）"}</h1>
          {originalTitle && originalTitle !== title && <div className="mv-subtitle">{originalTitle}</div>}

          <div className="mv-meta-row">
            {rating != null && rating > 0 && (
              <span className="mv-rating">
                <IcoStarL size={13} strokeWidth={0} className="fill" /> {rating.toFixed(1)}
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
          </div>
          {note && <div className="mv-note">{note}</div>}

          {(overview || topCast.length > 0) && (
            <div className="mv-glass">
              {overview && <p className="mv-desc">{overview}</p>}
              {topCast.length > 0 && (
                <div className="mv-cast">
                  {topCast.map((c, i) => {
                    const av = c.profile ? (c.profile.startsWith("/") ? tmdbImg(c.profile, "w185") : imgUrl(c.profile)) : "";
                    return (
                      <div className="mv-cast-item" key={`${c.id ?? i}`}>
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

      {stills.length > 0 && (
        <section className="mv-stills">
          <div className="mv-stills-head">
            <h3>剧照</h3>
            <span className="mv-muted">{stills.length} 张</span>
          </div>
          <ScrollRow boxClass="mv-still-row" imgSel=".mv-still img">
            {stills.map((st) => {
              const p = String(st.file_path ?? "");
              return (
                <div className="mv-still" key={p} onClick={() => onZoom?.(p)} title="点击查看原图">
                  <img src={tmdbImg(p, "w500")} loading="lazy" alt="" />
                  <span className="mv-zoom">⤢</span>
                </div>
              );
            })}
          </ScrollRow>
        </section>
      )}
    </>
  );
}
