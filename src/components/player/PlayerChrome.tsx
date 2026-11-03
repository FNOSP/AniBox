/* [lc-1924] mpv 播放器覆盖层 UI —— 逻辑照抄 Polaris（其 osc-cinema.lua 的自绘底栏被
 * `if false and` 关掉，注释写明「改由 mpv-overlay.html 覆盖层接管」）：
 *
 *   · 控件全部画在 DOM 里，mpv 只负责播与原生拖窗（begin-vo-dragging）；
 *   · 覆盖层窗 layered + transparent + owned by mpv → 浮在视频之上、空白区透出画面；
 *   · 默认整窗鼠标穿透（点视频 = 暂停 / 拖窗，全在 mpv 原生侧），
 *     只有指针压到 [data-chrome] 元素上才关穿透 —— Rust 经 overlay-mouse 回传光标做命中测试；
 *   · 控件 3 秒无操作自动隐藏（Polaris hideTimeout=3.0），鼠标一动即回显；
 *   · 布局取 osc-cinema 实测值：底栏下边距 100、按钮区 30、进度条高 8、按钮间距 14、顶栏 40。
 *
 * 数据：Rust 把 mpv 的 time-pos/duration/pause/volume/speed/playlist-pos/count
 * 聚合成 `mpv-pstate` 事件推来（Polaris 覆盖层同款数据源）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowLeft as IcoArrowLeft,
  Maximize as IcoMax,
  Minimize as IcoMin,
  Pause as IcoPause,
  Play as IcoPlay,
  RotateCcw as IcoBack10,
  RotateCw as IcoFwd10,
  SkipBack as IcoPrev,
  SkipForward as IcoNext,
  Sparkles as IcoShader,
  MessageSquare as IcoDm,
  Volume1 as IcoVol1,
  Volume2 as IcoVol2,
  VolumeX as IcoVolX,
} from "lucide-react";

type MpvLoad = { phase?: string; title?: string };
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
  const ignoreRef = useRef(true);
  const [loading, setLoading] = useState(true);
  const [title, setTitle] = useState("");
  const [st, setSt] = useState<PState>({});
  const [uiVisible, setUiVisible] = useState(true);
  const [seeking, setSeeking] = useState(false);
  const [seekPreview, setSeekPreview] = useState(0);
  const hideTimer = useRef<number | null>(null);
  const barRef = useRef<HTMLDivElement | null>(null);
  const hoverRef = useRef(false);

  const setIgnore = useCallback((v: boolean) => {
    if (ignoreRef.current === v) return;
    ignoreRef.current = v;
    void invoke("player_chrome_ignore", { ignore: v }).catch(() => {});
  }, []);

  /* 3 秒无操作自动隐藏（Polaris hideTimeout=3.0）；悬停底栏/拖进度/加载中不隐藏 */
  const wake = useCallback(() => {
    setUiVisible(true);
    if (hideTimer.current) window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => {
      if (hoverRef.current) return;
      setUiVisible(false);
    }, 3000);
  }, []);

  /* 命中测试：Rust 回传 mpv 侧光标位置（物理像素），压到 [data-chrome] 上才可点 */
  useEffect(() => {
    let un: (() => void) | undefined;
    listen<{ x: number; y: number }>("overlay-mouse", (e) => {
      const { x, y } = e.payload;
      if (x < 0) {
        setIgnore(true);
        wake();
        return;
      }
      const dpr = window.devicePixelRatio || 1;
      const el = document.elementFromPoint(x / dpr, y / dpr);
      setIgnore(!(el && el.closest("[data-chrome]")));
      wake();
    })
      .then((f) => {
        un = f;
      })
      .catch(() => {});
    return () => un?.();
  }, [setIgnore, wake]);

  /* 状态流 + 加载态 */
  useEffect(() => {
    const uns: Array<Promise<() => void>> = [
      listen<PState>("mpv-pstate", (e) => setSt((prev) => ({ ...prev, ...e.payload }))),
      listen<MpvLoad>("mpv-load", (e) => {
        const phase = e.payload?.phase;
        if (e.payload?.title) setTitle(e.payload.title);
        if (phase === "loaded") setLoading(false);
        else if (phase === "start") setLoading(true);
      }),
    ];
    return () => {
      uns.forEach((p) => void p.then((f) => f()).catch(() => {}));
    };
  }, []);

  useEffect(() => {
    wake();
    return () => {
      if (hideTimer.current) window.clearTimeout(hideTimer.current);
    };
  }, [wake]);

  const pos = st.pos || 0;
  const dur = st.dur || 0;
  const pct = dur > 0 ? Math.min(1, Math.max(0, (seeking ? seekPreview : pos) / dur)) : 0;
  const vol = st.volume ?? 100;
  const speed = st.speed ?? 1;

  const cmd = useCallback((name: string, args?: Record<string, unknown>) => {
    void invoke(name, args as never).catch(() => {});
  }, []);

  /** 着色器开关：读 glsl-shaders 现状 → 空则开模式A+A，非空则清空 */
  const toggleShader = useCallback(async () => {
    try {
      const cur = await invoke<string>("mpv_get_prop", { name: "glsl-shaders" });
      const on = String(cur || "").trim().length > 0;
      await invoke("mpv_script_message", {
        name: "fntv_shader",
        args: on ? ["clear"] : ["preset", "a3"],
      });
    } catch {
      /* 脚本未就绪时忽略 */
    }
  }, []);

  const setHover = (v: boolean) => {
    hoverRef.current = v;
    if (!v) wake();
  };

  /* 进度条：按下即预览、拖动实时预览、松开才 seek（避免拖动中疯狂 seek） */
  const seekFromEvent = (clientX: number) => {
    const el = barRef.current;
    if (!el || dur <= 0) return 0;
    const r = el.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    return ratio * dur;
  };
  const onTrackDown = (e: React.PointerEvent) => {
    if (dur <= 0) return;
    setSeeking(true);
    setSeekPreview(seekFromEvent(e.clientX));
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
  };
  const onTrackMove = (e: React.PointerEvent) => {
    if (!seeking) return;
    setSeekPreview(seekFromEvent(e.clientX));
  };
  const onTrackUp = (e: React.PointerEvent) => {
    if (!seeking) return;
    const t = seekFromEvent(e.clientX);
    setSeeking(false);
    cmd("mpv_seek", { seconds: t, absolute: true });
  };

  const epLabel = useMemo(() => {
    const c = st.plCount ?? 0;
    const p = st.plPos ?? -1;
    return c > 1 && p >= 0 ? `${p + 1}/${c}` : "";
  }, [st.plCount, st.plPos]);

  return (
    <div className={`player-chrome${uiVisible || loading ? "" : " hidden"}`}>
      {/* ── 顶栏：返回 + 标题 + 集数 + 全屏 ── */}
      <header className="chrome-top" data-chrome onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}>
        <button
          type="button"
          data-chrome
          className="chrome-btn"
          title="返回"
          aria-label="返回"
          onClick={() => void invoke("player_embed_leave", { stop: true }).catch(() => {})}
        >
          <IcoArrowLeft size={18} strokeWidth={1.9} />
        </button>
        <div className="chrome-title" data-chrome title={title}>
          <span className="ct-main">{title || "正在打开"}</span>
          {epLabel && <span className="ct-ep">{epLabel}</span>}
        </div>
        <div className="chrome-top-right">
          <button
            type="button"
            data-chrome
            className="chrome-btn"
            title={st.fullscreen ? "退出全屏" : "全屏"}
            onClick={() => cmd("mpv_set_ui_fullscreen", { on: !st.fullscreen })}
          >
            {st.fullscreen ? <IcoMin size={17} strokeWidth={1.9} /> : <IcoMax size={17} strokeWidth={1.9} />}
          </button>
        </div>
      </header>

      {/* ── 底栏：进度 + 时间 + 控制 ── */}
      <footer
        className="chrome-bottom"
        data-chrome
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => setHover(false)}
      >
        <div
          className="chrome-track"
          data-chrome
          onPointerDown={onTrackDown}
          onPointerMove={onTrackMove}
          onPointerUp={onTrackUp}
        >
          <div className="chrome-track-bg" ref={barRef}>
            <div className="chrome-track-fill" style={{ width: `${pct * 100}%` }} />
            <div className="chrome-track-knob" style={{ left: `${pct * 100}%` }} />
          </div>
        </div>

        <div className="chrome-row">
          <span className="chrome-time">
            {fmt(seeking ? seekPreview : pos)}
            <i>/</i>
            {fmt(dur)}
          </span>

          <div className="chrome-ctrl">
            <button type="button" data-chrome className="chrome-btn" title="上一集" onClick={() => cmd("mpv_prev_episode")}>
              <IcoPrev size={18} strokeWidth={1.9} />
            </button>
            <button
              type="button"
              data-chrome
              className="chrome-btn"
              title="后退 10 秒"
              onClick={() => cmd("mpv_seek", { seconds: -10, absolute: false })}
            >
              <IcoBack10 size={17} strokeWidth={1.8} />
            </button>
            <button
              type="button"
              data-chrome
              className="chrome-btn primary"
              title={st.paused ? "播放" : "暂停"}
              onClick={() => cmd("mpv_toggle_pause")}
            >
              {st.paused ? <IcoPlay size={20} strokeWidth={2} /> : <IcoPause size={20} strokeWidth={2} />}
            </button>
            <button
              type="button"
              data-chrome
              className="chrome-btn"
              title="前进 10 秒"
              onClick={() => cmd("mpv_seek", { seconds: 10, absolute: false })}
            >
              <IcoFwd10 size={17} strokeWidth={1.8} />
            </button>
            <button type="button" data-chrome className="chrome-btn" title="下一集" onClick={() => cmd("mpv_next_episode")}>
              <IcoNext size={18} strokeWidth={1.9} />
            </button>
          </div>

          <div className="chrome-right">
            <button
              type="button"
              data-chrome
              className="chrome-btn text"
              title="倍速（点一次 +0.25×，到 3× 归 1×）"
              onClick={() => cmd("mpv_set_rate", { rate: speed >= 3 ? 1 : Math.round((speed + 0.25) * 100) / 100 })}
            >
              {Number(speed.toFixed(2))}×
            </button>
            <button
              type="button"
              data-chrome
              className="chrome-btn"
              title="弹幕搜索 / 换源"
              onClick={() => cmd("mpv_script_message", { name: "uosc_danmaku", args: ["open_search_danmaku_menu"] })}
            >
              <IcoDm size={17} strokeWidth={1.8} />
            </button>
            <button
              type="button"
              data-chrome
              className="chrome-btn"
              title="着色器（开/关 模式A+A）"
              onClick={() => void toggleShader()}
            >
              <IcoShader size={17} strokeWidth={1.8} />
            </button>
            <div className="chrome-vol">
              <button
                type="button"
                data-chrome
                className="chrome-btn"
                title={vol > 0 ? "静音" : "取消静音"}
                onClick={() => cmd("mpv_set_volume", { volume: vol > 0 ? 0 : 100 })}
              >
                {vol <= 0 ? (
                  <IcoVolX size={17} strokeWidth={1.8} />
                ) : vol < 50 ? (
                  <IcoVol1 size={17} strokeWidth={1.8} />
                ) : (
                  <IcoVol2 size={17} strokeWidth={1.8} />
                )}
              </button>
              <input
                data-chrome
                className="chrome-vol-slider"
                type="range"
                min={0}
                max={100}
                value={Math.round(vol)}
                title={`音量 ${Math.round(vol)}`}
                onChange={(e) => cmd("mpv_set_volume", { volume: Number(e.target.value) })}
              />
            </div>
          </div>
        </div>
      </footer>

      {/* 加载态（mpv 出画前；也承担「黑屏等待期不能无反馈」） */}
      <div className={`chrome-opening${loading ? "" : " gone"}`} aria-hidden={!loading}>
        <span className="player-spin" />
        <span className="chrome-opening-title">{title || "正在打开"}</span>
      </div>
    </div>
  );
}
