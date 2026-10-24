/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createPortal } from "react-dom";
import { useDelayedUnmount } from "../../hooks/useDelayedUnmount";

export type WebrsSearchItem = { name: string; url: string };

export type WebrsRoad = { name: string; index: number; episodes: WebrsSearchItem[] };

// [lc-1606] 新增 captcha 状态：该源返回了反爬验证挑战页，需要人机验证
// [lc-1747] via:"harvest"（可选）：结果来自验证通过后的收割页缓存（无需网络），Rust 侧按需附加
export type WebrsGroup = { name: string; status: "ok" | "empty" | "error" | "captcha"; error: string; items: WebrsSearchItem[]; via?: string };

// [lc-1747] 嗅探进度事件（webrs-sniff-status，Rust 契约：字段名恒定、message 恒存在无话为空串）。
// 解析直链的 WebView 段发射；按 rule/url 匹配当前解析任务，无关事件忽略。
type SniffStatus = {
  rule: string;
  url: string;
  status: "loading" | "hooked" | "hit" | "timeout" | "failed";
  message: string;
};

export type WebrsRuleInfo = { name: string; version?: string; mode?: string; kind?: "kazumi" | "maccms" };

// [lc-1608] 在线规则目录条目（对应 Rust CatalogItem）+ 可更新项
export type WebrsCatalogItem = {
  name: string;
  version: string;
  author?: string;
  useNativePlayer?: boolean;
  lastUpdate?: number;
};
export type WebrsUpdateItem = { name: string; localVersion: string; remoteVersion: string };
// [lc-1757] 在线采集源目录条目（Rust MaccmsCatalogItem + 安装态）
export type MaccmsCatItem = {
  name: string;
  version: string;
  api: string;
  installed: boolean;
  localVersion: string;
  updatable: boolean;
};


export const WEBRS_ENABLED_KEY = "anibox.webrs.enabledSources";

