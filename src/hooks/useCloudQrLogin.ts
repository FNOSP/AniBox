/* [lc-1902] 网盘扫码登录的统一状态机（登录页与「网盘接入」页共用）。

   背景：两处各写了一份 `setInterval(2000)` + 各自的 qr/qrMsg/stopPoll，问题一致：
     · setInterval 不等待上一次 invoke 返回 → 轮询会叠罗汉（百度 channel/unicast 是 30s
       长轮询，一次请求还在飞，下一拍又发一条，越堆越多，上游直接限流）；
     · 后端回「已过期」只 setErr 停轮询 → 用户看到一张死码，必须自己点刷新；
     · 没有倒计时，不知道码什么时候失效。
   这里改成「一轮结束再等 2s」的串行循环：到点（expiresAtMs，与后端 TTL 同源）自动换码，
   过期/瞬时网络失败都有上限重试，最终停在明确文案上等用户手点。

   [lc-1903] 两处修正：
     · 到点先进 90s 宽限窗继续问上游（确认往往发生在最后几秒），只有上游明确回 expires 才换码，
       避免把「已确认的码」刷掉（表现就是「手机点了确认、桌面没反应」）；
     · 失败按 网络 / 授权 / 限流 三类给文案，不再一律「网络不稳定」。

   状态：loading（取码中）→ waiting（待扫码）→ scanned（已扫待确认）→ success
        / expired（连续换码仍过期）/ error（重试后仍失败） */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type CloudQrPhase = "idle" | "loading" | "waiting" | "scanned" | "success" | "expired" | "error";

type StartResp = {
  qrcode: string;
  state?: string;
  hint?: string;
  uid?: string;
  time?: string;
  sign?: string;
  expiresAtMs?: number;
};
type PollResp = { status: number; ok: boolean; userName?: string; expired?: boolean };

const POLL_INTERVAL_MS = 2000;
const DEFAULT_TTL_MS = 5 * 60 * 1000;
/** [lc-1903] 本地 TTL 到点后的宽限窗（继续问上游，避免把「刚确认」的码刷掉） */
const QR_GRACE_MS = 90 * 1000;
/** 连续自动换码上限：防止上游异常时无限刷码 */
const MAX_AUTO_REFRESH = 3;
/** 单次轮询允许的连续失败次数（网络抖动不打断整个流程） */
const MAX_POLL_FAILURES = 2;

const sleep = (ms: number) => new Promise<void>((r) => window.setTimeout(r, ms));

/** [lc-1903] 失败分类：网络不通 / 授权换取失败 / 上游限流，文案不再一律「网络不稳定」 */
type PollFailKind = "network" | "auth" | "rate";
function classifyPollError(e: unknown): { kind: PollFailKind; detail: string } {
  const detail = String(e).replace(/^Error:\s*/, "").trim();
  const l = detail.toLowerCase();
  const rate = /429|限流|频繁|too many|rate.?limit/.test(l);
  const net = /timed? ?out|timeout|超时|dns|connect|network|网络|econn|请求失败|响应读取失败|上游 http 5\d\d/.test(l);
  if (rate) return { kind: "rate", detail };
  if (net) return { kind: "network", detail };
  return { kind: "auth", detail };
}
function failText(kind: PollFailKind, detail: string): string {
  if (kind === "network") return `查询扫码状态失败（网络）：${detail}`;
  if (kind === "rate") return `上游限流，稍后自动重试：${detail}`;
  return `查询扫码状态失败（授权）：${detail}`;
}

