/* [lc-2354] 画廊页 —— 侧边栏入口。
   信息结构对照 AniCh 的「插画 / COSPLAY」两个画廊 tab（`home/views/artwork.dart`、
   `cosplay.dart`：瀑布流图片墙 + 点击看大图），但**数据源换了**：它的社区帖子流后端已死，
   这里分两个 tab 取真实可用的源：
     · 图库 = safebooru 公开 API（零认证、分页正常、有缩略图/原图/宽高/tag）
     · 本地 = 扫本机图片缓存 + 用户指定目录（只读）
   图片显示：外网图直连（https，imgUrl 直通）；本地图走 fntvimg 的 /__gallery/ 分支
   （绝对不能直接给 <img src="file://...">：tauri:// 源下读不了本地文件）。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Image as IcoImageL,
  FolderOpen as IcoFolderL,
  Heart as IcoHeartL,
  Search as IcoSearchL,
  X as IcoXL,
} from "lucide-react";
import { imgUrl } from "../../lib/images";

/** safebooru 条目（后端 map_post 已裁掉原始 24 字段里的冗余项） */
type OnlineItem = { id: number; w: number; h: number; thumb: string; full: string; tags: string; score: number };
/** 本地条目（后端 gallery_local 产出） */
type LocalItem = { name: string; path: string; size: number; mtime: number };

type OnlineResp = { items: OnlineItem[]; page: number; hasMore: boolean };
type LocalResp = { items: LocalItem[]; dirsScanned: string[]; truncated: boolean; max: number };

type Tab = "online" | "local";

/** 本地图片转 fntvimg URL —— 走 /__gallery/ 分支（后端 percent_decode 还原绝对路径）。 */
function localImgUrl(p: string): string {
  const enc = encodeURIComponent(p);
  return navigator.userAgent.includes("Windows")
    ? `http://fntvimg.localhost/__gallery/${enc}`
    : `fntvimg://localhost/__gallery/${enc}`;
}

/** 瀑布流分列：按累积高度最短优先（等比排版，不裁剪图片）。 */
function splitColumns<T>(items: T[], cols: number, ratioOf: (it: T) => number): T[][] {
  const buckets: T[][] = Array.from({ length: cols }, () => []);
  const heights = new Array(cols).fill(0);
  for (const it of items) {
    let min = 0;
    for (let i = 1; i < cols; i++) if (heights[i] < heights[min]) min = i;
    buckets[min].push(it);
    // 未知宽高按 3:4 估（竖图常见），保证后续分配不会全砸在一列
    heights[min] += 1 / Math.max(0.2, ratioOf(it));
  }
  return buckets;
}

