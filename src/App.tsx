import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
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

type Lib = { guid: string; title: string; category: string; posters?: string[] };

/** item/list（分页查询）返回的条目，snake_case */
type MediaItem = {
  guid: string;
  title: string;
  tv_title?: string;
  type?: string;
  poster?: string;
  poster_width?: number;
  poster_height?: number;
  is_favorite?: number;
  watched?: number;
  vote_average?: string;
  release_date?: string;
  overview?: string;
  season_number?: number;
  episode_number?: number;
  ts?: number;
  duration?: number;
  air_date?: string;
  media_stream?: unknown;
};

/** play/list（继续观看）返回的条目，camelCase 为主，读取时做双格式兼容 */
type PlayItem = Record<string, unknown>;

/** episode/list 返回的剧集，PlayListItem 形状（snake_case） */
type Episode = {
  guid: string;
  title: string;
  episode_number?: number;
  season_number?: number;
  poster?: string;
  ts?: number;
  duration?: number;
  air_date?: string;
  watched?: number;
};

type MpvEvent = { event: string; id?: number; data?: unknown };

type PlayTarget = { guid: string; title: string; tvTitle?: string };

function pick(o: Record<string, unknown>, keys: string[]): any {
  for (const k of keys) {
    const v = o?.[k];
    if (v !== undefined && v !== null && v !== "") return v;
  }
  return undefined;
}

