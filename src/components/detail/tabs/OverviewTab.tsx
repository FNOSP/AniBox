/* [lc-1536] 从 DetailView 拆出的标签页（纯搬移，逻辑与注释未改动）。
   [lc-1567] 标签/简介从首屏移入本页（用户要求）：版面 = 标签 → 简介 → 剧照，
   同一容器（.dpanel-single）、节间浅横线分隔。 */
import { TmdbExtra } from "../TmdbExtra";
import { BgmTag } from "../../../types";

type Props = {
  bgmTags: BgmTag[];
  overview: string;
  setZoom: (u: string) => void;
  tmdb: Record<string, unknown> | null;
  tmdbDone: boolean;
  /** [lc-1655] 是否渲染剧照块（TmdbExtra / 骨架 / 空态）。电影页传 false（剧照改全屏轮播），默认 true。 */
  showStills?: boolean;
};
export function OverviewTab({
  bgmTags,
  overview,
  setZoom,
  tmdb,
  tmdbDone,
  showStills = true,
}: Props) {
  return (
    <>
      {/* [lc-1420] `.dpanel-single`：本页所有节包进**一个**容器、节间用浅横线分隔（用户要求）。 */}
      <div className="dpanel dpanel-single">
        {/* [lc-1567] 标签（Bangumi tags 优先 / TMDB genres 兜底）：原「首屏标签框」原样移入 */}
        {bgmTags.length > 0 && (
          <div className="tmdb-sec">
            <div className="tmdb-sec-t">标签</div>
            <div className="dv2-itags">
              {bgmTags.slice(0, 12).map((t, i) => (
                <span key={`${t.name}-${i}`} className="dtag" title={t.count ? `${t.count} 人标记` : ""}>
                  {t.name}
                </span>
              ))}
            </div>
          </div>
        )}
        {/* [lc-2127] 简介**默认完全展示**，不再截断/不再有「展开」按钮（用户要求去掉收起）。
            长简介直接把这一节撑开即可 —— 标签页内容区（.dv2-body）本来就是滚动视口，
            内容变高只是多滚一点，不会顶坏版式。故 longIntro / introFull / onExpand 三个
            props 与「展开」弹窗一并移除（DetailView 侧同步清理）。 */}
        {overview && (
          <div className="tmdb-sec">
            <div className="tmdb-sec-t">简介</div>
            <div className="dv2-isum">
              <p className="dfold">{overview}</p>
            </div>
          </div>
        )}
        {/* [lc-1404] 剧照由 TmdbExtra 一处渲染（相似剧集/更多块见「更多」页，lc-1565/1566）。
            [lc-1655] 电影页（showStills=false）整块不渲染：剧照改由头图「剧照」按钮全屏轮播。 */}
        {/* [lc-1420] 分块骨架：TMDB 详情没回来之前摆「剧照行」骨架，到位无跳动。
            [lc-1507] 骨架只在请求**进行中**（!tmdbDone）显示 —— 失败后不再永久转圈。 */}
        {showStills && !tmdb && !tmdbDone && (
          <div className="tmdb-sec">
            <div className="dsk dsk-sec-t" />
            {/* [lc-1505] 骨架容器用专用类：.dvstill-row 本身没有 flex（真实剧照行的
                横排布局由 ScrollRow 提供），裸 div 会把骨架块竖着堆成一列 */}
            <div className="dsk-still-row">
              {[0, 1, 2, 3].map((i) => (
                <div key={i} className="dsk dsk-dstill" />
              ))}
            </div>
          </div>
        )}
        {showStills && tmdb && <TmdbExtra tmdb={tmdb} onZoom={setZoom} />}
        {/* [lc-1507] TMDB 失败：静默降级，剧照位给空态（不永久转圈） */}
        {showStills && !tmdb && tmdbDone && <div className="empty">暂无剧照</div>}
      </div>
    </>
  );
}
