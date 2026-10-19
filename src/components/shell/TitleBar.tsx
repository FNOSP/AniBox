/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Copy as IcoCopy, Menu as IcoMenuL, Minus as IcoMinus, Square as IcoSquare, X as IcoXL } from "lucide-react";
import { ChromeTone } from "../../types";
import type { MediaItem } from "../../types";
import { Tone, useAdaptiveTone } from "../../lib/adaptiveTone";
import { SearchLauncher } from "../search/SearchLauncher";

/* [lc-1709] 顶栏二值色调采样已抽到 src/lib/adaptiveTone.ts（与详情页头图共用同一实现）。
 * 这里只声明顶栏的采样区域：窗口顶部 40px 横带；跳过顶栏自身浮层，穿透到真实背景。 */
const SAMPLE_BAND = 40;
const OVERLAY_SELECTOR = ".titlebar, .tb-brand-layer, .tb-close-layer";

function titlebarPoints(): { x: number; y: number }[] {
  const w = window.innerWidth;
  const h = window.innerHeight;
  if (w <= 0 || h <= 0) return [];
  const y = Math.min(SAMPLE_BAND / 2, h - 1);
  return [0.05, 0.2, 0.35, 0.5, 0.65, 0.8, 0.95].map((r) => ({
    x: Math.min(w - 1, Math.round(w * r)),
    y,
  }));
}

/** 顶栏色调：初始值直接取主题，避免首帧闪白/闪黑。 */
function useTitlebarTone(page?: string, nav?: boolean): Tone {
  return useAdaptiveTone(
    {
      points: titlebarPoints,
      overlaySelector: OVERLAY_SELECTOR,
      band: () => ({ top: 0, bottom: SAMPLE_BAND }),
      extraKey: `${page ?? ""}|${nav ? 1 : 0}`,
    },
    [page, nav],
  );
}
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
  onSearchOpenItem,
  searchDisabled,
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
  /** [lc-123-rev4] 保留该 prop 以兼容调用方；[lc-1703] 顶栏已改为 JS 二值采样，不再消费它。 */
  chrome?: ChromeTone;
  /** [lc-1661] 点击左上角页名快速回首页；不传则仍渲染为纯文本（向后兼容） */
  onHome?: () => void;
  /** [lc-1725] 顶栏全局搜索：选中结果后的跳转（不传则不渲染搜索入口） */
  onSearchOpenItem?: (item: MediaItem) => void;
  /** [lc-1725] 登录页/游客态禁用搜索（不渲染入口） */
  searchDisabled?: boolean;
}) {
  const win = getCurrentWindow();
  const tone = useTitlebarTone(page, nav);
  const crumb = page === "discover" ? "发现" : "首页";
  // [fix] 最大化状态：用于把"最大化/还原"按钮的图标在 Square（最大化）与 Copy（还原）之间切换。
  // 优先用 Tauri 的 isMaximized() 初始化，并监听窗口 resize（拖动改变大小/双击标题栏等都会触发）持续同步。
  const [isMaximized, setIsMaximized] = useState(false);
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    const sync = () => {
      win
        .isMaximized()
        .then((v) => {
          if (alive) setIsMaximized(v);
        })
        .catch(() => {
          /* 非 Tauri 环境/查询失败：保留上一次状态 */
        });
    };
    sync();
    win
      .onResized(sync)
      .then((fn) => {
        if (alive) unlisten = fn;
        else fn(); // 组件已卸载：立刻注销，避免监听泄漏
      })
      .catch(() => {
        /* 注册失败：仅失去实时同步，按钮点击仍可用 */
      });
    return () => {
      alive = false;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <>
      {/* [lc-1703] 二值色调：data-tone 决定图标/文字用**实心纯黑**还是**实心纯白**。
          采样逻辑见文件顶部 useTitlebarTone（背景图片/纯色 + 主题兜底）。
          品牌 logo 与关闭键不在 .titlebar 内部，各自由独立图层承载（关闭键同步 tone）。 */}
      <div className="titlebar" data-tone={tone} data-tauri-drag-region>
        <div className="tb-left" data-tauri-drag-region>
          {nav && onToggleSidebar && (
            <button
              className="tb-icon"
              // [lc-1708] 标记为侧栏开关：App 的「点击外部收起」会跳过它，
              // 避免 pointerdown 先收起、click 又把它切回展开。
              data-sidebar-toggle=""
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
          {/* [lc-1725] 全局搜索入口：胶囊按钮 + Ctrl/Cmd+K；不参与拖拽 */}
          {onSearchOpenItem && <SearchLauncher onOpenItem={onSearchOpenItem} disabled={searchDisabled} />}
          <div className="tb-buttons">
            <button className="tb-btn" title="最小化" onClick={() => win.minimize()}>
              <IcoMinus size={16} strokeWidth={1.8} />
            </button>
            <button
              className="tb-btn"
              title={isMaximized ? "还原" : "最大化"}
              onClick={() => win.toggleMaximize()}
            >
              {isMaximized ? (
                <IcoCopy size={15} strokeWidth={1.8} />
              ) : (
                <IcoSquare size={15} strokeWidth={1.8} />
              )}
            </button>
          </div>
        </div>
      </div>

      {/* [lc-1703] 品牌 logo：独立图层（不参与色调切换，保持原渐变/浮动）。
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

      {/* [lc-1703] 关闭键独立图层：可点击（在 .titlebar 之上），同步 data-tone
          使用同一套实心纯黑/纯白；hover 红色保留。 */}
      <div className="tb-close-layer" data-tone={tone}>
        <button className="tb-btn tb-close" title="关闭" onClick={() => win.close()}>
          <IcoXL size={16} strokeWidth={1.8} />
        </button>
      </div>
    </>
  );
}

/* ────────────────────────── 登录页 ────────────────────────── */
