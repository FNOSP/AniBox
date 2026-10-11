import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { IcoHome, IcoStar, IcoGrid, IcoMovie, IcoTv, IcoLive, IcoFolder } from "./icons";
import "./App.css";

type Config = {
  server: string;
  username: string;
  token: string;
  password: string;
  auto_login: boolean;
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

type Season = {
  guid: string;
  title: string;
  season_number?: number;
  number_of_episodes?: number;
};

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

/** item 详情图片字段取值（对齐主仓库 carousel/images.ts pickImg：字符串直取；
 *  数组取首项的 path/url/字面量——新版 fnOS 实测为字符串，数组形态留作兼容） */
function pickImgStr(d: Record<string, unknown>, keys: string[]): string | undefined {
  for (const k of keys) {
    const v = d[k];
    if (v == null || v === "") continue;
    if (typeof v === "string") return v;
    if (Array.isArray(v) && v.length) {
      const first = v[0];
      if (typeof first === "string") return first;
      const obj = first as Record<string, unknown> | null;
      const p = obj?.path ?? obj?.url;
      if (typeof p === "string" && p) return p;
    }
  }
  return undefined;
}

/** 库内容加载：库根=ancestor 查询（Episode 由服务端白名单滤掉）；文件夹钻取=parent 查询。
 *  形状依据见 lib.rs get_items 注释（实测：库根传 parent_guid=库guid 返回 0 条，勿兜底）。 */
async function fetchLibItems(libGuid: string, parentGuid: string | null): Promise<MediaItem[]> {
  const d = await invoke<{ list: MediaItem[]; total: number }>("get_items", {
    ancestorGuid: parentGuid === null ? libGuid : null,
    parentGuid,
    page: 1,
  });
  return d.list ?? [];
}

/* ────────────────────────── 标题栏 ────────────────────────── */

function TitleBar({
  title,
  nav,
  sidebarOpen,
  onToggleSidebar,
  onHome,
  onRefresh,
  refreshing,
}: {
  title: string;
  nav?: boolean;
  sidebarOpen?: boolean;
  onToggleSidebar?: () => void;
  onHome?: () => void;
  onRefresh?: () => void;
  refreshing?: boolean;
}) {
  const win = getCurrentWindow();
  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="tb-left" data-tauri-drag-region>
        {nav && onToggleSidebar && (
          <button
            className="tb-icon"
            title={sidebarOpen ? "收起侧栏" : "展开侧栏"}
            onClick={onToggleSidebar}
          >
            ☰
          </button>
        )}
        {nav && onHome && (
          <button className="tb-link" onClick={onHome}>
            首页
          </button>
        )}
        {nav && onRefresh && (
          <button className="tb-icon" title="刷新首页数据" onClick={onRefresh} disabled={refreshing}>
            ⟳
          </button>
        )}
        {!nav && <span className="tb-page">{title}</span>}
      </div>
      <div className="tb-center" data-tauri-drag-region>
        <span className="tb-logo">▶</span>
        <span className="tb-brand">{title}</span>
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
            ✕
          </button>
        </div>
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
  const [remember, setRemember] = useState(false);
  const [autoLogin, setAutoLogin] = useState(false);
  const [mpvPath, setMpvPath] = useState("");
  const [proxyPath, setProxyPath] = useState("");
  const [showAdv, setShowAdv] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  // 自动登录一次性闸门：失败后停在本页报错，绝不循环重试
  const autoTried = useRef(false);

  useEffect(() => {
    invoke<Config>("get_config").then((c) => {
      setCfg(c);
      setServer(c.server);
      setUsername(c.username);
      setPassword(c.password || "");
      setRemember(!!c.password);
      setAutoLogin(c.auto_login && !!c.password);
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
        config: {
          ...cfg,
          server: server.trim(),
          username,
          password: remember ? password : "",
          auto_login: remember && autoLogin,
          mpv_path: mpvPath,
          proxy_path: proxyPath,
        },
      });
      await invoke("login", { server: server.trim(), username, password });
      onDone();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }

  // 启动自动登录：token 已由 check_session 把过关，走到登录页即代表失效；
  // 记住了密码且开了自动登录 → 用记住的凭据静默重登一次（成功直接进首页）
  useEffect(() => {
    if (autoTried.current || !cfg) return;
    if (!cfg.auto_login || !cfg.password || !cfg.server || !cfg.username) return;
    autoTried.current = true;
    setBusy(true);
    invoke("login", { server: cfg.server, username: cfg.username, password: cfg.password })
      .then(() => onDone())
      .catch((e) => {
        setErr("自动登录失败：" + String(e));
        setBusy(false);
      });
  }, [cfg, onDone]);

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
        <div className="login-opts">
          <label className="login-check">
            <input
              type="checkbox"
              checked={remember}
              onChange={(e) => {
                setRemember(e.target.checked);
                if (!e.target.checked) setAutoLogin(false);
              }}
            />
            记住密码
          </label>
          <label className="login-check">
            <input
              type="checkbox"
              checked={autoLogin}
              disabled={!remember}
              onChange={(e) => setAutoLogin(e.target.checked)}
            />
            自动登录
          </label>
        </div>
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

/* ────────────────────────── 首页全屏海报轮播（复刻 emby-crx Misty banner） ────────────────────────── */

function CarouselView({
  items,
  bds,
  logos,
  onPlay,
  onDetail,
}: {
  items: MediaItem[];
  bds: Record<string, string>;
  logos: Record<string, string>;
  onPlay: (t: PlayTarget) => void;
  onDetail: (item: MediaItem) => void;
}) {
  const [index, setIndex] = useState(0);
  const [armed, setArmed] = useState(false);
  const n = items.length;

  // 入场动画闸门：挂载后瞬间补 active，让首帧信息走 misty 同款上浮入场
  useEffect(() => {
    const t = setTimeout(() => setArmed(true), 80);
    return () => clearTimeout(t);
  }, []);

  // 8s 自动轮播（emby-crx 同款节奏；页面隐藏时不切，避免回前台跳帧）
  useEffect(() => {
    if (n < 2) return;
    const t = setInterval(() => {
      if (!document.hidden) setIndex((i) => (i + 1) % n);
    }, 8000);
    return () => clearInterval(t);
  }, [n]);

  if (n === 0) return null;
  const yearOf = (it: MediaItem) => String(it.release_date || it.air_date || "").slice(0, 4);

  return (
    <div className="mist-banner">
      <div className="mist-body" style={{ transform: `translateX(-${index * 100}%)` }}>
        {items.map((it, i) => {
          const bd = bds[it.guid];
          const meta = [
            yearOf(it),
            (it.type || "").toLowerCase() === "movie" ? "电影" : "剧集",
            it.vote_average && it.vote_average !== "0" ? `★ ${it.vote_average}` : "",
          ]
            .filter(Boolean)
            .join(" · ");
          return (
            <div key={it.guid} className={`mist-item ${i === index && armed ? "active" : ""}`}>
              {bd ? (
                <img
                  className="mist-cover"
                  src={imgUrl(bd)}
                  alt={it.title}
                  draggable={false}
                  loading={i === 0 ? "eager" : "lazy"}
                />
              ) : (
                <div
                  className="mist-cover mist-cover-fallback"
                  style={
                    it.poster
                      ? { backgroundImage: `url(${imgUrl(it.poster)})` }
                      : undefined
                  }
                />
              )}
              <div className="mist-veil" />
              <div className="mist-info">
                <div className="mist-meta">{meta}</div>
                {logos[it.guid] ? (
                  <img
                    className="mist-logo"
                    src={imgUrl(logos[it.guid])}
                    alt={it.title}
                    draggable={false}
                    loading="lazy"
                  />
                ) : (
                  <h1>{it.title}</h1>
                )}
                <div>
                  <p>{it.overview || ""}</p>
                </div>
                <div className="mist-actions">
                  <button
                    className="mist-play"
                    onClick={() => onPlay({ guid: it.guid, title: it.title })}
                  >
                    ▶ 开始播放
                  </button>
                  <button className="mist-more" onClick={() => onDetail(it)}>
                    MORE
                  </button>
                </div>
              </div>
            </div>
          );
        })}
      </div>
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

/* ────────────────────────── 首页 ────────────────────────── */

function HomeView({
  libs,
  hiddenLibs,
  onToggleHiddenLib,
  continueList,
  carousel,
  carouselBds,
  carouselLogos,
  libRows,
  onOpenLib,
  onPlay,
  onOpenItem,
}: {
  libs: Lib[];
  hiddenLibs: string[];
  onToggleHiddenLib: (guid: string) => void;
  continueList: PlayItem[];
  carousel: MediaItem[];
  carouselBds: Record<string, string>;
  carouselLogos: Record<string, string>;
  libRows: { lib: Lib; items: MediaItem[] }[];
  onOpenLib: (lib: Lib) => void;
  onPlay: (t: PlayTarget) => void;
  onOpenItem: (item: MediaItem) => void;
}) {
  const [manageOpen, setManageOpen] = useState(false);
  const shownLibs = libs.filter((l) => !hiddenLibs.includes(l.guid));
  return (
    <main className="content home-full">
      {/* 全屏 hero 轮播 + 底部媒体库条（Emby 布局） */}
      <div className="hero-zone">
        <CarouselView
          items={carousel}
          bds={carouselBds}
          logos={carouselLogos}
          onPlay={onPlay}
          onDetail={onOpenItem}
        />
        <div className="lib-strip">
          {shownLibs.map((lib) => (
            <div key={lib.guid} className="lib-strip-card" onClick={() => onOpenLib(lib)}>
              <div className="lsc-bg">
                {lib.posters && lib.posters[0] ? (
                  <img src={imgUrl(lib.posters[0])} loading="lazy" alt={lib.title} />
                ) : (
                  <div className="poster-empty">🎞</div>
                )}
              </div>
              <div className="lsc-shade" />
              <div className="lsc-title">{lib.title}</div>
              <div className="lsc-cat">{lib.category}</div>
              <button
                type="button"
                className="lsc-hide"
                title="首页不展示该库"
                onClick={(e) => {
                  e.stopPropagation();
                  onToggleHiddenLib(lib.guid);
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <div
            className={`lsc-manage ${shownLibs.length === 0 ? "always" : ""}`}
            title="管理首页媒体库展示"
            onClick={() => setManageOpen((v) => !v)}
          >
            ⚙ 管理
          </div>
        </div>
        {manageOpen && (
          <>
            <div className="lsc-pop-mask" onClick={() => setManageOpen(false)} />
            <div className="lsc-pop">
              <div className="lsc-pop-head">首页媒体库展示</div>
              {libs.map((lib) => (
                <label key={lib.guid} className="lsc-pop-row">
                  <input
                    type="checkbox"
                    checked={!hiddenLibs.includes(lib.guid)}
                    onChange={() => onToggleHiddenLib(lib.guid)}
                  />
                  <span>{lib.title}</span>
                  <span className="lsc-pop-cat">{lib.category}</span>
                </label>
              ))}
            </div>
          </>
        )}
      </div>

      {/* 继续观看 */}
      {continueList.length > 0 && (
        <section className="home-sec">
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
                    {sn != null && (
                      <span className="ep-badge">
                        S{sn}E{en ?? ""}
                      </span>
                    )}
                  </div>
                  <div className="card-title">{tvTitle || title}</div>
                  {tvTitle && <div className="card-sub">{title}</div>}
                </div>
              );
            })}
          </div>
        </section>
      )}

      {/* 按库顺序的剧集横排（隐藏的库不展示） */}
      {libRows
        .filter(({ lib }) => !hiddenLibs.includes(lib.guid))
        .map(({ lib, items }) =>
        items.length > 0 ? (
          <section className="home-sec" key={lib.guid}>
            <div className="section-head">
              <h3>{lib.title}</h3>
              <span className="muted">{items.length} 项</span>
              <span className="row-more" onClick={() => onOpenLib(lib)}>
                全部 ›
              </span>
            </div>
            <div className="hscroll">
              {items.map((it) => (
                <PosterCard key={it.guid} item={it} onOpen={onOpenItem} />
              ))}
            </div>
          </section>
        ) : null,
      )}
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
      // 虚拟库：收藏 / 全局分类浏览
      if (lib.guid === "_fav") {
        try {
          const d = await invoke<{ total: number; list: MediaItem[] | null }>("get_favorites", {
            page: 1,
          });
          setItems(d.list ?? []);
          setTotal(d.total ?? 0);
          if (!d.list?.length) setErr("还没有收藏");
        } catch (e) {
          setErr(String(e));
        }
        setLoading(false);
        return;
      }
      if (lib.guid.startsWith("_cat:")) {
        try {
          const types = lib.guid.slice(5).split(",").filter(Boolean);
          const d = await invoke<{ list: MediaItem[]; total: number }>("get_items_global", {
            typeFilter: types,
            page: 1,
          });
          setItems(d.list ?? []);
          setTotal(d.total ?? 0);
          if (!d.list?.length) setErr("该分类暂无已识别条目");
        } catch (e) {
          setErr(String(e));
        }
        setLoading(false);
        return;
      }
      const list = await fetchLibItems(lib.guid, parentGuid);
      setItems(list);
      setTotal(list.length);
      if (list.length === 0) setErr("该库暂无已识别条目");
      setLoading(false);
    },
    [lib.guid],
  );

  useEffect(() => {
    load(null);
  }, [load]);

  async function openItem(item: MediaItem) {
    // 只有未识别文件夹（Directory）继续逐层钻取；
    // TV/Movie/Episode/Video 一律进详情页 —— 已识别剧集的「季/集」由详情页走
    // season/list + episode/list 获取，item/list 白名单里根本没有 Season/Episode。
    if (item.type !== "Directory") {
      onOpenItem(item);
      return;
    }
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
  const [seasons, setSeasons] = useState<Season[]>([]);
  const [seasonIdx, setSeasonIdx] = useState(0);
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  const [err, setErr] = useState("");

  const isSeries = base.type === "TvSeries" || base.type === "TV";

  useEffect(() => {
    setData(null);
    setSeasons([]);
    setSeasonIdx(0);
    setEpisodes([]);
    invoke<Record<string, unknown>>("get_item_detail", { guid: base.guid })
      .then((d) => setData(d))
      .catch((e) => setErr(String(e)));
    // 层级：剧 → 季 → 集。episode/list 的入参是季 guid（传剧 guid 实测 data:null），
    // 所以先 season/list/{剧guid}，选中季后再 episode/list/{季guid}。
    if (isSeries) {
      invoke<Season[]>("get_seasons", { guid: base.guid })
        .then((ss) => setSeasons(Array.isArray(ss) ? ss : []))
        .catch(() => {});
    }
  }, [base.guid, isSeries]);

  useEffect(() => {
    const s = seasons[seasonIdx];
    if (!s) return;
    setEpisodes([]);
    invoke<Episode[]>("get_episodes", { guid: s.guid })
      .then((eps) => setEpisodes(Array.isArray(eps) ? eps : []))
      .catch(() => {});
  }, [seasons, seasonIdx]);

  const title = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
  const backdrop = (pick(data ?? {}, ["backdrops", "backdrop"]) as string) || base.poster || "";
  const poster = (pick(data ?? {}, ["posters", "poster"]) as string) || base.poster || "";
  const overview = String(pick(data ?? {}, ["overview"]) ?? base.overview ?? "");
  const rating = String(pick(data ?? {}, ["voteAverage", "vote_average"]) ?? base.vote_average ?? "");
  const releaseDate = String(
    pick(data ?? {}, ["releaseDate", "release_date"]) ?? base.release_date ?? "",
  );
  const numSeasons = pick(data ?? {}, ["numberOfSeasons", "number_of_seasons"]);
  const resumeAt = base.ts && base.duration && base.ts > 0 && base.ts < base.duration;

  return (
    <main className="content detail-page">
      <div
        className="hero"
        style={backdrop ? { backgroundImage: `url(${imgUrl(backdrop)})` } : undefined}
      >
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
                onClick={() =>
                  onPlay({
                    guid: base.guid,
                    title: base.title,
                    tvTitle: (data?.tvTitle as string) || base.tv_title,
                  })
                }
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
            {seasons.length > 1 && (
              <div className="season-tabs">
                {seasons.map((s, i) => (
                  <button
                    key={s.guid}
                    className={`season-tab ${i === seasonIdx ? "active" : ""}`}
                    onClick={() => setSeasonIdx(i)}
                  >
                    {s.title || `第 ${s.season_number ?? i + 1} 季`}
                  </button>
                ))}
              </div>
            )}
            <span className="muted">{episodes.length} 集</span>
          </div>
          <div className="grid ep-grid">
            {episodes.map((ep) => {
              const progress =
                ep.ts && ep.duration && ep.duration > 0 ? Math.min(100, (ep.ts / ep.duration) * 100) : 0;
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

/* ────────────────────────── 侧栏（保留现有样式） ────────────────────────── */

const SIDEBAR_CATS = [
  { key: "全部", types: [] as string[], ico: <IcoGrid />, sumKey: "total" },
  { key: "电影", types: ["Movie"], ico: <IcoMovie />, sumKey: "movie" },
  { key: "电视节目", types: ["TV"], ico: <IcoTv />, sumKey: "tv" },
  { key: "电视直播", types: ["Live"], ico: <IcoLive />, sumKey: "live" },
  { key: "其他", types: ["Video"], ico: <IcoFolder />, sumKey: "video" },
];

function Sidebar({
  activeGuid,
  collapsed,
  username,
  server,
  sum,
  favTotal,
  onOpenFav,
  onOpenCat,
  onHome,
}: {
  activeGuid: string | null;
  collapsed?: boolean;
  username?: string;
  server?: string;
  sum: Record<string, number>;
  favTotal: number | null;
  onOpenFav: () => void;
  onOpenCat: (cat: { key: string; types: string[] }) => void;
  onHome: () => void;
}) {
  const [toast, setToast] = useState("");
  const toastTimer = useRef<number | null>(null);
  // 快捷操作卡折叠态：持久化键与主仓库 embyWall.ts 相同（'1'=收起成图标行，缺省=展开）
  const [cardFold, setCardFold] = useState(
    () => localStorage.getItem("fntvSidebarActions.collapsed") === "1",
  );
  const [appVer, setAppVer] = useState("0.1.0");
  useEffect(() => {
    getVersion().then(setAppVer).catch(() => {});
  }, []);
  function setFold(v: boolean) {
    setCardFold(v);
    try {
      localStorage.setItem("fntvSidebarActions.collapsed", v ? "1" : "0");
    } catch {
      /* ignore */
    }
  }
  function showToast(msg: string) {
    setToast(msg);
    if (toastTimer.current) clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(""), 2200);
  }
  return (
    <aside className={`sidebar ${collapsed ? "collapsed" : ""}`}>
      <div className={`side-item ${activeGuid === null ? "active" : ""}`} onClick={onHome}>
        <span className="side-ico">
          <IcoHome />
        </span>{" "}
        首页
      </div>
      <div className={`side-item ${activeGuid === "_fav" ? "active" : ""}`} onClick={onOpenFav}>
        <span className="side-ico">
          <IcoStar />
        </span>{" "}
        收藏
        <span className="side-count">{favTotal ?? ""}</span>
      </div>
      <div className="side-head">分类</div>
      {SIDEBAR_CATS.map((c) => {
        const cnt = sum[c.sumKey];
        return (
          <div key={c.key} className="side-item" onClick={() => onOpenCat(c)}>
            <span className="side-ico">{c.ico}</span> {c.key}
            <span className="side-count">{typeof cnt === "number" ? cnt : ""}</span>
          </div>
        );
      })}
      {/* 底部：用户行 + 快捷操作卡 —— 结构与样式逐行复刻主仓库 embyWall.ts injectSettingsUI */}
      <div className="side-userrow">
        <div className="side-avatar">{(username || "U").slice(0, 1).toUpperCase()}</div>
        <div className="side-usermeta">
          <div className="side-user-name">{username || "未登录"}</div>
          <div className="side-user-server">{server ? server.replace(/^https?:\/\//, "") : "未连接"}</div>
        </div>
      </div>
      <div className="side-actions">
        {cardFold ? (
          <div className="sa-icons">
            <button type="button" className="sa-ico" title="设置" onClick={() => showToast("设置面板在阶段3接入")}>
              ⚙
            </button>
            <button type="button" className="sa-ico" title="观影记录" onClick={() => showToast("观影记录页在阶段3接入")}>
              🕐
            </button>
            <button type="button" className="sa-ico" title="软件反馈建议" onClick={() => showToast("反馈渠道在阶段3接入")}>
              💬
            </button>
            <button type="button" className="sa-ico sa-expand" title="展开快捷操作" onClick={() => setFold(false)}>
              ∧
            </button>
          </div>
        ) : (
          <>
            <div className="sa-head" title="收起快捷操作" onClick={() => setFold(true)}>
              收起
              <span className="sa-chev" />
            </div>
            <button type="button" className="sa-btn" onClick={() => showToast("设置面板在阶段3接入")}>
              ⚙ 设置
            </button>
            <button type="button" className="sa-btn" onClick={() => showToast("观影记录页在阶段3接入")}>
              🕐 观影记录
            </button>
            <button type="button" className="sa-btn" onClick={() => showToast("反馈渠道在阶段3接入")}>
              💬 软件反馈建议
            </button>
            <div className="sa-ver">v{appVer}</div>
          </>
        )}
      </div>
      {toast && <div className="side-toast">{toast}</div>}
    </aside>
  );
}

/* ────────────────────────── 根组件 ────────────────────────── */

function App() {
  const [view, setView] = useState<"login" | "home" | "lib" | "detail" | "player">("login");
  const [libs, setLibs] = useState<Lib[]>([]);
  const [continueList, setContinueList] = useState<PlayItem[]>([]);
  const [carousel, setCarousel] = useState<MediaItem[]>([]);
  const [carouselBds, setCarouselBds] = useState<Record<string, string>>({});
  const [carouselLogos, setCarouselLogos] = useState<Record<string, string>>({});
  const [libRows, setLibRows] = useState<{ lib: Lib; items: MediaItem[] }[]>([]);
  // 首页不展示的媒体库（仅影响首页媒体库条与按库横排；localStorage 持久化）
  const [hiddenLibs, setHiddenLibs] = useState<string[]>(() => {
    try {
      const v = JSON.parse(localStorage.getItem("fntv-hidden-libs") || "[]");
      return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
    } catch {
      return [];
    }
  });
  const toggleHiddenLib = useCallback((guid: string) => {
    setHiddenLibs((prev) => {
      const next = prev.includes(guid) ? prev.filter((g) => g !== guid) : [...prev, guid];
      try {
        localStorage.setItem("fntv-hidden-libs", JSON.stringify(next));
      } catch {
        /* ignore */
      }
      return next;
    });
  }, []);
  const [activeLib, setActiveLib] = useState<Lib | null>(null);
  const [detailItem, setDetailItem] = useState<MediaItem | null>(null);
  const [playTarget, setPlayTarget] = useState<PlayTarget | null>(null);
  const [err, setErr] = useState("");
  const [sidebarOpen, setSidebarOpen] = useState(
    () => localStorage.getItem("fntv-sidebar-open") !== "0",
  );
  const [username, setUserName] = useState("");
  const [server, setServer] = useState("");
  const [sum, setSum] = useState<Record<string, number>>({});
  const [favTotal, setFavTotal] = useState<number | null>(null);

  useEffect(() => {
    // 用户名 / 服务器地址：侧栏底部卡片展示用
    invoke<Config>("get_config")
      .then((c) => {
        setUserName(c.username);
        setServer(c.server);
      })
      .catch(() => {});
  }, []);

  // 首页数据一次性加载并常驻缓存：回首页零等待（轮播图过一会才出来的根因就是每次重拉）
  const loadHome = useCallback(async () => {
    setErr("");
    try {
      const [libData, cont, car, sumData, favData] = await Promise.all([
        invoke<Lib[]>("get_media_libs"),
        invoke<PlayItem[]>("get_continue_watching").catch(() => [] as PlayItem[]),
        invoke<{ list: MediaItem[]; total: number }>("get_carousel_items").catch(() => ({
          list: [],
          total: 0,
        })),
        invoke<Record<string, number>>("get_media_sum").catch(() => ({})),
        invoke<{ total: number }>("get_favorites", { page: 1 }).catch(() => ({ total: 0 })),
      ]);
      setSum(sumData ?? {});
      setFavTotal(typeof favData?.total === "number" ? favData.total : 0);
      const libsArr = Array.isArray(libData) ? libData : [];
      const carList = (car?.list ?? []).filter((it) => it && it.guid && it.title).slice(0, 8);
      const contArr = Array.isArray(cont) ? cont : [];
      setLibs(libsArr);
      setContinueList(contArr);
      setCarousel(carList);
      setLibRows(libsArr.map((l) => ({ lib: l, items: [] })));
      setView("home");

      // 轮播条目详情补齐（缓存到 App 层，只拉一次）：横版主图 + 简介/评分/年份回填
      Promise.allSettled(
        carList.map((it) => invoke<Record<string, unknown>>("get_item_detail", { guid: it.guid })),
      ).then((rs) => {
        const map: Record<string, string> = {};
        const lmap: Record<string, string> = {};
        const enrich: Record<string, Partial<MediaItem>> = {};
        rs.forEach((r, i) => {
          if (r.status === "fulfilled") {
            const d = r.value;
            const bd = pickImgStr(d, ["backdrops", "backdrop"]);
            if (bd) map[carList[i].guid] = bd;
            // [lc-570 同源] 飞牛自带透明 logo（item API data.logos，详情页 hero 同一字段）
            const lg = pickImgStr(d, ["logos", "logo"]);
            if (lg) lmap[carList[i].guid] = lg;
            const ov = pick(d, ["overview"]);
            const va = pick(d, ["vote_average"]);
            const rd = pick(d, ["release_date", "air_date"]);
            const patch: Partial<MediaItem> = {};
            if (ov && !carList[i].overview) patch.overview = String(ov);
            if (va && (!carList[i].vote_average || carList[i].vote_average === "0"))
              patch.vote_average = String(va);
            if (rd && !carList[i].release_date) patch.release_date = String(rd);
            if (Object.keys(patch).length) enrich[carList[i].guid] = patch;
          }
        });
        setCarouselBds(map);
        setCarouselLogos(lmap);
        if (Object.keys(enrich).length) {
          setCarousel((prev) => prev.map((it) => (enrich[it.guid] ? { ...it, ...enrich[it.guid] } : it)));
        }
      });

      // 每个库的第一页剧集（并行，按库顺序填回）
      libsArr.forEach((lib, idx) => {
        fetchLibItems(lib.guid, null).then((items) => {
          setLibRows((rows) => {
            const next = [...rows];
            if (next[idx]) next[idx] = { lib, items };
            return next;
          });
        });
      });
    } catch (e) {
      setErr(String(e));
      setView("home");
    }
  }, []);

  const play = useCallback((t: PlayTarget) => {
    setPlayTarget(t);
    setView("player");
  }, []);

  const openItem = useCallback((item: MediaItem) => {
    setDetailItem(item);
    setView("detail");
  }, []);

  const openFav = useCallback(() => {
    setActiveLib({ guid: "_fav", title: "收藏", category: "" });
    setView("lib");
  }, []);

  const openCat = useCallback((cat: { key: string; types: string[] }) => {
    setActiveLib({ guid: `_cat:${cat.types.join(",")}`, title: cat.key, category: "" });
    setView("lib");
  }, []);

  useEffect(() => {
    invoke("check_session")
      .then(() => loadHome())
      .catch(() => setView("login"));
  }, [loadHome]);

  let body: ReactNode;
  if (view === "login") {
    body = <LoginPage onDone={() => loadHome()} />;
  } else if (view === "home") {
    body = (
      <div className="main-row">
        <Sidebar
          activeGuid={null}
          collapsed={!sidebarOpen}
          username={username}
          server={server}
          sum={sum}
          favTotal={favTotal}
          onOpenFav={openFav}
          onOpenCat={openCat}
          onHome={() => loadHome()}
        />
        <HomeView
          libs={libs}
          continueList={continueList}
          carousel={carousel}
          carouselBds={carouselBds}
          carouselLogos={carouselLogos}
          libRows={libRows}
          hiddenLibs={hiddenLibs}
          onToggleHiddenLib={toggleHiddenLib}
          onOpenLib={(l) => {
            setActiveLib(l);
            setView("lib");
          }}
          onPlay={play}
          onOpenItem={openItem}
        />
      </div>
    );
  } else if (view === "lib" && activeLib) {
    body = (
      <div className="main-row">
        <Sidebar
          activeGuid={activeLib.guid}
          collapsed={!sidebarOpen}
          username={username}
          server={server}
          sum={sum}
          favTotal={favTotal}
          onOpenFav={openFav}
          onOpenCat={openCat}
          onHome={() => loadHome()}
        />
        <LibView lib={activeLib} onBackHome={() => loadHome()} onOpenItem={openItem} />
      </div>
    );
  } else if (view === "detail" && detailItem) {
    body = (
      <div className="main-row">
        <Sidebar
          activeGuid={activeLib?.guid ?? null}
          collapsed={!sidebarOpen}
          username={username}
          server={server}
          sum={sum}
          favTotal={favTotal}
          onOpenFav={openFav}
          onOpenCat={openCat}
          onHome={() => loadHome()}
        />
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
      <TitleBar
        title="Fntv-Plus Tauri2"
        nav={view !== "login"}
        sidebarOpen={sidebarOpen}
        onToggleSidebar={() =>
          setSidebarOpen((v) => {
            localStorage.setItem("fntv-sidebar-open", v ? "0" : "1");
            return !v;
          })
        }
        onHome={() => loadHome()}
        onRefresh={() => loadHome()}
      />
      {err && view !== "login" && <div className="err global-err">{err}</div>}
      {body}
    </div>
  );
}

export default App;
