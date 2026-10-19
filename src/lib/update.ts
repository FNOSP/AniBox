/* [lc-1710] 更新检查结果的归一化：设置面板与启动更新弹窗共用。
   后端字段可能是 camelCase 或 snake_case（并行开发期两版都可能出现），统一归一。 */
import type { UpdateCheckResult, UpdateType } from "../types";

export const asText = (v: unknown): string => {
  if (Array.isArray(v)) return v.map((x) => String(x)).join("\n");
  return v === null || v === undefined ? "" : String(v);
};

/** 把 `update_check` 的原始返回归一成前端契约结构。 */
export function normalizeCheck(raw: unknown): UpdateCheckResult {
  const r = (raw ?? {}) as Record<string, unknown>;
  const type = asText(r.updateType ?? r.update_type).toLowerCase();
  return {
    hasUpdate: Boolean(r.hasUpdate ?? r.has_update ?? false),
    currentVersion: asText(r.currentVersion ?? r.current_version),
    latestVersion: asText(r.latestVersion ?? r.latest_version),
    updateType: (type === "hotfix" ? "hotfix" : "full") as UpdateType,
    notes: asText(r.notes ?? r.changelog ?? r.body),
    publishedAt: asText(r.publishedAt ?? r.published_at),
    downloadUrl: asText(r.downloadUrl ?? r.download_url),
    patchUrl: asText(r.patchUrl ?? r.patch_url),
    htmlUrl: asText(r.htmlUrl ?? r.html_url),
    platform: asText(r.platform),
    arch: asText(r.arch),
    source: asText(r.source),
    cooldownRemaining: Number(r.cooldownRemaining ?? r.cooldown_remaining ?? 0) || 0,
    fromCache: Boolean(r.fromCache ?? r.from_cache ?? false),
  };
}

/** 下载速度显示（字节/秒 → 人类可读）。0 或缺省时返回空串。 */
export function formatSpeed(bps: number | undefined | null): string {
  const v = Number(bps);
  if (!Number.isFinite(v) || v <= 0) return "";
  if (v >= 1024 * 1024) return `${(v / (1024 * 1024)).toFixed(1)} MB/s`;
  if (v >= 1024) return `${(v / 1024).toFixed(0)} KB/s`;
  return `${Math.round(v)} B/s`;
}

/** 版本号统一显示成带 v 前缀的形式（后端通常已剥掉 v）。 */
export function versionLabel(v: string): string {
  const s = v.trim();
  if (!s) return "";
  return /^v/i.test(s) ? s : `v${s}`;
}
