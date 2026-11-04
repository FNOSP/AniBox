/* [lc-1909] 软件内播放（内嵌播放页）
 *
 * playerOpenMode="app" 时的播放形态：全屏覆盖层（盖在内容区上、在 TitleBar 之下），
 * 结构 = 顶部返回栏（返回 + 媒体标题）+ 下方纯黑视频容器。控制条/弹幕都在 mpv 内
 * 由 uosc / uosc_danmaku 渲染，本层不做任何播放控件。
 *
 * 与 Rust 的分工（mpv 窗口被精确钉到视频容器矩形上）：
 *   · 挂载后 invoke player_embed_enter —— Rust 挂 owner、先隐藏 mpv 窗口；
 *     hwnd 未就绪时 Rust 后台自行重试，调用方无需等待。
 *   · 视频容器矩形（getBoundingClientRect × devicePixelRatio = 物理像素，
 *     相对窗口内容区 (0,0)）在这些时机上报 player_embed_rect：
 *     首帧、ResizeObserver、窗口 onMoved / onResized / onScaleChanged。
 *     拖拽期间 Moved 连发，直接在回调里 invoke（Rust 侧幂等轻量）。
 *   · 返回键 invoke player_embed_leave { stop: true }（停播 + 隐藏 mpv）后卸载。
 *     卸载 effect 不再 leave —— StrictMode dev 双挂载的中间卸载不能把刚起播的会话停掉。
 *
 * 加载提示：容器中央「正在打开…」在 mpv-load{phase:"loaded"}（或 3s 兜底）后淡出；
 * 切集（phase:"start"）时重新显示。事件结构来自 mpv.rs（start / loaded / ended）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { ArrowLeft as IcoArrowLeft } from "lucide-react";

type MpvLoad = { phase?: string };

export function PlayerView({ title, onExit }: { title: string; onExit: () => void }) {
  /** 矩形上报的参照元素：flex-1 的纯黑视频容器（顶栏不包含在内） */
  const stageRef = useRef<HTMLDivElement | null>(null);
  /** 覆盖层根元素（退出动画用） */
  const rootRef = useRef<HTMLDivElement | null>(null);
  /** 退出只走一次（返回键 + Esc 双入口防重） */
  const exitingRef = useRef(false);
  // mpv 窗口钉上来之前容器中央的低调提示
  const [opening, setOpening] = useState(true);
  // StrictMode dev 双挂载去重：enter 只发一次（Rust 侧幂等，重复无害，但不浪费）
  const enteredRef = useRef(false);

  /** 容器矩形上报：CSS px × devicePixelRatio = 物理像素，相对内容区 (0,0)。
   *  getBoundingClientRect 本就以视口（decorations:false → 内容区=整窗）为原点，直接可用。
   *  容器尺寸为 0（尚未布局/被折叠）时跳过。 */
  const reportRect = useCallback(() => {
    const el = stageRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return;
    const dpr = window.devicePixelRatio || 1;
    void invoke("player_embed_rect", {
      x: Math.round(r.left * dpr),
      y: Math.round(r.top * dpr),
      width: Math.round(r.width * dpr),
      height: Math.round(r.height * dpr),
    }).catch(() => {
      /* 上报失败不影响播放；下一轮时机再报 */
    });
  }, []);

  // 进入内嵌会话（先于首帧矩形也行：Rust 会先隐藏 mpv，矩形后补施）
  useEffect(() => {
    if (enteredRef.current) return;
    enteredRef.current = true;
    void invoke("player_embed_enter").catch(() => {});
  }, []);

  // 矩形上报时机：挂载后首帧 + ResizeObserver + 窗口 Moved/Resized/ScaleFactorChanged
  useEffect(() => {
    reportRect();
    // 首帧布局稳定后再补一次（覆盖字体/过渡引起的微调）
    const raf = window.requestAnimationFrame(reportRect);
    const ro = new ResizeObserver(reportRect);
    if (stageRef.current) ro.observe(stageRef.current);
    const win = getCurrentWindow();
    const un: Array<Promise<() => void>> = [
      win.onMoved(() => reportRect()),
      win.onResized(() => reportRect()),
      // Tauri 2 的缩放事件名：onScaleChanged（DPI/缩放系数变化 → dpr 变了，矩形要重报）
      win.onScaleChanged(() => reportRect()),
    ];
    return () => {
      window.cancelAnimationFrame(raf);
      ro.disconnect();
      un.forEach((p) => void p.then((f) => f()).catch(() => {}));
    };
  }, [reportRect]);

  // 加载提示：loaded → 淡出；start（起播/切集）→ 重新显示；3s 兜底淡出（加载慢时容器保持纯黑）
  useEffect(() => {
    const t = window.setTimeout(() => setOpening(false), 3000);
    let un: (() => void) | undefined;
    listen<MpvLoad>("mpv-load", (e) => {
      const phase = e.payload?.phase;
      if (phase === "loaded") setOpening(false);
      else if (phase === "start") setOpening(true);
    })
      .then((f) => {
        un = f;
      })
      .catch(() => {
        /* 非 Tauri 环境：只靠 3s 兜底 */
      });
    return () => {
      window.clearTimeout(t);
      un?.();
    };
  }, []);

  // [lc-1913] Rust 侧 ESC 退出桥完成收尾（停播+隐藏+还原）后通知卸载 —— 不再 invoke leave
  useEffect(() => {
    let un: (() => void) | undefined;
    listen("mpv-embed-exit", () => {
      exitingRef.current = true;
      onExit();
    })
      .then((f) => {
        un = f;
      })
      .catch(() => {});
    return () => un?.();
  }, [onExit]);

  // 返回：停播 + 退嵌（Rust 清内嵌状态并隐藏 mpv），然后关闭播放层回详情页。
  // 先给覆盖层加 .leaving 走 200ms 退出动画，再真正卸载（onExit 由父层延迟触发）。
  const exit = useCallback(() => {
    if (exitingRef.current) return;
    exitingRef.current = true;
    void invoke("player_embed_leave", { stop: true }).catch(() => {});
    const el = rootRef.current;
    if (el && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      el.classList.add("leaving");
      window.setTimeout(onExit, 200);
    } else {
      onExit();
    }
  }, [onExit]);

  // Esc = 返回
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") exit();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [exit]);

  return (
    <div className="player-view" role="dialog" aria-label="播放" ref={rootRef}>
      <div className="player-topbar">
        <button
          type="button"
          className="player-back"
          title="返回"
          aria-label="返回"
          onClick={exit}
        >
          <IcoArrowLeft size={16} strokeWidth={1.8} />
        </button>
        <span className="player-title">{title}</span>
      </div>
      <div className="player-stage" ref={stageRef}>
        <div className={`player-opening${opening ? "" : " gone"}`} aria-hidden={!opening}>
          <span className="player-spin" />
          <span className="player-opening-text">正在打开</span>
        </div>
      </div>
    </div>
  );
}