/** [lc-1775] 相对时间（最近观看 chips 用；无时间戳 → 空串不渲染） */
function agoOf(ts?: number): string {
  if (!ts || ts <= 0) return "";
  const mins = Math.floor((Date.now() - ts) / 60000);
  if (mins < 1) return "刚刚";
  if (mins < 60) return `${mins}分钟前`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}小时前`;
  const days = Math.floor(hrs / 24);
  return days < 30 ? `${days}天前` : `${Math.floor(days / 30)}个月前`;
}
// [lc-1755] 采集源（TMDB 剧集侧）独立勾选存储 —— 与 Kazumi 规则源完全分家
export const WEBRS_MACCMS_KEY = "anibox.webrs.maccmsEnabled";

/** 已勾选源名单（localStorage 持久化）。null = 从未改过 → 视为全启用。 */

export function loadEnabled(): string[] | null {
  try {
    const raw = localStorage.getItem(WEBRS_ENABLED_KEY);
    if (raw == null) return null;
    const v = JSON.parse(raw);
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return null;
  }
}


/** [lc-1754] kind：按条目来源分源——有 BGM 编号（番剧）挂 kazumi 规则源，
 *  无编号（TMDB 剧集/电影）挂 maccms 采集源；undefined = 不过滤（全源，兼容旧调用）。 */
export function WebrsPanel({
  title,
  kind,
  poster,
}: {
  title: string;
  kind?: "kazumi" | "maccms";
  /** [lc-1769] 详情页竖版海报（代理 URL）——随播放写入观影记录，在线源条目不再缺封面 */
  poster?: string;
}) {
  const [groups, setGroups] = useState<WebrsGroup[] | null>(null); // null = 尚未搜索
  const [kw, setKw] = useState(title);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  // 源管理
  const [srcs, setSrcs] = useState<WebrsRuleInfo[]>([]);
  // [lc-1754] 当前上下文可见的源（按 kind 过滤；缺 kind 字段的老源视为 kazumi）
  const viewSrcs = useMemo(
    () => (kind ? srcs.filter((s) => (s.kind ?? "kazumi") === kind) : srcs),
    [srcs, kind],
  );
  // [lc-1755→1758] 勾选状态**按源类型**分家存储（而不是按视图）：kazumi 规则源永远读写
  // enabledSources key、采集源永远读写 maccmsEnabled key —— 同一个源在番剧页（全源视图）
  // 和 TMDB 页（采集源视图）的勾选状态一致，不会互相看不见/互相覆盖。
  const [enKaz, setEnKaz] = useState<string[] | null>(null); // null = 该类从未改过 → 全启用
  const [enMac, setEnMac] = useState<string[] | null>(null);
  // [lc-1775] 最近在线观看：**只显示当前条目**的 webrs 记录（标题+集+上次源+时间）
  const [recent, setRecent] = useState<
    { title: string; episode: string; sourceName: string; playedAt?: number }[]
  >([]);
  const loadRecent = useCallback(async () => {
    try {
      const r = await invoke<{
        items?: { title?: string; episode?: string; playedAt?: number; source?: string; sourceName?: string }[];
      }>("get_watch_history");
      const list = (Array.isArray(r?.items) ? r.items : [])
        .filter(
          (x) =>
            x &&
            x.source === "webrs" &&
            String(x.title ?? "").trim() === title.trim(),
        )
        .slice(0, 5);
      setRecent(
        list.map((x) => ({
          title: String(x.title),
          episode: String(x.episode ?? ""),
          sourceName: String(x.sourceName ?? ""),
          playedAt: x.playedAt,
        })),
      );
    } catch {
      /* 记录是锦上添花，失败静默 */
    }
  }, [title]);
  const readEnabledKey = (key: string): string[] | null => {
    try {
      const raw = localStorage.getItem(key);
      if (raw == null) return null;
      const v = JSON.parse(raw);
      return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
    } catch {
      return null;
    }
  };
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [addText, setAddText] = useState("");
  const [addErr, setAddErr] = useState("");
  // [lc-1752] 添加采集源（苹果CMS V10）：名称 + 采集接口地址，拼成 delimited 规则走 add_rule
  const [macOpen, setMacOpen] = useState(false);
  const [macName, setMacName] = useState("");
  const [macApi, setMacApi] = useState("");
  const [macReferer, setMacReferer] = useState("");
  const [macBusy, setMacBusy] = useState(false);
  const [macErr, setMacErr] = useState("");
  // [lc-1503] 一键测试源：对全部源各发一次真实搜索，行内显示 ✓ n / 无结果 / ✗
  const [tests, setTests] = useState<Record<string, { status: string; count: number; error: string }>>({});
  const [testing, setTesting] = useState(false);
  // [lc-1608] 在线规则目录：目录列表 + 可更新项 + 检查进度 + 行内安装/更新忙态
  const [catalog, setCatalog] = useState<WebrsCatalogItem[] | null>(null);
  // [lc-1757] 在线采集源目录（TMDB 剧集侧）
  const [mcat, setMcat] = useState<MaccmsCatItem[] | null>(null);
  const [mcatBusy, setMcatBusy] = useState(false);
  const [mcatMsg, setMcatMsg] = useState<{ kind: "ok" | "err" | "note"; text: string } | null>(null);
  const [mcatBusyName, setMcatBusyName] = useState("");
  const [updates, setUpdates] = useState<WebrsUpdateItem[]>([]);
  const [catBusy, setCatBusy] = useState(false);
  const [catMsg, setCatMsg] = useState<{ kind: "ok" | "err" | "note"; text: string } | null>(null);
  const [rowBusy, setRowBusy] = useState<Record<string, "install" | "update">>({});
  // 二级视图：选集
  const [sel, setSel] = useState<{ rule: string; item: WebrsSearchItem } | null>(null);
  const [roads, setRoads] = useState<WebrsRoad[]>([]);
  const [roadIdx, setRoadIdx] = useState(0);
  const [subBusy, setSubBusy] = useState("");

  // [lc-1606] 反爬验证：type1 的验证码图 + 输入；captchaMsg 是整体状态反馈
  const [captcha, setCaptcha] = useState<{ rule: string; image: string } | null>(null);
  const [captchaMsg, setCaptchaMsg] = useState<{ rule: string; status: "loading" | "verified" | "failed"; error: string } | null>(null);
  const [code, setCode] = useState("");
  // 同一源避免自动重复发起（验证成功后重搜若仍挑战 → 最多再试 2 次，防死循环）
  const captchaAttempts = useRef<Record<string, number>>({});
  const captchaBusyRef = useRef(false);

  // [lc-1747] 嗅探进度（webrs-sniff-status）：sniffTask 登记「当前在等的那次直链解析」，
  // 事件按 rule/url 匹配（Rust 侧 join 过 base_url，回传的是绝对地址，条目 URL 可能是
  // 相对路径 → 用前后缀兜底匹配）；lastSniffUrl 记每源最近一次嗅探页面 URL，
  // 供「打开网页手动验证」兜底取 URL（搜索 URL 模板不暴露给前端，同站页面等效）。
  const sniffTask = useRef<{ rule: string; url: string } | null>(null);
  const lastSniffUrl = useRef<Record<string, string>>({});
  // 解析中标志：比 subBusy 文本前缀可靠（嗅探进度会实时改写 subBusy，前缀判断会漏）
  const [resolving, setResolving] = useState(false);
  // [lc-1747] 手动验证兜底：webrs_captcha_visible 已请求打开可见窗（loading 行换文案）
  const [manualVerify, setManualVerify] = useState<{ rule: string } | null>(null);
  // [lc-1748] 最近一次 webrs 起播时间（ms）：mpv-end 只在该窗口内才归因到在线源播放，
  // 避免把库内播放的失败误报到本面板。
  const lastPlayAt = useRef(0);

  const kindOf = useCallback(
    (name: string): "kazumi" | "maccms" =>
      srcs.find((r) => r.name === name)?.kind ?? "kazumi",
    [srcs],
  );
  const isEnabled = useCallback(
    (name: string) => {
      const list = kindOf(name) === "maccms" ? enMac : enKaz;
      return list == null || list.includes(name);
    },
    [kindOf, enKaz, enMac],
  );
  /** 写某一类的勾选快照（null = 恢复「全启用」，不落盘）。 */
  const writeEnabled = (isMac: boolean, next: string[] | null) => {
    if (isMac) setEnMac(next);
    else setEnKaz(next);
    if (next != null) {
      localStorage.setItem(
        isMac ? WEBRS_MACCMS_KEY : WEBRS_ENABLED_KEY,
        JSON.stringify(next),
      );
    }
  };
  const enabledNames = useMemo(() => viewSrcs.filter((r) => isEnabled(r.name)).map((r) => r.name), [viewSrcs, isEnabled]);

  const refreshRules = useCallback(async () => {
    try {
      const r = await invoke<WebrsRuleInfo[]>("webrs_rules");
      setSrcs(r);
      return r;
    } catch (e) {
      setErr(String(e));
      return [];
    }
  }, []);

  // [lc-1508] 引导提示（中性样式，区别于真错误 err）：如"还没勾选任何源"
  const [srcHint, setSrcHint] = useState("");
  // [lc-1513] 二级视图（选集）返回时先播退出动画再卸载
  const [subMounted, subClosing] = useDelayedUnmount(!!sel);

  // [lc-1606] 发起反爬验证（Rust 建隐藏窗加载搜索页，按 captchaType 自动/交互完成）
  const startCaptcha = useCallback(async (rule: string, keyword: string) => {
    // [lc-1624] 同一时刻只允许一个验证会话：已在验证中就不要再起一个
    // （否则第二个会立刻把第一个顶掉，用户看到「失败 · 已取消」）。
    if (captchaBusyRef.current) return;
    captchaBusyRef.current = true;
    setCaptcha(null);
    setCode("");
    setCaptchaMsg({ rule, status: "loading", error: "" });
    try {
      await invoke("webrs_captcha_start", { ruleName: rule, keyword });
    } catch (e) {
      captchaBusyRef.current = false;
      setCaptchaMsg({ rule, status: "failed", error: String(e) });
    }
  }, []);

  const submitCaptcha = useCallback(async () => {
    if (!captcha) return;
    const c = code.trim();
    if (!c) return;
    try {
      await invoke("webrs_captcha_submit", { ruleName: captcha.rule, code: c });
      setCode("");
      setCaptcha(null);
      setCaptchaMsg({ rule: captcha.rule, status: "loading", error: "" });
    } catch (e) {
      setCaptchaMsg({ rule: captcha.rule, status: "failed", error: String(e) });
    }
  }, [captcha, code]);

  const cancelCaptcha = useCallback(async () => {
    captchaBusyRef.current = false;
    setCaptcha(null);
    setCode("");
    setCaptchaMsg(null);
    setManualVerify(null);
    try {
      await invoke("webrs_captcha_cancel");
    } catch {
      /* 忽略：窗口可能已自行销毁 */
    }
  }, []);

  // [lc-1747] 手动验证兜底用的 URL：优先该源最近一次嗅探上报的页面 URL（绝对地址），
  // 其次选集条目 URL（仅当本身已是绝对地址 —— webrs_captcha_visible 不会代拼 base_url）。
  // 注：规则的搜索 URL 模板（@keyword）不暴露给前端，这里取同站页面等效过验
  // （站点级挑战如 Cloudflare 过一次全站生效，收割的是 cookie+UA）。
  const manualVerifyUrlFor = useCallback(
    (rule: string): string | null => {
      const sniffed = lastSniffUrl.current[rule];
      if (sniffed && /^https?:\/\//i.test(sniffed)) return sniffed;
      if (sel?.rule === rule && /^https?:\/\//i.test(sel.item.url)) return sel.item.url;
      return null;
    },
    [sel],
  );

  // [lc-1747] 打开可见验证窗手动过验（隐藏窗自动验证兜不住的复杂挑战）：
  // 发起即返回；结果走现有 webrs-captcha-status（verified 后自动重搜，与自动验证同通道）。
  // 用户关窗 = cancelled（Rust 侧判定）。防重复：loading 期间 failed 行/弹窗不渲染按钮，
  // 且 captchaBusyRef 占住会话，「去验证」等自动流程不会再顶掉它。
  const openManualVerify = useCallback(
    async (rule: string) => {
      if (captchaBusyRef.current && manualVerify?.rule === rule) return;
      const url = manualVerifyUrlFor(rule);
      if (!url) return;
      captchaBusyRef.current = true;
      setCaptcha(null);
      setCode("");
      setManualVerify({ rule });
      setCaptchaMsg({ rule, status: "loading", error: "" });
      try {
        await invoke("webrs_captcha_visible", { ruleName: rule, url });
        // [lc-1747] 发起瞬间的「顶掉旧会话」会先发一条 cancelled（把上面设的态清掉），
        // flow 的 loading 事件又把 busy/captchaMsg 立回来 —— invoke 返回后会话已登记，
        // 这里统一重立「手动」标记，保证 loading 行文案正确（不依赖事件到达顺序）。
        captchaBusyRef.current = true;
        setManualVerify({ rule });
        setCaptchaMsg({ rule, status: "loading", error: "" });
      } catch (e) {
        captchaBusyRef.current = false;
        setManualVerify(null);
        setCaptchaMsg({ rule, status: "failed", error: String(e) });
      }
    },
    [manualVerify, manualVerifyUrlFor],
  );

  const doSearch = useCallback(
    async (k: string) => {
      const key = k.trim();
      if (!key) return;
      const picked = viewSrcs.filter((r) => isEnabled(r.name)).map((r) => r.name);
      if (picked.length === 0) {
        setSrcHint("还没有勾选任何源，点右侧 ⚙ 选择要用的源");
        return;
      }
      setSrcHint("");
      setBusy(true);
      setErr("");
      setSel(null);
      setRoads([]);
      try {
        const r = await invoke<WebrsGroup[]>("webrs_search_multi", { keyword: key, rules: picked });
        setGroups(r);
        // [lc-1622] 照 Kazumi「播放来源」：默认**全部展开**（不再只展开第一个）。
        // expanded 里没有某源的键 = 视为展开（见渲染处的 `expanded[name] !== false`）。
        setExpanded({});
        // [lc-1606] 有源需要人机验证 → 自动发起（一次一个，避免多隐藏窗）
        const need = r.filter((g) => g.status === "captcha");
        if (need.length > 0 && !captchaBusyRef.current) {
          const target = need.find((g) => (captchaAttempts.current[g.name] ?? 0) < 2) ?? need[0];
          const n = captchaAttempts.current[target.name] ?? 0;
          if (n < 2) {
            captchaAttempts.current[target.name] = n + 1;
            await startCaptcha(target.name, key);
          } else {
            setSrcHint(`源「${target.name}」需要人机验证（已尝试多次），可点该行「去验证」重试`);
          }
        }
      } catch (e) {
        setErr(String(e));
        setGroups([]);
      } finally {
        setBusy(false);
      }
    },
    [viewSrcs, isEnabled, startCaptcha],
  );

  // [lc-1606] 验证事件：type1 收到图 → 弹窗；status verified → 自动重搜
  const doSearchRef = useRef(doSearch);
  doSearchRef.current = doSearch;
  const kwRef = useRef(kw);
  kwRef.current = kw;
  useEffect(() => {
    let alive = true;
    let un1: (() => void) | undefined;
    let un2: (() => void) | undefined;
    listen<{ rule: string; image: string }>("webrs-captcha", (e) => {
      if (!alive) return;
      setCaptcha({ rule: e.payload.rule, image: e.payload.image });
    }).then((u) => {
      if (alive) un1 = u;
      else u();
    });
    listen<{ rule: string; status: string; error: string }>("webrs-captcha-status", (e) => {
      if (!alive) return;
      const p = e.payload;
      if (p.status === "loading") {
        // [lc-1747] 验证 flow 的起步信号（隐藏自动流 / 可见手动流都会发）：
        // 重立 busy + loading 态 —— 紧前面「被取代」旧会话的 cancelled 可能刚清过。
        // 手动流的专属文案由 openManualVerify 在 invoke 返回后统一重立（不依赖事件顺序）。
        captchaBusyRef.current = true;
        setCaptcha(null);
        setCaptchaMsg({ rule: p.rule, status: "loading", error: "" });
        return;
      }
      if (p.status === "verified") {
        captchaBusyRef.current = false;
        setCaptcha(null);
        setManualVerify(null);
        setCaptchaMsg({ rule: p.rule, status: "verified", error: "" });
        // 自动重试：cookie 已入 jar，重搜这次就能拿到结果
        doSearchRef.current(kwRef.current);
      } else if (p.status === "failed") {
        captchaBusyRef.current = false;
        setCaptcha(null);
        setManualVerify(null);
        setCaptchaMsg({ rule: p.rule, status: "failed", error: p.error });
      } else if (p.status === "cancelled") {
        // [lc-1624] 取消 / 被新会话顶掉**不是验证失败**：清态即可，不弹「验证失败」。
        // 后端已按原因分型（用户取消 / 被取代），这里一律静默收尾，避免误报。
        // [lc-1747] 若紧随其后的是手动验证流程的 loading 信号（Rust 先取消旧会话再起
        // 新流程、事件有序），下一步会把 loading / manualVerify 重新立起来。
        captchaBusyRef.current = false;
        setCaptcha(null);
        setManualVerify(null);
        setCaptchaMsg(null);
      }
    }).then((u) => {
      if (alive) un2 = u;
      else u();
    });
    return () => {
      alive = false;
      un1?.();
      un2?.();
    };
  }, []);

  // [lc-1747] 嗅探进度（webrs-sniff-status）：解析直链的 WebView 段进度，融进现有的
  // subBusy 行内提示（webrs-note），不另起新弹层。按 rule/url 匹配 sniffTask 登记
  // 的当前任务，无关事件忽略；任何状态都顺手记 lastSniffUrl（手动验证兜底取 URL 用）。
  useEffect(() => {
    let alive = true;
    let un: (() => void) | undefined;
    listen<SniffStatus>("webrs-sniff-status", (e) => {
      if (!alive) return;
      const p = e.payload;
      if (p.rule && /^https?:\/\//i.test(p.url)) lastSniffUrl.current[p.rule] = p.url;
      const task = sniffTask.current;
      // Rust 侧 join 过 base_url：事件 url 是绝对地址，任务登记的条目 url 可能是相对路径 → 前后缀兜底
      const matched =
        !!task &&
        task.rule === p.rule &&
        (task.url === p.url || p.url.endsWith(task.url) || task.url.endsWith(p.url));
      if (!matched) return;
      switch (p.status) {
        case "loading":
          setSubBusy("正在解析直链…");
          break;
        case "hooked":
          setSubBusy("已挂钩，等待命中…");
          break;
        case "hit":
          // 命中即收尾：后续提示由 playEp 接管（「正在拉起独立播放器…」），迟到的同任务事件忽略
          sniffTask.current = null;
          break;
        case "timeout":
          setSubBusy("解析超时，可稍后重试");
          break;
        case "failed":
          setSubBusy(p.message || "解析失败");
          break;
      }
    }).then((u) => {
      if (alive) un = u;
      else u();
    });
    return () => {
      alive = false;
      un?.();
    };
  }, []);

  // [lc-1748] 播放失败回显：mpv 的 end-file(error)（Rust 侧 emit mpv-end + OSD）。
  // 起播后 60s 内的失败才归因到本面板的在线源播放（mpv 会话是全局的，库内播放的
  // 失败不该报到这里）。之前 subBusy 停在「已在独立播放器窗口播放…」、真失败只在
  // mpv 窗口 OSD 上一闪而过 —— 用户报的「显示加载成功拉起播放器后显示播放失败」
  // 缺的正是这条行内回显。
  // [lc-1749] PotPlayer 是独立链路（无 mpv-end）：relay 转发失败/上游拒流时
  // Rust 侧 emit potplayer-relay-error，同一 60s 窗口、同一行内回显兜底。
  useEffect(() => {
    let alive = true;
    const unsubs: (() => void)[] = [];
    const onFail = (err: string | undefined, fallback: string) => {
      if (!alive) return;
      if (Date.now() - lastPlayAt.current > 60_000) return;
      lastPlayAt.current = 0;
      setSubBusy(`播放器报告播放失败：${err || fallback}`);
    };
    listen<{ reason?: string; error?: string }>("mpv-end", (e) => {
      onFail(e.payload?.error || e.payload?.reason, "未知错误");
    }).then((u) => {
      if (alive) unsubs.push(u);
      else u();
    });
    listen<{ url?: string; error?: string }>("potplayer-relay-error", (e) => {
      onFail(e.payload?.error, "PotPlayer 转发失败");
    }).then((u) => {
      if (alive) unsubs.push(u);
      else u();
    });
    return () => {
      alive = false;
      unsubs.forEach((u) => u());
    };
  }, []);

  // 进面板：拉源列表 + 自动按标题搜一次（只搜勾选的）
  const autoDone = useRef(false);
  useEffect(() => {
    if (autoDone.current) return;
    autoDone.current = true;
    loadRecent();
    const k = readEnabledKey(WEBRS_ENABLED_KEY);
    const m = readEnabledKey(WEBRS_MACCMS_KEY);
    setEnKaz(k);
    setEnMac(m);
    refreshRules().then((r) => {
      // [lc-1755] 自动首搜只看当前视图（kind 过滤后）的源；勾选按源类型各自判定
      const scope = kind ? r.filter((x) => (x.kind ?? "kazumi") === kind) : r;
      const picked = scope
        .filter((x) => {
          const list = (x.kind ?? "kazumi") === "maccms" ? m : k;
          return list == null || list.includes(x.name);
        })
        .map((x) => x.name);
      if (scope.length > 0 && picked.length > 0) doSearch(title);
      else
        setSrcHint(
          scope.length > 0
            ? "还没有勾选任何源，点右侧 ⚙ 选择要用的源"
            : kind === "maccms"
              ? "还没有可用采集源，点 ⚙ 用「＋ 采集源」添加"
              : "还没有可用规则源，点 ⚙ 添加或到规则目录安装",
        );
    });
  }, []);

  const toggleSource = (name: string) => {
    const isMac = kindOf(name) === "maccms";
    const list = isMac ? enMac : enKaz;
    // null（从未改过）→ 快照该类型当前全部源名再切换，与旧语义一致
    const cur =
      list ?? srcs.filter((r) => (r.kind ?? "kazumi") === (isMac ? "maccms" : "kazumi")).map((r) => r.name);
    const next = cur.includes(name) ? cur.filter((x) => x !== name) : [...cur, name];
    writeEnabled(isMac, next);
  };
  const setAllSources = (on: boolean) => {
    // [lc-1754/1758] 全选/清空只作用于当前视图（kind 过滤后）的源，且按源类型分别写
    // 各自的存储 —— 当前视图没有某类源时该类的勾选完全不动。
    for (const t of ["kazumi", "maccms"] as const) {
      const viewNames = new Set(viewSrcs.filter((r) => (r.kind ?? "kazumi") === t).map((r) => r.name));
      if (viewNames.size === 0) continue;
      const list = t === "maccms" ? enMac : enKaz;
      const cur = list ?? srcs.filter((r) => (r.kind ?? "kazumi") === t).map((r) => r.name);
      const kept = cur.filter((x) => !viewNames.has(x));
      const next = on ? Array.from(new Set([...kept, ...viewNames])) : kept;
      writeEnabled(t === "maccms", next);
    }
  };

  const openChapters = async (rule: string, item: WebrsSearchItem) => {
    setSel({ rule, item });
    setRoads([]);
    setRoadIdx(0);
    setSubBusy("拉取选集…");
    try {
      const r = await invoke<{ roads: WebrsRoad[] }>("webrs_chapters", {
        ruleName: rule,
        url: item.url,
      });
      setRoads(r.roads ?? []);
    } catch (e) {
      setErr(String(e));
      setSel(null);
    } finally {
      setSubBusy("");
    }
  };

  const playEp = async (ep: WebrsSearchItem) => {
    if (!sel || resolving) return;
    // [lc-1747] 登记当前解析任务：嗅探进度事件（webrs-sniff-status）按 rule/url 匹配它，
    // 只把本次解析的进度写进 subBusy；hit / 出错 / 播放启动后任务即结束。
    sniffTask.current = { rule: sel.rule, url: ep.url };
    setResolving(true);
    setErr("");
    setSubBusy("解析视频直链…（页面藏得深时拉起嗅探，最多约 20s）");
    try {
      const r = await invoke<{ url: string; referer: string; ua: string; via: string }>(
        "webrs_resolve",
        { ruleName: sel.rule, url: ep.url },
      );
      sniffTask.current = null;
      setSubBusy("正在拉起独立播放器…");
      // [lc-1615] 附观影记录元数据（番剧名/集名/源名/集页 URL）—— Rust 侧落本地历史用，不影响播放
      await invoke("webrs_play", {
        url: r.url,
        referer: r.referer,
        ua: r.ua,
        title: sel.item.name,
        episode: ep.name,
        ruleName: sel.rule,
        episodeUrl: ep.url,
        poster: poster || undefined,
      });
      // [lc-1771] 诊断：观影记录封面链路（poster 是否传到播放命令）
      console.debug("[webrs] play 记录封面:", poster ? `${poster.slice(0, 48)}…` : "(空)");
      // [lc-1775] 记一笔后刷新「最近在线观看」
      loadRecent();
      // [lc-1748] 记起播时间：此后 60s 内的 mpv-end(error) 归因到本次在线源播放
      lastPlayAt.current = Date.now();
      setSubBusy(`已在独立播放器窗口播放 · ${sel.item.name} · ${ep.name}（${r.via === "webview" ? "嗅探直链" : "页面直解"}）`);
    } catch (e) {
      setErr(String(e));
      setSubBusy("");
    } finally {
      sniffTask.current = null;
      setResolving(false);
    }
  };

  const addRule = async () => {
    setAddErr("");
    try {
      const name = await invoke<string>("webrs_add_rule", { jsonText: addText });
      setAddOpen(false);
      setAddText("");
      await refreshRules();
      setErr("");
      setSrcHint(`已添加源「${name}」，记得在列表里勾选它`);
    } catch (e) {
      setAddErr(String(e));
    }
  };

  // [lc-1752] 添加采集源：把「名称 + 苹果CMS 采集接口地址」拼成 delimited 规则 JSON，
  // 走现有 webrs_add_rule 入库；搜索 ?ac=videolist&wd=、选集 ?ac=detail&ids=、
  // vod_play_url 直链（后端 delimited 拆分 + 直链短路免嗅探）。
  const addMacSource = async () => {
    setMacErr("");
    const name = macName.trim();
    const api = macApi.trim();
    const referer = macReferer.trim();
    if (!name) {
      setMacErr("请填写源名称");
      return;
    }
    if (!/^https?:\/\//i.test(api)) {
      setMacErr("采集接口必须以 http:// 或 https:// 开头");
      return;
    }
    let base: string;
    try {
      base = new URL(api).origin;
    } catch {
      setMacErr("采集接口地址无效");
      return;
    }
    if (referer && !/^https?:\/\//i.test(referer)) {
      setMacErr("Referer 必须以 http:// 或 https:// 开头");
      return;
    }
    const rule = {
      name,
      version: "1.0.0",
      baseURL: base,
      ...(referer ? { referer } : {}),
      searchMode: "api",
      searchApiConfig: {
        request: { method: "GET", url: api, query: { ac: "videolist", wd: "@keyword" } },
        listPath: "$.list[*]",
        namePath: "$.vod_name",
        sourcePath: "$.vod_id",
      },
      chapterMode: "api",
      chapterApiConfig: {
        request: { method: "GET", url: api, query: { ac: "detail", ids: "@source" } },
        format: "delimited",
        roadNamesPath: "$.list[*].vod_play_from",
        roadEpisodesPath: "$.list[*].vod_play_url",
      },
    };
    setMacBusy(true);
    try {
      await invoke<string>("webrs_add_rule", { jsonText: JSON.stringify(rule, null, 2) });
      setMacOpen(false);
      setMacName("");
      setMacApi("");
      setMacReferer("");
      await refreshRules();
      setErr("");
      setSrcHint(`已添加采集源「${name}」，记得在列表里勾选它`);
    } catch (e) {
      setMacErr(String(e));
    } finally {
      setMacBusy(false);
    }
  };

  // [lc-1775] title 变化（面板复用）时重拉当前条目的观看记录
  useEffect(() => { loadRecent(); }, [loadRecent]);

  // [lc-1503] 一键测试：全部源（不管勾没勾）各跑一次搜索，结果挂到每行徽标。
  // 测试词用搜索框关键词（空则用通用词「少女」——各站收录差异最小的探针）。
  const testAll = async () => {
    if (testing) return;
    const key = kw.trim() || "少女";
    setTesting(true);
    setAddErr("");
    const loading: Record<string, { status: string; count: number; error: string }> = {};
    for (const r of viewSrcs) loading[r.name] = { status: "loading", count: 0, error: "" };
    setTests(loading);
    try {
      const r = await invoke<WebrsGroup[]>("webrs_search_multi", {
        keyword: key,
        rules: viewSrcs.map((s) => s.name),
      });
      const next: Record<string, { status: string; count: number; error: string }> = {};
      for (const g of r) next[g.name] = { status: g.status, count: g.items.length, error: g.error };
      setTests(next);
    } catch (e) {
      setAddErr(String(e));
      setTests({});
    } finally {
      setTesting(false);
    }
  };

  const removeRule = async (name: string) => {
    if (!window.confirm(`删除源「${name}」？（内置源会在下次启动时恢复）`)) return;
    try {
      await invoke("webrs_remove_rule", { ruleName: name });
      await refreshRules();
      const isMac = kindOf(name) === "maccms";
      const list = isMac ? enMac : enKaz;
      if (list != null) writeEnabled(isMac, list.filter((x) => x !== name));
      setGroups((g) => (g ? g.filter((x) => x.name !== name) : g));
    } catch (e) {
      setErr(String(e));
    }
  };

  // ───────────── [lc-1608] 在线规则目录：检查 / 安装 / 更新 / 全部更新 ─────────────
  // 本地版本表（小写名 → 版本），用于给目录行打「未装 / 已装 vX / 可更新」状态。
  const localVer = useMemo(() => {
    const m: Record<string, string> = {};
    for (const r of srcs) if (r.version) m[r.name.toLowerCase()] = r.version;
    return m;
  }, [srcs]);
  const updateMap = useMemo(() => {
    const m: Record<string, WebrsUpdateItem> = {};
    for (const u of updates) m[u.name.toLowerCase()] = u;
    return m;
  }, [updates]);

  // ───────── [lc-1757] 在线采集源目录：检查 / 安装 / 更新 ─────────
  const checkMCatalog = useCallback(
    async (force: boolean) => {
      if (mcatBusy) return;
      setMcatBusy(true);
      setMcatMsg({ kind: "note", text: force ? "正在刷新在线采集源…" : "正在读取在线采集源…" });
      try {
        const c = await invoke<{ items: MaccmsCatItem[] }>("webrs_maccms_catalog", { force });
        setMcat(c.items ?? []);
        const up = (c.items ?? []).filter((x) => x.updatable).length;
        const miss = (c.items ?? []).filter((x) => !x.installed).length;
        setMcatMsg({
          kind: "ok",
          text: `目录 ${c.items?.length ?? 0} 条采集源：${miss} 条未安装、${up} 条可更新`,
        });
      } catch (e) {
        setMcatMsg({ kind: "err", text: String(e) });
      } finally {
        setMcatBusy(false);
      }
    },
    [mcatBusy],
  );

  const installMaccms = useCallback(
    async (name: string) => {
      if (mcatBusyName) return;
      setMcatBusyName(name);
      try {
        await invoke("webrs_maccms_install", { name });
        await refreshRules();
        await checkMCatalog(true);
      } catch (e) {
        setMcatMsg({ kind: "err", text: String(e) });
      } finally {
        setMcatBusyName("");
      }
    },
    [mcatBusyName, refreshRules, checkMCatalog],
  );

  const checkUpdates = useCallback(async (force: boolean) => {
    if (catBusy) return;
    setCatBusy(true);
    setCatMsg({ kind: "note", text: force ? "正在刷新在线规则目录…" : "正在读取在线规则目录…" });
    try {
      const c = await invoke<{ items: WebrsCatalogItem[] }>("webrs_catalog", { force });
      setCatalog(c.items ?? []);
      // 目录已（内存）缓存，第二次调用很快；可更新判据在 Rust（Kazumi _remoteIsNewer 口径）
      const u = await invoke<{ items: WebrsUpdateItem[] }>("webrs_check_updates");
      setUpdates(u.items ?? []);
      setCatMsg({
        kind: "ok",
        text: `目录 ${c.items?.length ?? 0} 条规则，${u.items?.length ?? 0} 条可更新`,
      });
    } catch (e) {
      setCatMsg({ kind: "err", text: String(e) });
    } finally {
      setCatBusy(false);
    }
  }, [catBusy]);

  // 安装 / 更新单条。串行由调用方保证（行内按钮天然一次一个）。
  const installOrUpdate = useCallback(
    async (name: string, kind: "install" | "update") => {
      setRowBusy((m) => ({ ...m, [name]: kind }));
      setCatMsg({ kind: "note", text: `${kind === "install" ? "安装" : "更新"}「${name}」…` });
      try {
        const r = await invoke<{ name: string; version: string }>(
          kind === "install" ? "webrs_install_rule" : "webrs_update_rule",
          { name },
        );
        await refreshRules();
        const u = await invoke<{ items: WebrsUpdateItem[] }>("webrs_check_updates");
        setUpdates(u.items ?? []);
        setCatMsg({ kind: "ok", text: `已${kind === "install" ? "安装" : "更新"}「${r.name}」v${r.version}` });
      } catch (e) {
        setCatMsg({ kind: "err", text: `${kind === "install" ? "安装" : "更新"}「${name}」失败：${e}` });
      } finally {
        setRowBusy((m) => {
          const n = { ...m };
          delete n[name];
          return n;
        });
      }
    },
    [refreshRules],
  );

  // 全部更新：**串行**逐个调用（不并发轰炸上游），带 i/n 进度。
  const updateAll = useCallback(async () => {
    if (catBusy) return;
    const targets = updates.map((u) => u.name);
    if (targets.length === 0) {
      setCatMsg({ kind: "note", text: "没有可更新的规则" });
      return;
    }
    setCatBusy(true);
    let ok = 0;
    let fail = 0;
    for (let i = 0; i < targets.length; i++) {
      const name = targets[i];
      setRowBusy((m) => ({ ...m, [name]: "update" }));
      setCatMsg({ kind: "note", text: `更新中 ${i + 1}/${targets.length}：「${name}」…` });
      try {
        await invoke("webrs_update_rule", { name });
        ok++;
      } catch {
        fail++;
      } finally {
        setRowBusy((m) => {
          const n = { ...m };
          delete n[name];
          return n;
        });
      }
    }
    await refreshRules();
    try {
      const u = await invoke<{ items: WebrsUpdateItem[] }>("webrs_check_updates");
      setUpdates(u.items ?? []);
    } catch {
      /* 忽略：至少把已更新的结果报出去 */
    }
    setCatBusy(false);
    setCatMsg({ kind: fail ? "err" : "ok", text: `全部更新完成：成功 ${ok} 条${fail ? `，失败 ${fail} 条` : ""}` });
  }, [catBusy, updates, refreshRules]);

  // [lc-1622] 每个源一行状态（照 Kazumi source_sheet_view 的 statusLabel）：
  // ✅ 可直接播放 / 🔒 需要验证 / ⚪ 无结果 / ✗ 检索失败。
  const statusMeta = (g: WebrsGroup): { icon: string; label: string; cls: string } => {
    switch (g.status) {
      case "ok":
        // [lc-1747] via:"harvest" = 验证通过后收割页缓存的结果（本次没发网络请求）
        return { icon: "✅", label: `可直接播放 · ${g.items.length} 个结果${g.via === "harvest" ? "（验证缓存）" : ""}`, cls: "ok" };
      case "captcha":
        return { icon: "🔒", label: "需要验证", cls: "captcha" };
      case "empty":
        return { icon: "⚪", label: "无结果", cls: "empty" };
      default:
        return { icon: "✗", label: "检索失败", cls: "error" };
    }
  };

  return (
    <div className="webrs">
      {/* [lc-1609] 合规：常驻提示 —— 在线资源全部来自第三方网站，本软件不提供/不存储任何资源 */}
      <div className="webrs-hint">
        资源来自第三方网站，本软件不提供、不存储任何资源 · 详见「设置 → 关于 → 免责声明」
      </div>
      <div className="webrs-bar">
        <input
          value={kw}
          onChange={(e) => setKw(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !busy && doSearch(kw)}
          placeholder="搜索资源关键词"
        />
        <button className="webrs-go" disabled={busy} onClick={() => doSearch(kw)}>
          {busy ? "搜索中" : "搜索"}
        </button>
        {/* [lc-1775] 本条目的在线观看记录（最近几集 + 上次源 + 时间），点击重搜续看 */}
        {recent.length > 0 && (
          <div className="webrs-recent">
            <div className="webrs-recent-head">
              <span className="webrs-recent-title">上次观看</span>
              <span className="webrs-recent-sub">共 {recent.length} 条在线观看记录</span>
            </div>
            <div className="webrs-recent-list">
              {recent.map((r) => (
                <button
                  key={(r.playedAt ?? 0) + r.episode}
                  className="webrs-recent-chip"
                  title={`${r.episode || r.title} · ${r.sourceName || "在线源"} · ${agoOf(r.playedAt)}`}
                  onClick={() => {
                    setKw(r.title);
                    doSearch(r.title);
                  }}
                >
                  <span className="webrs-recent-ep">{r.episode || r.title}</span>
                  {r.sourceName && <span className="webrs-recent-src">{r.sourceName}</span>}
                  <span className="webrs-recent-ago">{agoOf(r.playedAt)}</span>
                </button>
              ))}
            </div>
          </div>
        )}
        <button
          className="webrs-gear"
          title={`源管理（已勾选 ${enabledNames.length}/${viewSrcs.length}）`}
          onClick={() => {
            setSettingsOpen(true);
            setAddErr("");
          }}
        >
          ⚙
        </button>
      </div>
      {busy && <div className="webrs-note">正在搜索 {enabledNames.length} 个已勾选源…</div>}
      {err && (
        <div className="webrs-err">
          {err}
          {/* [lc-1747] 选集视图里解析/嗅探失败 → 手动验证兜底入口（同站点挑战过一次全站生效） */}
          {sel && (
            <ManualVerifyButton rule={sel.rule} urlFor={manualVerifyUrlFor} onOpen={openManualVerify} />
          )}
        </div>
      )}
      {srcHint && !err && <div className="webrs-hint">{srcHint}</div>}
      {subBusy && <div className="webrs-note">{subBusy}</div>}
      {/* [lc-1606] 反爬验证状态 */}
      {captchaMsg?.status === "loading" && (
        <div className="webrs-note">
          {manualVerify?.rule === captchaMsg.rule
            ? `已打开验证窗口 · ${captchaMsg.rule} —— 请在弹出的网页里手动完成验证，过完自动继续…`
            : `人机验证 · ${captchaMsg.rule} —— 正在等待网站响应…`}
        </div>
      )}
      {captchaMsg?.status === "verified" && (
        <div className="webrs-captcha-ok">✓ 验证通过 · {captchaMsg.rule} —— 正在重新检索…</div>
      )}
      {captchaMsg?.status === "failed" && (
        <div className="webrs-err webrs-err-acts">
          <span>
            人机验证失败 · {captchaMsg.rule}：{captchaMsg.error}
          </span>
          <ManualVerifyButton rule={captchaMsg.rule} urlFor={manualVerifyUrlFor} onOpen={openManualVerify} />
        </div>
      )}

      {/* 二级视图：选集（返回时播退出动画；roads 在动画期间保留，内容不闪空） */}
      {(sel || subMounted) && (
        <div className={`webrs-sub${subClosing ? " view-out" : " view-in"}`}>
          <div className="webrs-crumbs">
            <span className="webrs-cur">{sel?.item.name ?? ""}</span>
            <span className="webrs-src">来源：{sel?.rule ?? ""}</span>
            <button
              className="webrs-back"
              onClick={() => setSel(null)}
              title="返回搜索结果"
            >
              ‹ 返回结果
            </button>
          </div>
          {roads.length > 1 && (
            <div className="webrs-roads">
              {roads.map((rd, i) => (
                <button key={`${rd.name}-${i}`} className={i === roadIdx ? "on" : ""} onClick={() => setRoadIdx(i)}>
                  {rd.name}
                </button>
              ))}
            </div>
          )}
          <div className="webrs-eps">
            {(roads[roadIdx]?.episodes ?? []).map((ep) => (
              <button key={ep.url} disabled={resolving} onClick={() => playEp(ep)} title={ep.url}>
                {ep.name}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* 一级视图：按源分组的结果。照 Kazumi「播放来源」——**默认全部展开**，
          每源一行（名称 + 状态 + 展开/收起），展开即该源全部结果，点结果直接进线路/选集。
          [lc-1622] 状态用图标 + 文案（✅ 可直接播放 / 🔒 需要验证 / ⚪ 无结果 / ✗ 检索失败）。 */}
      {!sel && groups !== null && (
        <div className="webrs-groups">
          {groups.map((g) => {
            const meta = statusMeta(g);
            // 缺键 = 默认展开（doSearch 里 setExpanded({}) 即「全展开」）
            const open = expanded[g.name] !== false;
            return (
              <div key={g.name} className={`webrs-g ${meta.cls}`}>
                <div className="webrs-g-hd">
                  <button
                    className="webrs-g-toggle"
                    onClick={() => setExpanded((m) => ({ ...m, [g.name]: !open }))}
                    title={g.error || g.name}
                    aria-expanded={open}
                  >
                    <span className="webrs-g-st" aria-hidden>
                      {meta.icon}
                    </span>
                    <span className="webrs-g-name">{g.name}</span>
                    <span className={`webrs-g-badge ${meta.cls}`}>{meta.label}</span>
                    <span className="webrs-g-arrow">{open ? "⌃" : "⌄"}</span>
                  </button>
                  {g.status === "captcha" && (
                    <button
                      className="webrs-g-verify"
                      onClick={() => startCaptcha(g.name, kw)}
                      title="重新发起人机验证"
                    >
                      去验证
                    </button>
                  )}
                </div>
                {open && g.status === "ok" && (
                  <div className="webrs-g-items">
                    {g.items.map((it) => (
                      <button key={it.url} onClick={() => openChapters(g.name, it)} title={it.url}>
                        <span className="webrs-g-ep">{it.name}</span>
                        <span className="webrs-g-play" aria-hidden>
                          ▶
                        </span>
                      </button>
                    ))}
                  </div>
                )}
                {open && g.status === "captcha" && (
                  <div className="webrs-g-tip">需要完成网站验证后才能检索 —— 点右上「去验证」。</div>
                )}
                {open && g.status === "empty" && <div className="webrs-g-tip">没有找到匹配结果。</div>}
                {open && g.status === "error" && (
                  <div className="webrs-g-tip">{g.error || "检索失败。"}</div>
                )}
              </div>
            );
          })}
          {groups.every((g) => g.status !== "ok") && (
            <div className="webrs-note">勾选的源都没有结果：换个关键词、换个源，或 ⚙ 里添加规则</div>
          )}
        </div>
      )}

      {/* ⚙ 源管理设置面板。portal 挂 body：弹窗留在 .content 里会被 sticky 的
          .dseg（z30 + backdrop-filter）合成穿透 —— 玻璃层画到遮罩上面成"白条"
          （[lc-1504] 用户截图实锤）。portal 后与 .dseg 同在顶层上下文，
          mask z40 > dseg z30 正常压制；顶栏 z50 仍在其上，窗口按钮不受影响。 */}
      {settingsOpen &&
        createPortal(
          <div className="ddet-mask" onClick={() => setSettingsOpen(false)}>
          <div className="sum-modal webrs-add-modal" onClick={(e) => e.stopPropagation()}>
            <div className="sum-modal-h">
                <h3>
                  {kind === "maccms" ? "采集源管理" : kind === "kazumi" ? "规则源管理" : "源管理"}{" "}
                  <span className="webrs-mgr-count">已勾选 {enabledNames.length} / {viewSrcs.length}</span>
                </h3>
              <button className="ddet-close sum-modal-x" onClick={() => setSettingsOpen(false)}>
                ×
              </button>
            </div>
            <div className="webrs-mgr-body">
              <div className="webrs-mgr-acts">
                <button onClick={testAll} disabled={testing} title="对全部源各发一次真实搜索，检验源是否可用">
                  {testing ? "测试中…" : "⚡ 一键测试"}
                </button>
                <button onClick={() => setAllSources(true)}>全选</button>
                <button onClick={() => setAllSources(false)}>清空</button>
                {kind !== "maccms" && (
                  <button className="webrs-mgr-addbtn" onClick={() => setAddOpen((v) => !v)}>
                    {addOpen ? "收起" : "＋ 添加源"}
                  </button>
                )}
                {/* [lc-1752/1758] 苹果CMS 采集源：只填一个接口地址，自动生成 delimited 规则。
                    采集源视图与全源视图（番剧页）都显示。 */}
                {kind !== "kazumi" && (
                  <button className="webrs-mgr-addbtn" onClick={() => setMacOpen((v) => !v)}>
                    {macOpen ? "收起" : "＋ 采集源"}
                  </button>
                )}
              </div>
            {macOpen && (
              <div className="webrs-mac-form">
                <div className="webrs-mac-hint">
                  苹果CMS V10 采集接口（剧集/电影资源站），地址形如 https://xxx/api.php/provide/vod
                  —— 只填地址即可，搜索与选集自动按标准协议生成。
                </div>
                <input
                  className="webrs-mac-input"
                  value={macName}
                  onChange={(e) => setMacName(e.target.value)}
                  placeholder="源名称（如：量子资源）"
                />
                <input
                  className="webrs-mac-input"
                  value={macApi}
                  onChange={(e) => setMacApi(e.target.value)}
                  placeholder="采集接口地址 https://xxx/api.php/provide/vod"
                />
                <input
                  className="webrs-mac-input"
                  value={macReferer}
                  onChange={(e) => setMacReferer(e.target.value)}
                  placeholder="Referer（可选，一般留空）"
                />
                {macErr && <div className="webrs-err">{macErr}</div>}
                <div className="webrs-add-acts">
                  <button
                    className="webrs-go"
                    disabled={macBusy || !macName.trim() || !macApi.trim()}
                    onClick={addMacSource}
                  >
                    {macBusy ? "添加中…" : "添加"}
                  </button>
                </div>
              </div>
            )}
            {addOpen && (
              <>
                <textarea
                  className="webrs-add-ta"
                  value={addText}
                  onChange={(e) => setAddText(e.target.value)}
                  placeholder="粘贴 Kazumi 规则 JSON（可从 KazumiRules 仓库复制规则文件全文），须包含 name 字段"
                  rows={10}
                />
                {addErr && <div className="webrs-err">{addErr}</div>}
                <div className="webrs-add-acts">
                  <button className="webrs-go" disabled={!addText.trim()} onClick={addRule}>
                    导入
                  </button>
                </div>
              </>
            )}
            <div className="webrs-mgr-list">
              {viewSrcs.map((r) => {
                const t = tests[r.name];
                return (
                  <div key={r.name} className={`webrs-mgr-row${isEnabled(r.name) ? " on" : ""}`}>
                    <label className="webrs-mgr-main">
                      <input type="checkbox" checked={isEnabled(r.name)} onChange={() => toggleSource(r.name)} />
                      <span className="webrs-mgr-name">{r.name}</span>
                      {r.mode === "api" && <span className="webrs-mgr-mode">API</span>}
                    </label>
                    {t && (
                      <span
                        className={`webrs-mgr-test ${t.status}`}
                        title={t.error || (t.status === "ok" ? `搜索正常，${t.count} 条结果` : "")}
                      >
                        {t.status === "loading"
                          ? "…"
                          : t.status === "ok"
                            ? `✓ ${t.count} 条`
                            : t.status === "empty"
                              ? "无结果"
                              : "✗ 失败"}
                      </span>
                    )}
                    <button className="webrs-mgr-x" title={`删除源 ${r.name}`} onClick={() => removeRule(r.name)}>
                      ×
                    </button>
                  </div>
                );
              })}
              {viewSrcs.length === 0 && (
                <div className="webrs-note">
                  {kind === "maccms"
                    ? "没有可用采集源 —— 「＋ 采集源」填一个苹果CMS 接口地址即可"
                    : "没有可用规则源 —— 「＋ 添加源」导入 Kazumi 规则，或到「规则目录」安装"}
                </div>
              )}
              </div>

              {/* [lc-1757/1758] 在线采集源目录（上游 = AniBox 仓库 docs/maccms-catalog/）：
                  检查更新 / 逐条安装·更新。采集源视图与全源视图（番剧页）都显示。 */}
              {kind !== "kazumi" && (
              <div className="webrs-cat">
                <div className="webrs-cat-hd">
                  <span className="webrs-cat-title">在线采集源</span>
                  <button onClick={() => checkMCatalog(true)} disabled={mcatBusy}>
                    {mcatBusy ? "检查中…" : "检查更新"}
                  </button>
                </div>
                {mcatMsg && (
                  <div className={`webrs-cat-msg ${mcatMsg.kind === "err" ? "err" : mcatMsg.kind === "ok" ? "ok" : ""}`}>
                    {mcatMsg.text}
                  </div>
                )}
                {mcat && (
                  <div className="webrs-cat-list">
                    {mcat.map((x) => {
                      const state = !x.installed ? "new" : x.updatable ? "update" : "installed";
                      return (
                        <div key={x.name} className="webrs-cat-row">
                          <span className="webrs-cat-name" title={x.api}>{x.name}</span>
                          <span className="webrs-cat-ver">v{x.version}</span>
                          <span className={`webrs-cat-state ${state}`}>
                            {state === "new"
                              ? "未装"
                              : state === "update"
                                ? `可更新 v${x.localVersion} → v${x.version}`
                                : `已装 v${x.localVersion || x.version}`}
                          </span>
                          {state !== "installed" && (
                            <button
                              className="webrs-cat-act"
                              disabled={mcatBusyName === x.name || mcatBusy}
                              onClick={() => installMaccms(x.name)}
                            >
                              {mcatBusyName === x.name ? "…" : state === "new" ? "安装" : "更新"}
                            </button>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}
                {!mcat && !mcatBusy && (
                  <div className="webrs-note">点「检查更新」拉取在线采集源目录（带 6 小时缓存）</div>
                )}
              </div>
              )}

              {/* [lc-1608] 在线规则目录（KazumiRules index.json）：检查更新 / 逐条安装·更新 / 全部更新。
                  [lc-1755] 属 Kazumi 规则源生态 —— 采集源视图隐藏（两版彻底分开）。 */}
              {kind !== "maccms" && (
              <div className="webrs-cat">
                <div className="webrs-cat-hd">
                  <span className="webrs-cat-title">在线规则</span>
                  <button onClick={() => checkUpdates(true)} disabled={catBusy}>
                    {catBusy ? "检查中…" : "检查更新"}
                  </button>
                  <button
                    className="webrs-cat-upall"
                    onClick={updateAll}
                    disabled={catBusy || updates.length === 0}
                    title={updates.length ? `更新 ${updates.length} 条可更新规则` : "暂无可更新规则"}
                  >
                    全部更新{updates.length ? ` (${updates.length})` : ""}
                  </button>
                </div>
                {catMsg && (
                  <div className={`webrs-cat-msg ${catMsg.kind === "err" ? "err" : catMsg.kind === "ok" ? "ok" : ""}`}>
                    {catMsg.text}
                  </div>
                )}
                {catalog && (
                  <div className="webrs-cat-list">
                    {catalog.map((it) => {
                      const lv = localVer[it.name.toLowerCase()];
                      const up = updateMap[it.name.toLowerCase()];
                      const busy = rowBusy[it.name];
                      const state = !lv ? "new" : up ? "update" : "installed";
                      return (
                        <div key={it.name} className="webrs-cat-row">
                          <span className="webrs-cat-name" title={it.author ? `${it.name} · ${it.author}` : it.name}>
                            {it.name}
                          </span>
                          <span className="webrs-cat-ver">v{it.version}</span>
                          <span className={`webrs-cat-state ${state}`}>
                            {state === "new"
                              ? "未装"
                              : state === "update"
                                ? `可更新 v${lv} → v${it.version}`
                                : `已装 v${lv}`}
                          </span>
                          {state === "new" && (
                            <button
                              className="webrs-cat-act"
                              disabled={!!busy || catBusy}
                              onClick={() => installOrUpdate(it.name, "install")}
                            >
                              {busy === "install" ? "安装中…" : "安装"}
                            </button>
                          )}
                          {state === "update" && (
                            <button
                              className="webrs-cat-act on"
                              disabled={!!busy || catBusy}
                              onClick={() => installOrUpdate(it.name, "update")}
                            >
                              {busy === "update" ? "更新中…" : "更新"}
                            </button>
                          )}
                        </div>
                      );
                    })}
                    {catalog.length === 0 && <div className="webrs-note">在线目录为空</div>}
                  </div>
                )}
                {!catalog && !catBusy && (
                  <div className="webrs-note">点「检查更新」从 KazumiRules 拉取在线规则目录（带 6 小时缓存）</div>
                )}
              </div>
              )}

              <div className="webrs-mgr-tip">
                {kind === "maccms"
                  ? "勾选后关闭本面板，搜索只查已勾选的采集源。一键测试用搜索框关键词（空则「少女」）。"
                  : "勾选后关闭本面板，搜索只查已勾选的源；内置规则源删除后随启动恢复，采集源删除后可到「检查更新」重装。一键测试用搜索框关键词（空则「少女」）。"}
              </div>
            </div>
          </div>
        </div>,
          document.body,
        )}

      {/* [lc-1606] type1 图片验证码弹窗：显示抓到的图 + 输入框 → webrs_captcha_submit。
          portal 挂 body（同源管理弹窗，避开 .dseg 合成穿透）。 */}
      {captcha &&
        createPortal(
          <div className="ddet-mask">
            <div className="sum-modal webrs-captcha-modal" onClick={(e) => e.stopPropagation()}>
              <div className="sum-modal-h">
                <h3>
                  人机验证 <span className="webrs-mgr-count">{captcha.rule}</span>
                </h3>
                <button className="ddet-close sum-modal-x" onClick={cancelCaptcha}>
                  ×
                </button>
              </div>
              <div className="webrs-captcha-body">
                <div className="webrs-captcha-tip">该来源需要验证，请输入下图中的字符以继续检索。</div>
                <div className="webrs-captcha-imgwrap">
                  <img className="webrs-captcha-img" src={captcha.image} alt="验证码" />
                </div>
                <input
                  className="webrs-captcha-input"
                  value={code}
                  autoFocus
                  spellCheck={false}
                  autoComplete="off"
                  placeholder="输入图片中的字符"
                  onChange={(e) => setCode(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && submitCaptcha()}
                />
                <div className="webrs-captcha-acts">
                  <button className="webrs-captcha-cancel" onClick={cancelCaptcha}>
                    取消
                  </button>
                  {/* [lc-1747] 看不清图/自动流程兜不住 → 手动验证兜底（有可用页面地址才显示） */}
                  {manualVerifyUrlFor(captcha.rule) && (
                    <button
                      className="webrs-captcha-cancel webrs-captcha-manual"
                      onClick={() => openManualVerify(captcha.rule)}
                      title="在可见窗口打开该源页面，手动完成网站验证"
                    >
                      打开网页手动验证
                    </button>
                  )}
                  <button className="webrs-go" disabled={!code.trim()} onClick={submitCaptcha}>
                    验证
                  </button>
                </div>
              </div>
            </div>
          </div>,
          document.body,
        )}
    </div>
  );
}

/** [lc-1747] 「打开网页手动验证」入口：可见窗手动过验（webrs_captcha_visible）。
 *  自动验证（隐藏窗）失败 / 嗅探失败（页面可能需验证）时出现在错误态区域。
 *  无可用 URL（前端拿不到该源任何绝对地址）时禁用，悬浮提示原因。 */
function ManualVerifyButton({
  rule,
  urlFor,
  onOpen,
}: {
  rule: string;
  urlFor: (rule: string) => string | null;
  onOpen: (rule: string) => void;
}) {
  const url = urlFor(rule);
  return (
    <button
      className="webrs-manual"
      disabled={!url}
      title={url ? `在可见窗口打开该源页面手动完成验证：${url}` : "暂无该源可用页面地址 —— 先播放/嗅探一次后再试"}
      onClick={() => onOpen(rule)}
    >
      打开网页手动验证
    </button>
  );
}

/**
 * [lc-1513] 延迟卸载：active 变 false 时先保留渲染 closing 时长（给退出动画留时间），
 * 到点再真正卸载。用于「返回上一级」的转场 —— 否则路由一切换旧视图瞬间消失。
 */