function fmtTime(sec: number | null | undefined): string {
  if (!sec || !isFinite(sec)) return "--:--";
  const s = Math.floor(sec % 60);
  const m = Math.floor((sec / 60) % 60);
  const h = Math.floor(sec / 3600);
  const mm = String(m).padStart(2, "0");
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

/** 图片统一走鉴权代理（WebView 直连 NAS 不带 Cookie 会全裂图）
 *  Windows 自定义协议形态：http://fntvimg.localhost/<poster 路径>；mac: fntvimg://localhost/<path> */
function imgUrl(p?: string | null): string {
  if (!p) return "";
  const path = p.startsWith("/") ? p : `/${p}`;
  if (navigator.userAgent.includes("Windows")) return `http://fntvimg.localhost${path}`;
  return `fntvimg://localhost${path}`;
}

/* ────────────────────────── 首页数据缓存（回首页秒出，后台静默刷新） ────────────────────────── */

type CarouselItem = MediaItem & { backdrop?: string };
type HomeCache = {
  ts: number;
  libs: Lib[];
  continueList: PlayItem[];
  carousel: CarouselItem[];
};
const HOME_CACHE_KEY = "fntv-home-cache-v1";
const HOME_TTL = 30 * 60 * 1000;

function readHomeCache(): HomeCache | null {
  try {
    const raw = localStorage.getItem(HOME_CACHE_KEY);
    if (!raw) return null;
    const c = JSON.parse(raw) as HomeCache;
    if (!c || !Array.isArray(c.libs)) return null;
    return c;
  } catch {
    return null;
  }
}

function writeHomeCache(c: HomeCache): void {
  try {
    localStorage.setItem(HOME_CACHE_KEY, JSON.stringify(c));
  } catch {
    /* 配额满就放弃缓存 */
  }
}

async function loadHomeData(): Promise<HomeCache> {
  const [libData, cont, car] = await Promise.all([
    invoke<Lib[]>("get_media_libs"),
    invoke<PlayItem[]>("get_continue_watching").catch(() => [] as PlayItem[]),
    invoke<{ list: MediaItem[]; total: number }>("get_carousel_items")
      .then((d) => d?.list ?? [])
      .catch(() => [] as MediaItem[]),
  ]);
  const rawCarousel = (Array.isArray(car) ? car : [])
    .filter((it) => it && it.guid && it.title)
    .slice(0, 8);
  // 横版主图：并行拉详情的 backdrops，失败保持渐变兜底
  const backdrops = await Promise.all(
    rawCarousel.map(async (it) => {
      try {
        const d = await invoke<Record<string, unknown>>("get_item_detail", { guid: it.guid });
        return (pick(d, ["backdrops", "backdrop"]) as string) || "";
      } catch {
        return "";
      }
    }),
  );
  const carousel: CarouselItem[] = rawCarousel.map((it, i) => ({
    ...it,
    backdrop: backdrops[i] || undefined,
  }));
  return {
    ts: Date.now(),
    libs: Array.isArray(libData) ? libData : [],
    continueList: Array.isArray(cont) ? cont : [],
    carousel,
  };
}

/* ────────────────────────── 标题栏 ────────────────────────── */

function TitleBar({ title }: { title: string }) {
  const win = getCurrentWindow();
  return (
    <div className="titlebar" data-tauri-drag-region>
      <span className="tb-title" data-tauri-drag-region>
        <span className="tb-logo">▶</span>
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

/* ────────────────────────── 登录页 ────────────────────────── */

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
      <div className="login-hero">
        <div className="login-hero-glow" />
        <h1>FN·TV</h1>
        <p>自绘 UI · 直调飞牛 API · 内嵌 mpv</p>
      </div>
      <form className="login-card" onSubmit={submit}>
        <h2>登录飞牛影视</h2>
        <label>服务器地址</label>
        <input
          value={server}
          onChange={(e) => setServer(e.target.value)}
          placeholder="如 http://192.168.31.170:18888 或 100.66.1.2:18888"
          required
        />
        <label>用户名</label>
        <input value={username} onChange={(e) => setUsername(e.target.value)} required />
        <label>密码</label>
        <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} required />
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

/* ────────────────────────── 海报卡 ────────────────────────── */

function PosterCard({
  item,
  onOpen,
  subtitle,
}: {
  item: MediaItem;
  onOpen: (item: MediaItem) => void;
  subtitle?: string;
}) {
  const isPlayable = item.type === "Movie" || item.type === "Episode";
  const progress =
    item.ts && item.duration && item.duration > 0 ? Math.min(100, (item.ts / item.duration) * 100) : 0;
  return (
    <div className="card" onClick={() => onOpen(item)} title={item.title}>
      <div className="poster-box">
        {item.poster ? (
          <img src={imgUrl(item.poster)} loading="lazy" alt={item.title} />
        ) : (
          <div className="poster-empty">🎬</div>
        )}
        <div className="card-hover">
          <span className="play-pill">{isPlayable ? "▶ 播放" : "打开"}</span>
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
        {item.watched === 1 && <span className="watched-badge">✓</span>}
      </div>
      <div className="card-title">{subtitle || item.title}</div>
      {subtitle && item.title !== subtitle && <div className="card-sub">{item.title}</div>}
    </div>
  );
}

/* ────────────────────────── 首页样式4轮播（3D 旋转木马，复刻主仓库 fntv-s4） ────────────────────────── */

const ACCENTS = ["#6eb5ff", "#d69b6a", "#b08fe0", "#5fb0a8", "#e08585"];

function CarouselView({
  items,
  onPlay,
  onDetail,
}: {
  items: CarouselItem[];
  onPlay: (t: PlayTarget) => void;
  onDetail: (item: MediaItem) => void;
}) {
  const [index, setIndex] = useState(0);
  const n = items.length;

  // 5s 自动轮播；手动切换后因 index 变化自然重置计时
  useEffect(() => {
    if (n < 2) return;
    const t = setTimeout(() => setIndex((i) => (i + 1) % n), 5000);
    return () => clearTimeout(t);
  }, [index, n]);

  if (n === 0) return null;
  const cls = (i: number): string => {
    let d = i - index;
    if (d < 0) d += n;
    if (d === 0) return "active";
    if (d === 1) return "next";
    if (d === n - 1) return "prev";
    return d < n / 2 ? "far-right" : "far-left";
  };
  const yearOf = (it: MediaItem) => String(it.release_date || it.air_date || "").slice(0, 4);

  return (
    <div className="carousel-s4">
      <div className="fntv-s4-track">
        {items.map((it, i) => {
          const state = cls(i);
          const accent = ACCENTS[i % ACCENTS.length];
          const meta = [
            yearOf(it),
            (it.type || "").toLowerCase() === "movie" ? "MOVIE" : "TV SERIES",
          ]
            .filter(Boolean)
            .join(" · ");
          return (
            <div
              key={it.guid}
              className={`fntv-s4-card ${state}`}
              onClick={() => {
                if (state !== "active") setIndex(i);
              }}
            >
              <div
                className="fntv-s4-bg"
                style={
                  it.backdrop
                    ? { backgroundImage: `url(${imgUrl(it.backdrop)})` }
                    : { backgroundImage: `linear-gradient(160deg, ${accent}55, #0b1219)` }
                }
              />
              <div className="fntv-s4-info">
                <div className="meta">{meta}</div>
                <h3>{it.title}</h3>
                <div className="desc">{it.overview || ""}</div>
                <div className="fntv-s4-actions">
                  <button
                    className="fntv-s4-play"
                    onClick={(e) => {
                      e.stopPropagation();
                      onPlay({ guid: it.guid, title: it.title });
                    }}
                  >
                    开始播放
                  </button>
                  <button
                    className="fntv-s4-detail"
                    onClick={(e) => {
                      e.stopPropagation();
                      onDetail(it);
                    }}
                  >
                    更多详情
                  </button>
                </div>
              </div>
            </div>
          );
        })}
      </div>
      <div className="fntv-s4-dots">
        {items.map((it, i) => (
          <span
            key={it.guid}
            className={`fntv-s4-dot ${i === index ? "active" : ""}`}
            onClick={() => setIndex(i)}
          />
        ))}
      </div>
      <button className="fntv-s4-nav prev" onClick={() => setIndex((i) => (i - 1 + n) % n)}>
        ‹
      </button>
      <button className="fntv-s4-nav next" onClick={() => setIndex((i) => (i + 1) % n)}>
        ›
      </button>
    </div>
  );
}

/* ────────────────────────── 首页 ────────────────────────── */

function HomeView({
  home,
  homeLoading,
  refreshing,
  onRefresh,
  onOpenLib,
  onPlay,
  onDetail,
}: {
  home: HomeCache | null;
  homeLoading: boolean;
  refreshing: boolean;
  onRefresh: () => void;
  onOpenLib: (lib: Lib) => void;
  onPlay: (t: PlayTarget) => void;
  onDetail: (item: MediaItem) => void;
}) {
  const libs = home?.libs ?? [];
  const continueList = home?.continueList ?? [];
  const carousel = home?.carousel ?? [];

  if (homeLoading && !home) {
    return (
      <main className="content home">
        <div className="skel skel-carousel" />
        <div className="section-head">
          <h3>我的媒体库</h3>
        </div>
        <div className="libgrid">
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className="skel skel-libcard" />
          ))}
        </div>
      </main>
    );
  }

  return (
    <main className="content home">
      {carousel.length > 0 && (
        <CarouselView items={carousel} onPlay={onPlay} onDetail={onDetail} />
      )}
      {continueList.length > 0 && (
        <>
          <div className="section-head">
            <h3>继续观看</h3>
          </div>
          <div className="hscroll">
            {continueList.map((raw) => {
              const guid = String(pick(raw, ["guid"]) ?? "");
              const title = String(pick(raw, ["title"]) ?? "");
              const tvTitle = pick(raw, ["tvTitle", "tv_title"]) as string | undefined;
              const poster = pick(raw, ["poster"]) as string | undefined;
              const ts = Number(pick(raw, ["ts"]) ?? 0);
              const duration = Number(pick(raw, ["duration"]) ?? 0);
              const sn = pick(raw, ["seasonNumber", "season_number"]);
              const en = pick(raw, ["episodeNumber", "episode_number"]);
              const progress = duration > 0 ? Math.min(100, (ts / duration) * 100) : 0;
              return (
                <div
                  key={guid}
                  className="card continue-card"
                  onClick={() => onPlay({ guid, title, tvTitle })}
                >
                  <div className="poster-box wide">
                    {poster ? (
                      <img src={imgUrl(poster)} loading="lazy" alt={title} />
                    ) : (
                      <div className="poster-empty">🎬</div>
                    )}
                    <div className="card-hover">
                      <span className="play-pill">▶ 续播</span>
                    </div>
                    {progress > 0 && (
                      <div className="progress">
                        <div style={{ width: `${progress}%` }} />
                      </div>
                    )}
                    {sn != null && <span className="ep-badge">S{sn}E{en ?? ""}</span>}
                  </div>
                  <div className="card-title">{tvTitle || title}</div>
                  {tvTitle && <div className="card-sub">{title}</div>}
                </div>
              );
            })}
          </div>
        </>
      )}
      <div className="section-head">
        <h3>我的媒体库</h3>
        <span className="muted">{libs.length} 个库</span>
        <span className="spacer" />
        <button className="refresh-btn" onClick={onRefresh} disabled={refreshing}>
          {refreshing ? "⟳ 刷新中…" : "⟳ 刷新"}
        </button>
      </div>
      <div className="libgrid">
        {libs.map((lib) => (
          <div key={lib.guid} className="libcard" onClick={() => onOpenLib(lib)}>
            <div className="libcard-bg">
              {lib.posters && lib.posters[0] ? (
                <img src={imgUrl(lib.posters[0])} loading="lazy" alt={lib.title} />
              ) : (
                <div className="poster-empty">🎞</div>
              )}
            </div>
            <div className="libcard-shade" />
            <div className="libcard-info">
              <div className="libcard-title">{lib.title}</div>
              <div className="libcard-cat">{lib.category}</div>
            </div>
            <span className="libcard-arrow">›</span>
          </div>
        ))}
      </div>
      {libs.length === 0 && <div className="empty">没有媒体库</div>}
    </main>
  );
}

