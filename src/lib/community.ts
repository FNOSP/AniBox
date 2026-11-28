/* [lc-2402] 社区 v2 前端数据层：类型 + invoke 封装。
 *
 * 安全口径（与 Rust community_auth.rs 一致）：会话 token 只在 Rust 侧持有与使用，
 * 前端拿到的是**脱敏后的会话**（accountId / 打码邮箱 / 昵称 / 角色），绝无 token。
 * 所有网络调用都走 Rust 命令 —— 本文件里没有任何 fetch。 */

import { invoke } from "@tauri-apps/api/core";
import { imgUrl } from "./images";

export type CommunityRole = "user" | "admin";

/** community_v2_session 返回的脱敏会话（token 不在其中） */
export type CommunitySession = {
  accountId: string;
  /** 已打码：us***@qq.com */
  email: string;
  nick: string;
  nickSuffix: string;
  role: CommunityRole;
  deviceId: string;
};

/** community_v2_me 返回（个人中心：含 stats 与封禁态） */
export type CommunityMe = {
  id: string;
  email: string;
  nick: string;
  nickSuffix: string;
  role: CommunityRole;
  status: string;
  banned?: boolean;
  createdAt: number;
  stats: { posts: number; comments: number };
};

/** community_v2_register 返回 */
export type CommunityPendingReg = {
  pendingId: string;
  maskedEmail: string;
  cooldownSec: number;
};

/** community_v2_devices 的单行（服务端产出，current 由服务端比对 token 判定） */
export type CommunityDevice = {
  id: string;
  os: string;
  screen: string;
  scale: number;
  ipNet: string;
  deviceCode: string;
  createdAt: number;
  lastSeenAt: number;
  current: boolean;
};

/** 显示名：重名时用 4 位后缀区分（小明 @a3f2） */
export function communityTagOf(s: { nick: string; nickSuffix?: string }): string {
  return s.nickSuffix ? `${s.nick} @${s.nickSuffix}` : s.nick;
}

/** 设备码 → 「标识」栏的 8 位大写（050C9DD3） */
export function deviceTag(deviceCode: string): string {
  return (deviceCode || "").slice(0, 8).toUpperCase();
}

/* ── 命令封装 ─────────────────────────────── */

export async function fetchSession(): Promise<CommunitySession | null> {
  try {
    const s = await invoke<CommunitySession | null>("community_v2_session");
    return s ?? null;
  } catch {
    return null;
  }
}

export const registerAccount = (email: string, password: string, nick: string) =>
  invoke<CommunityPendingReg>("community_v2_register", { email, password, nick });

export const resendCode = (pendingId: string) =>
  invoke<{ cooldownSec: number }>("community_v2_resend", { pendingId });

export const verifyCode = (pendingId: string, code: string) =>
  invoke<CommunitySession>("community_v2_verify", { pendingId, code });

export const loginAccount = (email: string, password: string) =>
  invoke<CommunitySession>("community_v2_login", { email, password });

export const logoutAccount = () => invoke<{ ok: boolean; remoteRevoked: boolean }>("community_v2_logout");

export const changeNick = (nick: string) => invoke<CommunitySession>("community_v2_set_nick", { nick });

export const fetchDevices = () => invoke<CommunityDevice[]>("community_v2_devices");

export const revokeDevice = (id: string) => invoke("community_v2_revoke_device", { id });

export const revokeOtherDevices = () => invoke("community_v2_revoke_others");

/* ── 图片（P2，lc-2442）────────────────────────────── */

/** community_prepare_image 返回（压缩完成后暂存于 Rust 侧，凭 preparedId 上传） */
export type PreparedImage = {
  preparedId: string;
  originalBytes: number;
  finalBytes: number;
  w: number;
  h: number;
  /** 原始格式名 jpeg|gif|heic|png|webp|bmp|tiff */
  fmt: string;
  /** data:image/jpeg;base64 预览（400px） */
  preview: string;
};

export const prepareImage = (path: string) => invoke<PreparedImage>("community_prepare_image", { path });

export type UploadedImage = {
  id: string;
  url: string | null;
  thumbUrl: string | null;
  w: number;
  h: number;
  bytes: number;
  fmt: string;
};

export const uploadImage = (preparedId: string, scope: "post" | "feedback") =>
  invoke<UploadedImage>("community_upload_image", { preparedId, scope });

export const discardPrepared = (preparedId: string) =>
  invoke("community_discard_prepared", { preparedId });

