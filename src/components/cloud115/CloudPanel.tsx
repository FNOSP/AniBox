/* [lc-1810/1811] 多云盘资源库面板：7 家网盘 + WebDAV 统一接入。
   · 登录：cloud_qr_start/poll（扫码）；WebDAV 走账号表单（连通性验证后落盘）。
   · 浏览：cloud_list / webdav_list 统一 cursor 分页。
   · 播放三档（LumiPlayer 播放决策对齐）：
     - 直链（默认）：Rust 续签代理 → mpv 拿稳定本地地址（过期自动换链，播放不中断）
     - 服务端转码：115/夸克/阿里（各家 m3u8，CDN 直连）
     - 本地转码：其余家 + 系统检测到 ffmpeg（-c copy remux HLS）
   · 改名：夸克/阿里/115/百度/123（行内编辑，✎ 按钮）。
   · 凭据只落本机（cloud115.json / direct-cloud-auth.json / webdav-accounts.json）。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Cloud as IcoCloudL,
  FileVideo as IcoVideoL,
  Folder as IcoFolderL,
  LogOut as IcoLogoutL,
  Pencil as IcoRenameL,
  Play as IcoPlayL,
  Plus as IcoPlusL,
  RefreshCw as IcoRefreshL,
  Trash2 as IcoTrashL,
} from "lucide-react";

type ProviderInfo = { id: string; label: string; logged: boolean; userName: string };

type Entry = {
  fileId: string;
  navId?: string;
  path?: string;
  name: string;
  isDir: boolean;
  size: number;
  kind: "dir" | "video" | "audio" | "other" | "sub";
};

type ListResp = { items: Entry[]; nextCursor: string; count?: number };
type Crumb = { fid: string; name: string };
type WebdavAccount = { id: string; name: string; url: string; username: string };

const HINTS: Record<string, string> = {
  "115": "打开 115 生活 App（或 115 App）扫一扫登录",
  quark: "打开夸克 App（或夸克网盘 App）扫一扫登录",
  baidu: "打开百度网盘 App 扫一扫登录",
  aliyun: "打开阿里云盘 App 扫一扫登录",
  tianyi: "打开天翼云盘 App 扫一扫登录",
  "123": "打开 123 云盘 App 扫一扫登录",
  guangya: "打开光鸭云盘 App 扫一扫登录",
};

/** [lc-1811] 播放档能力矩阵（server=服务端转码；local=本地 ffmpeg） */
const SERVER_HLS = new Set(["115", "quark", "aliyun"]);
const RENAMEABLE = new Set(["115", "quark", "aliyun", "baidu", "123"]);

