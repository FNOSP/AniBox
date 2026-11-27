/* [lc-2402] 社区登录 / 注册面板。
 *
 * 注册三段：表单（邮箱+密码+昵称+隐私勾选）→ 验证码（服务端只存哈希，10 分钟）
 * → 成功建号并签发本设备会话（token 在 Rust 侧，前端拿不到）。
 * 密码规则服务端裁定（8–64 位、字母+数字、弱密码黑名单），失败时展示服务端中文文案。
 * 隐私告知按施工文档 §7.8 逐字使用：不勾不能注册（按钮 disabled）。
 */
import { useEffect, useRef, useState } from "react";
import {
  KeyRound as IcoKey,
  Mail as IcoMail,
  RefreshCw as IcoRefresh,
  Send as IcoSend,
  UserRound as IcoUser,
} from "lucide-react";
import {
  loginAccount,
  registerAccount,
  resendCode,
  verifyCode,
  type CommunitySession,
} from "../../lib/community";

/** 注册页隐私告知（docs/community-v2-design.md §7.8 逐字；每句承诺都有代码依据） */
const PRIVACY_NOTICE =
  "使用社区功能需要注册账号。为了社区安全与违规追责，我们会记录你的发言内容、操作动作、账号、设备标识与 IP 网段，保留 90 天。这些记录仅用于处理举报与违规，不对外提供。\n\n你可以在设置中注销账号。注销后我们会删除你的账号信息与 90 天内的审核日志；已被举报并处置的内容记录会继续保留，用于应对必要的核查。";

type Stage = "login" | "regForm" | "regCode";

