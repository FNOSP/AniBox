/* [lc-1939] 播放器覆盖层 UI —— 交互模型逐条照抄 Polaris 的 mpv-overlay：
 *
 *   · 覆盖层窗 layered + 透明 + NOACTIVATE，与 mpv 窗口 1:1 等大、常驻置顶；
 *   · [lc-1944] 命中机制**就是 Polaris 那一套**：Rust 16ms 探针按光标与
 *     `[data-chrome-hit]` 矩形比 → 只拨 `WS_EX_TRANSPARENT`（Polaris 是 50ms +
 *     eval/elementFromPoint/invoke/ack 四个来回；我们单跳，端到端 ≤16ms）。
 *     已逐条排除「OS 级同步命中」：`WM_NCHITTEST→HTTRANSPARENT` 只对**同线程**窗口
 *     下传（Win32 文档/Wine 实现），mpv 是独立进程收不到；`SetWindowRgn` 会裁掉
 *     底部对比渐变。所以「点半天」的真因不在翻位窗口（实测 ≤16ms），而在 mpv 命令队列
 *     （见下条）。`mpv-overlay.html` 里的白名单、3s 隐藏、单击/双击语义逐条对齐。
 *   · [lc-1943] 可点区由页面在布局/显隐变化时上报一次（`player_chrome_hot_rects`，
 *     窗口内物理像素，`getBoundingClientRect × dpr`）；探针同时负责「光标在动 → 续显」
 *     与「悬在可点区 → 3s 自动隐藏让路」。
 *   · 状态：`mpv-pstate` 事件为主通道 + 500ms invoke 快照兜底（`capabilities/overlay.json`
 *     给了覆盖层 event listen 权限，lc-1943 之前这里被 ACL 拒绝，只能等 250ms 轮询）。
 *   · 控件一律 `pointerdown` 立即执行（`click` 只做键盘/兜底），并做乐观 UI：
 *     点击当帧先翻转本地状态，mpv 回报只做校正。
 *   · [lc-1944] 空白区仲裁照抄 Polaris：落进可点区但不是白名单元素的一击 → 单击
 *     播放/暂停（等系统双击间隔确认）、双击全屏、位移 ≥6px 丢弃；否则这一击会被
 *     WebView 静默吞掉，用户体感就是「点了没反应，再点一下」。
 *   · 3 秒无操作自动隐藏（Polaris hideTimeout=3.0）。
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
 *   · 面板打开期间可点区 = 整窗（模态），点空白/Esc/再点同一按钮关闭。
 *
 * [lc-1945] 控件弹窗换成用户给的 HTML/CSS 规格（覆盖此前从 Polaris 抄的数值）：
 *   · 白 45% + blur(28px) / 圆角 24 / 白描边 + 内高光 / 底部 8px 三角指针；
 *   · 面板底边 = 触发按钮顶边 - 80px（给进度条那一带留物理空白），跟随按钮居中；
 *   · 悬停打开（驻留 100ms）+ 移开 300ms 关闭 + 120×110 隐形连接桥；
 *     点击/触摸打开为固定态（移开不关），点空白 / Esc 关闭；
 *   · 布尔项改 48×28 胶囊开关，子页项 标签 + chevron，8 面板 + 10 子页同一套皮。
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AudioLines,
  Check,
  ChevronLeft,
  ChevronRight,
  Copy,
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
  /** [lc-1946] 播放器是否最大化（mpv 锚窗 IsZoomed）；顶栏按钮换图标用 */
  maximized?: boolean;
  phase?: string;
  title?: string;
};

/* ── [lc-1943] 延迟采样：页面侧拿不到进程日志，把「点击 → invoke 返回」这类单次测量
 * 经 `lat_trace` 写进 `anibox.log`（`[lat]` 前缀，与 Rust 侧 `[lat] mpv cmd=… rtt=…ms`
 * 一起拼出完整链路）。只在点击动作上打点，不跟帧、不刷日志。 */
const LAT_TRACE = true;
let latT0 = 0;
let latLabel = "";
function latMark(label: string) {
  if (!LAT_TRACE) return;
  latT0 = performance.now();
  latLabel = label;
}
function latDone(extra?: string) {
  if (!LAT_TRACE || !latT0) return;
  const dt = (performance.now() - latT0).toFixed(1);
  const msg = `ui ${latLabel}${extra ? ` ${extra}` : ""} ${dt}ms`;
  latT0 = 0;
  void invoke("lat_trace", { msg }).catch(() => {});
}

/* mpv 属性名 → 菜单数据字段：乐观写入用（等 mpv 回报再校正，勾选不必等一轮拉取）。 */
const PROP_FIELD: Record<string, keyof MenuData> = {
  speed: "speed",
  sid: "sid",
  aid: "aid",
  vid: "vid",
  "secondary-sid": "secondarySid",
  "sub-visibility": "subVisibility",
  "secondary-sub-visibility": "secondarySubVisibility",
  "sub-font-size": "subFontSize",
  "sub-pos": "subPos",
  "sub-border-size": "subBorderSize",
  "user-data/uosc_danmaku/danmaku-switch": "danmakuOn",
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

/* ── 通用行 / 分隔线 / 提示 ──
 * [lc-1945] 行型按用户规格分三类：
 *   · `toggle`  → 标签 + 48×28 胶囊开关（布尔项，如「显示弹幕」）；
 *   · `chevron` → 标签 + chevron（进入子页）；
 *   · `checked` → 标签 + ✓（列表里的单选：字幕轨 / 章节 / 倍速）。 */
function MenuRow({
  text,
  meta,
  checked,
  chevron,
  toggle,
  onClick,
  disabled,
}: {
  text: string;
  meta?: string;
  checked?: boolean;
  chevron?: boolean;
  toggle?: boolean;
  onClick?: () => void;
  disabled?: boolean;
}) {
  /* [lc-1943] 按下即执行：`click` 要等 pointerup（用户按多久就等多久）才派发。 */
  const lastTap = useRef(0);
  const fire = () => {
    if (disabled) return;
    onClick?.();
  };
  return (
    <button
      type="button"
      role={toggle ? "switch" : undefined}
      aria-checked={toggle ? !!checked : undefined}
      className={`pc-menu-row${toggle ? " is-switch" : ""}${chevron ? " is-nav" : ""}`}
      onMouseDown={(e) => e.preventDefault()}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        lastTap.current = performance.now();
        fire();
      }}
      onClick={() => {
        if (performance.now() - lastTap.current < 700) return;
        fire();
      }}
      disabled={disabled}
    >
      <span className="pc-menu-row-main">
        <span className="pc-menu-text">{text}</span>
        {meta ? <span className="pc-menu-meta">{meta}</span> : null}
      </span>
      {toggle ? (
        <span className={`pc-switch${checked ? " on" : ""}`} aria-hidden="true">
          <span className="pc-switch-knob" />
        </span>
      ) : checked ? (
        <span className="pc-menu-check">
          <Check size={18} strokeWidth={2.5} />
        </span>
      ) : null}
      {chevron ? (
        <span className="pc-menu-chev">
          <ChevronRight size={18} strokeWidth={2.5} strokeLinecap="round" strokeLinejoin="round" />
        </span>
      ) : null}
    </button>
  );
}

