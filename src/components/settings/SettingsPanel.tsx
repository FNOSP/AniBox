/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Check as IcoCheck, ChevronDown as IcoChevron, Star as IcoStar, X as IcoXL } from "lucide-react";
import { DisclaimerModal } from "../disclaimer/DisclaimerModal";
import {
  GAMEPAD_DEFAULTS,
  GAMEPAD_KEY_TABLE,
  GAMEPAD_RANGES,
  detectGamepad,
  fetchGamepadStatus,
  type GamepadStatusPayload,
} from "../../hooks/useGamepad";
import { fmtSize } from "../../lib/format";
import {
  AppInfo,
  AppSettings,
  Config,
  PatchCheckResult,
  UpdateCheckResult,
  UpdateDownloadResult,
  UpdateProbeReport,
  UpdateProgress,
  Webview2Status,
} from "../../types";
import { formatSpeed, normalizeCheck } from "../../lib/update";

/* [更新与维护] 「关于」区的占位文案。
   TODO: 等后端 `app_info()` 正式接入并返回稳定字段后，可删掉这两个常量，
         改由 app_info().description / repoUrl 直接提供（当前已做「有则优先」）。
   TODO: 项目地址换成正式仓库地址（仓库当前目录名 Fntv-Plus-Tauri2，产品名 AniBox）。 */
const SOFTWARE_DESC =
  "AniBox 是一款基于 Tauri + React 的影视与番剧聚合播放器：支持飞牛影视库、Bangumi 追番、" +
  "TMDB 元数据与自建弹幕源。";
const PROJECT_URL = "https://github.com/FNOSP/AniBox";

/** 用系统默认浏览器打开外链。
 *  Tauri/WebView2 会丢弃 target="_blank" 的新窗口请求，所以必须走 opener 插件；
 *  纯浏览器预览（vite dev）里插件不可用，再退回原生新标签页。
 *  （与 FeedbackPanel 的 openExternal 同一做法。） */
