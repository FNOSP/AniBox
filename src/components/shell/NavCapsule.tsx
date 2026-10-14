/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { House as IcoHouseL, Search as IcoSearchL } from "lucide-react";

export function NavCapsule({
  page,
  onNav,
}: {
  page: "home" | "discover";
  onNav: (p: "home" | "discover") => void;
}) {
  return (
    <div className="nav-cap" data-side={page} role="tablist" aria-label="主导航">
      {/* [lc-123-rev6] 滑动指示器（"水滴"）：绝对定位、宽度 = 一个槽位，translateX(0|100%) 搬家；
          切换瞬间内部 scaleX(.92)→1 做出黏滞回弹（两个不同 animation-name 保证每次切换都重放） */}
      <span className="nav-cap-ind" aria-hidden="true">
        <i />
      </span>
      <button
        role="tab"
        aria-selected={page === "home"}
        aria-label="首页"
        title="首页"
        className={`nav-cap-l${page === "home" ? " on" : ""}`}
        onClick={() => onNav("home")}
      >
        <IcoHouseL size={18} strokeWidth={1.8} />
      </button>
      <button
        role="tab"
        aria-selected={page === "discover"}
        aria-label="发现"
        title="发现"
        className={`nav-cap-l${page === "discover" ? " on" : ""}`}
        onClick={() => onNav("discover")}
      >
        <IcoSearchL size={18} strokeWidth={1.8} />
      </button>
    </div>
  );
}