/* ────────────────────────── 媒体库浏览 ────────────────────────── */

function LibView({
  lib,
  onBackHome,
  onOpenItem,
}: {
  lib: Lib;
  onBackHome: () => void;
  onOpenItem: (item: MediaItem) => void;
}) {
  const [items, setItems] = useState<MediaItem[]>([]);
  const [total, setTotal] = useState(0);
  const [stack, setStack] = useState<{ folder: MediaItem; items: MediaItem[] }[]>([]);
  const [err, setErr] = useState("");
  const [loading, setLoading] = useState(true);

  const current = stack.length > 0 ? stack[stack.length - 1].folder : null;

  const load = useCallback(
    async (parentGuid: string | null) => {
      setLoading(true);
      setErr("");
      try {
        // 请求形态兼容探测：库根(ancestor=parent=lib) → 仅 ancestor → 仅 parent
        const candidates: Array<[string | null, string | null]> =
          parentGuid === null
            ? [
                [lib.guid, lib.guid],
                [lib.guid, null],
                [null, lib.guid],
              ]
            : [[lib.guid, parentGuid]];
        let list: MediaItem[] = [];
        let t = 0;
        let lastErr = "";
        for (const [ancestor, parent] of candidates) {
          try {
            const data = await invoke<{ list: MediaItem[]; total: number }>("get_items", {
              ancestorGuid: ancestor,
              parentGuid: parent,
              page: 1,
            });
            if ((data.list?.length ?? 0) > 0 || (data.total ?? 0) > 0) {
              list = data.list ?? [];
              t = data.total ?? 0;
              break;
            }
          } catch (e) {
            lastErr = String(e);
          }
        }
        if (list.length === 0 && lastErr) throw new Error(lastErr);
        setItems(list);
        setTotal(t);
      } catch (e) {
        setErr(String(e));
      } finally {
        setLoading(false);
      }
    },
    [lib.guid],
  );

  useEffect(() => {
    load(null);
  }, [load]);

  async function openItem(item: MediaItem) {
    if (item.type === "Movie" || item.type === "Episode") {
      onOpenItem(item);
      return;
    }
    // 容器：钻取
    if (current) setStack((s) => [...s.slice(0, -1), { folder: current, items }]);
    else setStack((s) => [...s, { folder: item, items }]);
    await load(item.guid);
  }

  async function pop() {
    const prev = stack[stack.length - 1];
    setStack((s) => s.slice(0, -1));
    setItems(prev.items);
  }

  return (
    <main className="content">
      <div className="page-head">
        <button className="back" onClick={stack.length > 0 ? pop : onBackHome}>
          ‹
        </button>
        <h3>{current ? current.title : lib.title}</h3>
        <span className="muted">{total || items.length} 项</span>
      </div>
      {err && <div className="err">{err}</div>}
      {loading && <div className="empty">加载中…</div>}
      {!loading && (
        <div className="grid">
          {items.map((it) => (
            <PosterCard key={it.guid} item={it} onOpen={openItem} />
          ))}
        </div>
      )}
      {!loading && items.length === 0 && !err && <div className="empty">暂无内容</div>}
    </main>
  );
}

