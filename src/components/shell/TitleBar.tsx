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
  onHome,
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
  /** [lc-123-rev4] 保留该 prop 以兼容调用方；[lc-1702] 顶栏改用 CSS difference 混合自适应反色后不再消费它。 */
  chrome?: ChromeTone;
  /** [lc-1661] 点击左上角页名快速回首页；不传则仍渲染为纯文本（向后兼容） */
  onHome?: () => void;
}) {
  const win = getCurrentWindow();
  const crumb = page === "discover" ? "发现" : "首页";
  return (
    <>
      {/* [lc-1702] 自适应反色：整条顶栏是一个 `mix-blend-mode: difference` 混合组，
          白色基色在亮/暗背景上自动反相（亮底→黑图标，暗底→白图标）。
          品牌 logo 与关闭键不在这个组里（见下方两个独立图层）。 */}
      <div className="titlebar" data-tauri-drag-region>
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
          {/* [lc-1661] 用户要求：点这行字快速回首页。传了 onHome 才渲染成按钮（可点）；
              没传仍是纯文本，保持向后兼容。button 不是 data-tauri-drag-region 的命中目标，
              所以点击不会被拖拽区吞掉（同 .tb-icon 的既有行为）。 */}
          {nav &&
            (onHome ? (
              <button
                type="button"
                className="tb-crumb tb-crumb-btn"
                title="返回首页"
                aria-label="返回首页"
                onClick={onHome}
              >
                {crumb}
              </button>
            ) : (
              <span className="tb-crumb">{crumb}</span>
            ))}
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
        {/* [lc-1702] 中部只保留窗口按钮里的最小化/最大化（关闭键独立成层）。
            整条顶栏仍是拖拽区：中间的空白不拦截拖拽。 */}
        <div className="tb-right" data-tauri-drag-region>
          <div className="tb-buttons">
            <button className="tb-btn" title="最小化" onClick={() => win.minimize()}>
              ─
            </button>
            <button className="tb-btn" title="最大化/还原" onClick={() => win.toggleMaximize()}>
              ▢
            </button>
          </div>
        </div>
      </div>

      {/* [lc-1702] 品牌 logo：独立图层（不参与 difference 反色，保持原渐变/浮动）。
          pointer-events:none —— 不吃鼠标事件，拖拽仍由下方 .titlebar 接管。 */}
      <div className="tb-brand-layer">
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

      {/* [lc-1702] 关闭键独立图层：可点击（在 .titlebar 之上），平时白色 difference 反相，
          hover 时整层切回 normal —— 红色 hover 底色不会被混合弄脏。 */}
      <div className="tb-close-layer">
        <button className="tb-btn tb-close" title="关闭" onClick={() => win.close()}>
          <IcoXL size={16} strokeWidth={1.8} />
        </button>
      </div>
    </>
  );
}

/* ────────────────────────── 登录页 ────────────────────────── */
