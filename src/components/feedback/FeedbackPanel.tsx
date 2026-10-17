/* [lc-1599] 应用内反馈（工单制）面板。
   信息架构：顶部标题区 + 常驻双分区导航（反馈 / 日志与诊断），两条主线一眼可见。
   - 反馈：二级视图「我的工单 / 新建反馈」，工单会话作为该分区的详情层（可返回）。
   - 日志与诊断：工具条 + 自适应卡片网格（日志文件 / 调试日志 / 匿名统计 / 诊断信息）。
   外壳复用 .ddet-mask/.ddet（与 SettingsPanel 同款），内部样式全走 33-feedback.css 的 fbx-*。
   portal 到 body（对齐 WebrwsPanel 的成因）：面板由侧栏（覆盖式浮层，z45）内触发打开，
   留在原层级会受侧栏/顶栏合成层干扰；portal 后与 .dseg 等同在顶层上下文，z40 遮罩正常压住内容区。
   数据链路 = Rust 命令：feedback_submit / feedback_list / feedback_thread / feedback_reply；
   以及从旧 Electron 设置面板迁移来的日志/诊断命令：feedback_log_info / feedback_open_log /
   feedback_read_log / feedback_diagnostics / feedback_export_diagnostics；
   调试日志设置走固定契约 feedback_log_settings_get / feedback_log_settings_set。
   会话视图打开期间每 20 秒轮询一次（实时状态同步）；网络失败只行内提示、保留已有内容。 */
import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity as IcoActivity,
  ArrowLeft as IcoBack,
  Bug as IcoBug,
  Copy as IcoCopy,
  Download as IcoDownload,
  ExternalLink as IcoExt,
  FileText as IcoFile,
  Inbox as IcoInbox,
  LifeBuoy as IcoLifeBuoy,
  MessageSquarePlus as IcoNewFb,
  RefreshCw as IcoRefresh,
  ScrollText as IcoDiag,
  Send as IcoSend,
  X as IcoXL,
} from "lucide-react";
import { fmtSize } from "../../lib/format";

/** feedback_list 返回项（Rust 侧本地索引 + 远端状态合并；远端缺失时 status="unknown"） */
type FbListItem = {
  id: string;
  ts: number;
  title: string;
  status: string;
  updated: number | null;
  lastRole: string | null;
  lastTs: number | null;
};

/** feedback_thread 的 ticket 段（服务端冻结字段） */
type FbTicket = {
  id: string;
  ts: number;
  status: string;
  updated: number;
  message: string;
  has_log: boolean;
  ver: string;
  os: string;
  arch: string;
};

type FbMsg = { ts: number; role: string; text: string };
type FbThread = { ok: boolean; ticket: FbTicket; messages: FbMsg[] };

/** feedback_log_info（Rust 侧返回的日志路径/大小；mpv 日志不存在时 mpvExists=false） */
type LogInfo = {
  path: string;
  dir: string;
  size: number;
  exists: boolean;
  mpvPath: string;
  mpvSize: number;
  mpvExists: boolean;
  maxBytes: number;
};

/** 调试日志的组件级开关（键名与后端契约一致） */
type LogModules = {
  mpv: boolean;
  bgm: boolean;
  mirror: boolean;
  feedback: boolean;
  history: boolean;
  webrs: boolean;
  other: boolean;
};

/** feedback_log_settings_get / _set 的统一返回结构 */
type LogSettings = {
  debug: boolean;
  modules: LogModules;
  errorLogPath: string;
};

const MODULE_LABELS: { key: keyof LogModules; label: string }[] = [
  { key: "mpv", label: "MPV" },
  { key: "bgm", label: "BGM" },
  { key: "mirror", label: "镜像缓存" },
  { key: "feedback", label: "反馈" },
  { key: "history", label: "观影记录" },
  { key: "webrs", label: "在线源" },
  { key: "other", label: "其他" },
];

/** 状态枚举 → 中文（对齐服务端冻结枚举；unknown = 网络降级时的本地兜底） */
const STATUS_TEXT: Record<string, string> = {
  open: "待处理",
  confirmed: "已确认",
  progress: "已跟进",
  resolved: "已解决",
  closed: "已关闭",
  unknown: "未同步",
};

const KNOWN_STATUS = ["open", "confirmed", "progress", "resolved", "closed"];

const badgeCls = (s: string) => (KNOWN_STATUS.includes(s) ? s : "unknown");
const statusText = (s: string) => STATUS_TEXT[s] ?? "未同步";