/* ────────────────────────── 详情页（Netflix 风 hero） ────────────────────────── */

function DetailView({
  base,
  onPlay,
  onBack,
}: {
  base: MediaItem;
  onPlay: (t: PlayTarget) => void;
  onBack: () => void;
}) {
  const [data, setData] = useState<Record<string, unknown> | null>(null);
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  const [err, setErr] = useState("");

  useEffect(() => {
    setData(null);
    setEpisodes([]);
    invoke<Record<string, unknown>>("get_item_detail", { guid: base.guid })
      .then((d) => setData(d))
      .catch((e) => setErr(String(e)));
    if (base.type === "TvSeries" || base.type === "TV") {
      invoke<Episode[]>("get_episodes", { guid: base.guid })
        .then((eps) => setEpisodes(Array.isArray(eps) ? eps : []))
        .catch(() => {});
    }
  }, [base.guid]);

  const title = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
  const backdrop = (pick(data ?? {}, ["backdrops", "backdrop"]) as string) || base.poster || "";
  const poster = (pick(data ?? {}, ["posters", "poster"]) as string) || base.poster || "";
  const overview = String(pick(data ?? {}, ["overview"]) ?? base.overview ?? "");
  const rating = String(pick(data ?? {}, ["voteAverage", "vote_average"]) ?? base.vote_average ?? "");
  const releaseDate = String(pick(data ?? {}, ["releaseDate", "release_date"]) ?? base.release_date ?? "");
  const numSeasons = pick(data ?? {}, ["numberOfSeasons", "number_of_seasons"]);
  const isSeries = base.type === "TvSeries" || base.type === "TV";
  const resumeAt = base.ts && base.duration && base.ts > 0 && base.ts < base.duration;

  return (
    <main className="content detail-page">
      <div className="hero" style={backdrop ? { backgroundImage: `url(${imgUrl(backdrop)})` } : undefined}>
        <div className="hero-shade" />
        <button className="back hero-back" onClick={onBack}>
          ‹
        </button>
        <div className="hero-body">
          <div className="hero-poster">
            {poster ? (
              <img src={imgUrl(poster)} alt={title} />
            ) : (
              <div className="poster-empty">🎬</div>
            )}
          </div>
          <div className="hero-info">
            <h2>{title}</h2>
            <div className="meta-row">
              {rating && rating !== "0" && <span className="meta rating">★ {rating}</span>}
              {releaseDate && <span className="meta">{releaseDate.slice(0, 4)}</span>}
              {isSeries && numSeasons ? <span className="meta">{numSeasons} 季</span> : null}
              <span className="meta type-chip">{base.type}</span>
            </div>
            {overview && <p className="overview">{overview}</p>}
            <div className="hero-actions">
              <button
                className="play-big"
                onClick={() => onPlay({ guid: base.guid, title: base.title, tvTitle: (data?.tvTitle as string) || base.tv_title })}
              >
                ▶ {resumeAt ? "续播" : "立即播放"}
                {resumeAt && base.ts ? ` · ${fmtTime(base.ts)}` : ""}
              </button>
            </div>
          </div>
        </div>
      </div>
      {err && <div className="err">{err}</div>}
      {isSeries && episodes.length > 0 && (
        <>
          <div className="section-head">
            <h3>剧集</h3>
            <span className="muted">{episodes.length} 集</span>
          </div>
          <div className="grid ep-grid">
            {episodes.map((ep) => {
              const progress = ep.ts && ep.duration && ep.duration > 0 ? Math.min(100, (ep.ts / ep.duration) * 100) : 0;
              return (
                <div
                  key={ep.guid}
                  className="card"
                  onClick={() => onPlay({ guid: ep.guid, title: ep.title, tvTitle: title })}
                >
                  <div className="poster-box">
                    {ep.poster ? (
                      <img src={imgUrl(ep.poster)} loading="lazy" alt={ep.title} />
                    ) : (
                      <div className="poster-empty">🎞</div>
                    )}
                    <div className="card-hover">
                      <span className="play-pill">▶ 播放</span>
                    </div>
                    {progress > 0 && (
                      <div className="progress">
                        <div style={{ width: `${progress}%` }} />
                      </div>
                    )}
                    <span className="ep-badge">E{ep.episode_number ?? ""}</span>
                  </div>
                  <div className="card-title">{ep.title}</div>
                  {ep.air_date && <div className="card-sub">{ep.air_date}</div>}
                </div>
              );
            })}
          </div>
        </>
      )}
    </main>
  );
}

