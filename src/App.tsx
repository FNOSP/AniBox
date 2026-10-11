import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";

type Config = {
  server: string;
  username: string;
  token: string;
  mpv_path: string;
  proxy_path: string;
  nas_proxy_enabled: boolean;
};

type Lib = { guid: string; title: string; category: string };

type Item = {
  guid: string;
  title: string;
  tv_title?: string;
  parent_title?: string;
  type: string;
  poster: string;
  poster_width?: number;
  poster_height?: number;
  vote_average?: string;
  overview?: string;
  watched?: number;
  ts?: number;
  duration?: number;
  season_number?: number;
  episode_number?: number;
  air_date?: string;
};

type MpvEvent = { event: string; id?: number; data?: unknown };

function fmtTime(sec: number | null | undefined): string {
  if (!sec || !isFinite(sec)) return "--:--";
  const s = Math.floor(sec % 60);
  const m = Math.floor((sec / 60) % 60);
  const h = Math.floor(sec / 3600);
  const mm = String(m).padStart(2, "0");
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

function TitleBar({ title }: { title: string }) {
  const win = getCurrentWindow();
  return (
    <div className="titlebar" data-tauri-drag-region>
      <span className="tb-title" data-tauri-drag-region>
        {title}
      </span>
      <div className="tb-buttons">
        <button className="tb-btn" title="最小化" onClick={() => win.minimize()}>
          ─
        </button>
        <button className="tb-btn" title="最大化/还原" onClick={() => win.toggleMaximize()}>
          ▢
        </button>
        <button className="tb-btn tb-close" title="关闭" onClick={() => win.close()}>
          ✕
        </button>
      </div>
    </div>
  );
}

function LoginPage({ onDone }: { onDone: () => void }) {
  const [cfg, setCfg] = useState<Config | null>(null);
  const [server, setServer] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [mpvPath, setMpvPath] = useState("");
  const [proxyPath, setProxyPath] = useState("");
  const [showAdv, setShowAdv] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  useEffect(() => {
    invoke<Config>("get_config").then((c) => {
      setCfg(c);
      setServer(c.server);
      setUsername(c.username);
      setMpvPath(c.mpv_path);
      setProxyPath(c.proxy_path);
    });
  }, []);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setErr("");
    try {
      await invoke("save_config", {
        config: { ...cfg, server: server.trim(), username, mpv_path: mpvPath, proxy_path: proxyPath },
      });
      await invoke("login", { server: server.trim(), username, password });
      onDone();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="login-wrap">
      <form className="login-card" onSubmit={submit}>
        <h2>Fntv-Plus · Tauri2 PoC</h2>
        <p className="hint">自绘 UI · 直调飞牛 API · 内嵌 mpv</p>
        <label>服务器地址</label>
        <input
          value={server}
          onChange={(e) => setServer(e.target.value)}
          placeholder="如 http://192.168.31.170:18888"
          required
        />
        <label>用户名</label>
        <input value={username} onChange={(e) => setUsername(e.target.value)} required />
        <label>密码</label>
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          required
        />
        <button type="button" className="linklike" onClick={() => setShowAdv(!showAdv)}>
          高级设置（mpv / 代理路径）
        </button>
        {showAdv && (
          <>
            <label>mpv.exe 路径</label>
            <input value={mpvPath} onChange={(e) => setMpvPath(e.target.value)} />
            <label>Go 代理路径</label>
            <input value={proxyPath} onChange={(e) => setProxyPath(e.target.value)} />
          </>
        )}
        {err && <div className="err">{err}</div>}
        <button className="primary" disabled={busy}>
          {busy ? "登录中…" : "登 录"}
        </button>
      </form>
    </div>
  );
}

function PosterCard({
  item,
  baseUrl,
  onOpen,
}: {
  item: Item;
  baseUrl: string;
  onOpen: (item: Item) => void;
}) {
  const isPlayable = item.type === "Movie" || item.type === "Episode";
  const progress =
    item.ts && item.duration && item.duration > 0 ? Math.min(100, (item.ts / item.duration) * 100) : 0;
  return (
    <div className="card" onClick={() => onOpen(item)} title={item.title}>
      <div className="poster-box">
        {item.poster ? (
          <img
            src={`${baseUrl}/v/api/v1/sys/img${item.poster}`}
            loading="lazy"
            alt={item.title}
          />
        ) : (
          <div className="poster-empty">🎬</div>
        )}
        <div className="card-hover">
          <span>{isPlayable ? "▶ 播放" : "▶ 打开"}</span>
        </div>
        {item.vote_average && item.vote_average !== "0" && (
          <span className="rating">★ {item.vote_average}</span>
        )}
        {progress > 0 && (
          <div className="progress">
            <div style={{ width: `${progress}%` }} />
          </div>
        )}
        {item.type === "Episode" && item.season_number != null && (
          <span className="ep-badge">
            S{item.season_number}E{item.episode_number ?? ""}
          </span>
        )}
      </div>
      <div className="card-title">{item.title || item.tv_title}</div>
    </div>
  );
}

function PlayerView({
  item,
  onBack,
}: {
  item: Item;
  onBack: () => void;
}) {
  const stageRef = useRef<HTMLDivElement>(null);
  const [timePos, setTimePos] = useState(0);
  const [duration, setDuration] = useState(0);
  const [paused, setPaused] = useState(true);
  const [speed, setSpeed] = useState(1);
  const [volume, setVolume] = useState(100);
  const [scrub, setScrub] = useState<number | null>(null);
  const [err, setErr] = useState("");

  // 监听 mpv 属性事件
  useEffect(() => {
    const p = listen<MpvEvent>("mpv-event", (e) => {
      const ev = e.payload;
      if (ev.event !== "property-change") return;
      switch (ev.id) {
        case 1:
          setTimePos(typeof ev.data === "number" ? ev.data : 0);
          break;
        case 2:
          setDuration(typeof ev.data === "number" ? ev.data : 0);
          break;
        case 3:
          setPaused(!!ev.data);
          break;
        case 4:
          setSpeed(typeof ev.data === "number" ? ev.data : 1);
          break;
        case 5:
          setVolume(typeof ev.data === "number" ? ev.data : 100);
          break;
      }
    });
    return () => {
      p.then((f) => f());
    };
  }, []);

  // 占位区域尺寸 → 物理像素 bounds（视频窗口对位），随布局变化持续同步
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const apply = () => {
      const rect = el.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      invoke("player_set_bounds", {
        x: rect.left * dpr,
        y: rect.top * dpr,
        w: rect.width * dpr,
        h: rect.height * dpr,
      }).catch(() => {});
    };
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 进入播放页：显示嵌入窗口 + 起播；离开由 onBack 处理
  useEffect(() => {
    invoke("player_set_visible", { visible: true }).catch(() => {});
    invoke("play_item", { guid: item.guid }).catch((e) => setErr(String(e)));
    return () => {
      invoke("player_set_visible", { visible: false }).catch(() => {});
      invoke("player_stop").catch(() => {});
    };
  }, [item.guid]);

  const shownTime = scrub ?? timePos;

  async function back() {
    onBack();
  }

  return (
    <div className="player-page">
      <div className="player-stage" ref={stageRef} onDoubleClick={async () => {
        await getCurrentWindow().setFullscreen(!(await getCurrentWindow().isFullscreen()));
      }}>
        {err && <div className="err player-err">{err}</div>}
        {!duration && !err && <div className="player-loading">正在拉起 mpv…</div>}
      </div>
      <div className="control-bar">
        <div className="seek-row">
          <span className="time">{fmtTime(shownTime)}</span>
          <input
            type="range"
            min={0}
            max={duration || 100}
            step={1}
            value={shownTime}
            onInput={(e) => setScrub(Number((e.target as HTMLInputElement).value))}
            onPointerUp={(e) => {
              const v = Number((e.target as HTMLInputElement).value);
              setScrub(null);
              invoke("mpv_seek", { seconds: v, absolute: true }).catch(() => {});
            }}
            disabled={!duration}
          />
          <span className="time">{fmtTime(duration)}</span>
        </div>
        <div className="btn-row">
          <button onClick={back}>‹ 返回</button>
          <button className="big" onClick={() => invoke("mpv_toggle_pause").catch(() => {})}>
            {paused ? "▶" : "⏸"}
          </button>
          <button
            onClick={() => invoke("mpv_seek", { seconds: -10, absolute: false }).catch(() => {})}
          >
            −10s
          </button>
          <button
            onClick={() => invoke("mpv_seek", { seconds: 30, absolute: false }).catch(() => {})}
          >
            +30s
          </button>
          <span className="spacer" />
          <span className="ctl-label">速度</span>
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
          <span className="ctl-label">音量</span>
          <input
            type="range"
            className="vol"
            min={0}
            max={130}
            value={volume}
            onChange={(e) => invoke("mpv_set_volume", { volume: Number(e.target.value) }).catch(() => {})}
          />
          <button
            title="全屏"
            onClick={async () =>
              await getCurrentWindow().setFullscreen(!(await getCurrentWindow().isFullscreen()))
            }
          >
            ⛶
          </button>
        </div>
        <div className="now-playing">
          {item.tv_title ? `${item.tv_title} · ` : ""}
          {item.title}
          {item.air_date ? `（${item.air_date}）` : ""}
        </div>
      </div>
    </div>
  );
}

function App() {
  const [cfg, setCfg] = useState<Config | null>(null);
  const [view, setView] = useState<"login" | "main" | "player">("login");
  const [libs, setLibs] = useState<Lib[]>([]);
  const [activeLib, setActiveLib] = useState<Lib | null>(null);
  const [items, setItems] = useState<Item[]>([]);
  const [stack, setStack] = useState<{ lib: Lib; items: Item[] }[]>([]);
  const [detail, setDetail] = useState<{ item: Item; data: Record<string, unknown> } | null>(null);
  const [playItem, setPlayItem] = useState<Item | null>(null);
  const [err, setErr] = useState("");

  const baseUrl = cfg?.server ?? "";

  const openPlayer = useCallback((item: Item) => {
    setErr("");
    setPlayItem(item);
    setView("player");
  }, []);

  async function loadLibs() {
    try {
      const data = await invoke<unknown>("get_media_libs");
      const list = Array.isArray(data) ? (data as Lib[]) : [];
      setLibs(list);
      if (list.length > 0) {
        setActiveLib(list[0]);
        await loadItems(list[0].guid);
      }
    } catch (e) {
      setErr(String(e));
    }
  }

  async function loadItems(guid: string) {
    try {
      const data = await invoke<{ list: Item[]; total: number }>("get_items", {
        parentGuid: guid,
        excludeFolder: 0,
      });
      setItems(data.list ?? []);
    } catch (e) {
      setErr(String(e));
    }
  }

  async function openItem(item: Item) {
    setErr("");
    if (item.type === "Movie" || item.type === "Episode") {
      openPlayer(item);
      return;
    }
    // 容器（TvSeries / 文件夹）：拉详情做二级页
    try {
      const data = await invoke<Record<string, unknown>>("get_item_detail", { guid: item.guid });
      const children = await invoke<{ list: Item[] }>("get_items", {
        parentGuid: item.guid,
        excludeFolder: 0,
      });
      setDetail({ item, data });
      setItems(children.list ?? []);
      if (activeLib) setStack((s) => [...s, { lib: activeLib, items }]);
    } catch (e) {
      setErr(String(e));
    }
  }

  async function backToLib() {
    setDetail(null);
    if (stack.length > 0) {
      const prev = stack[stack.length - 1];
      setStack((s) => s.slice(0, -1));
      setActiveLib(prev.lib);
      setItems(prev.items);
    } else if (activeLib) {
      loadItems(activeLib.guid);
    }
  }

  useEffect(() => {
    invoke<Config>("get_config").then(async (c) => {
      setCfg(c);
      if (c.token) {
        try {
          await invoke("check_session");
          setView("main");
          loadLibs();
        } catch {
          setView("login");
        }
      } else {
        setView("login");
      }
    });
  }, []);

  if (view === "login") {
    return (
      <div className="app-shell">
        <TitleBar title="Fntv-Plus Tauri2" />
        <LoginPage
          onDone={() => {
            invoke<Config>("get_config").then((c) => {
              setCfg(c);
              setView("main");
              loadLibs();
            });
          }}
        />
      </div>
    );
  }

  if (view === "player" && playItem) {
    return (
      <div className="app-shell">
        <TitleBar title="Fntv-Plus Tauri2 · 播放" />
        <PlayerView item={playItem} onBack={() => setView("main")} />
      </div>
    );
  }

  return (
    <div className="app-shell">
      <TitleBar title="Fntv-Plus Tauri2" />
      <div className="main-row">
        <aside className="sidebar">
          <div className="side-head">媒体库</div>
          {libs.map((lib) => (
            <div
              key={lib.guid}
              className={`side-item ${activeLib?.guid === lib.guid ? "active" : ""}`}
              onClick={async () => {
                setActiveLib(lib);
                setDetail(null);
                setStack([]);
                await loadItems(lib.guid);
              }}
            >
              {lib.title}
            </div>
          ))}
          <div
            className="side-item logout"
            onClick={async () => {
              await invoke("logout");
              setView("login");
            }}
          >
            退出登录
          </div>
        </aside>
        <main className="content">
          {err && <div className="err">{err}</div>}
          {detail && (
            <div className="detail-bar">
              <button onClick={backToLib}>‹ 返回</button>
              <h3>
                {(detail.data.tvTitle as string) || detail.item.title}{" "}
                <span className="muted">
                  {detail.item.type === "TvSeries" ? `共 ${items.length} 个子项` : ""}
                </span>
              </h3>
              <p className="muted overview">{String(detail.data.overview ?? "")}</p>
            </div>
          )}
          {!detail && activeLib && (
            <div className="lib-head">
              <h3>{activeLib.title}</h3>
              <span className="muted">{items.length} 项</span>
            </div>
          )}
          <div className="grid">
            {items.map((it) => (
              <PosterCard key={it.guid} item={it} baseUrl={baseUrl} onOpen={openItem} />
            ))}
          </div>
          {items.length === 0 && <div className="empty">暂无内容</div>}
        </main>
      </div>
    </div>
  );
}

export default App;
