/* [lc-1993] 错误归类 —— 把「用户看得懂的内容」和「内部报文」分开。
 *
 * 背景：飞牛会话过期时回的是 **HTTP 200 + code=-2 + msg="Auth Expired"**，
 * 旧实现把原始报文一路透传到首页 `err` 状态，顶栏就出现一条
 * `HTTP 200 code=-2: Auth Expired`。现在约定：
 *   · 归类只认三类：会话失效（auth-expired）/ 网络类（network）/ 其它（other）；
 *   · 会话失效 → 不展示原文，交 `notifyAuthExpired()` 走「重登/跳登录」；
 *   · 网络类 → 翻成人话展示（NAS 离线/网络不可达本来就该让用户知道）；
 *   · 其它 → 原样展示（业务错误细节对用户有用，不能吞）。
 * 本文件是纯逻辑 + 一个进程内去重分发器，不依赖 Tauri/React，便于单测。
 */

export type ErrorKind = "auth-expired" | "network" | "other";

/** 会话失效时展示给用户的那一句话（登录页/提示条共用）。 */
export const AUTH_EXPIRED_TEXT = "登录已过期，请重新登录";

function textOf(raw: unknown): string {
  if (typeof raw === "string") return raw;
  if (raw instanceof Error) return raw.message;
  if (raw && typeof raw === "object") {
    const m = (raw as { message?: unknown }).message;
    if (typeof m === "string") return m;
    const e = (raw as { error?: unknown }).error;
    if (typeof e === "string") return e;
  }
  return raw == null ? "" : String(raw);
}

/* 会话失效的写法来源：
 *   · 新版 Rust 请求层（fnapi::ERR_AUTH_EXPIRED）→「登录已过期，请重新登录」
 *   · 旧版/其它链路透传的原始报文 → `HTTP 200 code=-2: Auth Expired`
 *   · HTTP 401 / 未登录 也是同一语义 */
const AUTH_PATTERNS: RegExp[] = [
  /\[auth-expired\]/i,
  /code\s*=\s*-2\b/,
  /auth\s*expired/i,
  /登录已过期/,
  /会话已过期/,
  /会话过期/,
  /未登录/,
  /\bHTTP\s*401\b/i,
  /unauthorized/i,
];

/* 网络类：这些是用户**需要**知道的（NAS 离线 / 网络不可达），展示但翻成人话。 */
const NETWORK_PATTERNS: RegExp[] = [
  /无法连接服务器/,
  /连接超时/,
  /网络错误/,
  /网络不可达/,
  /服务器地址格式无效/,
  /访问码会话重建失败/,
  /failed to fetch/i,
  /network\s*error/i,
  /econnrefused|etimedout|enotfound|dns\s*error/i,
];

export function classifyError(raw: unknown): ErrorKind {
  const text = textOf(raw).trim();
  if (!text) return "other";
  if (AUTH_PATTERNS.some((re) => re.test(text))) return "auth-expired";
  if (NETWORK_PATTERNS.some((re) => re.test(text))) return "network";
  return "other";
}

export function isAuthExpired(raw: unknown): boolean {
  return classifyError(raw) === "auth-expired";
}

function friendlyNetwork(text: string): string {
  if (/服务器地址格式无效/.test(text)) return "服务器地址格式无效，请检查填写";
  if (/访问码会话重建失败/.test(text)) return "访问码会话重建失败，请在登录页重新填写访问码";
  if (/无法连接服务器|econnrefused|connection refused/i.test(text)) {
    return "无法连接服务器，请确认 NAS 地址与网络连接";
  }
  if (/连接超时|timeout|etimedout/i.test(text)) return "连接超时，网络不可达或 NAS 未响应";
  return "网络连接异常，请检查网络后重试";
}

/** 转成可展示文案：会话失效返回统一提示（调用方也可据 [`isAuthExpired`] 不再展示）。 */
export function userFacingError(raw: unknown): string {
  const text = textOf(raw).trim();
  if (!text) return "操作失败，请稍后重试";
  switch (classifyError(text)) {
    case "auth-expired":
      return AUTH_EXPIRED_TEXT;
    case "network":
      return friendlyNetwork(text);
    default:
      return text;
  }
}

/* ───────────── 会话失效分发器：并发去重 ─────────────
 * 首页同时发 5 个请求（媒体库/继续看/轮播/统计/收藏），token 过期时可能多个
 * 同时报错。这里保证**同一轮失效只触发一次**处理（跳登录/提示不会连弹）；
 * 处理完成后 5s 冷却，避免登录页上的残余请求把流程反复拉回。 */
type AuthExpiredHandler = () => void | Promise<void>;

let authHandler: AuthExpiredHandler | null = null;
let authInFlight = false;
let authCooldownUntil = 0;
const AUTH_COOLDOWN_MS = 5000;

export function setAuthExpiredHandler(handler: AuthExpiredHandler): () => void {
  authHandler = handler;
  return () => {
    if (authHandler === handler) authHandler = null;
  };
}

/** 触发一次会话失效处理；本次被去重（已在处理/冷却中/未注册）时返回 false。 */
export function notifyAuthExpired(): boolean {
  if (!authHandler) return false;
  if (authInFlight || Date.now() < authCooldownUntil) return false;
  authInFlight = true;
  Promise.resolve()
    .then(() => authHandler?.())
    .catch(() => {
      /* 处理链路自身的失败不再上抛，避免二次报错 */
    })
    .finally(() => {
      authInFlight = false;
      authCooldownUntil = Date.now() + AUTH_COOLDOWN_MS;
    });
  return true;
}
