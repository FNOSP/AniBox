/* [lc-1599] 应用内反馈（工单制）面板：列表 / 新建 / 会话 三视图，同一弹窗内切换。
   外壳复用 .ddet-mask/.ddet（与 SettingsPanel 同款），内部样式全走 33-feedback.css 的 fbx-*。
   portal 到 body（对齐 WebrsPanel 的成因）：面板由侧栏（覆盖式浮层，z45）内触发打开，
   留在原层级会受侧栏/顶栏合成层干扰；portal 后与 .dseg 等同在顶层上下文，z40 遮罩正常压住内容区。
   数据链路 = Rust 四条命令：feedback_submit / feedback_list / feedback_thread / feedback_reply。
   会话视图打开期间每 20 秒轮询一次（实时状态同步）；网络失败只行内提示、保留已有内容。 */
import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { X as IcoXL } from "lucide-react";

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
  const [view, setView] = useState<"list" | "new" | "thread">("list");
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

  // 进入列表视图时拉取（打开面板 / 从新建或会话返回）
  useEffect(() => {
    if (view === "list") void loadList();
  }, [view, loadList]);

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
              <button className="fbx-btn" onClick={() => void loadList()} disabled={items === null}>
                刷新
              </button>
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
              {submitErr && <div className="fbx-note err">{submitErr}</div>}
              <div className="fbx-actions">
                <button className="fbx-btn" onClick={() => setView("list")} disabled={submitting}>
                  返回列表
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
