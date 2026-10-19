/* [library-cover] 首页媒体库卡片的封面解析 / 自定义上传。
 *
 * 默认封面规则（未上传自定义封面时）：
 *   1. 复用首页已加载的库条目（`libRows`），按 `air_date` / `release_date` 倒序取最新一条；
 *   2. 若列表条目自带横图（backdrops/backdrop/fanart…）直接用；
 *   3. 否则对该条调用 `get_item_detail`（Rust 侧有 120 天落盘缓存）取 `backdrops` 最大横图；
 *   4. 仍无横图 → 回退该条 poster；
 *   5. 连 poster 也没有 → 由调用方回退到媒体库自身封面。
 *
 * 结果带 6 小时 TTL（内存 + localStorage），避免每次首页刷新都重复请求。
 * 自定义封面由 Rust 命令 `save_library_cover` 落盘，返回 `/__cover/<file>`（走 fntvimg 协议）。
 */
import { invoke } from "@tauri-apps/api/core";
import { MediaItem } from "../types";
import { pickImgStr, pickLargestImg } from "./images";

const CACHE_KEY = "fntv-lib-default-cover-v1";
const TTL_MS = 6 * 60 * 60 * 1000;

type CacheEntry = { url: string; at: number };
type CacheMap = Record<string, CacheEntry>;

const mem = new Map<string, CacheEntry>();
/** 同一库的并发解析只保留一个 Promise：libRows 分批回填时会多次触发 effect。 */
const inflight = new Map<string, Promise<string | undefined>>();

function readStore(): CacheMap {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return {};
    const v = JSON.parse(raw);
    return v && typeof v === "object" ? (v as CacheMap) : {};
  } catch {
    return {};
  }
}

function writeStore(next: CacheMap) {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(next));
  } catch {
    /* 配额/隐私模式失败时只用内存缓存 */
  }
}

function cached(libGuid: string): string | undefined {
  const hit = mem.get(libGuid);
  if (hit && Date.now() - hit.at < TTL_MS) return hit.url;
  const stored = readStore()[libGuid];
  if (stored && Date.now() - stored.at < TTL_MS) {
    mem.set(libGuid, stored);
    return stored.url;
  }
  return undefined;
}

function remember(libGuid: string, url: string): string {
  const entry = { url, at: Date.now() };
  mem.set(libGuid, entry);
  const store = readStore();
  store[libGuid] = entry;
  writeStore(store);
  return url;
}

/** 列表条目里能直接当「横图」用的字段（不包含 poster —— poster 是竖版兜底）。 */
function backdropOf(obj: Record<string, unknown>): string | undefined {
  return (
    pickLargestImg(obj.backdrops) ||
    pickLargestImg(obj.backdrop) ||
    pickImgStr(obj, ["fanart", "landscape", "thumb"])
  );
}

function itemDate(it: MediaItem): string {
  return String(it.air_date || it.release_date || "");
}

/** 最新优先：有日期的按日期倒序，无日期的保持服务端原顺序（item/list 已是 create_time DESC）。 */
function newestFirst(items: MediaItem[]): MediaItem[] {
  return items
    .map((it, index) => ({ it, index }))
    .sort((a, b) => {
      const da = itemDate(a.it);
      const db = itemDate(b.it);
      if (da && db) {
        const c = db.localeCompare(da);
        if (c !== 0) return c;
      } else if (da) {
        return -1;
      } else if (db) {
        return 1;
      }
      return a.index - b.index;
    })
    .map((x) => x.it);
}

/**
 * 解析某个媒体库的默认封面（横图优先）。返回相对路径或完整 URL；没有可用图时返回 undefined。
 * 失败不抛异常（封面只是装饰，不应阻断首页）。
 */
export function resolveDefaultCover(
  libGuid: string,
  items: MediaItem[],
): Promise<string | undefined> {
  const running = inflight.get(libGuid);
  if (running) return running;
  const p = resolveDefaultCoverInner(libGuid, items).finally(() => {
    inflight.delete(libGuid);
  });
  inflight.set(libGuid, p);
  return p;
}

async function resolveDefaultCoverInner(
  libGuid: string,
  items: MediaItem[],
): Promise<string | undefined> {
  const hit = cached(libGuid);
  if (hit) return hit;
  if (!items.length) return undefined;

  const ordered = newestFirst(items);

  // ① 列表条目已经带横图（部分 fnOS 版本 item/list 会返回 backdrops）
  for (const it of ordered.slice(0, 10)) {
    const bd = backdropOf(it as unknown as Record<string, unknown>);
    if (bd) return remember(libGuid, bd);
  }

  // ② 对最新一条查详情拿 backdrops（Rust 侧 120 天缓存，同一条目只真正请求一次）
  const newest = ordered[0];
  try {
    const d = await invoke<Record<string, unknown>>("get_item_detail", { guid: newest.guid });
    const bd = backdropOf(d);
    if (bd) return remember(libGuid, bd);
    const poster =
      pickLargestImg(d.posters) || pickImgStr(d, ["posters", "poster"]) || newest.poster || "";
    if (poster) return remember(libGuid, poster);
  } catch {
    /* 详情失败 → 用列表 poster 兜底 */
  }

  // ③ 回退：列表条目的竖版海报（调用方还会再回退到库自身封面）
  if (newest.poster) return remember(libGuid, newest.poster);
  return undefined;
}

/** 读取全部自定义封面映射：`{ libGuid: "/__cover/<file>" }`。 */
export async function loadCustomCovers(): Promise<Record<string, string>> {
  const map = await invoke<Record<string, string>>("get_library_covers");
  return map && typeof map === "object" ? map : {};
}

/** 上传自定义封面。返回可直接用作 <img src> 的 URL（`/__cover/<file>`）。 */
export async function uploadLibraryCover(libGuid: string, file: File): Promise<string> {
  const dataUrl = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(reader.error ?? new Error("读取文件失败"));
    reader.readAsDataURL(file);
  });
  const comma = dataUrl.indexOf(",");
  if (comma < 0) throw new Error("读取文件失败");
  const dataB64 = dataUrl.slice(comma + 1);
  const r = await invoke<{ url: string }>("save_library_cover", { libId: libGuid, dataB64 });
  return r.url;
}

/** 恢复默认封面（删除该库的自定义副本）。 */
export async function resetLibraryCover(libGuid: string): Promise<void> {
  await invoke("reset_library_cover", { libId: libGuid });
}
