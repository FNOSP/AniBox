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
  /** item/{guid} 详情回填（原版剧集行有单集简介） */
  overview?: string;
  /** media_stream.resolutions[0] 去 p 尾（如 1080），原版标题旁的清晰度小徽标 */
  quality?: string;
  media_stream?: { resolutions?: string[] } | null;
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

/** 时长口语化（原版剧集行样式）：1420 → 「23 分钟 40 秒」 */
function fmtDur(sec: number | null | undefined): string {
  if (!sec || !isFinite(sec) || sec <= 0) return "";
  const m = Math.floor(sec / 60);
  const s = Math.round(sec % 60);
  if (m <= 0) return `${s} 秒`;
  return s > 0 ? `${m} 分钟 ${s} 秒` : `${m} 分钟`;
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
  onToggleTheme,
  overlay,
  ghost,
}: {
  title: string;
  nav?: boolean;
  sidebarOpen?: boolean;
  onToggleSidebar?: () => void;
  onHome?: () => void;
  onRefresh?: () => void;
  refreshing?: boolean;
  onToggleTheme?: () => void;
  /** 悬浮在海报/视频上时：白字 + 顶部压暗渐变 */
  overlay?: boolean;
  /** 透明但保持主题色文字（详情页：顶栏下无穿层内容） */
  ghost?: boolean;
}) {
  const win = getCurrentWindow();
  return (
    <div className={`titlebar${overlay ? " overlay" : ""}${ghost ? " ghost" : ""}`} data-tauri-drag-region>
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
          <button
            className={`tb-icon${refreshing ? " spinning" : ""}`}
            title="刷新首页数据"
            onClick={onRefresh}
            disabled={refreshing}
          >
            ⟳
          </button>
        )}
        {onToggleTheme && (
          <button className="tb-icon" title="切换深浅模式" onClick={onToggleTheme}>
            ◐
          </button>
        )}
        {!nav && <span className="tb-page">{title}</span>}
      </div>
      <div className="tb-center" data-tauri-drag-region>
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
          <>
            {/* [lc-114] 骨架屏放在图片**下层**（纯 CSS 层叠）。此前用 opacity 切换且依赖
                onLoad，而图片命中 WebView 缓存时常在 React 挂监听前就 complete ⇒ 回调不触发、
                图片永久 opacity:0 ⇒ "图片出不来"。层叠写法没有这个竞态。 */}
            <div className="poster-skeleton" />
            <img src={imgUrl(item.poster)} loading="lazy" alt={item.title} />
          </>
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

/* ────────────────────────── 首页骨架屏 ────────────────────────── */

