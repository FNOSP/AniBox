/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { X as IcoXL } from "lucide-react";
import { DisclaimerModal } from "../disclaimer/DisclaimerModal";
import { fmtSize } from "../../lib/format";
import {
  AppInfo,
  AppSettings,
  Config,
  PatchCheckResult,
  UpdateCheckResult,
  UpdateDownloadResult,
  UpdateProgress,
  UpdateType,
} from "../../types";

/* [更新与维护] 「关于」区的占位文案。
   TODO: 等后端 `app_info()` 正式接入并返回稳定字段后，可删掉这两个常量，
         改由 app_info().description / repoUrl 直接提供（当前已做「有则优先」）。
   TODO: 项目地址换成正式仓库地址（仓库当前目录名 Fntv-Plus-Tauri2，产品名 AniBox）。 */
const SOFTWARE_DESC =
  "AniBox 是一款基于 Tauri + React 的影视与番剧聚合播放器：支持飞牛影视库、Bangumi 追番、" +
  "TMDB 元数据与自建弹幕源。";
const PROJECT_URL = "https://github.com/AniBox/AniBox";

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

const asText = (v: unknown): string => {
  if (Array.isArray(v)) return v.map((x) => String(x)).join("\n");
  return v === null || v === undefined ? "" : String(v);
};

