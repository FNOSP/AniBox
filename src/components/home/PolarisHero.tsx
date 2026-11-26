/* [lc-2372] 首页轮播「放映厅」样式（Polaris Player 5.2.8 首页 Hero 的还原）。
 *
 * 与既有的 `CarouselView`（Misty 横滑）并列存在，由设置项 `carouselStyle` 二选一：
 *   "misty"    —— 原有横滑满屏海报（默认，行为一字未改）
 *   "spotlight" —— 本组件：背景交叉淡入 + 居中信息块 + 右侧竖排进度点
 *
 * 结构对照 Polaris（`<div class="pw-spotlight">`）逐层映射：
 *   .polaris-hero                        ← .pw-spotlight
 *     .polaris-hero-bg ×2（A/B 双层）     ← .pw-spotlight-bg + _pwHeroH1Fx 双缓冲
 *     .polaris-hero-shade-left/bottom    ← .pw-spotlight-gradient-left / -bottom
 *     .polaris-hero-content              ← .pw-spotlight-content
 *     .polaris-hero-dots                 ← .pw-carousel-dots（竖排 + 进度填充）
 *     .polaris-hero-picker               ← .pw-hero-picker（悬停缩略图选择器）
 */

import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { sampleChromeTone } from "../../lib/chrome";
import { imgUrl } from "../../lib/images";
import { prefersReducedMotion } from "../../lib/motion";
import { ChromeTone, MediaItem, PlayTarget } from "../../types";

/** Polaris 自动轮播间隔：6000ms（源码 `_0x50ae63` 的 setInterval 实参）。
 *  既有 Misty 是 8000ms —— 两者按各自的样式档独立取值，不共用一个常量。 */
const AUTO_MS = 6000;
/** 交叉淡入时长：Polaris `pwH1BgIn` 的 1.15s（进场）；退场 0.62s（`pwH1BgOut`）。
 *  这里用同一组数值，缓动也照抄 cubic-bezier(0.24,0.7,0.14,1) / (0.4,0,0.6,1)。 */
const BG_IN_MS = 1150;
/** 文案逐行入场：Polaris `.pw-hero-copy-cycle > *` 的 pwH1CopyIn 0.62s + 110ms 递增错峰。 */
const COPY_IN_MS = 620;
const COPY_STAGGER_MS = 110;
/** 缩略图选择器离开后的宽限（Polaris `_0x17506f` = 240ms）。 */
const PICKER_GRACE_MS = 240;

/** 时长/错峰写进 CSS 变量，样式表与组件共用一套数值（改档只改这里）。 */
const CSS_VARS = {
  "--polaris-bg-in": `${BG_IN_MS}ms`,
  "--polaris-copy-in": `${COPY_IN_MS}ms`,
  "--polaris-copy-stagger": `${COPY_STAGGER_MS}ms`,
  "--polaris-cycle": `${AUTO_MS}ms`,
} as CSSProperties;

