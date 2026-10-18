/* [lc-1700] 演员页作品卡（轻量、纯展示）。
   刻意**不复用 PosterCard**：PosterCard 绑定了飞牛条目（播放/收藏/元数据菜单），
   演员页的作品是 TMDB / Bangumi 的只读条目，只需要「图 + 年份 + 角色 + 标题 + 类型」。 */
import { bgmImg, tmdbImg } from "../../lib/images";
import type { PersonWork } from "../../types";

const TYPE_LABEL: Record<PersonWork["mediaType"], string> = {
  movie: "电影",
  tv: "剧集",
  anime: "动画",
  real: "三次元",
  other: "其他",
};

/** 年份归一：year 可能是 "2021" / 2021 / "2021-05-01"，统一取前 4 位数字 */
function yearOf(w: PersonWork): string {
  const raw = String(w.year ?? "").trim();
  const m = /^(\d{4})/.exec(raw);
  return m ? m[1] : "";
}

/** 封面 URL：TMDB 走 /__tmdb 代理（w342），BGM 走 /__bgm 代理 */
function posterOf(w: PersonWork): string {
  const p = String(w.poster ?? "").trim();
  if (!p) return "";
  if (w.tmdbId) return p.startsWith("http") ? p : tmdbImg(p, "w342");
  return bgmImg(p);
}

export function PersonWorkCard({
  work,
  index = 0,
  onOpen,
}: {
  work: PersonWork;
  /** 入场错峰序号 */
  index?: number;
  /** 有可打开的 id 时才可点 */
  onOpen?: (w: PersonWork) => void;
}) {
  const canOpen = !!onOpen && !!(work.tmdbId || work.bgmId);
  const poster = posterOf(work);
  const year = yearOf(work);
  const sub = work.character || work.job || work.staff || "";
  const typeLabel = TYPE_LABEL[work.mediaType] || "";

  return (
    <div
      className={`pw-card${canOpen ? " clickable" : ""}`}
      style={{ ["--pw-i" as string]: index }}
      role={canOpen ? "button" : undefined}
      tabIndex={canOpen ? 0 : undefined}
      title={work.title}
      onClick={
        canOpen
          ? () => {
              onOpen?.(work);
            }
          : undefined
      }
      onKeyDown={
        canOpen
          ? (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onOpen?.(work);
              }
            }
          : undefined
      }
    >
      <div className="pw-cv">
        {poster ? (
          <img src={poster} alt="" loading="lazy" draggable={false} />
        ) : (
          <div className="pw-cv-empty" aria-hidden="true">
            {String(work.title || "").slice(0, 1)}
          </div>
        )}
        {year && <span className="pw-year">{year}</span>}
        {sub && (
          <span className="pw-role" title={sub}>
            {sub}
          </span>
        )}
      </div>
      <div className="pw-nm">{work.title || "（无标题）"}</div>
      <div className="pw-meta">
        {typeLabel && <span className="pw-type">{typeLabel}</span>}
        {work.rating != null && Number(work.rating) > 0 && (
          <span className="pw-score">{Number(work.rating).toFixed(1)}</span>
        )}
        {work.episodeCount != null && Number(work.episodeCount) > 0 && (
          <span className="pw-eps">{work.episodeCount} 集</span>
        )}
      </div>
    </div>
  );
}
