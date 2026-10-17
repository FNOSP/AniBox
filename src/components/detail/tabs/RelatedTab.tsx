/* [lc-1536] 从 DetailView 拆出的标签页（纯搬移，逻辑与注释未改动）。 */
import { Film as IcoFilmL } from "lucide-react";
import { BgmListSkeleton } from "../BgmListSkeleton";
import { ScrollRow } from "../../common/ScrollRow";
import { bgmImg, tmdbImg } from "../../../lib/images";
import { BgmRelated } from "../../../types";

/** [lc-1658] TMDB 推荐条目（后端 tmdb_show_details 的 recommendations 映射）。
 *  BGM 无关联时用它兜底渲染可点击的横版卡片。 */
export type TmdbRecommendation = {
  tmdbId: number;
  title: string;
  year: string;
  poster: string;
  backdrop: string;
  rating: number;
};

type Props = {
  related: BgmRelated[];
  relatedGroups: { relation: string; items: BgmRelated[] }[];
  relatedBusy: boolean;
  relatedErr: string;
  retryRelated: () => void;
  /** [lc-1658] BGM 无关联时的 TMDB 兜底数据与加载状态 */
  tmdbRecs: TmdbRecommendation[];
  tmdbDone: boolean;
  /** 当前详情页的媒体类型，跳转 TMDB 推荐时透传 */
  mediaType: "tv" | "movie";
  onOpenTmdb?: (tmdbId: number, title: string, year: string, mediaType: "tv" | "movie") => void;
};
export function RelatedTab({
  related,
  relatedBusy,
  relatedErr,
  retryRelated,
  relatedGroups,
  tmdbRecs,
  tmdbDone,
  mediaType,
  onOpenTmdb,
}: Props) {
  // [lc-1658] 只有 BGM **确实有**关联时才走原版式；无数据（含请求失败）一律尝试 TMDB 兜底。
  const hasBgm = related.length > 0;
  const hasTmdb = tmdbRecs.length > 0;

  return (
    <>
      <div className="dpanel">
        {hasBgm ? (
          <>
            {/* [lc-1411] 按用户定稿版式重做：**横向分栏**（一列 = 一个关系），
                  列内竖向排卡片，列间竖分隔线；整条横向可翻页（复用 ScrollRow 的箭头）。
                  与旧版的区别：旧版是"每个关系一节、节内网格"，改成一列一节后，
                  同一屏能同时看到「续集 / 书籍 / 游戏 / 原声集 / 片头曲…」的对照关系。 */}
            <ScrollRow boxClass="drel-cols">
              {relatedGroups.map((g) => (
                <section className="drel-col" key={g.relation}>
                  <h3 className="drel-col-h">
                    {g.relation}
                    <span className="drel-col-n">{g.items.length}</span>
                  </h3>
                  {g.items.map((r, i) => {
                    const cover = r.images?.large || r.images?.common || r.images?.medium || r.images?.grid || "";
                    const nm = String(r.name_cn || r.name || "").trim() || "（无标题）";
                    return (
                      <div className="drel-card" key={`${r.id ?? i}`} title={String(r.name || "")}>
                        <div className="drel-cv">
                          {cover ? (
                            <img src={bgmImg(cover)} alt="" loading="lazy" draggable={false} />
                          ) : (
                            <div className="poster-empty"><IcoFilmL size={16} strokeWidth={1.8} /></div>
                          )}
                        </div>
                        <div className="drel-nm">{nm}</div>
                      </div>
                    );
                  })}
                </section>
              ))}
            </ScrollRow>
          </>
        ) : hasTmdb ? (
          // [lc-1658] BGM 无关联（或请求失败）→ TMDB 相似推荐横向卡片行，整卡可点跳详情。
          <ScrollRow boxClass="drel-tmdb-row" imgSel=".drel-tmdb-cv">
            {tmdbRecs.map((r) => {
              const img = r.backdrop || r.poster;
              return (
                <button
                  type="button"
                  className="drel-tmdb-card"
                  key={r.tmdbId}
                  title={r.title}
                  onClick={() => onOpenTmdb?.(r.tmdbId, r.title, r.year, mediaType)}
                >
                  <div className="drel-tmdb-cv">
                    {img ? (
                      <img src={tmdbImg(img, "w500")} alt="" loading="lazy" draggable={false} />
                    ) : (
                      <div className="poster-empty"><IcoFilmL size={18} strokeWidth={1.8} /></div>
                    )}
                    {r.rating > 0 && (
                      <span className="drel-tmdb-score">{r.rating.toFixed(1)}</span>
                    )}
                  </div>
                  <div className="drel-tmdb-nm">{r.title}</div>
                </button>
              );
            })}
          </ScrollRow>
        ) : relatedBusy || !tmdbDone ? (
          // 任一数据源仍在加载 → 骨架
          <BgmListSkeleton />
        ) : relatedErr ? (
          <div className="bgmc-state">
            <div className="bgmc-state-t">关联条目加载失败</div>
            <div className="bgmc-state-d">{relatedErr}</div>
            <button className="bgmc-retry" onClick={retryRelated}>
              重试
            </button>
          </div>
        ) : (
          <div className="empty">暂无关联条目</div>
        )}
      </div>
    </>
  );
}