/* ────────────────────────── 播放页 ────────────────────────── */

function PlayerView({ item, onBack }: { item: PlayTarget; onBack: () => void }) {
  const stageRef = useRef<HTMLDivElement>(null);
  const [timePos, setTimePos] = useState(0);
  const [duration, setDuration] = useState(0);
  const [paused, setPaused] = useState(true);
  const [speed, setSpeed] = useState(1);
  const [volume, setVolume] = useState(100);
  const [scrub, setScrub] = useState<number | null>(null);
  const [err, setErr] = useState("");

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

  useEffect(() => {
    invoke("player_set_visible", { visible: true }).catch(() => {});
    invoke("play_item", { guid: item.guid }).catch((e) => setErr(String(e)));
    return () => {
      invoke("player_set_visible", { visible: false }).catch(() => {});
      invoke("player_stop").catch(() => {});
    };
  }, [item.guid]);

  const shownTime = scrub ?? timePos;

  return (
    <div className="player-page">
      <div
        className="player-stage"
        ref={stageRef}
        onDoubleClick={async () =>
          await getCurrentWindow().setFullscreen(!(await getCurrentWindow().isFullscreen()))
        }
      >
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
          <button onClick={onBack}>‹ 返回</button>
          <button className="big" onClick={() => invoke("mpv_toggle_pause").catch(() => {})}>
            {paused ? "▶" : "⏸"}
          </button>
          <button onClick={() => invoke("mpv_seek", { seconds: -10, absolute: false }).catch(() => {})}>
            −10s
          </button>
          <button onClick={() => invoke("mpv_seek", { seconds: 30, absolute: false }).catch(() => {})}>
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
          {item.tvTitle ? `${item.tvTitle} · ` : ""}
          {item.title}
        </div>
      </div>
    </div>
  );
}