export function PolarisHero({
  items,
  bds,
  logos,
  onPlay,
  onDetail,
  onTone,
}: {
  items: MediaItem[];
  bds: Record<string, string>;
  logos: Record<string, string>;
  onPlay: (t: PlayTarget) => void;
  onDetail: (item: MediaItem) => void;
  /** 顶栏自适应反色：与 Misty 版同口径，取当前这张 hero 图采样 */
  onTone?: (t: ChromeTone) => void;
}) {
  const n = items.length;
  const [idx, setIdx] = useState(0);
  // [Polaris 双缓冲] A/B 两层背景交替：新图进 A 层做进场动画，旧图留在 B 层做退场，
  //  动画结束后 B 层清空。对应源码 `_pwHeroH1Fx` 的 `_pwH1Front` A/B 翻转。
  const [front, setFront] = useState<"A" | "B">("A");
  const [layerSrc, setLayerSrc] = useState<{ A: string; B: string }>({ A: "", B: "" });
  // 文案重放入场动画的 key：每次切页 +1，React 用 key 重挂载触发 CSS 动画
  const [copyKey, setCopyKey] = useState(0);
  // 缩略图选择器（Polaris 悬停右侧圆点弹出的竖排缩略图条）
  const [pickerOpen, setPickerOpen] = useState(false);
  const pickerTimer = useRef<number | null>(null);

  const activeItem = items[idx];
  const backdropOf = (it?: MediaItem) =>
    it ? imgUrl(bds[it.guid] || it.poster || "") : "";

  const clearPickerTimer = () => {
    if (pickerTimer.current != null) {
      window.clearTimeout(pickerTimer.current);
      pickerTimer.current = null;
    }
  };
  // 鼠标进入圆点/选择器 → 立即弹；离开 → 等 240ms 宽限（够鼠标移进选择器）
  const openPicker = useCallback(() => {
    clearPickerTimer();
    if (n >= 2) setPickerOpen(true);
  }, [n]);
  const schedulePickerClose = useCallback(() => {
    clearPickerTimer();
    pickerTimer.current = window.setTimeout(() => setPickerOpen(false), PICKER_GRACE_MS);
  }, []);
  useEffect(() => clearPickerTimer, []);

  // 切页：推进索引 + 背景双缓冲 + 文案重播
  const goTo = useCallback(
    (next: number) => {
      if (n < 2) return;
      const target = ((next % n) + n) % n;
      if (target === idx) return;
      setIdx(target);
      const src = backdropOf(items[target]);
      // [双缓冲] 写到「背面」那一层，然后翻转到它
      const back = front === "A" ? "B" : "A";
      setLayerSrc((prev) => ({ ...prev, [back]: src }));
      setFront(back);
      setCopyKey((k) => k + 1);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [idx, n, front, bds]
  );

  const go = useCallback((delta: number) => goTo(idx + delta), [goTo, idx]);

  // 数据加载/刷新（条目数变化）→ 回到第一张，并把首图直接灌进 A 层。
  // 首帧不走交叉淡入（Polaris `_pwHeroH1Fx` 在没有 `_pwH1CurUrl` 时也是直接赋值）。
  useEffect(() => {
    setIdx(0);
    setFront("A");
    setLayerSrc({ A: backdropOf(items[0]), B: "" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [n]);

  // 自动轮播 6s（Polaris 同款；页面隐藏时跳过，避免回前台连跳）
  useEffect(() => {
    if (n < 2) return;
    const t = window.setTimeout(() => {
      if (!document.hidden) go(1);
    }, AUTO_MS);
    return () => window.clearTimeout(t);
  }, [n, idx, go]);

  // 顶栏自适应反色（与 Misty 版同口径）
  const activeSrc = activeItem ? backdropOf(activeItem) : "";
  useEffect(() => {
    if (!onTone) return;
    if (!activeSrc) {
      onTone("auto");
      return;
    }
    let alive = true;
    sampleChromeTone(activeSrc).then((t) => {
      if (alive) onTone(t ?? "fallback");
    });
    return () => {
      alive = false;
    };
  }, [activeSrc, onTone]);

  if (n === 0) return null;

  const yearOf = (it: MediaItem) => String(it.release_date || it.air_date || "").slice(0, 4);
  const typeOf = (it: MediaItem) => ((it.type || "").toLowerCase() === "movie" ? "电影" : "剧集");
  const ratingOf = (it: MediaItem) =>
    it.vote_average && it.vote_average !== "0" ? Number(it.vote_average).toFixed(1) : "";

  const reduced = prefersReducedMotion();

  return (
    <div className={`polaris-hero${reduced ? " no-motion" : ""}`} style={CSS_VARS}>
      {/* 背景双层：A/B 各自独立，交叉淡入由 .is-front 驱动 */}
      {(["A", "B"] as const).map((slot) => (
        <div
          key={slot}
          className={`polaris-hero-bg polaris-hero-bg-${slot}${layerSrc[slot] ? "" : " is-empty"}`}
          style={layerSrc[slot] ? { backgroundImage: `url('${layerSrc[slot]}')` } : undefined}
          data-front={front === slot ? "1" : "0"}
        />
      ))}
      <div className="polaris-hero-shade-left" aria-hidden="true" />
      <div className="polaris-hero-shade-bottom" aria-hidden="true" />

      <div className="polaris-hero-content" key={copyKey}>
        <div className={`polaris-hero-title-wrap${logos[activeItem.guid] ? " has-logo" : ""}`}>
          {logos[activeItem.guid] ? (
            <img
              className="polaris-hero-logo"
              src={imgUrl(logos[activeItem.guid])}
              alt={activeItem.title}
              draggable={false}
            />
          ) : (
            <h1 className="polaris-hero-title">{activeItem.title}</h1>
          )}
        </div>
        <div className="polaris-hero-meta">
          <span>{yearOf(activeItem) || "-"}</span>
          <span className="polaris-hero-dot-sep" />
          <span>{typeOf(activeItem)}</span>
          {ratingOf(activeItem) ? (
            <>
              <span className="polaris-hero-dot-sep" />
              <span className="polaris-hero-rating">★ {ratingOf(activeItem)}</span>
            </>
          ) : null}
        </div>
        {activeItem.overview ? (
          <div className="polaris-hero-overview">{activeItem.overview}</div>
        ) : null}
        <div className="polaris-hero-btns">
          <button
            type="button"
            className="polaris-hero-play"
            onClick={() => onPlay({ guid: activeItem.guid, title: activeItem.title })}
          >
            <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
              <path
                d="M8 5.14v13.72c0 .87.95 1.4 1.69.93l10.83-6.86c.69-.44.69-1.42 0-1.86L9.69 4.21c-.74-.47-1.69.06-1.69.93z"
                fill="currentColor"
                stroke="currentColor"
                strokeWidth="1.8"
                strokeLinejoin="round"
                strokeLinecap="round"
              />
            </svg>
            播放
          </button>
          <button
            type="button"
            className="polaris-hero-icon-btn"
            title="查看详情"
            aria-label="查看详情"
            onClick={() => onDetail(activeItem)}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="12" cy="12" r="9" />
              <line x1="12" y1="10.5" x2="12" y2="16.5" />
              <circle cx="12" cy="7.8" r="0.9" fill="currentColor" stroke="none" />
            </svg>
          </button>
        </div>
      </div>

      {/* 右侧竖排进度点：当前项带 6s 自下而上的填充条（Polaris `pwHeroDotFillV`）。
          选择器弹出时圆点淡出（Polaris `.pw-hero-dots-hidden`），避免两套导航叠在一起。 */}
      {n > 1 && (
        <div
          className={`polaris-hero-dots${pickerOpen ? " is-hidden" : ""}`}
          onMouseEnter={openPicker}
          onMouseLeave={schedulePickerClose}
        >
          {items.map((it, i) => (
            <button
              key={`${it.guid}#${i}`}
              type="button"
              className={`polaris-hero-dot${i === idx ? " is-active" : ""}`}
              aria-label={`第 ${i + 1} 个：${it.title}`}
              aria-current={i === idx}
              onClick={() => goTo(i)}
            />
          ))}
        </div>
      )}

      {/* 悬停缩略图选择器（Polaris `.pw-hero-picker`，右侧竖排 16:9 小卡） */}
      {n > 1 && (
        <div
          className={`polaris-hero-picker${pickerOpen ? " is-open" : ""}`}
          onMouseEnter={openPicker}
          onMouseLeave={schedulePickerClose}
          aria-hidden={!pickerOpen}
        >
          {items.map((it, i) => {
            const thumb = backdropOf(it);
            return (
              <button
                key={`${it.guid}#pick${i}`}
                type="button"
                className={`polaris-hero-pick${i === idx ? " is-active" : ""}`}
                onClick={() => {
                  goTo(i);
                  schedulePickerClose();
                }}
                tabIndex={pickerOpen ? 0 : -1}
              >
                {thumb ? <img src={thumb} alt="" loading="lazy" /> : <div className="noart" />}
                <div className="grad" />
                {logos[it.guid] ? (
                  <img className="pick-logo" src={imgUrl(logos[it.guid])} alt="" loading="lazy" />
                ) : (
                  <div className="pick-title">{it.title}</div>
                )}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
