/* [lc-1599] 应用内反馈（工单制）面板：列表 / 新建 / 会话 / 诊断工具 四视图，同一弹窗内切换。
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
import { X as IcoXL } from "lucide-react";
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
  const [view, setView] = useState<"list" | "new" | "thread" | "diag">("list");
  // 列表
  const [items, setItems] = useState<FbListItem[] | null>(null);
  const [listErr, setListErr] = useState("");
  // 新建
  const [msg, setMsg] = useState("");
  const [contact, setContact] = useState("");
  const [includeLog, setIncludeLog] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [submitErr, setSubmitErr] = useState("");
  // 会话
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

  // 进入列表视图时拉取（打开面板 / 从新建或会话返回）
  useEffect(() => {
    if (view === "list") void loadList();
  }, [view, loadList]);

  // 面板打开即取日志信息（新建视图的“当前日志”提示也要用）
  useEffect(() => {
    void loadLogInfo();
  }, [loadLogInfo]);

  // 诊断视图：每次进入刷新日志信息、诊断文本、日志设置与匿名统计
  useEffect(() => {
    if (view === "diag") {
      void loadLogInfo();
      void loadDiag();
      void loadLogSettings();
      void loadUsageStats();
    }
  }, [view, loadLogInfo, loadDiag, loadLogSettings, loadUsageStats]);

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
    if (view !== "thread" || !threadId) return;
    void loadThread(threadId);
    const t = window.setInterval(() => void loadThread(threadId), 20000);
    return () => window.clearInterval(t);
  }, [view, threadId, loadThread]);

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
    setView("thread");
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
    }
  };

  const ticketStatus = thread?.ticket?.status ?? "";
  // 原始提交内容固定渲染为第一条 user 气泡；若服务端 messages[0] 与它完全相同则跳过（防重复）
  const origMsg = thread?.ticket?.message ?? "";
  const msgs = thread
    ? thread.messages.filter(
        (m, i) => !(i === 0 && m.role === "user" && m.text === origMsg),
      )
    : [];

  return createPortal(
    <div className="ddet-mask" onClick={onClose}>
      <div className="ddet fbx-card" onClick={(e) => e.stopPropagation()}>
        <button className="ddet-close" title="关闭（Esc）" onClick={onClose}>
          <IcoXL size={15} strokeWidth={1.8} />
        </button>
        <header className="ddet-hd">
          <div className="ddet-hdm">
            <h2>反馈</h2>
            <div className="ddet-meta">
              <span className="ddet-chip dim">工单制 · 匿名提交</span>
            </div>
          </div>
        </header>

        {view === "list" && (
          <div className="ddet-body fbx-body">
            <div className="fbx-toolbar">
              <button
                className="fbx-btn primary"
                onClick={() => {
                  setSubmitErr("");
                  setView("new");
                }}
              >
                ＋ 新建反馈
              </button>
              <div className="fbx-toolbar-r">
                <a
                  className="fbx-btn"
                  href="https://qm.qq.com/q/dUnIQVvoIw"
                  target="_blank"
                  rel="noreferrer"
                  title="加入 QQ 交流群"
                >
                  加入 Q 群
                </a>
                <button className="fbx-btn" onClick={() => void loadList()} disabled={items === null}>
                  刷新
                </button>
                <button
                  className="fbx-btn"
                  onClick={() => {
                    setToolNote("");
                    setToolErr("");
                    setView("diag");
                  }}
                >
                  日志与诊断
                </button>
              </div>
            </div>
            {listErr && <div className="fbx-note err">{listErr}</div>}
            <div className="fbx-list">
              {items === null ? (
                <div className="fbx-empty">加载中…</div>
              ) : items.length === 0 ? (
                <div className="fbx-empty">还没有反馈记录</div>
              ) : (
                items.map((it) => (
                  <button key={it.id} className="fbx-row" onClick={() => openThread(it.id)}>
                    <span className={`fbx-badge ${badgeCls(it.status)}`}>{statusText(it.status)}</span>
                    <span className="fbx-row-main">
                      <span className="fbx-row-title">{it.title || "（无标题）"}</span>
                      <span className="fbx-row-meta">
                        提交 {fmtTs(it.ts)}
                        {it.lastTs ? ` · 最后回复 ${fmtTs(it.lastTs)}` : ""}
                      </span>
                    </span>
                  </button>
                ))
              )}
            </div>
          </div>
        )}

        {view === "new" && (
          <div className="ddet-body fbx-body">
            <div className="fbx-form">
              <label className="fbx-k">
                问题描述
                <span className="fbx-req">必填，至少 5 字</span>
              </label>
              <textarea
                className="fbx-ta"
                rows={5}
                autoFocus
                value={msg}
                onChange={(e) => setMsg(e.target.value)}
                placeholder="描述你遇到的问题或建议（越具体越好）…"
              />
              <label className="fbx-k">
                联系方式<span className="fbx-req">（可选）</span>
              </label>
              <input
                className="fbx-input"
                value={contact}
                onChange={(e) => setContact(e.target.value)}
                placeholder="邮箱 / QQ / 其他，便于回复跟进"
              />
              <label className="fbx-check">
                <input
                  type="checkbox"
                  checked={includeLog}
                  onChange={(e) => setIncludeLog(e.target.checked)}
                />
                <span>附带最近日志（自动脱敏）</span>
              </label>
              <div className="fbx-hint">
                日志会先脱敏（账号/token/密码/手机号/邮箱/本机用户名），保留 NAS 地址与域名便于排查
              </div>
              <div className="fbx-hint">
                {logInfo
                  ? `当前日志：${fmtSize(logInfo.size)}（最多附带最近 ${fmtSize(Math.min(logInfo.maxBytes, 512 * 1024))}）`
                  : "正在读取日志信息…"}
              </div>
              {submitErr && <div className="fbx-note err">{submitErr}</div>}
              <div className="fbx-actions">
                <button className="fbx-btn" onClick={() => setView("list")} disabled={submitting}>
                  返回列表
                </button>
                <button className="fbx-btn" onClick={() => setView("diag")} disabled={submitting}>
                  日志与诊断
                </button>
                <button
                  className="fbx-btn primary"
                  onClick={() => void submit()}
                  disabled={submitting || msg.trim().length < 5}
                >
                  {submitting ? "提交中…" : "提交"}
                </button>
              </div>
            </div>
          </div>
        )}

        {view === "diag" && (
          <div className="ddet-body fbx-body">
            <div className="fbx-toolbar">
              <button className="fbx-btn" onClick={() => setView("list")}>
                ← 返回列表
              </button>
              <button className="fbx-btn" onClick={() => void loadLogInfo()}>
                刷新日志信息
              </button>
            </div>
            {toolErr && <div className="fbx-note err">{toolErr}</div>}
            {toolNote && <div className="fbx-note">{toolNote}</div>}

            <div className="fbx-diag">
              <section className="fbx-sec">
                <h4 className="fbx-sec-h">日志文件</h4>
                <div className="fbx-kv">
                  <span>诊断日志</span>
                  <code>{logInfo?.path || "读取中…"}</code>
                </div>
                <div className="fbx-kv">
                  <span>大小</span>
                  <span>{logInfo ? fmtSize(logInfo.size) : "…"}</span>
                </div>
                <div className="fbx-kv">
                  <span>MPV 日志</span>
                  <code>{logInfo?.mpvPath || "读取中…"}</code>
                </div>
                <div className="fbx-kv">
                  <span>MPV 日志大小</span>
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
                    复制日志（脱敏）
                  </button>
                  <button className="fbx-btn" onClick={() => void exportDiag()}>
                    导出诊断包
                  </button>
                  <button
                    className="fbx-btn"
                    onClick={() => void openLog("mpv")}
                    disabled={!logInfo?.mpvExists}
                  >
                    MPV 日志
                  </button>
                </div>
                <div className="fbx-hint">
                  复制与导出的日志都会先按 feedback.rs 的同一套规则脱敏（token / 密码 / Bearer /
                  手机号 / 邮箱 / 本机用户名），NAS 地址与域名保留用于排查网络。
                </div>
                <div className="fbx-hint">
                  导出诊断包包含：诊断文本 + 日志设置 + anibox.log + anibox-error.log（日志均已脱敏）。
                </div>
              </section>

              <section className="fbx-sec">
                <div className="fbx-sec-hd">
                  <h4 className="fbx-sec-h">调试日志</h4>
                  {savingLogSettings && <span className="fbx-hint">保存中…</span>}
                </div>
                {logSetErr && <div className="fbx-note err">{logSetErr}</div>}
                {logSettings === null ? (
                  <div className="fbx-hint">{logSetErr ? "日志设置读取失败" : "读取中…"}</div>
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
                      <code>{logSettings.errorLogPath || "—"}</code>
                    </div>
                    <div className="fbx-btnrow">
                      <button className="fbx-btn" onClick={() => void openLog("error")}>
                        打开报错日志
                      </button>
                    </div>
                    <div className="fbx-hint">
                      按组件分别开启日志，便于定位问题；关闭总开关后不再写入调试日志。
                    </div>
                  </>
                )}
              </section>

              <section className="fbx-sec">
                <div className="fbx-sec-hd">
                  <h4 className="fbx-sec-h">匿名统计</h4>
                  {savingStats && <span className="fbx-hint">保存中…</span>}
                </div>
                {statsErr && <div className="fbx-note err">{statsErr}</div>}
                {statsAvailable === null ? (
                  <div className="fbx-hint">读取中…</div>
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
                  <div className="fbx-hint">当前版本暂不支持匿名统计设置。</div>
                )}
                <div className="fbx-hint">
                  仅上报匿名使用数据（随机匿名 ID、应用版本、系统类型），不含账号、IP、媒体库或文件路径；
                  服务端不存储 IP，可随时在此关闭。
                </div>
              </section>

              <section className="fbx-sec">
                <div className="fbx-sec-hd">
                  <h4 className="fbx-sec-h">诊断信息</h4>
                  <div className="fbx-btnrow">
                    <button className="fbx-btn" onClick={() => void loadDiag()} disabled={diagLoading}>
                      {diagLoading ? "刷新中…" : "刷新"}
                    </button>
                    <button className="fbx-btn" onClick={() => void copyDiag()} disabled={!diagText}>
                      复制
                    </button>
                  </div>
                </div>
                <pre className="fbx-pre">{diagText || (diagLoading ? "加载中…" : "（无诊断信息）")}</pre>
              </section>
            </div>
          </div>
        )}

        {view === "thread" && (
          <div className="ddet-body fbx-body">
            <div className="fbx-thread-hd">
              <button className="fbx-btn" onClick={() => setView("list")}>
                ← 返回
              </button>
              <span className={`fbx-badge ${badgeCls(ticketStatus)}`}>
                {thread ? statusText(ticketStatus) : "…"}
              </span>
              {thread?.ticket?.has_log && <span className="ddet-chip dim">含日志</span>}
              <span className="fbx-tidwrap">
                <button className="fbx-tid" title="点击复制工单号" onClick={() => void copyId()}>
                  {threadId}
                </button>
                {copyNote && <span className="fbx-copy">{copyNote}</span>}
              </span>
            </div>
            {threadErr && <div className="fbx-note err">{threadErr}</div>}
            <div className="fbx-msgs">
              {thread === null ? (
                <div className="fbx-empty">{threadErr ? "加载失败，每 20 秒自动重试…" : "加载中…"}</div>
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
                {sending ? "发送中…" : "发送"}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
}
