/* [lc-1710] 启动更新提示弹窗。
   外壳复用仓库既有的 .ddet-mask / .ddet（与设置面板同款），内部样式全走 44-update.css 的 upx-*。
   portal 到 body：不被 .app-shell 的圆角/溢出与视图转场上下文裁剪。
   逻辑与设置面板「更新与维护」一致：update_check 的结果直接传入，下载/补丁复用
   update_download / patch_apply / update_install 与 update-progress / patch-progress 事件。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Download as IcoDownload, Sparkles as IcoSparkles, Star as IcoStar, X as IcoXL } from "lucide-react";
import type { UpdateCheckResult, UpdateDownloadResult, UpdateProgress } from "../../types";
import { asText, formatSpeed, versionLabel } from "../../lib/update";

const STAR_URL = "https://github.com/FNOSP/AniBox";

/** [lc-1776] 极简 markdown → HTML（更新日志渲染；不引入依赖）：
 *  支持标题/加粗/行内代码/无序列表/引用/水平线/链接/表格；其余按段落文本。
 *  先整体转义，再做行内替换，杜绝日志内容注入 HTML。 */
function escHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}
function inlineMd(s: string): string {
  return escHtml(s)
    .replace(/`([^`]+)`/g, '<code class="upx-md-code">$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noopener noreferrer">$1</a>');
}
function mdToHtml(md: string): string {
  const lines = md.replace(/\r/g, "").split("\n");
  const out: string[] = [];
  let inList = false;
  let tableRows: string[][] = [];
  const flushList = () => { if (inList) { out.push("</ul>"); inList = false; } };
  const flushTable = () => {
    if (!tableRows.length) return;
    const rows = tableRows.filter((r) => !r.every((c) => /^-{2,}$/.test(c.trim().replace(/\|/g, "")) || c.trim() === ""));
    if (rows.length) {
      const th = rows[0].map((c) => `<th>${inlineMd(c.trim())}</th>`).join("");
      const trs = rows.slice(1).map((r) => `<tr>${r.map((c) => `<td>${inlineMd(c.trim())}</td>`).join("")}</tr>`).join("");
      out.push(`<table class="upx-md-table"><thead><tr>${th}</tr></thead><tbody>${trs}</tbody></table>`);
    }
    tableRows = [];
  };
  for (const line of lines) {
    const t = line.trim();
    if (/^\|.*\|$/.test(t)) { flushList(); tableRows.push(t.slice(1, -1).split("|")); continue; }
    flushTable();
    if (!t) { flushList(); continue; }
    const h = /^(#{1,4})\s+(.*)$/.exec(t);
    if (h) { flushList(); const lv = Math.min(h[1].length + 1, 5); out.push(`<h${lv} class="upx-md-h">${inlineMd(h[2])}</h${lv}>`); continue; }
    if (/^(---+|\*\*\*+)$/.test(t)) { flushList(); out.push('<hr class="upx-md-hr" />'); continue; }
    if (t.startsWith("> ")) { flushList(); out.push(`<blockquote class="upx-md-quote">${inlineMd(t.slice(2))}</blockquote>`); continue; }
    if (/^[-*+]\s+/.test(t)) { if (!inList) { out.push('<ul class="upx-md-ul">'); inList = true; } out.push(`<li>${inlineMd(t.replace(/^[-*+]\s+/, ""))}</li>`); continue; }
    flushList();
    out.push(`<p class="upx-md-p">${inlineMd(t)}</p>`);
  }
  flushList(); flushTable();
  return out.join("\n");
}

/** 用系统默认浏览器打开外链（同 SettingsPanel / FeedbackPanel 的做法）。 */
async function openExternal(url: string) {
  try {
    await openUrl(url);
  } catch {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

type BusyPhase = "idle" | "downloading" | "applying" | "installing";

export function UpdatePrompt({
  info,
  onClose,
}: {
  info: UpdateCheckResult;
  onClose: () => void;
}) {
  const [closing, setClosing] = useState(false);
  const [busy, setBusy] = useState<BusyPhase>("idle");
  const [progress, setProgress] = useState<{ percent: number; message: string; speed: number } | null>(null);
  const [downloaded, setDownloaded] = useState<UpdateDownloadResult | null>(null);
  const [applied, setApplied] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  // 「取消」是用户主动发起的：此后 update_download 的 reject 不再当成错误展示
  const cancelRequested = useRef(false);

  const isHotfix = info.updateType === "hotfix";
  const latest = versionLabel(info.latestVersion);
  const current = versionLabel(info.currentVersion);

  const requestClose = useCallback(() => {
    if (closing || busy === "installing") return;
    setClosing(true);
    window.setTimeout(onClose, 170); // 与 CSS 动画时长对齐
  }, [closing, busy, onClose]);

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") requestClose();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [requestClose]);

  /* 进度事件：update-progress / patch-progress；percent 缺省时用 loaded/total 现算。 */
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
      const speed = Number(p.speed);
      setProgress({
        percent,
        message: asText(p.message ?? p.phase),
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

  const download = useCallback(async () => {
    cancelRequested.current = false;
    setBusy("downloading");
    setError("");
    setNotice("");
    setProgress({ percent: 0, message: "准备下载…", speed: 0 });
    try {
      const r = await invoke<UpdateDownloadResult>("update_download", {
        version: info.latestVersion,
      });
      setDownloaded(r);
      setProgress({ percent: 100, message: "下载完成", speed: 0 });
    } catch (e) {
      if (!cancelRequested.current) {
        setError(String(e));
        setProgress(null);
      } else {
        setProgress(null);
      }
    } finally {
      setBusy("idle");
    }
  }, [info.latestVersion]);

  const cancelDownload = useCallback(async () => {
    cancelRequested.current = true;
    setProgress((p) => (p ? { ...p, message: "正在取消…" } : p));
    try {
      await invoke("update_cancel");
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const install = useCallback(async () => {
    if (!downloaded?.path) return;
    setBusy("installing");
    setError("");
    try {
      // 成功后后端会拉起安装器并退出应用，Promise 可能不会返回
      await invoke("update_install", { path: downloaded.path });
    } catch (e) {
      setError(String(e));
      setBusy("idle");
    }
  }, [downloaded]);

  const applyPatch = useCallback(async () => {
    setBusy("applying");
    setError("");
    setNotice("");
    setProgress({ percent: 0, message: "正在应用补丁…", speed: 0 });
    try {
      const r = await invoke<{ applied: boolean }>("patch_apply", {
        version: info.latestVersion,
      });
      if (r?.applied) {
        setApplied(true);
        setProgress(null);
      } else {
        setError("补丁未应用（后端返回 applied=false）");
        setProgress(null);
      }
    } catch (e) {
      setError(String(e));
      setProgress(null);
    } finally {
      setBusy("idle");
    }
  }, [info.latestVersion]);

  const openDetails = useCallback(() => {
    const url = info.htmlUrl.trim();
    if (!url || url === "#") {
      setNotice("该版本未提供详情链接。");
      return;
    }
    void openExternal(url);
  }, [info.htmlUrl]);

  const downloading = busy === "downloading";
  const installing = busy === "installing";
  const applying = busy === "applying";
  const done = downloaded !== null;

  return createPortal(
    <div
      className={`ddet-mask${closing ? " upx-mask-closing" : ""}`}
      onClick={requestClose}
    >
      <div className="ddet upx-modal" onClick={(e) => e.stopPropagation()}>
        <button className="ddet-close" title="稍后（Esc）" aria-label="关闭" onClick={requestClose}>
          <IcoXL size={15} strokeWidth={2} />
        </button>

        <header className="upx-hd">
          <div className="upx-badge" aria-hidden="true">
            {isHotfix ? <IcoSparkles size={22} strokeWidth={2} /> : <IcoDownload size={22} strokeWidth={2} />}
          </div>
          <div className="upx-hdm">
            <h3 className="upx-title">发现新版本 {latest || "—"}</h3>
            <div className="upx-meta">
              <span className={`upx-chip${isHotfix ? " hot" : ""}`}>{isHotfix ? "热补丁" : "全量更新"}</span>
              {current && <span>当前版本 {current}</span>}
              {info.publishedAt && <span>发布于 {info.publishedAt}</span>}
              {info.source && <span>更新源 {info.source}</span>}
            </div>
          </div>
        </header>

        <div className="upx-body">
          {/* 纯文本渲染：React 默认转义，不解析/执行 HTML */}
          {/* [lc-1776] 更新日志按 markdown 渲染（此前 <pre> 裸文本暴露 # ** 等符号） */}
          <div
            className="upx-notes upx-md"
            dangerouslySetInnerHTML={{ __html: mdToHtml(info.notes.trim() || "本次更新暂无详细说明。") }}
          />

          {progress && (
            <div className="upx-progress">
              <div className="upx-bar">
                <i style={{ width: `${progress.percent}%` }} />
              </div>
              <div className="upx-progress-meta">
                <span className="upx-pct">{Math.round(progress.percent)}%</span>
                {progress.speed > 0 && <span className="upx-speed">{formatSpeed(progress.speed)}</span>}
                {progress.message && <span className="upx-msg">{progress.message}</span>}
              </div>
            </div>
          )}

          {installing && <div className="upx-hint">正在启动安装程序，应用即将退出…</div>}
          {!installing && done && (
            <div className="upx-hint ok">安装包已下载完成，点击「立即安装」将启动安装程序并退出应用。</div>
          )}
          {applied && <div className="upx-hint ok">补丁已应用，重启应用后生效。</div>}
          {notice && <div className="upx-hint">{notice}</div>}
          {error && (
            <div className="upx-err">
              <span>{error}</span>
              {!isHotfix && !done && (
                <button type="button" className="set-btn" disabled={busy !== "idle"} onClick={download}>
                  重试
                </button>
              )}
            </div>
          )}
        </div>

        <footer className="upx-acts">
          <button
            type="button"
            className="set-btn upx-star"
            disabled={installing}
            onClick={() => void openExternal(STAR_URL)}
            title="在 GitHub 上给 AniBox 点个星标"
          >
            <IcoStar size={14} strokeWidth={1.8} />
            给作者点个星标
          </button>
          <button type="button" className="set-btn" disabled={installing} onClick={requestClose}>
            稍后
          </button>
          <button type="button" className="set-btn" disabled={installing} onClick={openDetails}>
            查看详情
          </button>
          {downloading && (
            <button type="button" className="set-btn" onClick={cancelDownload}>
              取消下载
            </button>
          )}
          {isHotfix ? (
            <button
              type="button"
              className="set-btn primary"
              disabled={applied || busy !== "idle"}
              onClick={applyPatch}
            >
              {applied ? "补丁已应用" : applying ? "应用中…" : "应用补丁"}
            </button>
          ) : done ? (
            <button
              type="button"
              className="set-btn primary"
              disabled={installing || busy !== "idle"}
              onClick={install}
            >
              {installing ? "安装中…" : "立即安装"}
            </button>
          ) : (
            <button
              type="button"
              className="set-btn primary"
              disabled={downloading || busy !== "idle"}
              onClick={download}
            >
              {downloading ? "下载中…" : "下载安装包"}
            </button>
          )}
        </footer>
      </div>
    </div>,
    document.body,
  );
}