export function useCloudQrLogin(onSuccess: (provider: string) => void | Promise<void>) {
  const [provider, setProvider] = useState("");
  const [image, setImage] = useState("");
  const [message, setMessage] = useState("");
  const [phase, setPhase] = useState<CloudQrPhase>("idle");
  const [secondsLeft, setSecondsLeft] = useState(0);

  /** 代际号：每次 start/stop 自增，旧循环看到号变了立刻退出（不用手动清 timer） */
  const genRef = useRef(0);
  const deadlineRef = useRef(0);
  const hintRef = useRef("");
  const autorefreshRef = useRef(0);
  const aliveRef = useRef(true);
  const successRef = useRef(onSuccess);
  successRef.current = onSuccess;

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
      genRef.current += 1;
    };
  }, []);

  const stop = useCallback(() => {
    genRef.current += 1;
  }, []);

  const start = useCallback(async (prov: string, auto = false) => {
    if (!aliveRef.current) return;
    const gen = ++genRef.current;
    if (!auto) autorefreshRef.current = 0;
    setProvider(prov);
    setImage("");
    setPhase("loading");
    setMessage("正在获取二维码…");
    setSecondsLeft(0);
    try {
      let resp: StartResp;
      let poll: () => Promise<PollResp>;
      if (prov === "115") {
        const r = await invoke<StartResp>("cloud115_qr_start");
        resp = r;
        poll = () =>
          invoke<PollResp>("cloud115_qr_poll", { uid: r.uid, time: r.time, sign: r.sign });
      } else {
        const r = await invoke<StartResp>("cloud_qr_start", { provider: prov });
        resp = r;
        poll = () => invoke<PollResp>("cloud_qr_poll", { provider: prov, state: r.state });
      }
      if (!aliveRef.current || gen !== genRef.current) return;

      // 与后端 QrState 的 TTL 同源；后端没给就退回 5 分钟
      deadlineRef.current = resp.expiresAtMs ?? Date.now() + DEFAULT_TTL_MS;
      hintRef.current = prov === "115"
        ? "打开 115 生活 App（或 115 App）扫一扫登录"
        : resp.hint || "打开对应网盘 App 扫一扫登录";
      setImage(resp.qrcode);
      setPhase("waiting");
      setMessage(hintRef.current);

      // 倒计时（与后端 QrState 的 TTL 同源，只做展示；真正换码由下面循环判定）
      const tick = window.setInterval(() => {
        if (!aliveRef.current || gen !== genRef.current) return;
        setSecondsLeft(Math.max(0, Math.ceil((deadlineRef.current - Date.now()) / 1000)));
      }, 1000);
      const cleanup = () => window.clearInterval(tick);

      let failures = 0;
      // [lc-1903] 本地 TTL 到点后不立刻换码，先留一段「宽限窗」继续问上游：
      //   · 上游各家真实有效期不一（阿里实测 15 分钟），本地 5 分钟只是保守估计；
      //   · 用户常常在最后几秒才点确认，到点就换码会把「已确认的码」直接刷掉 → 「手机点了没反应」；
      //   · 真过期时上游会回 status=3（expired），下面照样立刻换码，不会白等。
      const hardDeadline = deadlineRef.current + QR_GRACE_MS;
      let graceNoted = false;
      // 串行轮询：上一轮返回后再等 2s，绝不叠请求
      while (aliveRef.current && gen === genRef.current) {
        const now = Date.now();
        if (now > hardDeadline) {
          cleanup();
          if (autorefreshRef.current < MAX_AUTO_REFRESH) {
            autorefreshRef.current += 1;
            setPhase("expired");
            setMessage("二维码已过期，正在刷新…");
            void start(prov, true);
          } else {
            setPhase("expired");
            setMessage("二维码多次过期，请点「刷新二维码」重试");
          }
          return;
        }
        const left = deadlineRef.current - now;
        const wait = left > 0 ? Math.min(POLL_INTERVAL_MS, left) : POLL_INTERVAL_MS;
        if (left <= 0 && !graceNoted) {
          graceNoted = true;
          setMessage("二维码已到展示时限，正在确认登录结果…");
        }
        await sleep(wait);
        if (!aliveRef.current || gen !== genRef.current) return;

        try {
          const p = await poll();
          if (!aliveRef.current || gen !== genRef.current) return;
          failures = 0;
          if (p.ok) {
            cleanup();
            setPhase("success");
            setMessage(p.userName ? `登录成功：${p.userName}` : "登录成功");
            setSecondsLeft(0);
            // 登录已经成功：后续刷新列表/登录态失败只影响展示，不能倒灌成「扫码失败」
            try {
              await successRef.current(prov);
            } catch {
              /* ignore */
            }
            return;
          }
          if (p.expired || p.status === 3) {
            cleanup();
            if (autorefreshRef.current < MAX_AUTO_REFRESH) {
              autorefreshRef.current += 1;
              setPhase("expired");
              setMessage("二维码已过期，正在刷新…");
              void start(prov, true);
            } else {
              setPhase("expired");
              setMessage("二维码多次过期，请点「刷新二维码」重试");
            }
            return;
          }
          if (p.status === 1) {
            setPhase("scanned");
            setMessage("已扫码，请在手机上确认登录");
          } else {
            setPhase("waiting");
            setMessage(hintRef.current);
          }
        } catch (e) {
          if (!aliveRef.current || gen !== genRef.current) return;
          failures += 1;
          const { kind, detail } = classifyPollError(e);
          if (failures > MAX_POLL_FAILURES) {
            cleanup();
            setPhase("error");
            setMessage(
              kind === "network"
                ? `${failText(kind, detail)}（检查网络/代理后点「刷新二维码」重试）`
                : kind === "rate"
                  ? `${failText(kind, detail)}（等 1 分钟后点「刷新二维码」）`
                  : `${failText(kind, detail)}（点「刷新二维码」重新扫码）`,
            );
            return;
          }
          setMessage(`${failText(kind, detail)}，重试中…（${failures}/${MAX_POLL_FAILURES}）`);
          await sleep(1000);
        }
      }
      cleanup();
    } catch (e) {
      if (!aliveRef.current || gen !== genRef.current) return;
      setPhase("error");
      setMessage(String(e));
    }
  }, []);

  return {
    provider,
    image,
    message,
    phase,
    secondsLeft,
    start,
    stop,
    /** 正在取码/轮询中（success/expired/error 即为已停） */
    busy: phase === "loading" || phase === "waiting" || phase === "scanned",
  };
}
