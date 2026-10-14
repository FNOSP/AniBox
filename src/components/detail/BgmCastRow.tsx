/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useState } from "react";
import { User as IcoUserL } from "lucide-react";
import { bgmImg } from "../../lib/images";

export function BgmCastRow({
  name,
  tag,
  sub,
  avatar,
  img,
  fallback,
}: {
  name?: string;
  tag?: string;
  sub?: string;
  avatar?: string;
  /** 已解析的完整图片 URL（优先于 avatar） */
  img?: string;
  fallback: string;
}) {
  const src = img || bgmImg(avatar);
  // [lc-1403] 图片加载失败 → 退回占位图标（否则 .bgmc-avatar 的圆形底色会留一个灰圆，
  // 在演员表里看着像"背景上有个大圆圈"）。onError 只触发一次，不重试、不打日志（避免刷屏）。
  const [failed, setFailed] = useState(false);
  const showImg = !!src && !failed;
  return (
    <li className="bgmc-item">
      {showImg ? (
        <img
          className="bgmc-avatar"
          src={src}
          alt=""
          loading="lazy"
          width={40}
          height={40}
          onError={() => setFailed(true)}
        />
      ) : (
        // [lc-1414] 兜底也用同一套配色（按名字取色），与吐槽页的 AvatarSafe 观感一致
        <span
          className="bgmc-avatar avatar-fallback"
          style={{ background: `hsl(${((): number => {
            const t = String(name || "");
            let h = 0;
            for (let i = 0; i < t.length; i++) h = (h * 31 + t.charCodeAt(i)) % 360;
            return h;
          })()} 42% 62%)` }}
          aria-hidden="true"
        >
          <IcoUserL size={15} strokeWidth={1.8} />
        </span>
      )}
      <div className="bgmc-main">
        <div className="bgmc-head">
          <span className="bgmc-nick">{name || fallback}</span>
          {tag && <span className="bgmc-role">{tag}</span>}
        </div>
        {sub && <div className="bgmc-sub">{sub}</div>}
      </div>
    </li>
  );
}

/** [lc-128] 飞牛侧演员表 —— **只做兜底**（TMDB 拿不到时才用）。飞牛的 item 详情是否带演员字段
 *  依版本/刮削源而定：有就用（兼容字符串数组与对象数组、多种键名），**没有就是空数组**，
 *  不报错、不渲染空区块。 */
