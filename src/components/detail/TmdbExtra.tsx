/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { ScrollRow } from "../common/ScrollRow";
import { tmdbImg } from "../../lib/images";


export function TmdbExtra({
  tmdb,
  onZoom,
}: {
  tmdb: Record<string, unknown>;
  /** [lc-128] 点剧照 → 以 original 尺寸放大（原图走同一套落盘缓存，拉了就不再拉） */
  onZoom?: (path: string) => void;
}) {
  const arr = (k: string): Record<string, unknown>[] =>
    Array.isArray(tmdb[k]) ? (tmdb[k] as Record<string, unknown>[]) : [];
  const strList = (k: string): string[] => (Array.isArray(tmdb[k]) ? (tmdb[k] as string[]) : []);
  const stills = arr("backdrops").slice(0, 6);
  const recs = arr("recommendations").slice(0, 8);
  const aliases = strList("aliases");
  const keywords = strList("keywords");
  const popularity = Number(tmdb.popularity ?? 0);
  const tmdbId = String(tmdb.tmdbId ?? "");
  const isMovie = String(tmdb.mediaType ?? "") === "movie";
  const imdb = String((tmdb.externalIds as Record<string, unknown> | undefined)?.imdb ?? "");
  const wiki = String((tmdb.externalIds as Record<string, unknown> | undefined)?.wikidata ?? "");
  const hasMore = aliases.length || keywords.length || popularity > 0;
  const links = [
    { label: "TMDB", url: tmdbId ? `https://www.themoviedb.org/${isMovie ? "movie" : "tv"}/${tmdbId}` : "" },
    { label: "IMDb", url: imdb ? `https://www.imdb.com/title/${imdb}` : "" },
    { label: "Wikidata", url: wiki ? `https://www.wikidata.org/wiki/${wiki}` : "" },
  ].filter((l) => l.url);
  return (
    <>
      {stills.length > 0 && (
        <div className="tmdb-sec">
          <div className="tmdb-sec-t">剧照</div>
          {/* [lc-1413] 用户反馈"剧照太多显示不完全，加同款雪佛龙按钮（不要背景色）" ——
              原来是一条 overflow-x:auto 的裸横排（项目全局隐藏了滚动条，滚轮又只滚纵向，
              所以多出来的剧照根本看不到）。这里改走 ScrollRow：复用首页那套**无底色**雪佛龙，
              smooth 翻页 + 两端自动隐藏。箭头垂直居中的基准改成剧照高度（.tmdb-stills img）。 */}
          <ScrollRow boxClass="dvstill-row" imgSel=".tmdb-stills img" inSec>
            {stills.map((st) => (
              <img
                key={String(st.file_path ?? st)}
                src={tmdbImg(String(st.file_path ?? st), "w500")}
                loading="lazy"
                alt=""
                title="点击查看原图"
                onClick={() => onZoom?.(String(st.file_path ?? st))}
              />
            ))}
          </ScrollRow>
        </div>
      )}
      {recs.length > 0 && (
        <div className="tmdb-sec">
          <div className="tmdb-sec-t">相似剧集</div>
          <p className="tmdb-inline">
            {recs.map((r, i) => (
              <span key={String(r.tmdbId ?? i)}>
                {i > 0 && <i> · </i>}
                {String(r.title)}
                {r.year ? ` (${r.year})` : ""}
              </span>
            ))}
          </p>
        </div>
      )}
      {/* [lc-1413] 用户要求删掉「预告片」与「分级 / 在线观看」两个模块 ——
          前者是外链 YouTube（国内打不开，点了也是白点），后者对国内用户基本是空的。 */}
      {hasMore && (
        <div className="tmdb-sec">
          <div className="tmdb-sec-t">更多</div>
          {aliases.length > 0 && (
            <div className="tmdb-mrow"><span className="tmdb-mk">别名</span><span className="tmdb-inline">{aliases.join(" · ")}</span></div>
          )}
          {keywords.length > 0 && (
            <div className="tmdb-mrow"><span className="tmdb-mk">关键词</span><span className="tmdb-inline">{keywords.join(" · ")}</span></div>
          )}
          {popularity > 0 && (
            <div className="tmdb-mrow"><span className="tmdb-mk">热度</span><span className="tmdb-inline">{Math.round(popularity).toLocaleString("zh-CN")}</span></div>
          )}
          <div className="tmdb-mrow">
            <span className="tmdb-mk">编号</span>
            <span className="tmdb-inline">
              {tmdbId && `TMDB ${tmdbId}`}
              {imdb && ` · Q${wiki.replace(/^Q/, "")}` && wiki && ` · ${wiki}`}
              {imdb && ` · ${imdb}`}
            </span>
          </div>
          {links.length > 0 && (
            <div className="tmdb-links">
              {links.map((l) => (
                <a key={l.label} href={l.url} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>
                  {l.label}
                </a>
              ))}
            </div>
          )}
        </div>
      )}
    </>
  );
}

/* ────────────────────────── 侧栏（保留现有样式） ────────────────────────── */
