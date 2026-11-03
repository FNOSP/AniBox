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
 *
 * [lc-1942] 菜单改版：此前「选集 / 字幕 / 音频 / 弹幕 / 片头尾 / 版本 / 设置」只发
 * `script-binding uosc/*`，菜单画在 mpv 自己的 OSD 里（覆盖层之下、且内嵌态无法交互）——
 * 用户体感就是「点了没反应」。现在统一改成覆盖层内的磨砂玻璃面板（一个组件 + 多份数据）：
 *   · 面板数据全部来自 `player_menu_data`（真实轨道/章节/播放列表/字幕样式/弹幕状态）；
 *   · 设置项全部走 `mpv_set_prop` / `mpv_script_message` / `playlist-play-index`，无空桩；
 *   · 面板打开期间覆盖层保持「不穿透」，点空白/Esc/再点同一按钮关闭，只有一块面板存在。
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AudioLines,
  Check,
  ChevronLeft,
  ChevronRight,
  Layers,
  ListOrdered,
  ListVideo,
  Maximize2,
  MessageSquare,
  Minus,
  Pause,
  Pin,
  Play,
  Plus,
  Search,
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

/* ── [lc-1942] 菜单数据（与 Rust `player_menu_data` 对齐）── */
type MpvTrack = {
  id?: number;
  type?: string;
  title?: string;
  lang?: string;
  codec?: string;
  selected?: boolean;
  external?: boolean;
  default?: boolean;
  forced?: boolean;
  image?: boolean;
  channels?: number;
  "demux-w"?: number;
  "demux-h"?: number;
};

type MpvChapter = { title?: string; time?: number };
type MpvPlaylistItem = {
  title?: string;
  filename?: string;
  playing?: boolean;
  current?: boolean;
};

type MenuData = {
  ready?: boolean;
  tracks?: MpvTrack[];
  sid?: number | string | null;
  aid?: number | string | null;
  vid?: number | string | null;
  secondarySid?: number | string | null;
  subVisibility?: boolean;
  secondarySubVisibility?: boolean;
  speed?: number;
  volume?: number;
  subFontSize?: number;
  subPos?: number;
  subBorderSize?: number;
  chapters?: MpvChapter[];
  playlist?: MpvPlaylistItem[];
  playlistPos?: number;
  mediaTitle?: string;
  videoParams?: { w?: number; h?: number; rotate?: number; pixelformat?: string } | null;
  videoCodec?: string;
  audioCodec?: string;
  fps?: number;
  hwdec?: string;
  fileFormat?: string;
  danmakuOn?: boolean | null;
  danmakuDelay?: number | null;
  danmakuInfo?: string | null;
};

type SubItem = {
  kind: "xunlei" | "assrt" | "osub";
  id: string;
  title: string;
  hint: string;
};

type MenuId =
  | "episodes"
  | "subtitles"
  | "audio"
  | "speed"
  | "danmaku"
  | "chapters"
  | "video"
  | "settings";

type SubPage =
  | "tracks"
  | "secondary"
  | "subsettings"
  | "subsearch"
  | "subsize"
  | "subpos"
  | "speed"
  | "danmaku"
  | "playlist"
  | "info";

