/* [lc-2445] 社区页（P3）：信息流 / 我的发言 / 通知 三分区 + 发帖。
 * 未登录 → AuthPanel；已登录 → 三 tab。帖子详情为页内详情层（可返回）。
 * 被封账号（banned）可浏览不可写（读接口不拦封禁，服务端语义）。
 * 通知角标由 App 层 useCommunityUnread 轮询，本页打开时立即刷新。 */
import { useCallback, useEffect, useState } from "react";
import { Bell as IcoBell, Edit3 as IcoEdit, Home as IcoHome, UserRound as IcoUser } from "lucide-react";
import {
  communityTagOf,
  deletePost,
  fetchFeed,
  fetchMinePosts,
  fetchNotifications,
  fetchSession,
  readNotifications,
  type CommunityNotification,
  type CommunityPost,
  type CommunitySession,
} from "../../lib/community";
import { AuthPanel } from "./AuthPanel";
import { DeviceManager } from "./DeviceManager";
import { PostCard } from "./PostCard";
import { PostComposer } from "./PostComposer";
import { PostDetail } from "./PostDetail";

type Tab = "feed" | "mine" | "notifications";
type LoadState = "loading" | CommunitySession | null;

function fmtRel(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

export function CommunityPage({ onBack }: { onBack: () => void }) {
  const [session, setSession] = useState<LoadState>("loading");
  const [tab, setTab] = useState<Tab>("feed");
  const [posts, setPosts] = useState<CommunityPost[] | null>(null);
  const [feedErr, setFeedErr] = useState("");
  const [nextCursor, setNextCursor] = useState<number | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [detailId, setDetailId] = useState("");
  const [composing, setComposing] = useState(false);
  const [notifs, setNotifs] = useState<CommunityNotification[] | null>(null);
  const [notifUnread, setNotifUnread] = useState(0);
  const [err, setErr] = useState("");

  // 会话加载；通知未读顺带刷新（本页打开即刷，不等 180s 轮询）
  useEffect(() => {
    let alive = true;
    void fetchSession().then((s) => {
      if (!alive) return;
      setSession(s);
      if (s) {
        void fetchNotifications(0)
          .then((r) => {
            if (!alive) return;
            setNotifUnread(r.unread ?? 0);
          })
          .catch(() => {});
      }
    });
    return () => {
      alive = false;
    };
  }, []);

  const loadList = useCallback(
    async (cursor: number) => {
      if (!session || session === "loading") return;
      setFeedErr("");
      if (cursor === 0) setPosts(null);
      try {
        const r =
          tab === "feed"
            ? await fetchFeed("new", cursor)
            : tab === "mine"
              ? await fetchMinePosts(cursor)
              : { items: [], nextCursor: null };
        setPosts((prev) => (cursor === 0 || !prev ? r.items : [...prev, ...r.items]));
        setNextCursor(r.nextCursor);
      } catch (e) {
        setFeedErr(String(e));
        if (cursor === 0) setPosts([]);
      }
    },
    [tab, session],
  );

  // tab 切换 / 会话就绪 → 拉对应列表
  useEffect(() => {
    if (!session || session === "loading") return;
    if (tab === "notifications") {
      void fetchNotifications(0)
        .then((r) => {
          setNotifs(r.items);
          setNotifUnread(r.unread ?? 0);
          setNextCursor(r.nextCursor);
        })
        .catch((e) => setFeedErr(String(e)));
      return;
    }
    void loadList(0);
  }, [tab, session, loadList]);

  const loadMore = async () => {
    if (loadingMore || nextCursor === null) return;
    setLoadingMore(true);
    if (tab === "notifications") {
      try {
        const r = await fetchNotifications(nextCursor);
        setNotifs((prev) => [...(prev ?? []), ...r.items]);
        setNextCursor(r.nextCursor);
      } catch (e) {
        setFeedErr(String(e));
      }
    } else {
      await loadList(nextCursor);
    }
    setLoadingMore(false);
  };

  const remove = async (p: CommunityPost) => {
    setErr("");
    try {
      await deletePost(p.id);
      setPosts((prev) => prev?.map((x) => (x.id === p.id ? { ...x, deleted: true, body: null } : x)) ?? prev);
    } catch (e) {
      setErr(String(e));
    } finally {
    }
  };

  const readAll = async () => {
    try {
      await readNotifications({ all: true });
      setNotifs((prev) => prev?.map((n) => ({ ...n, readAt: n.readAt ?? Date.now() })) ?? prev);
      setNotifUnread(0);
    } catch (e) {
      setErr(String(e));
    }
  };

  const readOne = async (n: CommunityNotification) => {
    if (n.readAt) return;
    try {
      await readNotifications({ ids: [n.id] });
      setNotifs((prev) => prev?.map((x) => (x.id === n.id ? { ...x, readAt: Date.now() } : x)) ?? prev);
      setNotifUnread((u) => Math.max(0, u - 1));
    } catch {
      /* 已读失败不打断（下次轮询校正） */
    }
  };

  if (session === "loading") {
    return (
      <main className="content lib-root cv2-page">
        <div className="page-head">
          <button className="back" onClick={onBack}>‹</button>
          <h3>社区</h3>
        </div>
        <div className="cv2-center"><div className="empty">加载中…</div></div>
      </main>
    );
  }

  if (session === null) {
    return (
      <main className="content lib-root cv2-page">
        <div className="page-head">
          <button className="back" onClick={onBack}>‹</button>
          <h3>社区</h3>
        </div>
        <div className="cv2-center">
          <AuthPanel onDone={(s) => setSession(s)} />
        </div>
      </main>
    );
  }

  const banned = "banned" in session && (session as CommunitySession & { banned?: boolean }).banned;
  const s = session as CommunitySession & { banned?: boolean };

  return (
    <main className="content lib-root cv2-page">
      <div className="page-head">
        <button className="back" onClick={onBack}>‹</button>
        <h3>社区</h3>
        <span className="muted">{communityTagOf(s)}</span>
      </div>

      {banned && <div className="set-warn cv2-warn">账号已被封禁，仅可浏览</div>}

      {detailId ? (
        <PostDetail postId={detailId} onBack={() => setDetailId("")} />
      ) : (
        <>
          <div className="cv2-tabs">
            <button type="button" className={tab === "feed" ? "on" : ""} onClick={() => setTab("feed")}>
              <IcoHome size={13} strokeWidth={2} aria-hidden="true" /> 信息流
            </button>
            <button type="button" className={tab === "mine" ? "on" : ""} onClick={() => setTab("mine")}>
              <IcoUser size={13} strokeWidth={2} aria-hidden="true" /> 我的发言
            </button>
            <button type="button" className={tab === "notifications" ? "on" : ""} onClick={() => setTab("notifications")}>
              <IcoBell size={13} strokeWidth={2} aria-hidden="true" /> 通知
              {notifUnread > 0 && <span className="cv2-tab-badge">{notifUnread > 99 ? "99+" : notifUnread}</span>}
            </button>
          </div>

          {tab === "feed" && !composing && !banned && (
            <button type="button" className="cv2-compose-open" onClick={() => setComposing(true)}>
              <IcoEdit size={13} strokeWidth={2} aria-hidden="true" /> 分享一部作品…
            </button>
          )}
          {tab === "feed" && composing && (
            <div className="cv2-card">
              <PostComposer
                onPosted={(p) => {
                  setComposing(false);
                  setPosts((prev) => [p, ...(prev ?? [])]);
                  setTab("feed");
                }}
                onCancel={() => setComposing(false)}
              />
            </div>
          )}

          {err && <div className="set-warn cv2-warn">{err}</div>}

          {tab === "notifications" ? (
            <div className="cv2-notif">
              {(notifs ?? []).length > 0 && notifUnread > 0 && (
                <button type="button" className="cmlink cv2-notif-readall" onClick={() => void readAll()}>
                  全部已读
                </button>
              )}
              <div className="cv2-post-list">
                {(notifs ?? []).map((n) => (
                  <button
                    key={n.id}
                    type="button"
                    className={`cv2-notif-item${n.readAt ? "" : " unread"}`}
                    onClick={() => void readOne(n)}
                  >
                    <span className="cv2-notif-nick">{n.actorNick}</span>
                    <span className="cv2-notif-text">
                      {n.kind === "reply" ? "回复了你的评论" : n.kind}
                    </span>
                    <span className="cv2-notif-ex">{n.excerpt}</span>
                    <span className="cv2-post-time">{fmtRel(n.createdAt)}</span>
                  </button>
                ))}
                {(notifs ?? []).length === 0 && <div className="empty">没有通知</div>}
              </div>
            </div>
          ) : (
            <>
              {feedErr && <div className="set-warn cv2-warn">{feedErr}</div>}
              <div className="cv2-post-list">
                {(posts ?? []).map((p) => (
                  <div key={p.id} onClick={() => !p.deleted && setDetailId(p.id)} role="presentation">
                    <PostCard post={p} onDelete={tab === "mine" ? (x) => void remove(x) : undefined} />
                  </div>
                ))}
                {posts === null && !feedErr && <div className="empty">加载中…</div>}
                {posts !== null && posts.length === 0 && !feedErr && (
                  <div className="empty">{tab === "mine" ? "还没有发言" : "还没有帖子，发第一笔"}</div>
                )}
              </div>
              {nextCursor !== null && (
                <button type="button" className="cv2-more" onClick={() => void loadMore()} disabled={loadingMore}>
                  {loadingMore ? "加载中…" : "加载更多"}
                </button>
              )}
            </>
          )}

          {tab === "mine" && <DeviceManager />}
        </>
      )}
    </main>
  );
}
