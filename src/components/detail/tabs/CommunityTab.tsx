/* [lc-2332] 公共评论（作品级）。
 *
 * 与「评论」「吐槽」两页的区别：那两页是 **Bangumi** 的数据（BgmReview / BgmComment），
 * 只读；这一页是我们的自建评论服务，可发可删。
 *
 * 安全：正文**必须用 React 的文本插值渲染**（`{c.body}`）。服务端原样存 HTML 不做转义
 * （存了就得考虑清洗，不存最安全），所以这里绝不能改成 dangerouslySetInnerHTML。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { MessageSquare as IcoMsgL, Send as IcoSendL, Trash2 as IcoTrashL, UserRound as IcoUserL } from "lucide-react";
import { communityTagOf, fetchSession, postComment, type CommunitySession } from "../../../lib/community";
import { CommentsTab } from "./CommentsTab";
import type { BgmComment } from "../../../types";

type Reply = {
  id: string;
  parentId: string | null;
  rootId: string | null;
  nick: string;
  body: string | null;
  deleted: boolean;
  createdAt: number;
  mine: boolean;
  replies?: Reply[];
};

type Page = { items: Reply[]; nextCursor: number | null; total: number };

type Props = {
  /** 'movie' | 'tv'；没有 tmdbId 时整页显示不可用 */
  mediaType: "movie" | "tv" | null;
  tmdbId: number | null;
  title: string;
  /**
   * [lc-2333] Bangumi 吐槽（只读）。
   *
   * 为什么是只读：**BGM 官方 API 没有发表评论的接口**（完整 spec 47 个路径里，
   * 写操作只有收藏/目录/搜索，没有任何 POST .../comments）。网页端发吐槽走私有
   * 接口 + CSRF + 会话 Cookie，不在开放 API 授权范围内，复刻会违反服务条款且
   * 随时可能失效。所以 BGM 那一半只展示、不发言；发言统一走我们自建的服务。
   *
   * 为 null 表示这部作品没有 BGM 条目（或还没探测完），此时不显示该段。
   */
  bgmComments?: BgmComment[] | null;
  bgmCommentsBusy?: boolean;
  bgmCommentsDone?: boolean;
  bgmCommentsErr?: string;
  bgmCommentsMoreBusy?: boolean;
  onBgmMore?: () => void;
  onBgmRetry?: () => void;
};

/** 确定性头像色：同一个昵称永远同一个颜色，不用存图片。 */
function avatarHue(nick: string): number {
  let h = 0;
  for (let i = 0; i < nick.length; i++) h = (h * 31 + nick.charCodeAt(i)) % 360;
  return h;
}

