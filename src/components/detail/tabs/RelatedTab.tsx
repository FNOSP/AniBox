/* [lc-1536] 从 DetailView 拆出的标签页（纯搬移，逻辑与注释未改动）。 */
import { Film as IcoFilmL } from "lucide-react";
import { BgmListSkeleton } from "../BgmListSkeleton";
import { ScrollRow } from "../../common/ScrollRow";
import { bgmImg } from "../../../lib/images";
import { BgmRelated } from "../../../types";

type Props = {
  related: BgmRelated[];
  relatedGroups: { relation: string; items: BgmRelated[] }[];
  relatedBusy: boolean;
  relatedErr: string;
  retryRelated: () => void;
};
export function RelatedTab({ related, relatedBusy, relatedErr, retryRelated, relatedGroups }: Props) {
  return (
    <>
      <div className="dpanel">
        {relatedBusy && !relatedErr && <BgmListSkeleton />}
        {relatedErr && (
          <div className="bgmc-state">
            <div className="bgmc-state-t">关联条目加载失败</div>
            <div className="bgmc-state-d">{relatedErr}</div>
            <button className="bgmc-retry" onClick={retryRelated}>
              重试
            </button>
          </div>
        )}
        {!relatedBusy && !relatedErr && related.length === 0 && (
          <div className="empty">暂无关联条目</div>
        )}
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
      </div>
    </>
  );
}