/** 私有图（反馈图）→ data URL（<img src> 直显；需 owner/管理员） */
export const feedbackImageData = (id: string) =>
  invoke<{ dataUrl: string; ct: string }>("community_feedback_image_data", { id });

/* ── v2 反馈（P2，lc-2442）────────────────────────── */

/** feedback_mine_v2 的单行 */
export type Fb2Item = {
  id: string;
  title: string;
  status: string;
  createdAt: number;
  updatedAt: number;
  imageCount: number;
};

/* ── 内容（P3，lc-2445）────────────────────────────── */

/** 帖子附图元数据（fullKey/thumbKey 为 null = 云端已过期 → 前端显示「配图已过期」） */
export type CommunityPostImage = {
  id: string;
  fullKey: string | null;
  thumbKey: string | null;
  fmt: string;
  w: number;
  h: number;
};

export type CommunityPost = {
  id: string;
  nick: string;
  /** 板块：talk=交流讨论（绑作品）| feedback=反馈建议（软件建议公开帖） */
  board: "talk" | "feedback";
  mediaType: "movie" | "tv";
  tmdbId: number;
  workTitle: string;
  score: number | null;
  title: string;
  body: string | null;
  deleted: boolean;
  retracted?: boolean;
  mine: boolean;
  imageIds: string[];
  images?: CommunityPostImage[];
  createdAt: number;
  updatedAt: number;
};

/** 作品短评（v1+v2 混一张表；accountId 为 null 是 v1 设备身份老行） */
export type CommunityComment2 = {
  id: string;
  parentId: string | null;
  rootId: string | null;
  nick: string;
  body: string | null;
  deleted: boolean;
  createdAt: number;
  mine: boolean;
  accountId: string | null;
};

export type CommunityWorkAgg = {
  score: { avg: number; count: number; dist: Record<string, number> } | null;
  posts: CommunityPost[];
  comments: CommunityComment2[];
};

export type CommunityPostDetail = {
  post: CommunityPost;
  workScore: { avg: number; count: number } | null;
  comments: CommunityComment2[];
};

export type CommunityNotification = {
  id: number;
  kind: string;
  refKind: string;
  refId: string;
  mediaType: string;
  tmdbId: number;
  actorNick: string;
  excerpt: string;
  createdAt: number;
  readAt: number | null;
};

/** 帖子图 → fntvimg 本地缓存代理 URL（/__cm/ 前缀，落 disk_cache，云端 30 天删后本地仍可看） */
export function communityImageUrl(key: string): string {
  if (!key) return "";
  return imgUrl(`/__cm/${key}`);
}

