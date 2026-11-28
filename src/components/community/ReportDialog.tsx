/* [lc-2446] 举报弹窗（登录用户可用；同人同内容去重由服务端 UNIQUE 约束保证）。
 * 3 个不同人举报同一内容 → 服务端自动隐藏待审。 */
import { useState } from "react";
import { Flag as IcoFlag } from "lucide-react";
import { submitReport } from "../../lib/community";

export function ReportDialog({
  kind,
  refId,
  onClose,
}: {
  kind: "post" | "comment" | "image" | "account";
  refId: string;
  onClose: () => void;
}) {
  const [reason, setReason] = useState("");
  const [detail, setDetail] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [done, setDone] = useState(false);

  const submit = async () => {
    if (!reason || busy) return;
    setBusy(true);
    setErr("");
    try {
      await submitReport({ kind, refId, reason, detail: detail.trim() });
      setDone(true);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="cv2-rpt-mask" onClick={onClose} role="presentation">
      <div
        className="cv2-rpt"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label="举报"
      >
        <h4 className="cv2-rpt-t">举报该{kind === "post" ? "帖子" : kind === "comment" ? "评论" : kind === "image" ? "图片" : "账号"}</h4>
        {done ? (
          <>
            <p className="cv2-rpt-done">已收到举报，管理员会尽快处理。</p>
            <button type="button" className="pl-solid pl-h-sm cv2-rpt-ok" onClick={onClose}>
              好
            </button>
          </>
        ) : (
          <>
            <div className="cv2-rpt-reasons">
              {(
                [
                  ["spam", "垃圾广告"],
                  ["abuse", "辱骂攻击"],
                  ["porn", "色情低俗"],
                  ["illegal", "违法信息"],
                  ["other", "其他"],
                ] as const
              ).map(([k, label]) => (
                <button
                  key={k}
                  type="button"
                  className={`cv2-rpt-reason${reason === k ? " on" : ""}`}
                  onClick={() => setReason(k)}
                >
                  {label}
                </button>
              ))}
            </div>
            <textarea
              className="cv2-input cv2-ta"
              rows={3}
              maxLength={500}
              value={detail}
              onChange={(e) => setDetail(e.target.value)}
              placeholder="补充说明（可选，≤500 字）"
            />
            <div className="cv2-composer-ft">
              <button type="button" className="cv2-btn-ghost" onClick={onClose}>
                取消
              </button>
              <button
                type="button"
                className="pl-solid pl-h-sm"
                onClick={() => void submit()}
                disabled={!reason || busy}
              >
                <IcoFlag size={13} strokeWidth={2} />
                {busy ? "提交中…" : "提交举报"}
              </button>
            </div>
            {err && <div className="set-warn cv2-warn">{err}</div>}
          </>
        )}
      </div>
    </div>
  );
}
