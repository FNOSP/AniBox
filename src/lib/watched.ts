/* [lc-1654] 本地「已看」降级标记的前端缓存。
 *
 * Rust `set_item_watched` 在飞牛接口不可用时会把 guid 写进 app_data_dir/watched.json；
 * 卡片首屏需要知道这些本地标记（飞牛列表里没有），但每个卡片各发一次 IPC 太浪费 ——
 * 这里做**进程内单例**：整页只拉一次 `watched_get_all`，卡片共享同一个 Set。
 */
import { invoke } from "@tauri-apps/api/core";

let cache: Set<string> | null = null;
let inflight: Promise<Set<string>> | null = null;

/** 拉取（并缓存）本地降级标记集合 */
export function loadWatchedOverrides(): Promise<Set<string>> {
  if (cache) return Promise.resolve(cache);
  if (!inflight) {
    inflight = invoke<string[]>("watched_get_all")
      .then((list) => new Set(Array.isArray(list) ? list : []))
      .catch(() => new Set<string>());
    inflight.then((s) => {
      cache = s;
    });
  }
  return inflight;
}

/** 同步更新缓存（本地降级写入/清除后调用） */
export function setWatchedOverride(guid: string, watched: boolean) {
  if (!cache) return;
  if (watched) cache.add(guid);
  else cache.delete(guid);
}
