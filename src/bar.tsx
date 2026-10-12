// 播放浮层控制条（独立 WS_CHILD 子窗里的独立 WebView）。
// 为什么单独开窗口：视频是原生窗、浮在主 WebView 之上，主页面里的 HTML 盖不到它 ——
// 悬停浮层条只能由「同样浮在视频之上」的另一个窗口来画（mpv/uosc 的观感：只在鼠标活动时出现、
// 浮在画面下缘、不占布局、不位移画面）。
//
// 与主页面共用同一套后端：mpv-event 是应用级事件（两边都能听），mpv_* 命令两边都能调。
import { useEffect, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import "./bar.css";

type MpvEvent = { event: string; id?: number; data?: unknown };

function fmtTime(sec: number | null | undefined): string {
  if (!sec || !isFinite(sec)) return "--:--";
  const s = Math.floor(sec % 60);
  const m = Math.floor(sec / 60);
  const h = Math.floor(m / 60);
  const mm = String(m % 60).padStart(2, "0");
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

function Bar() {
  const [duration, setDuration] = useState(0);
  const [timePos, setTimePos] = useState(0);
  const [paused, setPaused] = useState(false);
  const [speed, setSpeed] = useState(1);
  const [volume, setVolume] = useState(100);
  const [scrub, setScrub] = useState<number | null>(null);
  const idleTimer = useRef<number | null>(null);

  useEffect(() => {
    const p = listen<MpvEvent>("mpv-event", (e) => {
      const ev = e.payload;
      if (ev.event !== "property-change") return;
      if (ev.id === 1) setTimePos(typeof ev.data === "number" ? ev.data : 0);
      else if (ev.id === 2) setDuration(typeof ev.data === "number" ? ev.data : 0);
      else if (ev.id === 3) setPaused(!!ev.data);
      else if (ev.id === 4) setSpeed(typeof ev.data === "number" ? ev.data : 1);
      else if (ev.id === 5) setVolume(typeof ev.data === "number" ? ev.data : 100);
    });
    return () => {
      p.then((f) => f());
    };
  }, []);

  // 鼠标在条上活动 → 续命；静置 2.5s → 收起（主页面在视频上移动鼠标时也会把它唤起）。
  // 指针停在条上时（hovering）不收起——否则想去点进度条的路上它就消失了
  const hovering = useRef(false);
  const poke = () => {
    invoke("bar_show", { visible: true }).catch(() => {});
    if (idleTimer.current) window.clearTimeout(idleTimer.current);
    idleTimer.current = window.setTimeout(() => {
      if (!hovering.current) invoke("bar_show", { visible: false }).catch(() => {});
    }, 2500);
  };
  useEffect(() => {
    poke();
    return () => {
      if (idleTimer.current) window.clearTimeout(idleTimer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const shown = scrub ?? timePos;

  return (
    <div
      className="barbar"
      onMouseMove={poke}
      onMouseEnter={() => {
        hovering.current = true;
        poke();
      }}
      onMouseLeave={() => {
        hovering.current = false;
        poke();
      }}
    >
      <button
        className="big"
        title={paused ? "播放" : "暂停"}
        onClick={() => invoke("mpv_toggle_pause").catch(() => {})}
      >
        {paused ? "▶" : "⏸"}
      </button>
      <button title="后退 10 秒" onClick={() => invoke("mpv_seek", { seconds: -10, absolute: false }).catch(() => {})}>
        −10s
      </button>
      <span className="t">{fmtTime(shown)}</span>
      <span className="track">
        <input
          type="range"
          min={0}
          max={duration || 100}
          step={1}
          value={shown}
          disabled={!duration}
          onInput={(e) => setScrub(Number((e.target as HTMLInputElement).value))}
          onPointerUp={(e) => {
            const v = Number((e.target as HTMLInputElement).value);
            setScrub(null);
            invoke("mpv_seek", { seconds: v, absolute: true }).catch(() => {});
            poke();
          }}
        />
      </span>
      <span className="t">{fmtTime(duration)}</span>
      <button title="前进 30 秒" onClick={() => invoke("mpv_seek", { seconds: 30, absolute: false }).catch(() => {})}>
        +30s
      </button>
      <span className="label">速度</span>
      <select
        value={speed}
        onChange={(e) => invoke("mpv_set_rate", { rate: Number(e.target.value) }).catch(() => {})}
      >
        {[0.5, 0.75, 1, 1.25, 1.5, 2, 3].map((r) => (
          <option key={r} value={r}>
            {r}x
          </option>
        ))}
      </select>
      <span className="label">音量</span>
      <input
        className="vol"
        type="range"
        min={0}
        max={130}
        value={volume}
        onChange={(e) => invoke("mpv_set_volume", { volume: Number(e.target.value) }).catch(() => {})}
      />
      <button
        title="全屏"
        onClick={() => {
          // 布局切换（内嵌 ↔ 全页形态）与窗口全屏在主页面逻辑里做，这里只发信号
          emit("bar-action", { action: "fullscreen" }).catch(() => {});
          poke();
        }}
      >
        ⛶
      </button>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("bar-root") as HTMLElement).render(<Bar />);
