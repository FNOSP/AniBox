/* [lc-2355/2363] Pixiv 图库页 —— 侧边栏「Pixiv」入口。
 *
 * 版式对照 AniCh 1.5.27 的截图：
 *   · 顶栏：「搜索图片」输入框（横贯整行）
 *   · 页签行：Pixiv / 本地（选中项加粗 + 主题色下划线）
 *   · 图片墙：5 列等高瀑布流（卡片 192px、间距 7px，实测截图 2014x1394 @2x）
 *   · 卡片：圆角 7、底部黑色渐变压标题、右上角半透明角标
 *
 * 数据源：**只有 Pixiv**（用户明确要求去掉 safebooru 与 COSPLAY 源）。
 *   · Pixiv 排行/搜索/详情 —— 实测无需登录即可用（ranking.php / ajax/* 直连 200）
 *   · 本地 —— 本机图片扫描（离线）
 *
 * 角标：「NP」= 多图作品的张数（Pixiv 的 pageCount，**这就是原版「9P」的来源**）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Image as IcoImageL,
  FolderOpen as IcoFolderL,
  Search as IcoSearchL,
  X as IcoXL,
} from "lucide-react";

/** 本地图片条目（后端 gallery_local 产出） */
type LocalItem = { name: string; path: string; size: number; mtime: number };
type LocalResp = { items: LocalItem[]; dirsScanned: string[]; truncated: boolean; max: number };
/** [lc-2356] Pixiv 条目（后端 pixiv_ranking / pixiv_search 归一化后） */
type PixivItem = {
  id: number;
  title: string;
  user: string;
  w: number;
  h: number;
  thumb: string;
  full: string;
  tags: string[];
  /** 多图张数 —— 这就是原版「9P」角标的来源 */
  pages: number;
  rank: number;
  views: number;
  type: string;
};
type PixivResp = {
  items: PixivItem[];
  page: number;
  hasMore: boolean;
  total?: number;
};

/** 四个图流分区（对齐原版页签顺序）+ 本地 */
type Section = "pixiv" | "local";

const SECTIONS: { key: Section; label: string }[] = [
  { key: "pixiv", label: "Pixiv" },
];

const LOCAL_SECTION: { key: Section; label: string } = { key: "local", label: "本地" };

/** 本地图片转 fntvimg URL —— 走 /__gallery/ 分支（后端 percent_decode 还原绝对路径）。 */
function localImgUrl(p: string): string {
  const enc = encodeURIComponent(p);
  return navigator.userAgent.includes("Windows")
    ? `http://fntvimg.localhost/__gallery/${enc}`
    : `fntvimg://localhost/__gallery/${enc}`;
}

/** 瀑布流分列：按累积高度最短优先（等比排版，不裁剪图片，对齐原版 SliverMasonryGrid）。 */
function splitColumns<T>(items: T[], cols: number, ratioOf: (it: T) => number): T[][] {
  const buckets: T[][] = Array.from({ length: cols }, () => []);
  const heights = new Array(cols).fill(0);
  for (const it of items) {
    let min = 0;
    for (let i = 1; i < cols; i++) if (heights[i] < heights[min]) min = i;
    buckets[min].push(it);
    heights[min] += 1 / Math.max(0.2, ratioOf(it));
  }
  return buckets;
}

/** 列数按宽度算：原版是 `ceil(width/300)` 且至少 2 列；桌面端 5 列对应 ~1000 逻辑宽。 */
function columnCount(containerWidth: number): number {
  // 卡片目标宽度 192px（截图实测），列数 = 可用宽 / (卡片宽 + 间距)
  return Math.max(2, Math.floor(containerWidth / 199));
}