/** UUID v4（幂等键；worker 按它去重，网络重试不产生两条） */
export function newRequestId(): string {
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

export const createPost = (
  args: {
    board: "talk" | "feedback";
    mediaType?: "movie" | "tv";
    tmdbId?: number;
    workTitle?: string;
    score?: number | null;
    title?: string;
    body: string;
    imageIds?: string[];
  },
) =>
  invoke<{ post: CommunityPost; replayed: boolean }>("community_v2_post", {
    board: args.board,
    mediaType: args.mediaType ?? "tv",
    tmdbId: args.tmdbId ?? 0,
    workTitle: args.workTitle ?? "",
    score: args.score ?? null,
    title: args.title ?? "",
    body: args.body,
    imageIds: args.imageIds ?? [],
    clientRequestId: newRequestId(),
  });

export const fetchFeed = (sort: "new" | "hot", board: "talk" | "feedback" | null, cursor: number = 0) =>
  invoke<{ items: CommunityPost[]; nextCursor: number | null }>("community_v2_feed", { sort, board, cursor });

export const fetchWorkAgg = (mediaType: string, tmdbId: number) =>
  invoke<CommunityWorkAgg>("community_v2_work", { mediaType, tmdbId });

export const fetchPostDetail = (id: string) =>
  invoke<CommunityPostDetail>("community_v2_post_detail", { id });

export const deletePost = (id: string) => invoke("community_v2_delete_post", { id });

export const postComment = (args: {
  mediaType: string;
  tmdbId: number;
  workTitle: string;
  body: string;
  parentId?: string | null;
}) =>
  invoke<{ comment: CommunityComment2; replayed: boolean }>("community_v2_comment", {
    mediaType: args.mediaType,
    tmdbId: args.tmdbId,
    workTitle: args.workTitle,
    body: args.body,
    clientRequestId: newRequestId(),
    parentId: args.parentId ?? null,
  });

export const fetchMinePosts = (board: "talk" | "feedback" | null, cursor: number = 0) =>
  invoke<{ items: CommunityPost[]; nextCursor: number | null }>("community_v2_mine_posts", { board, cursor });

export const fetchMe = () => invoke<CommunityMe>("community_v2_me");

export const fetchNotifications = (cursor: number = 0) =>
  invoke<{ items: CommunityNotification[]; unread: number; nextCursor: number | null }>(
    "community_v2_notifications",
    { cursor },
  );

export const readNotifications = (args: { ids?: number[]; all?: boolean }) =>
  invoke("community_v2_read_notifications", { ids: args.ids, all: args.all });

/* ── P4：举报 + 管理（lc-2446）──────────────────────────── */

export const REPORT_REASONS = [
  { key: "spam", label: "垃圾广告" },
  { key: "abuse", label: "辱骂攻击" },
  { key: "porn", label: "色情低俗" },
  { key: "illegal", label: "违法信息" },
  { key: "other", label: "其他" },
] as const;

export type CommunityReport = {
  id: string;
  kind: string;
  refId: string;
  reason: string;
  detail: string | null;
  reporterId: string;
  status: string;
  excerpt: string | null;
  createdAt: number;
  handleNote: string | null;
};

export type CommunityAuditRow = {
  id: number;
  ts: number;
  actorId: string | null;
  actorNick: string | null;
  actorDev: string | null;
  ipNet: string;
  action: string;
  targetKind: string | null;
  targetId: string | null;
  bodySnap: string | null;
  detail: string | null;
};

export type CommunityStats = {
  users: number;
  posts: number;
  comments: number;
  images: number;
  openReports: number;
  storageUsed: number;
  storageCap: number;
};

export const submitReport = (args: { kind: string; refId: string; reason: string; detail?: string }) =>
  invoke<{ id: string; autoHidden: boolean }>("community_v2_report", {
    kind: args.kind,
    refId: args.refId,
    reason: args.reason,
    detail: args.detail ?? "",
  });

export const adminReports = (status: string, cursor: number = 0) =>
  invoke<{ items: CommunityReport[]; nextCursor: number | null }>("community_admin_reports", { status, cursor });

export const adminReportHandle = (id: string, action: "resolve" | "reject", note?: string) =>
  invoke<{ id: string; status: string }>("community_admin_report_handle", { id, action, note: note ?? "" });

export const adminRetractPost = (id: string, reason?: string) =>
  invoke<{ id: string; retracted: boolean }>("community_admin_retract_post", { id, reason: reason ?? "" });

export const adminUnretractPost = (id: string) =>
  invoke<{ id: string; retracted: boolean }>("community_admin_unretract_post", { id });

export const adminDeletePost = (id: string, reason?: string) =>
  invoke<{ id: string; deleted: boolean }>("community_admin_delete_post", { id, reason: reason ?? "" });

export const adminRetractComment = (id: string, reason?: string) =>
  invoke("community_admin_retract_comment", { id, reason: reason ?? "" });

export const adminDeleteComment = (id: string, reason?: string) =>
  invoke("community_admin_delete_comment", { id, reason: reason ?? "" });

export const adminDeleteImage = (id: string, reason?: string) =>
  invoke("community_admin_delete_image", { id, reason: reason ?? "" });

export const adminBanAccount = (id: string, reason?: string) =>
  invoke<{ id: string; status: string }>("community_admin_ban_account", { id, reason: reason ?? "" });

export const adminUnbanAccount = (id: string) =>
  invoke<{ id: string; status: string }>("community_admin_unban_account", { id });

export const adminMuteAccount = (id: string, minutes: number, reason?: string) =>
  invoke<{ id: string; status: string }>("community_admin_mute_account", { id, minutes, reason: reason ?? "" });

export const adminBanDevice = (deviceCode: string, reason?: string) =>
  invoke<{ deviceCode: string; banned: boolean }>("community_admin_ban_device", { deviceCode, reason: reason ?? "" });

export const adminAudit = (args: { actorId?: string; deviceCode?: string; action?: string; cursor?: number }) =>
  invoke<{ items: CommunityAuditRow[]; total: number; nextCursor: number | null }>("community_admin_audit", {
    actorId: args.actorId,
    deviceCode: args.deviceCode,
    action: args.action,
    cursor: args.cursor ?? 0,
  });

export const adminStats = () => invoke<CommunityStats>("community_admin_stats");
