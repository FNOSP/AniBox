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
 *   · 返回键 invoke player_embed_leave { stop: true }（停播 + 隐藏 mpv）后卸载。
 *     卸载 effect 不再 leave —— StrictMode dev 双挂载的中间卸载不能把刚起播的会话停掉。
 *
 * [lc-1995] 可见可恢复不变量：mpv 起播要 3s+（脚本加载），这段是黑窗；任何原因导致
 * 「窗口起来了但永远不出画」（会话被停掉 / 加载失败 / 超时），本层必须给出提示与出口，
 * 绝不允许无限黑屏：8s 未 loaded → 收起 mpv 锚窗 + 「打开超时」卡（重新播放 / 返回）；
 * 提前收到 ended（stop/quit/error）同样立刻落失败态。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { ArrowLeft as IcoArrowLeft, RotateCw as IcoRotate } from "lucide-react";

type MpvLoad = { phase?: string; reason?: string };

/** [lc-1995] 起播看门狗：连续这么久没有 loaded 就认定为「起不来」。
 *  实测 mpv 全量脚本加载 ~3.0s、file-loaded ~3.3s，8s 留足余量又不会让用户干等。 */
const OPEN_TIMEOUT_MS = 8000;

export function PlayerView({
  title,
  onExit,
  onRetry,
}: {
  title: string;
  onExit: () => void;
  /** [lc-1995] 重新起播（详情页把「当前集 + 版本」闭包传上来；首页直连也提供）。 */
  onRetry?: () => void;
}) {
  /** 矩形上报的参照元素：flex-1 的纯黑视频容器（顶栏不包含在内） */
  const stageRef = useRef<HTMLDivElement | null>(null);
  /** 覆盖层根元素（退出动画用） */
  const rootRef = useRef<HTMLDivElement | null>(null);
  /** 退出只走一次（返回键 + Esc 双入口防重） */
  const exitingRef = useRef(false);
  // mpv 窗口钉上来之前容器中央的低调提示
  const [opening, setOpening] = useState(true);
  // [lc-1995] 失败态（null = 正常）：文字直接展示给用户，不留黑屏无提示
  const [fail, setFail] = useState<string | null>(null);
  /** 本次加载是否已经 loaded（决定 ended 是「播完」还是「没播起来」） */
  const loadedRef = useRef(false);
  /** 重试计数：用作看门狗 effect 的依赖，重试即重新计时 */
  const [attempt, setAttempt] = useState(0);
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

  /* 加载态 + [lc-1995] 失败兜底：
     · loaded → 淡出加载提示；
     · start（起播/切集）→ 重新显示并重新计时；
     · ended 但从未 loaded → 说明这次根本没播起来（stop/quit/error），立刻落失败态；
     · 8s 还没 loaded → 超时失败态。
     失败时收起 mpv 锚窗（它是最上层的 idle 黑窗，会把提示卡盖住）。 */
  useEffect(() => {
    loadedRef.current = false;
    let disposed = false;
    const failNow = (text: string) => {
      if (disposed) return;
      setOpening(false);
      setFail(text);
      void invoke("player_embed_park").catch(() => {});
    };
    const timer = window.setTimeout(() => {
      if (!loadedRef.current) failNow("打开超时，视频没有出画");
    }, OPEN_TIMEOUT_MS);

    let un: (() => void) | undefined;
    listen<MpvLoad>("mpv-load", (e) => {
      const phase = e.payload?.phase;
      if (phase === "loaded") {
        loadedRef.current = true;
        setFail(null);
        setOpening(false);
        // 失败态曾把 mpv 锚窗收起（player_embed_park）：真正出画后重报一次矩形，
        // Rust 的 embed_apply_rect 会把锚窗重新显示（慢网络下 8s 超时后又能接上）。
        reportRect();
      } else if (phase === "start") {
        loadedRef.current = false;
        setFail(null);
        setOpening(true);
      } else if (phase === "ended" && !loadedRef.current) {
        const reason = e.payload?.reason;
        failNow(reason === "stop" || reason === "quit" ? "播放已停止" : "播放未能开始");
      }
    })
      .then((f) => {
        un = f;
      })
      .catch(() => {
        // 非 Tauri 环境：只靠超时兜底
      });

    return () => {
      disposed = true;
      window.clearTimeout(timer);
      un?.();
    };
  }, [attempt, reportRect]);

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

  /** [lc-1995] 失败卡「重新播放」：重新起播，看门狗重新计时。 */
  const retry = useCallback(() => {
    if (!onRetry) return;
    setFail(null);
    setOpening(true);
    setAttempt((n) => n + 1);
    onRetry();
  }, [onRetry]);

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
        <div
          className={`player-opening${opening && !fail ? "" : " gone"}`}
          aria-hidden={!!fail || !opening}
        >
          <span className="player-spin" />
          <span className="player-opening-text">正在打开</span>
        </div>
        {fail && (
          <div className="player-fail" role="alert">
            <p className="player-fail-text">{fail}</p>
            <div className="player-fail-acts">
              {onRetry && (
                <button type="button" className="player-fail-btn primary" onClick={retry}>
                  <IcoRotate size={14} strokeWidth={2} />
                  重新播放
                </button>
              )}
              <button type="button" className="player-fail-btn" onClick={exit}>
                返回
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