const Sep = () => <div className="pc-menu-sep" aria-hidden="true" />;
const Hint = ({ text }: { text: string }) => <div className="pc-menu-hint">{text}</div>;

/* ── [lc-1945] 面板几何（用户规格的落点）──
 *   `bottom: calc(100% + 80px)` 的等价实现：面板底边 = 触发按钮顶边 - 80px，
 *   这 80px 物理空白正是进度条经过的那一带；水平跟随触发按钮居中（不是窗口居中）。
 *   窗口不够高时翻到按钮下方（`side: "below"`），指针方向跟着翻。 */
const MENU_W = 300;
const MENU_GAP = 80; // 面板与触发按钮之间的物理空白
const MENU_MARGIN = 12; // 贴窗口边缘的安全边距
const MENU_ARROW_KEEPOUT = 26; // 三角指针离面板左右圆角的最小距离
const HOVER_CLOSE_MS = 300; // 移开 300ms 关闭（用户规格）
const HOVER_OPEN_MS = 100; // 悬停驻留 100ms 才开：穿过底栏时不连开 8 个面板
const BRIDGE_W = 120; // 隐形连接桥（用户规格）
const BRIDGE_H = 110;

type MenuPlacement = {
  left: number;
  side: "above" | "below";
  top?: number;
  bottom?: number;
  maxHeight: number;
  arrowX: number;
  bridgeLeft: number;
  bridgeTop: number;
};

