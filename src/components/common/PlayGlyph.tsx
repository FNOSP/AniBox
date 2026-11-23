/**
 * [lc-2321] 播放三角——全站唯一实现。
 *
 * 为什么不用 lucide 的 `Play` + `fill`：那是「描边图标再填色」，三角的顶点在
 * 描边宽度上被切平，且视觉重心天然偏左，各页面只好各写一份 `margin-left: 2px`
 * 之类的补丁去凑（PosterCard 就是这么写的），缩放一变又歪。
 *
 * 也不用字符 `▶`：形状随字体变化（雅黑 / 思源 / 回退字体的三角角度都不同），
 * 无法对齐，也无法随字号平滑缩放。
 *
 * 这里是**单一路径 + 圆角**：三点用圆弧连接，视觉重心按排版惯例右移
 * （三角的「重量」在左侧竖直边，几何居中会显得偏左）。所以调用方只需要
 * 给一个 size，不用再补 margin。
 */
export function PlayGlyph({
  size = 18,
  className,
  title,
}: {
  size?: number;
  className?: string;
  title?: string;
}) {
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden={title ? undefined : true}
      role={title ? "img" : undefined}
      focusable="false"
    >
      {title && <title>{title}</title>}
      {/* 圆角三角：竖直边 x=8.6，顶点 (19.1, 12)，圆角半径 2.1。
          这个比例（宽 10.5 / 高 14.6）与系统播放键一致；重心右移约 1.6 抵消
          左侧竖边的视觉重量，所以外框不必再补偿。 */}
      <path d="M9.9 4.5c-1.2-.7-2.7.2-2.7 1.6v11.8c0 1.4 1.5 2.3 2.7 1.6l9.9-5.9c1.2-.7 1.2-2.5 0-3.2z" />
    </svg>
  );
}

/**
 * 「播放中」的圆点标记（替代字符 `◉`）。
 * 实心圆心 + 细外环：比字符更稳，也不会因字体回退变成别的符号。
 */
export function PlayingGlyph({
  size = 18,
  className,
}: {
  size?: number;
  className?: string;
}) {
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      aria-hidden
      focusable="false"
    >
      <circle cx="12" cy="12" r="8.4" fill="none" stroke="currentColor" strokeWidth="1.8" />
      <circle cx="12" cy="12" r="3.4" fill="currentColor" />
    </svg>
  );
}
