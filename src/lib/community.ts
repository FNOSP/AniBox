/* [lc-2402] 社区 v2 前端数据层：类型 + invoke 封装。
 *
 * 安全口径（与 Rust community_auth.rs 一致）：会话 token 只在 Rust 侧持有与使用，
 * 前端拿到的是**脱敏后的会话**（accountId / 打码邮箱 / 昵称 / 角色），绝无 token。
 * 所有网络调用都走 Rust 命令 —— 本文件里没有任何 fetch。 */

import { invoke } from "@tauri-apps/api/core";

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