async function openExternal(url: string) {
  try {
    await openUrl(url);
  } catch {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

/** 面板关闭再打开时组件会重新挂载：把最近一次的检查/下载结果放到模块级，
 *  重开时恢复，避免「后台已经下载完了，面板里却还是空的」。 */
let lastCheck: UpdateCheckResult | null = null;
let lastDownload: UpdateDownloadResult | null = null;

/** 设置面板的标签分类。顺序即展示顺序；新增分类时同时补上对应面板内容，
 *  避免设置项「无处可去」或重复渲染。 */
const SET_TABS = [
  { key: "general", label: "通用" },
  { key: "account", label: "账号" },
  { key: "player", label: "播放器" },
  { key: "appearance", label: "外观" },
  { key: "danmu", label: "弹幕" },
  { key: "gamepad", label: "手柄" },
  { key: "storage", label: "缓存与存储" },
  { key: "diagnostics", label: "诊断" },
  { key: "update", label: "更新" },
  { key: "about", label: "关于" },
] as const;
type SetTabKey = (typeof SET_TABS)[number]["key"];

/** [bili-account] B 站账号体检结果（Rust `bili_account_status` 返回）。 */
type BiliStatus = {
  loggedIn: boolean;
  /** missing | valid | expired | error | unknown | no_script_dir */
  status: string;
  uname?: string | null;
  reason?: string | null;
  cookiePresent: boolean;
};

/** [bili-qr] 扫码登录二维码（Rust `bili_qr_generate` 返回）。 */
type BiliQr = { url: string; qrcodeKey: string; image: string };

/** [bili-qr] 扫码轮询结果（Rust `bili_qr_poll` 返回）。 */
type BiliQrPoll = {
  /** 0=成功 / 86101=未扫码 / 86090=已扫码待确认 / 86038=已过期 */
  code: number;
  message: string;
  ok: boolean;
  uname?: string | null;
  cookieSaved?: boolean;
};

/** [bili-qr] 扫码登录的界面状态机。 */
type BiliQrPhase = "idle" | "loading" | "waiting" | "scanned" | "success" | "expired" | "error";

/** [bili-account] 本机 Node 检测结果（Rust `detect_node` 返回）。 */
type NodeInfo = { found: boolean; version: string; path: string; error?: string };

/** [danmaku-diagnose] 单项诊断结果（Rust `danmaku_diagnose` 返回）。 */
type DiagStatus = "ok" | "warn" | "fail";
type DiagCheck = {
  id: string;
  label: string;
  ok: boolean;
  status: DiagStatus;
  detail: string;
  timeMs: number;
};
type DiagResult = {
  keyword: string;
  /** custom（用户输入）/ playing（当前播放标题）/ builtin（内置测试词） */
  keywordSource: string;
  enabled: boolean;
  checks: DiagCheck[];
};

const DIAG_SOURCE_LABEL: Record<string, string> = {
  custom: "手动输入",
  playing: "当前播放标题",
  builtin: "内置测试词",
};

const diagIcon = (status: DiagStatus) =>
  status === "ok" ? "✅" : status === "warn" ? "⚠️" : "❌";


const asText = (v: unknown): string => {
  if (Array.isArray(v)) return v.map((x) => String(x)).join("\n");
  return v === null || v === undefined ? "" : String(v);
};

/** 设置面板专用下拉选择器（替换原生 <select>）。
 *
 *  为什么不用原生 select：Windows/WebView2 下它是系统控件，选项面板由 OS 绘制，
 *  无法跟随深浅主题与 --accent，且圆角/内边距与面板其它控件对不齐。
 *
 *  实现要点：
 *  - 触发器是按钮（role=combobox），弹层用 createPortal 挂到 body：
 *    `.ddet-body` 有 overflow-y:auto，绝对定位弹层会被裁切，必须脱离滚动容器。
 *  - 弹层 position:fixed，按触发器 rect 计算坐标；空间不足时向上翻转。
 *  - 交互：点击外部关闭、Esc 关闭（capture 阶段 stopPropagation，
 *    避免同一次 Esc 把整个设置面板也关掉）、↑/↓/Home/End 移动、Enter/Space 选中、
 *    Tab 关闭；focus-visible 有独立描边。 */
type SetSelectOption = { value: string; label: string };

function SetSelect({
  value,
  options,
  onChange,
  disabled,
  label,
}: {
  value: string;
  options: SetSelectOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  label: string;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [pos, setPos] = useState<{
    left: number;
    top: number;
    width: number;
    maxHeight: number;
    up: boolean;
  } | null>(null);
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const popRef = useRef<HTMLDivElement | null>(null);
  const listId = useId();
  const optionId = (i: number) => `${listId}-opt-${i}`;

  // 旧配置里的未知值（如历史 launch_page="continue"）兜底到第一项显示，不让 UI 空白
  const selectedIndex = Math.max(0, options.findIndex((o) => o.value === value));
  const current = options[selectedIndex] ?? options[0];

  const place = useCallback(() => {
    const el = btnRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const gap = 6;
    const edge = 8;
    // 弹层至少与触发器同宽；右对齐触发器的右缘，但不出视口
    const width = Math.min(
      Math.max(r.width, 168),
      Math.max(120, window.innerWidth - edge * 2),
    );
    const left = Math.min(
      Math.max(edge, r.right - width),
      Math.max(edge, window.innerWidth - width - edge),
    );
    const below = window.innerHeight - r.bottom - gap - edge;
    const above = r.top - gap - edge;
    const up = below < 140 && above > below;
    const maxHeight = Math.max(80, Math.min(280, up ? above : below));
    setPos({ left, top: up ? r.top - gap : r.bottom + gap, width, maxHeight, up });
  }, []);

  const closeMenu = useCallback((focusBtn = false) => {
    setOpen(false);
    if (focusBtn) btnRef.current?.focus();
  }, []);

  const openMenu = useCallback(() => {
    if (disabled) return;
    setActive(Math.max(0, options.findIndex((o) => o.value === value)));
    setOpen(true);
  }, [disabled, options, value]);

  const choose = useCallback(
    (next: string) => {
      onChange(next);
      closeMenu(true);
    },
    [onChange, closeMenu],
  );

  // 打开期间：定位 + 跟随滚动/窗口尺寸变化（设置面板内部滚动也要跟上）
  useLayoutEffect(() => {
    if (!open) return;
    place();
    const onScroll = () => place();
    const onResize = () => place();
    window.addEventListener("resize", onResize);
    document.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("resize", onResize);
      document.removeEventListener("scroll", onScroll, true);
    };
  }, [open, place]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node | null;
      if (!t) return;
      if (popRef.current?.contains(t) || btnRef.current?.contains(t)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        // capture + stopPropagation：这一次 Esc 只关下拉，不关设置面板
        e.preventDefault();
        e.stopPropagation();
        closeMenu(true);
        return;
      }
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        e.stopPropagation();
        const n = options.length;
        if (!n) return;
        setActive((i) =>
          e.key === "ArrowDown" ? (i + 1) % n : (i - 1 + n) % n,
        );
        return;
      }
      if (e.key === "Home" || e.key === "End") {
        e.preventDefault();
        e.stopPropagation();
        setActive(e.key === "Home" ? 0 : options.length - 1);
        return;
      }
      if (e.key === "Enter" || e.key === " " || e.key === "Spacebar") {
        e.preventDefault();
        e.stopPropagation();
        const opt = options[Math.max(0, Math.min(active, options.length - 1))];
        if (opt) choose(opt.value);
        return;
      }
      if (e.key === "Tab") setOpen(false);
    };
    document.addEventListener("mousedown", onDown, true);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [open, options, active, choose, closeMenu]);

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className="set-dd"
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open ? optionId(active) : undefined}
        aria-label={label}
        disabled={disabled}
        onClick={() => (open ? closeMenu() : openMenu())}
        onKeyDown={(e) => {
          if (!open && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
            e.preventDefault();
            openMenu();
          }
        }}
      >
        <span className="set-dd-val">{current?.label ?? ""}</span>
        <IcoChevron className="set-dd-chev" size={14} strokeWidth={2} aria-hidden />
      </button>
      {open &&
        pos &&
        createPortal(
          <div
            ref={popRef}
            id={listId}
            className="set-dd-pop"
            role="listbox"
            aria-label={label}
            style={{
              left: pos.left,
              top: pos.top,
              width: pos.width,
              maxHeight: pos.maxHeight,
              // 向上翻转时以触发器上缘为基准整体上移
              transform: pos.up ? "translateY(-100%)" : undefined,
            }}
          >
            {options.map((o, i) => (
              <div
                key={o.value}
                id={optionId(i)}
                role="option"
                aria-selected={o.value === value}
                tabIndex={-1}
                className={
                  "set-dd-opt" +
                  (i === active ? " active" : "") +
                  (o.value === value ? " selected" : "")
                }
                onMouseDown={(e) => e.preventDefault()}
                onMouseEnter={() => setActive(i)}
                onClick={() => choose(o.value)}
              >
                <span className="set-dd-opt-l">{o.label}</span>
                {o.value === value && (
                  <IcoCheck className="set-dd-opt-check" size={14} strokeWidth={2.4} aria-hidden />
                )}
              </div>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}

export function SettingsPanel({
  onClose,
  onLogout,
  onLogin,
  loggedIn = true,
  settings,
  onSettings,
}: {
  onClose: () => void;
  /** 退出登录：由 App 负责清会话与切页（面板只发意图，不自己改全局状态） */
  onLogout: () => void;
  /** [lc-1623] 去飞牛登录页：飞牛未登录（游客态）时面板给「登录飞牛」入口。
   *  BGM 与飞牛已解耦，此入口只影响飞牛侧。 */
  onLogin?: () => void;
  /** [lc-1623] 飞牛是否已登录（游客态 = false）：账号区据此显示「登录飞牛」而非「退出登录」。 */
  loggedIn?: boolean;
  /** 设置项（App 持有：轮播 Logo / 性能模式都要在面板外生效） */
  settings: AppSettings;
  onSettings: (patch: Partial<AppSettings>) => void;
}) {
  type Stats = { bytes: number; files: number; limitMb: number; dir: string };
  const [stats, setStats] = useState<Stats | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState("");
  const [ver, setVer] = useState("4.0.0");
  const [user, setUser] = useState("");
  const [server, setServer] = useState("");
  const [confirmLogout, setConfirmLogout] = useState(false);
  // [lc-1607] 设置 → 关于 → 免责声明：打开只读态弹窗（只有「关闭」）
  const [showDisclaimer, setShowDisclaimer] = useState(false);
  // 关闭动画：先挂 class 让遮罩淡出 + 卡片缩下沉，动画走完再真正卸载
  const [closing, setClosing] = useState(false);
  const requestClose = useCallback(() => {
    if (closing) return;
    setClosing(true);
    window.setTimeout(onClose, 170); // 与 CSS 动画时长对齐
  }, [closing, onClose]);
  const [autostart, setAutostart] = useState(
    () => localStorage.getItem("fntv-autostart") === "1",
  );

  /* ── 标签分类 ──
     只做布局状态，不参与任何设置真值；切换标签不会触发任何 invoke。
     桌面宽窗为左侧竖向标签，窄窗由 CSS 改为顶部横向可滚动标签（同一套 DOM）。 */
  const [tab, setTab] = useState<SetTabKey>("general");
  const tabsRef = useRef<HTMLDivElement | null>(null);
  const paneRef = useRef<HTMLDivElement | null>(null);

  // 切标签后内容区回到顶部：否则从长页面（手柄/更新）切到短页面会停在半空
  useEffect(() => {
    paneRef.current?.scrollTo({ top: 0 });
  }, [tab]);

  /** 标签栏键盘导航（WAI-ARIA tabs 模式）：←/→/↑/↓ 循环、Home/End 跳首尾。
   *  竖向与横向布局共用同一套按键，避免窗口变窄后键盘失效。 */
  const onTabKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const idx = SET_TABS.findIndex((t) => t.key === tab);
    let next = -1;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") next = (idx + 1) % SET_TABS.length;
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp")
      next = (idx - 1 + SET_TABS.length) % SET_TABS.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = SET_TABS.length - 1;
    if (next < 0) return;
    e.preventDefault();
    e.stopPropagation();
    setTab(SET_TABS[next].key);
    const btns = tabsRef.current?.querySelectorAll<HTMLButtonElement>(".set-tab");
    btns?.[next]?.focus();
  };

  /* ── 手柄 ──
     配置真值在 Rust（get_settings / set_settings 的 gamepad* 字段），
     死区/阈值/长按连跳也由 Rust 侧消费；前端只负责编辑与显示。
     连接状态由 `gamepad-status` 事件刷新，初始化调 `gamepad_status`。 */
  const padEnabled = settings.gamepadEnabled ?? GAMEPAD_DEFAULTS.enabled;
  const padValues = {
    stickDeadzone: settings.gamepadDeadzone ?? GAMEPAD_DEFAULTS.stickDeadzone,
    directionThreshold:
      settings.gamepadDirectionThreshold ?? GAMEPAD_DEFAULTS.directionThreshold,
    repeatDelay: settings.gamepadRepeatDelay ?? GAMEPAD_DEFAULTS.repeatDelay,
    repeatInterval: settings.gamepadRepeatInterval ?? GAMEPAD_DEFAULTS.repeatInterval,
  } as const;
  const PAD_SETTING_FIELD = {
    stickDeadzone: "gamepadDeadzone",
    directionThreshold: "gamepadDirectionThreshold",
    repeatDelay: "gamepadRepeatDelay",
    repeatInterval: "gamepadRepeatInterval",
  } as const;

  const [padConn, setPadConn] = useState<{ connected: boolean; name: string }>({
    connected: false,
    name: "",
  });

  useEffect(() => {
    let un: (() => void) | undefined;
    void fetchGamepadStatus().then(setPadConn);
    listen<GamepadStatusPayload>("gamepad-status", (e) => {
      setPadConn({
        connected: !!e.payload?.connected,
        name: String(e.payload?.name ?? ""),
      });
    })
      .then((f) => {
        un = f;
      })
      .catch(() => {});
    return () => un?.();
  }, []);

  const detectPad = async () => {
    setPadConn(await detectGamepad());
  };

  const setPadNum = (
    key: keyof typeof PAD_SETTING_FIELD,
    value: number,
  ) => {
    onSettings({ [PAD_SETTING_FIELD[key]]: value } as Partial<AppSettings>);
  };

  /* ── [player] 播放器选择 ──
     playerKind: 真值在 settings（App 持有、set_settings 落盘）；默认 mpv（随包自带）。
     potPath   : 路径输入框的本地草稿；settings 回来/保存成功后再同步（输入过程中不回写）。
     potStatus : 探测/校验结果提示（已找到 / 路径无效 / 未安装）。
     留空是合法状态 —— 后端在起播时自动探测常见安装路径。 */
  const playerKind = settings.playerKind ?? "mpv";
  const [potPath, setPotPath] = useState(() => settings.potplayerPath ?? "");
  const [potStatus, setPotStatus] = useState<{
    tone: "ok" | "err" | "dim";
    text: string;
  } | null>(null);
  const [potBusy, setPotBusy] = useState(false);

  useEffect(() => {
    setPotPath(settings.potplayerPath ?? "");
  }, [settings.potplayerPath]);

  /** 切换播放器：立即落盘（与面板其它设置同一条 set_settings 流程）。
   * [lc-1794] 三选：mpv（随包）/ mpv-local（本机安装，自动检测，兼容问题不修）/ potplayer。 */
  const choosePlayer = (v: string) => {
    const kind = v === "potplayer" ? "potplayer" : v === "mpv-local" ? "mpv-local" : "mpv";
    setPotStatus(null);
    onSettings({ playerKind: kind });
  };

  /** 自动探测本机 PotPlayer：命中则回填并保存，未命中提示「未安装」。 */
  const detectPot = async () => {
    setPotBusy(true);
    setPotStatus(null);
    try {
      const r = await invoke<{ path?: string }>("detect_potplayer");
      const p = (r?.path ?? "").trim();
      if (p) {
        setPotPath(p);
        setPotStatus({ tone: "ok", text: `已找到：${p}` });
        onSettings({ playerKind: "potplayer", potplayerPath: p });
      } else {
        setPotStatus({
          tone: "err",
          text: "未安装（未在常见安装路径找到 PotPlayer）",
        });
      }
    } catch (e) {
      setPotStatus({ tone: "err", text: String(e) });
    } finally {
      setPotBusy(false);
    }
  };

  /** 校验路径：save=true 时通过后落盘（用于失焦保存）。空路径不校验，按「自动探测」处理。 */
  const validatePot = async (save: boolean) => {
    const p = potPath.trim();
    if (!p) {
      setPotStatus({
        tone: "dim",
        text: "路径为空：播放时将自动探测常见安装路径",
      });
      if (save && (settings.potplayerPath ?? "")) onSettings({ potplayerPath: "" });
      return;
    }
    setPotBusy(true);
    setPotStatus(null);
    try {
      const r = await invoke<{ valid?: boolean; resolved?: string }>(
        "validate_potplayer_path",
        { path: p },
      );
      if (r?.valid) {
        const resolved = (r.resolved || p).trim();
        setPotPath(resolved);
        setPotStatus({ tone: "ok", text: `已找到：${resolved}` });
        if (save && resolved !== (settings.potplayerPath ?? "")) {
          onSettings({ potplayerPath: resolved });
        }
      } else {
        setPotStatus({
          tone: "err",
          text: "路径无效（必须指向已存在的 .exe 文件）",
        });
      }
    } catch (e) {
      setPotStatus({ tone: "err", text: String(e) });
    } finally {
      setPotBusy(false);
    }
  };

  /* ── [更新与维护] 状态 ──
     upd        : 最近一次 update_check 结果（模块级缓存，重开面板可恢复）
     downloaded : 全量安装包下载完成后的 { path, size }
     progress   : update-progress / patch-progress 事件的最新进度
     phase      : 当前操作阶段，用来决定按钮/禁用态 */
  const [appMeta, setAppMeta] = useState<AppInfo | null>(null);
  /* [webview2] Windows 运行时状态：应用能启动但运行时缺失/过旧时，「关于」页给下载入口。
     注意：运行时**完全缺失**时应用通常根本起不来，那时只能靠根目录的
     `安装WebView2.cmd`（tools/ensure-webview2.ps1）兜底，见 README。 */
  const [wv2, setWv2] = useState<Webview2Status | null>(null);
  const [wv2Busy, setWv2Busy] = useState(false);
  const [wv2Err, setWv2Err] = useState("");
  const checkWebview2 = useCallback(async () => {
    setWv2Busy(true);
    setWv2Err("");
    try {
      setWv2(await invoke<Webview2Status>("check_webview2"));
    } catch (e) {
      setWv2Err(String(e));
    } finally {
      setWv2Busy(false);
    }
  }, []);
  const [upd, setUpd] = useState<UpdateCheckResult | null>(lastCheck);
  const [updChecking, setUpdChecking] = useState(false);
  const [updError, setUpdError] = useState("");
  const [downloading, setDownloading] = useState(false);
  const [downloaded, setDownloaded] = useState<UpdateDownloadResult | null>(lastDownload);
  const [applying, setApplying] = useState(false);
  const [patchApplied, setPatchApplied] = useState(false);
  const [progress, setProgress] = useState<{ percent: number; message: string; speed: number } | null>(null);
  const [showNotes, setShowNotes] = useState(false);
  // 「检测源」探测结果与冷却
  const [probe, setProbe] = useState<UpdateProbeReport | null>(null);
  const [probeBusy, setProbeBusy] = useState(false);
  const [probeError, setProbeError] = useState("");
  const [probeCooldown, setProbeCooldown] = useState(0);
  const [probeCopied, setProbeCopied] = useState(false);
  // 「取消」是用户主动发起的：此后 update_download 的 reject 不再当成错误展示
  const cancelRequested = useRef(false);

  const updBusy = updChecking || downloading || applying;

  const checkUpdate = useCallback(async () => {
    setUpdChecking(true);
    setUpdError("");
    setProgress(null);
    setDownloaded(null);
    lastDownload = null;
    setPatchApplied(false);
    setShowNotes(false);
    try {
      const raw = await invoke<unknown>("update_check");
      const r = normalizeCheck(raw);
      lastCheck = r;
      setUpd(r);
      // 命中热补丁时再取一次 patch_check 的权威说明（失败不影响主流程）
      if (r.hasUpdate && r.updateType === "hotfix") {
        try {
          const p = await invoke<PatchCheckResult>("patch_check");
          if (p?.hasPatch && p.notes) {
            const merged = { ...r, notes: asText(p.notes), latestVersion: asText(p.version) || r.latestVersion };
            lastCheck = merged;
            setUpd(merged);
          }
        } catch {
          /* patch_check 可选：以 update_check 的 notes 为准 */
        }
      }
    } catch (e) {
      lastCheck = null;
      setUpd(null);
      setUpdError(String(e));
    } finally {
      setUpdChecking(false);
    }
  }, []);

  const downloadUpdate = useCallback(async () => {
    if (!upd) return;
    cancelRequested.current = false;
    setDownloading(true);
    setUpdError("");
    setProgress({ percent: 0, message: "准备下载…", speed: 0 });
    try {
      const r = await invoke<UpdateDownloadResult>("update_download", {
        version: upd.latestVersion,
      });
      lastDownload = r;
      setDownloaded(r);
      setProgress({ percent: 100, message: "下载完成", speed: 0 });
    } catch (e) {
      if (!cancelRequested.current) setUpdError(String(e));
      else setProgress(null);
    } finally {
      setDownloading(false);
    }
  }, [upd]);

  const cancelDownload = useCallback(async () => {
    cancelRequested.current = true;
    setProgress((p) => (p ? { ...p, message: "正在取消…" } : p));
    try {
      await invoke("update_cancel");
    } catch (e) {
      setUpdError(String(e));
    }
  }, []);

  const applyPatch = useCallback(async () => {
    if (!upd) return;
    setApplying(true);
    setUpdError("");
    setProgress({ percent: 0, message: "正在应用补丁…", speed: 0 });
    try {
      const r = await invoke<{ applied: boolean }>("patch_apply", {
        version: upd.latestVersion,
      });
      setPatchApplied(Boolean(r?.applied));
      if (!r?.applied) setUpdError("补丁未应用（后端返回 applied=false）");
      else setProgress(null);
    } catch (e) {
      setUpdError(String(e));
    } finally {
      setApplying(false);
    }
  }, [upd]);

  const rollbackPatch = useCallback(async () => {
    setUpdError("");
    try {
      await invoke("patch_rollback");
      setPatchApplied(false);
    } catch (e) {
      setUpdError(String(e));
    }
  }, []);

  const installUpdate = useCallback(async () => {
    if (!downloaded?.path) return;
    setApplying(true);
    setUpdError("");
    try {
      await invoke("update_install", { path: downloaded.path });
    } catch (e) {
      setUpdError(String(e));
    } finally {
      setApplying(false);
    }
  }, [downloaded]);

  /* [更新与维护] 检测源：只做探测（不改更新状态），每源 30s 冷却由后端保证。 */
  const runProbe = useCallback(async () => {
    if (probeBusy || probeCooldown > 0) return;
    setProbeBusy(true);
    setProbeError("");
    setProbeCopied(false);
    try {
      const r = await invoke<UpdateProbeReport>("update_source_probe");
      setProbe(r);
      setProbeCooldown(Math.max(30, r?.cooldownRemaining ?? 0));
    } catch (e) {
      setProbeError(String(e));
    } finally {
      setProbeBusy(false);
    }
  }, [probeBusy, probeCooldown]);

  useEffect(() => {
    if (probeCooldown <= 0) return;
    const t = window.setInterval(() => {
      setProbeCooldown((v) => (v > 0 ? v - 1 : 0));
    }, 1000);
    return () => window.clearInterval(t);
  }, [probeCooldown > 0]);

  const copyProbe = useCallback(async () => {
    if (!probe) return;
    const text = probe.items
      .map((i) =>
        i.skipped
          ? `${i.label}: 冷却中（剩余 ${i.cooldownRemaining}s）`
          : `${i.label}: ${i.ok ? "正常" : "失败"}${i.status ? ` HTTP ${i.status}` : ""} · ${i.elapsedMs}ms` +
            (i.version ? ` · 版本 ${i.version}` : "") +
            (i.error ? ` · ${i.error}` : ""),
      )
      .join("\n");
    try {
      await navigator.clipboard.writeText(text);
      setProbeCopied(true);
      window.setTimeout(() => setProbeCopied(false), 1800);
    } catch (e) {
      setProbeError(`复制失败: ${String(e)}`);
    }
  }, [probe]);

  const refresh = useCallback(() => {
    invoke<Stats>("cache_stats")
      .then(setStats)
      .catch((e) => setNote(String(e)));
  }, []);

  useEffect(() => {
    refresh();
    getVersion().then(setVer).catch(() => {});
    invoke<Config>("get_config")
      .then((c) => {
        setUser(c.username);
        setServer(c.server);
      })
      .catch(() => {});
    // [更新与维护] app_info：拿到就用（description/repoUrl 为空时回退占位常量）
    invoke<AppInfo>("app_info")
      .then((info) => {
        setAppMeta(info);
        if (info?.version) setVer(info.version);
      })
      .catch(() => {});
    // [webview2] 只读检测（注册表/目录），开销可忽略；缺失时「关于」页显示下载入口
    void checkWebview2();
    const h = (e: KeyboardEvent) => {
      // [lc-1607] 免责声明弹窗打开时，Esc 归它（只关弹窗，不连设置面板一起关）
      if (e.key === "Escape" && !showDisclaimer) requestClose();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [requestClose, refresh, showDisclaimer, checkWebview2]);

  /* [更新与维护] 监听下载/补丁进度事件；组件卸载时取消监听。
     兼容：percent 缺省时用 loaded/total 现算；message 缺省时退回 phase。 */
  useEffect(() => {
    const onProgress = (e: { payload: unknown }) => {
      const p = (e.payload ?? {}) as Partial<UpdateProgress> & Record<string, unknown>;
      let percent = Number(p.percent);
      if (!Number.isFinite(percent)) {
        const loaded = Number(p.loaded);
        const total = Number(p.total);
        percent = Number.isFinite(loaded) && Number.isFinite(total) && total > 0 ? (loaded / total) * 100 : 0;
      }
      percent = Math.max(0, Math.min(100, percent));
      const message = asText(p.message ?? p.phase);
      const speed = Number(p.speed);
      setProgress({
        percent,
        message,
        speed: Number.isFinite(speed) && speed > 0 ? speed : 0,
      });
    };
    const u1 = listen("update-progress", onProgress);
    const u2 = listen("patch-progress", onProgress);
    return () => {
      u1.then((f) => f()).catch(() => {});
      u2.then((f) => f()).catch(() => {});
    };
  }, []);

  const setLimit = async (mb: number) => {
    setBusy(true);
    setNote("");
    try {
      const r = await invoke<Stats>("cache_set_limit", { limitMb: mb });
      setStats(r);
      setNote(mb === 0 ? "已设为不限制" : `上限已设为 ${fmtSize(mb * 1024 * 1024)}`);
    } catch (e) {
      setNote(String(e));
    } finally {
      setBusy(false);
    }
  };

  const clearCache = async () => {
    setBusy(true);
    setNote("");
    try {
      const r = await invoke<Stats>("cache_clear");
      setStats(r);
      setNote("缓存已清理（内存 + 磁盘）");
    } catch (e) {
      setNote(String(e));
    } finally {
      setBusy(false);
    }
  };

  /* [lc-1561] 自建弹幕接口（danmu_api）：地址多行输入 + 连通测试。
     保存走 onSettings → Rust 侧落盘后**立即**桥给 Node 客户端并同步 Lua 闸门，
     所以改完不必重启应用重启 mpv。地址非敏感，明文回填（对齐主仓库：地址不写进 conf）。 */
  const [dmBases, setDmBases] = useState(
    () => (settings.danmuApiBases || []).join("\n"),
  );
  const [dmNote, setDmNote] = useState("");
  const [dmTesting, setDmTesting] = useState(false);

  /* [lc-1780] 弹弹play 兼容源（外置 danmaku-api-sources.json）：mpv 侧 dandanplay.lua
     同读这份文件（首个启用的为默认源，失败轮换）。全量读写走 danmaku_sources_get/set，
     每次变更即时保存；mpv 会话缓存列表，重启播放器后生效。 */
  type DdSource = { name: string; baseUrl: string; enabled: boolean };
  const [ddSources, setDdSources] = useState<DdSource[]>([]);
  const [ddNewName, setDdNewName] = useState("");
  const [ddNewUrl, setDdNewUrl] = useState("");
  const [ddCredId, setDdCredId] = useState("");
  const [ddCredSecret, setDdCredSecret] = useState("");
  /* [lc-1789] 字幕源添加区 + 自定义密钥输入（值不回填，仅 Set 状态回显） */
  const [subKeyId, setSubKeyId] = useState("");
  const [subKeyOsub, setSubKeyOsub] = useState("");
  // [lc-1792] 固定五行布局：按 kind 取启停（缺省 true），开关写回 JSON 全量
  const subEnabled = (kind: string) => subSources.find((x) => x.kind === kind)?.enabled ?? true;
  const toggleSub = (kind: string) => {
    const FIXED: SubSource[] = [
      { kind: "xunlei", name: "迅雷", token: "", useBuiltin: false, enabled: true },
      { kind: "assrt", name: "assrt.net", token: "", useBuiltin: true, enabled: true },
      { kind: "opensubtitles", name: "OpenSubtitles", token: "", useBuiltin: true, enabled: true },
      { kind: "acgrip", name: "ACG 论坛", token: "", useBuiltin: false, enabled: true },
      { kind: "subhd", name: "SubHD", token: "", useBuiltin: false, enabled: true },
    ];
    const cur = new Map(subSources.map((x) => [x.kind, x]));
    const next = FIXED.map((f) => {
      const c = cur.get(f.kind);
      return c ? { ...c, enabled: f.kind === kind ? !c.enabled : c.enabled } : { ...f, enabled: f.kind === kind ? false : true };
    });
    setSubSources(next);
    void saveSubSources(next);
  };
  const [acgUser, setAcgUser] = useState("");
  const [acgPass, setAcgPass] = useState("");
  const [sbEmail, setSbEmail] = useState("");
  const [sbPass, setSbPass] = useState("");
  // [lc-1793] 行右侧凭证按钮的展开态（同一时间只展开一行）
  const [credOpen, setCredOpen] = useState<"assrt" | "osub" | "acg" | "subhd" | null>(null);
  const [ddNote, setDdNote] = useState("");
  const ddLoaded = useRef(false);

  const saveDdSources = async (list: DdSource[]) => {
    try {
      await invoke("danmaku_sources_set", { sources: list });
      setDdNote(`已保存 ${list.length} 条（重启播放器生效）`);
    } catch (e) {
      setDdNote(String(e));
    }
  };

  const addDdSource = async () => {
    const url = ddNewUrl.trim().replace(/\/+$/, "");
    if (!url) return;
    if (!/^https?:\/\//.test(url)) {
      setDdNote("地址必须以 http(s):// 开头");
      return;
    }
    if (ddSources.some((s) => s.baseUrl === url)) {
      setDdNote("该地址已在列表中");
      return;
    }
    if (ddSources.length >= 12) {
      setDdNote("最多 12 条");
      return;
    }
    const next = [...ddSources, { name: ddNewName.trim() || url, baseUrl: url, enabled: true }];
    setDdSources(next);
    setDdNewName("");
    setDdNewUrl("");
    await saveDdSources(next);
  };

  useEffect(() => {
    if (ddLoaded.current) return;
    ddLoaded.current = true;
    invoke<{ sources: DdSource[] }>("danmaku_sources_get")
      .then((r) => {
        if (Array.isArray(r?.sources)) setDdSources(r.sources);
      })
      .catch((e) => setDdNote(String(e)));
  }, []);

  /* [lc-1781] 字幕源（subtitle-sources.json）：kind ∈ xunlei/assrt/opensubtitles。
     mpv 侧 sub-assrt.lua 同读这份文件扇出搜索；迅雷无需 token。 */
  type SubSource = { kind: string; name: string; token: string; useBuiltin: boolean; enabled: boolean };
  const [subSources, setSubSources] = useState<SubSource[]>([]);
  const [subNote, setSubNote] = useState("");
  const subLoaded = useRef(false);

  const saveSubSources = async (list: SubSource[]) => {
    try {
      await invoke("subtitle_sources_set", { sources: list });
      setSubNote(`已保存 ${list.length} 条`);
    } catch (e) {
      setSubNote(String(e));
    }
  };

  useEffect(() => {
    if (subLoaded.current) return;
    subLoaded.current = true;
    invoke<{ sources: SubSource[] }>("subtitle_sources_get")
      .then((r) => {
        if (Array.isArray(r?.sources)) setSubSources(r.sources);
      })
      .catch((e) => setSubNote(String(e)));
  }, []);

  /* [danmaku-diagnose] 手动诊断：一次后端调用返回全部检查项，前端只展示。
     关键词留空 = 后端优先用当前播放标题，取不到再用内置测试词。 */
  const [diagKeyword, setDiagKeyword] = useState("");
  const [diagRunning, setDiagRunning] = useState(false);
  const [diagResult, setDiagResult] = useState<DiagResult | null>(null);
  const [diagError, setDiagError] = useState("");
  const [diagCopied, setDiagCopied] = useState(false);

  const parseBases = (text: string) =>
    text
      .split(/[\n;；]/)
      .map((s) => s.trim().replace(/\/+$/, ""))
      .filter(Boolean)
      .slice(0, 4);

  /** 统一提交弹幕相关设置（开关/地址/引擎都走这一条路径，不新增独立保存按钮）。
   *  engine 缺省 = 保留当前值（旧缓存可能没有该字段 → 回退 "node"）。 */
  const saveDanmu = (enabled: boolean, text: string, engine?: "node" | "rust") => {
    onSettings({
      danmuApiEnabled: enabled,
      danmuApiBases: parseBases(text),
      danmuEngine: engine ?? settings.danmuEngine ?? "node",
    });
  };

  const testDanmu = async () => {
    setDmTesting(true);
    setDmNote("");
    try {
      const list = parseBases(dmBases);
      if (!list.length) {
        // 留空 = 测试内置源
        setDmNote(await invoke<string>("test_danmu_api", { url: "" }));
        return;
      }
      const out: string[] = [];
      for (const b of list) {
        out.push(await invoke<string>("test_danmu_api", { url: b }));
      }
      setDmNote(out.join("　"));
    } catch (e) {
      setDmNote(String(e));
    } finally {
      setDmTesting(false);
    }
  };

  /* [danmaku-diagnose] 运行诊断：失败只记录错误，不影响面板其它内容。 */
  const runDiagnose = async () => {
    setDiagRunning(true);
    setDiagError("");
    setDiagCopied(false);
    try {
      const kw = diagKeyword.trim();
      const r = await invoke<DiagResult>("danmaku_diagnose", {
        keyword: kw || null,
      });
      setDiagResult(r);
    } catch (e) {
      setDiagError(String(e));
    } finally {
      setDiagRunning(false);
    }
  };

  /** 诊断结果转纯文本（便于贴给开发者排查）。 */
  const diagPlainText = (r: DiagResult): string => {
    const src = DIAG_SOURCE_LABEL[r.keywordSource] ?? r.keywordSource;
    const lines = [
      `AniBox 弹幕诊断 · 关键词：${r.keyword}（${src}）`,
      `自建接口：${r.enabled ? "已启用" : "未启用"}`,
      ...r.checks.map(
        (c) => `${diagIcon(c.status)} ${c.label}（${c.timeMs}ms）：${c.detail}`,
      ),
    ];
    return lines.join("\n");
  };

  const copyDiag = async () => {
    if (!diagResult) return;
    const text = diagPlainText(diagResult);
    try {
      await navigator.clipboard.writeText(text);
      setDiagCopied(true);
    } catch {
      // WebView 剪贴板不可用时退回 textarea + execCommand
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
        setDiagCopied(true);
      } finally {
        document.body.removeChild(ta);
      }
    }
  };

  /* [bili-account] B 站账号 + Node 环境（弹幕分类）。
     Cookie 真值沿用现有存储：mpv 脚本目录下的 bili_cookie.txt（bili_danmaku.js 直接读它），
     不复制进 config.json；后端命令只做读写与体检，不写日志。 */
  const [bili, setBili] = useState<BiliStatus | null>(null);
  const [biliCookie, setBiliCookie] = useState("");
  const [biliBusy, setBiliBusy] = useState(false);
  const [biliNote, setBiliNote] = useState("");
  const [node, setNode] = useState<NodeInfo | null>(null);
  const [nodeBusy, setNodeBusy] = useState(false);
  const [setupNote, setSetupNote] = useState("");

  /* [bili-qr] 扫码登录：二维码由后端生成（SVG data URI），前端每 2s 轮询一次。
     qrSeq 用于让「切换分区 / 关闭弹层 / 重新生成」后仍在飞的旧轮询失效，
     避免旧请求把新状态覆盖掉。 */
  const [biliQr, setBiliQr] = useState<BiliQr | null>(null);
  const [biliQrPhase, setBiliQrPhase] = useState<BiliQrPhase>("idle");
  const [biliQrNote, setBiliQrNote] = useState("");
  const qrTimer = useRef<number | null>(null);
  const qrSeq = useRef(0);

  const refreshBili = async () => {
    setBiliBusy(true);
    setBiliNote("");
    try {
      setBili(await invoke<BiliStatus>("bili_account_status"));
    } catch (e) {
      setBiliNote(String(e));
    } finally {
      setBiliBusy(false);
    }
  };

  const saveBili = async () => {
    const c = biliCookie.trim();
    if (!c) {
      setBiliNote("请先粘贴 Cookie");
      return;
    }
    setBiliBusy(true);
    setBiliNote("");
    try {
      const s = await invoke<BiliStatus>("bili_set_cookie", { cookie: c });
      setBili(s);
      setBiliCookie("");
      setBiliNote(
        s.loggedIn
          ? `已登录${s.uname ? `：${s.uname}` : ""}`
          : `Cookie 已保存，但校验未通过：${s.reason || s.status}`,
      );
    } catch (e) {
      setBiliNote(String(e));
    } finally {
      setBiliBusy(false);
    }
  };

  const clearBili = async () => {
    setBiliBusy(true);
    setBiliNote("");
    try {
      setBili(await invoke<BiliStatus>("bili_clear_cookie"));
      setBiliNote("已清除 B 站 Cookie（退出登录）");
    } catch (e) {
      setBiliNote(String(e));
    } finally {
      setBiliBusy(false);
    }
  };

  /** [bili-qr] 停止轮询（幂等）。 */
  const stopQrPoll = () => {
    if (qrTimer.current !== null) {
      window.clearInterval(qrTimer.current);
      qrTimer.current = null;
    }
  };

  /** [bili-qr] 关闭扫码层并让所有在飞请求失效。 */
  const cancelQrLogin = () => {
    qrSeq.current += 1;
    stopQrPoll();
    setBiliQr(null);
    setBiliQrPhase("idle");
    setBiliQrNote("");
  };

  /** [bili-qr] 轮询一次；seq 不匹配说明已被取消/重建，直接丢弃结果。 */
  const pollQr = async (qrcodeKey: string, seq: number) => {
    if (seq !== qrSeq.current) return;
    try {
      const p = await invoke<BiliQrPoll>("bili_qr_poll", { qrcodeKey });
      if (seq !== qrSeq.current) return;
      if (p.ok && p.code === 0) {
        stopQrPoll();
        setBiliQrPhase("success");
        setBiliQrNote(
          p.cookieSaved === false
            ? p.message
            : `登录成功${p.uname ? `：${p.uname}` : ""}`,
        );
        setBiliQr(null);
        await refreshBili();
      } else if (p.code === 86090) {
        setBiliQrPhase("scanned");
        setBiliQrNote("已扫码，请在手机上确认登录");
      } else if (p.code === 86038) {
        stopQrPoll();
        setBiliQrPhase("expired");
        setBiliQrNote("二维码已过期，请刷新");
      } else if (p.code === 86101) {
        setBiliQrPhase("waiting");
        setBiliQrNote("等待扫码…");
      } else {
        // 其他业务码（含 Cookie 提取失败）：不停止轮询，把原因显示出来
        setBiliQrNote(p.message || "登录状态异常");
      }
    } catch (e) {
      // 网络抖动不终止轮询，下一次继续尝试
      if (seq === qrSeq.current) setBiliQrNote(String(e));
    }
  };

  /** [bili-qr] 生成二维码并开始轮询。 */
  const startQrLogin = async () => {
    stopQrPoll();
    const seq = ++qrSeq.current;
    setBiliQr(null);
    setBiliQrPhase("loading");
    setBiliQrNote("正在获取二维码…");
    try {
      const r = await invoke<BiliQr>("bili_qr_generate");
      if (seq !== qrSeq.current) return;
      setBiliQr(r);
      setBiliQrPhase("waiting");
      setBiliQrNote("请使用 B 站手机客户端扫码");
      qrTimer.current = window.setInterval(() => {
        void pollQr(r.qrcodeKey, seq);
      }, 2000);
    } catch (e) {
      if (seq === qrSeq.current) {
        setBiliQrPhase("error");
        setBiliQrNote(String(e));
      }
    }
  };

  const refreshNode = async () => {
    try {
      setNode(await invoke<NodeInfo>("detect_node"));
    } catch (e) {
      setNode({ found: false, version: "", path: "", error: String(e) });
    }
  };

  /** 一键免配置自建弹幕源：后端启用自建接口 → 启动内置源 → 启动/复用弹幕 shim。 */
  const setupBuiltin = async () => {
    setNodeBusy(true);
    setSetupNote("");
    try {
      const r = await invoke<{
        ok: boolean;
        nodeVersion: string;
        base: string;
        usingBuiltin: boolean;
      }>("setup_builtin_danmaku");
      setSetupNote(
        `已就绪：Node ${r.nodeVersion} · ${r.usingBuiltin ? "内置源" : "自定义地址"} ${r.base}`,
      );
      // 后端已落盘；同步前端设置快照（避免面板仍显示「未启用」）
      onSettings({
        danmuApiEnabled: true,
        danmuApiBases: settings.danmuApiBases ?? [],
        danmuEngine: settings.danmuEngine ?? "node",
      });
      await refreshNode();
    } catch (e) {
      setSetupNote(String(e));
    } finally {
      setNodeBusy(false);
    }
  };

  // 面板打开即检测（命令只读，不启动任何进程）
  useEffect(() => {
    void refreshBili();
    void refreshNode();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /* [bili-qr] 轮询清理：切换分区 / 关闭面板 / 组件卸载时停止定时器并让在飞请求失效。 */
  useEffect(() => {
    if (tab !== "danmu") cancelQrLogin();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab]);
  useEffect(
    () => () => {
      qrSeq.current += 1;
      stopQrPoll();
    },
    [],
  );

  /** [lc-1437] 自启开关：真值在系统注册表里，前端只在 localStorage 记一份 UI 状态。   *  写入失败（权限/杀软拦截）时**回滚开关**并把错误显示出来 —— 不能出现
   *  "界面显示已开启、实际没写进注册表"这种静默不一致。 */
  const toggleAutostart = async () => {
    const next = !autostart;
    setBusy(true);
    setNote("");
    try {
      await invoke("set_autostart", { enabled: next });
      setAutostart(next);
      localStorage.setItem("fntv-autostart", next ? "1" : "0");
      setNote(next ? "已设为开机自启" : "已取消开机自启");
    } catch (e) {
      setNote(String(e));
    } finally {
      setBusy(false);
    }
  };

  const openPath = async (kind: "log" | "cache" | "app") => {
    setNote("");
    try {
      await invoke("open_path", { kind });
    } catch (e) {
      setNote(String(e));
    }
  };

  const limitMb = stats?.limitMb ?? 0;
  // 下拉里没有的当前值（手改过 config.json）也要显示出来，别让 UI 与真实配置不一致
  const known = [0, 1024, 2048, 5120, 10240];
  const extras = limitMb && !known.includes(limitMb) ? [limitMb] : [];

  /* [更新与维护] 状态行文案（检查中 / 失败 / 已是最新 / 热补丁 / 新版本） */
  const updStatus = (() => {
    if (updChecking) return { text: "检查中…", tone: "dim" as const };
    if (updError) return { text: "检查或操作失败", tone: "err" as const };
    if (!upd) return null;
    if (!upd.hasUpdate) return { text: "已是最新版本", tone: "ok" as const };
    if (upd.updateType === "hotfix")
      return { text: `发现热补丁 v${upd.latestVersion || "?"}`, tone: "warn" as const };
    return { text: `发现新版本 v${upd.latestVersion || "?"}`, tone: "warn" as const };
  })();
  const isHotfix = !!upd?.hasUpdate && upd.updateType === "hotfix";
  const isFull = !!upd?.hasUpdate && upd.updateType === "full";
  const showProgress = !!progress && (downloading || applying || !!downloaded);
  const projectUrl = appMeta?.repoUrl?.trim() || PROJECT_URL;
  const softwareDesc = appMeta?.description?.trim() || SOFTWARE_DESC;

  return (
    <div className={`ddet-mask${closing ? " set-mask-closing" : ""}`} onClick={requestClose}>
      <div className="ddet set-card" onClick={(e) => e.stopPropagation()}>
        <button className="ddet-close" title="关闭（Esc）" onClick={requestClose}>
          <IcoXL size={15} strokeWidth={1.8} />
        </button>
        <header className="ddet-hd">
          <div className="ddet-hdm">
            <h2>设置</h2>
            <div className="ddet-meta">
              <span className="ddet-chip dim">v{ver}</span>
            </div>
          </div>
        </header>
        <div className="set-layout">
          <div
            ref={tabsRef}
            className="set-tabs"
            role="tablist"
            aria-label="设置分类"
            onKeyDown={onTabKeyDown}
          >
            {SET_TABS.map((t) => (
              <button
                key={t.key}
                type="button"
                role="tab"
                id={`set-tab-${t.key}`}
                aria-selected={tab === t.key}
                aria-controls={tab === t.key ? `set-panel-${t.key}` : undefined}
                tabIndex={tab === t.key ? 0 : -1}
                className={`set-tab${tab === t.key ? " on" : ""}`}
                onClick={() => setTab(t.key)}
              >
                {t.label}
              </button>
            ))}
          </div>
          <div
            ref={paneRef}
            className="ddet-body set-body"
            role="tabpanel"
            id={`set-panel-${tab}`}
            aria-labelledby={`set-tab-${tab}`}
            tabIndex={0}
          >
            {/* key=tab：切换标签时整块重挂载，触发 CSS 入场动画（淡入 + 轻微上移） */}
            <div className="set-tabpane" key={tab}>
              {/* ── 通用 ── */}
              {tab === "general" && (
                <>
                  <div className="set-row">
                    <div>
                      <span className="set-k">启动页</span>
                      <div className="set-desc">登录成功后进入的页面</div>
                    </div>
                    <SetSelect
                      label="启动页"
                      value={settings.launchPage}
                      disabled={busy}
                      options={[
                        { value: "home", label: "首页" },
                        { value: "discover", label: "发现页" },
                      ]}
                      onChange={(v) => onSettings({ launchPage: v as AppSettings["launchPage"] })}
                    />
                  </div>
                  <div className="set-row">
                    <div>
                      <span className="set-k">开机自启</span>
                      <div className="set-desc">登录系统后自动启动本程序</div>
                    </div>
                    <button
                      type="button"
                      className={`set-sw${autostart ? " on" : ""}`}
                      disabled={busy}
                      aria-pressed={autostart}
                      aria-label="开机自启"
                      onClick={toggleAutostart}
                    />
                  </div>
                  <div className="set-row">
                    <div>
                      <span className="set-k">关闭按钮</span>
                      <div className="set-desc">点窗口右上角关闭时的行为</div>
                    </div>
                    <SetSelect
                      label="关闭按钮行为"
                      value={settings.exitMode}
                      disabled={busy}
                      options={[
                        { value: "ask", label: "每次询问" },
                        { value: "tray", label: "最小化到托盘" },
                        { value: "exit", label: "直接退出" },
                      ]}
                      onChange={(v) => onSettings({ exitMode: v as AppSettings["exitMode"] })}
                    />
                  </div>
                  {note && <div className="set-note">{note}</div>}

                </>
              )}

              {/* ── 播放器 ── */}
              {tab === "player" && (
                <>
                  {/* [lc-1753] 在线资源搜索：默认关闭；声明「不存储不分发」后手动开启 */}
                  <div className="set-row">
                    <div>
                      <span className="set-k">在线资源搜索</span>
                      <div className="set-desc">
                        本软件不存储、不分发任何影视资源；搜索与播放的内容均由第三方采集接口/规则站点
                        提供（内置少量公共采集接口模板，可自行增删）。 开启即表示你已知悉并同意遵守
                        当地法律法规。 开启后，没有资源的剧集/动漫详情页会显示在线资源搜索面板。
                      </div>
                    </div>
                    <button
                      type="button"
                      className={`set-sw${settings.webrsPanelEnabled ? " on" : ""}`}
                      aria-pressed={!!settings.webrsPanelEnabled}
                      aria-label="在线资源搜索"
                      title="本软件不存储、不分发任何影视资源；内容来自你自行添加的第三方源"
                      onClick={() =>
                        onSettings({ webrsPanelEnabled: !settings.webrsPanelEnabled })
                      }
                    />
                  </div>

                  {/* [lc-1781] 字幕源外置管理（subtitle-sources.json）：迅雷哈希/assrt/OpenSubtitles。
                      播放器字幕菜单（uosc「下载字幕」）按列表扇出搜索；哈希匹配仅本地文件可用。 */}
                  <div className="set-row set-row-col">
                    <span className="set-k">字幕源</span>
                    <div className="set-desc">
                      每行固定一个源：开关=启停；右侧按钮点击展开填写**自己的凭证**（留空保存=用
                      应用内置共享凭证，填写即直接顶掉内置，优先级最高）。五路并行搜索，失败自动跳过。
                    </div>
                    {/* 迅雷：免账号，只有开关 */}
                    <div className="set-row">
                      <button
                        type="button"
                        className={`set-sw${subEnabled("xunlei") ? " on" : ""}`}
                        aria-pressed={subEnabled("xunlei")}
                        aria-label="迅雷源启停"
                        onClick={() => toggleSub("xunlei")}
                      />
                      <span className="sp-badge sp-xunlei" style={{ flex: "0 0 96px", textAlign: "center" }}>迅雷</span>
                      <span className="set-desc" style={{ flex: 1 }}>官方接口 · 按文件哈希精确匹配 · 免账号</span>
                    </div>
                    {/* assrt */}
                    <div className="set-row set-row-col" style={{ gap: 6 }}>
                      <div className="set-row" style={{ flex: 1, gap: 10 }}>
                        <button
                          type="button"
                          className={`set-sw${subEnabled("assrt") ? " on" : ""}`}
                          aria-pressed={subEnabled("assrt")}
                          aria-label="assrt 源启停"
                          onClick={() => toggleSub("assrt")}
                        />
                        <span className="sp-badge sp-assrt" style={{ flex: "0 0 96px", textAlign: "center" }}>assrt</span>
                        <span className="set-desc" style={{ flex: 1 }}>中文字幕库</span>
                        <button
                          type="button"
                          className={`sub-cred-btn${settings.subAssrtTokenSet ? " on" : ""}`}
                          onClick={() => setCredOpen(credOpen === "assrt" ? null : "assrt")}
                        >
                          {settings.subAssrtTokenSet ? "自定义密钥已保存 · 点击修改" : "默认内置 · 点击使用自定义密钥"}
                        </button>
                      </div>
                      {credOpen === "assrt" && (
                        <div className="set-row" style={{ gap: 8, paddingLeft: 40 }}>
                          <input
                            className="set-input"
                            style={{ flex: 1 }}
                            autoFocus
                            placeholder="assrt 自定义 token"
                            value={subKeyId}
                            onChange={(e) => setSubKeyId(e.target.value)}
                          />
                          <button
                            className="set-btn"
                            onClick={() => {
                              onSettings({ subAssrtToken: subKeyId.trim() });
                              setSubKeyId("");
                              setCredOpen(null);
                              setSubNote(subKeyId.trim() ? "assrt：自定义密钥已启用（优先于内置）" : "assrt：已恢复内置共享");
                            }}
                          >
                            保存
                          </button>
                        </div>
                      )}
                    </div>
                    {/* OpenSubtitles */}
                    <div className="set-row set-row-col" style={{ gap: 6 }}>
                      <div className="set-row" style={{ flex: 1, gap: 10 }}>
                        <button
                          type="button"
                          className={`set-sw${subEnabled("opensubtitles") ? " on" : ""}`}
                          aria-pressed={subEnabled("opensubtitles")}
                          aria-label="OpenSubtitles 源启停"
                          onClick={() => toggleSub("opensubtitles")}
                        />
                        <span className="sp-badge sp-osub" style={{ flex: "0 0 96px", textAlign: "center" }}>OpenSubs</span>
                        <span className="set-desc" style={{ flex: 1 }}>国际库 · 哈希精确</span>
                        <button
                          type="button"
                          className={`sub-cred-btn${settings.subOsubKeySet ? " on" : ""}`}
                          onClick={() => setCredOpen(credOpen === "osub" ? null : "osub")}
                        >
                          {settings.subOsubKeySet ? "自定义密钥已保存 · 点击修改" : "默认内置 · 点击使用自定义密钥"}
                        </button>
                      </div>
                      {credOpen === "osub" && (
                        <div className="set-row" style={{ gap: 8, paddingLeft: 40 }}>
                          <input
                            className="set-input"
                            style={{ flex: 1 }}
                            autoFocus
                            placeholder="OpenSubtitles 自定义 Api-Key"
                            value={subKeyOsub}
                            onChange={(e) => setSubKeyOsub(e.target.value)}
                          />
                          <button
                            className="set-btn"
                            onClick={() => {
                              onSettings({ subOsubKey: subKeyOsub.trim() });
                              setSubKeyOsub("");
                              setCredOpen(null);
                              setSubNote(subKeyOsub.trim() ? "OpenSubtitles：自定义密钥已启用" : "OpenSubtitles：已恢复内置共享");
                            }}
                          >
                            保存
                          </button>
                        </div>
                      )}
                    </div>
                    {/* ACG 论坛 */}
                    <div className="set-row set-row-col" style={{ gap: 6 }}>
                      <div className="set-row" style={{ flex: 1, gap: 10 }}>
                        <button
                          type="button"
                          className={`set-sw${subEnabled("acgrip") ? " on" : ""}`}
                          aria-pressed={subEnabled("acgrip")}
                          aria-label="ACG 论坛源启停"
                          onClick={() => toggleSub("acgrip")}
                        />
                        <span className="sp-badge sp-assrt" style={{ flex: "0 0 96px", textAlign: "center" }}>ACG</span>
                        <span className="set-desc" style={{ flex: 1 }}>bbs.acgrip.com · 附件下载需账号</span>
                        <button
                          type="button"
                          className={`sub-cred-btn${settings.acgUserSet ? " on" : ""}`}
                          onClick={() => setCredOpen(credOpen === "acg" ? null : "acg")}
                        >
                          {settings.acgUserSet ? "账号已保存 · 点击修改" : "点击填写账号（游客可搜索）"}
                        </button>
                      </div>
                      {credOpen === "acg" && (
                        <div className="set-row" style={{ gap: 8, paddingLeft: 40 }}>
                          <input
                            className="set-input"
                            style={{ flex: 1 }}
                            autoFocus
                            placeholder="论坛用户名"
                            value={acgUser}
                            onChange={(e) => setAcgUser(e.target.value)}
                          />
                          <input
                            className="set-input"
                            style={{ flex: 1 }}
                            type="password"
                            placeholder="论坛密码"
                            value={acgPass}
                            onChange={(e) => setAcgPass(e.target.value)}
                          />
                          <button
                            className="set-btn"
                            onClick={() => {
                              if (acgUser.trim() !== "" && acgPass.trim() === "") {
                                setSubNote("ACG：密码不能为空");
                                return;
                              }
                              onSettings({ acgUser: acgUser.trim(), acgPass: acgPass.trim() });
                              setAcgUser("");
                              setAcgPass("");
                              setCredOpen(null);
                              setSubNote(acgUser.trim() ? "ACG：账号已保存（下载时自动登录）" : "ACG：已登出（游客模式）");
                            }}
                          >
                            保存
                          </button>
                        </div>
                      )}
                    </div>
                    {/* SubHD */}
                    <div className="set-row set-row-col" style={{ gap: 6 }}>
                      <div className="set-row" style={{ flex: 1, gap: 10 }}>
                        <button
                          type="button"
                          className={`set-sw${subEnabled("subhd") ? " on" : ""}`}
                          aria-pressed={subEnabled("subhd")}
                          aria-label="SubHD 源启停"
                          onClick={() => toggleSub("subhd")}
                        />
                        <span className="sp-badge sp-assrt" style={{ flex: "0 0 96px", textAlign: "center" }}>SubHD</span>
                        <span className="set-desc" style={{ flex: 1 }}>subhd.me · 下载需账号</span>
                        <button
                          type="button"
                          className={`sub-cred-btn${settings.subhdEmailSet ? " on" : ""}`}
                          onClick={() => setCredOpen(credOpen === "subhd" ? null : "subhd")}
                        >
                          {settings.subhdEmailSet ? "账号已保存 · 点击修改" : "点击填写账号（游客可搜索）"}
                        </button>
                      </div>
                      {credOpen === "subhd" && (
                        <div className="set-row" style={{ gap: 8, paddingLeft: 40 }}>
                          <input
                            className="set-input"
                            style={{ flex: 1 }}
                            autoFocus
                            placeholder="SubHD 邮箱"
                            value={sbEmail}
                            onChange={(e) => setSbEmail(e.target.value)}
                          />
                          <input
                            className="set-input"
                            style={{ flex: 1 }}
                            type="password"
                            placeholder="SubHD 密码"
                            value={sbPass}
                            onChange={(e) => setSbPass(e.target.value)}
                          />
                          <button
                            className="set-btn"
                            onClick={() => {
                              if (sbEmail.trim() !== "" && sbPass.trim() === "") {
                                setSubNote("SubHD：密码不能为空");
                                return;
                              }
                              onSettings({ subhdEmail: sbEmail.trim(), subhdPass: sbPass.trim() });
                              setSbEmail("");
                              setSbPass("");
                              setCredOpen(null);
                              setSubNote(sbEmail.trim() ? "SubHD：账号已保存（下载时自动登录）" : "SubHD：已清空账号（仅游客搜索）");
                            }}
                          >
                            保存
                          </button>
                        </div>
                      )}
                    </div>
                    {subNote && <span className="set-note">{subNote}</span>}
                  </div>
                  <div className="set-row">
                    <div>
                      <span className="set-k">播放器选择</span>
                      <div className="set-desc">
                        默认随包自带 mpv；也可改用本机安装的 mpv（自动检测常见安装路径）或 PotPlayer。
                        本机 mpv 出现兼容问题时请自行处理，应用不对其做适配修复。
                      </div>
                    </div>
                    <SetSelect
                      label="播放器选择"
                      value={playerKind}
                      disabled={busy}
                      options={[
                        { value: "mpv", label: "自带 mpv（推荐）" },
                        { value: "mpv-local", label: "本机 mpv（兼容自负）" },
                        { value: "potplayer", label: "本机 PotPlayer" },
                      ]}
                      onChange={choosePlayer}
                    />
                  </div>
                  {playerKind === "potplayer" && (
                    <>
                      <div className="set-row set-row-col">
                        <span className="set-k">PotPlayer 路径</span>
                        <input
                          className="set-input"
                          type="text"
                          spellCheck={false}
                          placeholder="留空将自动探测常见安装路径，或填写 PotPlayerMini64.exe 的完整路径"
                          value={potPath}
                          disabled={busy || potBusy}
                          onChange={(e) => setPotPath(e.target.value)}
                          onBlur={() => {
                            if (potPath.trim() !== (settings.potplayerPath ?? "")) {
                              void validatePot(true);
                            }
                          }}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") {
                              e.preventDefault();
                              void validatePot(true);
                            }
                          }}
                        />
                        <div className="set-actions">
                          <button
                            type="button"
                            className="set-btn"
                            disabled={busy || potBusy}
                            onClick={detectPot}
                          >
                            自动探测
                          </button>
                          <button
                            type="button"
                            className="set-btn"
                            disabled={busy || potBusy}
                            onClick={() => validatePot(false)}
                          >
                            校验
                          </button>
                          {potStatus && (
                            <span className={`set-pot-status ${potStatus.tone}`}>
                              {potStatus.text}
                            </span>
                          )}
                        </div>
                        <div className="set-desc">
                          路径为空时，将在起播时自动探测常见安装路径；填写后需指向已存在的 .exe。
                        </div>
                      </div>
                      <div className="set-warn">
                        PotPlayer 模式不支持内置弹幕、uosc 底栏与精确暂停/跳转/倍速控制；
                        需要 Referer/UA 的 HLS 源会提示切回 mpv。
                      </div>
                </>
              )}

                </>
              )}

              {/* ── 手柄 ── */}
              {tab === "gamepad" && (
                <>
                  <div className="set-row">
                    <div>
                      <span className="set-k">手柄操作</span>
                      <div className="set-desc">用 Xbox 布局手柄浏览界面、控制播放</div>
                    </div>
                    <button
                      type="button"
                      className={`set-sw${padEnabled ? " on" : ""}`}
                      aria-pressed={padEnabled}
                      aria-label="手柄操作"
                      onClick={() => onSettings({ gamepadEnabled: !padEnabled })}
                    />
                  </div>
                  <div className="set-row">
                    <div>
                      <span className="set-k">连接状态</span>
                      <div className="set-desc">浏览器需先按一下手柄才会识别设备</div>
                    </div>
                    <div className="set-pad-status">
                      <span className={`set-pad-dot${padConn.connected ? " on" : " off"}`} />
                      <span className="set-pad-name" title={padConn.name}>
                        {padConn.connected ? padConn.name || "已连接" : "未检测到"}
                      </span>
                      <button
                        type="button"
                        className="set-btn"
                        onClick={() => void detectPad()}
                      >
                        检测
                      </button>
                    </div>
                  </div>
                  {(
                    [
                      {
                        key: "stickDeadzone",
                        label: "摇杆死区",
                        desc: "低于此值视为没有推杆（0.10–0.50）",
                        fmt: (v: number) => v.toFixed(2),
                      },
                      {
                        key: "directionThreshold",
                        label: "方向阈值",
                        desc: "推杆超过此值才触发方向（0.20–0.80）",
                        fmt: (v: number) => v.toFixed(2),
                      },
                      {
                        key: "repeatDelay",
                        label: "长按延迟",
                        desc: "按住多久开始连续移动（100–800ms）",
                        fmt: (v: number) => `${Math.round(v)}ms`,
                      },
                      {
                        key: "repeatInterval",
                        label: "连跳间隔",
                        desc: "连续移动的间隔（50–400ms）",
                        fmt: (v: number) => `${Math.round(v)}ms`,
                      },
                    ] as const
                  ).map((s) => {
                    const range = GAMEPAD_RANGES[s.key];
                    return (
                      <div className="set-row set-pad-slider" key={s.key}>
                        <div>
                          <span className="set-k">{s.label}</span>
                          <div className="set-desc">{s.desc}</div>
                        </div>
                        <div className="set-range-wrap">
                          <input
                            type="range"
                            className="set-range"
                            min={range.min}
                            max={range.max}
                            step={range.step}
                            value={padValues[s.key]}
                            disabled={!padEnabled}
                            aria-label={s.label}
                            onChange={(e) => setPadNum(s.key, Number(e.target.value))}
                          />
                          <span className="set-range-val">{s.fmt(padValues[s.key])}</span>
                        </div>
                      </div>
                    );
                  })}
                  <div className="set-pad-keys" aria-label="手柄按键说明">
                    {GAMEPAD_KEY_TABLE.map((k) => (
                      <div className="set-pad-key" key={k.btn}>
                        <span className="set-pad-btn">{k.label}</span>
                        <span className="set-pad-act">{k.action}</span>
                      </div>
                    ))}
                  </div>
                  <div className="set-hint">
                    播放时 A 暂停、LB/RB 快退/快进 5 秒、LT/RT 调倍速、B 停止；界面中方向键移动白色焦点框，
                    A 确认。窗口失去焦点时浏览器会暂停 Gamepad API，重新点回窗口即可恢复。
                  </div>

                </>
              )}

              {/* ── 外观 ── */}
              {tab === "appearance" && (
                <>
                  <div className="set-row">
                    <div>
                      <span className="set-k">性能模式</span>
                      <div className="set-desc">关闭动画与磨砂效果，低配机更流畅</div>
                    </div>
                    <button
                      type="button"
                      className={`set-sw${settings.perfMode ? " on" : ""}`}
                      aria-pressed={settings.perfMode}
                      aria-label="性能模式"
                      onClick={() => onSettings({ perfMode: !settings.perfMode })}
                    />
                  </div>
                  <div className="set-row">
                    <div>
                      <span className="set-k">轮播透明标题</span>
                      <div className="set-desc">首页轮播用影片透明 Logo 代替文字标题（无 Logo 时自动用文字）</div>
                    </div>
                    <button
                      type="button"
                      className={`set-sw${settings.carouselLogo ? " on" : ""}`}
                      aria-pressed={settings.carouselLogo}
                      aria-label="轮播透明标题"
                      onClick={() => onSettings({ carouselLogo: !settings.carouselLogo })}
                    />
                  </div>

                </>
              )}

              {/* ── 弹幕 ── */}
              {tab === "danmu" && (
                <>
                  {/* ── B 站账号 ── */}
                  <div className="set-row">
                    <span className="set-k">B 站账号</span>
                    <span className="set-v">
                      {bili === null
                        ? "检测中…"
                        : bili.loggedIn
                          ? `已登录${bili.uname ? ` · ${bili.uname}` : ""}`
                          : bili.cookiePresent
                            ? `未登录（Cookie ${bili.status === "expired" ? "已过期" : "无效"}）`
                            : "未登录"}
                    </span>
                  </div>
                  <div className="set-row set-row-col">
                    <span className="set-k">Cookie / SESSDATA</span>
                    <textarea
                      className="set-ta"
                      rows={2}
                      placeholder="粘贴完整 Cookie（含 SESSDATA=…），或只粘贴 SESSDATA 的值"
                      value={biliCookie}
                      disabled={biliBusy}
                      onChange={(e) => setBiliCookie(e.target.value)}
                    />
                    <div className="set-actions">
                      <button
                        className="set-btn"
                        disabled={biliBusy || !biliCookie.trim()}
                        onClick={saveBili}
                      >
                        {biliBusy ? "处理中…" : "保存并验证"}
                      </button>
                      <button className="set-btn" disabled={biliBusy} onClick={refreshBili}>
                        刷新状态
                      </button>
                      <button
                        className="set-btn primary"
                        disabled={biliQrPhase === "loading"}
                        onClick={startQrLogin}
                      >
                        {biliQrPhase === "loading" ? "获取二维码…" : "扫码登录"}
                      </button>
                      {bili?.cookiePresent && (
                        <button className="set-btn danger" disabled={biliBusy} onClick={clearBili}>
                          退出登录
                        </button>
                      )}
                      {biliNote && <span className="set-note">{biliNote}</span>}
                    </div>
                    {/* [bili-qr] 扫码登录层：二维码由后端渲染成 SVG data URI */}
                    {biliQrPhase !== "idle" && (
                      <div className="set-qr">
                        {(biliQrPhase === "waiting" || biliQrPhase === "scanned") && biliQr ? (
                          <>
                            <img
                              className="set-qr-img"
                              src={biliQr.image}
                              alt="B 站登录二维码"
                            />
                            <div className={`set-qr-tip${biliQrPhase === "scanned" ? " ok" : ""}`}>
                              {biliQrNote}
                            </div>
                            <button className="set-btn" onClick={cancelQrLogin}>
                              取消
                            </button>
                          </>
                        ) : biliQrPhase === "expired" || biliQrPhase === "error" ? (
                          <>
                            <div className="set-qr-tip err">{biliQrNote}</div>
                            <div className="set-qr-actions">
                              <button className="set-btn primary" onClick={startQrLogin}>
                                刷新二维码
                              </button>
                              <button className="set-btn" onClick={cancelQrLogin}>
                                关闭
                              </button>
                            </div>
                          </>
                        ) : biliQrPhase === "success" ? (
                          <>
                            <div className="set-qr-tip ok">{biliQrNote}</div>
                            <button className="set-btn" onClick={cancelQrLogin}>
                              完成
                            </button>
                          </>
                        ) : (
                          <div className="set-qr-tip">{biliQrNote}</div>
                        )}
                      </div>
                    )}
                    <div className="set-desc">
                      登录态可绕过 B 站匿名风控、提升弹幕获取成功率。浏览器登录 bilibili.com 后，
                      在开发者工具 → Application → Cookies 复制 SESSDATA（或整段 Cookie）。
                      Cookie 仅保存在本机 mpv 脚本目录（bili_cookie.txt），不会写入日志。
                    </div>
                  </div>

                  {/* ── Node 环境 / 一键自建源 ── */}
                  <div className="set-row">
                    <div>
                      <span className="set-k">Node.js 环境</span>
                      <div className="set-desc">
                        {node === null
                          ? "检测中…"
                          : node.found
                            ? `已检测到 ${node.version}（${node.path}）`
                            : "未检测到 Node.js。自建弹幕源与弹幕服务需要本机安装 Node.js（建议 LTS 版本）。"}
                      </div>
                    </div>
                    <div style={{ display: "flex", gap: 8, flexWrap: "wrap", justifyContent: "flex-end" }}>
                      <button className="set-btn" onClick={() => void refreshNode()}>
                        重新检测
                      </button>
                      {node?.found && (
                        <button className="set-btn" disabled={nodeBusy} onClick={() => void setupBuiltin()}>
                          {nodeBusy ? "配置中…" : "一键配置自建弹幕源"}
                        </button>
                      )}
                      {node && !node.found && (
                        <button
                          className="set-btn"
                          onClick={() => void openExternal("https://nodejs.org/zh-cn/download")}
                        >
                          安装指引
                        </button>
                      )}
                    </div>
                  </div>
                  {setupNote && <div className="set-note">{setupNote}</div>}

                  <div className="set-row">
                    <div>
                      <span className="set-k">自建弹幕接口</span>
                      <div className="set-desc">
                        内置 danmu_api 本地独立运行（零配置），聚合多平台弹幕；未命中自动降级内置 B站。
                        也可填写自部署地址覆盖内置源。
                      </div>
                    </div>
                    <button
                      type="button"
                      className={`set-sw${settings.danmuApiEnabled ? " on" : ""}`}
                      aria-pressed={settings.danmuApiEnabled}
                      aria-label="自建弹幕接口"
                      onClick={() => saveDanmu(!settings.danmuApiEnabled, dmBases)}
                    />
                  </div>

                  {/* [lc-1780] 弹弹play 兼容源（外置 danmaku-api-sources.json）：
                      增删/启停/排序；首个启用的为默认源，传输层失败按序自动切换。
                      mpv 侧脚本加载时读一次，改动需重启播放器生效。 */}
                  <div className="set-row set-row-col">
                    <span className="set-k">弹弹play 兼容源</span>
                    <div className="set-desc">
                      自建弹幕接口与 B 站都未命中时的兜底弹幕库（弹弹play API 兼容端点）。
                      自上而下为优先级，请求失败自动切下一个；自定义源需兼容 dandanplay 开放 API。
                      修改后需重启播放器（mpv）生效。列表存本机 danmaku-api-sources.json。
                    </div>
                    {ddSources.map((s, i) => (
                      <div className="set-row" key={`${i}-${s.baseUrl}`}>
                        <button
                          type="button"
                          className={`set-sw${s.enabled ? " on" : ""}`}
                          aria-pressed={s.enabled}
                          aria-label={`${s.name} 启停`}
                          onClick={() => {
                            const next = ddSources.map((x, j) => (j === i ? { ...x, enabled: !x.enabled } : x));
                            setDdSources(next);
                            void saveDdSources(next);
                          }}
                        />
                        <input
                          className="set-input"
                          style={{ flex: "0 0 150px" }}
                          value={s.name}
                          placeholder="名称"
                          onChange={(e) => {
                            const next = ddSources.map((x, j) => (j === i ? { ...x, name: e.target.value } : x));
                            setDdSources(next);
                          }}
                          onBlur={() => void saveDdSources(ddSources)}
                        />
                        <input
                          className="set-input"
                          style={{ flex: 1 }}
                          value={s.baseUrl}
                          placeholder="https://…（dandanplay 兼容 API）"
                          onChange={(e) => {
                            const next = ddSources.map((x, j) => (j === i ? { ...x, baseUrl: e.target.value } : x));
                            setDdSources(next);
                          }}
                          onBlur={() => void saveDdSources(ddSources)}
                        />
                        <button
                          className="set-btn"
                          title="上移（提高优先级）"
                          disabled={i === 0}
                          onClick={() => {
                            const next = ddSources.slice();
                            [next[i - 1], next[i]] = [next[i], next[i - 1]];
                            setDdSources(next);
                            void saveDdSources(next);
                          }}
                        >
                          ↑
                        </button>
                        <button
                          className="set-btn"
                          title="删除此源"
                          onClick={() => {
                            const next = ddSources.filter((_, j) => j !== i);
                            setDdSources(next);
                            void saveDdSources(next);
                          }}
                        >
                          ✕
                        </button>
                      </div>
                    ))}
                    <div className="set-actions">
                      <input
                        className="set-input"
                        style={{ flex: "0 0 150px" }}
                        placeholder="新源名称"
                        value={ddNewName}
                        onChange={(e) => setDdNewName(e.target.value)}
                      />
                      <input
                        className="set-input"
                        style={{ flex: 1 }}
                        placeholder="https://…（dandanplay 兼容 API 根地址）"
                        value={ddNewUrl}
                        onChange={(e) => setDdNewUrl(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") void addDdSource();
                        }}
                      />
                      <button
                        className="set-btn"
                        disabled={!ddNewUrl.trim()}
                        onClick={() => void addDdSource()}
                      >
                        添加
                      </button>
                      <span className="set-note">{ddNote}</span>
                    </div>
                    {/* [lc-1788] 弹弹play 自定义凭证：填写即**优先于内置**（直接顶掉内置共享凭证，
                        lua dd_custom_credentials 覆盖链）；留空保存=恢复内置。只存本机 config.json。 */}
                    <div className="set-row" style={{ flexWrap: "wrap" }}>
                      <span className="set-k" style={{ flex: "0 0 100%" }}>
                        弹弹play 自定义凭证
                      </span>
                      <input
                        className="set-input"
                        style={{ flex: 1 }}
                        placeholder={
                          settings.danmuDdAppIdSet
                            ? "AppId 已设置（清空两个框并保存 = 恢复内置）"
                            : "AppId（留空 = 使用内置共享凭证）"
                        }
                        onChange={(e) => setDdCredId(e.target.value)}
                      />
                      <input
                        className="set-input"
                        style={{ flex: 1 }}
                        placeholder={settings.danmuDdSecretSet ? "Secret 已设置" : "Secret（留空 = 内置）"}
                        onChange={(e) => setDdCredSecret(e.target.value)}
                      />
                      <button
                        className="set-btn"
                        onClick={() => {
                          onSettings({ danmuDdAppId: ddCredId.trim(), danmuDdSecret: ddCredSecret.trim() });
                          setDdCredId("");
                          setDdCredSecret("");
                          setDdNote(
                            ddCredId.trim() && ddCredSecret.trim()
                              ? "已启用自定义凭证（优先于内置，重启播放器生效）"
                              : ddCredId.trim() || ddCredSecret.trim()
                                ? "AppId 与 Secret 需成对填写；本次未保存"
                                : "已清空自定义凭证，恢复内置共享（重启播放器生效）",
                          );
                        }}
                      >
                        保存凭证
                      </button>
                    </div>
                  </div>
                  <div className="set-row set-row-col">
                    <span className="set-k">服务地址</span>
                    <textarea
                      className="set-ta"
                      rows={2}
                      placeholder={"留空即用内置源（本机自动启动）\n如需覆盖：http://192.168.31.170:9321/你的TOKEN"}
                      value={dmBases}
                      disabled={busy}
                      onChange={(e) => setDmBases(e.target.value)}
                      onBlur={() => saveDanmu(settings.danmuApiEnabled, dmBases)}
                    />
                    <div className="set-actions">
                      <input
                        className="set-input set-diag-kw"
                        placeholder="测试关键词（留空=当前播放标题/内置词）"
                        value={diagKeyword}
                        disabled={diagRunning}
                        onChange={(e) => setDiagKeyword(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" && !diagRunning) void runDiagnose();
                        }}
                      />
                      <button
                        className="set-btn"
                        disabled={diagRunning}
                        onClick={() => void runDiagnose()}
                      >
                        {diagRunning ? "诊断中…" : "诊断"}
                      </button>
                      <button
                        className="set-btn"
                        disabled={dmTesting}
                        onClick={testDanmu}
                      >
                        {dmTesting ? "测试中…" : "测试连接"}
                      </button>
                      <button
                        className="set-btn"
                        disabled={busy}
                        onClick={() => saveDanmu(settings.danmuApiEnabled, dmBases)}
                      >
                        保存
                      </button>
                      <span className="set-note">{dmNote}</span>
                    </div>
                    <div className="set-desc">
                      每行一条（最多 4 条，故障转移）。地址只存本机，不写入 mpv 配置。
                      内置弹幕源需要本机安装 Node.js；未安装时会自动降级为内置 B 站弹幕。
                    </div>
                    {/* [danmaku-diagnose] 诊断结果面板：逐项 ✅/⚠️/❌ + 说明 + 耗时 */}
                    {(diagRunning || diagResult || diagError) && (
                      <div className="set-diag">
                        <div className="set-diag-head">
                          <span className="set-k">诊断结果</span>
                          {diagResult && (
                            <span className="set-diag-src">
                              关键词：{diagResult.keyword}（
                              {DIAG_SOURCE_LABEL[diagResult.keywordSource] ??
                                diagResult.keywordSource}
                              ）
                            </span>
                          )}
                          <button
                            className="set-btn"
                            disabled={!diagResult}
                            onClick={() => void copyDiag()}
                          >
                            {diagCopied ? "已复制" : "复制诊断信息"}
                          </button>
                        </div>
                        {diagError && <div className="set-diag-err">诊断失败：{diagError}</div>}
                        {diagResult && (
                          <ul className="set-diag-list">
                            {diagResult.checks.map((c) => (
                              <li key={c.id} className={`set-diag-item ${c.status}`}>
                                <span className="set-diag-ico">{diagIcon(c.status)}</span>
                                <span className="set-diag-label">{c.label}</span>
                                <span className="set-diag-time">{c.timeMs}ms</span>
                                <span className="set-diag-detail">{c.detail}</span>
                              </li>
                            ))}
                          </ul>
                        )}
                        {diagRunning && (
                          <div className="set-diag-run">正在检查…（每项最长 5s）</div>
                        )}
                      </div>
                    )}
                  </div>
                  <div className="set-row">
                    <div>
                      <span className="set-k">弹幕引擎</span>
                      <div className="set-desc">
                        Rust 为测试版：自建源（danmu_api）走 Rust，B 站等其余端点仍由 Node 承担。
                        {!settings.danmuApiEnabled && "（需先启用「自建弹幕接口」）"}
                      </div>
                    </div>
                    <SetSelect
                      label="弹幕引擎"
                      value={settings.danmuEngine ?? "node"}
                      disabled={busy || !settings.danmuApiEnabled}
                      options={[
                        { value: "node", label: "Node（默认/稳定）" },
                        { value: "rust", label: "Rust（测试版）" },
                      ]}
                      onChange={(v) =>
                        saveDanmu(settings.danmuApiEnabled, dmBases, v as "node" | "rust")
                      }
                    />
                  </div>

                </>
              )}

              {/* ── 账号 ── */}
              {tab === "account" && (
                <>
                  <div className="set-row">
                    <span className="set-k">当前账号</span>
                    <span className="set-v">{loggedIn ? user || "—" : "未登录"}</span>
                  </div>
                  <div className="set-row">
                    <span className="set-k">服务器</span>
                    <span className="set-v">{server || "—"}</span>
                  </div>
                  {/* [lc-1623] 飞牛未登录（游客态）→ 给「登录飞牛」入口；已登录 → 仍是「退出登录」。
                      BGM 追番与飞牛已解耦，未登录飞牛不影响 BGM。 */}
                  {loggedIn ? (
                    <div className="set-row">
                      <span className="set-k">退出登录</span>
                      {confirmLogout ? (
                        <div className="set-confirm">
                          <span className="set-confirm-q">退出后需重新登录，确定？</span>
                          <button className="set-btn" disabled={busy} onClick={() => setConfirmLogout(false)}>
                            取消
                          </button>
                          <button
                            className="set-btn danger"
                            disabled={busy}
                            onClick={() => {
                              setConfirmLogout(false);
                              onLogout();
                            }}
                          >
                            确定退出
                          </button>
                        </div>
                      ) : (
                        <button className="set-btn danger" onClick={() => setConfirmLogout(true)}>
                          退出登录
                        </button>
                      )}
                    </div>
                  ) : (
                    <div className="set-row">
                      <div>
                        <span className="set-k">飞牛登录</span>
                        <div className="set-desc">未登录飞牛时，Bangumi 追番等功能仍照常可用</div>
                      </div>
                      <button className="set-btn" onClick={() => onLogin?.()}>
                        登录飞牛
                      </button>
                    </div>
              )}

                </>
              )}

              {/* ── 缓存 ── */}
              {tab === "storage" && (
                <>
                  <div className="set-row">
                    <span className="set-k">占用上限</span>
                    <select
                      className="set-select"
                      value={limitMb}
                      disabled={busy}
                      onChange={(e) => setLimit(Number(e.target.value))}
                    >
                      {extras.map((mb) => (
                        <option key={mb} value={mb}>
                          {fmtSize(mb * 1024 * 1024)}
                        </option>
                      ))}
                      <option value={1024}>1 GB</option>
                      <option value={2048}>2 GB</option>
                      <option value={5120}>5 GB</option>
                      <option value={10240}>10 GB</option>
                      <option value={0}>不限制</option>
                    </select>
                  </div>
                  <div className="set-row">
                    <span className="set-k">当前占用</span>
                    <span className="set-v">
                      {stats ? `${fmtSize(stats.bytes)} · ${stats.files} 个文件` : "读取中…"}
                    </span>
                  </div>
                  <div className="set-row">
                    <span className="set-k">清理缓存</span>
                    <button className="set-btn" onClick={clearCache} disabled={busy || !stats?.bytes}>
                      立即清理
                    </button>
                  </div>
                  <div className="set-hint">
                    图片与演员表缓存在软件数据目录（{stats?.dir || "…"}）：命中本地就不再请求，超上限按最后访问
                    时间自动淘汰；清理会同时清掉内存与磁盘。
                  </div>
                  {note && <div className="set-note">{note}</div>}

                </>
              )}

              {/* ── 诊断 ── */}
              {tab === "diagnostics" && (
                <>
                  <div className="set-row">
                    <span className="set-k">打开目录</span>
                    <div style={{ display: "flex", gap: 8, flexWrap: "wrap", justifyContent: "flex-end" }}>
                      <button className="set-btn" onClick={() => openPath("log")}>
                        诊断日志
                      </button>
                      <button className="set-btn" onClick={() => openPath("cache")}>
                        缓存目录
                      </button>
                      <button className="set-btn" onClick={() => openPath("app")}>
                        数据目录
                      </button>
                    </div>
                  </div>
                  <div className="set-hint">
                    诊断日志在系统临时目录（anibox.log），记录了镜像请求、图片缓存命中与播放链路；
                    反馈问题时把最后一段贴出来即可。
                  </div>
                  {note && <div className="set-note">{note}</div>}

                </>
              )}

              {/* ── 更新与维护 ── */}
              {tab === "update" && (
                <>
                  <div className="set-row">
                    <span className="set-k">当前版本</span>
                    <span className="set-v">v{ver}</span>
                  </div>
                  <div className="set-row">
                    <div>
                      <span className="set-k">检查更新</span>
                      <div className="set-desc">热补丁无需重装；全量更新后台下载，完成后提示安装</div>
                    </div>
                    <button
                      type="button"
                      className="set-btn"
                      disabled={updBusy}
                      onClick={checkUpdate}
                    >
                      {updChecking ? "检查中…" : "检查更新"}
                    </button>
                  </div>

                  <div className="set-row">
                    <div>
                      <span className="set-k">检测源</span>
                      <div className="set-desc">
                        依次探测 自建镜像 → Gitee → 原生 GitHub（只读，不改变更新状态；每源 30 秒冷却）
                      </div>
                    </div>
                    <button
                      type="button"
                      className="set-btn"
                      disabled={probeBusy || probeCooldown > 0}
                      onClick={() => void runProbe()}
                    >
                      {probeBusy ? "检测中…" : probeCooldown > 0 ? `冷却 ${probeCooldown}s` : "检测源"}
                    </button>
                  </div>

                  {(probe || probeError) && (
                    <div className="set-row set-row-col set-upd">
                      <div className="set-upd-status">
                        <span className="set-upd-status-text">源探测结果</span>
                        {probe && (
                          <button type="button" className="set-btn" onClick={() => void copyProbe()}>
                            {probeCopied ? "已复制" : "复制结果"}
                          </button>
                        )}
                      </div>
                      {probeError && <div className="set-upd-err"><span>{probeError}</span></div>}
                      {probe?.items.map((it) => (
                        <div key={it.id} className="set-row" style={{ padding: "4px 0" }}>
                          <div>
                            <span className="set-k">{it.label}</span>
                            <div className="set-desc">{it.url}</div>
                          </div>
                          <span className="set-v">
                            {it.skipped
                              ? `冷却中 ${it.cooldownRemaining}s`
                              : it.ok
                                ? `正常 · ${it.elapsedMs}ms${it.version ? ` · v${it.version.replace(/^v/i, "")}` : ""}`
                                : `${it.status ? `HTTP ${it.status} · ` : ""}${it.elapsedMs}ms${it.error ? ` · ${it.error}` : ""}`}
                          </span>
                        </div>
                      ))}
                    </div>
                  )}

                  {(updStatus || updError) && (
                    <div className="set-row set-row-col set-upd">
                      <div className="set-upd-status">
                        {updStatus && <span className={`set-upd-dot ${updStatus.tone}`} />}
                        <span className="set-upd-status-text">{updStatus?.text}</span>
                        {upd?.publishedAt && <span className="set-upd-date">发布于 {upd.publishedAt}</span>}
                        {upd?.source && <span className="set-upd-date">更新源 {upd.source}</span>}
                        {!!upd?.fromCache && upd.cooldownRemaining ? (
                          <span className="set-upd-date">冷却缓存（{upd.cooldownRemaining}s）</span>
                        ) : null}
                      </div>

                      {!!upd?.notes && (
                        <div className="set-upd-notes-wrap">
                          <button
                            type="button"
                            className="set-btn set-upd-notes-toggle"
                            onClick={() => setShowNotes((v) => !v)}
                          >
                            {showNotes ? "收起更新说明" : "查看更新说明"}
                          </button>
                          {/* 纯文本渲染：React 默认转义，不解析/执行 HTML */}
                          {showNotes && <pre className="set-upd-notes">{upd?.notes}</pre>}
                        </div>
                      )}

                      {showProgress && progress && (
                        <div className="set-upd-progress">
                          <div className="set-upd-bar">
                            <i style={{ width: `${progress.percent}%` }} />
                          </div>
                          <div className="set-upd-progress-meta">
                            <span className="set-upd-pct">{Math.round(progress.percent)}%</span>
                            {progress.speed > 0 && <span className="set-upd-msg">{formatSpeed(progress.speed)}</span>}
                            {progress.message && <span className="set-upd-msg">{progress.message}</span>}
                          </div>
                        </div>
                      )}

                      <div className="set-actions">
                        {downloading && (
                          <button type="button" className="set-btn" onClick={cancelDownload}>
                            取消
                          </button>
                        )}
                        {isHotfix && !patchApplied && (
                          <button
                            type="button"
                            className="set-btn primary"
                            disabled={applying || downloading}
                            onClick={applyPatch}
                          >
                            {applying ? "应用中…" : "应用补丁"}
                          </button>
                        )}
                        {isHotfix && patchApplied && (
                          <>
                            <span className="set-upd-ok">补丁已应用</span>
                            <button type="button" className="set-btn" onClick={rollbackPatch}>
                              回滚补丁
                            </button>
                          </>
                        )}
                        {isFull && !downloaded && (
                          <button
                            type="button"
                            className="set-btn primary"
                            disabled={downloading}
                            onClick={downloadUpdate}
                          >
                            {downloading ? "下载中…" : "下载安装包"}
                          </button>
                        )}
                        {isFull && downloaded && (
                          <button
                            type="button"
                            className="set-btn primary"
                            disabled={applying}
                            onClick={installUpdate}
                          >
                            {applying ? "安装中…" : "立即安装"}
                          </button>
                        )}
                      </div>

                      {updError && (
                        <div className="set-upd-err">
                          <span>{updError}</span>
                          <button type="button" className="set-btn" disabled={updBusy} onClick={checkUpdate}>
                            重试
                          </button>
                        </div>
                      )}
                    </div>
              )}

                </>
              )}

              {/* ── 关于 ── */}
              {tab === "about" && (
                <>
                  <div className="set-row set-row-col">
                    <span className="set-k">软件说明</span>
                    <div className="set-desc">{softwareDesc}</div>
                  </div>
                  {/* [webview2] 运行时状态：缺失/版本过低 → 官方 Bootstrapper 下载入口。
                      运行时完全缺失时本应用通常无法启动，此时请用根目录「安装WebView2.cmd」。 */}
                  <div className="set-row">
                    <div>
                      <span className="set-k">WebView2 运行时</span>
                      <div className="set-desc">
                        {wv2Busy && !wv2
                          ? "检测中…"
                          : wv2
                            ? !wv2.installed
                              ? "未检测到（应用可能无法正常显示界面）"
                              : wv2.outdated
                                ? `版本过低（v${wv2.version || "?"}，需要 ≥ v${wv2.minimumVersion}）`
                                : `已安装${wv2.version ? ` · v${wv2.version}` : ""}`
                            : wv2Err
                              ? `检测失败：${wv2Err}`
                              : "未检测"}
                      </div>
                    </div>
                    <div className="set-actions">
                      {wv2 && (!wv2.installed || wv2.outdated) && (
                        <button
                          type="button"
                          className="set-btn primary"
                          onClick={() => void openExternal(wv2.downloadUrl)}
                        >
                          下载安装
                        </button>
                      )}
                      <button
                        type="button"
                        className="set-btn"
                        disabled={wv2Busy}
                        onClick={() => void checkWebview2()}
                      >
                        {wv2Busy ? "检测中…" : "重新检测"}
                      </button>
                    </div>
                  </div>
                  {wv2 && (!wv2.installed || wv2.outdated) && (
                    <div className="set-row set-row-col">
                      <div className="set-desc">
                        下载安装包会自动打开微软官方页面（Evergreen Bootstrapper，约 1.8 MB）。
                        无网络/内网环境可改用
                        <a
                          href={wv2.standaloneUrl}
                          target="_blank"
                          rel="noreferrer"
                          style={{ marginLeft: 4 }}
                          onClick={(e) => {
                            // WebView2 会丢弃 target="_blank"：统一走 opener 插件
                            e.preventDefault();
                            void openExternal(wv2.standaloneUrl);
                          }}
                        >
                          官方 x64 离线安装包
                        </a>
                        ；安装完成后点「重新检测」。
                      </div>
                    </div>
                  )}
                  <div className="set-row">
                    <div>
                      <span className="set-k">项目地址</span>
                      <div className="set-desc">{projectUrl}</div>
                    </div>
                    <div className="set-actions">
                      <button
                        type="button"
                        className="set-btn"
                        onClick={() => openExternal(PROJECT_URL)}
                        title="在 GitHub 上给 AniBox 点个星标"
                      >
                        <IcoStar size={14} strokeWidth={1.8} style={{ marginRight: 6, verticalAlign: "-2px" }} />
                        点个星标
                      </button>
                      <button type="button" className="set-btn" onClick={() => openExternal(projectUrl)}>
                        打开
                      </button>
                    </div>
                  </div>
                  <div className="set-attrib">
                    <span>This product uses the TMDB API but is not endorsed or certified by TMDB.</span>
                    <span>
                      <a href="https://www.themoviedb.org/" target="_blank" rel="noreferrer">
                        themoviedb.org
                      </a>
                    </span>
                    <span>
                      数据与部分内容来自{" "}
                      <a href="https://bgm.tv/" target="_blank" rel="noreferrer">
                        Bangumi 番组计划
                      </a>
                    </span>
                  </div>
                  {/* [lc-1607] 版权 / 资源纠纷风险规避入口：免责声明（只读弹窗）+ 开源许可 */}
                  <div className="set-row">
                    <div>
                      <span className="set-k">免责声明</span>
                      <div className="set-desc">首次启动需阅读并同意的完整条款</div>
                    </div>
                    <button className="set-btn" onClick={() => setShowDisclaimer(true)}>
                      查看
                    </button>
                  </div>
                  <div className="set-row">
                    <div>
                      <span className="set-k">开源许可（GPL-3.0）</span>
                      <div className="set-desc">本项目基于 GNU GPL-3.0 许可证开源发布（见仓库根目录 LICENSE）</div>
                    </div>
                    <a
                      className="set-btn"
                      href="https://www.gnu.org/licenses/gpl-3.0.html"
                      target="_blank"
                      rel="noreferrer"
                      style={{ textDecoration: "none" }}
                    >
                      许可证全文
                    </a>
                  </div>
                </>
              )}
            </div>
          </div>
        </div>
      </div>
      {/* [lc-1607] 免责声明只读态：portal 到 body，只有「关闭」 */}
      {showDisclaimer && (
        <DisclaimerModal variant="info" onClose={() => setShowDisclaimer(false)} />
      )}
    </div>
  );
}

/* ────────────────────────── 根组件 ────────────────────────── */