/* ────────────────────────── 根组件 ────────────────────────── */

function App() {
  const [view, setView] = useState<"login" | "home" | "lib" | "detail" | "player">("login");
  const [home, setHome] = useState<HomeCache | null>(null);
  const [homeLoading, setHomeLoading] = useState(false);
  const [username, setUserName] = useState("");
  const [activeLib, setActiveLib] = useState<Lib | null>(null);
  const [detailItem, setDetailItem] = useState<MediaItem | null>(null);
  const [playTarget, setPlayTarget] = useState<PlayTarget | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [err, setErr] = useState("");

  // 打开首页：有缓存秒出（回首页零请求）；30 分钟内不重拉，过期/强制/启动时后台静默刷新
  const openHome = useCallback((opts?: { force?: boolean; startup?: boolean }) => {
    const force = opts?.force ?? false;
    const startup = opts?.startup ?? false;
    const cached = readHomeCache();
    if (cached) {
      setHome(cached);
      setView("home");
      if (!force && !startup && Date.now() - cached.ts < HOME_TTL) return;
      loadHomeData()
        .then((fresh) => {
          setHome(fresh);
          writeHomeCache(fresh);
        })
        .catch(() => {});
      return;
    }
    // 无缓存（首次打开/退出后重登）：骨架屏同步加载一次
    setHomeLoading(true);
    setErr("");
    setView("home");
    loadHomeData()
      .then((data) => {
        setHome(data);
        writeHomeCache(data);
      })
      .catch((e) => setErr(String(e)))
      .finally(() => setHomeLoading(false));
  }, []);

  // 手动刷新：按钮转圈，期间继续显示旧数据
  const manualRefresh = useCallback(() => {
    setRefreshing(true);
    loadHomeData()
      .then((f) => {
        setHome(f);
        writeHomeCache(f);
      })
      .catch((e) => setErr(String(e)))
      .finally(() => setRefreshing(false));
  }, []);

  const play = useCallback((t: PlayTarget) => {
    setPlayTarget(t);
    setView("player");
  }, []);

  const openItem = useCallback((item: MediaItem) => {
    setDetailItem(item);
    setView("detail");
  }, []);

  const logout = useCallback(async () => {
    try {
      localStorage.removeItem(HOME_CACHE_KEY);
      await invoke("logout");
    } finally {
      window.location.reload();
    }
  }, []);

  useEffect(() => {
    // 有会话直接进首页；check_session 内部覆盖 token 为空的情况，并顺手固化真实基址
    invoke<Record<string, unknown>>("check_session")
      .then(async (info) => {
        const uname = String(pick(info, ["name", "username", "user"]) ?? "");
        setUserName(uname);
        try {
          const c = await invoke<Config>("get_config");
          if (c.username) setUserName(c.username);
        } catch {
          /* 用户名仅展示用 */
        }
        openHome({ startup: true });
      })
      .catch(() => setView("login"));
  }, [openHome]);

  // 每 30 分钟自动后台刷新一次首页数据
  useEffect(() => {
    if (view === "login") return;
    const iv = setInterval(() => {
      loadHomeData()
        .then((f) => {
          setHome(f);
          writeHomeCache(f);
        })
        .catch(() => {});
    }, HOME_TTL);
    return () => clearInterval(iv);
  }, [view]);

  const sidebar = (
    <Sidebar
      libs={home?.libs ?? []}
      activeGuid={activeLib?.guid ?? null}
      username={username}
      view={view}
      onHome={() => openHome()}
      onLib={(l) => {
        setActiveLib(l);
        setView("lib");
      }}
      onLogout={logout}
    />
  );

  let body: ReactNode;
  if (view === "login") {
    body = <LoginPage onDone={() => openHome({ force: true })} />;
  } else if (view === "home") {
    body = (
      <div className="main-row">
        {sidebar}
        <HomeView
          home={home}
          homeLoading={homeLoading}
          refreshing={refreshing}
          onRefresh={manualRefresh}
          onOpenLib={(l) => {
            setActiveLib(l);
            setView("lib");
          }}
          onPlay={play}
          onDetail={openItem}
        />
      </div>
    );
  } else if (view === "lib" && activeLib) {
    body = (
      <div className="main-row">
        {sidebar}
        <LibView lib={activeLib} onBackHome={() => openHome()} onOpenItem={openItem} />
      </div>
    );
  } else if (view === "detail" && detailItem) {
    body = (
      <div className="main-row">
        {sidebar}
        <DetailView
          base={detailItem}
          onPlay={play}
          onBack={() => setView(activeLib ? "lib" : "home")}
        />
      </div>
    );
  } else if (view === "player" && playTarget) {
    body = (
      <PlayerView
        item={playTarget}
        onBack={() => setView(detailItem ? "detail" : activeLib ? "lib" : "home")}
      />
    );
  } else {
    body = <div className="empty">…</div>;
  }

  return (
    <div className="app-shell">
      <TitleBar title="Fntv-Plus Tauri2" />
      {err && view !== "login" && <div className="err global-err">{err}</div>}
      {body}
    </div>
  );
}

