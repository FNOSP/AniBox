/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。
   [lc-1565] 「更多」块（别名/关键词/热度/编号/链接）移出为独立标签页（tabs/ExtraTab）。 */
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
  const stills = arr("backdrops").slice(0, 6);
  const recs = arr("recommendations").slice(0, 8);
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
    </>
  );
}

/* ────────────────────────── 侧栏（保留现有样式） ────────────────────────── */
