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
            title="刷新首页数据"
            onClick={onRefresh}
            disabled={refreshing}
          >
            ⟳
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
        {/* [lc-1610] nav 时中间是品牌字标（挂 .tb-brand-logo：内嵌方正少儿简体 + 多色渐变）；
            非 nav 时显示页名，保持原来的弱化小字样式。 */}
        <span className={`tb-brand${nav ? " tb-brand-logo" : ""}`}>{nav ? "AniBox" : title}</span>
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