export function GalleryPage({ onBack }: { onBack: () => void }) {
  const [tab, setTab] = useState<Tab>("online");

  /* ── 图库（外网）── */
  const [onlineItems, setOnlineItems] = useState<OnlineItem[]>([]);
  const [onlinePage, setOnlinePage] = useState(0);
  const [onlineMore, setOnlineMore] = useState(true);
  const [onlineBusy, setOnlineBusy] = useState(false);
  const [onlineErr, setOnlineErr] = useState("");
  const [tagQuery, setTagQuery] = useState("");
  const [appliedTag, setAppliedTag] = useState("");

  /* ── 本地 ── */
  const [localItems, setLocalItems] = useState<LocalItem[]>([]);
  const [localDirs, setLocalDirs] = useState<string[]>([]);
  const [localTrunc, setLocalTrunc] = useState(false);
  const [localBusy, setLocalBusy] = useState(false);
  const [localErr, setLocalErr] = useState("");
  const [localDir, setLocalDir] = useState("");

  /* ── 大图预览 ── */
  const [preview, setPreview] = useState<{ src: string; label: string; tags?: string } | null>(null);

  const loadOnline = useCallback(async (page: number, tags: string, append: boolean) => {
    setOnlineBusy(true);
    setOnlineErr("");
    try {
      const r = await invoke<OnlineResp>("gallery_online", { page, tags: tags || null });
      setOnlineItems((prev) => (append ? [...prev, ...r.items] : r.items));
      setOnlinePage(r.page);
      setOnlineMore(r.hasMore);
    } catch (e) {
      setOnlineErr(String(e));
    } finally {
      setOnlineBusy(false);
    }
  }, []);

  const loadLocal = useCallback(async (dir: string) => {
    setLocalBusy(true);
    setLocalErr("");
    try {
      const r = await invoke<LocalResp>("gallery_local", { dir: dir || null });
      setLocalItems(r.items);
      setLocalDirs(r.dirsScanned ?? []);
      setLocalTrunc(!!r.truncated);
    } catch (e) {
      setLocalErr(String(e));
    } finally {
      setLocalBusy(false);
    }
  }, []);

  // 首次进入按当前 tab 拉一次；tab 切换时若还没数据也拉
  const inited = useRef<{ online: boolean; local: boolean }>({ online: false, local: false });
  useEffect(() => {
    if (tab === "online" && !inited.current.online) {
      inited.current.online = true;
      void loadOnline(0, "", false);
    }
    if (tab === "local" && !inited.current.local) {
      inited.current.local = true;
      void loadLocal("");
    }
  }, [tab, loadOnline, loadLocal]);

  // Esc 关预览
  useEffect(() => {
    if (!preview) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setPreview(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [preview]);

  const applyTag = () => {
    const t = tagQuery.trim();
    setAppliedTag(t);
    void loadOnline(0, t, false);
  };

  return (
    <main className="content lib-root gal-page">
      <div className="page-head">
        <button className="back" onClick={onBack}>‹</button>
        <h3>画廊</h3>
        <span className="muted">
          {tab === "online" ? `${onlineItems.length} 张` : `${localItems.length} 张`}
        </span>
      </div>

      {/* 分节胶囊：与 JavHub / 媒体库页同一套 libf-bar + libf-open 原生类 */}
      <div className="libf-bar">
        <button
          type="button"
          title="图库"
          className={`libf-open${tab === "online" ? " on" : ""}`}
          onClick={() => setTab("online")}
        >
          <IcoImageL size={13} strokeWidth={2} />
          图库
        </button>
        <button
          type="button"
          title="本地"
          className={`libf-open${tab === "local" ? " on" : ""}`}
          onClick={() => setTab("local")}
        >
          <IcoFolderL size={13} strokeWidth={2} />
          本地
        </button>
      </div>

      {tab === "online" && (
        <>
          <div className="gal-search">
            <IcoSearchL size={14} strokeWidth={2} className="gal-search-ico" />
            <input
              className="set-input gal-search-in"
              placeholder="按标签筛选，空格分隔多个（如 touhou vocaloid）"
              value={tagQuery}
              onChange={(e) => setTagQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") applyTag();
              }}
            />
            {tagQuery && (
              <button type="button" className="gal-search-clear" onClick={() => { setTagQuery(""); setAppliedTag(""); void loadOnline(0, "", false); }} aria-label="清除">
                <IcoXL size={13} strokeWidth={2} />
              </button>
            )}
          </div>
          {onlineErr && <div className="gal-err">加载失败：{onlineErr}</div>}
          {onlineItems.length > 0 && (
            <div className="gal-grid">
              {splitColumns(onlineItems, 4, (it) => (it.w && it.h ? it.w / it.h : 0.75)).map((col, ci) => (
                <div className="gal-col" key={ci}>
                  {col.map((it) => (
                    <button
                      type="button"
                      key={it.id}
                      className="gal-card"
                      onClick={() => setPreview({ src: imgUrl(it.full), label: `#${it.id}`, tags: it.tags })}
                    >
                      <img src={imgUrl(it.thumb)} alt="" loading="lazy" draggable={false} />
                      {it.score > 0 && (
                        <span className="gal-score">
                          <IcoHeartL size={11} strokeWidth={2} />
                          {it.score}
                        </span>
                      )}
                    </button>
                  ))}
                </div>
              ))}
            </div>
          )}
          {onlineBusy && <div className="gal-more">加载中…</div>}
          {!onlineBusy && onlineItems.length === 0 && !onlineErr && (
            <div className="gal-empty">{appliedTag ? `没有匹配「${appliedTag}」的图片` : "暂无图片"}</div>
          )}
          {!onlineBusy && onlineMore && onlineItems.length > 0 && (
            <button type="button" className="gal-more-btn" onClick={() => void loadOnline(onlinePage + 1, appliedTag, true)}>
              加载更多
            </button>
          )}
        </>
      )}

      {tab === "local" && (
        <>
          <div className="gal-search">
            <IcoFolderL size={14} strokeWidth={2} className="gal-search-ico" />
            <input
              className="set-input gal-search-in"
              placeholder="额外扫描的目录（绝对路径，留空只扫应用图片缓存）"
              value={localDir}
              onChange={(e) => setLocalDir(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void loadLocal(localDir.trim());
              }}
            />
            <button type="button" className="gal-search-btn" onClick={() => void loadLocal(localDir.trim())}>
              扫描
            </button>
          </div>
          {localDirs.length > 0 && (
            <div className="gal-dirs">
              扫描了 {localDirs.length} 个目录{localTrunc ? "（已达上限，只显示前 500 张）" : ""}
            </div>
          )}
          {localErr && <div className="gal-err">加载失败：{localErr}</div>}
          {localItems.length > 0 && (
            <div className="gal-grid">
              {splitColumns(localItems, 4, () => 0.75).map((col, ci) => (
                <div className="gal-col" key={ci}>
                  {col.map((it) => (
                    <button
                      type="button"
                      key={it.path}
                      className="gal-card"
                      title={it.name}
                      onClick={() => setPreview({ src: localImgUrl(it.path), label: it.name })}
                    >
                      <img src={localImgUrl(it.path)} alt="" loading="lazy" draggable={false} />
                    </button>
                  ))}
                </div>
              ))}
            </div>
          )}
          {localBusy && <div className="gal-more">扫描中…</div>}
          {!localBusy && localItems.length === 0 && !localErr && (
            <div className="gal-empty">没有找到图片。填一个目录路径再扫描试试。</div>
          )}
        </>
      )}

      {preview && (
        <div className="gal-lightbox" onClick={() => setPreview(null)} role="dialog" aria-label="图片预览">
          <button type="button" className="gal-lightbox-x" onClick={() => setPreview(null)} aria-label="关闭">
            <IcoXL size={18} strokeWidth={2} />
          </button>
          <img
            src={preview.src}
            alt={preview.label}
            onClick={(e) => e.stopPropagation()}
            draggable={false}
          />
          <div className="gal-lightbox-cap">
            {preview.label}
            {preview.tags ? ` · ${preview.tags.slice(0, 80)}${preview.tags.length > 80 ? "…" : ""}` : ""}
          </div>
        </div>
      )}
    </main>
  );
}
