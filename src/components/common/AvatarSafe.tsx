/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useState } from "react";
import { User as IcoUserL } from "lucide-react";

export function AvatarSafe({
  src,
  name,
  size = 36,
  className = "dvtc-ava",
}: {
  src?: string;
  /** 用于取稳定配色与 title（同一个人每次都是同一个颜色） */
  name?: string;
  size?: number;
  className?: string;
}) {
  const [failed, setFailed] = useState(false);
  const showImg = !!src && !failed;
  // 稳定的色相：把名字哈希成 0..360（同一个人刷新前后颜色一致，不会"闪色"）
  const hue = (() => {
    const s = String(name || "");
    let h = 0;
    for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360;
    return h;
  })();
  if (showImg) {
    return (
      <img
        className={className}
        src={src}
        alt=""
        loading="lazy"
        width={size}
        height={size}
        onError={() => setFailed(true)}
      />
    );
  }
  return (
    <span
      className={`${className} avatar-fallback`}
      style={{ background: `hsl(${hue} 42% 62%)` }}
      title={name || ""}
      aria-hidden="true"
    >
      <IcoUserL size={Math.round(size * 0.5)} strokeWidth={1.9} />
    </span>
  );
}

/** [lc-1417] 详情页骨架屏（用户反馈"没做骨架屏"，进页面时是空等）。
 *
 *  形状按详情页真实布局摆：左列 = 海报 + 标签 chips + 简介行；右列 = 选集行（图 + 两行文字）。
 *  复用既有 `.dsk` 的 shimmer 语言（与首页/发现页同一套），不再另造一套动画。
 *
 *  为什么必须连"右栏选集"一起摆：详情页的等待感主要来自右侧——条目详情没回来时
 *  选集整个是空的（用户截图里右侧就是一片白），只给左列摆骨架的话右侧空白更显慢。
 */