function Sidebar({
  libs,
  activeGuid,
  username,
  view,
  onHome,
  onLib,
  onLogout,
}: {
  libs: Lib[];
  activeGuid: string | null;
  username: string;
  view: string;
  onHome: () => void;
  onLib: (l: Lib) => void;
  onLogout: () => void;
}) {
  const icoOf = (c: string): string => {
    const k = (c || "").toLowerCase();
    if (k === "tv" || k === "tvseries") return "📺";
    if (k === "movie") return "🎬";
    if (k === "mix") return "🎞";
    return "🗂";
  };
  return (
    <aside className="sidebar">
      <div className="side-logo" onClick={onHome} title="首页">
        <span className="side-logo-mark">▶</span>
        <div className="side-logo-text">
          <div className="side-logo-name">FN·TV</div>
          <div className="side-logo-sub">Tauri2 Edition</div>
        </div>
      </div>
      <div className={`side-item ${view === "home" ? "active" : ""}`} onClick={onHome}>
        <span className="side-ico">🏠</span> 首页
      </div>
      <div className="side-head">媒体库</div>
      {libs.map((lib) => (
        <div
          key={lib.guid}
          className={`side-item ${activeGuid === lib.guid ? "active" : ""}`}
          onClick={() => onLib(lib)}
        >
          <span className="side-ico">{icoOf(lib.category)}</span> {lib.title}
        </div>
      ))}
      <div className="side-user">
        <div className="side-avatar">{(username || "U").slice(0, 1).toUpperCase()}</div>
        <div className="side-user-name">{username || "未登录"}</div>
      </div>
      <div className="side-item logout" onClick={onLogout}>
        <span className="side-ico">⎋</span> 退出登录
      </div>
    </aside>
  );
}

export default App;