/** 时间戳格式化：秒/毫秒自适应（服务端字段单位未知时兜底）；今年只显示 MM-DD HH:mm */
function fmtTs(ts: number | null | undefined): string {
  if (!ts || !Number.isFinite(ts)) return "";
  const ms = ts > 1e11 ? ts : ts * 1000;
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "";
  const p = (n: number) => String(n).padStart(2, "0");
  const base = `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
  return d.getFullYear() === new Date().getFullYear() ? base : `${d.getFullYear()}-${base}`;
}

export function FeedbackPanel({ onClose, page }: { onClose: () => void; page?: string }) {
  // 一级分区（常驻导航）与反馈区二级视图
  const [section, setSection] = useState<"feedback" | "diag">("feedback");
  const [fbTab, setFbTab] = useState<"tickets" | "new">("tickets");
  // 列表
  const [items, setItems] = useState<FbListItem[] | null>(null);
  const [listErr, setListErr] = useState("");
  // 新建
  const [msg, setMsg] = useState("");
  const [contact, setContact] = useState("");
  const [includeLog, setIncludeLog] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [submitErr, setSubmitErr] = useState("");
  // 会话（反馈分区的详情层）
  const [threadOpen, setThreadOpen] = useState(false);
  const [threadId, setThreadId] = useState("");
  const [thread, setThread] = useState<FbThread | null>(null);
  const [threadErr, setThreadErr] = useState("");
  const [reply, setReply] = useState("");
  const [sending, setSending] = useState(false);
  const [copyNote, setCopyNote] = useState("");
  // 日志 / 诊断工具（旧 Electron 设置面板迁移）
  const [logInfo, setLogInfo] = useState<LogInfo | null>(null);
  const [diagText, setDiagText] = useState("");
  const [diagLoading, setDiagLoading] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [toolNote, setToolNote] = useState("");
  const [toolErr, setToolErr] = useState("");
  // 调试日志设置（固定契约 feedback_log_settings_get / _set）
  const [logSettings, setLogSettings] = useState<LogSettings | null>(null);
  const [savingLogSettings, setSavingLogSettings] = useState(false);
  const [logSetErr, setLogSetErr] = useState("");
  // 匿名统计（后端命令尚未在 Rust 侧实现时降级为不可用，不影响其它功能）
  const [statsEnabled, setStatsEnabled] = useState<boolean | null>(null);
  const [statsAvailable, setStatsAvailable] = useState<boolean | null>(null);
  const [savingStats, setSavingStats] = useState(false);
  const [statsErr, setStatsErr] = useState("");

  const loadList = useCallback(async () => {
    setListErr("");
    try {
      const list = await invoke<FbListItem[]>("feedback_list");
      setItems(Array.isArray(list) ? list : []);
    } catch (e) {
      // 理论上 Rust 侧已降级不报错；真出错也只提示、保留已有内容
      setListErr(String(e));
      setItems((prev) => prev ?? []);
    }
  }, []);

  const loadLogInfo = useCallback(async () => {
    try {
      setLogInfo(await invoke<LogInfo>("feedback_log_info"));
    } catch {
      // 日志信息只是展示项，失败不影响工单功能
    }
  }, []);

  const loadDiag = useCallback(async () => {
    setDiagLoading(true);
    setToolErr("");
    try {
      const r = await invoke<{ text: string }>("feedback_diagnostics");
      setDiagText(r.text || "");
    } catch (e) {
      setToolErr(String(e));
    } finally {
      setDiagLoading(false);
    }
  }, []);

  const loadLogSettings = useCallback(async () => {
    setLogSetErr("");
    try {
      setLogSettings(await invoke<LogSettings>("feedback_log_settings_get"));
    } catch (e) {
      setLogSetErr(String(e));
    }
  }, []);

  const loadUsageStats = useCallback(async () => {
    setStatsErr("");
    try {
      const r = await invoke<{ enabled: boolean }>("usage_stats_get");
      setStatsEnabled(!!r?.enabled);
      setStatsAvailable(true);
    } catch {
      // 命令缺失/后端未实现：降级为“不可用”，不抛错、不影响其它区块
      setStatsAvailable(false);
    }
  }, []);

  /** 保存调试日志设置：乐观更新 → 失败回滚并提示 */
  const saveLogSettings = async (patch: { debug?: boolean; modules?: LogModules }) => {
    const prev = logSettings;
    if (!prev || savingLogSettings) return;
    const next = {
      debug: patch.debug ?? prev.debug,
      modules: patch.modules ?? prev.modules,
    };
    setLogSettings({ ...prev, ...next });
    setSavingLogSettings(true);
    setLogSetErr("");
    try {
      const r = await invoke<LogSettings>("feedback_log_settings_set", next);
      setLogSettings(r);
    } catch (e) {
      setLogSettings(prev); // 回滚到保存前
      setLogSetErr(`保存失败：${String(e)}`);
    } finally {
      setSavingLogSettings(false);
    }
  };

  const toggleModule = (key: keyof LogModules, checked: boolean) => {
    if (!logSettings) return;
    void saveLogSettings({ modules: { ...logSettings.modules, [key]: checked } });
  };

  /** 匿名统计开关：同样乐观更新、失败回滚 */
  const saveStats = async (enabled: boolean) => {
    if (savingStats || statsEnabled === null) return;
    const prev = statsEnabled;
    setStatsEnabled(enabled);
    setSavingStats(true);
    setStatsErr("");
    try {
      const r = await invoke<{ enabled: boolean }>("usage_stats_set", { enabled });
      if (typeof r?.enabled === "boolean") setStatsEnabled(r.enabled);
    } catch (e) {
      setStatsEnabled(prev);
      setStatsErr(`保存失败：${String(e)}`);
    } finally {
      setSavingStats(false);
    }
  };

  // 进入工单列表时拉取（打开面板 / 从新建或会话返回）
  useEffect(() => {
    if (section === "feedback" && fbTab === "tickets" && !threadOpen) void loadList();
  }, [section, fbTab, threadOpen, loadList]);

  // 面板打开即取日志信息（新建视图的“当前日志”提示也要用）
  useEffect(() => {
    void loadLogInfo();
  }, [loadLogInfo]);

  // 诊断分区：每次进入刷新日志信息、诊断文本、日志设置与匿名统计（并清掉上次的提示）
  useEffect(() => {
    if (section === "diag") {
      setToolNote("");
      void loadLogInfo();
      void loadDiag();
      void loadLogSettings();
      void loadUsageStats();
    }
  }, [section, loadLogInfo, loadDiag, loadLogSettings, loadUsageStats]);

  const loadThread = useCallback(async (id: string) => {
    try {
      const t = await invoke<FbThread>("feedback_thread", { id });
      setThread(t);
      setThreadErr("");
    } catch (e) {
      setThreadErr(String(e)); // 网络失败：行内提示，保留已有内容
    }
  }, []);

  // 会话视图打开期间每 20 秒轮询一次（实时状态同步；失败不打断，等下一轮自愈）
  useEffect(() => {
    if (section !== "feedback" || !threadOpen || !threadId) return;
    void loadThread(threadId);
    const t = window.setInterval(() => void loadThread(threadId), 20000);
    return () => window.clearInterval(t);
  }, [section, threadOpen, threadId, loadThread]);

  // Escape 关闭（输入法合成中的 Escape 不关 —— 那是取消候选词）
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.isComposing) onClose();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);

  const openThread = (id: string) => {
    setThreadId(id);
    setThread(null);
    setThreadErr("");
    setReply("");
    setSection("feedback");
    setThreadOpen(true);
  };

  const backToTickets = () => {
    setThreadOpen(false);
    setFbTab("tickets");
  };

  const submit = async () => {
    const text = msg.trim();
    if (text.length < 5 || submitting) return;
    setSubmitting(true);
    setSubmitErr("");
    try {
      const r = await invoke<{ id: string }>("feedback_submit", {
        message: text,
        contact: contact.trim() || undefined,
        includeLog,
        page: page || undefined,
      });
      setMsg("");
      setContact("");
      openThread(r.id); // 成功后切到该工单的会话视图
    } catch (e) {
      setSubmitErr(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const sendReply = async () => {
    const text = reply.trim();
    if (!text || sending || !threadId) return;
    setSending(true);
    setThreadErr("");
    try {
      await invoke("feedback_reply", { id: threadId, text });
      setReply("");
      // 乐观追加（随后 refetch 校正；refetch 失败也不至于「发了看不到」）
      setThread((prev) =>
        prev
          ? {
              ...prev,
              messages: [
                ...prev.messages,
                { ts: Math.floor(Date.now() / 1000), role: "user", text },
              ],
            }
          : prev,
      );
      await loadThread(threadId);
    } catch (e) {
      setThreadErr(String(e));
    } finally {
      setSending(false);
    }
  };

  const copyId = async () => {
    try {
      await navigator.clipboard.writeText(threadId);
      setCopyNote("已复制");
    } catch {
      setCopyNote("复制失败");
    }
    window.setTimeout(() => setCopyNote(""), 1500);
  };

  // ── 日志 / 诊断工具（对应旧 Electron 设置面板的日志按钮） ──
  const openLog = async (kind: "log" | "dir" | "mpv" | "diagnostics" | "error") => {
    setToolErr("");
    setToolNote("");
    try {
      await invoke("feedback_open_log", { kind });
    } catch (e) {
      setToolErr(String(e));
    }
  };

  const copyLog = async () => {
    setToolErr("");
    setToolNote("");
    try {
      const r = await invoke<{ text: string; bytes: number }>("feedback_read_log", {});
      if (!r.text) {
        setToolNote("日志为空或尚未生成");
        return;
      }
      await navigator.clipboard.writeText(r.text);
      setToolNote(`已复制日志（已脱敏，${fmtSize(r.bytes)}）`);
    } catch (e) {
      setToolErr(String(e));
    }
  };

  const copyDiag = async () => {
    if (!diagText) return;
    try {
      await navigator.clipboard.writeText(diagText);
      setToolNote("诊断信息已复制");
    } catch {
      setToolErr("复制失败");
    }
  };

  const exportDiag = async () => {
    if (exporting) return;
    setExporting(true);
    setToolErr("");
    setToolNote("正在导出诊断包…");
    try {
      const r = await invoke<{ path: string }>("feedback_export_diagnostics");
      setToolNote(`诊断包已导出：${r.path}`);
      // 导出后直接打开所在目录（与旧项目“导出日志”后给路径的体验一致）
      await invoke("feedback_open_log", { kind: "diagnostics" }).catch(() => {});
    } catch (e) {
      setToolNote("");
      setToolErr(String(e));
    } finally {
      setExporting(false);
    }
  };

  const refreshDiag = () => {
    void loadLogInfo();
    void loadDiag();
    void loadLogSettings();
    void loadUsageStats();
  };

  const ticketStatus = thread?.ticket?.status ?? "";
  // 原始提交内容固定渲染为第一条 user 气泡；若服务端 messages[0] 与它完全相同则跳过（防重复）
  const origMsg = thread?.ticket?.message ?? "";
  const msgs = thread
    ? thread.messages.filter(
        (m, i) => !(i === 0 && m.role === "user" && m.text === origMsg),
      )
    : [];
  const canSubmit = msg.trim().length >= 5 && !submitting;

  return createPortal(
    <div className="ddet-mask" onClick={onClose}>
      <div className="ddet fbx-card" onClick={(e) => e.stopPropagation()}>
        <button className="ddet-close fbx-close" title="关闭（Esc）" aria-label="关闭反馈面板" onClick={onClose}>
          <IcoXL size={15} strokeWidth={1.8} />
        </button>

        {/* ── 顶部标题区 ── */}
        <header className="fbx-hd">
          <span className="fbx-brand-ico" aria-hidden="true">
            <IcoLifeBuoy size={19} strokeWidth={1.9} />
          </span>
          <div className="fbx-hd-txt">
            <h2 className="fbx-title">反馈与诊断</h2>
            <p className="fbx-subtitle">提交问题、跟踪工单，或导出脱敏日志协助排查</p>
          </div>
        </header>

        {/* ── 一级分区导航（常驻，两条主线一眼可见） ── */}
        <nav className="fbx-nav" role="group" aria-label="反馈面板分区">
          <button
            className={`fbx-nav-btn${section === "feedback" ? " is-active" : ""}`}
            aria-pressed={section === "feedback"}
            onClick={() => setSection("feedback")}
          >
            <IcoNewFb size={17} strokeWidth={1.9} aria-hidden="true" />
            <span className="fbx-nav-txt">
              <b>反馈</b>
              <small>提交问题 · 我的工单</small>
            </span>
          </button>
          <button
            className={`fbx-nav-btn${section === "diag" ? " is-active" : ""}`}
            aria-pressed={section === "diag"}
            onClick={() => setSection("diag")}
          >
            <IcoActivity size={17} strokeWidth={1.9} aria-hidden="true" />
            <span className="fbx-nav-txt">
              <b>日志与诊断</b>
              <small>日志文件 · 调试开关 · 导出</small>
            </span>
          </button>
        </nav>

        <div className="ddet-body fbx-body">
          {/* ═════════ 反馈分区 ═════════ */}
          {section === "feedback" && !threadOpen && (
            <div className="fbx-fb">
              <div className="fbx-subbar">
                <div className="fbx-tabs" role="group" aria-label="反馈视图">
                  <button
                    className={`fbx-tab${fbTab === "tickets" ? " is-active" : ""}`}
                    aria-pressed={fbTab === "tickets"}
                    onClick={() => setFbTab("tickets")}
                  >
                    我的工单{items ? ` (${items.length})` : ""}
                  </button>
                  <button
                    className={`fbx-tab${fbTab === "new" ? " is-active" : ""}`}
                    aria-pressed={fbTab === "new"}
                    onClick={() => {
                      setSubmitErr("");
                      setFbTab("new");
                    }}
                  >
                    新建反馈
                  </button>
                </div>
                <div className="fbx-subbar-actions">
                  <a
                    className="fbx-btn subtle"
                    href="https://qm.qq.com/q/dUnIQVvoIw"
                    target="_blank"
                    rel="noreferrer"
                    title="加入 QQ 交流群"
                  >
                    <IcoExt size={13} aria-hidden="true" />
                    加入 Q 群
                  </a>
                  {fbTab === "tickets" && (
                    <button
                      className="fbx-btn subtle"
                      onClick={() => void loadList()}
                      disabled={items === null}
                      title="刷新工单"
                    >
                      <IcoRefresh size={13} aria-hidden="true" />
                      刷新
                    </button>
                  )}
                </div>
              </div>

              {fbTab === "tickets" ? (
                <>
                  {listErr && items && items.length > 0 && (
                    <div className="fbx-note err" role="alert">
                      <span>{listErr}</span>
                      <button className="fbx-linkbtn" onClick={() => void loadList()}>
                        重试
                      </button>
                    </div>
                  )}
                  <div className="fbx-list">
                    {items === null ? (
                      <div className="fbx-state">
                        <span className="fbx-spinner" aria-hidden="true" />
                        <p className="fbx-state-title">正在加载工单…</p>
                      </div>
                    ) : items.length === 0 ? (
                      listErr ? (
                        <div className="fbx-state">
                          <span className="fbx-state-ico" aria-hidden="true">
                            <IcoInbox size={22} strokeWidth={1.6} />
                          </span>
                          <p className="fbx-state-title">工单列表加载失败</p>
                          <p className="fbx-state-txt">{listErr}</p>
                          <button className="fbx-btn" onClick={() => void loadList()}>
                            <IcoRefresh size={14} aria-hidden="true" />
                            重试
                          </button>
                        </div>
                      ) : (
                        <div className="fbx-state">
                          <span className="fbx-state-ico" aria-hidden="true">
                            <IcoInbox size={22} strokeWidth={1.6} />
                          </span>
                          <p className="fbx-state-title">还没有反馈记录</p>
                          <p className="fbx-state-txt">
                            遇到问题或有建议？提交后会生成一个可追踪的工单，处理进展会在这里同步。
                          </p>
                          <button
                            className="fbx-btn primary"
                            onClick={() => {
                              setSubmitErr("");
                              setFbTab("new");
                            }}
                          >
                            <IcoNewFb size={14} aria-hidden="true" />
                            写第一条反馈
                          </button>
                        </div>
                      )
                    ) : (
                      items.map((it) => (
                        <button key={it.id} className="fbx-row" onClick={() => openThread(it.id)}>
                          <span className="fbx-row-main">
                            <span className="fbx-row-title">{it.title || "（无标题）"}</span>
                            <span className="fbx-row-meta">
                              提交 {fmtTs(it.ts)}
                              {it.lastTs ? ` · 最后回复 ${fmtTs(it.lastTs)}` : ""}
                            </span>
                          </span>
                          <span className="fbx-row-side">
                            <span className={`fbx-badge ${badgeCls(it.status)}`}>
                              {statusText(it.status)}
                            </span>
                          </span>
                        </button>
                      ))
                    )}
                  </div>
                </>
              ) : (
                <div className="fbx-form">
                  <div className="fbx-form-scroll">
                    <section className="fbx-pane">
                      <div className="fbx-pane-hd">
                        <h3 className="fbx-pane-h">提交新反馈</h3>
                        <span className="fbx-chip">匿名提交 · 可追踪</span>
                      </div>

                      <label className="fbx-k" htmlFor="fbx-msg">
                        问题描述
                        <span className="fbx-req">必填 · 至少 5 字</span>
                      </label>
                      <textarea
                        id="fbx-msg"
                        className="fbx-ta"
                        rows={5}
                        autoFocus
                        value={msg}
                        onChange={(e) => setMsg(e.target.value)}
                        placeholder="描述你遇到的问题或建议（越具体越好）…"
                      />
                      <div className="fbx-count">{msg.trim().length} 字</div>

                      <label className="fbx-k" htmlFor="fbx-contact">
                        联系方式
                        <span className="fbx-req">可选</span>
                      </label>
                      <input
                        id="fbx-contact"
                        className="fbx-input"
                        value={contact}
                        onChange={(e) => setContact(e.target.value)}
                        placeholder="邮箱 / QQ / 其他，便于回复跟进"
                      />

                      <label className="fbx-toggle-row">
                        <input
                          type="checkbox"
                          checked={includeLog}
                          onChange={(e) => setIncludeLog(e.target.checked)}
                        />
                        <span className="fbx-toggle-txt">
                          <b>附带最近日志</b>
                          <small>自动脱敏后随工单提交</small>
                        </span>
                      </label>

                      <p className="fbx-hint">
                        日志会先脱敏（账号 / token / 密码 / 手机号 / 邮箱 / 本机用户名），保留 NAS
                        地址与域名便于排查。
                      </p>
                      <p className="fbx-hint">
                        {logInfo
                          ? `当前日志：${fmtSize(logInfo.size)}（最多附带最近 ${fmtSize(
                              Math.min(logInfo.maxBytes, 512 * 1024),
                            )}）`
                          : "正在读取日志信息…"}
                      </p>

                      {submitErr && (
                        <div className="fbx-note err" role="alert">
                          {submitErr}
                        </div>
                      )}
                    </section>
                  </div>
                  <div className="fbx-form-ft">
                    <button
                      className="fbx-btn"
                      onClick={() => setFbTab("tickets")}
                      disabled={submitting}
                    >
                      取消
                    </button>
                    <button
                      className="fbx-btn primary"
                      onClick={() => void submit()}
                      disabled={!canSubmit}
                    >
                      {submitting ? (
                        <>
                          <span className="fbx-spinner sm" aria-hidden="true" />
                          提交中…
                        </>
                      ) : (
                        "提交反馈"
                      )}
                    </button>
                  </div>
                </div>
              )}
            </div>
          )}

          {/* ═════════ 工单会话（反馈分区详情层） ═════════ */}
          {section === "feedback" && threadOpen && (
            <div className="fbx-thread">
              <div className="fbx-thread-hd">
                <button className="fbx-btn subtle" onClick={backToTickets}>
                  <IcoBack size={14} aria-hidden="true" />
                  我的工单
                </button>
                <span className={`fbx-badge ${badgeCls(ticketStatus)}`}>
                  {thread ? statusText(ticketStatus) : "…"}
                </span>
                {thread?.ticket?.has_log && <span className="fbx-chip">含日志</span>}
                <span className="fbx-tidwrap">
                  <button className="fbx-tid" title="点击复制工单号" onClick={() => void copyId()}>
                    {threadId}
                  </button>
                  {copyNote && <span className="fbx-copy">{copyNote}</span>}
                </span>
              </div>

              {threadErr && (
                <div className="fbx-note err" role="alert">
                  {threadErr}
                </div>
              )}

              <div className="fbx-msgs">
                {thread === null ? (
                  <div className="fbx-state">
                    {threadErr ? null : <span className="fbx-spinner" aria-hidden="true" />}
                    <p className="fbx-state-title">
                      {threadErr ? "加载失败" : "正在加载会话…"}
                    </p>
                    {threadErr && (
                      <p className="fbx-state-txt">每 20 秒自动重试，也可稍后返回列表重进。</p>
                    )}
                  </div>
                ) : (
                  <>
                    <div className="fbx-msg user">
                      <div className="fbx-msg-txt">{thread.ticket.message}</div>
                      <div className="fbx-msg-time">{fmtTs(thread.ticket.ts)}</div>
                    </div>
                    {msgs.map((m, i) => (
                      <div
                        key={`${m.ts}-${i}`}
                        className={`fbx-msg ${m.role === "admin" ? "admin" : "user"}`}
                      >
                        <div className="fbx-msg-txt">{m.text}</div>
                        <div className="fbx-msg-time">{fmtTs(m.ts)}</div>
                      </div>
                    ))}
                    {msgs.length === 0 && (
                      <p className="fbx-thread-empty">
                        暂无更多回复，处理进展会在这里同步（每 20 秒自动刷新）。
                      </p>
                    )}
                  </>
                )}
              </div>

              <div className="fbx-reply">
                <textarea
                  className="fbx-ta fbx-reply-ta"
                  rows={2}
                  value={reply}
                  onChange={(e) => setReply(e.target.value)}
                  onKeyDown={(e) => {
                    // Enter 发送、Shift+Enter 换行；输入法合成中的 Enter 不算（那是选字）
                    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                      e.preventDefault();
                      void sendReply();
                    }
                  }}
                  placeholder="补充说明或回复…（Enter 发送，Shift+Enter 换行）"
                  disabled={sending}
                />
                <button
                  className="fbx-btn primary fbx-send"
                  onClick={() => void sendReply()}
                  disabled={sending || !reply.trim()}
                >
                  <IcoSend size={14} aria-hidden="true" />
                  {sending ? "发送中…" : "发送"}
                </button>
              </div>
            </div>
          )}

          {/* ═════════ 日志与诊断分区 ═════════ */}
          {section === "diag" && (
            <div className="fbx-diag-wrap">
              <div className="fbx-diag-toolbar">
                <div className="fbx-diag-heading">
                  <h3 className="fbx-pane-h">日志与诊断</h3>
                  <p className="fbx-hint">查看或导出脱敏日志，调整调试日志与匿名统计。</p>
                </div>
                <div className="fbx-diag-actions">
                  <button
                    className="fbx-btn subtle"
                    onClick={refreshDiag}
                    disabled={diagLoading}
                  >
                    <IcoRefresh size={13} aria-hidden="true" />
                    {diagLoading ? "刷新中…" : "刷新全部"}
                  </button>
                  <button
                    className="fbx-btn primary"
                    onClick={() => void exportDiag()}
                    disabled={exporting}
                  >
                    <IcoDownload size={14} aria-hidden="true" />
                    {exporting ? "导出中…" : "导出诊断包"}
                  </button>
                </div>
              </div>

              {toolErr && (
                <div className="fbx-note err" role="alert">
                  {toolErr}
                </div>
              )}
              {toolNote && (
                <div className="fbx-note" role="status">
                  {toolNote}
                </div>
              )}

              <div className="fbx-diag">
                <div className="fbx-diag-grid">
                  {/* 日志文件 */}
                  <section className="fbx-sec">
                    <div className="fbx-sec-hd">
                      <h4 className="fbx-sec-h">
                        <IcoFile size={14} aria-hidden="true" />
                        日志文件
                      </h4>
                    </div>
                    <div className="fbx-kv">
                      <span>诊断日志</span>
                      <code title={logInfo?.path || ""}>{logInfo?.path || "读取中…"}</code>
                    </div>
                    <div className="fbx-kv">
                      <span>大小</span>
                      <span>{logInfo ? fmtSize(logInfo.size) : "…"}</span>
                    </div>
                    <div className="fbx-kv">
                      <span>MPV 日志</span>
                      <code title={logInfo?.mpvPath || ""}>{logInfo?.mpvPath || "读取中…"}</code>
                    </div>
                    <div className="fbx-kv">
                      <span>MPV 大小</span>
                      <span>
                        {logInfo ? (logInfo.mpvExists ? fmtSize(logInfo.mpvSize) : "尚未生成") : "…"}
                      </span>
                    </div>
                    <div className="fbx-btnrow">
                      <button
                        className="fbx-btn"
                        onClick={() => void openLog("log")}
                        disabled={!logInfo?.exists}
                      >
                        打开日志
                      </button>
                      <button className="fbx-btn" onClick={() => void openLog("error")}>
                        打开报错日志
                      </button>
                      <button className="fbx-btn" onClick={() => void openLog("dir")}>
                        打开日志目录
                      </button>
                      <button className="fbx-btn" onClick={() => void copyLog()}>
                        <IcoCopy size={13} aria-hidden="true" />
                        复制日志（脱敏）
                      </button>
                      <button
                        className="fbx-btn"
                        onClick={() => void openLog("mpv")}
                        disabled={!logInfo?.mpvExists}
                      >
                        MPV 日志
                      </button>
                    </div>
                    <p className="fbx-hint">
                      复制与导出的日志都会先按 feedback.rs 的同一套规则脱敏（token / 密码 / Bearer /
                      手机号 / 邮箱 / 本机用户名），NAS 地址与域名保留用于排查网络。
                    </p>
                    <p className="fbx-hint">
                      诊断包包含：诊断文本 + 日志设置 + anibox.log + anibox-error.log（均已脱敏）。
                    </p>
                  </section>

                  {/* 调试日志 */}
                  <section className="fbx-sec">
                    <div className="fbx-sec-hd">
                      <h4 className="fbx-sec-h">
                        <IcoBug size={14} aria-hidden="true" />
                        调试日志
                      </h4>
                      {savingLogSettings && <span className="fbx-hint">保存中…</span>}
                    </div>
                    {logSetErr && (
                      <div className="fbx-note err" role="alert">
                        {logSetErr}
                      </div>
                    )}
                    {logSettings === null ? (
                      <p className="fbx-hint">{logSetErr ? "日志设置读取失败" : "读取中…"}</p>
                    ) : (
                      <>
                        <label className="fbx-check">
                          <input
                            type="checkbox"
                            checked={logSettings.debug}
                            disabled={savingLogSettings}
                            onChange={(e) => void saveLogSettings({ debug: e.target.checked })}
                          />
                          <span>启用调试日志</span>
                        </label>
                        <div className="fbx-modgrid">
                          {MODULE_LABELS.map(({ key, label }) => (
                            <label key={key} className="fbx-check">
                              <input
                                type="checkbox"
                                checked={!!logSettings.modules[key]}
                                disabled={savingLogSettings || !logSettings.debug}
                                onChange={(e) => toggleModule(key, e.target.checked)}
                              />
                              <span>{label}</span>
                            </label>
                          ))}
                        </div>
                        <div className="fbx-kv">
                          <span>报错日志</span>
                          <code title={logSettings.errorLogPath || ""}>
                            {logSettings.errorLogPath || "—"}
                          </code>
                        </div>
                        <div className="fbx-btnrow">
                          <button className="fbx-btn" onClick={() => void openLog("error")}>
                            打开报错日志
                          </button>
                        </div>
                        <p className="fbx-hint">
                          按组件分别开启日志，便于定位问题；关闭总开关后不再写入调试日志。
                        </p>
                      </>
                    )}
                  </section>

                  {/* 匿名统计 */}
                  <section className="fbx-sec fbx-span2">
                    <div className="fbx-sec-hd">
                      <h4 className="fbx-sec-h">
                        <IcoActivity size={14} aria-hidden="true" />
                        匿名统计
                      </h4>
                      {savingStats && <span className="fbx-hint">保存中…</span>}
                    </div>
                    {statsErr && (
                      <div className="fbx-note err" role="alert">
                        {statsErr}
                      </div>
                    )}
                    {statsAvailable === null ? (
                      <p className="fbx-hint">读取中…</p>
                    ) : statsAvailable ? (
                      <label className="fbx-check">
                        <input
                          type="checkbox"
                          checked={!!statsEnabled}
                          disabled={savingStats}
                          onChange={(e) => void saveStats(e.target.checked)}
                        />
                        <span>参与匿名统计</span>
                      </label>
                    ) : (
                      <p className="fbx-hint">当前版本暂不支持匿名统计设置。</p>
                    )}
                    <p className="fbx-hint">
                      仅上报匿名使用数据（随机匿名 ID、应用版本、系统类型），不含账号、IP、媒体库或文件路径；
                      服务端不存储 IP，可随时在此关闭。
                    </p>
                  </section>

                  {/* 诊断信息 */}
                  <section className="fbx-sec fbx-span2">
                    <div className="fbx-sec-hd">
                      <h4 className="fbx-sec-h">
                        <IcoDiag size={14} aria-hidden="true" />
                        诊断信息
                      </h4>
                      <div className="fbx-btnrow fbx-btnrow-inline">
                        <button
                          className="fbx-btn subtle"
                          onClick={() => void loadDiag()}
                          disabled={diagLoading}
                        >
                          <IcoRefresh size={13} aria-hidden="true" />
                          {diagLoading ? "刷新中…" : "刷新"}
                        </button>
                        <button
                          className="fbx-btn subtle"
                          onClick={() => void copyDiag()}
                          disabled={!diagText}
                        >
                          <IcoCopy size={13} aria-hidden="true" />
                          复制
                        </button>
                      </div>
                    </div>
                    <pre className="fbx-pre">
                      {diagText || (diagLoading ? "加载中…" : "（无诊断信息）")}
                    </pre>
                  </section>
                </div>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
