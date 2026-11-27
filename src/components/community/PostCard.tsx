/* [lc-2445] 帖子卡（信息流 / 我的发言共用）。
 * 图片走 fntvimg /__cm/ 代理（落 disk_cache）；fullKey 为 null = 云端 30 天已过期
 * → 显示「配图已过期」占位（正文还在，不装作没图也不给碎图）。 */
import { useState } from "react";
import { Star as IcoStar, Trash2 as IcoTrash } from "lucide-react";
import {
  communityImageUrl,
  communityTagOf,
  type CommunityPost,
} from "../../lib/community";
import "../../styles/56-community.css";

function fmtTime(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  if (diff < 30 * 86_400_000) return `${Math.floor(diff / 86_400_000)} 天前`;
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** 单张帖子图：优先缩略图；过期显示占位 */
function PostImage({ img }: { img: NonNullable<CommunityPost["images"]>[number] }) {
  const [failed, setFailed] = useState(false);
  const src = communityImageUrl(img.thumbKey || img.fullKey || "");
  if (!img.thumbKey && !img.fullKey) {
    return (
      <div className="cv2-pimg lost" role="img" aria-label="配图已过期">
        配图已过期
      </div>
    );
  }
  return (
    <div className="cv2-pimg-wrap">
      {failed ? (
        <div className="cv2-pimg lost">配图已过期</div>
      ) : (
        <img
          className="cv2-pimg"
          src={src}
          alt=""
          loading="lazy"
          draggable={false}
          onError={() => setFailed(true)}
        />
      )}
    </div>
  );
}

export function PostCard({
  post,
  onDelete,
}: {
  post: CommunityPost;
  onDelete?: (p: CommunityPost) => void;
}) {
  return (
    <article className={`cv2-post${post.deleted ? " deleted" : ""}`}>
      <div className="cv2-post-head">
        <span className="cv2-post-nick">{communityTagOf({ nick: post.nick })}</span>
        <span className="cv2-post-time">{fmtTime(post.createdAt)}</span>
        {post.deleted && <span className="cv2-post-flag">已删除</span>}
        {post.retracted && !post.deleted && <span className="cv2-post-flag">已撤回</span>}
        {post.mine && onDelete && !post.deleted && (
          <button
            type="button"
            className="cv2-post-del"
            onClick={() => onDelete(post)}
            title="删除这条发言"
          >
            <IcoTrash size={12} strokeWidth={2} />
          </button>
        )}
      </div>
      {post.workTitle && (
        <div className="cv2-post-work">
          <span className="cv2-work-chip">{post.mediaType === "movie" ? "电影" : "剧集"}</span>
          <span className="cv2-work-name">{post.workTitle}</span>
          {post.score ? (
            <span className="cv2-work-score" title="我的评分">
              <IcoStar size={12} strokeWidth={2} aria-hidden="true" />
              {post.score}
            </span>
          ) : null}
        </div>
      )}
      {post.title && <h4 className="cv2-post-title">{post.title}</h4>}
      {post.body && <p className="cv2-post-body">{post.body}</p>}
      {(post.images?.length ?? 0) > 0 && (
        <div className="cv2-post-imgs">
          {(post.images ?? []).map((img) => (
            <PostImage key={img.id} img={img} />
          ))}
        </div>
      )}
    </article>
  );
}
