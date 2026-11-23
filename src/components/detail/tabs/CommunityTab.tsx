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

export function CommunityTab({ mediaType, tmdbId, title }: Props) {
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

  if (!enabled) {
    return <div className="dpanel cmpanel"><div className="empty">这部作品缺少 TMDB 编号，无法关联评论</div></div>;
  }

  return (
    <div className="dpanel cmpanel">
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
    </div>
  );
}
