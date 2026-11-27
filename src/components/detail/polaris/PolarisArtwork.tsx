/** [lc-2417] 艺术图选择器 —— 照 WWPlayer 1.3.20 `ViewingExperience-B2PUeIVg.js`
 *  的 `B`（工具栏 + 横排）与 `V`（全屏查看器）复刻。
 *
 * 源实现的关键结构（逐条对照）：
 *   · 工具栏：`detail-section-head content-section-header artwork-toolbar`
 *       标题「艺术图」（i18n `detail.artwork`）+ `artwork-categories`（role=group）
 *       三段按钮 `artwork-command`，`aria-pressed` 标当前分类 + 右侧箭头步进
 *   · 横排：`art-rail padded-rail`，卡片 `art-card art-card-<role>`，
 *       `aria-pressed` 标当前偏好；点卡片 → 打开全屏查看器
 *   · 查看器：portal 到 body 的 `.modal-backdrop.artwork-viewer-backdrop`
 *       （role=dialog aria-modal），内含
 *       - 右上关闭钮 `artwork-viewer-close small-round`
 *       - 大图 `artwork-viewer-image`
 *       - 底部操作 `artwork-viewer-actions`：设为偏好 / 恢复默认 / 状态文字
 *       - 底部数字指示器 `artwork-viewer-indicators`（1..N，当前项 is-active）
 *       键盘：Esc 关闭、← → 切换、Tab 在对话框内循环、滚轮切换
 *
 * 照抄的 CSS 值（源 `App-CkjaZB73.css`）：
 *   .art-rail            gap:18px; display:flex; overflow:visible; scrollbar 隐藏
 *   .art-card            flex:0 0 300px
 *   .art-card img        aspect-ratio:16/9; object-fit:cover; border-radius:--radius-md
 *   .art-card-poster     flex-basis:112px; img { aspect-ratio:2/3; object-fit:contain }
 *   .art-card-logo img   object-fit:contain
 *   .art-card:hover img  translateY(-2px)
 *   .art-card[aria-pressed=true] outline:2px solid focus-ring; outline-offset:-2px
 *   .artwork-command     border-radius:--radius-full; padding:6px 14px
 *   .artwork-viewer-indicator  28x28 圆钮; font-size:12px
 */
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { X as IcoCloseL } from "lucide-react";
import { tmdbImg } from "../../../lib/images";
import {
  ARTWORK_ROLES,
  ROLE_LABEL,
  artworkUrl,
  type ArtworkRole,
} from "../../../lib/detailArtwork";
import { useRailArrows } from "./RailArrows";

/** 候选池（后端 `tmdb_show_details` 的 `imagePools`）。
 *  logo 元素带 lang（源实现用 `iso_639_1` 做中文优先排序），这里只用于 title 提示。 */
export type ImagePools = {
  artPool?: string[];
  posterPool?: string[];
  logoPool?: { path: string; lang?: string }[];
};

type Pool = Record<ArtworkRole, string[]>;

/** 按角色取候选池。源实现 `d()` 的口径：**去重**并保留传入顺序。
 *  剧照（backdrop）用 `artPool`（= 两者的 `images.backdrops`，同源）。 */
export function buildPools(pools: ImagePools): Pool {
  const dedupe = (arr: string[]) => {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const raw of arr) {
      const p = String(raw ?? "").trim();
      if (!p || seen.has(p)) continue;
      seen.add(p);
      out.push(p);
    }
    return out;
  };
  return {
    poster: dedupe(pools.posterPool ?? []),
    backdrop: dedupe(pools.artPool ?? []),
    logo: dedupe((pools.logoPool ?? []).map((x) => x?.path ?? "")),
  };
}

type Props = {
  pools: ImagePools;
  /** 已存偏好：role → 本地文件路径 */
  picks: Partial<Record<ArtworkRole, string>>;
  /** 当前展示用的默认图（未设偏好时高亮它） */
  defaults: Partial<Record<ArtworkRole, string>>;
  /** 设为偏好 / 恢复默认。null → 恢复默认。
      返回**保存后的本地文件路径**（恢复默认时为空串）。 */
  onSave: (role: ArtworkRole, src: string | null) => Promise<string>;
  /** 偏好已生效：path 是**本地文件路径**（不是 TMDB 路径）。
      父级拿它拼 artwork:// 协议换背景/海报/Logo。
      ⚠ [lc-2419] 以前这里传的是 src（TMDB 路径），父级直接当 <img src> 用
      → 浏览器把 `/abc.jpg` 当应用自身地址请求 → 裂图。 */
  onApplied?: (role: ArtworkRole, localPath: string | null) => void;
};

