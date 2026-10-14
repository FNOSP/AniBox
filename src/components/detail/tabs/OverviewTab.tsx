/* [lc-1536] 从 DetailView 拆出的标签页（纯搬移，逻辑与注释未改动）。 */
import { TmdbExtra } from "../TmdbExtra";

type Props = {
  genres: string[];
  overview: string;
  setZoom: (u: string) => void;
  tmdb: Record<string, unknown> | null;
  tmdbDone: boolean;
};
export function OverviewTab({ genres, overview, setZoom, tmdb, tmdbDone }: Props) {
  return (
    <>
      {/* [lc-1420] `.dpanel-single`：本页所有节包进**一个**容器、节间用浅横线分隔（用户要求）。
         见 App.css 里同名类的注释（为什么不用 :has）。 */}
      <div className="dpanel dpanel-single">
        {/* [lc-1405] 原本这里还有一节「类型标签」渲染 genres —— 与左列顶部那个标签框**同一份数据**，
            重复；且用户已要求标签框直接取 BGM/TMDB，这一节就失去存在意义。故删除。 */}
        {/* [lc-1404] 剧照 / 分级 / 在线观看 / 相似 / 预告 / 别名关键词：**全部由 TmdbExtra 一处渲染**。
            此前这里是"自己一条 .dstill-strip + TmdbExtra 里再来一条 .tmdb-stills"，
            结果是同一批 TMDB 剧照出现**两组**（用户反馈）。现在只留 TmdbExtra 那一份。 */}
        {/* [lc-1420] 分块骨架：TMDB 详情也是一条独立请求，没回来之前这块是空的。
            摆"剧照 + 文本行"的骨架（形状与 TmdbExtra 的首节一致），到位时无跳动。
            [lc-1507] 骨架只在请求**进行中**（!tmdbDone）显示 —— 失败后不再永久转圈。 */}
        {!tmdb && !tmdbDone && (
          <>
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
            <div className="tmdb-sec">
              <div className="dsk dsk-sec-t" />
              <div className="dsk dsk-sline" />
              <div className="dsk dsk-sline short" />
            </div>
          </>
        )}
        {tmdb && <TmdbExtra tmdb={tmdb} onZoom={setZoom} />}
        {/* [lc-1507] TMDB 失败后的兜底：有简介（Bangumi）就显示简介节，
            什么都没有才显示空态 —— 原先骨架与「暂无详情信息」会同时出现 */}
        {!tmdb && tmdbDone && (
          <>
            {overview && (
              <div className="tmdb-sec">
                <div className="tmdb-sec-t">简介</div>
                <p className="dfold">{overview}</p>
              </div>
            )}
            {!overview && genres.length === 0 && (
              <div className="empty">TMDB 与 Bangumi 都没有该条目的详情信息</div>
            )}
          </>
        )}
      </div>
    </>
  );
}
