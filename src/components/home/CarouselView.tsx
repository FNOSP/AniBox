/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useState } from "react";
import { sampleChromeTone } from "../../lib/chrome";
import { imgUrl } from "../../lib/images";
import { ChromeTone, MediaItem, PlayTarget } from "../../types";


export function CarouselView({
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
  /** [lc-123-rev4] 上报"当前这张 hero 图的顶栏 chrome 色"（App 透传给 TitleBar；仅首页用） */
  onTone?: (t: ChromeTone) => void;
}) {
  // [lc-1523] 真无限轮播：轨道 = [最后一张克隆, ...items, 第一张克隆]，
  //   pos 是**轨道下标**（1..n 为真实位；0 / n+1 为首尾克隆位）。
  //   跨接时先滑到克隆位（动画连续），transitionend 后再无动画瞬移回真实位。
  const [pos, setPos] = useState(1);
  const [armed, setArmed] = useState(false);
  // [lc-116] 鼠标靠近哪一侧："" | "prev" | "next"（用来淡入对应箭头）
  const [near, setNear] = useState<"" | "prev" | "next">("");
  // [lc-117] 首尾跨接的那一帧临时关掉 .mist-body 的 1.5s 横向过渡
  const [noAnim, setNoAnim] = useState(false);
  const n = items.length;

  const onBannerMove = (e: React.MouseEvent<HTMLDivElement>) => {
    const box = e.currentTarget;
    const w = box.clientWidth || 1;
    const x = e.clientX - box.getBoundingClientRect().left;
    const zone = Math.min(260, w * 0.22);
    setNear(x <= zone ? "prev" : x >= w - zone ? "next" : "");
  };

  /**
   * [lc-1523] 统一走这里切页，delta = -1 / +1。克隆轨道上直接 ±1：
   * 最后一张( pos=n )再 +1 → 滑到第一张克隆( n+1 )，动画连续；transitionend
   * 后 onTransEnd 无动画瞬移回真实第一张( 1 )。第一张( 1 )再 -1 同理走 last 克隆( 0 )。
   *
   * [lc-1630] 修「切到最后一张再点下一个没反应」：
   *   克隆位（pos===0 / n+1）在轨道上没有"更远"的下标，原来的 `target > n+1` 越界判断
   *   会直接 return —— 点击被吞掉。用户停在克隆位再点、或在过渡进行中（pos 已置为克隆位、
   *   但 transitionend 还没触发）就点，都会命中这条路径。
   *   做法：先把点击"映射回真实位"（克隆位与真实位是同一张图，复位瞬间无可见跳动），
   *   下一帧再走这一步。real+delta 仍落在合法轨道范围（n≥2 时 ∈ {0,1,2,n-1,n,n+1}），
   *   所以复位后照常走原有跨接逻辑，绝不会"点了没反应"。
   */
  const go = useCallback(
    (delta: number) => {
      if (n < 2) return;
      if (pos === n + 1 || pos === 0) {
        const real = pos === n + 1 ? 1 : n;
        setNoAnim(true);
        setPos(real);
        // 双 rAF：先让"关过渡 + 瞬移回真实位"真正渲染一帧，再开过渡走这一步，
        // 否则同一帧里改两次 transform 会被合并、复位不可见（时序不稳 → 可见跳动）。
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            setNoAnim(false);
            setPos(real + delta);
          })
        );
        return;
      }
      const target = pos + delta;
      if (target < 0 || target > n + 1) return;
      setPos(target);
    },
    [pos, n]
  );

  /** [lc-1523] 滑到克隆位后的瞬间重置：关过渡 → 瞬移回真实位 → 恢复过渡。 */
  const onTransEnd = useCallback((e: React.TransitionEvent) => {
    // 只认容器自身 transform 的过渡结束：子元素（图/遮罩/文字）的 transition 冒泡上来不理会
    if (e.target !== e.currentTarget || e.propertyName !== "transform") return;
    if (n < 2) return;
    if (pos === n + 1 || pos === 0) {
      setNoAnim(true);
      setPos(pos === n + 1 ? 1 : n);
      requestAnimationFrame(() => requestAnimationFrame(() => setNoAnim(false)));
    }
  }, [pos, n]);

  // [lc-1630] 克隆位兜底复位：正常由 onTransEnd 的 transitionend 复位；但过渡被打断、
  //   或 transition 被禁用（如 prefers-reduced-motion）时该事件不会触发，轨道会卡在克隆位。
  //   这里加一个比过渡（0.45s）稍长的定时器双保险；两者幂等，谁先到都一样。
  useEffect(() => {
    if (n < 2) return;
    if (pos !== 0 && pos !== n + 1) return;
    const t = setTimeout(() => {
      setNoAnim(true);
      setPos(pos === n + 1 ? 1 : n);
      requestAnimationFrame(() => requestAnimationFrame(() => setNoAnim(false)));
    }, 600);
    return () => clearTimeout(t);
  }, [pos, n]);

  // items 数量变化（刷新/换库）时把轨道拉回真实首位，避免 pos 越界
  useEffect(() => {
    setPos(1);
  }, [n]);

  // 入场动画闸门：挂载后瞬间补 active，让首帧信息走 misty 同款上浮入场
  useEffect(() => {
    const t = setTimeout(() => setArmed(true), 80);
    return () => clearTimeout(t);
  }, []);

  // 8s 自动轮播（emby-crx 同款节奏；页面隐藏时不切，避免回前台跳帧）
  // [lc-117] 依赖当前片（pos）而不是常驻 interval：手动点箭头后重新起算 8s，
  // 否则刚点过去就被自动轮播顶走，看着像「点了没反应」。
  useEffect(() => {
    if (n < 2) return;
    const t = setTimeout(() => {
      if (!document.hidden) go(1);
    }, 8000);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [n, pos]);

  // [lc-123-rev4] 顶栏自适应反色：把**当前这一张** hero 图的 URL 采样成 chrome 色上报。
  // 换片（pos 变）才重采；同一张图有 URL 级缓存（sampleChromeTone 内部）。采样失败 → "fallback"。
  const seq = n >= 2 ? [items[n - 1], ...items, items[0]] : items;
  const activeItem = seq[Math.min(pos, seq.length - 1)];
  const activeSrc = activeItem
    ? bds[activeItem.guid]
      ? imgUrl(bds[activeItem.guid])
      : activeItem.poster
        ? imgUrl(activeItem.poster)
        : null
    : null;
  useEffect(() => {
    if (!onTone) return;
    if (!activeSrc) {
      onTone("auto"); // 这一张没有图（垫底色块）→ 交回默认色
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

  return (
    <div
      className="mist-banner"
      onMouseMove={onBannerMove}
      onMouseLeave={() => setNear("")}
    >
      <div
        className={`mist-body${noAnim ? " no-anim" : ""}`}
        style={{ transform: `translateX(-${pos * 100}%)` }}
        onTransitionEnd={onTransEnd}
      >
        {seq.map((it, i) => {
          const bd = bds[it.guid];
          const meta = [
            yearOf(it),
            (it.type || "").toLowerCase() === "movie" ? "电影" : "剧集",
            it.vote_average && it.vote_average !== "0" ? `${it.vote_average}` : "",
          ]
            .filter(Boolean)
            .join(" · ");
          return (
            <div key={`${it.guid}#${i}`} className={`mist-item ${i === pos && armed ? "active" : ""}`}>
              {bd ? (
                <img
                  className="mist-cover"
                  src={imgUrl(bd)}
                  alt={it.title}
                  draggable={false}
                  loading={i <= 1 ? "eager" : "lazy"}
                />
              ) : (
                <div
                  className="mist-cover mist-cover-fallback"
                  style={
                    it.poster
                      ? { backgroundImage: `url(${imgUrl(it.poster)})` }
                      : undefined
                  }
                />
              )}
              <div className="mist-veil" />
              <div className="mist-info">
                <div className="mist-meta">{meta}</div>
                {logos[it.guid] ? (
                  <img
                    className="mist-logo"
                    src={imgUrl(logos[it.guid])}
                    alt={it.title}
                    draggable={false}
                    loading="lazy"
                  />
                ) : (
                  <h1>{it.title}</h1>
                )}
                <div>
                  <p>{it.overview || ""}</p>
                </div>
                <div className="mist-actions">
                  <button
                    className="mist-play"
                    onClick={() => onPlay({ guid: it.guid, title: it.title })}
                  >
                    ▶ 开始播放
                  </button>
                  <button className="mist-more" onClick={() => onDetail(it)}>
                    MORE
                  </button>
                </div>
              </div>
            </div>
          );
        })}
      </div>
      {/* [lc-116] 左右切换箭头：默认完全透明；鼠标移到对应侧的"附近区域"
          （左右各约 22% 宽，最多 260px）才淡入 —— 用 mousemove 判断靠近哪一侧，
          而不是 hover 整个 banner（hero 占满一屏，那样等于一直显示）。
          [lc-117] 三个坑：
          1) onMouseDown 里 preventDefault —— 原生 button 按下会抢焦点，浏览器把焦点元素
             滚进视口会让整页上下窜一下（第二个「跳动」来源）。
          2) 事件用 go(-1)/go(+1)，取模实现首尾反向回绕（第一张往左 = 最后一张）。
          3) 不再用 pointer-events:none 门控可点性 —— 鼠标静止时 mousemove 不刷新 near，
             按钮可能停在「看得见但点不动」的状态；透明时点到也只是切页，无副作用。 */}
      {n > 1 && (
        <>
          <button
            type="button"
            className={`mist-nav mist-nav-prev${near === "prev" ? " is-on" : ""}`}
            title="上一张"
            aria-label="上一张"
            onMouseEnter={() => setNear("prev")}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => go(-1)}
          >
            ‹
          </button>
          <button
            type="button"
            className={`mist-nav mist-nav-next${near === "next" ? " is-on" : ""}`}
            title="下一张"
            aria-label="下一张"
            onMouseEnter={() => setNear("next")}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => go(1)}
          >
            ›
          </button>
        </>
      )}
    </div>
  );
}

/* ────────────────────────── 海报卡 ────────────────────────── */