function fmtTime(ms: number): string {
  const d = new Date(ms);
  const now = Date.now();
  const diff = now - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} 天前`;
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** UUID v4 生成（服务端用它做幂等键：重发同一条不会产生两条评论）。 */
function newRequestId(): string {
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

function Avatar({ nick }: { nick: string }) {
  const hue = avatarHue(nick);
  return (
    <div
      className="cmava"
      style={{ background: `hsl(${hue} 42% 42%)` }}
      aria-hidden
    >
      <IcoUserL size={16} strokeWidth={2} />
    </div>
  );
}

function CommentRow({
  c,
  onReply,
  onDelete,
  busy,
  depth = 0,
}: {
  c: Reply;
  onReply?: (c: Reply) => void;
  onDelete: (c: Reply) => void;
  busy: boolean;
  depth?: number;
}) {
  return (
    <div className={`cmitem${depth > 0 ? " cmreply" : ""}`}>
      <Avatar nick={c.nick} />
      <div className="cmbody">
        <div className="cmhead">
          <span className="cmnick">{c.nick}</span>
          <span className="cmtime">{fmtTime(c.createdAt)}</span>
          {c.mine && <span className="cmmine">我</span>}
        </div>
        <p className="cmtext">
          {c.deleted ? <span className="cmdeleted">该评论已删除</span> : c.body}
        </p>
        {!c.deleted && (
          <div className="cmops">
            {onReply && depth === 0 && (
              <button type="button" onClick={() => onReply(c)} disabled={busy}>
                回复
              </button>
            )}
            {c.mine && (
              <button type="button" onClick={() => onDelete(c)} disabled={busy} className="cmdel">
                <IcoTrashL size={12} strokeWidth={2} />
                删除
              </button>
            )}
          </div>
        )}
        {c.replies && c.replies.length > 0 && (
          <div className="cmreplies">
            {c.replies.map((r) => (
              <CommentRow key={r.id} c={r} onDelete={onDelete} busy={busy} depth={depth + 1} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

export function CommunityTab({
  mediaType,
  tmdbId,
  title,
  bgmComments,
  bgmCommentsBusy,
  bgmCommentsDone,
  bgmCommentsErr,
  bgmCommentsMoreBusy,
  onBgmMore,
  onBgmRetry,
}: Props) {
  const [page, setPage] = useState<Page>({ items: [], nextCursor: null, total: 0 });
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState("");
  const [draft, setDraft] = useState("");
  const [replyTo, setReplyTo] = useState<Reply | null>(null);
  const [nick, setNick] = useState("");
  const [nickEditing, setNickEditing] = useState(false);
  const [nickDraft, setNickDraft] = useState("");
  /** 幂等键：用户点发送时生成，失败重试沿用同一条 → 服务端不会存两条 */
  const requestIdRef = useRef<string | null>(null);
  const textRef = useRef<HTMLTextAreaElement | null>(null);

  const enabled = !!mediaType && !!tmdbId;

  const load = useCallback(
    async (cursor: number) => {
      if (!enabled) return;
      setLoading(true);
      setErr("");
      try {
        const p = await invoke<Page>("community_list", {
          mediaType,
          tmdbId,
          cursor,
          limit: 20,
        });
        setPage((prev) =>
          cursor === 0
            ? p
            : { items: [...prev.items, ...p.items], nextCursor: p.nextCursor, total: p.total }
        );
      } catch (e) {
        setErr(String(e));
      } finally {
        setLoading(false);
      }
    },
    [enabled, mediaType, tmdbId]
  );

  useEffect(() => {
    if (!enabled) return;
    setPage({ items: [], nextCursor: null, total: 0 });
    setDraft("");
    setReplyTo(null);
    setErr("");
    void load(0);
    invoke<{ nick: string }>("community_status")
      .then((s) => setNick(s?.nick ?? ""))
      .catch(() => {});
  }, [enabled, load]);

  const submit = async () => {
    const text = draft.trim();
    if (!text || busy || !enabled) return;
    if (text.length < 3) {
      setErr("评论至少 3 个字");
      return;
    }
    setBusy(true);
    setErr("");
    // 幂等键只在真正发出去时生成；成功后清空，下次是新评论
    if (!requestIdRef.current) requestIdRef.current = newRequestId();
    try {
      await invoke("community_post", {
        mediaType,
        tmdbId,
        title,
        body: text,
        clientRequestId: requestIdRef.current,
        parentId: replyTo?.id ?? null,
      });
      requestIdRef.current = null;
      setDraft("");
      setReplyTo(null);
      await load(0);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (c: Reply) => {
    setBusy(true);
    setErr("");
    try {
      await invoke("community_delete", { id: c.id });
      await load(0);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const startReply = (c: Reply) => {
    setReplyTo(c);
    textRef.current?.focus();
  };

  const saveNick = async () => {
    try {
      const r = await invoke<{ nick: string }>("community_set_nick", { nick: nickDraft.trim() });
      setNick(r?.nick ?? "");
      setNickEditing(false);
    } catch (e) {
      setErr(String(e));
    }
  };

  /** [lc-2445] 社区账号会话（登录后显示短评输入框；未登录保持 v1 只读浏览） */
  const [cSession, setCSession] = useState<CommunitySession | null>(null);
  const [cDraft, setCDraft] = useState("");
  const [cBusy, setCBusy] = useState(false);
  const [cErr, setCErr] = useState("");

  useEffect(() => {
    let alive = true;
    void fetchSession().then((s) => {
      if (alive) setCSession(s);
    });
    return () => {
      alive = false;
    };
  }, []);

  const sendComment = async () => {
    const text = cDraft.trim();
    if (!cSession || !mediaType || !tmdbId || text.length < 3 || cBusy) return;
    setCBusy(true);
    setCErr("");
    try {
      await postComment({ mediaType, tmdbId, workTitle: title, body: text });
      setCDraft("");
      await load(0); // 复用既有评论刷新（v1/v2 同表，重拉即可见）
    } catch (e) {
      setCErr(String(e));
    } finally {
      setCBusy(false);
    }
  };

  if (!enabled) {
    return <div className="dpanel cmpanel"><div className="empty">这部作品缺少 TMDB 编号，无法关联评论</div></div>;
  }

  const hasBgm = bgmComments !== null && bgmComments !== undefined;

  return (
    <div className="dpanel cmpanel">
      {/* ── 段一：Bangumi 吐槽（只读）────────────────────────────────
          BGM 官方 API 没有发评论接口（见 Props 注释），所以这一段只展示。
          有 BGM 条目时才出现；没有就整段不渲染，用户看到的只是「本站讨论」。 */}
      {hasBgm && (
        <section className="cmsec">
          <h3 className="cmsec-t">
            Bangumi 吐槽
            <span className="cmsec-hint">来自 Bangumi，只读</span>
          </h3>
          <CommentsTab
            comments={bgmComments ?? []}
            commentsBusy={!!bgmCommentsBusy}
            commentsDone={!!bgmCommentsDone}
            commentsErr={bgmCommentsErr ?? ""}
            commentsMoreBusy={!!bgmCommentsMoreBusy}
            retryComments={onBgmRetry ?? (() => {})}
          />
          {!bgmCommentsDone && !bgmCommentsBusy && (bgmComments?.length ?? 0) > 0 && onBgmMore && (
            <button type="button" className="cmmore" onClick={onBgmMore}>
              加载更多吐槽
            </button>
          )}
        </section>
      )}

      {/* ── 段二：本站讨论（可发可回复可删）────────────────────────── */}
      <section className="cmsec">
        <h3 className="cmsec-t">
          本站讨论
          <span className="cmsec-hint">所有用 AniBox 的人都能看到</span>
        </h3>
        {/* [lc-2445] 短评输入（登录社区账号后出现；v2 账号体系，回复可通知对方） */}
        {cSession && (
          <div className="cmcompose cmv2compose">
            <div className="cmreplyto">
              以 <b>{communityTagOf(cSession)}</b> 发短评
            </div>
            <textarea
              className="cminput"
              value={cDraft}
              maxLength={2000}
              rows={2}
              onChange={(e) => setCDraft(e.target.value)}
              placeholder="写短评…（3–2000 字，Ctrl+Enter 发送）"
              onKeyDown={(e) => {
                if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
                  e.preventDefault();
                  void sendComment();
                }
              }}
            />
            <div className="cmcomposerow">
              <span className="cmcounter">{cDraft.trim().length} / 2000</span>
              <button
                type="button"
                className="pl-solid pl-h-sm"
                disabled={cBusy || cDraft.trim().length < 3}
                onClick={() => void sendComment()}
              >
                <IcoSendL size={13} strokeWidth={2} />
                {cBusy ? "发送中…" : "发送"}
              </button>
            </div>
            {cErr && <div className="set-warn cmwarn">{cErr}</div>}
          </div>
        )}
      {/* 昵称行：改昵称只影响之后的评论（服务端每条存昵称快照），所以文案要说清 */}
      <div className="cmnickrow">
        <IcoUserL size={13} strokeWidth={2} />
        <span className="cmnicklabel">昵称</span>
        {nickEditing ? (
          <>
            <input
              className="cmnickinput"
              value={nickDraft}
              maxLength={24}
              placeholder="留空则自动生成"
              onChange={(e) => setNickDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void saveNick();
                if (e.key === "Escape") setNickEditing(false);
              }}
            />
            <button type="button" className="cmlink" onClick={() => void saveNick()}>
              保存
            </button>
            <button type="button" className="cmlink" onClick={() => setNickEditing(false)}>
              取消
            </button>
          </>
        ) : (
          <>
            <span className="cmnickvalue">{nick || "自动生成"}</span>
            <button
              type="button"
              className="cmlink"
              onClick={() => {
                setNickDraft(nick);
                setNickEditing(true);
              }}
            >
              修改
            </button>
            <span className="cmnickhint">改名只影响之后的评论</span>
          </>
        )}
      </div>

      {page.total > 0 && (
        <div className="cmcount">
          <IcoMsgL size={13} strokeWidth={2} />
          {page.total} 条评论
        </div>
      )}

      {page.items.length > 0 && (
        <div className="cmlist">
          {page.items.map((c) => (
            <CommentRow
              key={c.id}
              c={c}
              onReply={startReply}
              onDelete={remove}
              busy={busy}
            />
          ))}
        </div>
      )}
      {loading && page.items.length === 0 && <div className="empty">加载中…</div>}
      {!loading && !err && page.items.length === 0 && (
        <div className="empty">还没有评论，说两句？</div>
      )}

      {page.nextCursor !== null && (
        <button
          type="button"
          className="cmmore"
          disabled={loading}
          onClick={() => void load(page.nextCursor ?? 0)}
        >
          {loading ? "加载中…" : "加载更多"}
        </button>
      )}

      {/* 输入区 */}
      <div className="cmcompose">
        {replyTo && (
          <div className="cmreplyto">
            回复 <b>{replyTo.nick}</b>
            <button type="button" onClick={() => setReplyTo(null)}>
              取消
            </button>
          </div>
        )}
        <textarea
          ref={textRef}
          className="cminput"
          value={draft}
          placeholder="说点什么…（3–2000 字）"
          maxLength={2000}
          rows={3}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            // Ctrl/Cmd+Enter 发送：评论框是长文本，回车应该换行
            if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
              e.preventDefault();
              void submit();
            }
          }}
        />
        <div className="cmcomposerow">
          <span className="cmcounter">{draft.trim().length} / 2000</span>
          <button
            type="button"
            className="pl-solid pl-h-sm"
            disabled={busy || draft.trim().length < 3}
            onClick={() => void submit()}
          >
            <IcoSendL size={13} strokeWidth={2} />
            {busy ? "发送中…" : "发送"}
          </button>
        </div>
      </div>

      {err && <div className="set-warn cmwarn">{err}</div>}
      </section>
    </div>
  );
}
