/* [lc-2181] 青空（sorani）站源登录弹窗。
   青空模块播放必须登录（accessToken），流程照抄上游 guoapp（lib/ui/settings/
   sorani_account_view.dart，GPL-3.0）：账号/邮箱 + 密码 → 滑块拼图验证
   （拖动轨迹随登录请求提交）→ 登录成功回调。会话由核心持久化，重启后仍有效。

   协议（Rust 侧 duanju_captcha / duanju_login，照抄上游 core_bridge.dart）：
   captcha → { id, backgroundImage, templateImage(dataURI), backgroundWidth/Height,
               templateWidth/Height }（图为服务器原生尺寸，前端按显示宽等比缩放，
               轨迹坐标与尺寸都用**缩放后**的显示值，与上游 _SoraniLoginDialog 同口径）。
   login   → identifier/password/captchaId/track，trackList 点 = {x,y,type,t}
             （x,y 为相对按下原点的位移，t 为相对按下的毫秒数）。 */

import { useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { ChevronRight as IcoRight, RotateCw as IcoReload } from "lucide-react";
import { DUR } from "../../lib/motion";

type SoraniCaptcha = {
  id: string;
  backgroundImage: string;
  templateImage: string;
  backgroundWidth: number;
  backgroundHeight: number;
  templateWidth: number;
  templateHeight: number;
};

type CaptchaPoint = { x: number; y: number; type: string; t: number };

/** 显示宽度上限（与上游桌面端同口径：min(300, 可用宽)） */
const CAP_WIDTH = 300;
/** 轨迹点数上限（上游 _maxPoints） */
const MAX_POINTS = 4000;

/** 上游 SoraniCaptchaTrack 同形状 */
function buildTrack(
  captcha: SoraniCaptcha,
  scale: number,
  limit: number,
  points: CaptchaPoint[],
  startTime: number,
): Record<string, unknown> {
  return {
    bgImageWidth: Math.round(CAP_WIDTH),
    bgImageHeight: Math.round(captcha.backgroundHeight * scale),
    templateImageWidth: Math.round(captcha.templateWidth * scale),
    templateImageHeight: Math.round(captcha.templateHeight * scale),
    startTime,
    stopTime: Date.now(),
    trackList: points.map((p) => ({
      // x 钳在滑块可达范围内（上游 _record 同口径）
      x: Math.round(Math.max(0, Math.min(limit, p.x))),
      y: Math.round(p.y),
      type: p.type,
      t: p.t,
    })),
  };
}

export function SoraniLoginModal({
  onDone,
  onClose,
}: {
  /** 登录成功（onDone(账号名)）；调用方负责刷新账号状态与重试播放 */
  onDone: (name: string) => void;
  /** 取消/关闭 */
  onClose: () => void;
}) {
  const [verifying, setVerifying] = useState(false); // 是否进入滑块验证步
  const [identifier, setIdentifier] = useState("");
  const [password, setPassword] = useState("");
  const [captcha, setCaptcha] = useState<SoraniCaptcha | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [offset, setOffset] = useState(0);

  // 拖动状态（refs：拖动中高频更新，不进 state）
  const originRef = useRef<number | null>(null);
  const originYRef = useRef(0);
  const pointsRef = useRef<CaptchaPoint[]>([]);
  const startedRef = useRef(0);

  // 关闭动画（照 AdultGateModal 口径）
  const [closing, setClosing] = useState(false);
  const run = (fn: () => void) => {
    if (closing) return;
    setClosing(true);
    window.setTimeout(fn, DUR.tap);
  };

  async function loadCaptcha() {
    setError("");
    setCaptcha(null);
    setOffset(0);
    originRef.current = null;
    pointsRef.current = [];
    try {
      const c = await invoke<SoraniCaptcha>("duanju_captcha");
      if (c?.id) setCaptcha(c);
      else setError(String(c) || "验证码获取失败");
    } catch (e) {
      setError(String(e));
    }
  }

  function next() {
    if (!identifier.trim() || !password) {
      setError("请输入用户名或邮箱和密码");
      return;
    }
    setError("");
    setVerifying(true);
    loadCaptcha();
  }

  async function submitTrack(
    captcha0: SoraniCaptcha,
    scale: number,
    limit: number,
    points: CaptchaPoint[],
    startTime: number,
  ) {
    setBusy(true);
    setError("");
    try {
      const track = buildTrack(captcha0, scale, limit, points, startTime);
      const r = await invoke<{ loggedIn?: boolean; name?: string }>("duanju_login", {
        identifier: identifier.trim(),
        password,
        captchaId: captcha0.id,
        track,
      });
      run(() => onDone(r?.name || "青空账号"));
    } catch (e) {
      const msg = String(e);
      setBusy(false);
      setError(msg);
      if (!msg.includes("安全验证")) {
        // 非验证码问题（密码错误等）：回表单（照上游口径）
        setVerifying(false);
        setCaptcha(null);
      } else {
        // 验证码校验失败：原地换一张
        loadCaptcha();
      }
    }
  }

  // ── 滑块拖动 ──
  function down(e: React.PointerEvent<HTMLDivElement>) {
    if (busy || originRef.current !== null) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    originRef.current = e.clientX;
    originYRef.current = e.clientY;
    startedRef.current = Date.now();
    pointsRef.current = [];
    pointsRef.current.push({ x: 0, y: 0, type: "down", t: 0 });
  }

  function move(e: React.PointerEvent<HTMLDivElement>, limit: number) {
    const origin = originRef.current;
    if (origin === null || busy) return;
    const x = Math.max(0, Math.min(limit, e.clientX - origin));
    setOffset(x);
    const pts = pointsRef.current;
    if (pts.length < MAX_POINTS) {
      pts.push({
        x,
        y: Math.round(e.clientY - originYRef.current),
        type: "move",
        t: Date.now() - startedRef.current,
      });
    }
  }

  function up(e: React.PointerEvent<HTMLDivElement>, captcha0: SoraniCaptcha, scale: number, limit: number) {
    const origin = originRef.current;
    if (origin === null || busy) return;
    const x = Math.max(0, Math.min(limit, e.clientX - origin));
    pointsRef.current.push({ x, y: 0, type: "up", t: Date.now() - startedRef.current });
    originRef.current = null;
    setOffset(x);
    const pts = pointsRef.current;
    if (x < 2 || pts.length < 2) {
      // 位移过小/无轨迹：不算一次尝试（照上游口径直接复位）
      setOffset(0);
      pointsRef.current = [];
      return;
    }
    submitTrack(captcha0, scale, limit, pts, startedRef.current);
  }

  const scale = captcha ? CAP_WIDTH / captcha.backgroundWidth : 1;
  const tplW = captcha ? captcha.templateWidth * scale : 0;
  const tplH = captcha ? captcha.templateHeight * scale : 0;
  const limit = CAP_WIDTH - tplW;

  return createPortal(
    <div className={`ddet-mask${closing ? " dsc-mask-closing" : ""}`}>
      <div className="ddet dsc-modal" onClick={(e) => e.stopPropagation()}>
        <header className="ddet-hd">
          <div className="ddet-hdm">
            <h2>{verifying ? "安全验证" : "登录青空"}</h2>
          </div>
        </header>
        <div className="dsc-body sor-login">
          {!verifying ? (
            <>
              <input
                className="sor-input"
                placeholder="用户名或邮箱"
                maxLength={128}
                autoFocus
                value={identifier}
                onChange={(e) => setIdentifier(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") next();
                }}
              />
              <input
                className="sor-input"
                placeholder="密码"
                type="password"
                maxLength={128}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") next();
                }}
              />
              {error && <div className="sor-error">{error}</div>}
            </>
          ) : (
            <>
              <div className="sor-cap-tip">拖动下方滑块，让拼图块与缺口重合</div>
              {captcha && (
                <div className="sor-cap-img" style={{ width: CAP_WIDTH, height: captcha.backgroundHeight * scale }}>
                  <img src={captcha.backgroundImage} alt="" draggable={false} />
                  <img
                    src={captcha.templateImage}
                    alt=""
                    draggable={false}
                    style={{ left: offset, width: tplW, height: tplH }}
                  />
                  {busy && <div className="sor-cap-busy" />}
                </div>
              )}
              {captcha && (
                <div className="sor-slider" style={{ width: CAP_WIDTH }}>
                  <div className="sor-slider-fill" style={{ width: offset + tplW }} />
                  <div
                    className="sor-slider-handle"
                    style={{ left: offset, width: tplW }}
                    onPointerDown={(e) => down(e)}
                    onPointerMove={(e) => move(e, limit)}
                    onPointerUp={(e) => up(e, captcha, scale, limit)}
                  >
                    <IcoRight size={16} strokeWidth={2.4} />
                  </div>
                </div>
              )}
              {!captcha && !error && <div className="sor-cap-loading">加载验证码…</div>}
              {error && <div className="sor-error">{error}</div>}
            </>
          )}
        </div>
        <div className="dsc-acts">
          <button
            className="dsc-btn ghost"
            disabled={busy}
            onClick={() => {
              if (verifying) {
                setVerifying(false);
                setError("");
                setCaptcha(null);
              } else {
                run(onClose);
              }
            }}
          >
            {verifying ? "返回" : "取消"}
          </button>
          {verifying ? (
            <button className="dsc-btn ghost" disabled={busy} onClick={loadCaptcha}>
              <IcoReload size={14} strokeWidth={2.2} />
              换一张
            </button>
          ) : (
            <button className="dsc-btn primary" onClick={next}>
              下一步
            </button>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
