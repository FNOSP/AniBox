/** [lc-2417] 详情页「图片偏好」—— 照 WWPlayer 1.3.20 的
 *  `dist/assets/detailArtwork-CnbqF2uh.js` 复刻。
 *
 * 源实现（逐条对照）：
 * ```js
 * let h = ["poster", "backdrop", "logo"];            // 三个角色，顺序固定
 * function u(e) {                                     // owner 串
 *   let t = e.mediaType === "episode" && e.seriesId;
 *   return JSON.stringify([e.sourceServerId || "metadata",
 *     e.localLibraryId || e.moduleRef?.moduleId || "",
 *     t ? "tv" : e.mediaType,
 *     t || e.embyId || e.id]);
 * }
 * function d(e, t = [], n = e.artwork) { ... }        // 候选池归一化+去重
 * function f(e, t) {                                  // 偏好覆盖
 *   return { poster: t.poster || e.detailPoster || e.poster, ... };
 * }
 * ```
 *
 * 与源实现的差异（本项目没有 sourceServerId / emby / moduleRef 这些概念）：
 *   · owner 串改成 `["tmdb", mediaType, tmdbId]` —— 稳定、可复现、跨季不串。
 *   · 候选池直接取自 `tmdb_show_details` 的 `imagePools`（后端已按面积降序排好）。
 */
import { invoke } from "@tauri-apps/api/core";

/** 三个角色 —— 与后端 `detail_artwork::ROLES` 及源实现的 `["poster","backdrop","logo"]` 逐字一致。
 *  ⚠ 不含 "art"：「艺术图」是区块标题（源 i18n `detail.artwork`），不是分类。 */
export const ARTWORK_ROLES = ["poster", "backdrop", "logo"] as const;
export type ArtworkRole = (typeof ARTWORK_ROLES)[number];

/** 角色 → 界面标签。取自源 i18n：
 *  `detail.art.poster` = 海报 / `detail.art.backdrop` = 剧照 / `detail.art.logo` = Logo */
export const ROLE_LABEL: Record<ArtworkRole, string> = {
  poster: "海报",
  backdrop: "剧照",
  logo: "Logo",
};

/** owner 串：本项目用 TMDB 身份（源实现用 sourceServerId/libraryId/embyId 组合）。
 *  形如 `["tmdb","tv",206630]` —— 同一部剧的所有集共用同一个 owner（源实现里
 *  剧集也用 seriesId 而不是单集 id，语义相同）。 */
export function artworkOwner(tmdbId: number | string, mediaType: "tv" | "movie"): string {
  return JSON.stringify(["tmdb", mediaType, String(tmdbId ?? "")]);
}

/** 已存偏好：role → 本地文件绝对路径（后端给的是路径，这里拼成协议 URL）。 */
export type ArtworkPicks = Partial<Record<ArtworkRole, string>>;

/** 本地偏好文件 → 可放进 <img src> 的 URL。
 *  协议：`artwork://preferred/<sha256(owner)>/<role>?v=<mtime>`
 *  与源实现的 `ww-image://preferred-artwork/<id>/<role>?v=mtimeNs` 同构。 */
export function artworkUrl(localPath: string, mtimeSec?: number): string {
  const p = String(localPath ?? "").trim();
  if (!p) return "";
  // 路径形如 .../detail-artwork/<64hex>/<role>.webp
  const m = /([a-f0-9]{64})[\\/](poster|backdrop|logo)\.webp$/i.exec(p);
  if (!m) return "";
  const v = mtimeSec ?? 0;
  return `artwork://preferred/${m[1].toLowerCase()}/${m[2].toLowerCase()}${v ? `?v=${v}` : ""}`;
}

/** 读已存偏好（对应源 `window.wwPlayer.readDetailArtwork`）。 */
export async function readDetailArtwork(owner: string): Promise<ArtworkPicks> {
  const raw = await invoke<Record<string, string>>("read_detail_artwork", { owner });
  const out: ArtworkPicks = {};
  for (const role of ARTWORK_ROLES) {
    const p = raw?.[role];
    if (p) out[role] = p;
  }
  return out;
}

/** 设置 / 清除偏好（对应源 `window.wwPlayer.setDetailArtwork({owner,role,src})`）。
 *  `src === null` → 恢复默认（删本地文件），返回空串。 */
export async function setDetailArtwork(
  owner: string,
  role: ArtworkRole,
  src: string | null,
): Promise<string> {
  return invoke<string>("set_detail_artwork", { owner, role, src });
}
