/* [lc-1536] 从 DetailView 拆出的标签页（纯搬移，逻辑与注释未改动）。 */
import { BgmListSkeleton } from "../BgmListSkeleton";
import { bgmImg } from "../../../lib/images";
import { fmtBgmTime } from "../../../lib/format";
import { BgmReview } from "../../../types";
import { MessageSquare as IcoMsgL } from "lucide-react";

type Props = {
  loadMoreReviews: () => void;
  retryReviews: () => void;
  reviews: BgmReview[];
  reviewsBusy: boolean;
  reviewsDone: boolean;
  reviewsErr: string;
  reviewsMoreBusy: boolean;
};
export function ReviewsTab({ loadMoreReviews, retryReviews, reviews, reviewsBusy, reviewsDone, reviewsErr, reviewsMoreBusy }: Props) {
  return (
    <>
      <div className="dpanel dvrev">
        {reviewsBusy && reviews.length === 0 && !reviewsErr && <BgmListSkeleton />}
        {reviewsErr && (
          <div className="bgmc-state">
            <div className="bgmc-state-t">评论加载失败</div>
            <div className="bgmc-state-d">{reviewsErr}</div>
            <button className="bgmc-retry" onClick={retryReviews}>
              重试
            </button>
          </div>
        )}
        {!reviewsBusy && !reviewsErr && reviews.length === 0 && (
          <div className="empty">暂无评论</div>
        )}
        {reviews.map((r) => {
          const icon = String(r.entry?.icon ?? "");
          const hasCover = !!icon && !/no_photo/i.test(icon);   // 占位图不当封面用
          const date = fmtBgmTime(r.entry?.createdAt ?? r.entry?.updatedAt);
          const replies = Number(r.entry?.replies ?? 0) || 0;
          return (
            <article className="dvrev-item" key={String(r.id)}>
              <div className="dvrev-cv">
                {hasCover ? (
                  <img src={bgmImg(icon)} alt="" loading="lazy" draggable={false} />
                ) : (
                  <span className="dvrev-cv-ph" aria-hidden="true">
                    <IcoMsgL size={20} strokeWidth={1.7} />
                  </span>
                )}
              </div>
              <div className="dvrev-main">
                <h3 className="dvrev-t">{String(r.entry?.title || "（无标题）")}</h3>
                {r.entry?.summary && <p className="dvrev-sum">{String(r.entry.summary)}</p>}
                <div className="dvrev-meta">
                  <span className="dvrev-user">{r.user?.nickname || r.user?.username || "匿名"}</span>
                  {date && <><span className="dvrev-dot">·</span><span>{date}</span></>}
                  <span className="dvrev-dot">·</span>
                  <span>{replies} 回复</span>
                </div>
              </div>
            </article>
          );
        })}
        {reviews.length > 0 && reviewsMoreBusy && <div className="bgmc-more">加载中…</div>}
        {reviews.length > 0 && reviewsDone && <div className="bgmc-more">没有更多评论了</div>}
        {!reviewsDone && reviews.length > 0 && !reviewsMoreBusy && (
          <div className="dvrev-more" onClick={() => void loadMoreReviews()}>
            更多影评 ›
          </div>
        )}
      </div>
    </>
  );
}