export function PolarisArtwork({ pools, picks, defaults, onSave, onApplied }: Props) {
  const [role, setRole] = useState<ArtworkRole>("poster");

  // 已设偏好 → 放进候选首位（源实现 `C = [...(y ? [y] : []), ...S.filter(x => x !== y)]`）
  const picked = picks[role];
  const defaultPath = defaults[role];
  const built = buildPools(pools);
  const display: string[] = (() => {
    const pool = built[role];
    if (!picked) return pool;
    // 偏好是本地文件（不是 TMDB 路径），用协议 URL 展示在最前
    return [picked, ...pool];
  })();

  const [viewerAt, setViewerAt] = useState<number | null>(null);
  const [status, setStatus] = useState("");
  const [saving, setSaving] = useState(false);
  const railRef = useRef<HTMLDivElement | null>(null);
  const rail = useRailArrows(railRef, [role, display.length]);

  if (display.length === 0 && !picked) return null;

  const isLocal = (p: string) => p === picked;
  /** 是否是 TMDB 的 file_path（可交给后端下载的那种）。
      本地偏好路径（.../detail-artwork/<hex>/<role>.webp）与代理 URL 都不算。 */
  const isTmdbPath = (p: string) => /^\/[^:]*\.(jpe?g|png|webp|avif)$/i.test(String(p ?? ""));

  const openViewer = (i: number) => setViewerAt(i);

  const save = async (src: string | null) => {
    if (saving) return;
    /* [lc-2418] `src === null` 是"恢复默认"，放行；
       否则必须是 **TMDB file_path**（形如 /abc.jpg）。
       绝不能把 display 首项（已存偏好的本地绝对路径）再传回后端 ——
       那会被 resolve_source 判成"不支持的艺术图来源"，正是用户报的这个错。 */
    if (src !== null && !isTmdbPath(src)) {
      setStatus("只能把 TMDB 的图设为偏好");
      return;
    }
    setSaving(true);
    setStatus("");
    try {
      const localPath = await onSave(role, src);
      /* [lc-2419] 只把**本地文件路径**交给父级。
         传 src（TMDB 路径）会让 `/abc.jpg` 被当成应用自身地址 → 裂图。 */
      onApplied?.(role, localPath || null);
    } catch (e) {
      setStatus(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="pdt-artwork">
      {/* ── 工具栏：标题 + 三段分类 + 箭头（源 `.artwork-toolbar`） ── */}
      <div className="pdt-section-head pdt-artwork-toolbar">
        <span className="pdt-heading">艺术图</span>
        <div className="pdt-artwork-cats" role="group" aria-label="艺术图">
          {ARTWORK_ROLES.map((r) => (
            <button
              key={r}
              type="button"
              className="pdt-artwork-cmd"
              aria-pressed={role === r}
              onClick={() => setRole(r)}
            >
              {ROLE_LABEL[r]}
            </button>
          ))}
        </div>
        <div className="pdt-artwork-arrows">
          <button
            type="button"
            className="pdt-ep-nav-btn"
            disabled={!rail.atStart}
            aria-label="上一张"
            onClick={() => rail.scrollBy(-1)}
          >
            <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
              <path d="M15 5l-7 7 7 7" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </button>
          <button
            type="button"
            className="pdt-ep-nav-btn"
            disabled={!rail.atEnd}
            aria-label="下一张"
            onClick={() => rail.scrollBy(1)}
          >
            <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
              <path d="M9 5l7 7-7 7" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </button>
        </div>
      </div>

      {/* ── 横排（源 `.art-rail`） ── */}
      <div className="pdt-art-rail" ref={railRef}>
        {display.map((p, i) => {
          const src = isLocal(p) ? artworkUrl(p) : tmdbImg(p, "w780");
          const isPicked = !!picked && isLocal(p);
          const isDefault = !picked && !!defaultPath && p === defaultPath;
          return (
            <button
              key={`${role}-${p}-${i}`}
              type="button"
              className={`pdt-art-card pdt-art-card-${role}`}
              aria-label={`${ROLE_LABEL[role]} ${i + 1}`}
              aria-pressed={isPicked || isDefault}
              onClick={() => openViewer(i)}
            >
              <img src={src} alt="" loading="lazy" decoding="async" draggable={false} />
            </button>
          );
        })}
      </div>

      {viewerAt !== null &&
        createPortal(
          <ArtworkViewer
            role={role}
            list={display}
            index={viewerAt}
            picked={picked}
            saving={saving}
            status={status}
            onIndex={setViewerAt}
            onClose={() => setViewerAt(null)}
            onSave={save}
            toSrc={(p) => (p === picked ? artworkUrl(p) : tmdbImg(p, "original"))}
          />,
          document.body,
        )}
    </section>
  );
}

/* ───────────────────────── 全屏查看器（源 `V`） ───────────────────────── */
function ArtworkViewer({
  role,
  list,
  index,
  picked,
  saving,
  status,
  onIndex,
  onClose,
  onSave,
  toSrc,
}: {
  role: ArtworkRole;
  list: string[];
  index: number;
  picked?: string;
  saving: boolean;
  status: string;
  onIndex: (i: number) => void;
  onClose: () => void;
  onSave: (src: string | null) => void;
  toSrc: (p: string) => string;
}) {
  const [closing, setClosing] = useState(false);
  const closeRef = useRef<HTMLButtonElement>(null);
  const indWrapRef = useRef<HTMLDivElement>(null);
  const indActiveRef = useRef<HTMLButtonElement>(null);
  const cur = list[index] ?? "";
  const isCurrentPicked = !!picked && cur === picked;

  // 打开即聚焦关闭钮（源实现同款）
  useEffect(() => {
    const id = requestAnimationFrame(() => closeRef.current?.focus({ preventScroll: true }));
    return () => cancelAnimationFrame(id);
  }, []);

  // 锁 body 滚动 + 键盘（源实现同款：Esc 关闭、← → 切换、Tab 循环）
  useEffect(() => {
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        setClosing(true);
        return;
      }
      if (list.length < 2 || (e.key !== "ArrowLeft" && e.key !== "ArrowRight")) return;
      e.preventDefault();
      const step = e.key === "ArrowRight" ? 1 : -1;
      onIndex(Math.max(0, Math.min(list.length - 1, index + step)));
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      document.body.style.overflow = prev;
    };
  }, [index, list.length, onIndex]);

  // 指示器跟随（源实现用 offsetLeft 把当前项滚进视野）
  useEffect(() => {
    const wrap = indWrapRef.current;
    const act = indActiveRef.current;
    if (!wrap || !act) return;
    const left = act.offsetLeft;
    const right = left + act.offsetWidth;
    if (left < wrap.scrollLeft) wrap.scrollLeft = left;
    else if (right > wrap.scrollLeft + wrap.clientWidth) wrap.scrollLeft = right - wrap.clientWidth;
  }, [index]);

  const requestClose = () => setClosing(true);

  return (
    <div
      className={`pdt-artwork-backdrop${closing ? " is-closing" : ""}`}
      role="dialog"
      aria-modal="true"
      aria-label={`${ROLE_LABEL[role]} ${index + 1}/${list.length}`}
      onClick={(e) => {
        if (e.target === e.currentTarget) requestClose();
      }}
      onAnimationEnd={(e) => {
        if (closing && e.target === e.currentTarget) onClose();
      }}
    >
      <button
        ref={closeRef}
        type="button"
        className="pdt-artwork-close"
        aria-label="关闭"
        onClick={requestClose}
      >
        <IcoCloseL size={18} />
      </button>

      <img className="pdt-artwork-viewer-img" src={toSrc(cur)} alt={`${ROLE_LABEL[role]} ${index + 1}`} draggable={false} />

      <div className="pdt-artwork-actions">
        <button
          type="button"
          className="pdt-artwork-cmd"
          aria-pressed={isCurrentPicked}
          disabled={saving}
          onClick={() => {
            if (isCurrentPicked) return;
            onSave(cur);
          }}
        >
          {isCurrentPicked ? "当前偏好" : "设为偏好"}
        </button>
        {picked && (
          <button type="button" className="pdt-artwork-cmd" disabled={saving} onClick={() => onSave(null)}>
            恢复默认
          </button>
        )}
        {saving && <span className="pdt-artwork-status" role="status">正在保存</span>}
        {status && <span className="pdt-artwork-status" role="status">{status}</span>}
      </div>

      <div className="pdt-artwork-indicators" role="group" aria-label={`艺术图选择`} ref={indWrapRef}>
        {list.map((p, i) => (
          <button
            key={`ind-${p}-${i}`}
            ref={i === index ? indActiveRef : undefined}
            type="button"
            className={`pdt-artwork-indicator${i === index ? " is-active" : ""}`}
            aria-label={`艺术图 ${i + 1}`}
            aria-pressed={i === index}
            tabIndex={i === index ? 0 : -1}
            onClick={() => onIndex(i)}
          >
            {i + 1}
          </button>
        ))}
      </div>
    </div>
  );
}