export function AuthPanel({ onDone }: { onDone: (s: CommunitySession) => void }) {
  const [stage, setStage] = useState<Stage>("login");
  const [email, setEmail] = useState("");
  const [pwd, setPwd] = useState("");
  const [nick, setNick] = useState("");
  const [agree, setAgree] = useState(false);
  const [pending, setPending] = useState<{ pendingId: string; maskedEmail: string } | null>(null);
  const [code, setCode] = useState("");
  const [cooldown, setCooldown] = useState(0);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const codeRef = useRef<HTMLInputElement | null>(null);

  // 重发倒计时
  useEffect(() => {
    if (cooldown <= 0) return;
    const t = window.setInterval(() => setCooldown((c) => (c > 0 ? c - 1 : 0)), 1000);
    return () => window.clearInterval(t);
  }, [cooldown > 0]);

  const run = async (fn: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setErr("");
    try {
      await fn();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const submitRegister = () =>
    run(async () => {
      const r = await registerAccount(email.trim(), pwd, nick.trim());
      setPending({ pendingId: r.pendingId, maskedEmail: r.maskedEmail });
      setCooldown(r.cooldownSec || 60);
      setStage("regCode");
      window.setTimeout(() => codeRef.current?.focus(), 50);
    });

  const submitVerify = () =>
    run(async () => {
      if (!pending) return;
      const s = await verifyCode(pending.pendingId, code.trim());
      onDone(s);
    });

  const submitResend = () =>
    run(async () => {
      if (!pending) return;
      const r = await resendCode(pending.pendingId);
      setCooldown(r.cooldownSec || 60);
    });

  const submitLogin = () =>
    run(async () => {
      const s = await loginAccount(email.trim(), pwd);
      onDone(s);
    });

  const canRegister = email.trim().length > 0 && pwd.length >= 8 && nick.trim().length > 0 && agree && !busy;
  const canVerify = /^\d{6}$/.test(code.trim()) && !busy;
  const canLogin = email.trim().length > 0 && pwd.length > 0 && !busy;

  return (
    <div className="cv2-auth">
      <div className="cv2-auth-switch" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={stage === "login"}
          className={stage === "login" ? "on" : ""}
          onClick={() => {
            setStage("login");
            setErr("");
          }}
        >
          登录
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={stage !== "login"}
          className={stage !== "login" ? "on" : ""}
          onClick={() => {
            setStage("regForm");
            setErr("");
          }}
        >
          注册
        </button>
      </div>

      {stage === "login" && (
        <form
          className="cv2-form"
          onSubmit={(e) => {
            e.preventDefault();
            void submitLogin();
          }}
        >
          <label className="cv2-field">
            <span className="cv2-field-label">
              <IcoMail size={13} strokeWidth={2} /> 邮箱
            </span>
            <input
              className="cv2-input"
              type="email"
              value={email}
              autoComplete="email"
              onChange={(e) => setEmail(e.target.value)}
              placeholder="you@example.com"
            />
          </label>
          <label className="cv2-field">
            <span className="cv2-field-label">
              <IcoKey size={13} strokeWidth={2} /> 密码
            </span>
            <input
              className="cv2-input"
              type="password"
              value={pwd}
              autoComplete="current-password"
              onChange={(e) => setPwd(e.target.value)}
              placeholder="8 位以上，字母和数字"
            />
          </label>
          <button type="submit" className="pl-solid pl-h-sm" disabled={!canLogin}>
            {busy ? "登录中…" : "登录"}
          </button>
        </form>
      )}

      {stage === "regForm" && (
        <form
          className="cv2-form"
          onSubmit={(e) => {
            e.preventDefault();
            void submitRegister();
          }}
        >
          <label className="cv2-field">
            <span className="cv2-field-label">
              <IcoMail size={13} strokeWidth={2} /> 邮箱
            </span>
            <input
              className="cv2-input"
              type="email"
              value={email}
              autoComplete="email"
              onChange={(e) => setEmail(e.target.value)}
              placeholder="you@example.com"
            />
          </label>
          <label className="cv2-field">
            <span className="cv2-field-label">
              <IcoKey size={13} strokeWidth={2} /> 密码
            </span>
            <input
              className="cv2-input"
              type="password"
              value={pwd}
              autoComplete="new-password"
              onChange={(e) => setPwd(e.target.value)}
              placeholder="8 位以上，字母和数字"
            />
          </label>
          <label className="cv2-field">
            <span className="cv2-field-label">
              <IcoUser size={13} strokeWidth={2} /> 昵称
            </span>
            <input
              className="cv2-input"
              value={nick}
              maxLength={24}
              onChange={(e) => setNick(e.target.value)}
              placeholder="社区里显示的名字"
            />
          </label>
          <label className="cv2-agree">
            <input type="checkbox" checked={agree} onChange={(e) => setAgree(e.target.checked)} />
            <span>我已阅读并同意以下内容</span>
          </label>
          <div className="cv2-privacy">{PRIVACY_NOTICE}</div>
          <button type="submit" className="pl-solid pl-h-sm" disabled={!canRegister}>
            {busy ? "提交中…" : "获取验证码"}
          </button>
        </form>
      )}

      {stage === "regCode" && pending && (
        <form
          className="cv2-form"
          onSubmit={(e) => {
            e.preventDefault();
            void submitVerify();
          }}
        >
          <div className="cv2-code-hint">
            验证码已发送至 <b>{pending.maskedEmail}</b>
            <br />
            若未收到，请检查垃圾邮件
          </div>
          <input
            ref={codeRef}
            className="cv2-input cv2-code"
            inputMode="numeric"
            maxLength={6}
            value={code}
            onChange={(e) => setCode(e.target.value.replace(/\D/g, ""))}
            placeholder="6 位验证码"
          />
          <button type="submit" className="pl-solid pl-h-sm" disabled={!canVerify}>
            <IcoSend size={13} strokeWidth={2} />
            {busy ? "验证中…" : "创建账号"}
          </button>
          <div className="cv2-resend">
            {cooldown > 0 ? (
              <span className="cv2-resend-wait">{cooldown}s 后可重发</span>
            ) : (
              <button type="button" className="cmlink" onClick={() => void submitResend()} disabled={busy}>
                <IcoRefresh size={12} strokeWidth={2} /> 重新发送
              </button>
            )}
            <button type="button" className="cmlink" onClick={() => setStage("regForm")}>
              返回修改
            </button>
          </div>
        </form>
      )}

      {err && <div className="set-warn cv2-warn">{err}</div>}
    </div>
  );
}
