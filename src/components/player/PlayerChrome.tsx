/* [lc-1939] 播放器覆盖层 UI —— 视觉模型照抄 Polaris 的 mpv-overlay。
 *
 * [lc-1974] 全屏废除穿透机制（用户第 N 次实证后的模式级修法）：
 *   · 全屏期 Rust 把穿透位锁死为「恒可点」（FULLSCREEN_INPUT_LOCK，任何路径不得置
 *     WS_EX_TRANSPARENT），覆盖层整窗收输入 —— 控件点击零等待，不再依赖 hover 链
 *     在全屏下的存活（mpv ontop_mode.lua 拉锯 z 序 / elementFromPoint 抖动均免疫）。
 *   · 视频区点击：本文件的 `mpv_forward_mouse` 转发链（lc-1949 原样，穿透态下零
 *     流量）在全屏态自然接管 —— 落点不在 `[data-chrome-hit]` 上的事件 PostMessage
 *     给 mpv，单击暂停/双击全屏/滚轮全是 mpv 原生行为；全屏不需要拖窗，「慢半拍」
 *     可接受，正确性优先。
 *   · hover 事务在全屏态短路「拨回穿透」请求（发了也会被锁拒绝）；退出全屏边沿
 *     补一发 hover=false，把窗口模式的穿透默认态立即对齐。
 *   · 窗口模式不动：Polaris 穿透 + hover 事务（下述 [lc-1960]）照旧。
 *
 * [lc-1960] 交互架构按 Polaris 法证结论二次重做（穿透默认态 + hover 事务）：
 *
 *   · **穿透默认态**：控件隐藏（绝大多数时间）覆盖层 WS_EX_TRANSPARENT 恒置位
 *     （Rust 起播接线 + hover 事务单点管理），真实鼠标直达 mpv —— 单击暂停/
 *     双击全屏/拖动/滚轮全是 mpv 原生 input.conf 行为，WebView 零鼠标事件流。
 *     lc-1949 的「常驻可点 + 全量转发」就是「反应慢半拍」与「全屏点击漏给 mpv
 *     变暂停」的结构根源，已废弃。
 *   · **hover 事务**：光标悬进 `[data-chrome-hit]` 白名单 → invoke('mpv_overlay_hover',
 *     {hover, generation, requestSeq}) → Rust 拨位并读回真实 ex-style 作 ack；
 *     决策 = elementFromPoint（DOM 唯一真源），坐标 = Rust 50ms 光标探针
 *     （overlay-force-hover-check / overlay-mouse-activity），ack 失配走
 *     40/120/300/700ms 退避重试（Polaris 同款）。菜单打开期间 hover 恒可交互。
 *   · **空白区转发保留**：hover 翻位后的可点窗口里，落点不在控件上的事件仍经
 *     `mpv_forward_mouse` PostMessage 转给 mpv（拖动捕获续投也走这里）；
 *     穿透态下这些事件根本不进 WebView，转发通道稳态零流量。
 *   · 3 秒无操作自动隐藏（Polaris hideTimeout=3.0）；隐藏时 hover 事务强制穿透。
 *   · 状态：`mpv-pstate` 事件为主通道 + 500ms invoke 快照兜底。
 *
 * 视觉：Polaris 的值 —— 底栏胶囊 rgba(18,18,18,.88) / 圆角 20 / 内距 4 / 按钮 34×34
 * 圆角 17、播放 40×40 圆角 20、图标 17（播放 26）/ 进度轨 7px（悬停 10）/ 时间 12px 700 /
 * 顶栏圆钮 26。全层只有底部 132px 对比渐变，**顶部没有任何遮罩或渐变**。
 *
 * [lc-1942] 菜单：覆盖层内的磨砂玻璃面板（一个组件 + 多份数据）：
 *   · 面板数据全部来自 `player_menu_data`（真实轨道/章节/播放列表/字幕样式/弹幕状态）；
 *   · 设置项全部走 `mpv_set_prop` / `mpv_script_message` / `playlist-play-index`，无空桩；
 *   · 面板打开期间点空白/Esc/再点同一按钮关闭。
 *
 * [lc-1945] 控件弹窗规格：
 *   · 白 45% + blur(28px) / 圆角 24 / 白描边 + 内高光 / 底部 8px 三角指针；
 *   · 面板底边 = 触发按钮顶边 - 80px，跟随按钮居中；
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
  Layers,
  ListOrdered,
  ListVideo,
  Maximize2,
  MessageSquare,
  Minimize2,
  Minus,
  Pause,
  Pin,
  Play,
  Plus,
  Search,
  Settings,
  SkipForward,
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

/* [lc-1988] NVIDIA 覆盖层拦截提示只弹一次：Rust fs-snap 命中根窗进程含 nvidia 时
 * 广播 `nvidia-overlay-block`，这里显示一次性提示（覆盖层页面常驻，跨播放会话不复弹）。 */
