/* [lc-2446] 管理台（仅 role=admin 可见；每个操作服务端 requireAdmin 二次校验）。
 * 三子页：举报队列（处理 + 快捷撤回/删帖/删图/封号/封设备）、审计日志（过滤查询）、
 * 总览（stats：用户/内容/存储/待处理举报）。 */
import { useCallback, useEffect, useState } from "react";
import {
  adminAudit,
  adminBanAccount,
  adminDeleteImage,
  adminDeletePost,
  adminMuteAccount,
  adminReportHandle,
  adminRetractComment,
  adminRetractPost,
  adminReports,
  adminStats,
  type CommunityAuditRow,
  type CommunityReport,
  type CommunityStats,
} from "../../lib/community";

const REASON_LABEL: Record<string, string> = {
  spam: "垃圾广告",
  abuse: "辱骂攻击",
  porn: "色情低俗",
  illegal: "违法信息",
  other: "其他",
};

function fmtTs(ms: number | null): string {
  if (!ms) return "—";
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

function fmtSize(n: number): string {
  if (n < 1024 * 1024) return `${Math.round(n / 1024)}KB`;
  return `${(n / (1024 * 1024)).toFixed(1)}MB`;
}

type SubTab = "reports" | "audit" | "stats";

export function AdminPanel() {
  const [sub, setSub] = useState<SubTab>("reports");
  const [reports, setReports] = useState<CommunityReport[] | null>(null);
  const [busyId, setBusyId] = useState("");
  const [note, setNote] = useState("");
  const [err, setErr] = useState("");
  const [stats, setStats] = useState<CommunityStats | null>(null);
  // 审计查询
  const [auditAction, setAuditAction] = useState("admin_");
  const [auditRows, setAuditRows] = useState<CommunityAuditRow[] | null>(null);
  const [auditTotal, setAuditTotal] = useState(0);

  const loadReports = useCallback(async () => {
    try {
      const r = await adminReports("open");
      setReports(r.items);
    } catch (e) {
      setErr(String(e));
    }
  }, []);

  const loadStats = useCallback(async () => {
    try {
      setStats(await adminStats());
    } catch (e) {
      setErr(String(e));
    }
  }, []);

  const loadAudit = useCallback(async () => {
    try {
      const r = await adminAudit({ action: auditAction });
      setAuditRows(r.items);
      setAuditTotal(r.total);
    } catch (e) {
      setErr(String(e));
    }
  }, [auditAction]);

  useEffect(() => {
    if (sub === "reports") void loadReports();
    if (sub === "audit") void loadAudit();
    if (sub === "stats") void loadStats();
  }, [sub, loadReports, loadAudit, loadStats]);

  const run = async (id: string, fn: () => Promise<void>) => {
    setBusyId(id);
    setErr("");
    setNote("");
    try {
      await fn();
      setNote("已执行");
      await loadReports();
      await loadStats();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusyId("");
    }
  };

  /** 举报行快捷处置：按 kind 执行对应管理动作（不 resolve 举报本身——先处置再人工关） */
  const quickAct = (r: CommunityReport, act: string) =>
    run(r.id, async () => {
      const reason = REASON_LABEL[r.reason] ?? "违规";
      if (r.kind === "post" && act === "retract") await adminRetractPost(r.refId, reason);
      else if (r.kind === "post" && act === "delete") await adminDeletePost(r.refId, reason);
      else if (r.kind === "comment") await adminRetractComment(r.refId, reason);
      else if (r.kind === "image") await adminDeleteImage(r.refId, reason);
      else if (r.kind === "account" && act === "ban") await adminBanAccount(r.refId, reason);
      // 顺手把这条举报关掉
      await adminReportHandle(r.id, "resolve", `${act} by quick action`);
    });

  return (
    <div className="cv2-admin">
      <div className="cv2-tabs">
        <button type="button" className={sub === "reports" ? "on" : ""} onClick={() => setSub("reports")}>
          举报队列
        </button>
        <button type="button" className={sub === "audit" ? "on" : ""} onClick={() => setSub("audit")}>
          审计日志
        </button>
        <button type="button" className={sub === "stats" ? "on" : ""} onClick={() => setSub("stats")}>
          总览
        </button>
      </div>

      {err && <div className="set-warn cv2-warn">{err}</div>}
      {note && <div className="cv2-admin-note">{note}</div>}

      {sub === "reports" && (
        <div className="cv2-post-list">
          {reports === null && <div className="empty">加载中…</div>}
          {reports !== null && reports.length === 0 && <div className="empty">没有待处理的举报</div>}
          {(reports ?? []).map((r) => (
            <div className="cv2-card cv2-rptrow" key={r.id}>
              <div className="cv2-rptrow-head">
                <span className="cv2-work-chip">{r.kind}</span>
                <span className="cv2-rptrow-reason">{REASON_LABEL[r.reason] ?? r.reason}</span>
                <span className="cv2-post-time">{fmtTs(r.createdAt)}</span>
              </div>
              {r.excerpt && <div className="cv2-rptrow-ex">{r.excerpt}</div>}
              {r.detail && <div className="cv2-rptrow-detail">举报说明：{r.detail}</div>}
              <div className="cv2-rptrow-ops">
                {r.kind === "post" && (
                  <>
                    <button type="button" className="cmlink" onClick={() => void quickAct(r, "retract")} disabled={busyId === r.id}>
                      撤回该帖
                    </button>
                    <button type="button" className="cmlink cv2-post-adminact danger" onClick={() => void quickAct(r, "delete")} disabled={busyId === r.id}>
                      删除该帖
                    </button>
                  </>
                )}
                {r.kind === "comment" && (
                  <button type="button" className="cmlink" onClick={() => void quickAct(r, "retract")} disabled={busyId === r.id}>
                    撤回该评论
                  </button>
                )}
                {r.kind === "image" && (
                  <button type="button" className="cmlink cv2-post-adminact danger" onClick={() => void quickAct(r, "delete")} disabled={busyId === r.id}>
                    删除该图
                  </button>
                )}
                {r.kind === "account" && (
                  <>
                    <button type="button" className="cmlink" onClick={() => void quickAct(r, "ban")} disabled={busyId === r.id}>
                      封禁该账号
                    </button>
                    <button
                      type="button"
                      className="cmlink"
                      onClick={() => void run(r.id, async () => { await adminMuteAccount(r.refId, 60 * 24, "禁言 24h"); })}
                      disabled={busyId === r.id}
                    >
                      禁言 24h
                    </button>
                  </>
                )}
                <button
                  type="button"
                  className="cmlink"
                  onClick={() => void run(r.id, async () => { await adminReportHandle(r.id, "resolve", "已处理"); })}
                  disabled={busyId === r.id}
                >
                  标记已处理
                </button>
                <button
                  type="button"
                  className="cmlink"
                  onClick={() => void run(r.id, async () => { await adminReportHandle(r.id, "reject", "证据不足"); })}
                  disabled={busyId === r.id}
                >
                  驳回
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {sub === "audit" && (
        <div className="cv2-admin-audit">
          <div className="cv2-admin-auditbar">
            <span>动作前缀：</span>
            {["admin_", "post_", "comment_", "image_", "report_", "feedback_"].map((a) => (
              <button key={a} type="button" className={`cv2-composer-mt buttonlike${auditAction === a ? " on" : ""}`} onClick={() => setAuditAction(a)}>
                {a.replace("_", "")}
              </button>
            ))}
            <button type="button" className="cmlink" onClick={() => void loadAudit()}>
              刷新
            </button>
            <span className="cv2-post-time">共 {auditTotal} 条（admin_* 长期保留，其余 90 天）</span>
          </div>
          <div className="cv2-admin-auditlist">
            {(auditRows ?? []).map((r) => (
              <div className="cv2-auditrow" key={r.id}>
                <span className="cv2-auditrow-ts">{fmtTs(r.ts)}</span>
                <span className="cv2-auditrow-action">{r.action}</span>
                <span className="cv2-auditrow-who">
                  {r.actorNick || "—"} {r.actorDev ? `· ${r.actorDev.slice(0, 8).toUpperCase()}` : ""} · {r.ipNet}
                </span>
                {(r.bodySnap || r.detail) && (
                  <span className="cv2-auditrow-snap">{(r.bodySnap || r.detail || "").slice(0, 120)}</span>
                )}
              </div>
            ))}
            {auditRows !== null && auditRows.length === 0 && <div className="empty">没有匹配记录</div>}
            {auditRows === null && <div className="empty">加载中…</div>}
          </div>
        </div>
      )}

      {sub === "stats" && stats && (
        <div className="cv2-admin-stats">
          <div className="cv2-statrow">
            <span className="cv2-statnum">{stats.users}</span>
            <span className="cv2-statlabel">账号</span>
          </div>
          <div className="cv2-statrow">
            <span className="cv2-statnum">{stats.posts}</span>
            <span className="cv2-statlabel">帖子</span>
          </div>
          <div className="cv2-statrow">
            <span className="cv2-statnum">{stats.comments}</span>
            <span className="cv2-statlabel">短评</span>
          </div>
          <div className="cv2-statrow">
            <span className="cv2-statnum">{stats.images}</span>
            <span className="cv2-statlabel">图片</span>
          </div>
          <div className="cv2-statrow">
            <span className="cv2-statnum">{stats.openReports}</span>
            <span className="cv2-statlabel">待处理举报</span>
          </div>
          <div className="cv2-statrow">
            <span className="cv2-statnum">
              {fmtSize(stats.storageUsed)} / {fmtSize(stats.storageCap)}
            </span>
            <span className="cv2-statlabel">图片存储（超限自动淘汰）</span>
          </div>
        </div>
      )}
    </div>
  );
}