export function PlayerChrome() {
  const [st, setSt] = useState<PState>({});
  const [visible, setVisible] = useState(true);
  const [seeking, setSeeking] = useState(false);
  const [seekPreview, setSeekPreview] = useState(0);

  /* ── [lc-1942] 磨砂菜单状态 ── */
  const [menu, setMenu] = useState<MenuId | null>(null);
  const [menuClosing, setMenuClosing] = useState(false);
  /* [lc-1945] 入场/退场由 `.is-active` 类驱动 CSS 过渡（规格的 opacity/transform/visibility）
   * 而不是 keyframes —— 面板用 `left` + `translateX(-50%)` 定水平位置，keyframes 的
   * transform 会把它覆盖掉。 */
  const [menuActive, setMenuActive] = useState(false);
  const [sub, setSub] = useState<SubPage | null>(null);
  const [mdata, setMdata] = useState<MenuData>({});
  const [menuPos, setMenuPos] = useState<MenuPlacement | null>(null);
  const [subQuery, setSubQuery] = useState("");
  const [subResults, setSubResults] = useState<SubItem[]>([]);
  const [subBusy, setSubBusy] = useState(false);
  const [subErr, setSubErr] = useState("");
  const [subNotice, setSubNotice] = useState("");

  const hideTimer = useRef<number | null>(null);
  const hoverInteractive = useRef(false);
  const dragLock = useRef(false);
  const trackRef = useRef<HTMLDivElement | null>(null);
  const menuOpenRef = useRef(false);
  const menuPanelRef = useRef<HTMLDivElement | null>(null);
  const menuAnchorRef = useRef<DOMRect | null>(null);
  const menuTriggerRef = useRef<HTMLElement | null>(null);
  const menuCloseTimer = useRef<number | null>(null);
  /* [lc-1945] 悬停交互：`menuPinned` = 点击打开（固定态，移开不关），
   * `hoverOpenTimer` / `hoverCloseTimer` = 悬停打开的驻留等待 / 移开后的关闭等待。 */
  const menuPinned = useRef(false);
  const hoverOpenTimer = useRef<number | null>(null);
  const hoverCloseTimer = useRef<number | null>(null);
  const hoverTarget = useRef<MenuId | null>(null);
  const menuPulling = useRef(false);
  const menuPullAgain = useRef(false);
  const pullTimer = useRef<number | null>(null);
  const keyboardFocusTimer = useRef<number | null>(null);
  const tapAt = useRef(0);
  /* [lc-1944] 空白区仲裁的抑制窗口（关闭面板/拖拽后的余波点击不再算作「点画面」） */
  const ptSuppressUntil = useRef(0);
  /* [lc-1946] 置顶为乐观状态：mpv 忙时属性回包可能迟到数秒，期间不允许 500ms 快照
   * 用旧值把图标打回去。5s 内以本地点击结果为准。 */
  const ontopLocalAt = useRef(0);
  /* [lc-1946] 关闭只跑一次：退出清理最长 2s，用户会连点；重复 invoke 会被 Rust 去重，
   * 前端也先挡一层，并立刻把控件层淡出给出反馈。 */
  const closingRef = useRef(false);
  const [closing, setClosing] = useState(false);

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

  /** 带 `[lat]` 计时的命令：量「点击 → invoke 返回」（含 IPC + Rust 处理 + mpv 往返） */
  const cmdLat = useCallback((label: string, name: string, args?: Record<string, unknown>) => {
    latMark(label);
    invoke(name, args as never)
      .then(() => latDone())
      .catch(() => latDone("err"));
  }, []);

  /** [lc-1943] 按下即执行：`click` 要等 pointerup（用户按多久就等多久）+ 一次事件派发，
   * 这是「点了要等一会儿」里唯一可控的固定开销。`pointerdown` 立刻触发，
   * `onClick` 保留给键盘/兜底，700ms 内去重（一次物理点击只执行一次）。 */
  const tap = useCallback(
    (fn: (el: HTMLElement) => void) => ({
      onMouseDown: (e: React.MouseEvent) => e.preventDefault(),
      onPointerDown: (e: React.PointerEvent<HTMLButtonElement>) => {
        if (e.button !== 0) return;
        tapAt.current = performance.now();
        // [lc-1946] 按下当帧的可见反馈：`preventDefault` 会让部分场景拿不到 `:active`，
        // 这里显式打一个 160ms 的按下态类（顶栏窗控「点了没反应」首先是**看不出按下**）。
        const el = e.currentTarget;
        el.classList.add("pc-pressed");
        window.setTimeout(() => el.classList.remove("pc-pressed"), 160);
        fn(el);
      },
      onClick: (e: React.MouseEvent<HTMLButtonElement>) => {
        if (performance.now() - tapAt.current < 700) return;
        fn(e.currentTarget);
      },
    }),
    [],
  );

  /* ── [lc-1943] 可点区上报：穿透判定由 Rust 探针独占，页面只负责「哪些矩形可点」──
   * 只在布局/显隐变化时上报（一次交互最多 1 次 IPC），不再每帧 eval + invoke 往返。
   *
   * [lc-1946] 顶栏窗控固定热区：auto-hide 时 `.pc-layer` 会 opacity 0 + 位移 10px，
   * 此时按 DOM 实测值上报会把顶栏按钮报成「不可点」→ 探针保持全窗穿透 → 光标移到
   * 右上角也是穿透态，点击直接落到 mpv（用户体感 = 右上角按钮点了没反应）。
   * 这里单独缓存最后一次**可见时**实测的 `.pc-top-right` 矩形（物理像素），隐藏时用
   * 缓存值上报；窗口第一次上报之前用常量兜底。缓存只在实测到正尺寸时刷新，不读隐藏态。 */
  const TOP_CLUSTER_W = 200; // CSS px：4×28 圆钮 + 3×12 间距 + 24 右内距 + 余量（兜底）
  const TOP_CLUSTER_H = 64;
  const TOP_CLUSTER_RIGHT = 24;
  /** 缓存：距窗口右边的内距 / 顶 / 宽 / 高（CSS px）。存相对值，窗口 resize 后仍正确。 */
  const topHotRef = useRef<{ right: number; top: number; w: number; h: number } | null>(null);
  const fixedTopHot = useCallback(
    (dpr: number, measure: boolean): [number, number, number, number] => {
      // 只在控件**可见**时实测并刷新缓存：隐藏态带 translateY(10px)，测到的不是可用值
      const el = measure ? document.querySelector(".pc-top-right") : null;
      if (el) {
        const r = el.getBoundingClientRect();
        if (r.width > 0 && r.height > 0) {
          topHotRef.current = {
            right: (window.innerWidth || 0) - r.right,
            top: r.top,
            w: r.width,
            h: r.height,
          };
        }
      }
      const vw = window.innerWidth || 0;
      const c = topHotRef.current;
      const w = c ? c.w : Math.min(TOP_CLUSTER_W, vw);
      const h = c ? c.h : TOP_CLUSTER_H;
      const top = c ? c.top : 0;
      const left = Math.max(0, vw - (c ? c.right : TOP_CLUSTER_RIGHT) - w);
      return [
        Math.round(left * dpr),
        Math.round(top * dpr),
        Math.round(w * dpr),
        Math.round(h * dpr),
      ];
    },
    [],
  );

  const layoutRef = useRef({ showChrome: true, full: false });
  layoutRef.current = { showChrome: visible || loading, full: !!menu || seeking };
  const rectRaf = useRef<number | null>(null);
  const reportRects = useCallback(() => {
    if (rectRaf.current !== null) window.cancelAnimationFrame(rectRaf.current);
    rectRaf.current = window.requestAnimationFrame(() => {
      rectRaf.current = null;
      const dpr = window.devicePixelRatio || 1;
      const { showChrome, full } = layoutRef.current;
      const rects: number[][] = [];
      // 控件隐藏时不报普通矩形 → Rust 恢复全窗穿透（点击继续交给 mpv）
      if (showChrome || full) {
        /* [lc-1945] 一并上报悬停专用区（隐形连接桥）：穿透态下 WebView 收不到
         * mousemove，只有被探针判为可点的矩形里才有 hover；桥必须算在内，
         * 否则从按钮移向面板途中会先掉 hover 再重新进入，面板闪退。
         * 它不带 `data-chrome-hit`，所以点击白名单/空白区仲裁不受影响。 */
        document.querySelectorAll("[data-chrome-hit],[data-chrome-hover]").forEach((el) => {
          const r = el.getBoundingClientRect();
          if (r.width <= 0 || r.height <= 0) return;
          rects.push([
            Math.round(r.left * dpr),
            Math.round(r.top * dpr),
            Math.round(r.width * dpr),
            Math.round(r.height * dpr),
          ]);
        });
      }
      // [lc-1946] 顶栏窗控热区**始终**上报（auto-hide 与否都算可点）：
      // 只有这样光标移入右上角时探针才翻成「可点」，hover/点击才能到 WebView。
      rects.push(fixedTopHot(dpr, showChrome || full));
      void invoke("player_chrome_hot_rects", { rects, full }).catch(() => {});
    });
  }, [fixedTopHot]);

  useEffect(() => {
    reportRects();
  }, [visible, loading, menu, menuPos, menuClosing, sub, seeking, reportRects]);

  useEffect(() => {
    const onResize = () => reportRects();
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      if (rectRaf.current !== null) window.cancelAnimationFrame(rectRaf.current);
      // 卸载时清空可点区：别把一个已经没人处理的窗口留在「可点」态
      void invoke("player_chrome_hot_rects", { rects: [], full: false }).catch(() => {});
    };
  }, [reportRects]);

  /* [lc-1946] 新一次播放（phase=start）重置关闭态：覆盖层窗口跨会话常驻，
   * 上一轮关闭时置上的 `closing` 必须清掉，否则下次起播控件层仍是被淡出的。 */
  useEffect(() => {
    if (st.phase !== "start") return;
    closingRef.current = false;
    setClosing(false);
  }, [st.phase]);

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

  /* ── [lc-1943] Rust 探针的「光标移动」通知：续显控件 / 记录是否悬在可点区 ── */
  useEffect(() => {
    let alive = true;
    let un: (() => void) | undefined;
    listen<{ x: number; y: number; hot: boolean; inside: boolean }>("overlay-cursor", (e) => {
      if (!alive) return;
      hoverInteractive.current = !!e.payload?.hot;
      if (e.payload?.inside) wake();
    })
      .then((f) => {
        un = f;
      })
      .catch(() => {
        /* 非 Tauri 环境：控件仍可点，只是不再自动续显（3s 后隐藏） */
      });
    return () => {
      alive = false;
      un?.();
    };
  }, [wake]);

  /* ── [lc-1944] 空白区点击仲裁（照抄 Polaris mpv-overlay 的 passthrough arbiter）──
   * 画面区点击在穿透态下直接归 mpv；但当光标落在可点区（含 16px 外扩 / 控件内距）
   * 而命中的不是白名单元素时，这一击会进 WebView 然后什么都不发生 —— 用户体感就是
   * 「点了没反应，再点一下」。这里按 Polaris 的语义把它翻译成最终动作，绝不静默丢弃。
   *
   * Polaris 原值逐个照搬：拖拽阈值 6px、拖后抑制 900ms、单击/双击间隔取
   * **系统双击间隔**（`GetDoubleClickTime`，不硬编码 500ms）；单击 = 播放/暂停、
   * 双击 = 全屏。绝不向 mpv 合成 mbtn_left —— 会和 Lua 的单击计时器互相抵消。 */
  const stRef = useRef(st);
  stRef.current = st;
  useEffect(() => {
    const DRAG_THRESHOLD = 6;
    const DRAG_SUPPRESS_MS = 900;
    let dblMs = 500;
    let dblReady = false;
    let armed: { gen: number; at: number } | null = null;
    let singleTimer: number | null = null;
    let track: {
      x: number;
      y: number;
      moved: boolean;
      clickCount: number;
      suppressed: boolean;
      doubleDispatched?: boolean;
    } | null = null;
    let generation = 0;

    const commitArmed = () => {
      if (!armed) return;
      const { gen, at } = armed;
      armed = null;
      if (gen !== generation) return;
      singleTimer = window.setTimeout(() => {
        singleTimer = null;
        if (gen !== generation || Date.now() < ptSuppressUntil.current) return;
        setSt((p) => ({ ...p, paused: !p.paused }));
        void invoke("mpv_toggle_pause").catch(() => {});
      }, Math.max(0, dblMs - (Date.now() - at)));
    };

    void invoke<number>("overlay_double_click_interval_ms")
      .then((v) => {
        const n = Number(v);
        if (Number.isFinite(n) && n > 0 && n <= 5000) dblMs = Math.max(1, Math.round(n));
      })
      .catch(() => {})
      .then(() => {
        dblReady = true;
        if (armed) commitArmed();
      });

    /* 白名单语义与 Rust 上报的 `[data-chrome-hit]` 同一份（Polaris 的
     * `_isOverlayHoverInteractive` 同款）：点击瞬间已被同步摘下的节点仍算控件。 */
    const interactive = (t: EventTarget | null) => {
      const el = t instanceof Element ? t : null;
      if (el && !el.isConnected) return true;
      return !!(el && el.closest("[data-chrome-hit]"));
    };
    const cancelSingle = () => {
      generation++;
      if (singleTimer !== null) {
        window.clearTimeout(singleTimer);
        singleTimer = null;
      }
    };

    const onDown = (e: MouseEvent) => {
      if (e.button !== 0) return;
      if (interactive(e.target)) {
        cancelSingle();
        return;
      }
      // 面板打开（含正在关闭的 200ms）与拖拽余波期间的点击不算「点画面」
      if (menuOpenRef.current || Date.now() < ptSuppressUntil.current) {
        cancelSingle();
        track = null;
        return;
      }
      const count = Math.max(1, Number(e.detail) || 1);
      track = {
        x: e.clientX,
        y: e.clientY,
        moved: false,
        clickCount: count,
        suppressed: false,
      };
      if (count === 2) {
        // 第二下按下就是系统语义的 dblclick：先撤掉未提交的单击，再全屏
        cancelSingle();
        track.doubleDispatched = true;
        const next = !stRef.current.fullscreen;
        setSt((p) => ({ ...p, fullscreen: next }));
        void invoke("mpv_set_ui_fullscreen", { on: next }).catch(() => {});
      } else if (count > 2) {
        cancelSingle();
        track.suppressed = true;
      }
    };

    const onMove = (e: MouseEvent) => {
      if (!track || track.moved) return;
      if (
        Math.abs(e.clientX - track.x) >= DRAG_THRESHOLD ||
        Math.abs(e.clientY - track.y) >= DRAG_THRESHOLD
      ) {
        track.moved = true;
      }
    };

    const onUp = (e: MouseEvent) => {
      if (e.button !== 0) return;
      const t = track;
      track = null;
      if (!t) return;
      if (interactive(e.target)) {
        cancelSingle();
        return;
      }
      if (t.moved) {
        ptSuppressUntil.current = Date.now() + DRAG_SUPPRESS_MS;
        return;
      }
      if (
        t.suppressed ||
        t.doubleDispatched ||
        t.clickCount !== 1 ||
        Date.now() < ptSuppressUntil.current
      ) {
        return;
      }
      cancelSingle();
      armed = { gen: generation, at: Date.now() };
      // 双击间隔的权威值还在路上：先挂着，等拿到系统值再按「已用时间」扣减
      if (dblReady) commitArmed();
    };

    document.addEventListener("mousedown", onDown, true);
    document.addEventListener("mousemove", onMove, true);
    document.addEventListener("mouseup", onUp, true);
    return () => {
      cancelSingle();
      document.removeEventListener("mousedown", onDown, true);
      document.removeEventListener("mousemove", onMove, true);
      document.removeEventListener("mouseup", onUp, true);
    };
  }, []);

  /* ── 状态：事件为主通道，低频快照兜底 ──
   * [lc-1943] 之前 `listen` 因覆盖层缺少 capability 被 ACL 拒绝（promise reject），
   * 事件从未到达，状态只能靠 250ms 轮询 → 每次点击要等 0~250ms 才有反应。
   * 补上 `capabilities/overlay.json` 后事件成为主通道；轮询降到 500ms 只做兜底。 */
  const lastPstateAt = useRef(0);
  useEffect(() => {
    const unP = listen<PState>("mpv-pstate", (e) => {
      lastPstateAt.current = performance.now();
      // [lc-1946] 置顶是乐观状态：5s 内不让迟到的 mpv 旧值把图标打回去
      const p = { ...(e.payload || {}) };
      if (performance.now() - ontopLocalAt.current < 5000) delete p.ontop;
      setSt((prev) => ({ ...prev, ...p }));
    });
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
        if (!alive || !s) return;
        // 事件通道 300ms 内有推送 = 它比这份快照新；用旧采样覆盖会把乐观状态打回去
        if (performance.now() - lastPstateAt.current < 300) return;
        // [lc-1946] 同一条乐观保护：刚点完置顶的 5s 内不采纳快照里的 ontop
        const next = { ...s };
        if (performance.now() - ontopLocalAt.current < 5000) delete next.ontop;
        setSt((prev) => ({ ...prev, ...next }));
      } catch {
        /* 播放器未起时忽略 */
      }
    };
    void pull();
    const id = window.setInterval(() => void pull(), 500);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);

  /* ── [lc-1942] 菜单数据拉取（同一时刻只允许一发，避免慢响应叠队）──
   * [lc-1943] 旧实现在「已有请求在飞」时**直接 return**，把这次刷新吞掉 ——
   * 点了选项恰好撞上一次 1s 轮询，勾选/数值就要再等一整秒。现在记一笔「还欠一次」，
   * 回来立刻补拉。 */
  const pullMenu = useCallback(async () => {
    if (menuPulling.current) {
      menuPullAgain.current = true;
      return;
    }
    menuPulling.current = true;
    try {
      const d = await invoke<MenuData>("player_menu_data");
      if (d) setMdata(d);
    } catch {
      /* 播放器未起时保持空态 */
    } finally {
      menuPulling.current = false;
      if (menuPullAgain.current) {
        menuPullAgain.current = false;
        window.setTimeout(() => void pullMenu(), 80);
      }
    }
  }, []);

  /** 延迟合并一次拉取（连点/连拖滑块时只发最后一发） */
  const schedulePull = useCallback(
    (ms = 160) => {
      if (pullTimer.current) window.clearTimeout(pullTimer.current);
      pullTimer.current = window.setTimeout(() => {
        pullTimer.current = null;
        void pullMenu();
      }, ms);
    },
    [pullMenu],
  );

  useEffect(() => {
    if (!menu) return;
    void pullMenu();
    // 面板打开期间 1s 轮询：实时反映「当前轨 / 播放进度 / 弹幕开关」的变化
    const id = window.setInterval(() => void pullMenu(), 1000);
    return () => {
      window.clearInterval(id);
    };
  }, [menu, pullMenu]);

  /* ── 面板定位 [lc-1945]：用户规格的 `bottom: calc(100% + 80px)` ──
   *   · 水平：跟随**触发按钮**居中（面板中心 = 按钮中心），贴边时把中心点夹回来，
   *     三角指针再按实际偏移指回按钮中心 —— 绝不是「固定居中整个窗口」；
   *   · 垂直：面板底边 = 按钮顶边 - 80px（80px 物理空白留给进度条那一带）；
   *   · 上方放不下就翻到按钮下方（`bottom: -80px` 的镜像），指针翻向朝上；
   *   · 高度上限 = 可用空间，绝不超过「窗口高 - 80px 留白 - 12px 安全边距」。 */
  const placeMenu = useCallback(() => {
    const panel = menuPanelRef.current;
    const el = menuTriggerRef.current;
    const a = el?.getBoundingClientRect() ?? menuAnchorRef.current;
    if (!a) return;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const card = (panel?.firstElementChild as HTMLElement | null) ?? panel;
    const w = Math.min(MENU_W, vw - MENU_MARGIN * 2);
    const half = w / 2;
    // 自然高度（卡片刻意 overflow:hidden，用 scrollHeight 拿未被裁掉的内容高）
    const rawH = Math.max(card?.offsetHeight || 0, card?.scrollHeight || 0) || 320;
    const aboveMax = a.top - MENU_GAP - MENU_MARGIN;
    const belowMax = vh - a.bottom - MENU_GAP - MENU_MARGIN;
    const side: "above" | "below" =
      rawH <= aboveMax
        ? "above"
        : rawH <= belowMax
          ? "below"
          : aboveMax >= belowMax
            ? "above"
            : "below";
    const maxHeight = Math.max(
      96,
      Math.min(side === "above" ? aboveMax : belowMax, vh - MENU_GAP - MENU_MARGIN),
    );
    const center = a.left + a.width / 2;
    const left = Math.round(Math.min(Math.max(center, half + MENU_MARGIN), vw - half - MENU_MARGIN));
    // 指针在面板内的位置：面板被夹回后仍指向按钮中心
    const arrowLocal = center - (left - half);
    const arrowX = Math.round(
      Math.max(MENU_ARROW_KEEPOUT, Math.min(arrowLocal, w - MENU_ARROW_KEEPOUT)),
    );
    setMenuPos({
      left,
      side,
      maxHeight: Math.round(maxHeight),
      arrowX,
      // 隐形连接桥：覆盖「按钮顶边 - 80px」到「按钮顶边 + 30px」这一段
      bridgeLeft: Math.round(center - BRIDGE_W / 2),
      bridgeTop: Math.round(side === "above" ? a.top - MENU_GAP : a.bottom - (BRIDGE_H - MENU_GAP)),
      ...(side === "above"
        ? { bottom: Math.round(vh - (a.top - MENU_GAP)) }
        : { top: Math.round(a.bottom + MENU_GAP) }),
    });
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

  /* [lc-1945] 摆好位置后再加 `.is-active`：元素必须以隐藏态先渲染一帧，
   * opacity/transform/visibility 的过渡才会真的跑起来。 */
  useLayoutEffect(() => {
    if (!menu || !menuPos || menuClosing) {
      setMenuActive(false);
      return;
    }
    const raf = requestAnimationFrame(() => setMenuActive(true));
    return () => cancelAnimationFrame(raf);
  }, [menu, menuPos, menuClosing]);

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

  /* ── 悬停计时器 ── */
  const clearHoverOpen = useCallback(() => {
    if (hoverOpenTimer.current) {
      window.clearTimeout(hoverOpenTimer.current);
      hoverOpenTimer.current = null;
    }
  }, []);
  const clearHoverClose = useCallback(() => {
    if (hoverCloseTimer.current) {
      window.clearTimeout(hoverCloseTimer.current);
      hoverCloseTimer.current = null;
    }
  }, []);

  /* ── 打开 / 关闭（同一按钮再点一次、空白处、Esc 都关闭）──
   * [lc-1945] `clickGuard`：只有「点空白关面板」这条路径需要抑制窗口 ——
   * pointerdown（外面这一击）先于 mousedown（空白区仲裁）派发，不抑制的话
   * 同一次点击会被仲裁当成「点画面」再按一次暂停。悬停/Esc/行内动作都不涉及
   * 画面区点击，别把用户的正常单击一并吞掉。 */
  const closeMenu = useCallback(
    (clickGuard = false) => {
      if (!menuOpenRef.current) return;
      menuOpenRef.current = false;
      // [lc-1945] 悬停态一并复位：关掉的面板不能带着「固定」或残留计时器回来
      menuPinned.current = false;
      hoverTarget.current = null;
      clearHoverOpen();
      clearHoverClose();
      if (clickGuard) ptSuppressUntil.current = Date.now() + 400;
      releaseKeyboard();
      setMenuClosing(true);
      setMenuActive(false);
      if (menuCloseTimer.current) window.clearTimeout(menuCloseTimer.current);
      // 减少动态偏好：没有退场动画就直接卸载，别让面板多停 300ms
      const delay = window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 300;
      menuCloseTimer.current = window.setTimeout(() => {
        setMenu(null);
        setMenuClosing(false);
        setSub(null);
        setMenuPos(null);
      }, delay);
    },
    [releaseKeyboard, clearHoverOpen, clearHoverClose],
  );

  /* 离开搜索子面板（返回 / 切菜单 / 关闭）时立刻归还键盘 */
  useEffect(() => {
    if (sub !== "subsearch") return;
    return () => releaseKeyboard();
  }, [sub, releaseKeyboard]);

  /* [lc-1945] `pinned`：点击/触摸/键盘打开 = 固定态（移开不关）；悬停打开 = 临时态 */
  const openMenu = (id: MenuId, el: HTMLElement | null, pinned = true) => {
    clearHoverOpen();
    clearHoverClose();
    if (menu === id && menuOpenRef.current) {
      if (!pinned) return;
      // 悬停打开的面板被点一下 → 固定；已固定的再点 → 关闭（原有 toggle 语义）
      if (menuPinned.current) {
        closeMenu();
        return;
      }
      menuPinned.current = true;
      setMenuActive(true);
      return;
    }
    if (menuCloseTimer.current) window.clearTimeout(menuCloseTimer.current);
    menuPinned.current = pinned;
    hoverTarget.current = id;
    menuTriggerRef.current = el;
    menuAnchorRef.current = el ? el.getBoundingClientRect() : null;
    menuOpenRef.current = true;
    setMenuClosing(false);
    setMenuActive(false);
    setSub(null);
    setSubResults([]);
    setSubErr("");
    setSubNotice("");
    setMenuPos(null);
    setMenu(id);
    // [lc-1943] 面板打开 = 模态：透传由 layoutRef → 可点区上报声明整窗可点，
    // 不再需要单独 invoke 一次穿透开关。
    void pullMenu();
  };

  /* ── [lc-1945] 悬停交互 ──
   * 打开：鼠标进入触发按钮后驻留 `HOVER_OPEN_MS`（100ms）才弹面板 —— 规格只规定了
   * 「移开 300ms 关闭」，驻留延迟是为了让「沿底栏划过」不至于连开 8 个面板、
   * 也不会连发 8 轮 `player_menu_data`；移开按钮 / 面板 / 连接桥任一 → 300ms 后关闭。
   * 关闭：固定态（点击打开）不参与，只有悬停态面板会自己收起来。 */
  const scheduleHoverClose = () => {
    clearHoverClose();
    if (menuPinned.current || !menuOpenRef.current) return;
    hoverCloseTimer.current = window.setTimeout(() => {
      hoverCloseTimer.current = null;
      if (menuPinned.current) return;
      closeMenu();
    }, HOVER_CLOSE_MS);
  };

  const hoverEnterMenu = (id: MenuId) => (e: React.MouseEvent<HTMLElement>) => {
    clearHoverClose();
    if (menuOpenRef.current && menu === id) return;
    clearHoverOpen();
    hoverTarget.current = id;
    const el = e.currentTarget;
    hoverOpenTimer.current = window.setTimeout(() => {
      hoverOpenTimer.current = null;
      if (hoverTarget.current !== id || !el.isConnected) return;
      openMenu(id, el, false);
    }, HOVER_OPEN_MS);
  };

  const hoverLeaveMenu = (id: MenuId) => () => {
    if (hoverTarget.current === id) hoverTarget.current = null;
    clearHoverOpen();
    scheduleHoverClose();
  };

  /** 进入面板 / 隐形连接桥：撤销关闭计时；若面板已在退场则原地救回 */
  const keepMenuOpen = () => {
    clearHoverClose();
    if (menuOpenRef.current || !menu) return;
    if (menuCloseTimer.current) {
      window.clearTimeout(menuCloseTimer.current);
      menuCloseTimer.current = null;
    }
    menuOpenRef.current = true;
    setMenuClosing(false);
    setMenuActive(true);
  };

  useEffect(() => {
    return () => {
      if (menuCloseTimer.current) window.clearTimeout(menuCloseTimer.current);
      if (keyboardFocusTimer.current) window.clearTimeout(keyboardFocusTimer.current);
      if (hoverOpenTimer.current) window.clearTimeout(hoverOpenTimer.current);
      if (hoverCloseTimer.current) window.clearTimeout(hoverCloseTimer.current);
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
      // 这一次 pointerdown 会紧接着触发 mousedown 仲裁：给它上抑制窗口
      closeMenu(true);
    };
    window.addEventListener("keydown", onKey, true);
    document.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      document.removeEventListener("pointerdown", onDown, true);
    };
  }, [menu, closeMenu, releaseKeyboard]);

  /* ── 菜单动作 ──
   * [lc-1943] 乐观 UI：先在本地把字段写成目标值（勾选/数值当帧就动），
   * mpv 回报只做校正。面板关闭也提前到 IPC 之前 —— 用户看到的「点完立刻合上」
   * 不再依赖 mpv 往返。 */
  const applyProp = useCallback(
    async (name: string, value: unknown, closeAfter = false) => {
      const field = PROP_FIELD[name];
      if (field) setMdata((prev) => ({ ...prev, [field]: value as never }));
      if (closeAfter) closeMenu();
      latMark(`prop:${name}`);
      try {
        await invoke("mpv_set_prop", { name, value });
      } catch {
        /* 写入失败：稍后的一次拉取会把真实状态带回来 */
      } finally {
        latDone();
      }
      schedulePull(160);
    },
    [closeMenu, schedulePull],
  );

  const playIndex = useCallback(
    async (index: number) => {
      setMdata((prev) => ({ ...prev, playlistPos: index }));
      closeMenu();
      latMark("playlist:play-index");
      try {
        await invoke("mpv_playlist_play_index", { index });
      } catch {
        /* ignore */
      } finally {
        latDone();
      }
      schedulePull(200);
    },
    [closeMenu, schedulePull],
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
      // [lc-1943] 乐观写：偏移数值当帧就变，脚本回报后由拉取校正
      setMdata((prev) => ({ ...prev, danmakuDelay: next }));
      cmd("mpv_script_message", { name: "uosc_danmaku", args: ["danmaku-delay", String(next)] });
      schedulePull(180);
    },
    [cmd, mdata.danmakuDelay, schedulePull],
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
        toggle
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
                <Search size={18} strokeWidth={2.2} />
              </button>
            </div>
            <MenuRow
              text="字幕"
              toggle
              checked={subOn}
              onClick={() => {
                if (subOn) void applyProp("sid", "no");
                else void applyProp("sid", subTracks[0]?.id ?? "auto");
              }}
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
              text="静音"
              toggle
              checked={mdata.aid === "no"}
              onClick={() => void applyProp("aid", mdata.aid === "no" ? "auto" : "no")}
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
                text="副字幕"
                toggle
                checked={mdata.secondarySid !== "no" && mdata.secondarySid != null}
                onClick={() => {
                  if (mdata.secondarySid !== "no" && mdata.secondarySid != null) {
                    void applyProp("secondary-sid", "no");
                  } else {
                    void applyProp("secondary-sid", subTracks[0]?.id ?? "auto");
                    void applyProp("secondary-sub-visibility", true);
                  }
                }}
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
    <div
      className={`player-chrome${
        visible || loading || menu ? "" : " pc-hidden"
      }${closing ? " pc-leaving" : ""}`}
    >
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
              {...tap(() => {
                const next = !st.ontop;
                // [lc-1946] 乐观 UI + 真实窗口置顶（mpv 锚窗 + 覆盖层，零 mpv IPC 等待）
                ontopLocalAt.current = performance.now();
                setSt((p) => ({ ...p, ontop: next }));
                void invoke<boolean>("player_window_ontop", { on: next })
                  .then((v) => {
                    if (typeof v === "boolean") setSt((p) => ({ ...p, ontop: v }));
                  })
                  .catch(() => {});
              })}
            >
              <Pin size={14} />
            </button>
            <button
              type="button"
              className="pc-round"
              data-tip="最小化"
              {...tap(() => {
                // [lc-1946] 真最小化：收起覆盖层 + mpv 锚窗，最小化主窗（任务栏可恢复）
                void invoke("player_window_minimize").catch(() => {});
              })}
            >
              <Minus size={14} />
            </button>
            <button
              type="button"
              className={`pc-round${st.maximized ? " pc-active" : ""}`}
              data-tip={st.maximized ? "还原" : "最大化"}
              {...tap(() => {
                // [lc-1946] 对 mpv 锚窗做 SW_MAXIMIZE / SW_RESTORE，覆盖层由跟随循环同步
                void invoke<boolean>("player_window_maximize")
                  .then((v) => {
                    if (typeof v === "boolean") setSt((p) => ({ ...p, maximized: v }));
                  })
                  .catch(() => {});
              })}
            >
              {st.maximized ? <Copy size={12} /> : <Square size={12} />}
            </button>
            <button
              type="button"
              className="pc-round pc-close"
              data-tip="关闭"
              {...tap(() => {
                // [lc-1946] 先立刻淡出控件层（视觉第一步），Rust 侧也把「收窗」提到
                // 停止/还原之前；清理转后台。连点在前后端各挡一层。
                if (closingRef.current) return;
                closingRef.current = true;
                setClosing(true);
                // 2.5s 兜底复位：万一这次退出没走到（例如 mpv 会话异常），
                // 控件层不能永久停在「已淡出」状态。
                window.setTimeout(() => {
                  closingRef.current = false;
                  setClosing(false);
                }, 2500);
                void invoke("player_embed_leave", { stop: true }).catch(() => {});
              })}
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
                aria-label="选集"
                className={`pc-btn pc-icon${menu === "episodes" ? " pc-open" : ""}`}
                {...tap((el) => openMenu("episodes", el))}
                onMouseEnter={hoverEnterMenu("episodes")}
                onMouseLeave={hoverLeaveMenu("episodes")}
              >
                <ListVideo size={17} />
              </button>
              <button
                type="button"
                aria-label="字幕"
                className={`pc-btn pc-icon${menu === "subtitles" ? " pc-open" : ""}`}
                {...tap((el) => openMenu("subtitles", el))}
                onMouseEnter={hoverEnterMenu("subtitles")}
                onMouseLeave={hoverLeaveMenu("subtitles")}
              >
                <Subtitles size={17} />
              </button>
              <button
                type="button"
                aria-label="音频"
                className={`pc-btn pc-icon${menu === "audio" ? " pc-open" : ""}`}
                {...tap((el) => openMenu("audio", el))}
                onMouseEnter={hoverEnterMenu("audio")}
                onMouseLeave={hoverLeaveMenu("audio")}
              >
                <AudioLines size={17} />
              </button>
            </div>
          </div>

          <div className="pc-center">
            <div className="pc-dock pc-dock-bare" data-chrome-hit>
              <button
                type="button"
                aria-label="播放速度"
                className={`pc-btn pc-text${menu === "speed" ? " pc-open" : ""}`}
                {...tap((el) => openMenu("speed", el))}
                onMouseEnter={hoverEnterMenu("speed")}
                onMouseLeave={hoverLeaveMenu("speed")}
              >
                {Number(speed.toFixed(2))}×
              </button>
              <button
                type="button"
                className="pc-btn pc-play"
                data-tip={st.paused ? "播放" : "暂停"}
                {...tap(() => {
                  // 乐观：图标当帧翻转，mpv 回报只做校正
                  setSt((p) => ({ ...p, paused: !p.paused }));
                  cmdLat("pause", "mpv_toggle_pause");
                })}
              >
                {st.paused ? <Play size={26} /> : <Pause size={26} />}
              </button>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip="下一集"
                {...tap(() => cmdLat("next", "mpv_next_episode"))}
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
                  {...tap(() => {
                    const next = vol > 0 ? 0 : 100;
                    setSt((p) => ({ ...p, volume: next }));
                    cmdLat("mute", "mpv_set_volume", { volume: next });
                  })}
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
                aria-label="弹幕"
                className={`pc-btn pc-icon${menu === "danmaku" ? " pc-open" : ""}`}
                {...tap((el) => openMenu("danmaku", el))}
                onMouseEnter={hoverEnterMenu("danmaku")}
                onMouseLeave={hoverLeaveMenu("danmaku")}
              >
                <MessageSquare size={17} />
              </button>
              <button
                type="button"
                aria-label="片头片尾"
                className={`pc-btn pc-icon${menu === "chapters" ? " pc-open" : ""}`}
                {...tap((el) => openMenu("chapters", el))}
                onMouseEnter={hoverEnterMenu("chapters")}
                onMouseLeave={hoverLeaveMenu("chapters")}
              >
                <ListOrdered size={17} />
              </button>
              <button
                type="button"
                aria-label="版本"
                className={`pc-btn pc-icon${menu === "video" ? " pc-open" : ""}`}
                {...tap((el) => openMenu("video", el))}
                onMouseEnter={hoverEnterMenu("video")}
                onMouseLeave={hoverLeaveMenu("video")}
              >
                <Layers size={17} />
              </button>
              <button
                type="button"
                aria-label="设置"
                className={`pc-btn pc-icon${menu === "settings" ? " pc-open" : ""}`}
                {...tap((el) => openMenu("settings", el))}
                onMouseEnter={hoverEnterMenu("settings")}
                onMouseLeave={hoverLeaveMenu("settings")}
              >
                <Settings size={17} />
              </button>
              <button
                type="button"
                className="pc-btn pc-icon"
                data-tip={st.fullscreen ? "退出全屏" : "全屏"}
                {...tap(() => {
                  const next = !st.fullscreen;
                  setSt((p) => ({ ...p, fullscreen: next }));
                  cmdLat("fullscreen", "mpv_set_ui_fullscreen", { on: next });
                })}
              >
                <Maximize2 size={17} />
              </button>
            </div>
          </div>
        </div>
      </div>

      {/* ── [lc-1945] 隐形连接桥：覆盖「触发按钮 → 面板」那 80px 空白 ──
       * 悬停态下鼠标要能一路从按钮穿过进度条那一带进到面板，中间不能掉 hover；
       * 它带 `data-chrome-hover`（进可点区上报，穿透态下 WebView 才有 mousemove）
       * 但**不是** `data-chrome-hit`，所以点击白名单与空白区仲裁不受影响。 */}
      {menu && menuPos ? (
        <div
          className="pc-menu-bridge"
          data-chrome-hover
          aria-hidden="true"
          style={{
            left: menuPos.bridgeLeft,
            top: menuPos.bridgeTop,
            width: BRIDGE_W,
            height: BRIDGE_H,
          }}
          onMouseEnter={keepMenuOpen}
          onMouseLeave={scheduleHoverClose}
        />
      ) : null}

      {/* ── [lc-1942] 通用磨砂菜单：一个组件、多份数据 ──
       * [lc-1945] 位置来自 `placeMenu`（跟随触发按钮居中 + 80px 留白），
       * 显隐来自 `.is-active` 的 CSS 过渡（规格的 opacity/transform/visibility）。 */}
      {menu ? (
        <div
          ref={menuPanelRef}
          data-chrome-hit
          className={`pc-menu pc-menu-${menu}${menuPos?.side === "below" ? " is-below" : ""}${
            menuActive ? " is-active" : ""
          }`}
          style={
            menuPos
              ? {
                  left: menuPos.left,
                  ...(menuPos.side === "above"
                    ? { bottom: menuPos.bottom }
                    : { top: menuPos.top }),
                  maxHeight: menuPos.maxHeight,
                  ["--pc-arrow-x" as string]: `${menuPos.arrowX}px`,
                }
              : { left: 0, bottom: 0, visibility: "hidden" }
          }
          onMouseEnter={keepMenuOpen}
          onMouseLeave={scheduleHoverClose}
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
