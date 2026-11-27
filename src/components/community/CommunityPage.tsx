/* [lc-2402] 社区页（P1：账号 + 设备管理）。
 * 未登录 → AuthPanel（登录/注册）；已登录 → 账号卡 + 设备管理 + 退出登录。
 * 页头与 JavHubPage 同款：原生 page-head（圆‹返回 + 标题）。
 * P3 会在此页扩展信息流 / 通知；P4 扩展管理台（role=admin）。 */
import { useEffect, useState } from "react";
import { LogOut as IcoLogout, ShieldCheck as IcoShield } from "lucide-react";
import {
  communityTagOf,
  fetchSession,
  logoutAccount,
  type CommunitySession,
} from "../../lib/community";
import { AuthPanel } from "./AuthPanel";
import { DeviceManager } from "./DeviceManager";

type LoadState = "loading" | CommunitySession | null;

export function CommunityPage({ onBack }: { onBack: () => void }) {
  const [session, setSession] = useState<LoadState>("loading");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  useEffect(() => {
    let alive = true;
    void fetchSession().then((s) => {
      if (alive) setSession(s);
    });
    return () => {
      alive = false;
    };
  }, []);

  const doLogout = async () => {
    setBusy(true);
    setErr("");
    try {
      await logoutAccount();
      setSession(null);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const isAdmin = session !== "loading" && session !== null && session.role === "admin";

  return (
    <main className="content lib-root cv2-page">
      <div className="page-head">
        <button className="back" onClick={onBack}>‹</button>
        <h3>社区</h3>
        {session !== "loading" && session !== null && (
          <span className="muted">{communityTagOf(session)}</span>
        )}
        {isAdmin && (
          <span className="cv2-admin-tag" title="管理员">
            <IcoShield size={12} strokeWidth={2} /> 管理员
          </span>
        )}
      </div>

      {session === "loading" && <div className="cv2-center"><div className="empty">加载中…</div></div>}

      {session !== "loading" && session === null && (
        <div className="cv2-center">
          <AuthPanel
            onDone={(s) => {
              setSession(s);
            }}
          />
        </div>
      )}

      {session !== "loading" && session !== null && (
        <div className="cv2-body">
          <section className="cv2-card cv2-acct">
            <span className="cv2-avatar" aria-hidden>
              {session.nick.slice(0, 1).toUpperCase() || "?"}
            </span>
            <div className="cv2-acct-main">
              <div className="cv2-acct-name">{communityTagOf(session)}</div>
              <div className="cv2-acct-mail">{session.email || "—"}</div>
            </div>
            <button
              type="button"
              className="cv2-btn-ghost"
              onClick={() => void doLogout()}
              disabled={busy}
            >
              <IcoLogout size={13} strokeWidth={2} />
              {busy ? "退出中…" : "退出登录"}
            </button>
          </section>

          <DeviceManager />

          {err && <div className="set-warn cv2-warn">{err}</div>}
        </div>
      )}
    </main>
  );
}
