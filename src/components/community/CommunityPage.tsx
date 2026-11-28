/* [lc-2402→lc-2447] 社区页：
 *   四分区 tab：交流讨论（talk，绑作品）｜ 反馈建议（feedback，软件建议公开帖）
 *              ｜ 个人中心（账号卡+统计+改昵称+设备管理+我的发言）｜ 通知
 *   role=admin 追加「管理台」。
 * 未登录 → AuthPanel；被封（banned）可浏览不可写（服务端 readOk 语义）。
 * 通知角标由 App 层 useCommunityUnread 轮询，本页打开时立即刷新。 */
import { useCallback, useEffect, useState } from "react";
import { Bell as IcoBell, Edit3 as IcoEdit, LogOut as IcoLogout, ShieldCheck as IcoShield, UserRound as IcoUser } from "lucide-react";
import {
  changeNick,
  communityTagOf,
  deletePost,
  fetchFeed,
  fetchMe,
  fetchMinePosts,
  fetchNotifications,
  fetchSession,
  logoutAccount,
  readNotifications,
  type CommunityMe,
  type CommunityNotification,
  type CommunityPost,
  type CommunitySession,
} from "../../lib/community";
import { AdminPanel } from "./AdminPanel";
import { AuthPanel } from "./AuthPanel";
import { DeviceManager } from "./DeviceManager";
import { PostCard } from "./PostCard";
import { PostComposer } from "./PostComposer";
import { PostDetail } from "./PostDetail";

type Tab = "talk" | "feedback" | "me" | "notifications" | "admin";
type LoadState = "loading" | CommunitySession | null;

