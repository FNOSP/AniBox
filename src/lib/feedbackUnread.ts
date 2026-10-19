/* [lc-1701] 反馈工单「未读 / 有新回复」的本地快照与去重逻辑。
 *
 * 设计要点：
 *   · 只持久化「用户已看到的状态」，未读永远是 **当前列表 vs 已读快照** 的差集。
 *     这样轮询可以反复跑，不会把同一个事件重复标成未读；重启后仍能重新算出未读。
 *   · 快照里没有的工单（本机新提交 / 超出远端同步批次的历史工单）只做基线登记，
 *     不算未读 —— 首次运行不会把所有历史工单一次性标红。
 *   · `acked` 是「面板级已确认」：用户打开过反馈面板后侧栏角标清零，但工单行仍保留
 *     「有新回复 / 状态已更新」标记，直到真正点开该工单（markTicketSeen）。
 *   · 存储键带版本号，结构变化时可整体丢弃而不影响工单数据本身。
 */

/** feedback_list 返回项（Rust 侧本地索引 + 远端状态合并；远端缺失时 status="unknown"） */
export type FbListItem = {
  id: string;
  ts: number;
  title: string;
  status: string;
  updated: number | null;
  lastRole: string | null;
  lastTs: number | null;
};

/** 单条工单的未读标记（两者可同时成立） */
export type FbUnreadInfo = {
  /** 服务端最后一条是管理员回复，且时间晚于已读快照 */
  newReply: boolean;
  /** 状态与已读快照不同（unknown 不参与比较） */
  statusChanged: boolean;
  /** 事件签名（状态 + 最后回复时间 + 标记组合），用于面板层确认与 toast 去重 */
  sig: string;
};

/** 已读快照：用户上次看到的状态与最后回复时间 */
type SeenState = { status: string; lastTs: number | null };

type ReadStore = {
  v: 1;
  initialized: boolean;
  tickets: Record<string, SeenState>;
  /** 已在面板层确认过的工单 → 确认时的事件签名。
      存签名而不是布尔值：同一工单之后再来新回复/新状态时签名变化，角标会重新亮起。 */
  acked: Record<string, string>;
};

export const FEEDBACK_READ_KEY = "anibox-feedback-read.v1";
/** 全局轮询间隔：12 小时。面板内会话轮询仍是 20s（只在该工单打开时跑）。 */
export const FEEDBACK_POLL_MS = 12 * 60 * 60 * 1000;
/** 快照条目上限：工单索引本身最多 100 条，这里留一倍余量 */
const MAX_SNAPSHOT = 200;

const EMPTY: ReadStore = { v: 1, initialized: false, tickets: {}, acked: {} };

/** 时间戳归一到秒：服务端字段可能是秒也可能是毫秒 */
export function toSec(ts: number | null | undefined): number {
  const n = Number(ts);
  if (!Number.isFinite(n) || n <= 0) return 0;
  return n > 1e11 ? Math.floor(n / 1000) : Math.floor(n);
}

/** 状态是否「可信」：unknown = 远端未同步（网络降级），不参与状态变化比较 */
function knownStatus(s: string | null | undefined): s is string {
  return !!s && s !== "unknown";
}

function sanitize(raw: unknown): ReadStore {
  if (!raw || typeof raw !== "object") return { ...EMPTY };
  const o = raw as Partial<ReadStore>;
  const tickets: Record<string, SeenState> = {};
  if (o.tickets && typeof o.tickets === "object") {
    for (const [id, v] of Object.entries(o.tickets)) {
      if (!v || typeof v !== "object") continue;
      const t = v as Partial<SeenState>;
      if (typeof t.status !== "string") continue;
      tickets[id] = { status: t.status, lastTs: toSec(t.lastTs) || null };
    }
  }
  // 兼容早期数组形态：数组里的 id 视为「已确认过但签名未知」→ 空签名，
  // 当前事件签名非空时角标仍会重新亮起（不会永久吞掉后续更新）。
  const acked: Record<string, string> = {};
  const rawAcked = o.acked as unknown;
  if (Array.isArray(rawAcked)) {
    for (const id of rawAcked) if (typeof id === "string") acked[id] = "";
  } else if (rawAcked && typeof rawAcked === "object") {
    for (const [id, sig] of Object.entries(rawAcked)) {
      if (typeof sig === "string") acked[id] = sig;
    }
  }
  return { v: 1, initialized: !!o.initialized, tickets, acked };
}