const fmt = (s?: number) => {
  const v = Math.max(0, Math.floor(s || 0));
  const h = Math.floor(v / 3600);
  const m = Math.floor((v % 3600) / 60);
  const sec = v % 60;
  const p = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${p(m)}:${p(sec)}` : `${m}:${p(sec)}`;
};

const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2, 3];

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const trackSelected = (cur: unknown, id?: number) => isNum(cur) && id != null && cur === id;

const trackName = (t: MpvTrack, i: number, kind: "sub" | "audio" | "video") => {
  if (t.title) return t.title;
  if (t.lang) return t.lang;
  return kind === "sub" ? `字幕 ${i + 1}` : kind === "audio" ? `音轨 ${i + 1}` : `画面 ${i + 1}`;
};

const trackMeta = (t: MpvTrack) => {
  const bits: string[] = [];
  if (t.lang) bits.push(t.lang);
  if (t.codec) bits.push(t.codec);
  if (t.channels) bits.push(`${t.channels}ch`);
  if (t.external) bits.push("外挂");
  if (t.forced) bits.push("强制");
  return bits.join(" · ");
};

/* ── 通用行 / 分隔线 / 提示 ── */
function MenuRow({
  text,
  meta,
  checked,
  chevron,
  onClick,
  disabled,
}: {
  text: string;
  meta?: string;
  checked?: boolean;
  chevron?: boolean;
  onClick?: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className="pc-menu-row"
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      disabled={disabled}
    >
      <span className="pc-menu-row-main">
        <span className="pc-menu-text">{text}</span>
        {meta ? <span className="pc-menu-meta">{meta}</span> : null}
      </span>
      {checked ? (
        <span className="pc-menu-check">
          <Check size={16} strokeWidth={2.5} />
        </span>
      ) : null}
      {chevron ? (
        <span className="pc-menu-chev">
          <ChevronRight size={15} strokeWidth={2.2} />
        </span>
      ) : null}
    </button>
  );
}

const Sep = () => <div className="pc-menu-sep" aria-hidden="true" />;
const Hint = ({ text }: { text: string }) => <div className="pc-menu-hint">{text}</div>;

export function PlayerChrome() {
  const [st, setSt] = useState<PState>({});
  const [visible, setVisible] = useState(true);
  const [seeking, setSeeking] = useState(false);
  const [seekPreview, setSeekPreview] = useState(0);

  /* ── [lc-1942] 磨砂菜单状态 ── */
  const [menu, setMenu] = useState<MenuId | null>(null);
  const [menuClosing, setMenuClosing] = useState(false);
  const [sub, setSub] = useState<SubPage | null>(null);
  const [mdata, setMdata] = useState<MenuData>({});
  const [menuPos, setMenuPos] = useState<{ left: number; top: number; maxHeight: number } | null>(
    null,
  );
  const [subQuery, setSubQuery] = useState("");
  const [subResults, setSubResults] = useState<SubItem[]>([]);
  const [subBusy, setSubBusy] = useState(false);
  const [subErr, setSubErr] = useState("");
  const [subNotice, setSubNotice] = useState("");

  const hideTimer = useRef<number | null>(null);
  const hoverInteractive = useRef(false);
  const dragLock = useRef(false);
  const ignoreRef = useRef(true);
  const lastProbe = useRef({ x: -1, y: -1 });
  const trackRef = useRef<HTMLDivElement | null>(null);
  const menuOpenRef = useRef(false);
  const menuPanelRef = useRef<HTMLDivElement | null>(null);
  const menuAnchorRef = useRef<DOMRect | null>(null);
  const menuTriggerRef = useRef<HTMLElement | null>(null);
  const menuCloseTimer = useRef<number | null>(null);
  const menuPulling = useRef(false);
  const keyboardFocusTimer = useRef<number | null>(null);

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

  /* ── 3 秒自动隐藏（面板打开时不隐藏）── */
  const wake = useCallback(() => {
    setVisible(true);
    if (hideTimer.current) window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => {
      if (hoverInteractive.current || dragLock.current || menuOpenRef.current) return;
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
        // [lc-1942] 面板打开期间不切回穿透：否则移出面板那一拍会把点击交给 mpv，
        // 面板外点击无法命中「关闭面板」逻辑。
        if (!dragLock.current && !menuOpenRef.current) setIgnore(true);
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
      if (menuOpenRef.current) {
        // 面板打开 = 模态：整窗可点（点空白处用于关闭面板）
        hoverInteractive.current = true;
        setIgnore(false);
        return;
      }
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
      if (menuOpenRef.current) {
        hoverInteractive.current = true;
        setIgnore(false);
        return;
      }
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

  /* ── [lc-1942] 菜单数据拉取（同一时刻只允许一发，避免慢响应叠队）── */
  const pullMenu = useCallback(async () => {
    if (menuPulling.current) return;
    menuPulling.current = true;
    try {
      const d = await invoke<MenuData>("player_menu_data");
      if (d) setMdata(d);
    } catch {
      /* 播放器未起时保持空态 */
    } finally {
      menuPulling.current = false;
    }
  }, []);

  useEffect(() => {
    if (!menu) return;
    void pullMenu();
    // 面板打开期间 1s 轮询：实时反映「当前轨 / 播放进度 / 弹幕开关」的变化
    const id = window.setInterval(() => void pullMenu(), 1000);
    return () => {
      window.clearInterval(id);
    };
  }, [menu, pullMenu]);

  /* ── 面板定位：贴着触发按钮上方，空间不足翻转，始终在窗口内 ── */
  const placeMenu = useCallback(() => {
    const panel = menuPanelRef.current;
    const a = menuAnchorRef.current;
    if (!panel || !a) return;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const card = (panel.firstElementChild as HTMLElement | null) ?? panel;
    const w = card.offsetWidth || 300;
    const rawH = card.offsetHeight || 320;
    const maxHeight = Math.max(120, vh - 24);
    const h = Math.min(rawH, maxHeight);
    let left = a.left + a.width / 2 - w / 2;
    left = Math.max(12, Math.min(left, vw - w - 12));
    let top = a.top - h - 12;
    if (top < 12) top = a.bottom + 12;
    top = Math.max(12, Math.min(top, vh - h - 12));
    setMenuPos({ left: Math.round(left), top: Math.round(top), maxHeight });
  }, []);

  useLayoutEffect(() => {
    if (!menu) {
      setMenuPos(null);
      return;
    }
    // [lc-1942] 布局期先摆一次（首帧不闪 0,0），再交给 ResizeObserver / resize 细调
    placeMenu();
    const panel = menuPanelRef.current;
    const ro = panel ? new ResizeObserver(() => placeMenu()) : null;
    if (panel && ro) ro.observe(panel);
    const onResize = () => placeMenu();
    window.addEventListener("resize", onResize);
    return () => {
      ro?.disconnect();
      window.removeEventListener("resize", onResize);
    };
  }, [menu, sub, placeMenu]);

  /* ── [lc-1942] 输入框键盘租约：覆盖层常驻 WS_EX_NOACTIVATE（点控件不抢 mpv 焦点），
   * 只有搜索输入框真正需要键盘时才临时摘掉，失焦/关面板装回（Polaris 同款租约）。 ── */
  const acquireKeyboard = useCallback((el: HTMLInputElement) => {
    void invoke("player_chrome_focusable", { on: true }).catch(() => {});
    // 摘 NOACTIVATE + set_focus 是异步的；50ms 后再 focus 一次，确保键盘真的落到输入框
    if (keyboardFocusTimer.current) window.clearTimeout(keyboardFocusTimer.current);
    keyboardFocusTimer.current = window.setTimeout(() => {
      keyboardFocusTimer.current = null;
      if (!document.contains(el)) return;
      try {
        el.focus();
      } catch {
        /* ignore */
      }
    }, 60);
  }, []);

  const releaseKeyboard = useCallback(() => {
    if (keyboardFocusTimer.current) {
      window.clearTimeout(keyboardFocusTimer.current);
      keyboardFocusTimer.current = null;
    }
    void invoke("player_chrome_focusable", { on: false }).catch(() => {});
  }, []);

  /* ── 打开 / 关闭（同一按钮再点一次、空白处、Esc 都关闭）── */
  const closeMenu = useCallback(() => {
    if (!menuOpenRef.current) return;
    menuOpenRef.current = false;
    releaseKeyboard();
    setMenuClosing(true);
    if (menuCloseTimer.current) window.clearTimeout(menuCloseTimer.current);
    // 减少动态偏好：没有退场动画就直接卸载，别让面板多停 200ms
    const delay = window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 200;
    menuCloseTimer.current = window.setTimeout(() => {
      setMenu(null);
      setMenuClosing(false);
      setSub(null);
      setMenuPos(null);
    }, delay);
  }, [releaseKeyboard]);

  /* 离开搜索子面板（返回 / 切菜单 / 关闭）时立刻归还键盘 */
  useEffect(() => {
    if (sub !== "subsearch") return;
    return () => releaseKeyboard();
  }, [sub, releaseKeyboard]);

  const openMenu = (id: MenuId, e: React.MouseEvent<HTMLElement>) => {
    const el = e.currentTarget;
    if (menu === id && !menuClosing) {
      closeMenu();
      return;
    }
    if (menuCloseTimer.current) window.clearTimeout(menuCloseTimer.current);
    menuTriggerRef.current = el;
    menuAnchorRef.current = el.getBoundingClientRect();
    menuOpenRef.current = true;
    setMenuClosing(false);
    setSub(null);
    setSubResults([]);
    setSubErr("");
    setSubNotice("");
    setMenuPos(null);
    setMenu(id);
    setIgnore(false);
    void pullMenu();
  };

  useEffect(() => {
    return () => {
      if (menuCloseTimer.current) window.clearTimeout(menuCloseTimer.current);
      if (keyboardFocusTimer.current) window.clearTimeout(keyboardFocusTimer.current);
    };
  }, []);

  useEffect(() => {
    if (!menu) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        closeMenu();
      }
    };
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node | null;
      if (!t) return;
      // 输入框内的指针操作保留键盘；面板内其它位置（行/按钮）归还键盘
      const el = t instanceof Element ? t : null;
      if (!el?.closest(".pc-menu-input")) releaseKeyboard();
      if (menuPanelRef.current?.contains(t)) return;
      if (menuTriggerRef.current?.contains(t)) return;
      closeMenu();
    };
    window.addEventListener("keydown", onKey, true);
    document.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      document.removeEventListener("pointerdown", onDown, true);
    };
  }, [menu, closeMenu, releaseKeyboard]);

  /* ── 菜单动作 ── */
  const applyProp = useCallback(
    async (name: string, value: unknown, closeAfter = false) => {
      try {
        await invoke("mpv_set_prop", { name, value });
      } catch {
        /* 写入失败时保持面板，轮询会显示真实状态 */
      }
      if (closeAfter) closeMenu();
      else void pullMenu();
    },
    [closeMenu, pullMenu],
  );

  const playIndex = useCallback(
    async (index: number) => {
      try {
        await invoke("mpv_playlist_play_index", { index });
      } catch {
        /* ignore */
      }
      closeMenu();
    },
    [closeMenu],
  );

  const seekTo = useCallback(
    (time?: number) => {
      if (!isNum(time)) return;
      cmd("mpv_seek", { seconds: time, absolute: true });
      closeMenu();
    },
    [cmd, closeMenu],
  );

  const toggleDanmaku = useCallback(async () => {
    await applyProp("user-data/uosc_danmaku/danmaku-switch", !mdata.danmakuOn);
  }, [applyProp, mdata.danmakuOn]);

  const nudgeDanmaku = useCallback(
    async (delta: number) => {
      const base = isNum(mdata.danmakuDelay) ? mdata.danmakuDelay : 0;
      const next = Math.max(-30, Math.min(30, Math.round((base + delta) * 10) / 10));
      cmd("mpv_script_message", { name: "uosc_danmaku", args: ["danmaku-delay", String(next)] });
      window.setTimeout(() => void pullMenu(), 180);
    },
    [cmd, mdata.danmakuDelay, pullMenu],
  );

  const searchSubs = useCallback(async () => {
    const term = subQuery.trim() || mdata.mediaTitle || title;
    if (!term) return;
    setSubBusy(true);
    setSubErr("");
    setSubNotice("");
    try {
      const r = await invoke<{ items?: SubItem[] }>("subtitle_search", { query: term });
      const items = Array.isArray(r?.items) ? r.items : [];
      setSubResults(items);
      if (items.length === 0) setSubErr("没有搜到字幕，换个关键词再试");
    } catch (e) {
      setSubErr(String(e));
    } finally {
      setSubBusy(false);
    }
  }, [subQuery, mdata.mediaTitle, title]);

  const downloadSub = useCallback(
    async (it: SubItem) => {
      setSubBusy(true);
      setSubErr("");
      setSubNotice("");
      try {
        const r = await invoke<{ title?: string; mounted?: boolean }>("subtitle_download", {
          kind: it.kind,
          id: it.id,
          filename: it.title,
        });
        setSubNotice(r?.mounted ? `已下载并挂载：${r.title ?? it.title}` : `已下载：${r.title ?? it.title}`);
        void pullMenu();
      } catch (e) {
        setSubErr(String(e));
      } finally {
        setSubBusy(false);
      }
    },
    [pullMenu],
  );

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

  /* ── 菜单数据派生 ── */
  const tracks = Array.isArray(mdata.tracks) ? mdata.tracks : [];
  const subTracks = tracks.filter((t) => t.type === "sub");
  const audTracks = tracks.filter((t) => t.type === "audio");
  const vidTracks = tracks.filter((t) => t.type === "video" && !t.image);
  const chapters = Array.isArray(mdata.chapters) ? mdata.chapters : [];
  const playlist = Array.isArray(mdata.playlist) ? mdata.playlist : [];
  const plPos = isNum(mdata.playlistPos) ? mdata.playlistPos : st.plPos ?? -1;
  const curSub = subTracks.find((t) => trackSelected(mdata.sid, t.id));
  const speedNow = isNum(mdata.speed) ? mdata.speed : speed;
  const subOn = mdata.sid !== "no" && mdata.subVisibility !== false;

  const currentChapter = useMemo(() => {
    let idx = -1;
    for (let i = 0; i < chapters.length; i += 1) {
      const t = chapters[i]?.time;
      if (isNum(t) && t <= pos + 0.2) idx = i;
      else break;
    }
    return idx;
  }, [chapters, pos]);

  const subParent = (p: SubPage) =>
    p === "tracks" || p === "secondary" || p === "subsettings" || p === "subsearch"
      ? "字幕"
      : "设置";

  /* ── 各菜单主体 ── */
  const speedRows = (
    <>
      {SPEEDS.map((v) => (
        <MenuRow
          key={v}
          text={`${v}×`}
          checked={Math.abs(speedNow - v) < 0.01}
          onClick={() => void applyProp("speed", v, true)}
        />
      ))}
    </>
  );

  const playlistRows = () =>
    playlist.length === 0 ? (
      <Hint text="没有可用的播放列表" />
    ) : (
      playlist.map((it, i) => (
        <MenuRow
          key={`${i}-${it.title ?? it.filename ?? ""}`}
          text={it.title || it.filename || `第 ${i + 1} 集`}
          checked={i === plPos}
          onClick={() => void playIndex(i)}
        />
      ))
    );

  const danmakuRows = (
    <>
      <MenuRow
        text="显示弹幕"
        meta={mdata.danmakuOn ? "开" : "关"}
        checked={!!mdata.danmakuOn}
        onClick={() => void toggleDanmaku()}
      />
      <Sep />
      <div className="pc-menu-slider-row">
        <div className="pc-menu-slider-head">
          <span className="pc-menu-text">弹幕偏移</span>
          <span className="pc-menu-value">
            {(isNum(mdata.danmakuDelay) ? mdata.danmakuDelay : 0).toFixed(1)} s
          </span>
        </div>
        <div className="pc-menu-stepper">
          <button
            type="button"
            className="pc-menu-step"
            aria-label="弹幕偏移减小"
            onClick={() => void nudgeDanmaku(-0.1)}
          >
            <Minus size={15} strokeWidth={2.4} />
          </button>
          <button
            type="button"
            className="pc-menu-step"
            aria-label="弹幕偏移增大"
            onClick={() => void nudgeDanmaku(0.1)}
          >
            <Plus size={15} strokeWidth={2.4} />
          </button>
        </div>
      </div>
      {mdata.danmakuInfo ? <Hint text={String(mdata.danmakuInfo)} /> : null}
    </>
  );

  const subSizeRow = (
    <div className="pc-menu-slider-row">
      <div className="pc-menu-slider-head">
        <span className="pc-menu-text">字号</span>
        <span className="pc-menu-value">{Math.round(mdata.subFontSize ?? 55)}</span>
      </div>
      <input
        className="pc-menu-range"
        type="range"
        min={20}
        max={80}
        step={1}
        value={Math.round(mdata.subFontSize ?? 55)}
        onChange={(e) => void applyProp("sub-font-size", Number(e.target.value))}
      />
    </div>
  );

  const subPosRow = (
    <div className="pc-menu-slider-row">
      <div className="pc-menu-slider-head">
        <span className="pc-menu-text">位置</span>
        <span className="pc-menu-value">{Math.round(mdata.subPos ?? 100)}</span>
      </div>
      <input
        className="pc-menu-range"
        type="range"
        min={0}
        max={150}
        step={1}
        value={Math.round(mdata.subPos ?? 100)}
        onChange={(e) => void applyProp("sub-pos", Number(e.target.value))}
      />
    </div>
  );

  const subBorderRow = (
    <div className="pc-menu-slider-row">
      <div className="pc-menu-slider-head">
        <span className="pc-menu-text">阴影</span>
        <span className="pc-menu-value">{(mdata.subBorderSize ?? 2.4).toFixed(1)}</span>
      </div>
      <input
        className="pc-menu-range"
        type="range"
        min={0}
        max={6}
        step={0.2}
        value={mdata.subBorderSize ?? 2.4}
        onChange={(e) => void applyProp("sub-border-size", Number(e.target.value))}
      />
    </div>
  );

  const infoRows = (
    <>
      <InfoRow label="分辨率" value={
        mdata.videoParams?.w && mdata.videoParams?.h
          ? `${mdata.videoParams.w} × ${mdata.videoParams.h}`
          : "—"
      } />
      <InfoRow label="帧率" value={isNum(mdata.fps) ? `${mdata.fps.toFixed(3)} fps` : "—"} />
      <InfoRow label="视频编码" value={mdata.videoCodec || "—"} />
      <InfoRow label="音频编码" value={mdata.audioCodec || "—"} />
      <InfoRow label="硬件解码" value={mdata.hwdec || "—"} />
      <InfoRow label="封装格式" value={mdata.fileFormat || "—"} />
    </>
  );

  const renderMain = () => {
    switch (menu) {
      case "episodes":
        return (
          <>
            <div className="pc-menu-head">
              <span className="pc-menu-title">选集</span>
              <span className="pc-menu-count">{playlist.length} 集</span>
            </div>
            <div className="pc-menu-list">{playlistRows()}</div>
          </>
        );
      case "subtitles":
        return (
          <>
            <div className="pc-menu-head">
              <span className="pc-menu-title">字幕</span>
              <button
                type="button"
                className="pc-menu-fab"
                title="搜索字幕"
                aria-label="搜索字幕"
                onClick={() => {
                  setSub("subsearch");
                  setSubQuery(mdata.mediaTitle || title || "");
                  setSubResults([]);
                  setSubErr("");
                  setSubNotice("");
                }}
              >
                <Search size={15} strokeWidth={2.2} />
              </button>
            </div>
            <MenuRow
              text="关闭字幕"
              checked={!subOn || mdata.sid === "no"}
              onClick={() => void applyProp("sid", "no")}
            />
            <Sep />
            <MenuRow
              text="字幕列表"
              meta={curSub ? trackName(curSub, 0, "sub") : "未启用"}
              chevron
              onClick={() => setSub("tracks")}
            />
            <MenuRow
              text="副字幕（双语）"
              meta={mdata.secondarySid === "no" || mdata.secondarySid == null ? "关闭" : "已启用"}
              chevron
              onClick={() => setSub("secondary")}
            />
            <MenuRow text="显示设置" chevron onClick={() => setSub("subsettings")} />
          </>
        );
      case "audio":
        return (
          <>
            <div className="pc-menu-head">
              <span className="pc-menu-title">音频</span>
              <span className="pc-menu-count">{audTracks.length} 轨</span>
            </div>
            <MenuRow
              text="静音音频"
              checked={mdata.aid === "no"}
              onClick={() => void applyProp("aid", "no", true)}
            />
            <Sep />
            <div className="pc-menu-list">
              {audTracks.length === 0 ? (
                <Hint text="未检测到音轨" />
              ) : (
                audTracks.map((t, i) => (
                  <MenuRow
                    key={t.id ?? i}
                    text={trackName(t, i, "audio")}
                    meta={trackMeta(t)}
                    checked={trackSelected(mdata.aid, t.id)}
                    onClick={() => void applyProp("aid", t.id, true)}
                  />
                ))
              )}
            </div>
          </>
        );
      case "speed":
        return (
          <>
            <div className="pc-menu-head">
              <span className="pc-menu-title">播放速度</span>
              <span className="pc-menu-count">{Number(speedNow.toFixed(2))}×</span>
            </div>
            {speedRows}
          </>
        );
      case "danmaku":
        return (
          <>
            <div className="pc-menu-head">
              <span className="pc-menu-title">弹幕</span>
            </div>
            {danmakuRows}
          </>
        );
      case "chapters":
        return (
          <>
            <div className="pc-menu-head">
              <span className="pc-menu-title">章节</span>
              <span className="pc-menu-count">{chapters.length} 段</span>
            </div>
            <div className="pc-menu-list">
              {chapters.length === 0 ? (
                <Hint text="本片没有章节信息" />
              ) : (
                chapters.map((c, i) => (
                  <MenuRow
                    key={`${i}-${c.time ?? 0}`}
                    text={c.title || `章节 ${i + 1}`}
                    meta={fmt(c.time)}
                    checked={i === currentChapter}
                    onClick={() => seekTo(c.time)}
                  />
                ))
              )}
            </div>
          </>
        );
      case "video":
        return (
          <>
            <div className="pc-menu-head">
              <span className="pc-menu-title">画面版本</span>
              <span className="pc-menu-count">{vidTracks.length} 轨</span>
            </div>
            {vidTracks.length === 0 ? (
              <Hint text="未检测到视频轨" />
            ) : (
              vidTracks.map((t, i) => (
                <MenuRow
                  key={t.id ?? i}
                  text={trackName(t, i, "video")}
                  meta={trackMeta(t)}
                  checked={trackSelected(mdata.vid, t.id)}
                  onClick={() => void applyProp("vid", t.id, true)}
                />
              ))
            )}
          </>
        );
      case "settings":
        return (
          <>
            <div className="pc-menu-head">
              <span className="pc-menu-title">设置</span>
            </div>
            <MenuRow
              text="播放速度"
              meta={`${Number(speedNow.toFixed(2))}×`}
              chevron
              onClick={() => setSub("speed")}
            />
            <MenuRow
              text="播放列表"
              meta={playlist.length > 0 ? `${plPos + 1}/${playlist.length}` : "—"}
              chevron
              onClick={() => setSub("playlist")}
            />
            <Sep />
            <MenuRow
              text="字幕字号"
              meta={String(Math.round(mdata.subFontSize ?? 55))}
              chevron
              onClick={() => setSub("subsize")}
            />
            <MenuRow
              text="字幕位置"
              meta={String(Math.round(mdata.subPos ?? 100))}
              chevron
              onClick={() => setSub("subpos")}
            />
            <MenuRow
              text="弹幕"
              meta={mdata.danmakuOn ? "开" : "关"}
              chevron
              onClick={() => setSub("danmaku")}
            />
            <Sep />
            <MenuRow text="视频信息" chevron onClick={() => setSub("info")} />
          </>
        );
      default:
        return null;
    }
  };

  const renderSub = () => {
    if (!sub) return null;
    switch (sub) {
      case "tracks":
        return (
          <>
            <BackRow parent={subParent(sub)} onBack={() => setSub(null)} />
            <div className="pc-menu-title pc-menu-subtitle">字幕列表</div>
            <div className="pc-menu-list">
              <MenuRow
                text="关闭字幕"
                checked={mdata.sid === "no"}
                onClick={() => void applyProp("sid", "no", true)}
              />
              <Sep />
              {subTracks.length === 0 ? (
                <Hint text="未检测到字幕轨" />
              ) : (
                subTracks.map((t, i) => (
                  <MenuRow
                    key={t.id ?? i}
                    text={trackName(t, i, "sub")}
                    meta={trackMeta(t)}
                    checked={trackSelected(mdata.sid, t.id)}
                    onClick={() => void applyProp("sid", t.id, true)}
                  />
                ))
              )}
            </div>
          </>
        );
      case "secondary":
        return (
          <>
            <BackRow parent={subParent(sub)} onBack={() => setSub(null)} />
            <div className="pc-menu-title pc-menu-subtitle">副字幕（双语）</div>
            <div className="pc-menu-list">
              <MenuRow
                text="关闭副字幕"
                checked={mdata.secondarySid === "no" || mdata.secondarySid == null}
                onClick={() => void applyProp("secondary-sid", "no")}
              />
              <Sep />
              {subTracks.length === 0 ? (
                <Hint text="未检测到字幕轨" />
              ) : (
                subTracks.map((t, i) => (
                  <MenuRow
                    key={t.id ?? i}
                    text={trackName(t, i, "sub")}
                    meta={trackMeta(t)}
                    checked={trackSelected(mdata.secondarySid, t.id)}
                    onClick={() => {
                      void applyProp("secondary-sid", t.id);
                      void applyProp("secondary-sub-visibility", true);
                      closeMenu();
                    }}
                  />
                ))
              )}
            </div>
            <Hint text="副字幕显示在主字幕正下方，可在「显示设置」里调整字号与位置。" />
          </>
        );
      case "subsettings":
        return (
          <>
            <BackRow parent={subParent(sub)} onBack={() => setSub(null)} />
            <div className="pc-menu-title pc-menu-subtitle">显示设置</div>
            {subSizeRow}
            {subPosRow}
            {subBorderRow}
          </>
        );
      case "subsize":
        return (
          <>
            <BackRow parent={subParent(sub)} onBack={() => setSub(null)} />
            <div className="pc-menu-title pc-menu-subtitle">字幕字号</div>
            {subSizeRow}
          </>
        );
      case "subpos":
        return (
          <>
            <BackRow parent={subParent(sub)} onBack={() => setSub(null)} />
            <div className="pc-menu-title pc-menu-subtitle">字幕位置</div>
            {subPosRow}
          </>
        );
      case "subsearch":
        return (
          <>
            <BackRow parent={subParent(sub)} onBack={() => setSub(null)} />
            <div className="pc-menu-title pc-menu-subtitle">搜索字幕</div>
            <div className="pc-menu-searchbar">
              <input
                className="pc-menu-input"
                value={subQuery}
                placeholder="输入剧名 / 电影名"
                onPointerDown={(e) => acquireKeyboard(e.currentTarget)}
                onFocus={(e) => acquireKeyboard(e.currentTarget)}
                onChange={(e) => setSubQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !subBusy) void searchSubs();
                }}
              />
              <button
                type="button"
                className="pc-menu-search-btn"
                disabled={subBusy}
                onClick={() => void searchSubs()}
              >
                {subBusy ? "…" : "搜索"}
              </button>
            </div>
            {subErr ? <div className="pc-menu-hint pc-menu-hint-error">{subErr}</div> : null}
            {subNotice ? <div className="pc-menu-hint">{subNotice}</div> : null}
            <div className="pc-menu-list">
              {subResults.map((it) => (
                <MenuRow
                  key={`${it.kind}:${it.id}`}
                  text={it.title}
                  meta={`${it.kind === "xunlei" ? "迅雷" : it.kind === "assrt" ? "assrt" : "OpenSubs"} · ${it.hint}`}
                  chevron
                  disabled={subBusy}
                  onClick={() => void downloadSub(it)}
                />
              ))}
            </div>
          </>
        );
      case "speed":
        return (
          <>
            <BackRow parent={subParent(sub)} onBack={() => setSub(null)} />
            <div className="pc-menu-title pc-menu-subtitle">播放速度</div>
            {speedRows}
          </>
        );
      case "playlist":
        return (
          <>
            <BackRow parent={subParent(sub)} onBack={() => setSub(null)} />
            <div className="pc-menu-title pc-menu-subtitle">播放列表</div>
            <div className="pc-menu-list">{playlistRows()}</div>
          </>
        );
      case "danmaku":
        return (
          <>
            <BackRow parent={subParent(sub)} onBack={() => setSub(null)} />
            <div className="pc-menu-title pc-menu-subtitle">弹幕</div>
            {danmakuRows}
          </>
        );
      case "info":
        return (
          <>
            <BackRow parent={subParent(sub)} onBack={() => setSub(null)} />
            <div className="pc-menu-title pc-menu-subtitle">视频信息</div>
            {infoRows}
          </>
        );
      default:
        return null;
    }
  };

  return (
    <div className={`player-chrome${visible || loading || menu ? "" : " pc-hidden"}`}>
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
                className={`pc-btn pc-icon${menu === "episodes" ? " pc-open" : ""}`}
                data-tip="选集"
                onClick={(e) => openMenu("episodes", e)}
              >
                <ListVideo size={17} />
              </button>
              <button
                type="button"
                className={`pc-btn pc-icon${menu === "subtitles" ? " pc-open" : ""}`}
                data-tip="字幕"
                onClick={(e) => openMenu("subtitles", e)}
              >
                <Subtitles size={17} />
              </button>
              <button
                type="button"
                className={`pc-btn pc-icon${menu === "audio" ? " pc-open" : ""}`}
                data-tip="音频"
                onClick={(e) => openMenu("audio", e)}
              >
                <AudioLines size={17} />
              </button>
            </div>
          </div>

          <div className="pc-center">
            <div className="pc-dock pc-dock-bare" data-chrome-hit>
              <button
                type="button"
                className={`pc-btn pc-text${menu === "speed" ? " pc-open" : ""}`}
                data-tip="播放速度"
                onClick={(e) => openMenu("speed", e)}
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
                className={`pc-btn pc-icon${menu === "danmaku" ? " pc-open" : ""}`}
                data-tip="弹幕"
                onClick={(e) => openMenu("danmaku", e)}
              >
                <MessageSquare size={17} />
              </button>
              <button
                type="button"
                className={`pc-btn pc-icon${menu === "chapters" ? " pc-open" : ""}`}
                data-tip="片头片尾"
                onClick={(e) => openMenu("chapters", e)}
              >
                <ListOrdered size={17} />
              </button>
              <button
                type="button"
                className={`pc-btn pc-icon${menu === "video" ? " pc-open" : ""}`}
                data-tip="版本"
                onClick={(e) => openMenu("video", e)}
              >
                <Layers size={17} />
              </button>
              <button
                type="button"
                className={`pc-btn pc-icon${menu === "settings" ? " pc-open" : ""}`}
                data-tip="设置"
                onClick={(e) => openMenu("settings", e)}
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

      {/* ── [lc-1942] 通用磨砂菜单：一个组件、多份数据 ── */}
      {menu ? (
        <div
          ref={menuPanelRef}
          data-chrome-hit
          className={`pc-menu pc-menu-${menu}${menuClosing ? " is-closing" : ""}`}
          style={
            menuPos
              ? { left: menuPos.left, top: menuPos.top, maxHeight: menuPos.maxHeight }
              : { left: 0, top: 0, visibility: "hidden" }
          }
        >
          <div
            className="pc-menu-card"
            style={menuPos ? { maxHeight: menuPos.maxHeight } : undefined}
          >
            <div className={`pc-menu-body${sub ? " show-sub" : ""}`}>
              <div className="pc-menu-page pc-menu-page-main">
                <div className="pc-menu-scroll">{renderMain()}</div>
              </div>
              {sub ? (
                <div className="pc-menu-page pc-menu-page-sub">
                  <div className="pc-menu-scroll">{renderSub()}</div>
                </div>
              ) : null}
            </div>
          </div>
        </div>
      ) : null}

      {/* 加载态：mpv 出画前（Spinner + 片名），loaded 后淡出 */}
      <div className={`pc-opening${loading ? "" : " gone"}`} aria-hidden={!loading}>
        <span className="player-spin" />
        <span className="pc-opening-title">{title || "正在打开"}</span>
      </div>
    </div>
  );
}

function BackRow({ parent, onBack }: { parent: string; onBack: () => void }) {
  return (
    <button type="button" className="pc-menu-back" onClick={onBack}>
      <ChevronLeft size={16} strokeWidth={2.5} />
      <span>{parent}</span>
    </button>
  );
}

function InfoRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="pc-menu-info">
      <span className="pc-menu-info-label">{label}</span>
      <span className="pc-menu-info-value" title={value}>
        {value}
      </span>
    </div>
  );
}