function fmtRel(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function fmtDate(ms: number): string {
  if (!ms) return "—";
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

export function CommunityPage({ onBack }: { onBack: () => void }) {
  const [session, setSession] = useState<LoadState>("loading");
  const [tab, setTab] = useState<Tab>("talk");
  const [posts, setPosts] = useState<CommunityPost[] | null>(null);
  const [feedErr, setFeedErr] = useState("");
  const [nextCursor, setNextCursor] = useState<number | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [detailId, setDetailId] = useState("");
  const [composing, setComposing] = useState(false);
  const [notifs, setNotifs] = useState<CommunityNotification[] | null>(null);
  const [notifUnread, setNotifUnread] = useState(0);
  const [me, setMe] = useState<CommunityMe | null>(null);
  const [nickEditing, setNickEditing] = useState(false);
  const [nickDraft, setNickDraft] = useState("");
  const [nickBusy, setNickBusy] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  // 会话加载；本页打开即刷一次通知未读
  useEffect(() => {
    let alive = true;
    void fetchSession().then((v) => {
      if (!alive) return;
      setSession(v);
      if (v) {
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
      if (tab === "admin") return; // 管理台自己拉数据
      setFeedErr("");
      if (cursor === 0) setPosts(null);
      try {
        const r =
          tab === "talk" || tab === "feedback"
            ? await fetchFeed("new", tab, cursor)
            : tab === "me"
              ? await fetchMinePosts(null, cursor)
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

  // tab 切换 / 会话就绪 → 拉对应列表或个人数据
  useEffect(() => {
    if (!session || session === "loading") return;
    if (tab === "admin") return;
    if (tab === "me") {
      void fetchMe()
        .then((m) => setMe(m))
        .catch((e) => setFeedErr(String(e)));
    }
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

  const doLogout = async () => {
    setBusy(true);
    setErr("");
    try {
      await logoutAccount();
      setSession(null);
      setMe(null);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
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
          <AuthPanel onDone={(v) => setSession(v)} />
        </div>
      </main>
    );
  }

  const banned = "banned" in session && (session as CommunitySession & { banned?: boolean }).banned;
  const sess = session as CommunitySession & { banned?: boolean };
  const isAdmin = sess.role === "admin";
  const isBoard = tab === "talk" || tab === "feedback";

  return (
    <main className="content lib-root cv2-page">
      <div className="page-head">
        <button className="back" onClick={onBack}>‹</button>
        <h3>社区</h3>
        <span className="muted">{communityTagOf(sess)}</span>
        {isAdmin && (
          <span className="cv2-admin-tag" title="管理员">
            <IcoShield size={12} strokeWidth={2} /> 管理员
          </span>
        )}
      </div>

      {banned && <div className="set-warn cv2-warn">账号已被封禁，仅可浏览</div>}

      {detailId ? (
        <PostDetail postId={detailId} onBack={() => setDetailId("")} />
      ) : (
        <>
          <div className="cv2-tabs">
            <button type="button" className={tab === "talk" ? "on" : ""} onClick={() => setTab("talk")}>
              交流讨论
            </button>
            <button type="button" className={tab === "feedback" ? "on" : ""} onClick={() => setTab("feedback")}>
              反馈建议
            </button>
            <button type="button" className={tab === "me" ? "on" : ""} onClick={() => setTab("me")}>
              <IcoUser size={13} strokeWidth={2} aria-hidden="true" /> 个人中心
            </button>
            <button type="button" className={tab === "notifications" ? "on" : ""} onClick={() => setTab("notifications")}>
              <IcoBell size={13} strokeWidth={2} aria-hidden="true" /> 通知
              {notifUnread > 0 && <span className="cv2-tab-badge">{notifUnread > 99 ? "99+" : notifUnread}</span>}
            </button>
            {isAdmin && (
              <button type="button" className={tab === "admin" ? "on" : ""} onClick={() => setTab("admin")}>
                管理台
              </button>
            )}
          </div>

          {isBoard && !composing && !banned && (
            <button type="button" className="cv2-compose-open" onClick={() => setComposing(true)}>
              <IcoEdit size={13} strokeWidth={2} aria-hidden="true" />
              {tab === "talk" ? "分享一部作品…" : "提一条反馈建议…"}
            </button>
          )}
          {isBoard && composing && (
            <div className="cv2-card">
              <PostComposer
                board={tab === "feedback" ? "feedback" : "talk"}
                onPosted={(p) => {
                  setComposing(false);
                  setPosts((prev) => [p, ...(prev ?? [])]);
                  setTab(p.board);
                }}
                onCancel={() => setComposing(false)}
              />
            </div>
          )}

          {err && <div className="set-warn cv2-warn">{err}</div>}

          {tab === "admin" ? (
            <AdminPanel />
          ) : tab === "me" ? (
            <div className="cv2-me">
              {me && (
                <section className="cv2-card cv2-acct">
                  <span className="cv2-avatar" aria-hidden>
                    {me.nick.slice(0, 1).toUpperCase() || "?"}
                  </span>
                  <div className="cv2-acct-main">
                    <div className="cv2-acct-name">{communityTagOf(me)}</div>
                    <div className="cv2-acct-mail">{me.email}</div>
                    <div className="cv2-acct-stats">
                      发帖 {me.stats.posts} · 短评 {me.stats.comments} · 注册于 {fmtDate(me.createdAt)}
                    </div>
                  </div>
                  {!nickEditing && (
                    <>
                      <button
                        type="button"
                        className="cv2-btn-ghost"
                        onClick={() => {
                          setNickEditing(true);
                          setNickDraft(me.nick);
                        }}
                      >
                        改昵称
                      </button>
                      <button
                        type="button"
                        className="cv2-btn-ghost"
                        onClick={() => void doLogout()}
                        disabled={busy}
                      >
                        <IcoLogout size={13} strokeWidth={2} />
                        退出
                      </button>
                    </>
                  )}
                </section>
              )}
              {nickEditing && (
                <div className="cv2-card cv2-nickedit">
                  <input
                    className="cv2-input"
                    value={nickDraft}
                    maxLength={24}
                    onChange={(e) => setNickDraft(e.target.value)}
                    placeholder="新昵称（1–24 字）"
                  />
                  <button
                    type="button"
                    className="pl-solid pl-h-sm"
                    disabled={nickBusy || nickDraft.trim().length < 1}
                    onClick={() =>
                      void (async () => {
                        setNickBusy(true);
                        setErr("");
                        try {
                          await changeNick(nickDraft.trim());
                          const m = await fetchMe();
                          setMe(m);
                          setSession((prev) =>
                            prev && prev !== "loading" ? { ...prev, nick: m.nick } : prev,
                          );
                          setNickEditing(false);
                        } catch (e) {
                          setErr(String(e));
                        } finally {
                          setNickBusy(false);
                        }
                      })()
                    }
                  >
                    {nickBusy ? "保存中…" : "保存"}
                  </button>
                  <button type="button" className="cv2-btn-ghost" onClick={() => setNickEditing(false)}>
                    取消
                  </button>
                </div>
              )}
              <DeviceManager />
              <h4 className="cv2-cmsec-t">我的发言（永久可翻）</h4>
              <div className="cv2-post-list">
                {(posts ?? []).map((p) => (
                  <div key={p.id} onClick={() => !p.deleted && setDetailId(p.id)} role="presentation">
                    <PostCard
                      post={p}
                      onDelete={(x) => void remove(x)}
                      canReport={false}
                      onChanged={(updated) =>
                        setPosts((prev) => prev?.map((x) => (x.id === updated.id ? updated : x)) ?? prev)
                      }
                    />
                  </div>
                ))}
                {posts === null && !feedErr && <div className="empty">加载中…</div>}
                {posts !== null && posts.length === 0 && !feedErr && <div className="empty">还没有发言</div>}
              </div>
              {nextCursor !== null && (
                <button type="button" className="cv2-more" onClick={() => void loadMore()} disabled={loadingMore}>
                  {loadingMore ? "加载中…" : "加载更多"}
                </button>
              )}
            </div>
          ) : tab === "notifications" ? (
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
                    <PostCard
                      post={p}
                      canReport={!p.mine}
                      isAdmin={isAdmin}
                      onChanged={(updated) =>
                        setPosts((prev) => prev?.map((x) => (x.id === updated.id ? updated : x)) ?? prev)
                      }
                    />
                  </div>
                ))}
                {posts === null && !feedErr && <div className="empty">加载中…</div>}
                {posts !== null && posts.length === 0 && !feedErr && (
                  <div className="empty">还没有帖子，发第一笔</div>
                )}
              </div>
              {nextCursor !== null && (
                <button type="button" className="cv2-more" onClick={() => void loadMore()} disabled={loadingMore}>
                  {loadingMore ? "加载中…" : "加载更多"}
                </button>
              )}
            </>
          )}
        </>
      )}
    </main>
  );
}