export function GalleryPage({ onBack }: { onBack: () => void }) {
  const [section, setSection] = useState<Section>("pixiv");

  /* ── 搜索框（顶栏）── */
  const [query, setQuery] = useState("");

  /** 已应用的搜索词（Pixiv 搜索模式；空 = 走排行） */
  const [appliedTag, setAppliedTag] = useState("");

  /* ── Pixiv 分区 ── */
  const [pxItems, setPxItems] = useState<PixivItem[]>([]);
  const [pxPage, setPxPage] = useState(1);
  const [pxMore, setPxMore] = useState(true);
  const [pxBusy, setPxBusy] = useState(false);
  const [pxErr, setPxErr] = useState("");

  /* ── 本地分区 ── */
  const [localItems, setLocalItems] = useState<LocalItem[]>([]);
  const [localDirs, setLocalDirs] = useState<string[]>([]);
  const [localTrunc, setLocalTrunc] = useState(false);
  const [localBusy, setLocalBusy] = useState(false);
  const [localErr, setLocalErr] = useState("");
  const [localDir, setLocalDir] = useState("");

  /* ── 预览 ── */
  const [preview, setPreview] = useState<{
    src: string;
    label: string;
    sub?: string;
    /** [lc-2356] 多图作品：全部图片 URL，预览里可用左右切换 */
    gallery?: string[];
  } | null>(null);
  /** 预览里的当前图索引（多图作品切换用） */
  const [previewIdx, setPreviewIdx] = useState(0);

  /* ── 列数（按容器宽自适应）── */
  const gridRef = useRef<HTMLDivElement | null>(null);
  const [cols, setCols] = useState(5);
  useEffect(() => {
    const el = gridRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      setCols(columnCount(el.clientWidth));
    });
    ro.observe(el);
    setCols(columnCount(el.clientWidth));
    return () => ro.disconnect();
  }, [section]);

  /** Pixiv 排行/搜索。`kw` 非空 → 走搜索，否则走排行。 */
  const loadPixiv = useCallback(async (p: number, kw: string, append: boolean) => {
    setPxBusy(true);
    setPxErr("");
    try {
      const r = kw
        ? await invoke<PixivResp>("pixiv_search", { keyword: kw, page: p })
        : await invoke<PixivResp>("pixiv_ranking", { page: p, section: "all", tag: null });
      setPxItems((prev) => (append ? [...prev, ...r.items] : r.items));
      setPxPage(r.page);
      setPxMore(r.hasMore);
    } catch (e) {
      setPxErr(String(e));
    } finally {
      setPxBusy(false);
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

  /* 分区/标签变化时拉第一页。用 ref 记录上次加载的组合，避免重复请求。 */
  const lastLoad = useRef<string>("");
  useEffect(() => {
    const key = `${section}|${appliedTag}`;
    if (lastLoad.current === key) return;
    lastLoad.current = key;

    if (section === "local") {
      if (localItems.length === 0) void loadLocal("");
    } else {
      void loadPixiv(1, appliedTag, false);
    }
  }, [section, appliedTag, loadLocal, loadPixiv, localItems.length]);

  // [lc-2356] 打开新预览时重置多图索引（否则从上一个作品的第 3 张开始）
  useEffect(() => {
    setPreviewIdx(0);
  }, [preview?.label, preview?.gallery]);

  // Esc 关预览
  useEffect(() => {
    if (!preview) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setPreview(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [preview]);

  /** 顶栏搜索：空词回排行，有词走 Pixiv 搜索。 */
  const submitSearch = () => {
    const q = query.trim();
    setAppliedTag(q);
    lastLoad.current = ""; // 强制重拉
    void loadPixiv(1, q, false);
  };

  const isPixiv = section === "pixiv";

  return (
    <main className="content lib-root gal-page">
      {/* ── 顶栏：搜索图片（横贯整行，对齐截图）── */}
      <div className="gal-topbar">
        <div className="gal-topbar-inner">
          <IcoSearchL size={15} strokeWidth={2} className="gal-topbar-ico" />
          <input
            className="gal-topbar-input"
            placeholder="搜索 Pixiv 作品"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submitSearch();
            }}
          />
          {query && (
            <button
              type="button"
              className="gal-topbar-clear"
              aria-label="清除"
              onClick={() => {
                setQuery("");
                setAppliedTag("");
                lastLoad.current = "";
                void loadPixiv(1, "", false);
              }}
            >
              <IcoXL size={14} strokeWidth={2} />
            </button>
          )}
        </div>
        <button className="gal-back" onClick={onBack} aria-label="返回">
          ‹
        </button>
      </div>

      {/* ── 页签行：Pixiv / 本地 ── */}
      <div className="gal-tabs" role="tablist">
        {SECTIONS.map((s) => (
          <button
            key={s.key}
            type="button"
            role="tab"
            aria-selected={section === s.key}
            className={`gal-tab${section === s.key ? " on" : ""}`}
            onClick={() => {
              setSection(s.key);
              setAppliedTag("");
            }}
          >
            {s.label}
          </button>
        ))}
        <span className="gal-tabs-sep" />
        <button
          type="button"
          role="tab"
          aria-selected={section === LOCAL_SECTION.key}
          className={`gal-tab${section === LOCAL_SECTION.key ? " on" : ""}`}
          onClick={() => setSection(LOCAL_SECTION.key)}
        >
          <IcoFolderL size={12} strokeWidth={2} /> {LOCAL_SECTION.label}
        </button>
        {appliedTag && <span className="gal-filter-chip">{appliedTag}</span>}
      </div>

      {/* ── 内容区 ── */}
      <div className="gal-body" ref={gridRef}>
        {isPixiv && (
          <>
            {/* [lc-2361] Pixiv API 不可达时给「可行动的提示」而不是裸错误。
                实测本机对 www.pixiv.net 的 TCP 443 会在 TLS 握手阶段被重置
                （不只是 DNS 污染），所以 Pixiv 分区在无代理环境下拿不到数据。
                这里如实说明原因 + 给出两条出路，而不是让用户看着「加载失败」。 */}
            {pxErr && (
              <div className="gal-unreachable">
                <div className="gal-unreachable-t">Pixiv 当前无法访问</div>
                <div className="gal-unreachable-d">
                  本机到 pixiv 的连接在 TLS 握手阶段被中断（不只是 DNS 污染）。
                  图片仍可通过自建镜像正常显示，但排行与搜索需要能连上 pixiv 的网络。
                </div>
                <div className="gal-unreachable-actions">
                  <button
                    type="button"
                    className="gal-more-btn"
                    onClick={() => {
                      lastLoad.current = "";
                      void loadPixiv(1, appliedTag, false);
                    }}
                  >
                    重试
                  </button>
                  <button
                    type="button"
                    className="gal-more-btn"
                    onClick={() => setSection("local")}
                  >
                    看本地图片
                  </button>
                </div>
                <details className="gal-unreachable-detail">
                  <summary>技术详情</summary>
                  <code>{pxErr}</code>
                </details>
              </div>
            )}
            {pxItems.length > 0 && (
              <div className="gal-grid">
                {splitColumns(pxItems, cols, (it) => (it.w && it.h ? it.w / it.h : 0.75)).map((col, ci) => (
                  <div className="gal-col" key={ci}>
                    {col.map((it) => (
                      <button
                        type="button"
                        key={it.id}
                        className="gal-card"
                        onClick={async () => {
                          // 点开先拉详情拿全部图片 URL（多图作品要 p0..pN）
                          try {
                            const d = await invoke<{ images: string[]; title: string; user: string; pages: number }>(
                              "pixiv_detail",
                              { id: it.id },
                            );
                            setPreview({
                              src: d.images[0] || it.full,
                              label: d.title || it.title,
                              sub: `${d.user}${d.pages > 1 ? ` · ${d.pages} 张` : ""}`,
                              gallery: d.images,
                            });
                          } catch {
                            // 详情失败就退回单图（不打断浏览）
                            setPreview({ src: it.full, label: it.title, sub: it.user });
                          }
                        }}
                      >
                        <img src={it.thumb} alt="" loading="lazy" draggable={false} />
                        <div className="gal-card-foot">
                          <span className="gal-card-title">{it.title || it.tags[0] || `#${it.id}`}</span>
                        </div>
                        <div className="gal-card-badges">
                          {/* 「9P」= 多图张数（pixiv 的 pageCount），原版同款语义 */}
                          {it.pages > 1 && <span className="gal-badge">{it.pages}P</span>}
                          {it.rank > 0 && it.rank <= 100 && <span className="gal-badge">#{it.rank}</span>}
                        </div>
                      </button>
                    ))}
                  </div>
                ))}
              </div>
            )}
            {pxBusy && <div className="gal-more">加载中…</div>}
            {!pxBusy && pxItems.length === 0 && !pxErr && (
              <div className="gal-empty">{appliedTag ? `没有匹配「${appliedTag}」的作品` : "暂无作品"}</div>
            )}
            {!pxBusy && pxMore && pxItems.length > 0 && (
              <button
                type="button"
                className="gal-more-btn"
                onClick={() => void loadPixiv(pxPage + 1, appliedTag, true)}
              >
                加载更多
              </button>
            )}
          </>
        )}

        {section === "local" && (
          <>
            <div className="gal-local-bar">
              <input
                className="set-input"
                placeholder="额外扫描的目录（绝对路径，留空只扫应用图片缓存）"
                value={localDir}
                onChange={(e) => setLocalDir(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void loadLocal(localDir.trim());
                }}
              />
              <button type="button" className="gal-more-btn" onClick={() => void loadLocal(localDir.trim())}>
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
                {splitColumns(localItems, cols, () => 0.75).map((col, ci) => (
                  <div className="gal-col" key={ci}>
                    {col.map((it) => (
                      <button
                        type="button"
                        key={it.path}
                        className="gal-card"
                        onClick={() => setPreview({ src: localImgUrl(it.path), label: it.name })}
                      >
                        <img src={localImgUrl(it.path)} alt="" loading="lazy" draggable={false} />
                        <div className="gal-card-foot">
                          <span className="gal-card-title">{it.name}</span>
                        </div>
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
      </div>

      {preview && (
        <div className="gal-lightbox" onClick={() => setPreview(null)} role="dialog" aria-label="图片预览">
          <button type="button" className="gal-lightbox-x" onClick={() => setPreview(null)} aria-label="关闭">
            <IcoXL size={18} strokeWidth={2} />
          </button>
          {/* [lc-2356] 多图作品（原版「9P」那种）左右切换：点图左半/右半翻页 */}
          {preview.gallery && preview.gallery.length > 1 && (
            <>
              <button
                type="button"
                className="gal-lb-nav left"
                aria-label="上一张"
                disabled={previewIdx <= 0}
                onClick={(e) => {
                  e.stopPropagation();
                  setPreviewIdx((i) => Math.max(0, i - 1));
                }}
              >
                ‹
              </button>
              <button
                type="button"
                className="gal-lb-nav right"
                aria-label="下一张"
                disabled={previewIdx >= preview.gallery.length - 1}
                onClick={(e) => {
                  e.stopPropagation();
                  setPreviewIdx((i) => Math.min(preview.gallery!.length - 1, i + 1));
                }}
              >
                ›
              </button>
            </>
          )}
          <img
            src={(preview.gallery && preview.gallery[previewIdx]) || preview.src}
            alt={preview.label}
            onClick={(e) => e.stopPropagation()}
            draggable={false}
          />
          <div className="gal-lightbox-cap">
            {preview.label}
            {preview.sub ? ` · ${preview.sub}` : ""}
            {preview.gallery && preview.gallery.length > 1
              ? ` · ${previewIdx + 1}/${preview.gallery.length}`
              : ""}
          </div>
        </div>
      )}

      {/* 保留导入以免未使用（Image 图标在 tabs 里可能按需启用） */}
      <IcoImageL size={0} style={{ display: "none" }} />
    </main>
  );
}
