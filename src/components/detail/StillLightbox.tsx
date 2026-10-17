/* [lc-1655] 电影详情页「剧照」全屏轮播：
   全屏深色遮罩 + contain 大图 + 左右循环箭头 + 右上关闭 + 底部计数（3 / 6）。
   键盘 ← → 切换、Esc 关闭；点遮罩空白处关闭（点图片不关）；默认 5s 自动轮播，鼠标悬停暂停。
   图片走 `tmdbImg(path, "original")`（fntvimg 代理，拉了即落盘）。 */
import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { tmdbImg } from "../../lib/images";

type Props = {
  /** TMDB file_path 列表（已在 DetailView 截到最多 6 条） */
  paths: string[];
  startIndex?: number;
  onClose: () => void;
};

export function StillLightbox({ paths, startIndex = 0, onClose }: Props) {
  const n = paths.length;
  const [idx, setIdx] = useState(() => (n ? Math.min(Math.max(startIndex, 0), n - 1) : 0));
  const [hover, setHover] = useState(false);

  const next = useCallback(() => setIdx((i) => (n ? (i + 1) % n : 0)), [n]);
  const prev = useCallback(() => setIdx((i) => (n ? (i - 1 + n) % n : 0)), [n]);

  // 键盘：← → 循环、Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      else if (e.key === "ArrowRight") next();
      else if (e.key === "ArrowLeft") prev();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [next, prev, onClose]);

  // 打开期间锁住页面滚动
  useEffect(() => {
    const prevOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = prevOverflow;
    };
  }, []);

  // 自动轮播：每 5s 下一张，悬停暂停（仅多于 1 张时）
  useEffect(() => {
    if (n <= 1 || hover) return;
    const t = window.setInterval(next, 5000);
    return () => window.clearInterval(t);
  }, [n, hover, next]);

  if (!n) return null;

  return createPortal(
    <div
      className="slb-mask"
      onClick={onClose}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      role="dialog"
      aria-modal="true"
      aria-label="剧照轮播"
    >
      <button className="slb-close" title="关闭（Esc）" onClick={onClose}>
        ×
      </button>

      <button
        className="slb-nav slb-prev"
        title="上一张（←）"
        onClick={(e) => {
          e.stopPropagation();
          prev();
        }}
      >
        ‹
      </button>

      <img
        className="slb-img"
        key={paths[idx]}
        src={tmdbImg(paths[idx], "original")}
        alt={`剧照 ${idx + 1}`}
        onClick={(e) => e.stopPropagation()}
      />

      <button
        className="slb-nav slb-next"
        title="下一张（→）"
        onClick={(e) => {
          e.stopPropagation();
          next();
        }}
      >
        ›
      </button>

      <div className="slb-count" onClick={(e) => e.stopPropagation()}>
        {idx + 1} / {n}
      </div>
    </div>,
    document.body,
  );
}