/** 后端字段可能是 camelCase 或 snake_case（并行开发期两版都可能出现），统一归一。 */
function normalizeCheck(raw: unknown): UpdateCheckResult {
  const r = (raw ?? {}) as Record<string, unknown>;
  const type = asText(r.updateType ?? r.update_type).toLowerCase();
  return {
    hasUpdate: Boolean(r.hasUpdate ?? r.has_update ?? false),
    currentVersion: asText(r.currentVersion ?? r.current_version),
    latestVersion: asText(r.latestVersion ?? r.latest_version),
    updateType: (type === "hotfix" ? "hotfix" : "full") as UpdateType,
    notes: asText(r.notes ?? r.changelog ?? r.body),
    publishedAt: asText(r.publishedAt ?? r.published_at),
    downloadUrl: asText(r.downloadUrl ?? r.download_url),
    patchUrl: asText(r.patchUrl ?? r.patch_url),
    htmlUrl: asText(r.htmlUrl ?? r.html_url),
  };
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
  const [ver, setVer] = useState("0.1.0");
  const [user, setUser] = useState("");
  const [server, setServer] = useState("");
  const [confirmLogout, setConfirmLogout] = useState(false);
  // [lc-1607] 设置 → 关于 → 免责声明：打开只读态弹窗（只有「关闭」）
  const [showDisclaimer, setShowDisclaimer] = useState(false);
  const [autostart, setAutostart] = useState(
    () => localStorage.getItem("fntv-autostart") === "1",
  );

  /* ── [更新与维护] 状态 ──
     upd        : 最近一次 update_check 结果（模块级缓存，重开面板可恢复）
     downloaded : 全量安装包下载完成后的 { path, size }
     progress   : update-progress / patch-progress 事件的最新进度
     phase      : 当前操作阶段，用来决定按钮/禁用态 */
  const [appMeta, setAppMeta] = useState<AppInfo | null>(null);
  const [upd, setUpd] = useState<UpdateCheckResult | null>(lastCheck);
  const [updChecking, setUpdChecking] = useState(false);
  const [updError, setUpdError] = useState("");
  const [downloading, setDownloading] = useState(false);
  const [downloaded, setDownloaded] = useState<UpdateDownloadResult | null>(lastDownload);
  const [applying, setApplying] = useState(false);
  const [patchApplied, setPatchApplied] = useState(false);
  const [progress, setProgress] = useState<{ percent: number; message: string } | null>(null);
  const [showNotes, setShowNotes] = useState(false);
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
    setProgress({ percent: 0, message: "准备下载…" });
    try {
      const r = await invoke<UpdateDownloadResult>("update_download", {
        version: upd.latestVersion,
      });
      lastDownload = r;
      setDownloaded(r);
      setProgress({ percent: 100, message: "下载完成" });
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
    setProgress({ percent: 0, message: "正在应用补丁…" });
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
    const h = (e: KeyboardEvent) => {
      // [lc-1607] 免责声明弹窗打开时，Esc 归它（只关弹窗，不连设置面板一起关）
      if (e.key === "Escape" && !showDisclaimer) onClose();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose, refresh, showDisclaimer]);

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
      setProgress({ percent, message });
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

  const parseBases = (text: string) =>
    text
      .split(/[\n;；]/)
      .map((s) => s.trim().replace(/\/+$/, ""))
      .filter(Boolean)
      .slice(0, 4);

  const saveDanmu = (enabled: boolean, text: string) => {
    onSettings({ danmuApiEnabled: enabled, danmuApiBases: parseBases(text) });
  };

  const testDanmu = async () => {
    setDmTesting(true);
    setDmNote("");
    try {
      const list = parseBases(dmBases);
      if (!list.length) {
        setDmNote("请先填写服务地址");
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
    <div className="ddet-mask" onClick={onClose}>
      <div className="ddet set-card" onClick={(e) => e.stopPropagation()}>
        <button className="ddet-close" title="关闭（Esc）" onClick={onClose}>
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
        <div className="ddet-body set-body">
          {/* ── 通用 ── */}
          <h3 className="ddet-h3">通用</h3>
          <div className="set-row">
            <div>
              <span className="set-k">启动页</span>
              <div className="set-desc">登录成功后进入的页面</div>
            </div>
            <select
              className="set-select"
              value={settings.launchPage}
              disabled={busy}
              onChange={(e) => onSettings({ launchPage: e.target.value as AppSettings["launchPage"] })}
            >
              <option value="home">首页</option>
              <option value="continue">继续观看</option>
            </select>
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
            <select
              className="set-select"
              value={settings.exitMode}
              disabled={busy}
              onChange={(e) => onSettings({ exitMode: e.target.value as AppSettings["exitMode"] })}
            >
              <option value="ask">每次询问</option>
              <option value="tray">最小化到托盘</option>
              <option value="exit">直接退出</option>
            </select>
          </div>

          {/* ── 外观 ── */}
          <h3 className="ddet-h3">外观</h3>
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

          {/* ── 弹幕 ── */}
          <h3 className="ddet-h3">弹幕</h3>
          <div className="set-row">
            <div>
              <span className="set-k">自建弹幕接口</span>
              <div className="set-desc">
                自部署 danmu_api（LogVar 弹幕API）作为优选源，聚合多平台弹幕；未命中自动降级内置 B站
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
          <div className="set-row set-row-col">
            <span className="set-k">服务地址</span>
            <textarea
              className="set-ta"
              rows={2}
              placeholder={"http://192.168.31.170:9321/你的TOKEN\nhttp://100.66.1.2:9321/你的TOKEN"}
              value={dmBases}
              disabled={busy}
              onChange={(e) => setDmBases(e.target.value)}
              onBlur={() => saveDanmu(settings.danmuApiEnabled, dmBases)}
            />
            <div className="set-actions">
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
            </div>
          </div>

          {/* ── 账号 ── */}
          <h3 className="ddet-h3">账号</h3>
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

          {/* ── 缓存 ── */}
          <h3 className="ddet-h3">缓存</h3>
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

          {/* ── 诊断 ── */}
          <h3 className="ddet-h3">诊断</h3>
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

          {/* ── 更新与维护 ── */}
          <h3 className="ddet-h3">更新与维护</h3>
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

          {(updStatus || updError) && (
            <div className="set-row set-row-col set-upd">
              <div className="set-upd-status">
                {updStatus && <span className={`set-upd-dot ${updStatus.tone}`} />}
                <span className="set-upd-status-text">{updStatus?.text}</span>
                {upd?.publishedAt && <span className="set-upd-date">发布于 {upd.publishedAt}</span>}
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

          {/* ── 关于 ── */}
          <h3 className="ddet-h3">关于</h3>
          <div className="set-row set-row-col">
            <span className="set-k">软件说明</span>
            <div className="set-desc">{softwareDesc}</div>
          </div>
          <div className="set-row">
            <div>
              <span className="set-k">项目地址</span>
              <div className="set-desc">{projectUrl}</div>
            </div>
            <button type="button" className="set-btn" onClick={() => openExternal(projectUrl)}>
              打开
            </button>
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
          {note && <div className="set-note">{note}</div>}
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