let nvHintShown = false;
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
  /* [lc-1960] hover 事务桥：状态机 effect 把 request 函数挂上来，供自动隐藏 /
   * 转发路径调用（声明在 effect 之前，避免闭包引用顺序问题）。 */
  const hoverRequestRef = useRef<((hover: boolean, source: string, force?: boolean) => void) | null>(null);
  const wakeRef = useRef<() => void>(() => {});
  /* [lc-1949] 视频区按下转发状态（0=左 1=中 2=右）；hover 状态机也读它做拖动锁 */
  const fwdRef = useRef<{ btn: number } | null>(null);
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
      // [lc-1960] 控件隐藏 = 穿透归位（Polaris controls-hidden 分支）：隐藏态下
      // 覆盖层必须交还鼠标，否则「看不见但可点」的幽灵控件再次出现。
      hoverRequestRef.current?.(false, "controls-hidden");
    }, 3000);
  }, []);
  wakeRef.current = wake;

  useEffect(() => {
    wake();
    return () => {
      if (hideTimer.current) window.clearTimeout(hideTimer.current);
    };
  }, [wake]);

  /* [lc-1991] 新会话（phase=start）同时唤醒控件层：覆盖层窗口跨会话常驻，
   * 上一轮若是 mpv 侧退出（例如点到 mpv 自己 OSC 的窗控 → mpv quit），
   * 控件层的 3s 自动隐藏可能停在 `pc-hidden`；下一次起播必须从可见态开始。 */
  useEffect(() => {
    if (st.phase === "start") wake();
  }, [st.phase, wake]);

  /* ── [lc-1988] NVIDIA 覆盖层拦截的一次性提示 ──
   * Rust fs-snap 判定全屏点击被 NVIDIA overlay 截走（根窗进程含 nvidia）时广播本事件。
   * 提示 8s 自散；每次覆盖层页面生命周期只弹一次，不再打扰。 */
  const [nvHint, setNvHint] = useState(false);
  useEffect(() => {
    const un = listen("nvidia-overlay-block", () => {
      if (nvHintShown) return;
      nvHintShown = true;
      setNvHint(true);
      window.setTimeout(() => setNvHint(false), 8000);
    });
    return () => {
      void un.then((f) => f()).catch(() => {});
    };
  }, []);

  /* ── [lc-1960] hover 事务状态机（Polaris mpv-overlay-hover 同款）──
   * 穿透默认态：控件隐藏期间覆盖层 WS_EX_TRANSPARENT 恒置位（Rust 起播接线已置），
   * 真实鼠标直达 mpv（单击暂停/双击全屏/拖动全原生），WebView 收不到任何鼠标事件。
   * 光标悬进 `[data-chrome-hit]` 白名单时，本状态机经 `mpv_overlay_hover` 事务
   * （generation/seq + Rust 原生样式读回 ack + 40/120/300/700ms 退避重试）把穿透位
   * 摘掉；离开白名单 / 控件层隐藏时拨回。
   *
   * 决策权 = elementFromPoint（DOM 是热区唯一真源，杜绝旧 lc-1939 热区矩形上报漂移）；
   * 坐标事实 = Rust 50ms 光标探针（穿透态下 WebView 无事件可依，Polaris [overlay_probe]
   * 同款）：`overlay-force-hover-check`（inside=false = 真离开播放器的权威信号）+
   * `overlay-mouse-activity` 心跳（控件续显）。
   *
   * 与 Polaris 的两处刻意差异：
   *   · 菜单打开期间 hover 恒可交互 —— 「点空白关面板」那一击必须落在 WebView，
   *     落给 mpv 就会顺带暂停视频；
   *   · 按下转发中（fwdRef）/拖动锁（dragLock）不摘 hover —— 拖到一半穿透会切断
   *     pointer capture（Polaris _overlayAnyDragActive 同义）。 */
  useEffect(() => {
    let hoverGen = 0;
    let hoverSeq = Date.now();
    let hoverDesired = false;
    let inFlightSeq = 0;
    let retryTimer: number | null = null;
    let retryAttempt = 0;
    const disposers: Array<() => void> = [];

    const clearRetry = () => {
      if (retryTimer !== null) {
        window.clearTimeout(retryTimer);
        retryTimer = null;
      }
    };

    type HoverAckPayload = {
      success?: boolean;
      generation?: number;
      requestSeq?: number;
      appliedHover?: boolean;
      latestRequestSeq?: number;
      desiredHover?: boolean;
      state?: HoverAckPayload;
    };

    const adopt = (payload: unknown, replaceDesired: boolean, replaceApplied: boolean) => {
      const raw = payload as HoverAckPayload | null;
      const data = raw && typeof raw === "object" && raw.state ? raw.state : raw;
      if (!data || typeof data !== "object") return;
      const generation = Number(data.generation || 0);
      if (Number.isFinite(generation) && generation > 0 && generation < hoverGen) return;
      if (Number.isFinite(generation) && generation > hoverGen) {
        hoverGen = generation;
        retryAttempt = 0;
        clearRetry();
      }
      const latest = Number(data.latestRequestSeq || 0);
      if (Number.isSafeInteger(latest) && latest > hoverSeq) hoverSeq = latest;
      if (replaceDesired && typeof data.desiredHover === "boolean") hoverDesired = data.desiredHover;
      if (replaceApplied && typeof data.appliedHover === "boolean") {
        hoverInteractive.current = data.appliedHover;
      }
    };

    const request = (hover: boolean, source: string, force = false) => {
      const desired = !!hover;
      const changed = desired !== hoverDesired;
      hoverDesired = desired;
      if (changed) {
        retryAttempt = 0;
        clearRetry();
      }
      // [lc-1974] 全屏态穿透位已被 Rust 输入锁死为「恒可点」：「拨回穿透」的请求
      // 短路不发 —— 发了也会被锁拒绝，只会制造事务噪音（Rust ack 会按请求值结算，
      // 这里提前拦掉，账面同样成立）。hover=true（悬进控件）照常发送，维持
      // FRONTEND_HOVER_LAST 语义，退出全屏时 Rust 据此决定是否恢复穿透默认态。
      if (!hover && stRef.current.fullscreen) {
        hoverInteractive.current = false;
        return;
      }
      // 纯边沿：目标态未变且已账实相符时不发事务（稳态零管道流量，Polaris 同款）
      if (!force && !changed && hoverInteractive.current === desired && retryTimer === null) return;
      const seq = ++hoverSeq;
      inFlightSeq = seq;
      invoke<HoverAckPayload | null>("mpv_overlay_hover", {
        hover: desired,
        generation: hoverGen,
        requestSeq: seq,
      })
        .then((ack) => {
          const stillLatest = seq === inFlightSeq && desired === hoverDesired;
          const genBefore = hoverGen;
          adopt(ack, false, stillLatest);
          if (!stillLatest) {
            if (hoverGen > genBefore) scheduleRetry(source);
            return;
          }
          if (
            ack &&
            ack.success === true &&
            Number(ack.generation || 0) === hoverGen &&
            ack.appliedHover === desired
          ) {
            retryAttempt = 0;
            clearRetry();
            return;
          }
          // ack 失配：保留真实 DOM 意图，按服务端代际退避重试
          scheduleRetry(source);
        })
        .catch(() => {
          if (seq === inFlightSeq && desired === hoverDesired) scheduleRetry(source);
        });
    };
    hoverRequestRef.current = request;

    const scheduleRetry = (source: string) => {
      const delays = [40, 120, 300, 700];
      if (retryTimer !== null || retryAttempt >= delays.length) return;
      const delay = delays[retryAttempt++];
      retryTimer = window.setTimeout(() => {
        retryTimer = null;
        request(hoverDesired, `${source}-retry`, true);
      }, delay);
    };

    const applyHover = (el: Element | null, source: string) => {
      const dragActive = !!(fwdRef.current || dragLock.current);
      const interactive = dragActive
        ? hoverDesired
        : menuOpenRef.current ||
          !!(el && (!el.isConnected || (el.closest && el.closest("[data-chrome-hit]"))));
      hoverInteractive.current = interactive;
      request(!!interactive, source);
    };

    // 探针物理坐标（窗口内）→ CSS 视口坐标（Polaris _overlayProbePoint 同款映射；
    // 不能假定 DPI/96 = WebView CSS 栅格，显式按 surface 与 clientWidth 比例换算）
    const probePoint = (d: {
      physicalX?: number;
      physicalY?: number;
      surfacePhysicalWidth?: number;
      surfacePhysicalHeight?: number;
    }) => {
      const px = Number(d.physicalX);
      const py = Number(d.physicalY);
      const sw = Number(d.surfacePhysicalWidth);
      const sh = Number(d.surfacePhysicalHeight);
      const cw = document.documentElement.clientWidth || window.innerWidth;
      const ch = document.documentElement.clientHeight || window.innerHeight;
      if ([px, py, sw, sh, cw, ch].every((v) => Number.isFinite(v) && v > 0)) {
        return { x: (px * cw) / sw, y: (py * ch) / sh };
      }
      return null;
    };

    let disposed = false;
    void (async () => {
      const un = await Promise.all([
        listen("overlay-interaction-state", (e) => adopt(e.payload, true, true)),
        listen<{
          physicalX: number;
          physicalY: number;
          surfacePhysicalWidth: number;
          surfacePhysicalHeight: number;
          inside: boolean;
        }>("overlay-force-hover-check", (e) => {
          const d = e.payload || ({} as typeof e.payload);
          if (d.inside === false) {
            // 真离开播放器（全局光标 vs 覆盖层矩形，≤50ms 必达）→ 穿透 + 隐控件
            request(false, "probe-outside");
            if (!menuOpenRef.current && !closingRef.current) {
              if (hideTimer.current) {
                window.clearTimeout(hideTimer.current);
                hideTimer.current = null;
              }
              setVisible(false);
            }
            return;
          }
          wakeRef.current();
          const pt = probePoint(d);
          if (!pt || pt.x < 0 || pt.y < 0) {
            request(false, "probe-invalid");
            return;
          }
          applyHover(document.elementFromPoint(pt.x, pt.y), "probe");
        }),
        listen("overlay-mouse-activity", () => wakeRef.current()),
      ]);
      if (disposed) {
        for (const f of un) void f();
        return;
      }
      disposers.push(...un);
      // 页面（重）加载后先与 Rust 对一次账：代际/真实穿透位以服务端为准
      try {
        adopt(await invoke("mpv_overlay_hover_state"), true, true);
      } catch {
        /* 命令未就绪时忽略，探针事件随后会推 */
      }
    })();

    // 可交互窗口内的 mousemove 精化（穿透态下收不到该事件，零开销；Polaris checkHoverState 同位）
    const onMouseMove = (e: MouseEvent) => {
      if (fwdRef.current || dragLock.current) return;
      const el = e.target instanceof Element ? e.target : document.elementFromPoint(e.clientX, e.clientY);
      applyHover(el, "mousemove");
    };
    document.addEventListener("mousemove", onMouseMove, true);
    disposers.push(() => document.removeEventListener("mousemove", onMouseMove, true));

    return () => {
      disposed = true;
      hoverRequestRef.current?.(false, "unmount");
      hoverRequestRef.current = null;
      clearRetry();
      for (const d of disposers) {
        try {
          d();
        } catch {
          /* ignore */
        }
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /* ── [lc-1974] 全屏输入模式切换的对账 ──
   * 退出全屏：Rust 已解锁（FULLSCREEN_INPUT_LOCK OFF），但全屏期间「拨回穿透」的
   * hover 请求全被前端短路了 —— 这里在退出边沿补一发 hover=false，把窗口模式的
   * 穿透默认态立即对齐（不等下一个鼠标事件；光标静止跨边界也不会留「可点幽灵层」）。
   * 进入全屏：无需动作，Rust 在 on_player_fullscreen_changed 里锁位 + 摘穿透。 */
  useEffect(() => {
    if (!st.fullscreen) {
      hoverRequestRef.current?.(false, "fullscreen-exit");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [st.fullscreen]);

  /* ── [lc-1964] 控件带几何热区上报（Rust 探针直翻可点位的保险丝）──
   * 1s 心跳把 `[data-chrome-hit]` 矩形（CSS 视口坐标）+ 视口尺寸交给 Rust；
   * 探针每拍直接对「光标落在控件带内」翻可点位，完全不依赖 elementFromPoint ——
   * WebView 判定链全挂（JS 死 / 事件丢 / 映射错）时控件仍可点。
   * chrome 隐藏（容器 opacity:0 + pointer-events:none）→ 有效 pointer-events 过滤
   * （与 elementFromPoint 同一规则）→ 上报空表 → 点击照旧穿透。
   * resize / 全屏切换即时补报；卸载时上报空表 + 零视口（Rust 侧回固定带兜底语义）。 */
  useEffect(() => {
    let disposed = false;
    const report = () => {
      if (disposed) return;
      const vw = document.documentElement.clientWidth || window.innerWidth;
      const vh = document.documentElement.clientHeight || window.innerHeight;
      const zones: Array<[number, number, number, number]> = [];
      document.querySelectorAll<HTMLElement>("[data-chrome-hit]").forEach((el) => {
        try {
          // 有效 pointer-events 判定（与 elementFromPoint 同一规则）：chrome 隐藏 =
          // 容器 opacity:0 + pointer-events:none，opacity 不继承但 pointer-events
          // 生效于子树，沿祖先链查 none 即可精确滤掉隐藏态控件。
          let hidden = false;
          let anc: HTMLElement | null = el;
          while (anc) {
            if (getComputedStyle(anc).pointerEvents === "none") {
              hidden = true;
              break;
            }
            anc = anc.parentElement;
          }
          if (hidden) return;
          const r = el.getBoundingClientRect();
          if (r.width < 2 || r.height < 2 || r.bottom <= 0 || r.top >= vh || r.right <= 0 || r.left >= vw) return;
          const x = Math.max(0, r.left);
          const y = Math.max(0, r.top);
          const w = Math.min(r.right, vw) - x;
          const h = Math.min(r.bottom, vh) - y;
          if (w > 2 && h > 2) zones.push([x, y, w, h]);
        } catch {
          /* 单个节点异常不拖垮整次上报 */
        }
      });
      void invoke("mpv_overlay_hotzones", {
        zones,
        viewportW: vw,
        viewportH: vh,
      }).catch(() => {
        /* 命令未就绪（老后端）静默忽略 —— 探针侧还有固定带兜底 */
      });
    };
    report();
    const iv = window.setInterval(report, 1000);
    window.addEventListener("resize", report);
    return () => {
      disposed = true;
      window.clearInterval(iv);
      window.removeEventListener("resize", report);
      void invoke("mpv_overlay_hotzones", { zones: [], viewportW: 0, viewportH: 0 }).catch(() => {});
    };
  }, []);

  const stRef = useRef(st);
  stRef.current = st;

  /** [lc-1948] 进入全屏前的最大化态：窗口级退出全屏会按它还原，图标跟着对齐。 */
  const maxBeforeFs = useRef(false);
  /** [lc-1948] 全屏切换入口：右上角窗控 / 底栏全屏键共用同一条「显示器矩形」路径
   * （mpv fullscreen 属性 → Rust on_player_fullscreen_changed）。双击画面不经过这里：
   * 转发给 mpv 后由它原生合成 MBTN_LEFT_DBL → 同一个 fullscreen 属性，路径仍然唯一。
   * 不再有任何 UI 走 SW_MAXIMIZE：最大化只铺工作区（任务栏露着），且 mpv 对最大化
   * 窗口的边框补偿会把窗口矩形挪出屏幕（右/下露缝、顶栏不消失），那不是全屏。 */
  const toggleFullscreen = useCallback(
    (on?: boolean) => {
      const next = typeof on === "boolean" ? on : !stRef.current.fullscreen;
      if (next === !!stRef.current.fullscreen) return;
      if (next) maxBeforeFs.current = !!stRef.current.maximized;
      setSt((p) => ({
        ...p,
        fullscreen: next,
        maximized: next ? false : maxBeforeFs.current,
      }));
      cmdLat("fullscreen", "mpv_set_ui_fullscreen", { on: next });
    },
    [cmdLat],
  );

  /* ── [lc-1949] 视频区输入转发（覆盖层常驻可点架构的另一半）──
   * 落点不在控件上的鼠标事件原样转给 mpv（`mpv_forward_mouse` → PostMessage），
   * mpv 的 win32 后端按真实输入处理：
   *   · 单击 = fntv_osc 分发（画面空白处暂停）；
   *   · 双击 = mpv 自己按 --input-doubleclick-time 合成 MBTN_LEFT_DBL（全屏）——
   *     **不代发 WM_LBUTTONDBLCLK**：PostMessage 不会让系统自动合成，而 mpv 的
   *     合成器只认普通按钮序列，代发会造成二次触发；
   *   · 按住拖动 = --input-builtin-dragging（mpv 内部 ReleaseCapture+HTCAPTION
   *     原生拖窗，DWM 搬运、零跟随延迟）；
   *   · 滚轮 = input.conf（音量）；右键 = MBTN_RIGHT（input.conf: ignore）。
   * 分流规则：target 命中 [data-chrome-hit]（或已从 DOM 摘除的节点）→ 覆盖层自己处理；
   * 菜单打开 / 关面板抑制窗口内 → 不转发（点空白 = 关面板）；其余 → 转发。
   * [lc-1960] fwdRef 上移到组件 refs 区（hover 状态机读它做拖动锁）。 */
  useEffect(() => {
    const dpr = () => window.devicePixelRatio || 1;
    const overChrome = (t: EventTarget | null) => {
      const el = t instanceof Element ? t : null;
      // 点击瞬间已被同步摘下的节点仍算控件（与旧白名单语义一致）
      if (el && !el.isConnected) return true;
      return !!(el && el.closest("[data-chrome-hit]"));
    };
    /* [lc-1990] 几何兜底用：光标 CSS 点是否落在任一「有效可见」控件的矩形内。
     * 可见判定与热区上报同一套规则（pointer-events 沿祖先链为 none = 隐藏态控件，
     * 不参与兜底 —— 隐藏控件带内的点击必须照旧转发 mpv，不能被兜底吞掉）。 */
    const inVisibleChromeRect = (x: number, y: number) => {
      const nodes = document.querySelectorAll<HTMLElement>("[data-chrome-hit]");
      for (let i = 0; i < nodes.length; i++) {
        const el = nodes[i];
        try {
          let hidden = false;
          let anc: HTMLElement | null = el;
          while (anc) {
            if (getComputedStyle(anc).pointerEvents === "none") {
              hidden = true;
              break;
            }
            anc = anc.parentElement;
          }
          if (hidden) continue;
          const r = el.getBoundingClientRect();
          if (x >= r.left && x < r.right && y >= r.top && y < r.bottom) return true;
        } catch {
          /* 单个节点异常不拖垮整次判定 */
        }
      }
      return false;
    };
    const mkFlags = (e: MouseEvent) => {
      // MK_*：0x1 左键 / 0x2 右键 / 0x4 Shift / 0x8 Ctrl / 0x10 中键
      let f = 0;
      if (e.buttons & 1) f |= 1;
      if (e.buttons & 2) f |= 2;
      if (e.buttons & 4) f |= 16;
      if (e.shiftKey) f |= 4;
      if (e.ctrlKey) f |= 8;
      return f;
    };
    /* [lc-1988] 全屏点击分流日志（与 Rust `[lc-1988] fwd-recv` 逐行对账）：
     *   · local   = target 命中 [data-chrome-hit]，控件自己处理；
     *   · fwd     = 落在空白区，转给 mpv；
     *   · swallow = 菜单/抑制窗口吞掉；
     * target= 实际命中元素、efpt= elementFromPoint(clientX,clientY) 的独立复核 ——
     * 两者不一致 = WebView2 输入变换与 DOM 命中错位（全屏 resize/DPI 类问题的直接证据）。
     * 只在全屏 + 按键类 pointerdown 打（move/滚轮/非全屏一律静默，防刷屏）。 */
    const describeNode = (n: Element | null) => {
      if (!n) return "null";
      if (!n.isConnected) return "detached";
      const hitEl = n.closest("[data-chrome-hit]");
      const cls =
        n instanceof HTMLElement && n.className
          ? String(n.className).split(/\s+/)[0]
          : n.tagName.toLowerCase();
      return `${hitEl ? "hit" : "blank"}:${cls}`;
    };
    const routeLog = (route: string, e: PointerEvent) => {
      const el = e.target instanceof Element ? e.target : null;
      const ef = document.elementFromPoint(e.clientX, e.clientY);
      void invoke("lat_trace", {
        msg: `lc-1988 down ${route} target=${describeNode(el)} efpt=${describeNode(
          ef,
        )} btn=${e.button} x=${Math.round(e.clientX)} y=${Math.round(e.clientY)} dpr=${dpr()}`,
      }).catch(() => {});
    };
    /* [lc-1990] CSS → mpv 客户区物理像素：唯一、显式的换算点。dpr() 就是 WebView 的
     * 真实 CSS↔物理比（实机对账 lc-1990：clientX*dpr 与 Rust fs-snap 光标物理坐标
     * 逐像素一致，dpr=1.75/3840x2160 与 1.72 全部成立）——分流判定（overChrome /
     * elementFromPoint / 几何兜底）全程留在 CSS 域，只有转发 mpv 这一步换物理。 */
    const cssToMpvPhysical = (e: MouseEvent) => ({
      x: Math.round(e.clientX * dpr()),
      y: Math.round(e.clientY * dpr()),
    });
    const send = (kind: string, e: MouseEvent, delta = 0) => {
      const pt = cssToMpvPhysical(e);
      void invoke("mpv_forward_mouse", {
        kind,
        x: pt.x,
        y: pt.y,
        flags: mkFlags(e),
        delta,
      }).catch(() => {});
    };
    const upKind = (btn: number) => (btn === 0 ? "up" : btn === 1 ? "mup" : "rup");

    const onDown = (e: PointerEvent) => {
      wake();
      const chrome = overChrome(e.target);
      // 悬停态喂给 3s 自动隐藏的判定：光标停在控件上时不隐藏（旧探针 hot 位语义）
      hoverInteractive.current = chrome;
      // [lc-1988] 分流日志：全屏 + 按键类点击才打（与 Rust `[lc-1988] fwd-recv` 对账）
      const loggable =
        stRef.current.fullscreen && (e.button === 0 || e.button === 1 || e.button === 2);
      if (chrome) {
        if (loggable) routeLog("local", e);
        return;
      }
      // 面板打开（点空白=关面板）与关面板抑制窗口内：这一击不归 mpv
      if (menuOpenRef.current || Date.now() < ptSuppressUntil.current) {
        if (loggable) routeLog(menuOpenRef.current ? "swallow:menu" : "swallow:suppress", e);
        return;
      }
      if (e.button !== 0 && e.button !== 1 && e.button !== 2) return;
      // [lc-1990] 几何兜底（纯 CSS 域内判断，坐标不换算）：全屏下若 DOM 分流判「空白」
      // 但光标落在任一可见控件的 CSS 矩形内 —— DOM 命中与视觉错位（WebView 输入变换 /
      // 布局抖动类问题的直接特征）—— 这一击按 local 处理不转发（转发必然表现为
      // 「点啥都暂停」），并把控件带标记为可交互防 3s 自动隐藏。
      if (stRef.current.fullscreen && inVisibleChromeRect(e.clientX, e.clientY)) {
        if (loggable) routeLog("swallow:band", e);
        hoverInteractive.current = true;
        return;
      }
      if (loggable) routeLog("fwd", e);
      // 上一轮按下没等到抬起（mpv 原生拖窗的 modal move loop 会吞掉释放事件）：
      // 先补一发 up 清掉 mpv 的按钮态，避免下一次点击被它的双击合成器误判。
      if (fwdRef.current) send(upKind(fwdRef.current.btn), e);
      fwdRef.current = { btn: e.button };
      send(e.button === 0 ? "down" : e.button === 1 ? "mdown" : "rdown", e);
      // 捕获指针：拖动越过控件 / 抖出窗口也能持续收到 move/up
      try {
        (e.target as Element | null)?.setPointerCapture?.(e.pointerId);
      } catch {
        /* ignore */
      }
    };

    // move 转发按 rAF 合帧：高频 pointermove 只落一次 PostMessage（拖动手感与真实输入一致）
    let moveRaf = 0;
    let pendingMove: PointerEvent | null = null;
    const flushMove = () => {
      moveRaf = 0;
      const e = pendingMove;
      pendingMove = null;
      if (e && !overChrome(e.target)) send("move", e);
    };
    const onMove = (e: PointerEvent) => {
      // 控件续显由真实 mousemove 驱动（旧「探针 overlay-cursor」通道已退役）
      wake();
      hoverInteractive.current = overChrome(e.target);
      if (overChrome(e.target)) return;
      pendingMove = e;
      if (!moveRaf) moveRaf = window.requestAnimationFrame(flushMove);
    };

    const onUp = (e: PointerEvent) => {
      const fwd = fwdRef.current;
      if (!fwd) return;
      if (e.type === "pointerup" && e.button !== fwd.btn) return;
      fwdRef.current = null;
      send(upKind(fwd.btn), e);
      try {
        (e.target as Element | null)?.releasePointerCapture?.(e.pointerId);
      } catch {
        /* ignore */
      }
    };

    // mpv 原生拖窗接管鼠标后，释放事件到不了 WebView：捕获丢失立刻补 up 清状态
    const onLost = () => {
      const fwd = fwdRef.current;
      if (!fwd) return;
      fwdRef.current = null;
      void invoke("mpv_forward_mouse", {
        kind: upKind(fwd.btn),
        x: 0,
        y: 0,
        flags: 0,
        delta: 0,
      }).catch(() => {});
    };

    const onWheel = (e: WheelEvent) => {
      wake();
      if (overChrome(e.target)) return;
      if (menuOpenRef.current || Date.now() < ptSuppressUntil.current) return;
      // 一格 = 120（WHEEL_DELTA）；方向取 deltaY 符号，mpv 只看增量正负
      send("wheel", e, e.deltaY < 0 ? 120 : -120);
    };

    // 右键菜单在覆盖层里没有任何用途（控件都有自己的面板菜单），一律拦掉
    const onCtx = (e: MouseEvent) => e.preventDefault();

    document.addEventListener("pointerdown", onDown, true);
    document.addEventListener("pointermove", onMove, true);
    document.addEventListener("pointerup", onUp, true);
    document.addEventListener("pointercancel", onUp, true);
    document.addEventListener("lostpointercapture", onLost, true);
    document.addEventListener("wheel", onWheel, { capture: true, passive: true });
    document.addEventListener("contextmenu", onCtx);
    return () => {
      if (moveRaf) window.cancelAnimationFrame(moveRaf);
      document.removeEventListener("pointerdown", onDown, true);
      document.removeEventListener("pointermove", onMove, true);
      document.removeEventListener("pointerup", onUp, true);
      document.removeEventListener("pointercancel", onUp, true);
      document.removeEventListener("lostpointercapture", onLost, true);
      document.removeEventListener("wheel", onWheel, true);
      document.removeEventListener("contextmenu", onCtx);
      fwdRef.current = null;
    };
  }, [wake]);

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
              className={`pc-round${st.fullscreen ? " pc-active" : ""}`}
              data-tip={st.fullscreen ? "退出全屏" : "全屏"}
              {...tap(() => {
                // [lc-1948] 这颗键 = 真全屏：与底栏全屏键 / 双击画面同一条「显示器矩形」
                // 路径（mpv fullscreen 属性 → on_player_fullscreen_changed）。无边框、
                // 铺满整个显示器（含任务栏），再点恢复进入前状态。不再 SW_MAXIMIZE ——
                // 最大化只铺工作区，且 mpv 的最大化边框补偿会把窗口矩形挪出屏幕
                // （右/下露缝、顶栏还在），用户体感就是「不是真全屏」。
                toggleFullscreen();
              })}
            >
              {st.fullscreen ? <Minimize2 size={12} /> : <Maximize2 size={12} />}
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
                  // [lc-1948] 统一走 toggleFullscreen（与右上角窗控 / 双击同路径）
                  toggleFullscreen();
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

      {/* [lc-1988] NVIDIA 覆盖层拦截的一次性提示（fs-snap 命中根窗进程含 nvidia 时触发） */}
      {nvHint ? (
        <div
          style={{
            position: "absolute",
            top: 56,
            left: "50%",
            transform: "translateX(-50%)",
            zIndex: 60,
            padding: "8px 14px",
            borderRadius: 12,
            background: "rgba(18,18,18,.88)",
            color: "#fff",
            fontSize: 12,
            fontWeight: 600,
            whiteSpace: "nowrap",
            pointerEvents: "none",
          }}
        >
          检测到 NVIDIA 覆盖层弹窗，可能拦截点击：可在 NVIDIA App 关闭游戏内覆盖层
        </div>
      ) : null}
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