const fmtSize = (n: number) => {
  if (!n) return "";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${u[i]}`;
};

export function CloudPanel({ onBack }: { onBack: () => void }) {
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [active, setActive] = useState("115");
  const [qr, setQr] = useState<{ qrcode: string; state?: string; uid?: string; time?: string; sign?: string } | null>(null);
  const [qrMsg, setQrMsg] = useState("");
  const [crumbs, setCrumbs] = useState<Crumb[]>([{ fid: "", name: "根目录" }]);
  const [items, setItems] = useState<Entry[]>([]);
  const [cursor, setCursor] = useState("");
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState("");
  const [mode, setMode] = useState(""); // "" 直链(续签) | "server" | "local"
  const [ffmpegOk, setFfmpegOk] = useState<boolean | null>(null);
  // [lc-1811] 改名（行内编辑）
  const [renaming, setRenaming] = useState<{ id: string; value: string } | null>(null);
  // [lc-1811] WebDAV
  const [wdAccounts, setWdAccounts] = useState<WebdavAccount[]>([]);
  const [wdActive, setWdActive] = useState<string | null>(null);
  const [wdForm, setWdForm] = useState<{ name: string; url: string; user: string; pass: string } | null>(null);
  const alive = useRef(true);
  const pollTimer = useRef<number | null>(null);

  const stopPoll = () => {
    if (pollTimer.current !== null) {
      window.clearInterval(pollTimer.current);
      pollTimer.current = null;
    }
  };

  const cur = providers.find((p) => p.id === active);
  const isWebdav = active === "webdav";
  const logged = isWebdav ? !!wdActive : !!cur?.logged;

  const refreshProviders = useCallback(async (): Promise<ProviderInfo[]> => {
    const r = await invoke<ProviderInfo[]>("cloud_providers");
    if (alive.current) setProviders(r);
    return r;
  }, []);

  const loadList = useCallback(
    async (fid: string, cur_: string, replace: boolean, provider: string) => {
      setLoading(true);
      setErr("");
      try {
        const r = await invoke<ListResp>("cloud_list", { provider, fid: fid || null, cursor: cur_ || null });
        if (!alive.current) return;
        setItems((prev) => (replace ? r.items : [...prev, ...r.items]));
        setCursor(r.nextCursor || "");
      } catch (e) {
        if (alive.current) setErr(String(e));
      } finally {
        if (alive.current) setLoading(false);
      }
    },
    [],
  );

  const loadWebdav = useCallback(async (accountId: string, path: string, replace: boolean) => {
    setLoading(true);
    setErr("");
    try {
      const r = await invoke<ListResp>("webdav_list", { accountId, path: path || null });
      if (!alive.current) return;
      setItems((prev) => (replace ? r.items : [...prev, ...r.items]));
      setCursor(r.nextCursor || "");
    } catch (e) {
      if (alive.current) setErr(String(e));
    } finally {
      if (alive.current) setLoading(false);
    }
  }, []);

  const loadWdAccounts = useCallback(async () => {
    try {
      const r = await invoke<WebdavAccount[]>("webdav_accounts");
      if (alive.current) setWdAccounts(r);
      return r;
    } catch {
      return [];
    }
  }, []);

  const startQr = useCallback(
    async (provider: string) => {
      setErr("");
      setQr(null);
      setQrMsg("正在获取二维码…");
      stopPoll();
      try {
        if (provider === "115") {
          const r = await invoke<{ qrcode: string; uid: string; time: string; sign: string }>("cloud115_qr_start");
          if (!alive.current) return;
          setQr(r);
          setQrMsg(HINTS[provider]);
          pollTimer.current = window.setInterval(async () => {
            try {
              const p = await invoke<{ status: number; ok: boolean; userName?: string }>("cloud115_qr_poll", {
                uid: r.uid,
                time: r.time,
                sign: r.sign,
              });
              if (!alive.current) return stopPoll();
              if (p.ok) {
                stopPoll();
                await refreshProviders();
                void loadList("", "", true, provider);
              } else if (p.status === 1) {
                setQrMsg("已扫码，请在手机上确认登录");
              }
            } catch (e) {
              stopPoll();
              if (alive.current) setErr(String(e));
            }
          }, 2000);
          return;
        }
        const r = await invoke<{ qrcode: string; state: string; hint: string }>("cloud_qr_start", { provider });
        if (!alive.current) return;
        setQr(r);
        setQrMsg(r.hint || HINTS[provider]);
        pollTimer.current = window.setInterval(async () => {
          try {
            const p = await invoke<{ status: number; ok: boolean; userName?: string }>("cloud_qr_poll", {
              provider,
              state: r.state,
            });
            if (!alive.current) return stopPoll();
            if (p.ok) {
              stopPoll();
              await refreshProviders();
              void loadList("", "", true, provider);
            } else if (p.status === 1) {
              setQrMsg("已扫码，请在手机上确认登录");
            }
          } catch (e) {
            stopPoll();
            if (alive.current) setErr(String(e));
          }
        }, 2000);
      } catch (e) {
        if (alive.current) setErr(String(e));
      }
    },
    [loadList, refreshProviders],
  );

  useEffect(() => {
    alive.current = true;
    refreshProviders()
      .then((list) => {
        const a = list.find((p) => p.id === active);
        if (a?.logged) void loadList("", "", true, active);
        else if (!a?.logged) void startQr(active);
      })
      .catch((e) => alive.current && setErr(String(e)));
    loadWdAccounts();
    invoke<{ available: boolean }>("cloud_ffmpeg_available")
      .then((r) => alive.current && setFfmpegOk(r.available))
      .catch(() => alive.current && setFfmpegOk(false));
    return () => {
      alive.current = false;
      stopPoll();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const switchProvider = (id: string) => {
    if (id === active) return;
    stopPoll();
    setActive(id);
    setQr(null);
    setQrMsg("");
    setErr("");
    setItems([]);
    setCursor("");
    setMode("");
    setCrumbs([{ fid: "", name: "根目录" }]);
    setRenaming(null);
    if (id === "webdav") {
      if (wdActive) void loadWebdav(wdActive, "", true);
      return;
    }
    const p = providers.find((x) => x.id === id);
    if (p?.logged) void loadList("", "", true, id);
    else void startQr(id);
  };

  const openDir = (e: Entry) => {
    const nav = e.navId ?? e.fileId;
    setCrumbs((prev) => [...prev, { fid: nav, name: e.name }]);
    setRenaming(null);
    if (isWebdav) void loadWebdav(wdActive!, e.path || nav, true);
    else void loadList(nav, "", true, active);
  };

  const jumpCrumb = (i: number) => {
    const next = crumbs.slice(0, i + 1);
    setCrumbs(next);
    setRenaming(null);
    if (isWebdav) void loadWebdav(wdActive!, next[next.length - 1].fid, true);
    else void loadList(next[next.length - 1].fid, "", true, active);
  };

  const play = async (e: Entry) => {
    setErr("");
    try {
      if (isWebdav) {
        await invoke("webdav_play", { accountId: wdActive, url: e.fileId, name: e.name });
      } else {
        await invoke("cloud_play", { provider: active, fileId: e.fileId, name: e.name, mode: mode || null });
      }
    } catch (ex) {
      setErr(String(ex));
    }
  };

  const logout = async () => {
    stopPoll();
    if (isWebdav) {
      setWdActive(null);
      setItems([]);
      setCrumbs([{ fid: "", name: "根目录" }]);
      return;
    }
    await invoke("cloud_logout", { provider: active }).catch(() => {});
    await refreshProviders();
    setItems([]);
    setCrumbs([{ fid: "", name: "根目录" }]);
    void startQr(active);
  };

  // 改名提交
  const submitRename = async (e: Entry) => {
    if (!renaming || !renaming.value.trim() || renaming.value === e.name) {
      setRenaming(null);
      return;
    }
    setErr("");
    try {
      await invoke("cloud_rename", { provider: active, fileId: e.fileId, newName: renaming.value.trim() });
      setItems((prev) => prev.map((it) => (it.fileId === e.fileId ? { ...it, name: renaming.value.trim() } : it)));
      setRenaming(null);
    } catch (ex) {
      setErr(String(ex));
    }
  };

  // WebDAV 账号操作
  const addWdAccount = async () => {
    if (!wdForm) return;
    setErr("");
    try {
      await invoke("webdav_account_add", { name: wdForm.name, url: wdForm.url, username: wdForm.user, password: wdForm.pass });
      setWdForm(null);
      const list = await loadWdAccounts();
      if (list.length) {
        setWdActive(list[list.length - 1].id);
        void loadWebdav(list[list.length - 1].id, "", true);
      }
    } catch (ex) {
      setErr(String(ex));
    }
  };

  const delWdAccount = async (id: string) => {
    await invoke("webdav_account_del", { id }).catch(() => {});
    const list = await loadWdAccounts();
    if (wdActive === id) {
      setWdActive(list.length ? list[0].id : null);
      setItems([]);
      setCrumbs([{ fid: "", name: "根目录" }]);
      if (list.length) void loadWebdav(list[0].id, "", true);
    }
  };

  const pickWdAccount = (id: string) => {
    setWdActive(id);
    setCrumbs([{ fid: "", name: "根目录" }]);
    void loadWebdav(id, "", true);
  };

  const showServerHls = !isWebdav && SERVER_HLS.has(active);
  const showLocalHls = !showServerHls && ffmpegOk === true && active !== "115";

  return (
    <main className="c115">
      {/* 头部与库/观影记录页同构：page-head + 统一 back 胶囊 */}
      <div className="page-head">
        <button className="back" onClick={onBack}>‹ 返回</button>
        <h3 className="c115-title">
          <span className="c115-title-ico">
            <IcoCloudL size={16} strokeWidth={2.2} />
          </span>
          网盘接入
        </h3>
      </div>

      {/* 厂商 tab 行（含 WebDAV） */}
      <div className="c115-tabs">
        {providers.map((p) => (
          <button
            key={p.id}
            type="button"
            className={`c115-tab${p.id === active ? " on" : ""}`}
            onClick={() => switchProvider(p.id)}
          >
            {p.label}
            <span className={`c115-dot${p.logged ? " ok" : ""}`} title={p.logged ? "已登录" : "未登录"} />
          </button>
        ))}
        <button type="button" className={`c115-tab${isWebdav ? " on" : ""}`} onClick={() => switchProvider("webdav")}>
          WebDAV
          <span className={`c115-dot${wdActive ? " ok" : ""}`} title={wdActive ? "已连接" : "未连接"} />
        </button>
      </div>

      {/* 工具行：用户/退出（左）+ 播放档（右） */}
      {logged && (
        <div className="c115-bar">
          {!isWebdav && cur && (
            <>
              <span className="c115-user">{cur.userName ? `已登录：${cur.userName}` : "已登录"}</span>
              <button className="c115-mini" onClick={logout} title="退出登录（清除本机凭据）">
                <IcoLogoutL size={13} strokeWidth={2} /> 退出
              </button>
            </>
          )}
          {isWebdav && wdActive && (
            <button className="c115-mini" onClick={logout} title="断开当前账号（不删除配置）">
              <IcoLogoutL size={13} strokeWidth={2} /> 断开
            </button>
          )}
          {/* [lc-1811] 播放三档：直链(续签) / 服务端转码 / 本地转码 */}
          {(showServerHls || showLocalHls) && (
            <div className="c115-mode" title="直链走本地续签代理（链接过期自动换新，播放不中断）；转码档用于编码不兼容或非会员限速">
              <button type="button" className={`c115-seg${mode === "" ? " on" : ""}`} onClick={() => setMode("")}>
                直链·自动续签
              </button>
              {showServerHls && (
                <button type="button" className={`c115-seg${mode === "server" ? " on" : ""}`} onClick={() => setMode("server")}>
                  服务端转码
                </button>
              )}
              {showLocalHls && (
                <button type="button" className={`c115-seg${mode === "local" ? " on" : ""}`} onClick={() => setMode("local")}>
                  本地转码
                </button>
              )}
            </div>
          )}
        </div>
      )}

      {err && <p className="c115-err">{err}</p>}

      {/* WebDAV：账号管理（未连接时） */}
      {isWebdav && !wdActive && (
        <div className="c115-login">
          <div className="c115-wdlist">
            {wdAccounts.map((a) => (
              <div key={a.id} className="c115-wdrow">
                <button type="button" className="c115-wdpick" onClick={() => pickWdAccount(a.id)}>
                  <span className="c115-wdname">{a.name}</span>
                  <span className="c115-wdurl">{a.url}</span>
                </button>
                <button className="c115-mini danger" title="删除账号" onClick={() => void delWdAccount(a.id)}>
                  <IcoTrashL size={13} strokeWidth={2} />
                </button>
              </div>
            ))}
            {wdAccounts.length === 0 && <p className="c115-dim">还没有 WebDAV 账号 —— 支持 NAS / Alist / 坚果云等任意 WebDAV 服务</p>}
          </div>
          {wdForm ? (
            <div className="c115-wdform">
              <input placeholder="名称（如：家里 NAS）" value={wdForm.name} onChange={(e) => setWdForm({ ...wdForm, name: e.target.value })} />
              <input placeholder="地址 https://dav.example.com/dav" value={wdForm.url} onChange={(e) => setWdForm({ ...wdForm, url: e.target.value })} />
              <input placeholder="账号" value={wdForm.user} onChange={(e) => setWdForm({ ...wdForm, user: e.target.value })} />
              <input placeholder="密码" type="password" value={wdForm.pass} onChange={(e) => setWdForm({ ...wdForm, pass: e.target.value })} />
              <div className="c115-wdform-btns">
                <button className="c115-mini" onClick={addWdAccount}>验证并保存</button>
                <button className="c115-mini" onClick={() => setWdForm(null)}>取消</button>
              </div>
            </div>
          ) : (
            <button className="c115-mini" onClick={() => setWdForm({ name: "", url: "", user: "", pass: "" })}>
              <IcoPlusL size={13} strokeWidth={2} /> 添加 WebDAV 账号
            </button>
          )}
          <p className="c115-dim">密码用本机 AES-256-GCM 加密存储（enc:v1），不会上传到任何服务器。</p>
        </div>
      )}

      {/* 扫码登录（网盘厂商） */}
      {providers.length > 0 && !isWebdav && !logged && (
        <div className="c115-login">
          {qr ? (
            <>
              <div className="c115-qrwrap">
                <img className="c115-qr" src={qr.qrcode} alt="登录二维码" />
              </div>
              <p className="c115-qrmsg">{qrMsg || HINTS[active]}</p>
            </>
          ) : (
            <div className="c115-qrwrap">
              <div className="c115-qr" aria-hidden="true" />
            </div>
          )}
          <button className="c115-mini" onClick={() => void startQr(active)}>
            <IcoRefreshL size={13} strokeWidth={2} /> 刷新二维码
          </button>
          <p className="c115-dim">
            登录凭据只保存在本机（{active === "115" ? "cloud115.json" : "direct-cloud-auth.json"}），不会上传到任何服务器。
          </p>
        </div>
      )}

      {logged && (
        <>
          <div className="c115-crumbs">
            {crumbs.map((c, i) => (
              <span key={`${c.fid}-${i}`} className="c115-crumb">
                {i > 0 && <span className="c115-sep">/</span>}
                <button type="button" onClick={() => jumpCrumb(i)}>
                  {c.name}
                </button>
              </span>
            ))}
          </div>
          <div className="c115-grid">
            {items.map((e) => (
              <div
                key={`${e.fileId}-${e.name}`}
                className={`c115-item${e.isDir ? " dir" : ""}`}
                onClick={() => (e.isDir ? openDir(e) : undefined)}
              >
                <span className={`c115-ico ${e.kind}`}>
                  {e.isDir ? <IcoFolderL size={15} strokeWidth={1.8} /> : <IcoVideoL size={15} strokeWidth={1.8} />}
                </span>
                {/* 改名态：行内编辑；WebDAV 不支持改名（协议侧无 rename 证据） */}
                {renaming?.id === e.fileId ? (
                  <input
                    className="c115-rename"
                    autoFocus
                    value={renaming.value}
                    onClick={(ev) => ev.stopPropagation()}
                    onChange={(ev) => setRenaming({ id: e.fileId, value: ev.target.value })}
                    onKeyDown={(ev) => {
                      if (ev.key === "Enter") void submitRename(e);
                      if (ev.key === "Escape") setRenaming(null);
                    }}
                    onBlur={() => void submitRename(e)}
                  />
                ) : (
                  <span className="c115-name" title={e.name}>
                    {e.name}
                  </span>
                )}
                <span className="c115-size">{fmtSize(e.size)}</span>
                {!isWebdav && RENAMEABLE.has(active) && !e.isDir && renaming?.id !== e.fileId && (
                  <button
                    className="c115-play ghost"
                    title="重命名"
                    onClick={(ev) => {
                      ev.stopPropagation();
                      setRenaming({ id: e.fileId, value: e.name });
                    }}
                  >
                    <IcoRenameL size={12} strokeWidth={2} />
                  </button>
                )}
                {!e.isDir && e.kind !== "other" && e.kind !== "sub" && (
                  <button
                    className="c115-play"
                    title={mode === "server" ? "服务端转码播放" : mode === "local" ? "本地 ffmpeg 转码播放" : "直链播放（过期自动续链）"}
                    onClick={(ev) => {
                      ev.stopPropagation();
                      void play(e);
                    }}
                  >
                    <IcoPlayL size={13} strokeWidth={2.2} />
                  </button>
                )}
              </div>
            ))}
          </div>
          {loading && <div className="c115-loading">加载中…</div>}
          {!loading && items.length === 0 && (
            <div className="c115-empty">
              <IcoFolderL size={34} strokeWidth={1.4} />
              <p>空目录（或没有可播放的媒体文件）</p>
            </div>
          )}
          {cursor && !loading && !isWebdav && (
            <button className="c115-more" onClick={() => loadList(crumbs[crumbs.length - 1].fid, cursor, false, active)}>
              加载更多
            </button>
          )}
        </>
      )}
    </main>
  );
}
