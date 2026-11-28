/* [lc-2445] 帖子卡（信息流 / 我的发言共用）。
 * 图片走 fntvimg /__cm/ 代理（落 disk_cache）；fullKey 为 null = 云端 30 天已过期
 * → 显示「配图已过期」占位（正文还在，不装作没图也不给碎图）。 */
import { useState } from "react";
import { Flag as IcoFlag, Star as IcoStar, Trash2 as IcoTrash } from "lucide-react";
import {
  adminDeleteImage,
  adminDeletePost,
  adminRetractPost,
  adminUnretractPost,
  communityImageUrl,
  communityTagOf,
  type CommunityPost,
} from "../../lib/community";
import { ReportDialog } from "./ReportDialog";
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
    <>
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
    </>
  );
}

export function PostCard({
  post,
  onDelete,
  canReport = false,
  isAdmin = false,
  onChanged,
}: {
  post: CommunityPost;
  onDelete?: (p: CommunityPost) => void;
  /** 登录用户可举报（服务端同人去重） */
  canReport?: boolean;
  /** role=admin：显示撤回/恢复/删除/删图（服务端 requireAdmin 二次校验） */
  isAdmin?: boolean;
  /** admin 动作后同步最新 post 状态（撤回/删除） */
  onChanged?: (p: CommunityPost) => void;
}) {
  const [reporting, setReporting] = useState(false);
  const [busyAdmin, setBusyAdmin] = useState("");
  const [adminErr, setAdminErr] = useState("");

  const adminAct = async (fn: () => Promise<CommunityPost>) => {
    if (!isAdmin || busyAdmin) return;
    setBusyAdmin(post.id);
    setAdminErr("");
    try {
      const updated = await fn();
      onChanged?.(updated);
    } catch (e) {
      setAdminErr(String(e));
    } finally {
      setBusyAdmin("");
    }
  };

  const actRetract = () =>
    adminAct(async () => {
      if (post.retracted) {
        await adminUnretractPost(post.id);
        return { ...post, retracted: false, body: post.body ?? "（内容已恢复）" };
      }
      await adminRetractPost(post.id, "违规内容");
      return { ...post, retracted: true, body: null };
    });

  const actDelete = () =>
    adminAct(async () => {
      await adminDeletePost(post.id, "违规内容");
      return { ...post, deleted: true, body: null };
    });

  const actDeleteImage = async (imgId: string) => {
    if (!isAdmin || busyAdmin) return;
    setBusyAdmin(imgId);
    setAdminErr("");
    try {
      await adminDeleteImage(imgId, "违规图");
      onChanged?.({ ...post, images: (post.images ?? []).map((x) => (x.id === imgId ? { ...x, fullKey: null, thumbKey: null } : x)) });
    } catch (e) {
      setAdminErr(String(e));
    } finally {
      setBusyAdmin("");
    }
  };

  return (
    <article className={`cv2-post${post.deleted ? " deleted" : ""}`}>
      <div className="cv2-post-head">
        <span className="cv2-post-nick">{communityTagOf({ nick: post.nick, nickSuffix: post.nickSuffix, badge: post.badge })}</span>
        {post.badge === "admin" && <span className="cv2-badge-admin">认证</span>}
        <span className="cv2-post-time">{fmtTime(post.createdAt)}</span>
        {post.deleted && <span className="cv2-post-flag">已删除</span>}
        {post.retracted && !post.deleted && <span className="cv2-post-flag">已撤回</span>}
        <span className="cv2-post-headend">
          {canReport && !post.deleted && !post.retracted && !post.mine && (
            <button type="button" className="cv2-post-report" onClick={() => setReporting(true)} title="举报">
              <IcoFlag size={12} strokeWidth={2} />
            </button>
          )}
          {isAdmin && !post.deleted && (
            <button
              type="button"
              className="cmlink cv2-post-adminact"
              onClick={() => void actRetract()}
              disabled={busyAdmin === post.id}
            >
              {post.retracted ? "恢复" : "撤回"}
            </button>
          )}
          {isAdmin && !post.deleted && (
            <button
              type="button"
              className="cmlink cv2-post-adminact danger"
              onClick={() => void actDelete()}
              disabled={busyAdmin === post.id}
            >
              删除
            </button>
          )}
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
        </span>
      </div>
      {post.board === "feedback" && (
        <div className="cv2-post-work">
          <span className="cv2-work-chip cv2-work-chip-fb">反馈建议</span>
        </div>
      )}
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
            <div key={img.id} className="cv2-pimg-cell">
              <PostImage img={img} />
              {isAdmin && img.fullKey && (
                <button
                  type="button"
                  className="cv2-pimg-del"
                  title="删除该图"
                  onClick={() => void actDeleteImage(img.id)}
                  disabled={busyAdmin === img.id}
                >
                  删图
                </button>
              )}
            </div>
          ))}
        </div>
      )}
      {adminErr && <div className="set-warn cv2-warn">{adminErr}</div>}
      {reporting && (
        <ReportDialog kind="post" refId={post.id} onClose={() => setReporting(false)} />
      )}
    </article>
  );
}