function HomeSkeleton() {
  return (
    <main className="content home-full">
      <div className="skel hero-skel" />
      <div className="skel-row">
        {Array.from({ length: 5 }).map((_, i) => (
          <div key={i} className="skel lsc-skel" />
        ))}
      </div>
      {[0, 1].map((r) => (
        <section className="home-sec" key={r}>
          <div className="skel sec-title-skel" />
          <div className="skel-row">
            {Array.from({ length: 6 }).map((_, i) => (
              <div key={i} className="skel card-skel" />
            ))}
          </div>
        </section>
      ))}
    </main>
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
  onScrolled,
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
  /** 首页滚动状态上报：滚动后顶栏从 overlay 透明切回实底 */
  onScrolled: (scrolled: boolean) => void;
}) {
  const [manageOpen, setManageOpen] = useState(false);
  const shownLibs = libs.filter((l) => !hiddenLibs.includes(l.guid));
  return (
    <main
      className="content home-full"
      onScroll={(e) => onScrolled(e.currentTarget.scrollTop > 8)}
    >
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

function DetailView({ base, onBack }: { base: MediaItem; onBack: () => void }) {
  // 内嵌播放：点集/播放按钮后在页内 mpv 播放（离开详情页自动 stop）
  const [data, setData] = useState<Record<string, unknown> | null>(null);
  const [seasons, setSeasons] = useState<Season[]>([]);
  const [seasonIdx, setSeasonIdx] = useState(0);
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  // [lc-085] 视频全屏（mpv 双击/全屏键触发）：内嵌形态 ↔ 全页播放器形态 + 窗口全屏
  const [tmdb, setTmdb] = useState<Record<string, unknown> | null>(null);
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
      .then((eps) =>
        setEpisodes(
          (Array.isArray(eps) ? eps : []).map((e) => ({
            ...e,
            quality:
              Array.isArray(e?.media_stream?.resolutions) && e.media_stream.resolutions[0]
                ? String(e.media_stream.resolutions[0]).replace(/p$/i, "")
                : undefined,
          })),
        ),
      )
      .catch(() => {});
  }, [seasons, seasonIdx]);

  // TMDB 完整详情（剧照/相似/更多/演职人员）：主仓库 fetchShowDetails 同款链路，失败静默降级
  const tmdbTitle = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
  useEffect(() => {
    setTmdb(null);
    if (!tmdbTitle) return;
    let alive = true;
    invoke<Record<string, unknown>>("tmdb_show_details", {
      arg: {
        title: tmdbTitle,
        year: String(base.release_date || "").slice(0, 4),
        mediaType: base.type === "Movie" ? "movie" : "tv",
      },
    })
      .then((d) => {
        if (alive && d && !d.error) setTmdb(d);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [base.guid, tmdbTitle]);

  // 单集简介回填：原版剧集行带简介，episode/list 不含该字段，逐集拉 item 详情补（与轮播同一模式）
  useEffect(() => {
    if (!episodes.length) return;
    let alive = true;
    Promise.allSettled(
      episodes.map((ep) => invoke<Record<string, unknown>>("get_item_detail", { guid: ep.guid })),
    ).then((rs) => {
      if (!alive) return;
      setEpisodes((prev) =>
        prev.map((ep, i) => {
          const r = rs[i];
          if (r.status !== "fulfilled") return ep;
          const ov = pick(r.value, ["overview"]);
          if (!ov || ep.overview) return ep;
          return { ...ep, overview: String(ov) };
        }),
      );
    });
    return () => {
      alive = false;
    };
  }, [episodes.length]);

  const title = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
  const poster = (pick(data ?? {}, ["posters", "poster"]) as string) || base.poster || "";
  const overview = String(pick(data ?? {}, ["overview"]) ?? base.overview ?? "");
  const rating = String(pick(data ?? {}, ["voteAverage", "vote_average"]) ?? base.vote_average ?? "");
  const releaseDate = String(
    pick(data ?? {}, ["releaseDate", "release_date"]) ?? base.release_date ?? "",
  );
  const numSeasons = pick(data ?? {}, ["numberOfSeasons", "number_of_seasons"]);
  const resumeAt = base.ts && base.duration && base.ts > 0 && base.ts < base.duration;


  // [lc-106] 独立 mpv 窗口模式：点播放直接拉起独立 mpv（与主仓库一致），页面留在详情页
  const [playNote, setPlayNote] = useState("");
  const [playing, setPlaying] = useState(false);
  const [playingGuid, setPlayingGuid] = useState<string | null>(null);
  const startPlay = async () => {
    const ep = episodes[0];
    const guid = isSeries && ep ? ep.guid : base.guid;
    setPlaying(true);
    setPlayNote("正在打开独立播放器…");
    try {
      await invoke("play_item", { guid });
      setPlayNote("已在独立播放器窗口播放（可回到本页继续浏览）");
    } catch (e) {
      setPlayNote(String(e));
    } finally {
      setPlaying(false);
    }
  };

  return (
    <main className="content detail-full">
      <div className="detail-headrow">
        <button className="back" onClick={onBack} title="返回上一页">
          ‹ 返回
        </button>
        <h1 className="detail-title">{title}</h1>
        {rating && rating !== "0" && <span className="meta meta-rating detail-rating">★ {rating}</span>}
        {/* [lc-107] 视频占位框已删除（独立 mpv 窗口模式，页面不留视频区）：播放入口收进标题行 */}
        <button className="btn-play-head" onClick={playing ? undefined : startPlay} disabled={playing}>
          {playing ? "⏳ 打开中…" : `▶ ${resumeAt ? "续播" : "播放"}`}
        </button>
      </div>
      {playNote && <div className="play-note">{playNote}</div>}
      <div className="detail-cols">
        <div className="detail-left">
          {err && <div className="err">{err}</div>}
          <div className="tmdb-card">
            <div className="tmdb-poster">
              {poster ? <img src={imgUrl(poster)} alt={title} /> : <div className="poster-empty">🎬</div>}
            </div>
            <div className="tmdb-info">
              <div className="tmdb-name">{title}</div>
              <div className="meta-row">
                {releaseDate && <span className="meta">{releaseDate.slice(0, 4)}</span>}
                {isSeries && numSeasons ? <span className="meta">{numSeasons} 季</span> : null}
                <span className="meta type-chip">{base.type}</span>
              </div>
              {overview && <p className="tmdb-overview">{overview}</p>}
            </div>
          </div>
          {tmdb && <TmdbExtra tmdb={tmdb} />}
        </div>
        {isSeries && (
          <div className="detail-right">
            <div className="section-head">
              <h3>选集</h3>
              {seasons.length > 1 && (
                <div className="season-tabs">
                  {seasons.map((sn, i) => (
                    <button
                      key={sn.guid}
                      className={`season-tab ${i === seasonIdx ? "active" : ""}`}
                      onClick={() => setSeasonIdx(i)}
                    >
                      {sn.title || `第 ${sn.season_number ?? i + 1} 季`}
                    </button>
                  ))}
                </div>
              )}
              <span className="muted">{episodes.length} 集</span>
            </div>
            <div className="ep-list">
              {episodes.map((ep) => {
                const progress =
                  ep.ts && ep.duration && ep.duration > 0 ? Math.min(100, (ep.ts / ep.duration) * 100) : 0;
                const label =
                  ep.episode_number != null
                    ? `第 ${ep.episode_number} 集${ep.title ? `. ${ep.title}` : ""}`
                    : ep.title || "未命名剧集";
                const playing = playingGuid === ep.guid;
                return (
                  <div
                    key={ep.guid}
                    className={`ep-row${playing ? " playing" : ""}`}
                    onClick={() => {
                      setPlayingGuid(ep.guid);
                      invoke("play_item", { guid: ep.guid }).catch((e) => setErr(String(e)));
                    }}
                  >
                    <div className="ep-thumb">
                      {ep.poster ? (
                        <img src={imgUrl(ep.poster)} loading="lazy" alt={label} />
                      ) : (
                        <div className="poster-empty">🎞</div>
                      )}
                      <span className="ep-play">{playing ? "◉" : "▶"}</span>
                      {progress > 0 && (
                        <div className="progress">
                          <div style={{ width: `${progress}%` }} />
                        </div>
                      )}
                    </div>
                    <div className="ep-body">
                      <div className="ep-row-title">
                        {label}
                        {ep.quality && <span className="ep-quality">{ep.quality}</span>}
                      </div>
                      {ep.overview && <p className="ep-overview">{ep.overview}</p>}
                      {fmtDur(ep.duration) && <div className="ep-duration">{fmtDur(ep.duration)}</div>}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </div>
    </main>
  );
}


/* ────────────────────────── TMDB 扩展信息（剧照/相似/更多/演职人员） ──────────────────────────
   复刻主仓库 embyWall/detail/tmdbCard.ts ⑥⑧⑨⑩ 四节；剧照走 fntvimg 代理（TMDB 原图 URL 的 path 部分不可用，
   改用 image.tmdb.org 直链经 imgproto？——不行，imgproto 只映射 NAS sys/img。这里用 TMDB w500 直链 <img>，
   WebView 可直连 TMDB 图片 CDN（无鉴权）。 */

function TmdbExtra({ tmdb }: { tmdb: Record<string, unknown> }) {
  const arr = (k: string): Record<string, unknown>[] =>
    Array.isArray(tmdb[k]) ? (tmdb[k] as Record<string, unknown>[]) : [];
  const strList = (k: string): string[] => (Array.isArray(tmdb[k]) ? (tmdb[k] as string[]) : []);
  const stills = arr("backdrops").slice(0, 6);
  const recs = arr("recommendations").slice(0, 8);
  const cast = arr("cast").slice(0, 20);
  const aliases = strList("aliases");
  const keywords = strList("keywords");
  const providers = strList("providers");
  const popularity = Number(tmdb.popularity ?? 0);
  const tmdbId = String(tmdb.tmdbId ?? "");
  const imdb = String((tmdb.externalIds as Record<string, unknown> | undefined)?.imdb ?? "");
  const wiki = String((tmdb.externalIds as Record<string, unknown> | undefined)?.wikidata ?? "");
  const hasMore = aliases.length || keywords.length || providers.length || popularity > 0;
  const links = [
    { label: "TMDB", url: tmdbId ? `https://www.themoviedb.org/tv/${tmdbId}` : "" },
    { label: "IMDb", url: imdb ? `https://www.imdb.com/title/${imdb}` : "" },
    { label: "Wikidata", url: wiki ? `https://www.wikidata.org/wiki/${wiki}` : "" },
  ].filter((l) => l.url);
  return (
    <>
      {stills.length > 0 && (
        <div className="tmdb-sec">
          <div className="tmdb-sec-t">剧照</div>
          <div className="tmdb-stills">
            {stills.map((st) => (
              <img key={String(st.file_path ?? st)} src={`https://image.tmdb.org/t/p/w500${st.file_path ?? st}`} loading="lazy" alt="" />
            ))}
          </div>
        </div>
      )}
      {recs.length > 0 && (
        <div className="tmdb-sec">
          <div className="tmdb-sec-t">相似剧集</div>
          <p className="tmdb-inline">
            {recs.map((r, i) => (
              <span key={String(r.tmdbId ?? i)}>
                {i > 0 && <i> · </i>}
                {String(r.title)}
                {r.year ? ` (${r.year})` : ""}
              </span>
            ))}
          </p>
        </div>
      )}
      {hasMore && (
        <div className="tmdb-sec">
          <div className="tmdb-sec-t">更多</div>
          {aliases.length > 0 && (
            <div className="tmdb-mrow"><span className="tmdb-mk">别名</span><span className="tmdb-inline">{aliases.join(" · ")}</span></div>
          )}
          {keywords.length > 0 && (
            <div className="tmdb-mrow"><span className="tmdb-mk">关键词</span><span className="tmdb-inline">{keywords.join(" · ")}</span></div>
          )}
          {providers.length > 0 && (
            <div className="tmdb-mrow"><span className="tmdb-mk">在线看</span><span className="tmdb-inline">{providers.join(" · ")}</span></div>
          )}
          {popularity > 0 && (
            <div className="tmdb-mrow"><span className="tmdb-mk">热度</span><span className="tmdb-inline">{Math.round(popularity).toLocaleString("zh-CN")}</span></div>
          )}
          <div className="tmdb-mrow">
            <span className="tmdb-mk">编号</span>
            <span className="tmdb-inline">
              {tmdbId && `TMDB ${tmdbId}`}
              {imdb && ` · Q${wiki.replace(/^Q/, "")}` && wiki && ` · ${wiki}`}
              {imdb && ` · ${imdb}`}
            </span>
          </div>
          {links.length > 0 && (
            <div className="tmdb-links">
              {links.map((l) => (
                <a key={l.label} href={l.url} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>
                  {l.label}
                </a>
              ))}
            </div>
          )}
        </div>
      )}
      {cast.length > 0 && (
        <div className="tmdb-sec">
          <div className="tmdb-sec-t">演职人员</div>
          <div className="tmdb-cast">
            {cast.map((c, i) => (
              <div key={`${c.name}-${i}`} className="tmdb-cast-row">
                {c.profile ? (
                  <img className="tmdb-cast-ava" src={`https://image.tmdb.org/t/p/w185${c.profile}`} loading="lazy" alt="" />
                ) : (
                  <span className="tmdb-cast-ava tmdb-cast-empty">👤</span>
                )}
                <span className="tmdb-cast-name">{String(c.name)}</span>
                {c.character ? <span className="tmdb-cast-role">饰 {String(c.character)}</span> : null}
              </div>
            ))}
          </div>
        </div>
      )}
    </>
  );
}

/* ────────────────────────── 播放页 ────────────────────────── */

function PlayerView({
  item,
  onBack,
  embedded,
  onBig,
}: {
  item: PlayTarget;
  onBack?: () => void;
  embedded?: boolean;
  onBig?: (on: boolean) => void;
}) {
  const stageRef = useRef<HTMLDivElement>(null);
  const [err, setErr] = useState("");
  // [lc-105] 页面内播放（B站/爱奇艺式真内嵌）：优先把代理直链交给页面里的 <video>，
  // 视频成为 DOM 的一部分（随滚动/圆角/裁剪，HTML 控件随便盖，无窗口套窗口、无挖洞）。
  // 浏览器播不了的容器（MKV/HEVC 等）→ onError → htmlFailed → 自动回退到 mpv 路径。
  // 想强制用 mpv：localStorage["fntv-player"]="mpv"。
  const [htmlSrc, setHtmlSrc] = useState<string | null>(null);
  const [htmlFailed, setHtmlFailed] = useState(false);
  const FORCE_MPV =
    typeof localStorage !== "undefined" && localStorage.getItem("fntv-player") === "mpv";
  const htmlMode = !FORCE_MPV && !!htmlSrc && !htmlFailed;
  useEffect(() => {
    let alive = true;
    setHtmlSrc(null);
    setHtmlFailed(false);
    if (FORCE_MPV) return;
    invoke<{ url: string; guid: string }>("player_source", { guid: item.guid })
      .then((r) => {
        if (alive && r && r.url) setHtmlSrc(r.url);
        else if (alive) setHtmlFailed(true);
      })
      .catch((e) => {
        if (alive) {
          setHtmlFailed(true);
          setErr(String(e));
        }
      });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [item.guid]);
  // [lc-101] 加载阶段状态机：把"半天没反应"变成可见进度，并让加载失败真正显示出来。
  // 之前只认 duration(id=2)：等不到就永远停在「正在拉起 mpv…」；而 mpv 的失败信息全在
  // start-file / file-loaded / end-file 事件里 —— 后端 lc-101 已把它们转成下面两个应用事件。
  const [phase, setPhase] = useState<"spawn" | "loading" | "playing" | "buffering" | "failed">("spawn");
  const [mpvErr, setMpvErr] = useState("");
  const [waited, setWaited] = useState(0);
  const t0Ref = useRef(Date.now());

  useEffect(() => {
    t0Ref.current = Date.now();
    setPhase("spawn");
    setMpvErr("");
    setWaited(0);
  }, [item.guid]);

  useEffect(() => {
    const p1 = listen<MpvEvent>("mpv-event", (e) => {
      const ev = e.payload;
      if (ev.event !== "property-change") return;
      // duration 到了说明文件已就绪（播放态）；进度/播放态本身仍归 mpv 侧 uosc 自己用
      if (ev.id === 2 && typeof ev.data === "number" && ev.data > 0) setPhase("playing");
      // id=7 paused-for-cache：NAS/代理慢时 mpv 在缓冲，必须让用户看到，而不是干等
      if (ev.id === 7) setPhase((s) => (ev.data ? "buffering" : s === "buffering" ? "playing" : s));
    });
    const p2 = listen<{ phase: string }>("mpv-load", (e) => {
      const ph = e.payload.phase;
      if (ph === "start") setPhase((s) => (s === "failed" ? s : "loading"));
      else if (ph === "loaded") setPhase("playing");
    });
    const p3 = listen<{ reason: string; error: string }>("mpv-end", (e) => {
      setMpvErr(e.payload.error || `播放失败（${e.payload.reason}）`);
      setPhase("failed");
    });
    return () => {
      p1.then((f) => f());
      p2.then((f) => f());
      p3.then((f) => f());
    };
  }, []);

  // [lc-101] 计时 + 超时：spawn/loading 阶段每 0.5s 刷新已等待秒数；
  // 25s 仍未就绪 → 判定失败并直接给出排查清单（NAS 可达？代理 22346 在跑？直链 401/404？）
  useEffect(() => {
    const arm = phase === "spawn" || phase === "loading";
    const id = window.setInterval(
      () => setWaited(Math.round((Date.now() - t0Ref.current) / 1000)),
      500,
    );
    const to = arm
      ? window.setTimeout(() => {
          setMpvErr(
            "加载超时（25s）：mpv 已启动，但没等到媒体就绪。请检查 NAS 是否可达、Go 代理(22346)是否在跑、该条目直链是否 401/404。",
          );
          setPhase("failed");
        }, 25000)
      : 0;
    return () => {
      window.clearInterval(id);
      if (to) window.clearTimeout(to);
    };
  }, [phase]);

  // [lc-097] 把视频占位矩形（主窗客户区物理像素）同步给后端 —— 它同时决定「挖洞」位置和
  // mpv 宿主窗位置。抽成 useCallback 是为了让「形态切换补测」那个 effect 也能复用。
  const syncBounds = useCallback(() => {
    const el = stageRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    invoke("player_set_bounds", {
      x: rect.left * dpr,
      y: rect.top * dpr,
      w: rect.width * dpr,
      h: rect.height * dpr,
    }).catch(() => {});
  }, []);

  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    // [lc-088] 视频占位矩形现在同时决定 WebView 的「挖洞」位置：除了尺寸变化，
    // 滚动/窗口缩放会让它**位移而不改尺寸**（ResizeObserver 收不到）→ 洞会留错位。
    // 故再挂 scroll(capture)/resize，并用 rAF 合并（Rust 侧另有去重兜底）。
    let raf = 0;
    const schedule = () => {
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        syncBounds();
      });
    };
    syncBounds();
    const ro = new ResizeObserver(schedule);
    ro.observe(el);
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, { capture: true, passive: true } as AddEventListenerOptions);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, { capture: true } as AddEventListenerOptions);
      if (raf) cancelAnimationFrame(raf);
    };
  }, [syncBounds]);

  // [lc-099] 逐帧同步（根治"视频区位置跟不上"这一类问题）。
  // 背景：视频是原生子窗，"画面该在哪"完全依赖前端把占位矩形推给后端。此前用
  // ResizeObserver + resize + scroll + 定时补测 这套**事件猜谜**式同步，必然存在遗漏/滞后 ——
  // 「要点一下画面才生效」「切换全屏那一拍错位」都是它引起的（实测日志证实：Resized 先到、
  // 前端重传后到，中间那一拍就是错的）。
  // 改为：播放页存续期间按 rAF 每帧对比一次矩形，变了才推。每帧一次 getBoundingClientRect
  // 的开销可忽略，且天然覆盖滚动/动画/布局变化/全屏切换，不再依赖任何事件的时序。
  useEffect(() => {
    let raf = 0;
    let last = "";
    const tick = () => {
      const el = stageRef.current;
      if (el) {
        const r = el.getBoundingClientRect();
        const dpr = window.devicePixelRatio || 1;
        const key = `${Math.round(r.left * dpr)},${Math.round(r.top * dpr)},${Math.round(
          r.width * dpr,
        )},${Math.round(r.height * dpr)}`;
        if (key !== last) {
          last = key;
          syncBounds();
        }
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [syncBounds]);

  // [lc-091] 收尾防抖：React18 StrictMode（main.tsx 开着）与快速重挂载会「setup→cleanup→setup」，
  // 旧写法在 cleanup 里立刻 player_set_visible(false)+player_stop，会把刚建立的播放状态打掉 ——
  // 后端拿到 6 条并发命令（true/play/false/stop/true/play），最终状态取决于完成顺序，
  // 一旦停在 false 洞就被清除且不会恢复 → 视频被 WebView 盖住，只剩「正在拉起 mpv…」。
  // 现在把收尾延后 150ms：期间若重新 setup 就取消，等于"重挂载不打断播放"。
  const teardownRef = useRef<number | null>(null);
  useEffect(() => {
    if (teardownRef.current) {
      window.clearTimeout(teardownRef.current);
      teardownRef.current = null;
    }
    // [lc-105] 页面内播放模式：不显示 mpv 宿主窗（否则页面里会留一个"坑"）
    if (htmlMode) {
      invoke("player_set_visible", { visible: false }).catch(() => {});
      return;
    }
    invoke("player_set_visible", { visible: true }).catch(() => {});
    invoke("play_item", { guid: item.guid }).catch((e) => setErr(String(e)));
    return () => {
      teardownRef.current = window.setTimeout(() => {
        teardownRef.current = null;
        invoke("player_set_visible", { visible: false }).catch(() => {});
        invoke("player_stop").catch(() => {});
      }, 150);
    };
  }, [item.guid, htmlMode]);

  // [lc-093] 把「内嵌小窗 / 全页全屏」形态镜像到 mpv 的 fullscreen 属性 ——
  // uosc 用它当底栏双形态判据：小窗只留 播放暂停+上下一集 | 时间 | 全屏；全屏保持原样。
  // 单点驱动（只认 embedded），mpv 侧的回声由后端 fs_expect 吞掉，不会循环。
  useEffect(() => {
    invoke("mpv_set_ui_fullscreen", { on: !embedded }).catch(() => {});
  }, [embedded]);

  // 全屏请求（mpv 侧事件 / 页面双击舞台）：内嵌形态 ↔ 全页形态 + 窗口全屏
  useEffect(() => {
    const p = listen<{ on: boolean }>("mpv-fullscreen", (e) => {
      const on = !!e.payload.on;
      onBig?.(on);
      getCurrentWindow().setFullscreen(on).catch(() => {});
    });
    return () => {
      p.then((f) => f());
    };
  }, [onBig]);

  const setBig = useCallback(
    (on: boolean) => {
      onBig?.(on);
      getCurrentWindow().setFullscreen(on).catch(() => {});
    },
    [onBig],
  );

  // [lc-087] 舞台鼠标/滚轮/双击全屏全部交给 mpv 原生处理（独立 mpv.exe 进程自带消息循环，
  // 子窗在宿主内正常收鼠标）——页面不再代收、不再转发、不再开浮层条窗口（uosc 底栏接管）。
  // 键位转发（mpv/input.conf 体系）：空格暂停、方向键进退/音量、f 全屏、m 静音、方括号变速
  useEffect(() => {
    const map: Record<string, string> = {
      " ": "SPACE",
      ArrowLeft: "LEFT",
      ArrowRight: "RIGHT",
      ArrowUp: "UP",
      ArrowDown: "DOWN",
      "[": "[",
      "]": "]",
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") {
        setBig(false);
        return;
      }
      const key = map[ev.key];
      if (!key) return;
      ev.preventDefault();
      invoke("mpv_input", { kind: "key", x: 0, y: 0, key }).catch(() => {});
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setBig]);

  return (
    <div className={embedded ? "player-embed" : "player-page"}>
      {!embedded && (
        <div className="player-topbar">
          {onBack && (
            <button className="back" onClick={onBack} title="返回上一页">
              ‹ 返回
            </button>
          )}
          <span className="player-title">
            {item.tvTitle ? `${item.tvTitle} · ` : ""}
            {item.title}
          </span>
        </div>
      )}
      <div
        className="player-stage"
        ref={stageRef}
      >
        {htmlMode && (
          <video
            className="player-video"
            src={htmlSrc ?? undefined}
            controls
            autoPlay
            playsInline
            onError={() => setHtmlFailed(true)}
          />
        )}
        {!htmlMode && err && <div className="err player-err">{err}</div>}
        {!htmlMode && !err && mpvErr && <div className="err player-err">{mpvErr}</div>}
        {!htmlMode && !err && !mpvErr && phase !== "playing" && (
          <div className="player-loading">
            {phase === "buffering"
              ? `缓冲中…（已等待 ${waited}s）`
              : phase === "loading"
                ? `正在加载媒体… ${waited}s`
                : `正在启动 mpv… ${waited}s`}
          </div>
        )}
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
  // 深浅主题：默认浅色；html.dark 类由本状态驱动，首帧前 main.tsx 已按存储值预置防闪烁
  const [theme, setTheme] = useState<"light" | "dark">(() =>
    localStorage.getItem("fntv-theme") === "dark" ? "dark" : "light",
  );
  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
    localStorage.setItem("fntv-theme", theme);
    // SWCA 模糊染色随主题切换（浅色近白/深色近黑）
    invoke("set_acrylic_tint", { light: theme === "light" }).catch(() => {});
  }, [theme]);
  // 首页加载中（首次/手动刷新）：无数据时渲染骨架屏；已有数据时静默刷新不闪骨架
  const [homeLoading, setHomeLoading] = useState(false);
  // 首页是否已滚动：滚动后 overlay 透明顶栏切实底（防内容从顶栏下穿过）
  const [homeScrolled, setHomeScrolled] = useState(false);
  // 启动会话检查期：不渲染任何视图，避免带有效会话时闪登录页
  const [booting, setBooting] = useState(true);
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

  // 首页数据完整加载（启动无缓存/手动刷新/登录后）：拉全量并回填
  const loadHome = useCallback(async () => {
    setErr("");
    setHomeLoading(true);
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
      // [lc-112] 原来只有 .then() 没有 .catch()：任何一个库取数失败都会被静默吞掉，
      // 该库 items 永远为空 ⇒ 整排不渲染（首页看起来"少了几个库"），且没有任何报错提示。
      libsArr.forEach((lib, idx) => {
        fetchLibItems(lib.guid, null)
          .then((items) => {
            setLibRows((rows) => {
              const next = [...rows];
              if (next[idx]) next[idx] = { lib, items };
              return next;
            });
          })
          .catch((e) => setErr(`加载媒体库「${lib.title}」失败：${String(e)}`));
      });
    } catch (e) {
      setErr(String(e));
      setView("home");
    } finally {
      setHomeLoading(false);
    }
  }, []);

  // 回首页：数据还在内存就直接切视图（进详情/库再回来不重拉），否则完整加载
  const goHome = useCallback(() => {
    if (view === "home") return;
    if (carousel.length > 0) setView("home");
    else loadHome();
  }, [view, carousel.length, loadHome]);

  // 首页数据落盘（30 分钟 TTL）：重启/会话恢复时零请求直出；数据落定 1.5s 后快照
  useEffect(() => {
    if (view !== "home" || !carousel.length) return;
    const t = setTimeout(() => {
      try {
        localStorage.setItem(
          "fntv-home-cache",
          JSON.stringify({
            t: Date.now(),
            libs,
            continueList,
            carousel,
            carouselBds,
            carouselLogos,
            sum,
            favTotal,
            libRows,
          }),
        );
      } catch {
        /* 配额满等忽略 */
      }
    }, 1500);
    return () => clearTimeout(t);
  }, [view, libs, continueList, carousel, carouselBds, carouselLogos, sum, favTotal, libRows]);

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
      .then(() => {
        // 30 分钟内的落盘快照直接恢复（重启零请求直出）；否则完整加载（骨架屏）
        let restored = false;
        try {
          const raw = localStorage.getItem("fntv-home-cache");
          const c = raw ? JSON.parse(raw) : null;
          if (
            c &&
            typeof c.t === "number" &&
            Date.now() - c.t <= 30 * 60 * 1000 &&
            Array.isArray(c.carousel) &&
            c.carousel.length &&
            Array.isArray(c.libs)
          ) {
            setLibs(c.libs);
            setContinueList(Array.isArray(c.continueList) ? c.continueList : []);
            setCarousel(c.carousel);
            setCarouselBds(c.carouselBds ?? {});
            setCarouselLogos(c.carouselLogos ?? {});
            setSum(c.sum ?? {});
            setFavTotal(typeof c.favTotal === "number" ? c.favTotal : 0);
            const rows0 =
              Array.isArray(c.libRows) && c.libRows.length
                ? c.libRows
                : c.libs.map((l: Lib) => ({ lib: l, items: [] }));
            setLibRows(rows0);
            // [lc-112] 快照直出后**仍要把各库第一页补齐**：快照里 items 常是空的
            // （写快照那一刻往往还没填完），空 items ⇒ 整排不渲染；而 restored=true
            // 意味着不会再走完整加载 —— 结果就是"首页只剩继续观看"，且 30 分钟内反复出现。
            rows0.forEach((row: { lib: Lib; items: MediaItem[] }, idx: number) => {
              if (row.items.length) return;
              fetchLibItems(row.lib.guid, null)
                .then((items) => {
                  setLibRows((prev) => {
                    const next = [...prev];
                    if (next[idx]) next[idx] = { lib: next[idx].lib, items };
                    return next;
                  });
                })
                .catch((e) => setErr(`加载媒体库「${row.lib.title}」失败：${String(e)}`));
            });
            setView("home");
            restored = true;
          }
        } catch {
          /* 缓存损坏 → 走完整加载 */
        }
        if (!restored) loadHome();
        setBooting(false);
      })
      .catch(() => {
        setBooting(false);
        setView("login");
      });
  }, [loadHome]);

  let body: ReactNode;
  if (booting) {
    body = null; // 会话检查期不出任何视图
  } else if (view === "login") {
    body = <LoginPage onDone={() => loadHome()} />;
  } else {
    // 首页常驻挂载：切到库/详情只隐藏不卸载（图片零重请求、滚动位置保留），player 全屏接管
    const homeReady = carousel.length > 0;
    body = (
      <div className="main-row" style={view === "player" ? { display: "none" } : undefined}>
        <Sidebar
          activeGuid={view === "home" ? null : activeLib?.guid ?? null}
          collapsed={!sidebarOpen}
          username={username}
          server={server}
          sum={sum}
          favTotal={favTotal}
          onOpenFav={openFav}
          onOpenCat={openCat}
          onHome={goHome}
        />
        <div className="view-stack">
          <div className={`view-pane${view === "home" ? " active" : ""}`}>
            {homeReady ? (
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
                onScrolled={setHomeScrolled}
              />
            ) : homeLoading ? (
              <HomeSkeleton />
            ) : null}
          </div>
          {view === "lib" && activeLib && (
            <div className="view-pane active">
              <LibView lib={activeLib} onBackHome={goHome} onOpenItem={openItem} />
            </div>
          )}
          {view === "detail" && detailItem && (
            <div className="view-pane active">
              <DetailView base={detailItem} onBack={() => setView(activeLib ? "lib" : "home")} />
            </div>
          )}
        </div>
      </div>
    );
    if (view === "player" && playTarget) {
      body = (
        <PlayerView
          item={playTarget}
          onBack={() => setView(detailItem ? "detail" : activeLib ? "lib" : "home")}
        />
      );
    }
  }

  return (
    <div className="app-shell">
      {/* [lc-094] 无边框窗口的缩放热区：tao 自带热区只有窗口外侧 SM_CXFRAME（175% 缩放 ≈ 7 物理像素
          ≈ 4 CSS px），那几像素又常被 WebView2 子窗吃掉 → 手感"不灵敏、有时拖不动"。
          这里在四边/四角铺透明拖拽条，由 WebView 收指针后调 startResizeDragging，热区稳定。
          注意：全页/全屏形态下视频铺满窗口，左右/下边缘落在"挖洞"里（鼠标归 mpv），那边靠 mpv 侧不参与
          —— 全屏本来也不需要拖拽缩放（先退回窗口态即可）。 */}
      {(
        [
          ["North", "rsz rsz-n"],
          ["South", "rsz rsz-s"],
          ["West", "rsz rsz-w"],
          ["East", "rsz rsz-e"],
          ["NorthWest", "rsz rsz-nw"],
          ["SouthWest", "rsz rsz-sw"],
          ["SouthEast", "rsz rsz-se"],
        ] as const
      ).map(([dir, cls]) => (
        <div
          key={dir}
          className={cls}
          onPointerDown={(e) => {
            e.preventDefault();
            getCurrentWindow()
              .startResizeDragging(dir)
              .catch(() => {});
          }}
        />
      ))}
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
        onHome={goHome}
        onRefresh={() => loadHome()}
        refreshing={homeLoading}
        overlay={(view === "home" && !homeScrolled) || view === "player"}
        // [lc-111] 详情页顶栏透明化（与首页同一观感）：.titlebar.ghost = 透明玻璃、保留主题色文字
        ghost={view === "detail"}
        onToggleTheme={() => setTheme((t) => (t === "dark" ? "light" : "dark"))}
      />
      {err && view !== "login" && <div className="err global-err">{err}</div>}
      {body}
    </div>
  );
}

export default App;
