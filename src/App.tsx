/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
// [lc-123-rev8] 图标统一用 Lucide（ISC 许可，https://lucide.dev）：stroke=currentColor、24 viewBox，
// 按需导入 tree-shake；仓库内不再保留手拷贝的 SVG path。
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Lock as IcoLock } from "lucide-react";
import { DetailView } from "./components/detail/DetailView";
import { DiscoverView } from "./components/discover/DiscoverView";
// [lc-1615] 观影记录页（侧栏快捷操作卡入口；飞牛 play/list ∪ 本机播放历史）
import { WatchHistoryView } from "./components/history/WatchHistoryView";
import { HomeSkeleton } from "./components/home/HomeSkeleton";
import { HomeView } from "./components/home/HomeView";
import { LibView } from "./components/library/LibView";
import { LoginPage } from "./components/login/LoginPage";
import { FeedbackPanel } from "./components/feedback/FeedbackPanel";
import { DisclaimerModal } from "./components/disclaimer/DisclaimerModal";
import { DISCLAIMER_ACK_KEY } from "./components/disclaimer/DisclaimerText";
import { SettingsPanel } from "./components/settings/SettingsPanel";
import { NavCapsule } from "./components/shell/NavCapsule";
import { TitleBar } from "./components/shell/TitleBar";
import { Sidebar } from "./components/sidebar/Sidebar";
import { useDelayedUnmount } from "./hooks/useDelayedUnmount";
import { RefreshContext } from "./hooks/useRefresh";
import { fetchLibItems } from "./lib/api";
import { pick } from "./lib/format";
import { bgmImg, pickImgStr, pickLargestImg, tmdbImg } from "./lib/images";
import { prefersReducedMotion } from "./lib/motion";
import { DEFAULT_SETTINGS } from "./lib/settings";
import { AppSettings, BgmCalendarItem, ChromeTone, Config, Lib, MediaItem, PlayItem, PlayTarget, TmdbDiscoverItem } from "./types";
import "./styles/index.css";


