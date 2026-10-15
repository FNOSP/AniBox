/* [lc-1565] 「更多」标签页：原「详情」页底部的 更多 块（别名/关键词/热度/编号/链接）独立成页。
   [lc-1566] 「相似剧集」也从「详情」页移入本页（用户：「把这两个移动到新的更多标签里」）。
   内容与样式原样从 TmdbExtra 拆出（.tmdb-sec / .tmdb-mrow 沿用，视觉不变）。
   [lc-1413] 「预告片」「分级 / 在线观看」两节已按用户要求删除（前者 YouTube 外链打不开、
   后者对国内用户基本为空），此处不再有。
   注：内部 `more` 分段是「角色」页（历史命名）；本页 id 为 `extra`，才是界面上的「更多」。 */

type Props = {
  tmdb: Record<string, unknown> | null;
  tmdbDone: boolean;
};
export function ExtraTab({ tmdb, tmdbDone }: Props) {
  if (!tmdb) {
    // 加载中 → 两节骨架（与最终形状一致）；TMDB 失败（tmdbDone）→ 空态（不永久转圈，同 lc-1507 口径）
    return (
      <div className="dpanel dpanel-single">
        {!tmdbDone ? (
          <>
            <div className="tmdb-sec">
              <div className="dsk dsk-sec-t" />
              <div className="dsk dsk-sline" />
              <div className="dsk dsk-sline short" />
            </div>
            <div className="tmdb-sec">
              <div className="dsk dsk-sec-t" />
              <div className="dsk dsk-sline" />
              <div className="dsk dsk-sline short" />
            </div>
          </>
        ) : (
          <div className="empty">暂无更多信息</div>
        )}
      </div>
    );
  }
  const arr = (k: string): Record<string, unknown>[] =>
    Array.isArray(tmdb[k]) ? (tmdb[k] as Record<string, unknown>[]) : [];
  const strList = (k: string): string[] => (Array.isArray(tmdb[k]) ? (tmdb[k] as string[]) : []);
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
  if (!recs.length && !hasMore) {
    return (
      <div className="dpanel dpanel-single">
        <div className="empty">暂无更多信息</div>
      </div>
    );
  }
  return (
    <div className="dpanel dpanel-single">
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
    </div>
  );
}
