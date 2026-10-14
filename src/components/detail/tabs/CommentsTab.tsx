/* [lc-1536] 从 DetailView 拆出的标签页（纯搬移，逻辑与注释未改动）。 */
import { BgmListSkeleton } from "../BgmListSkeleton";
import { AvatarSafe } from "../../common/AvatarSafe";
import { bgmImg } from "../../../lib/images";
import { fmtBgmTime } from "../../../lib/format";
import { BgmComment } from "../../../types";
import { Star as IcoStarL } from "lucide-react";

type Props = {
  comments: BgmComment[];
  commentsBusy: boolean;
  commentsDone: boolean;
  commentsErr: string;
  commentsMoreBusy: boolean;
  retryComments: () => void;
};
export function CommentsTab({ comments, commentsBusy, commentsDone, commentsErr, commentsMoreBusy, retryComments }: Props) {
  return (
    <>
      <div className="dpanel bgmc-panel">
        {commentsBusy && comments.length === 0 && !commentsErr && <BgmListSkeleton />}
        {commentsErr && (
          <div className="bgmc-state">
            <div className="bgmc-state-t">吐槽加载失败</div>
            <div className="bgmc-state-d">{commentsErr}</div>
            <button className="bgmc-retry" onClick={retryComments}>
              重试
            </button>
          </div>
        )}
        {!commentsBusy && !commentsErr && comments.length === 0 && (
          <div className="empty">暂无吐槽</div>
        )}
        {comments.length > 0 && (
          <div className="dvtc-list">
            {comments.map((c) => {
              // 星级：Bangumi rate 是 0~10，展示成 5 星（半星用 CSS 宽度裁）
              const rate = Number(c.rate ?? 0) || 0;
              const stars = rate > 0 ? Math.round(rate / 2) : 0;
              // reactions：上游只有 4/50 条带，且形状是 [{users:[...],value:0}]
              const rLike = (c.reactions ?? []).reduce<number>(
                (n, x) => n + (Array.isArray(x?.users) ? x.users.length : 0),
                0,
              );
              return (
                <div className="dvtc-item" key={String(c.id)}>
                  <AvatarSafe
                    className="dvtc-ava"
                    size={36}
                    name={c.user?.nickname || c.user?.username || "匿名"}
                    src={bgmImg(c.user?.avatar?.large || c.user?.avatar?.grid || c.user?.avatar?.small)}
                  />
                  <div className="dvtc-body">
                    <div className="dvtc-head">
                      <span className="dvtc-nick">{c.user?.nickname || c.user?.username || "匿名"}</span>
                      {stars > 0 && (
                        <span className="dvtc-stars" title={`${rate} 分`}>
                          {[0, 1, 2, 3, 4].map((i) => (
                            <IcoStarL
                              key={i}
                              size={12}
                              strokeWidth={1.8}
                              className={`rating-ico${i < stars ? " fill" : ""}`}
                            />
                          ))}
                        </span>
                      )}
                      <span className="dvtc-time">{fmtBgmTime(c.updatedAt)}</span>
                    </div>
                    <div className="dvtc-bubble">
                      <p className="dvtc-text">{c.comment}</p>
                      <span className={`dvtc-like${rLike > 0 ? " on" : ""}`} aria-hidden="true">
                        {rLike > 0 ? `♥ ${rLike}` : "♡"}
                      </span>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        )}
        {comments.length > 0 && commentsMoreBusy && <div className="bgmc-more">加载中…</div>}
        {comments.length > 0 && commentsDone && <div className="bgmc-more">没有更多吐槽了</div>}
      </div>
    </>
  );
}