function App() {
  // [lc-1615] 新增 "history"（观影记录页；侧栏快捷操作卡入口）
  const [view, setView] = useState<"login" | "home" | "discover" | "lib" | "detail" | "history">("login");
  // [lc-123] 「页」= 顶栏胶囊的两个面（首页/发现）。与 view 分开：在库里/详情页时胶囊仍要
  // 记住用户是从哪一页进去的（发现页点条目进 Bangumi 详情，左上角应仍显示「发现」）。
  const [page, setPage] = useState<"home" | "discover">("home");
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
  // [lc-1647] 从首页横排直接点进「未识别文件夹」时的起始层（个人视频/网盘类库根层即文件夹）
  const [libFolder, setLibFolder] = useState<MediaItem | null>(null);
  const [detailItem, setDetailItem] = useState<MediaItem | null>(null);
  // [lc-1444] 进详情的来源（返回键的目标）：lib = 库里进，discover = 发现页进，home = 首页进。
  // 不用 view 现查 —— 用户从发现页进详情后 view 已是 "detail"，现查只能靠猜。
  // [lc-1615] 增加 history = 观影记录页进。
  const [detailFrom, setDetailFrom] = useState<"lib" | "discover" | "home" | "history">("home");
  const [err, setErr] = useState("");
  // [lc-1607] 首次启动的免责声明同意闸门：localStorage 未置位 → 弹；同意后置位，不再弹。
  const [disclaimerOpen, setDisclaimerOpen] = useState(() => {
    try {
      return localStorage.getItem(DISCLAIMER_ACK_KEY) !== "1";
    } catch {
      return true;
    }
  });
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

  /* ── [lc-1437] 设置项（性能模式 / 轮播 Logo / 退出行为 / 启动页）──
     状态放 App 顶层：前两项要在面板之外生效（html.perf 类、CarouselView 入参），
     后两项是"下一次动作"才生效的（关闭按钮、登录后落点）。
     真值在 Rust 的 config.json，这里只是它的镜像 + 一份 localStorage 兜底：
     **启动瞬间就用本地值**（不等 IPC 回来，避免性能模式用户先看到一帧动画），
     IPC 回来后再以配置为准校正一次。 */
  const [settings, setSettings] = useState<AppSettings>(() => {
    let cached: Partial<AppSettings> = {};
    try {
      const raw = localStorage.getItem("fntv-settings");
      if (raw) cached = JSON.parse(raw) as Partial<AppSettings>;
    } catch {
      /* 损坏就当没有 */
    }
    return { ...DEFAULT_SETTINGS, ...cached };
  });
  // 性能模式：html.perf 类驱动全局降级（CSS 见 App.css [lc-1437]）
  useEffect(() => {
    document.documentElement.classList.toggle("perf", settings.perfMode);
  }, [settings.perfMode]);
  // 启动时拉一次真实配置（校正本地镜像；失败就继续用本地值）
  useEffect(() => {
    invoke<AppSettings>("get_settings")
      .then((s) => {
        if (!s) return;
        setSettings((prev) => ({ ...prev, ...s }));
        try {
          localStorage.setItem("fntv-settings", JSON.stringify(s));
        } catch {
          /* ignore */
        }
      })
      .catch(() => {});
  }, []);
  /** 改设置：**先本地生效**（开关点下去立刻有反应），再落盘；落盘失败则重拉真实配置覆盖回来。 */
  const patchSettings = useCallback(async (patch: Partial<AppSettings>) => {
    setSettings((prev) => ({ ...prev, ...patch }));
    try {
      const saved = await invoke<AppSettings>("set_settings", { arg: patch });
      setSettings((prev) => ({ ...prev, ...saved }));
      try {
        localStorage.setItem("fntv-settings", JSON.stringify(saved));
      } catch {
        /* ignore */
      }
    } catch {
      // 回滚：不能让界面停在一个"没写进配置"的值上。重拉最稳
      // （逐个键反推"改之前"容易出错，且 Rust 侧可能只拒了其中一个键）
      invoke<AppSettings>("get_settings")
        .then((s) => s && setSettings((prev) => ({ ...prev, ...s })))
        .catch(() => {});
    }
  }, []);

  /** [lc-1439] 关闭询问：原生关闭按钮被 Rust 拦下后 emit close-ask，这里弹框。
   *  仅桌面端有该事件（Web 预览下 listen 不可用 → 静默跳过）。 */
  const [closeAsk, setCloseAsk] = useState(false);
  // [lc-1514] 「记住我的选择」勾选态：每次弹问重置（不把上次的勾选带给下一次）
  const [rememberClose, setRememberClose] = useState(false);
  useEffect(() => {
    if (closeAsk) setRememberClose(false);
  }, [closeAsk]);
  useEffect(() => {
    let un: (() => void) | undefined;
    listen("close-ask", () => setCloseAsk(true))
      .then((f) => {
        un = f;
      })
      .catch(() => {
        /* 非 Tauri 环境（如 vite preview）：没有该事件 */
      });
    return () => un?.();
  }, []);

  // 首页加载中（首次/手动刷新）：无数据时渲染骨架屏；已有数据时静默刷新不闪骨架
  const [homeLoading, setHomeLoading] = useState(false);
  /* ── [lc-1640] 顶栏 ⟳ =「按当前页强制刷新该页数据」──
     tick：每点一次自增；discover/lib/history/detail 各自的视图监听它触发自己的 force 刷新
       （经 RefreshContext 下发，见 src/hooks/useRefresh.ts）。home 不走 tick —— App 直接调
       loadHome/loadGuestHome（飞牛各命令本身无 Rust 缓存，就是强制重拉）。
     pageRefreshing：各视图用 reportBusy 上报"正在刷新"，这里汇总成顶栏 ⟳ 的 spinning 态
       （home 的 homeLoading 一并算进去，见下方 refreshing 的计算）。 */
  const [refreshTick, setRefreshTick] = useState(0);
  const [pageRefreshing, setPageRefreshing] = useState(false);
  const busySourcesRef = useRef<Set<string>>(new Set());
  const reportRefreshBusy = useCallback((id: string, busy: boolean) => {
    const s = busySourcesRef.current;
    if (busy) s.add(id);
    else s.delete(id);
    setPageRefreshing(s.size > 0);
  }, []);
  const refreshCtx = useMemo(
    () => ({ tick: refreshTick, reportBusy: reportRefreshBusy }),
    [refreshTick, reportRefreshBusy],
  );
  // [lc-123-rev4] 顶栏 chrome 色（首页 hero 亮度采样结果；CarouselView 上报）。
  // 只在首页生效：发现/库/详情页顶栏底下是应用底色 → 交回 "auto"（跟随主题）。
  const [heroTone, setHeroTone] = useState<ChromeTone>("auto");
  const onHeroTone = useCallback((t: ChromeTone) => setHeroTone(t), []);
  // 启动会话检查期：不渲染任何视图，避免带有效会话时闪登录页
  const [booting, setBooting] = useState(true);
  const [username, setUserName] = useState("");
  const [server, setServer] = useState("");
  // [lc-128] 设置面板（缓存 + 关于）开关；入口在侧栏快捷操作卡（设置）
  const [settingsOpen, setSettingsOpen] = useState(false);
  // [lc-1599] 反馈面板（工单制）开关；入口在侧栏快捷操作卡（反馈）
  const [feedbackOpen, setFeedbackOpen] = useState(false);
  // [lc-1600] 游客模式（未登录也能用）：true = 本会话按游客态走 —— 首页取数走
  // 「AniList 当季 + TMDB 周热门」兜底链；查看详情/播放改为弹登录引导。
  // 初始值从启动标记推导（LoginPage 写入）；登录成功（loadHome）时清掉标记并置回 false。
  const [guest, setGuest] = useState(() => localStorage.getItem("anibox-guest") === "1");
  // [lc-1600] 「查看详情 / 播放需要登录」引导弹窗
  const [guestAsk, setGuestAsk] = useState(false);
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
    // [lc-1600] 走到这里 = 登录成功（或已有有效会话）：游客标记完成使命，清掉
    try {
      localStorage.removeItem("anibox-guest");
    } catch {
      /* ignore */
    }
    setGuest(false);
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
      setPage("home");
      setView("home");

      // 轮播条目详情补齐（缓存到 App 层，只拉一次）：横版主图 + 简介/评分/年份回填
      Promise.allSettled(
        carList.map((it) => invoke<Record<string, unknown>>("get_item_detail", { guid: it.guid })),
      ).then((rs) => {
        const map: Record<string, string> = {};
        const lmap: Record<string, string> = {};
        const enrich: Record<string, Partial<MediaItem>> = {};
        const fb: { guid: string; it: MediaItem; arg: Record<string, unknown> }[] = [];
        rs.forEach((r, i) => {
          if (r.status === "fulfilled") {
            const d = r.value;
            // [lc-1433] 飞牛这张**只作兜底**（见下）：它是飞牛自己抓的单张 webp，
            //   实测缓存里大量只有 507×338 —— 整屏 hero 拉伸必糊，而且 `backdrops` 是
            //   **字符串**（不是数组），所以 pickLargestImg 在这里根本无从选择。
            const bd = pickLargestImg(d.backdrops) || pickImgStr(d, ["backdrops", "backdrop"]);
            if (bd) map[carList[i].guid] = bd;
            // 无论飞牛有没有图都进候选：TMDB 有图就用 TMDB 的（飞牛没有时更是唯一来源）
            // [lc-1433] **带锚点**：飞牛条目详情里就带 trim_id（tm*/tt*/bg*）与 tmdb_id/imdb_id，
            //   传进去 Rust 侧就能精确定位、**完全不做标题搜索** —— 否则"同名不同剧"会取错图
            //   （标题搜索有相似度门槛，但近似标题仍可能命中别的条目，那比糊更糟）。
            const str = (v: unknown): string => (v == null ? "" : String(v).trim());
            const trim = str(pick(d, ["trim_id", "trimId"]));
            const directTmdb = str(pick(d, ["tmdbId", "tmdb_id"]));
            let tmdbId = /^\d+$/.test(directTmdb) ? directTmdb : "";
            if (!tmdbId) {
              const m = /^(?:tm)(\d+)$/i.exec(trim);
              if (m) tmdbId = m[1];
              else if (/^\d+$/.test(trim)) tmdbId = trim;
            }
            const imdbId = str(pick(d, ["imdb_id", "imdbId"])) || (/^tt\d+$/i.test(trim) ? trim.toLowerCase() : "");
            fb.push({
              guid: carList[i].guid,
              it: carList[i],
              arg: {
                // 标题：条目详情里的（tvTitle/title）优先 —— 它与锚点同源，最不容易与
                //   列表页的简称错位（锚点缺失时这是唯一的匹配依据）
                title: String(pick(d, ["tvTitle", "title"]) ?? carList[i].title),
                year: String(carList[i].release_date || carList[i].air_date || "").slice(0, 4),
                mediaType: (carList[i].type || "").toLowerCase() === "movie" ? "movie" : "tv",
                trimId: trim || undefined,
                tmdbId: tmdbId || undefined,
                imdbId: imdbId || undefined,
              },
            });
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

        // [lc-1433] **高清横图：我们自己向 TMDB 要**（用户要求）——飞牛那张只当兜底。
        //   为什么必须换源：飞牛的 backdrops 是它抓的单张小图（实测多为 507×338），
        //   hero 是满屏宽度（1400~1900px），拉伸必糊。TMDB 的 images.backdrops 里有
        //   3840×2160 的原图，`heroBackdrop` 就是**按面积挑出的最大那张**（Rust 侧挑好）。
        //   取 `t/p/original` 档：走 fntvimg 代理**拉了即落盘**（下次零网络），
        //   与剧照放大用的是同一套缓存，不会多存一份。
        //   定位靠锚点优先（trim_id=tm*/tt*、tmdbId、imdbId），没有才标题搜索 —— 与详情页
        //   完全同一条链路（tmdb_show_details），命中同一份落盘缓存，所以这里几乎零额外成本。
        //   失败（TMDB 没匹配上/没图）就**保留飞牛那张**，绝不出现空 hero。
        void Promise.allSettled(
          fb.map(({ guid, arg }) =>
            invoke<Record<string, unknown>>("tmdb_show_details", { arg })
              .then((d) => ({ guid, path: String(d?.heroBackdrop ?? "") }))
          )
        ).then((trs) => {
          const hi: Record<string, string> = {};
          for (const t of trs) {
            if (t.status === "fulfilled" && t.value.path) hi[t.value.guid] = t.value.path;
          }
          if (!Object.keys(hi).length) return;
          setCarouselBds((prev) => {
            const next = { ...prev };
            for (const [guid, path] of Object.entries(hi)) next[guid] = tmdbImg(path, "original");
            return next;
          });
        });
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

  /* ── [lc-1600] 游客首页取数：未登录 → 飞牛接口不可用，改用公开数据源合成 6 条伪 MediaItem ──
     [lc-1604] 两半都来自 TMDB（中文名/中文简介 + 官方横图 backdrop + 官方海报，同一套来源）：
     3 条「热门番剧」（mode=anime）+ 3 条 TMDB 周热门剧集（mode=trending），
     顺序：番剧在前、剧集在后。任一来源失败只丢那一半（Promise.allSettled）——
     能拿几条算几条，两条链都空就是空首页，**绝不进错误态**（游客仍可去发现页）。 */
  const loadGuestHome = useCallback(async () => {
    setErr("");
    setHomeLoading(true);
    try {
      const [animeRes, tvRes] = await Promise.allSettled([
        invoke<{ items?: TmdbDiscoverItem[] }>("tmdb_discover", {
          arg: { mode: "anime", mediaType: "tv", page: 1 },
        }),
        invoke<{ items?: TmdbDiscoverItem[] }>("tmdb_discover", {
          arg: { mode: "trending", mediaType: "tv", page: 1 },
        }),
      ]);

      const items: MediaItem[] = [];
      const bds: Record<string, string> = {};
      // [lc-1604] 两半共用 `guest-tmdb-{id}` 作 guid：同一部动画也可能同时进周热门，
      // 用 Set 记已用 id，只保留先出现的（番剧半边在前）。
      const used = new Set<number>();

      // ① 番剧：TMDB 热门番剧前 3（[lc-1604] 与剧集半边同一套映射：中文名/简介 + 官方横图/海报）
      if (animeRes.status === "fulfilled") {
        for (const it of (animeRes.value?.items ?? []).slice(0, 3)) {
          const id = Number(it.tmdbId) || 0;
          const title = String(it.title || it.originalTitle || "").trim();
          if (!id || !title || used.has(id)) continue;
          used.add(id);
          const guid = `guest-tmdb-${id}`;
          items.push({
            guid,
            title,
            type: "TV",
            poster: it.poster ? tmdbImg(it.poster, "w780") : undefined,
            overview: it.overview || "",
            vote_average: it.rating ? String(it.rating) : "",
            release_date: it.date || (it.year ? `${it.year}-01-01` : ""),
          });
          if (it.backdropPath) bds[guid] = tmdbImg(it.backdropPath, "original");
        }
      }

      // ② 剧集：TMDB 周热门前 3（海报/横图都走 fntvimg 代理；横图取 original 档，与登录态 hero 同规格）
      if (tvRes.status === "fulfilled") {
        for (const it of (tvRes.value?.items ?? []).slice(0, 3)) {
          const id = Number(it.tmdbId) || 0;
          const title = String(it.title || it.originalTitle || "").trim();
          if (!id || !title || used.has(id)) continue;
          used.add(id);
          const guid = `guest-tmdb-${id}`;
          items.push({
            guid,
            title,
            type: "TV",
            poster: it.poster ? tmdbImg(it.poster, "w780") : undefined,
            overview: it.overview || "",
            vote_average: it.rating ? String(it.rating) : "",
            release_date: it.date || (it.year ? `${it.year}-01-01` : ""),
          });
          if (it.backdropPath) bds[guid] = tmdbImg(it.backdropPath, "original");
        }
      }

      setLibs([]);
      setContinueList([]);
      setCarousel(items);
      setCarouselBds(bds);
      setCarouselLogos({});
      setLibRows([]);
      setSum({});
      setPage("home");
      setView("home");
    } catch {
      /* 兜底：取数异常也绝不把游客首页打成错误态 */
    } finally {
      setHomeLoading(false);
    }
  }, []);

  // [lc-1640] 顶栏 ⟳：按**当前视图**强制刷新该页数据（原先写死只重拉首页）。
  //   · home：loadHome / loadGuestHome 本身就是强制重拉（get_media_libs 等命令无 Rust 缓存，
  //     每次都直连飞牛；游客链走 tmdb_discover，同样无本地 TTL 命中问题）——直接调用。
  //   · discover/lib/history/detail：bump tick，由对应视图消费后触发它自己的 force 刷新
  //     （TmdbPane/BgmPane 的 force、DetailView.refreshAll、LibView 重载、HistoryView load(true)）。
  //   · login：nav=false ⇒ 顶栏根本不渲染刷新键（无数据可刷，见 TitleBar），这里不必处理。
  const onRefresh = useCallback(() => {
    if (view === "home") {
      if (guest) void loadGuestHome();
      else void loadHome();
      return;
    }
    setRefreshTick((n) => n + 1);
  }, [view, guest, loadHome, loadGuestHome]);

  // [lc-1600] 从登录页进入游客模式（标记已由 LoginPage 写入）：切态 + 切首页；
  // 内存里已有数据（引导弹窗折返回登录页又点「先逛逛」）就直接复用，不重拉。
  const enterGuest = useCallback(() => {
    setGuest(true);
    setPage("home");
    setView("home");
    if (!carousel.length) void loadGuestHome();
  }, [carousel.length, loadGuestHome]);

  // 回首页：数据还在内存就直接切视图（进详情/库再回来不重拉），否则完整加载
  const goHome = useCallback(() => {
    setPage("home");
    if (view === "home") return;
    if (carousel.length > 0) setView("home");
    else if (guest) loadGuestHome();
    else loadHome();
  }, [view, carousel.length, loadHome, loadGuestHome, guest]);

  // [lc-123] 去发现页：**不预取**（DiscoverView 的首拉闸门是 active=view==="discover"），
  // 首页数据原样留在内存（切回来不重拉）
  const goDiscover = useCallback(() => {
    setPage("discover");
    setView("discover");
  }, []);

  // 顶栏胶囊：首页走 goHome（有数据不重拉），发现走 goDiscover
  // [lc-123-rev6] 页面切换动效（首页 ↔ 发现）：两条视图都是常驻挂载，切换时给**进入/离开各一套**
  // 动画类（出去的 opacity→0 + 反方向位移，进来的反之）；进入侧 animationend 摘掉动画类
  // （另配 400ms 安全兜底：reduced-motion 下没有动画事件，不能留退场层盖在上面）。
  const [paneAnim, setPaneAnim] = useState<{
    dir: "to-discover" | "to-home";
    leaving: "home" | "discover";
  } | null>(null);
  const animTimer = useRef<number | null>(null);
  // [lc-1513] 子页（库/详情）转场：进出都播动画；返回时目标 pane 补一次淡入
  const [pageAnim, setPageAnim] = useState<string | null>(null);
  const [libEnterAnim, setLibEnterAnim] = useState(false);
  const [detailMounted, detailClosing] = useDelayedUnmount(view === "detail" && !!detailItem);
  const [libMounted, libClosing] = useDelayedUnmount(view === "lib" && !!activeLib);
  // [lc-1615] 观影记录页：条件挂载 + 延迟卸载（与库页同套转场）
  const [historyMounted, historyClosing] = useDelayedUnmount(view === "history");
  const clearPaneAnim = useCallback(() => {
    if (animTimer.current) {
      window.clearTimeout(animTimer.current);
      animTimer.current = null;
    }
    setPaneAnim(null);
  }, []);
  const onNav = useCallback(
    (p: "home" | "discover") => {
      if (p !== page) {
        if (prefersReducedMotion()) {
          clearPaneAnim(); // 少动效：瞬时切换
        } else {
          setPaneAnim({ dir: p === "discover" ? "to-discover" : "to-home", leaving: page });
          if (animTimer.current) window.clearTimeout(animTimer.current);
          animTimer.current = window.setTimeout(() => setPaneAnim(null), 400);
        }
      }
      if (p === "discover") goDiscover();
      else goHome();
    },
    [page, goDiscover, goHome, clearPaneAnim],
  );

  // [lc-123-rev7] 深浅模式切换：**「外部扩散」= 镂空反向扩散**（用户否掉 v6 的实色圆吞屏：
  // 「从按钮处往外扩散」：主题**立即切换**（DOM/类名马上换），随后铺一层**新主题的底色**
  // （CSS 变量按层次铺，见 App.css .theme-veil），clip-path: circle() 从**按钮处 0 长到全屏** ——
  // 圆内是新主题底色、圆外是尚未被盖住的旧界面，圆边向外推进 = 新主题从按钮荡开盖满屏。
  // 印象层**不放截图**（WebView2 无页面截图通道），也不用纯 #fff/#000。
  const [veil, setVeil] = useState<{ x: number; y: number } | null>(null);
  const toggleTheme = useCallback(
    (e: React.MouseEvent<HTMLButtonElement>) => {
      const next = theme === "dark" ? "light" : "dark";
      // [lc-1437] 性能模式/少动效偏好：跳过扩散遮罩（那是一次全屏重绘，正是性能模式要省的）
      if (prefersReducedMotion() || settings.perfMode) {
        setVeil(null);
        setTheme(next);
        return;
      }
      const r = e.currentTarget.getBoundingClientRect();
      // 主题先切（新主题立刻生效），遮罩再铺**新主题底色**从按钮处长出
      setTheme(next);
      setVeil({ x: r.left + r.width / 2, y: r.top + r.height / 2 });
    },
    [theme, settings.perfMode],
  );
  // [lc-1406] 遮罩移除的**双保险**：
  //   ① 正常路径：animationend（动画结束）
  //   ② 兜底：定时器（动画时长 420ms → 700ms 后无条件清掉）
  //   为什么必须有 ②：animationend 并非总能到达 —— 窗口失焦/最小化/后台时 Chromium 会节流甚至
  //   暂停动画，事件可能永不触发；此遮罩是 z-index:80 的**固定全屏层**，一旦残留就永久盖着页面
  //   （用户反馈的"演职人员背景有个大圆还在"就是它的形状：clip-path 圆停在中途，边缘是一段大圆弧）。
  //   定时器以 veil 的 x/y 为依赖：每次切换都会重置，所以它只兜"这一次动画没结束"，不会误杀下一次。
  const onVeilEnd = useCallback(() => setVeil(null), []);
  useEffect(() => {
    if (!veil) return;
    const t = window.setTimeout(() => setVeil(null), 700);
    return () => window.clearTimeout(t);
  }, [veil]);

  // [lc-1406] 切换视图/页面时清掉遮罩：遮罩属于"主题切换这一瞬间"的视觉，不该跨视图存活。
  useEffect(() => {
    setVeil(null);
  }, [view, page]);

  // 首页数据落盘（30 分钟 TTL）：重启/会话恢复时零请求直出；数据落定 1.5s 后快照
  useEffect(() => {
    // [lc-1600] 游客数据不写快照（快照是登录态恢复路径在用；游客条目混进去会在
    // 下次带会话启动时被恢复出来）。guest 只作守卫、不进依赖：若因它触发一次
    // "旧游客数据"的写入，反而会污染快照。
    if (guest || view !== "home" || !carousel.length) return;
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, libs, continueList, carousel, carouselBds, carouselLogos, sum, favTotal, libRows]);

  // [lc-118] 首页轮播的「开始播放」= **直接拉起独立 mpv 窗口**（与详情页同一个入口）。
  const [playMsg, setPlayMsg] = useState("");
  const play = useCallback((t: PlayTarget) => {
    // [lc-1600] 游客：播放依赖飞牛会话（guest-* 伪条目也没有可播源）→ 弹登录引导，不出"播放失败"
    if (guest) {
      setGuestAsk(true);
      return;
    }
    setPlayMsg("");
    invoke("play_item", { guid: t.guid })
      .then(() => {
        setPlayMsg(`已在独立播放器窗口打开${t.title ? `：${t.title}` : ""}`);
        window.setTimeout(() => setPlayMsg(""), 4000);
      })
      .catch((e) => {
        setPlayMsg(`播放失败：${String(e)}`);
        window.setTimeout(() => setPlayMsg(""), 6000);
      });
  }, [guest]);

  const openItem = useCallback(
    (item: MediaItem) => {
      // [lc-1600] 游客：不跳详情页，弹登录引导（轮播 MORE / 首页横排共用这条链路）
      if (guest) {
        setGuestAsk(true);
        return;
      }
      setDetailItem(item);
      setDetailFrom(view === "lib" ? "lib" : "home");
      setView("detail");
    },
    [view, guest],
  );

  // [lc-1444] 发现页条目 → 主详情页：合成 MediaItem。guid 用 `discover-<tmdbId>` 占位标识
  // （DetailView 借此进入「无飞牛条目」形态：跳过飞牛请求、选集区占位、
  //  TMDB 定位直接用条目自带的 tmdb_id，不用等飞牛详情的锚点）。
  const openDiscoverDetail = useCallback(
    (it: TmdbDiscoverItem, mediaType: "tv" | "movie") => {
      setDetailItem({
        guid: `discover-${it.tmdbId}`,
        title: it.title,
        overview: it.overview,
        release_date: it.date || (it.year ? `${it.year}-01-01` : ""),
        tmdb_id: it.tmdbId,
        type: mediaType === "movie" ? "Movie" : "TV",
      });
      setDetailFrom("discover");
      setView("detail");
    },
    [],
  );

  // [lc-1444] 发现页右栏（新番表）条目 → 主详情页：条目自带 Bangumi subject id，
  //   guid 用 `discover-bgm-<id>` 占位标识；封面直接带（代理 URL）；TMDB 侧没有 id，走标题搜索兜底。
  const openDiscoverBgm = useCallback((it: BgmCalendarItem) => {
    setDetailItem({
      guid: `discover-bgm-${it.id}`,
      title: it.name_cn || it.name,
      release_date: it.air_date,
      poster: bgmImg(it.cover),
      bgm_id: it.id,
      type: "TV",
    });
    setDetailFrom("discover");
    setView("detail");
  }, []);

  // [lc-1563] 发现页右栏「已在库」条目 → 直接开库内条目（真 guid）：此前一律走 discover-*
  //   占位详情，在库条目也会落成「无源 → 在线资源面板」；已在库的番应进库内详情（可播放）。
  const openDiscoverLib = useCallback((item: MediaItem) => {
    setDetailItem(item);
    setDetailFrom("discover");
    setView("detail");
  }, []);

  const openFav = useCallback(() => {
    setLibFolder(null);
    setActiveLib({ guid: "_fav", title: "收藏", category: "" });
    setPage("home");
    setView("lib");
  }, []);

  const openCat = useCallback((cat: { key: string; types: string[] }) => {
    setLibFolder(null);
    setActiveLib({ guid: `_cat:${cat.types.join(",")}`, title: cat.key, category: "" });
    setPage("home");
    setView("lib");
  }, []);

  // [lc-1647] 首页横排点「未识别文件夹」→ 开库并直接钻到该层（不再落到无意义的详情页）
  const openLibFolder = useCallback((lib: Lib, folder: MediaItem) => {
    setLibFolder(folder);
    setActiveLib(lib);
    setPage("home");
    setView("lib");
  }, []);

  // [lc-1615] 观影记录页：侧栏快捷操作卡入口 → 全页视图（与库/详情同套 view-pane 转场）
  const openHistory = useCallback(() => {
    setView("history");
  }, []);

  // [lc-1615] 观影记录页点库内条目 → 详情（返回目标 = history；与 openItem 同一条详情链路）
  const openHistoryItem = useCallback((item: MediaItem) => {
    setDetailItem(item);
    setDetailFrom("history");
    setView("detail");
  }, []);

  useEffect(() => {
    // [lc-1600] 启动先看游客标记：是游客 → **跳过登录/会话检查**，直接进首页（游客取数链）。
    // 先切视图再拉数据：homeLoading 一置起即渲染骨架屏（不闪登录页、不空白）。
    if (localStorage.getItem("anibox-guest") === "1") {
      setGuest(true);
      setPage("home");
      setView("home");
      setBooting(false);
      void loadGuestHome();
      return;
    }
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
            setPage("home");
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
        // [lc-1623] 飞牛会话失效 / 未登录：**不再无条件停在登录页** —— 那会把 BGM 侧（BGM 账号卡、
        //   发现页 BGM 面板、追番同步）一起挡在门外，正是用户感知的「飞牛退出 BGM 也没了」。
        //   曾配置过飞牛（config.server 非空）→ 进游客态：App 照常可用，发现页/BGM 面板直接可达；
        //   首页/媒体库这类飞牛依赖区仍按游客态处理（播放/详情引导登录），不误伤。
        //   两个例外仍回登录页：① 全新安装（无 server，否则新用户找不到飞牛登录入口）；
        //   ② 配了「自动登录」（记住的密码 + auto_login）—— 让登录页照旧静默重登，别把这个既有功能弄没了。
        //   重新登录飞牛的常规入口在设置面板「账号 → 登录飞牛」。
        invoke<Config>("get_config")
          .then((c) => {
            const autoArmed = !!(c?.auto_login && c?.password && c?.username && c?.server);
            if (c?.server && !autoArmed) {
              setGuest(true);
              setPage("home");
              setView("home");
              void loadGuestHome();
            } else {
              setView("login");
            }
          })
          .catch(() => setView("login"))
          .finally(() => setBooting(false));
      });
  }, [loadHome, loadGuestHome]);

  // [lc-123-rev6] 常驻视图的动画类：进入侧挂 anim-*、退场侧额外加 leaving（退场层 position:absolute 叠在上面，
  // pointer-events:none —— 动画中照样能点，不锁交互）。动画类只在这 ~260ms 里存在，结束即摘。
  const paneCls = (which: "home" | "discover") => {
    const active = view === which;
    const entering = paneAnim && active && paneAnim.leaving !== which;
    const leaving = paneAnim && !active && paneAnim.leaving === which;
    return (
      `view-pane${active ? " active" : ""}` +
      (paneAnim && (entering || leaving) ? ` anim-${paneAnim.dir}` : "") +
      (leaving ? " leaving" : "") +
      // [lc-1513] 从详情页返回：目标主页补一次进入淡入（class 添加即触发）
      (pageAnim && active ? " view-in" : "")
    );
  };

  let body: ReactNode;
  if (booting) {
    body = null; // 会话检查期不出任何视图
  } else if (view === "login") {
    body = <LoginPage onDone={() => loadHome()} onGuest={enterGuest} />;
  } else {
    // 首页常驻挂载：切到库/详情只隐藏不卸载（图片零重请求、滚动位置保留）
    const homeReady = carousel.length > 0;
    body = (
      <div className="main-row">
        {/* [lc-1427] 侧栏改为覆盖式浮层后的配套交互：展开时铺一层**透明遮罩**，
            点击它收起侧栏（覆盖式侧栏的标准行为）。它必须：
              · 在侧栏之下（z-index 44 < 侧栏 45）→ 不挡侧栏自己的点击；
              · 在顶栏之下（< 50）→ 汉堡键仍可点（否则点不开/关不上）；
              · 完全透明（不加暗化）—— 用户要的是"覆盖上层"而不是"模态弹窗"，
                加暗化会让首页变暗、显得像对话框。 */}
        {sidebarOpen && (
          <div
            className="sidebar-scrim"
            aria-hidden="true"
            onClick={() => {
              setSidebarOpen(false);
              localStorage.setItem("fntv-sidebar-open", "0");
            }}
          />
        )}
        <Sidebar
          // [lc-1615] history 页没有任何侧栏项应高亮（"_history" 不匹配首页/收藏/分类）
          activeGuid={view === "home" ? null : view === "history" ? "_history" : activeLib?.guid ?? null}
          collapsed={!sidebarOpen}
          // [lc-1623] 游客态（飞牛未登录/已退出）不显示飞牛用户名 —— 底栏如实显示「未登录」，
          //   与首页/媒体库那类飞牛依赖区的提示口径一致（不误伤）。
          username={guest ? "" : username}
          server={server}
          sum={sum}
          favTotal={favTotal}
          onOpenFav={openFav}
          onOpenCat={openCat}
          onHome={goHome}
          onOpenSettings={() => setSettingsOpen(true)}
          onOpenFeedback={() => setFeedbackOpen(true)}
          onOpenHistory={openHistory}
        />
        <div className="view-stack">
          {/* [lc-123-rev2] 「首页 | 发现」胶囊是**浮层**（.navbar = absolute，不占布局高度）：
              浮在内容区（.view-stack，侧栏右侧）顶部居中、盖在海报上；首页 hero 保持满幅紧贴标题栏。
              只在两个"主页"上出现：库/详情是带返回入口的子页，且详情页的分段栏（.dseg，sticky top 40px）
              正好在胶囊的落点上，浮上去会压住中间的 tab（见 App.css 注释）。 */}
          {(view === "home" || view === "discover") && (
            <div className="navbar">
              <NavCapsule page={page} onNav={onNav} />
            </div>
          )}
          <div
            className={paneCls("home")}
            onAnimationEnd={() => {
              clearPaneAnim();
              setPageAnim(null);
            }}
          >
            {homeReady ? (
              <HomeView
                libs={libs}
                continueList={continueList}
                carousel={carousel}
                carouselBds={carouselBds}
                carouselLogos={carouselLogos}
                carouselLogoOn={settings.carouselLogo}
                launchPage={settings.launchPage}
                libRows={libRows}
                hiddenLibs={hiddenLibs}
                onToggleHiddenLib={toggleHiddenLib}
                onOpenLib={(l) => {
                  setLibFolder(null);
                  setActiveLib(l);
                  setView("lib");
                }}
                onOpenFolder={openLibFolder}
                onPlay={play}
                onOpenItem={openItem}
                onHeroTone={onHeroTone}
              />
            ) : homeLoading ? (
              <HomeSkeleton />
            ) : null}
          </div>
          {/* [lc-123] 发现页：与首页同款「常驻挂载」——两栏的滚动位置/已取数据在切页后保留；
              首次拉取的闸门是 active={view === "discover"}（不进这一页就不发任何请求） */}
          <div
            className={paneCls("discover")}
            onAnimationEnd={() => {
              clearPaneAnim();
              setPageAnim(null);
            }}
          >
            <DiscoverView
              active={view === "discover"}
              onOpenDetail={openDiscoverDetail}
              onOpenBgm={openDiscoverBgm}
              onOpenLib={openDiscoverLib}
            />
          </div>
          {(view === "lib" || libMounted) && activeLib && (
            <div
              className={`view-pane active${libClosing ? " view-out" : libEnterAnim ? " view-in" : ""}`}
              onAnimationEnd={() => setLibEnterAnim(false)}
            >
              <LibView lib={activeLib} initialFolder={libFolder} onBackHome={goHome} onOpenItem={openItem} />
            </div>
          )}
          {/* [lc-1615] 观影记录页：条件挂载 + view-in/view-out 转场（与库/详情页同套）。
              从详情返回时本 pane 重新挂载（view-in 随挂载触发），无需额外补动画。 */}
          {(view === "history" || historyMounted) && (
            <div className={`view-pane active${historyClosing ? " view-out" : " view-in"}`}>
              <WatchHistoryView onBack={goHome} onOpenItem={openHistoryItem} />
            </div>
          )}
          {/* [lc-1513] 详情/库页：进入播 view-in；返回时（view 切走）先播 view-out
              再延迟卸载；返回目标（home/discover/lib）同时补一次 view-in 淡入，
              不会出现「返回后旧视图瞬间闪回、新视图硬切」。 */}
          {(view === "detail" || detailMounted) && detailItem && (
            <div className={`view-pane active${detailClosing ? " view-out" : " view-in"}`}>
              {/* [lc-1444] 返回目标按进详情的来源（detailFrom）决定，不用 view/activeLib 现猜 */}
              <DetailView
                base={detailItem}
                onBack={() => {
                  // 返回动画：目标 pane 补一次进入淡入（class 添加即触发，animationend 后移除）
                  if (detailFrom === "lib") setLibEnterAnim(true);
                  // [lc-1615] history 不补 pageAnim：本 pane 重新挂载时自带 view-in。
                  // （pageAnim 只被 home/discover 的 animationend 清理；目标 pane 非激活时会滞留，
                  //   下次进首页才补播一次「过期的」淡入。）
                  else if (detailFrom !== "history") setPageAnim("view-in");
                  setView(
                    detailFrom === "discover"
                      ? "discover"
                      : detailFrom === "lib"
                        ? "lib"
                        : detailFrom === "history"
                          ? "history"
                          : "home",
                  );
                }}
              />
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="app-shell">
      {/* [lc-118] 播放反馈浮标：轮播/首页没有 err 区域，播放失败必须有可见反馈 */}
      {playMsg && (
        <div
          style={{
            position: "fixed",
            left: "50%",
            bottom: 28,
            transform: "translateX(-50%)",
            padding: "10px 18px",
            borderRadius: 12,
            zIndex: 9999,
            background: "rgba(20,22,28,.92)",
            color: "#fff",
            fontSize: 13,
            boxShadow: "0 8px 28px rgba(0,0,0,.45)",
            pointerEvents: "none",
          }}
        >
          {playMsg}
        </div>
      )}
      {/* [lc-094] 无边框窗口的缩放热区：tao 自带热区只有窗口外侧 SM_CXFRAME（175% 缩放 ≈ 7 物理像素
          ≈ 4 CSS px），那几像素又常被 WebView2 子窗吃掉 → 手感"不灵敏、有时拖不动"。
          这里在四边/四角铺透明拖拽条，由 WebView 收指针后调 startResizeDragging，热区稳定。 */}
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
      {/* [lc-123-rev7] 深浅切换的**镂空反向扩散**：旧主题印象层（CSS 变量底色，无截图通道）
          用 clip-path: circle() 从 150% 收缩到 0 —— 圆内是旧界面、圆外已是新界面，"透明的外部扩散"。
          原点来自按钮 rect（--tx/--ty）；z-index 80 > 顶栏 50；pointer-events:none；
          animationend 后从 DOM 移除。 */}
      {veil && (
        <div
          className="theme-veil"
          style={
            {
              "--tx": `${veil.x}px`,
              "--ty": `${veil.y}px`,
            } as React.CSSProperties
          }
          onAnimationEnd={onVeilEnd}
          aria-hidden="true"
        />
      )}
      <TitleBar
        title="AniBox"
        nav={view !== "login"}
        page={page}
        sidebarOpen={sidebarOpen}
        onToggleSidebar={() =>
          setSidebarOpen((v) => {
            localStorage.setItem("fntv-sidebar-open", v ? "0" : "1");
            return !v;
          })
        }
        // [lc-1640] 顶栏 ⟳：按当前视图强制刷新该页数据（见 onRefresh）。
        onRefresh={onRefresh}
        // spinning 汇总：home 用 homeLoading，其余页面用各视图 reportBusy 的聚合（pageRefreshing）。
        refreshing={homeLoading || pageRefreshing}
        // [lc-123-rev3] 顶栏现在**一律全透明**（无底色/描边/渐变），可读性靠 text-shadow；
        // overlay/ghost 两个"底色模式"修饰类随之取消（App.css 里的 .titlebar.overlay/.ghost 已删）。
        // [lc-123-rev4] 首页 hero 是满幅图 → chrome 色按**图上条带亮度采样**自适应；
        // 其余页（发现/库/详情）顶栏底下是应用底色 → "auto" 跟随主题（浅色主题深字 / 深色主题浅字）。
        chrome={view === "home" ? heroTone : "auto"}
        onToggleTheme={toggleTheme}
      />
      {err && view !== "login" && <div className="err global-err">{err}</div>}
      {settingsOpen && (
        <SettingsPanel
          onClose={() => setSettingsOpen(false)}
          settings={settings}
          onSettings={(patch) => void patchSettings(patch)}
          // [lc-1623] 飞牛未登录（游客态）时设置面板给「登录飞牛」入口 —— 退出/失效后进的是
          //   游客态而非登录页，这里是回到飞牛登录的唯一常规入口（BGM 与它无关，不受影响）。
          loggedIn={!guest}
          onLogin={() => {
            setSettingsOpen(false);
            setView("login");
          }}
          onLogout={() => {
            // [lc-1437] 退出登录：清会话 + 清首页快照（那是上一个账号的数据，
            // 留着会让下一个账号先看到别人的片单）。
            // [lc-1623] 但**不回登录页**：飞牛退出不该牵连 BGM（两者已解耦）——进游客态，
            // 发现页/BGM 面板照常可用；想重新登录飞牛走设置面板「账号 → 登录飞牛」。
            // Rust 侧 logout 只清飞牛 token，绝不碰 bgm_token（见 lib.rs）。
            setSettingsOpen(false);
            invoke("logout").catch(() => {});
            try {
              localStorage.removeItem("fntv-home-cache");
            } catch {
              /* ignore */
            }
            setCarousel([]);
            setCarouselBds({});
            setCarouselLogos({});
            setLibRows([]);
            setLibs([]);
            setContinueList([]);
            setGuest(true);
            setPage("home");
            setView("home");
            void loadGuestHome();
          }}
        />
      )}
      {/* [lc-1599] 反馈面板（工单制）：入口在侧栏快捷操作卡（反馈）。
          page = 当前视图名，随工单一起上报（日志环境块的 page=xxx 同源）。 */}
      {feedbackOpen && (
        <FeedbackPanel page={view} onClose={() => setFeedbackOpen(false)} />
      )}
      {/* [lc-1600] 游客「查看详情 / 播放」引导弹窗（portal 到 body，与详情页弹窗同一套） */}
      {guestAsk && (
        <GuestLoginPrompt
          onClose={() => setGuestAsk(false)}
          onGoLogin={() => {
            setGuestAsk(false);
            setView("login");
          }}
        />
      )}
      {/* [lc-1439] 关闭询问框：原生关闭按钮被 Rust 拦下后弹（三选一）。
          用 fixed 浮层而非复用设置面板 —— 它必须在任何页面状态下都能弹出来。
          [lc-1514] 玻璃化（半透明 + 动态模糊）+「记住我的选择」：勾上则把
          exit_mode 写进 settings，之后 Rust 的 CloseRequested 直接按记忆执行，不再问。 */}
      {closeAsk && (
        <div className="close-ask" onClick={() => setCloseAsk(false)}>
          <div className="close-ask-box" onClick={(e) => e.stopPropagation()}>
            <h3>关闭窗口</h3>
            <p>要退出程序，还是最小化到系统托盘继续在后台运行？</p>
            <label className="close-ask-remember">
              <input
                type="checkbox"
                checked={rememberClose}
                onChange={(e) => setRememberClose(e.target.checked)}
              />
              <span>记住我的选择，下次不再询问</span>
            </label>
            <div className="close-ask-acts">
              <button className="set-btn" onClick={() => setCloseAsk(false)}>
                取消
              </button>
              <button
                className="set-btn"
                onClick={() => {
                  setCloseAsk(false);
                  if (rememberClose) patchSettings({ exitMode: "tray" });
                  invoke("resolve_close", { action: "hide" }).catch(() => {});
                }}
              >
                最小化到托盘
              </button>
              <button
                className="set-btn danger"
                onClick={() => {
                  setCloseAsk(false);
                  if (rememberClose) patchSettings({ exitMode: "exit" });
                  invoke("resolve_close", { action: "exit" }).catch(() => {});
                }}
              >
                退出程序
              </button>
            </div>
          </div>
        </div>
      )}
      {/* [lc-1640] 刷新通道下发：discover/lib/history/detail 视图经 useRefresh 消费 tick 触发自身 force 刷新。 */}
      <RefreshContext.Provider value={refreshCtx}>{body}</RefreshContext.Provider>
      {/* [lc-1607] 首次启动的免责声明同意闸门：未同意过则弹（登录页/主界面都覆盖，portal 到 body）。
          「我已阅读并同意」→ 置位 localStorage 并关闭；「不同意并退出」→ 走仓库既有退出链路
          （resolve_close("exit") = app.exit(0)，退场清理必然执行）。 */}
      {disclaimerOpen && (
        <DisclaimerModal
          variant="first"
          onAgree={() => {
            try {
              localStorage.setItem(DISCLAIMER_ACK_KEY, "1");
            } catch {
              /* ignore */
            }
            setDisclaimerOpen(false);
          }}
          onExit={() => invoke("resolve_close", { action: "exit" }).catch(() => {})}
          onClose={() => setDisclaimerOpen(false)}
        />
      )}
    </div>
  );
}

/* ────────────────────────── [lc-1600] 游客登录引导弹窗 ──────────────────────────
   游客态点「查看详情 / 开始播放」时弹出。外壳复用仓库弹层（.ddet-mask 遮罩 + .ddet 卡片，
   同 SettingsPanel / 详情页简介弹窗的用法），portal 到 body —— 不被 .app-shell 的
   圆角/溢出与视图转场动画上下文裁剪。Esc / 点遮罩关闭。 */
function GuestLoginPrompt({ onClose, onGoLogin }: { onClose: () => void; onGoLogin: () => void }) {
  // [lc-1603] 关闭动画：先挂 `.gst-mask-closing` 让遮罩淡出 + 卡片缩下沉，动画走完再真卸载
  // （照 SeasonArchive 的既有模式；条件挂载本身没有退场动画，必须延迟卸载）
  const [closing, setClosing] = useState(false);
  const requestClose = () => {
    if (closing) return;
    setClosing(true);
    window.setTimeout(onClose, 170); // 与 CSS 动画时长对齐
  };
  // [lc-1603]「去登录」也先播退场动画再走
  const goLogin = () => {
    if (closing) return;
    setClosing(true);
    window.setTimeout(onGoLogin, 170);
  };
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") requestClose();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
    // requestClose 每次渲染都是新函数，但只读 closing —— 依赖里放 closing 语义等价
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [closing]);
  return createPortal(
    <div className={`ddet-mask${closing ? " gst-mask-closing" : ""}`} onClick={requestClose}>
      <div className="ddet gst-modal" onClick={(e) => e.stopPropagation()}>
        <div className="gst-badge">
          <IcoLock size={24} strokeWidth={2} />
        </div>
        <h3 className="gst-hd">登录后即可查看详情、播放与管理媒体库</h3>
        <p className="gst-sub">在设置里填入飞牛服务器地址与账号即可登录</p>
        <div className="gst-acts">
          <button className="gst-btn ghost" onClick={requestClose}>
            再看看
          </button>
          <button className="gst-btn primary" onClick={goLogin}>
            去登录
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

export default App;
