/* [lc-1939] 播放器覆盖层 UI —— 交互模型逐条照抄 Polaris 的 mpv-overlay：
 *
 *   · 覆盖层窗 layered + transparent，**常开 WS_EX_TRANSPARENT（点击穿透给 mpv）**；
 *   · Rust 50ms 全局光标探针（GetCursorPos）把窗口内物理坐标 eval 给页面
 *     → document.elementFromPoint → 命中 [data-chrome-hit] 才 invoke player_chrome_ignore(false)
 *     → 松开穿透，控件可点；离开控件立刻恢复穿透。
 *   · 这条链路不依赖 WebView 的 mousemove（穿透态下根本收不到），也不依赖 mpv 的 mouse-pos
 *     （坐标是 mpv 客户区、且事件链会滞后）——lc-1924~1931 收不到点击的根因就此绕开。
 *   · 状态：mpv-pstate 事件 + 250ms invoke 快照双通道（历史版本事件到不了覆盖层，拉取兜底）。
 *   · 3 秒无操作自动隐藏（Polaris hideTimeout=3.0）；控件隐藏时 visibility:hidden
 *     自动退出 elementFromPoint 命中树，点击继续落到 mpv。
 *
 * 视觉：Polaris 的值 —— 底栏胶囊 rgba(18,18,18,.88) / 圆角 20 / 内距 4 / 按钮 34×34
 * 圆角 17、播放 40×40 圆角 20、图标 17（播放 26）/ 进度轨 7px（悬停 10）/ 时间 12px 700 /
 * 顶栏圆钮 26。全层只有底部 132px 对比渐变，**顶部没有任何遮罩或渐变**。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AudioLines,
  Layers,
  ListOrdered,
  ListVideo,
  Maximize2,
  MessageSquare,
  Minus,
  Pause,
  Pin,
  Play,
  Settings,
  SkipForward,
  Square,
  Subtitles,
  Volume1,
  Volume2,
  VolumeX,
  X,
} from "lucide-react";

type PState = {
  pos?: number;
  dur?: number;
  paused?: boolean;
  volume?: number;
  speed?: number;
  buffering?: boolean;
  plPos?: number;
  plCount?: number;
  fullscreen?: boolean;
  ontop?: boolean;
  phase?: string;
  title?: string;
};

type ProbeData = {
  inside?: boolean;
  physicalX?: number;
  physicalY?: number;
  surfacePhysicalWidth?: number;
  surfacePhysicalHeight?: number;
};

const fmt = (s?: number) => {
  const v = Math.max(0, Math.floor(s || 0));
  const h = Math.floor(v / 3600);
  const m = Math.floor((v % 3600) / 60);
  const sec = v % 60;
  const p = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${p(m)}:${p(sec)}` : `${m}:${p(sec)}`;
};

export function PlayerChrome() {
  const [st, setSt] = useState<PState>({});
  const [visible, setVisible] = useState(true);
  const [seeking, setSeeking] = useState(false);
  const [seekPreview, setSeekPreview] = useState(0);

  const hideTimer = useRef<number | null>(null);
  const hoverInteractive = useRef(false);
  const dragLock = useRef(false);
  const ignoreRef = useRef(true);
  const lastProbe = useRef({ x: -1, y: -1 });
  const trackRef = useRef<HTMLDivElement | null>(null);

  const loading = st.phase !== "loaded" && st.phase !== "ended";
  const title = st.title || "";
  const pos = st.pos || 0;
  const dur = st.dur || 0;
  const vol = st.volume ?? 100;
  const speed = st.speed ?? 1;
  const cur = seeking ? seekPreview : pos;
  const pct = dur > 0 ? Math.min(1, Math.max(0, cur / dur)) : 0;

  const cmd = useCallback((name: string, args?: Record<string, unknown>) => {
    void invoke(name, args as never).catch(() => {});
  }, []);

  /* ── 穿透开关：只在状态真变化时过 IPC ── */
  const setIgnore = useCallback((ignore: boolean) => {
    if (ignoreRef.current === ignore) return;
    ignoreRef.current = ignore;
    void invoke("player_chrome_ignore", { ignore }).catch(() => {});
  }, []);

  const hitAt = (x: number, y: number) => {
    const el = document.elementFromPoint(x, y);
    return !!(el && el.closest("[data-chrome-hit]"));
  };

  /* ── 3 秒自动隐藏 ── */
  const wake = useCallback(() => {
    setVisible(true);
    if (hideTimer.current) window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => {
      if (hoverInteractive.current || dragLock.current) return;
      setVisible(false);
    }, 3000);
  }, []);

  useEffect(() => {
    wake();
    return () => {
      if (hideTimer.current) window.clearTimeout(hideTimer.current);
    };
  }, [wake]);

  /* ── Rust 光标探针入口：物理坐标 → CSS 视口坐标（不假定 DPI 比值）── */
  useEffect(() => {
    const w = window as unknown as { __aniboxProbe?: (d: ProbeData) => void };
    w.__aniboxProbe = (data: ProbeData) => {
      if (!data) return;
      if (data.inside === false) {
        hoverInteractive.current = false;
        if (!dragLock.current) setIgnore(true);
        return;
      }
      const pw = Number(data.surfacePhysicalWidth);
      const ph = Number(data.surfacePhysicalHeight);
      const px = Number(data.physicalX);
      const py = Number(data.physicalY);
      const cw = document.documentElement.clientWidth;
      const ch = document.documentElement.clientHeight;
      if (!(pw > 0 && ph > 0 && cw > 0 && ch > 0)) return;
      const x = (px * cw) / pw;
      const y = (py * ch) / ph;
      const lp = lastProbe.current;
      if (Math.abs(x - lp.x) > 2 || Math.abs(y - lp.y) > 2) wake();
      lastProbe.current = { x, y };
      const hit = hitAt(x, y);
      hoverInteractive.current = hit;
      if (!dragLock.current) setIgnore(!hit);
    };
    return () => {
      delete w.__aniboxProbe;
    };
  }, [setIgnore, wake]);

  /* 已获得鼠标（穿透关闭）时，真实 mousemove 负责把穿透及时开回去 */
  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      wake();
      const hit = hitAt(e.clientX, e.clientY);
      hoverInteractive.current = hit;
      if (!dragLock.current) setIgnore(!hit);
    };
    document.addEventListener("mousemove", onMove);
    return () => document.removeEventListener("mousemove", onMove);
  }, [setIgnore, wake]);

  /* ── 状态：事件 + 轮询双通道 ── */
  useEffect(() => {
    const unP = listen<PState>("mpv-pstate", (e) => setSt((prev) => ({ ...prev, ...e.payload })));
    const unL = listen<{ phase?: string; title?: string }>("mpv-load", (e) => {
      setSt((prev) => ({
        ...prev,
        phase: e.payload?.phase ?? prev.phase,
        title: e.payload?.title || prev.title,
      }));
    });
    return () => {
      void unP.then((f) => f()).catch(() => {});
      void unL.then((f) => f()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    let alive = true;
    const pull = async () => {
      try {
        const s = await invoke<PState>("player_chrome_state");
        if (alive && s) setSt((prev) => ({ ...prev, ...s }));
      } catch {
        /* 播放器未起时忽略 */
      }
    };
    void pull();
    const id = window.setInterval(() => void pull(), 250);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);

  /* ── 进度条：按下预览、拖动实时预览、松开才 seek ── */
  const seekFrom = (clientX: number) => {
    const el = trackRef.current;
    if (!el || dur <= 0) return 0;
    const r = el.getBoundingClientRect();
    return Math.min(1, Math.max(0, (clientX - r.left) / r.width)) * dur;
  };
  const onTrackDown = (e: React.PointerEvent) => {
    if (dur <= 0) return;
    dragLock.current = true;
    setSeeking(true);
    setSeekPreview(seekFrom(e.clientX));
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
  };
  const onTrackMove = (e: React.PointerEvent) => {
    if (!seeking) return;
    setSeekPreview(seekFrom(e.clientX));
  };
  const onTrackUp = (e: React.PointerEvent) => {
    if (!seeking) return;
    const t = seekFrom(e.clientX);
    setSeeking(false);
    dragLock.current = false;
    cmd("mpv_seek", { seconds: t, absolute: true });
    wake();
  };

  const epLabel = useMemo(() => {
    const c = st.plCount ?? 0;
    const p = st.plPos ?? -1;
    return c > 1 && p >= 0 ? `${p + 1}/${c}` : "";
  }, [st.plCount, st.plPos]);

  const volIcon =
    vol <= 0 ? <VolumeX size={17} /> : vol < 50 ? <Volume1 size={17} /> : <Volume2 size={17} />;

  return (
    <div className={`player-chrome${visible || loading ? "" : " pc-hidden"}`}>
      <div className="pc-layer">
        <div className="pc-scrim" aria-hidden="true" />

        {/* ── 顶栏：左片名；右上窗控（[lc-1941] 返回键已移除，退出统一走右上关闭）── */}
        <header className="pc-top">
          <div className="pc-title-wrap">
            <span className="pc-title">{title || "正在打开"}</span>
            {epLabel && <span className="pc-ep">{epLabel}</span>}
          </div>
          <div className="pc-top-right" data-chrome-hit>
            <button
              type="button"
              className={`pc-round${st.ontop ? " pc-active" : ""}`}
              data-tip={st.ontop ? "取消置顶" : "置顶"}
              onClick={() => cmd("mpv_set_ontop", { on: !st.ontop })}
            >
              <Pin size={14} />
            </button>
            <button
              type="button"
              className="pc-round"
              data-tip="最小化"
              onClick={() => cmd("mpv_window_minimize")}
            >
              <Minus size={14} />
            </button>
            <button
              type="button"
              className="pc-round"
              data-tip="最大化"
              onClick={() => cmd("mpv_toggle_maximize")}
            >
              <Square size={12} />
            </button>
            <button
              type="button"
              className="pc-round pc-close"
              data-tip="关闭"
              onClick={() => cmd("player_embed_leave", { stop: true })}
            >
              <X size={14} />
            </button>
          </div>
        </header>

        {/* ── 行 1：已播 —— 进度条 —— 剩余 ── */}
        <div className="pc-progress">
          <span className="pc-time">{fmt(cur)}</span>
          <div
            ref={trackRef}
            data-chrome-hit
            className={`pc-track${seeking ? " pc-dragging" : ""}`}
            onPointerDown={onTrackDown}
            onPointerMove={onTrackMove}
            onPointerUp={onTrackUp}
            onPointerCancel={onTrackUp}
          >
            <div className="pc-fill" style={{ width: `${pct * 100}%` }} />
            <div className="pc-knob" style={{ left: `${pct * 100}%` }} />
          </div>
          <span className="pc-time pc-time-right">{fmt(Math.max(0, dur - cur))}</span>
        </div>

        {/* ── 行 2：三段 ── */}
        <div className="pc-bottom">
          <div className="pc-left">
            <div className="pc-dock" data-chrome-hit>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip="选集"
                onClick={() => cmd("mpv_script_binding", { binding: "uosc/playlist" })}
              >
                <ListVideo size={17} />
              </button>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip="字幕"
                onClick={() => cmd("mpv_script_binding", { binding: "uosc/subtitles" })}
              >
                <Subtitles size={17} />
              </button>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip="音频"
                onClick={() => cmd("mpv_script_binding", { binding: "uosc/audio" })}
              >
                <AudioLines size={17} />
              </button>
            </div>
          </div>

          <div className="pc-center">
            <div className="pc-dock pc-dock-bare" data-chrome-hit>
              <button
                type="button"
                className="pc-btn pc-text"
                data-tip="倍速（点一次 +0.25×，到 3× 归 1×）"
                onClick={() =>
                  cmd("mpv_set_rate", {
                    rate: speed >= 3 ? 1 : Math.round((speed + 0.25) * 100) / 100,
                  })
                }
              >
                {Number(speed.toFixed(2))}×
              </button>
              <button
                type="button"
                className="pc-btn pc-play"
                data-tip={st.paused ? "播放" : "暂停"}
                onClick={() => cmd("mpv_toggle_pause")}
              >
                {st.paused ? <Play size={26} /> : <Pause size={26} />}
              </button>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip="下一集"
                onClick={() => cmd("mpv_next_episode")}
              >
                <SkipForward size={17} />
              </button>
            </div>
          </div>

          <div className="pc-right">
            <div className="pc-dock" data-chrome-hit>
              <div className="pc-vol" data-chrome-hit>
                <button
                  type="button"
                  className="pc-btn pc-icon"
                  data-tip={vol > 0 ? "静音" : "取消静音"}
                  onClick={() => cmd("mpv_set_volume", { volume: vol > 0 ? 0 : 100 })}
                >
                  {volIcon}
                </button>
                <input
                  className="pc-vol-slider"
                  type="range"
                  min={0}
                  max={100}
                  value={Math.round(vol)}
                  data-tip={`音量 ${Math.round(vol)}`}
                  onChange={(e) => cmd("mpv_set_volume", { volume: Number(e.target.value) })}
                />
              </div>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip="弹幕"
                onClick={() =>
                  cmd("mpv_script_message", {
                    name: "uosc_danmaku",
                    args: ["open_search_danmaku_menu"],
                  })
                }
              >
                <MessageSquare size={17} />
              </button>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip="片头片尾"
                onClick={() => cmd("mpv_script_binding", { binding: "uosc/chapters" })}
              >
                <ListOrdered size={17} />
              </button>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip="版本"
                onClick={() => cmd("mpv_script_binding", { binding: "uosc/video" })}
              >
                <Layers size={17} />
              </button>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip="设置"
                onClick={() => cmd("mpv_script_binding", { binding: "uosc/menu-blurred" })}
              >
                <Settings size={17} />
              </button>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip={st.fullscreen ? "退出全屏" : "全屏"}
                onClick={() => cmd("mpv_set_ui_fullscreen", { on: !st.fullscreen })}
              >
                <Maximize2 size={17} />
              </button>
            </div>
          </div>
        </div>
      </div>

      {/* 加载态：mpv 出画前（Spinner + 片名），loaded 后淡出 */}
      <div className={`pc-opening${loading ? "" : " gone"}`} aria-hidden={!loading}>
        <span className="player-spin" />
        <span className="pc-opening-title">{title || "正在打开"}</span>
      </div>
    </div>
  );
}
