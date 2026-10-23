/* [lc-1728] 「手动获取元数据 / 手动匹配影片」两个入口共用的「应用选中结果」辅助：
   ① 本地元数据覆盖（metadata_set_override）—— 关键步骤，失败抛错由调用方在弹窗内展示；
   ② best-effort 回填飞牛（getEditDetail → saveEditDetail，标题/简介带 *_locked）——
      失败不影响本地结果，只 console.warn + 轻提示（AppToastHost），绝不阻塞/绝不抛出。
   两个弹窗的数据结构不同（TMDB/BGM 统一结果 vs scrap/search 命中），这里只约定
   覆盖命令真正需要的字段（与 Rust MetadataChoice 对齐，camelCase），多余的键会被忽略。 */
import { invoke } from "@tauri-apps/api/core";

/** 与 src-tauri/src/metadata_override.rs 的 MetadataChoice 对齐（camelCase）。 */
export type MetaChoicePayload = {
  /** "tmdb" | "bgm" | "trim_id" 等，仅要求非空 */
  source: string;
  /** TMDB id / Bangumi subject id / trim id，仅要求非空 */
  id: string;
  title: string;
  originalTitle?: string;
  year?: string;
  date?: string;
  overview?: string;
  rating?: number;
  poster?: string;
  backdrop?: string;
  seasons?: number;
  episodes?: number;
  subtitle?: string;
};

/* ───────────────────── 全局轻提示（无阻塞 toast） ───────────────────── */

const TOAST_EVENT = "anibox-app-toast";

/** 派发一条全局轻提示（App 根部的 <AppToastHost/> 负责渲染，3.5s 自动消失）。 */
export function showAppToast(text: string): void {
  if (!text) return;
  try {
    window.dispatchEvent(new CustomEvent(TOAST_EVENT, { detail: text }));
  } catch {
    /* 极端环境（事件被禁）退化为 console，绝不让提示本身炸掉主流程 */
    console.info("[metaApply] toast:", text);
  }
}

export type AppToastItem = { id: number; text: string };

/** 监听 showAppToast 的事件（供 AppToastHost 使用）。返回解绑函数。 */
export function onAppToast(cb: (text: string) => void): () => void {
  const handler = (e: Event) => cb(String((e as CustomEvent).detail ?? ""));
  window.addEventListener(TOAST_EVENT, handler);
  return () => window.removeEventListener(TOAST_EVENT, handler);
}

/* ───────────────────── 本地覆盖（关键步骤） ───────────────────── */

/** 写入本地元数据覆盖。失败时抛错（调用方必须在弹窗内展示，不允许静默）。 */
export async function writeMetadataOverride(
  seriesGuid: string,
  choice: MetaChoicePayload,
): Promise<void> {
  await invoke("metadata_set_override", {
    seriesGuid,
    choice: { ...choice, updatedAt: Date.now() },
  });
}

/* ───────────────────── best-effort 回填飞牛 ───────────────────── */

/**
 * 把标题/简介回填到飞牛（参考「保存到飞牛」链路：全量对象原样回传 + `*_locked:true`，
 * 防止下次刮削覆盖）。**尽力而为**：任一步失败只 console.warn + 轻提示，返回 false，
 * 不抛出、不阻塞本地已生效的结果。标题与简介都为空时直接跳过（无事可做）。
 */
export async function backfillFnosText(
  itemGuid: string,
  title: string,
  overview: string,
): Promise<boolean> {
  const t = String(title ?? "").trim();
  const o = String(overview ?? "").trim();
  if (!itemGuid.trim() || (!t && !o)) return true;
  try {
    const detail = await invoke<Record<string, unknown> | null>("get_edit_detail", { guid: itemGuid });
    const body: Record<string, unknown> = { ...(detail ?? {}) };
    if (t) {
      body.title = t;
      body.title_locked = true;
    }
    if (o) {
      body.overview = o;
      body.overview_locked = true;
    }
    await invoke("save_edit_detail", { payload: body });
    return true;
  } catch (e) {
    console.warn("[metaApply] 回填飞牛失败（本地覆盖已生效，不受影响）:", e);
    showAppToast("本地元数据已生效；回填飞牛失败（可稍后在「元数据」面板重试保存）");
    return false;
  }
}