export function loadReadStore(): ReadStore {
  try {
    const raw = localStorage.getItem(FEEDBACK_READ_KEY);
    if (!raw) return { ...EMPTY };
    return sanitize(JSON.parse(raw));
  } catch {
    // 存储损坏 / 隐私模式不可用：退回空快照，下次成功轮询时重新建立基线
    return { ...EMPTY };
  }
}

export function saveReadStore(s: ReadStore): void {
  try {
    localStorage.setItem(FEEDBACK_READ_KEY, JSON.stringify(s));
  } catch {
    /* 写失败不影响本次会话内的未读计算 */
  }
}

/** 首次运行：把当前列表整体记为已读基线，不产生任何未读 */
export function initReadStore(items: FbListItem[]): ReadStore {
  const tickets: Record<string, SeenState> = {};
  for (const it of items) {
    tickets[it.id] = { status: it.status, lastTs: toSec(it.lastTs) || null };
  }
  return { v: 1, initialized: true, tickets, acked: {} };
}

/** 把快照里还没有的工单补成基线（本机新提交、或远端批次外的旧工单），返回是否有变化 */
export function syncReadStore(store: ReadStore, items: FbListItem[]): ReadStore {
  let changed = false;
  const tickets = { ...store.tickets };
  for (const it of items) {
    if (!tickets[it.id]) {
      tickets[it.id] = { status: it.status, lastTs: toSec(it.lastTs) || null };
      changed = true;
    }
  }
  if (!changed) return store;

  // 截断：按 lastTs 保留最近的 MAX_SNAPSHOT 条，避免 localStorage 无限增长
  const ids = Object.keys(tickets);
  if (ids.length > MAX_SNAPSHOT) {
    ids
      .sort((a, b) => (tickets[b]?.lastTs ?? 0) - (tickets[a]?.lastTs ?? 0))
      .slice(MAX_SNAPSHOT)
      .forEach((id) => delete tickets[id]);
  }
  return { ...store, tickets };
}

/** 当前列表相对已读快照的未读标记 */
export function computeUnread(
  items: FbListItem[],
  store: ReadStore,
): Record<string, FbUnreadInfo> {
  const out: Record<string, FbUnreadInfo> = {};
  if (!store.initialized) return out;

  for (const it of items) {
    // 远端未同步（Rust 合并时 status=unknown，且 lastTs/lastRole 均为空）：
    // 这一轮的数据不可信，既不报新回复也不报状态变化，等下一次成功同步。
    if (!knownStatus(it.status)) continue;
    const prev = store.tickets[it.id];
    if (!prev) continue; // 没有基线 → 新工单，不算未读

    const curLast = toSec(it.lastTs);
    const prevLast = toSec(prev.lastTs);
    // 只有管理员/服务端最后发言才算「有新回复」；本机自己的回复（lastRole=user）不算
    const newReply = curLast > prevLast && it.lastRole !== "user";
    const statusChanged =
      knownStatus(prev.status) && knownStatus(it.status) && prev.status !== it.status;

    if (newReply || statusChanged) {
      const info = { newReply, statusChanged, sig: "" };
      info.sig = unreadSignature(it, info);
      out[it.id] = info;
    }
  }
  return out;
}

/** 面板打开：记录当前事件的签名（角标清零，行内标记保留） */
export function ackUnread(store: ReadStore, unread: Record<string, FbUnreadInfo>): ReadStore {
  const ids = Object.keys(unread);
  if (ids.length === 0) return store;
  const acked = { ...store.acked };
  for (const id of ids) acked[id] = unread[id].sig;
  return { ...store, acked };
}

/** 单条工单已读：快照推进到当前状态，并从 acked 中移除 */
export function markSeen(
  store: ReadStore,
  t: { id: string; status: string; lastTs: number | null },
): ReadStore {
  const tickets = {
    ...store.tickets,
    [t.id]: { status: t.status, lastTs: toSec(t.lastTs) || null },
  };
  const acked = { ...store.acked };
  delete acked[t.id];
  return { ...store, tickets, acked };
}

/** 侧栏角标数量：未读、且当前事件签名尚未在面板层确认过的工单数 */
export function unreadBadgeCount(
  unread: Record<string, FbUnreadInfo>,
  store: ReadStore,
): number {
  return Object.keys(unread).filter((id) => store.acked[id] !== unread[id].sig).length;
}

/** 事件签名：用于 toast 去重（同一工单同一事件只提醒一次） */
export function unreadSignature(it: FbListItem, u: FbUnreadInfo): string {
  return `${it.id}|${it.status}|${toSec(it.lastTs)}|${u.newReply ? "r" : ""}${
    u.statusChanged ? "s" : ""
  }`;
}
