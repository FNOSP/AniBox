/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Menu as IcoMenuL, X as IcoXL } from "lucide-react";
import { ChromeTone } from "../../types";


export function TitleBar({
  title,
  nav,
  page,
  sidebarOpen,
  onToggleSidebar,
  onRefresh,
  refreshing,
  onToggleTheme,
  chrome,
}: {
  title: string;
  nav?: boolean;
  /** 当前"页"（首页/发现）：左上角那行字跟着它切（胶囊已挪到内容区浮层，见 NavCapsule） */
  page?: "home" | "discover";
  sidebarOpen?: boolean;
  onToggleSidebar?: () => void;
  onRefresh?: () => void;
  refreshing?: boolean;
  /** [lc-123-rev6] 传事件：深浅切换的圆形扩散要拿**按钮中心**做原点（getBoundingClientRect） */
  onToggleTheme?: (e: React.MouseEvent<HTMLButtonElement>) => void;
  /** [lc-123-rev4] 顶栏 chrome 色：auto=跟随主题（默认）；dark/light=按 hero 亮度采样；fallback=白字强阴影 */
  chrome?: ChromeTone;
}) {
  const win = getCurrentWindow();
  const crumb = page === "discover" ? "发现" : "首页";
  return (
    <div
      className="titlebar"
      data-tauri-drag-region
      data-chrome={chrome && chrome !== "auto" ? chrome : undefined}
    >
      <div className="tb-left" data-tauri-drag-region>
        {nav && onToggleSidebar && (
          <button
            className="tb-icon"
            title={sidebarOpen ? "收起侧栏" : "展开侧栏"}
            onClick={onToggleSidebar}
          >
            <IcoMenuL size={16} strokeWidth={1.8} />
          </button>
        )}
        {/* [lc-123] 左上角这行字 = **当前页名**（首页/发现），随胶囊切换（不再是"回首页"按钮） */}
        {nav && <span className="tb-crumb">{crumb}</span>}
        {nav && onRefresh && (
          <button
            className={`tb-icon${refreshing ? " spinning" : ""}`}
            // [lc-1640] 语义由"刷新首页数据"改为"按当前页强制刷新"（各视图分派到自己的 force 刷新）。
            title="刷新当前页数据"
            onClick={onRefresh}
            disabled={refreshing}
          >
            <span className="tb-spin">⟳</span>
          </button>
        )}
        {onToggleTheme && (
          <button className="tb-icon" title="切换深浅模式" onClick={(e) => onToggleTheme(e)}>
            ◐
          </button>
        )}
        {!nav && <span className="tb-page">{title}</span>}
      </div>
      {/* [lc-123-rev] 中部只剩弱化的品牌小字（原来这里是「首页|发现」胶囊，已挪到 .navbar 行）。
          整条顶栏仍是拖拽区：中间的 span 不拦截拖拽，窗口拖动不受影响。 */}
      <div className="tb-center" data-tauri-drag-region>
        {/* [lc-1614] 中部品牌字标：内联 SVG（多色渐变 + 轻微浮动），字体 Baloo 2（见 index.html）。
            替换 lc-1610 的内嵌方正少儿简体（商业字体内嵌有授权风险，已整体删除）。 */}
        <svg
          className="tb-brand tb-brand-logo"
          viewBox="0 0 800 240"
          xmlns="http://www.w3.org/2000/svg"
          role="img"
          aria-label="AniBox"
        >
          <defs>
            {/* 动漫感多色相渐变（青 → 蓝紫 → 紫 → 粉） */}
            <linearGradient id="aniboxLogoInk" x1="0" y1="0" x2="1" y2="0">
              <stop offset="0" stopColor="#5EC8E8" />
              <stop offset="0.33" stopColor="#7B8FE8" />
              <stop offset="0.66" stopColor="#C77BD8" />
              <stop offset="1" stopColor="#F58EB5" />
            </linearGradient>
          </defs>
          <g className="mark">
            <text
              x="400"
              y="162"
              textAnchor="middle"
              fontFamily="'Baloo 2', system-ui, sans-serif"
              fontWeight="800"
              fontSize="150"
              letterSpacing="1"
              fill="url(#aniboxLogoInk)"
            >AniBox</text>
          </g>
        </svg>
      </div>
      <div className="tb-right" data-tauri-drag-region>
        <div className="tb-buttons">
          <button className="tb-btn" title="最小化" onClick={() => win.minimize()}>
            ─
          </button>
          <button className="tb-btn" title="最大化/还原" onClick={() => win.toggleMaximize()}>
            ▢
          </button>
          <button className="tb-btn tb-close" title="关闭" onClick={() => win.close()}>
            <IcoXL size={16} strokeWidth={1.8} />
          </button>
        </div>
      </div>
    </div>
  );
}

/* ────────────────────────── 登录页 ────────────────────────── */
