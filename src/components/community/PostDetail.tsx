/* [lc-2445] 帖子详情：单帖全量 + 作品评分摘要 + 作品级短评（可回复）。
 * 短评来自 v1+v2 混一张 comments 表（accountId 为 null 的老行也能显示与回复，
 * 只是回复 v1 老行不产生通知——服务端语义，前端不假装）。 */
import { useCallback, useEffect, useState } from "react";
import { ArrowLeft as IcoBack, Send as IcoSend, Star as IcoStar } from "lucide-react";
import {
  fetchPostDetail,
  postComment,
  communityTagOf,
  type CommunityComment2,
  type CommunityPostDetail,
} from "../../lib/community";
import { PostCard } from "./PostCard";

function fmtTime(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

export function PostDetail({
  postId,
  onBack,
}: {
  postId: string;
  onBack: () => void;
}) {
  const [data, setData] = useState<CommunityPostDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState("");
  const [draft, setDraft] = useState("");
  const [replyTo, setReplyTo] = useState<CommunityComment2 | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setErr("");
    try {
      setData(await fetchPostDetail(postId));
    } catch (e) {
      setErr(String(e));
    } finally {
      setLoading(false);
    }
  }, [postId]);

  useEffect(() => {
    void load();
  }, [load]);

  const send = async () => {
    const text = draft.trim();
    if (!data || text.length < 3 || busy) return;
    setBusy(true);
    setErr("");
    try {
      const r = await postComment({
        mediaType: data.post.mediaType,
        tmdbId: data.post.tmdbId,
        workTitle: data.post.workTitle,
        body: text,
        parentId: replyTo?.id ?? null,
      });
      setDraft("");
      setReplyTo(null);
      // 本地追加（避免整页重拉；后 20s 无轮询，重进即校正）
      if (data && !r.replayed) {
        setData({
          ...data,
          comments: [{ ...r.comment, replies: [] } as unknown as CommunityComment2, ...data.comments],
        });
      }
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="cv2-detail">
      <div className="cv2-detail-hd">
        <button type="button" className="cv2-btn-ghost" onClick={onBack}>
          <IcoBack size={13} strokeWidth={2} /> 返回
        </button>
        {data?.workScore && (
          <span className="cv2-detail-score" title="AniBox 评分">
            <IcoStar size={12} strokeWidth={2} aria-hidden="true" />
            {data.workScore.avg.toFixed(1)} · {data.workScore.count} 人
          </span>
        )}
      </div>

      {loading && <div className="empty">加载中…</div>}
      {err && <div className="set-warn cv2-warn">{err}</div>}

      {data && (
        <>
          <PostCard post={data.post} />

          <div className="cv2-cmsec">
            <h4 className="cv2-cmsec-t">短评（{data.comments.length}）</h4>
            {replyTo && (
              <div className="cv2-replyto">
                回复 <b>{communityTagOf({ nick: replyTo.nick })}</b>
                <button type="button" onClick={() => setReplyTo(null)}>
                  取消
                </button>
              </div>
            )}
            <div className="cv2-cm-inputrow">
              <textarea
                className="cv2-input cv2-ta"
                rows={2}
                value={draft}
                maxLength={2000}
                onChange={(e) => setDraft(e.target.value)}
                placeholder="写短评…（3–2000 字）"
              />
              <button
                type="button"
                className="pl-solid pl-h-sm"
                onClick={() => void send()}
                disabled={busy || draft.trim().length < 3}
              >
                <IcoSend size={13} strokeWidth={2} />
                {busy ? "发送中…" : "发送"}
              </button>
            </div>
            <div className="cv2-cm-list">
              {data.comments.map((c) => (
                <div className="cv2-cm-item" key={c.id}>
                  <div className="cv2-cm-head">
                    <span className="cv2-post-nick">{communityTagOf({ nick: c.nick })}</span>
                    <span className="cv2-post-time">{fmtTime(c.createdAt)}</span>
                    {c.mine && <span className="cv2-post-flag me">我</span>}
                    {!c.deleted && (
                      <button
                        type="button"
                        className="cmlink"
                        onClick={() => setReplyTo(c)}
                      >
                        回复
                      </button>
                    )}
                  </div>
                  <p className="cv2-cm-body">
                    {c.deleted ? <span className="cv2-cm-deleted">该评论已删除</span> : c.body}
                  </p>
                </div>
              ))}
              {data.comments.length === 0 && <div className="empty">还没有短评</div>}
            </div>
          </div>
        </>
      )}
    </div>
  );
}
