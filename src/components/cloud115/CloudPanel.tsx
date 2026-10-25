/* [lc-1810] 多云盘资源库面板：7 家网盘统一扫码登录 → 文件浏览 → 播放。
   · 厂商清单/登录态：cloud_providers 一次拉全；tab 切换厂商，各自独立登录。
   · 登录：cloud_qr_start 出二维码（图片 URL 或本地渲 SVG data URI）+ state，
     cloud_qr_poll 2s 轮询；115 走原 cloud115_qr_start/poll 旧三元组。
   · 凭据只落本机（115: cloud115.json；其余: direct-cloud-auth.json），不上传。
   · 浏览：cloud_list 统一 cursor 分页（nextCursor 空串 = 到底）。
   · 播放：cloud_play 现取现用直链（各家链接都有 TTL）；115 保留「转码播放」档。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowLeft as IcoBackL,
  Cloud as IcoCloudL,
  FileVideo as IcoVideoL,
  Folder as IcoFolderL,
  LogOut as IcoLogoutL,
  Play as IcoPlayL,
  RefreshCw as IcoRefreshL,
} from "lucide-react";

type ProviderInfo = { id: string; label: string; logged: boolean; userName: string };

type Entry = {
  fileId: string;
  navId?: string;
  name: string;
  isDir: boolean;
  size: number;
  kind: "dir" | "video" | "audio" | "other";
};

type ListResp = { items: Entry[]; nextCursor: string; count?: number };

type Crumb = { fid: string; name: string };

const HINTS: Record<string, string> = {
  "115": "打开 115 生活 App（或 115 App）扫一扫登录",
  quark: "打开夸克 App（或夸克网盘 App）扫一扫登录",
  baidu: "打开百度网盘 App 扫一扫登录",
  aliyun: "打开阿里云盘 App 扫一扫登录",
  tianyi: "打开天翼云盘 App 扫一扫登录",
  "123": "打开 123 云盘 App 扫一扫登录",
  guangya: "打开光鸭云盘 App 扫一扫登录",
};

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
  const [hls, setHls] = useState(false); // 仅 115：直链 / 转码播放
  const alive = useRef(true);
  const pollTimer = useRef<number | null>(null);

  const stopPoll = () => {
    if (pollTimer.current !== null) {
      window.clearInterval(pollTimer.current);
      pollTimer.current = null;
    }
  };

  const cur = providers.find((p) => p.id === active);
  const logged = !!cur?.logged;

  const refreshProviders = useCallback(async (): Promise<ProviderInfo[]> => {
    const r = await invoke<ProviderInfo[]>("cloud_providers");
    if (alive.current) setProviders(r);
    return r;
  }, []);

  const loadList = useCallback(
    async (fid: string, cur: string, replace: boolean, provider: string) => {
      setLoading(true);
      setErr("");
      try {
        const r =
          provider === "115"
            ? await invoke<{ items: Entry[]; nextCursor: string }>("cloud_list", {
                provider,
                fid: fid || null,
                cursor: cur || null,
              })
            : await invoke<ListResp>("cloud_list", { provider, fid: fid || null, cursor: cur || null });
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

  // 初次挂载：拉厂商状态；当前 tab 已登录则直接列目录
  useEffect(() => {
    alive.current = true;
    refreshProviders()
      .then((list) => {
        const a = list.find((p) => p.id === active);
        if (a?.logged) void loadList("", "", true, active);
      })
      .catch((e) => alive.current && setErr(String(e)));
    return () => {
      alive.current = false;
      stopPoll();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 切 tab：重置浏览态；已登录列目录，未登录起扫码
  const switchProvider = (id: string) => {
    if (id === active) return;
    stopPoll();
    setActive(id);
    setQr(null);
    setQrMsg("");
    setErr("");
    setItems([]);
    setCursor("");
    setCrumbs([{ fid: "", name: "根目录" }]);
    const p = providers.find((x) => x.id === id);
    if (p?.logged) void loadList("", "", true, id);
    else void startQr(id);
  };

  const openDir = (e: Entry) => {
    const nav = e.navId ?? e.fileId;
    setCrumbs((prev) => [...prev, { fid: nav, name: e.name }]);
    void loadList(nav, "", true, active);
  };

  const jumpCrumb = (i: number) => {
    const next = crumbs.slice(0, i + 1);
    setCrumbs(next);
    void loadList(next[next.length - 1].fid, "", true, active);
  };

  const play = async (e: Entry) => {
    setErr("");
    try {
      if (active === "115") {
        await invoke("cloud115_play", { pickcode: e.fileId, name: e.name, hls });
      } else {
        await invoke("cloud_play", { provider: active, fileId: e.fileId, name: e.name });
      }
    } catch (ex) {
      setErr(String(ex));
    }
  };

  const logout = async () => {
    stopPoll();
    await invoke("cloud_logout", { provider: active }).catch(() => {});
    await refreshProviders();
    setItems([]);
    setCrumbs([{ fid: "", name: "根目录" }]);
    void startQr(active);
  };

  return (
    <div className="c115 view-scroll">
      <div className="c115-hd">
        <button className="c115-back" onClick={onBack} title="返回">
          <IcoBackL size={16} strokeWidth={2} />
        </button>
        <h2 className="c115-title">
          <IcoCloudL size={17} strokeWidth={2} /> 网盘接入
        </h2>
        {logged && cur && (
          <>
            <span className="c115-user">{cur.userName ? `已登录：${cur.userName}` : "已登录"}</span>
            <button className="c115-mini" onClick={logout} title="退出登录（清除本机凭据）">
              <IcoLogoutL size={13} strokeWidth={2} /> 退出
            </button>
          </>
        )}
        {logged && active === "115" && (
          <div className="c115-mode" title="编码不兼容或不想占本地 CPU 时用转码播放（115 服务端转 HLS）">
            <button type="button" className={`c115-seg${!hls ? " on" : ""}`} onClick={() => setHls(false)}>
              直链播放
            </button>
            <button type="button" className={`c115-seg${hls ? " on" : ""}`} onClick={() => setHls(true)}>
              转码播放
            </button>
          </div>
        )}
      </div>

      {/* [lc-1810] 厂商 tab 行 */}
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
      </div>

      {err && <p className="c115-err">{err}</p>}

      {providers.length > 0 && !logged && (
        <div className="c115-login">
          {qr ? (
            <>
              <img className="c115-qr" src={qr.qrcode} alt="登录二维码" />
              <p className="c115-qrmsg">{qrMsg || HINTS[active]}</p>
            </>
          ) : (
            <p className="c115-qrmsg">二维码加载中…</p>
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
                <span className="c115-name" title={e.name}>
                  {e.name}
                </span>
                <span className="c115-size">{fmtSize(e.size)}</span>
                {!e.isDir && e.kind !== "other" && (
                  <button
                    className="c115-play"
                    title={active === "115" && hls ? "转码播放（115 服务端转 HLS）" : "直链播放"}
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
          {loading && <p className="c115-dim">加载中…</p>}
          {!loading && items.length === 0 && <p className="c115-dim">空目录（或没有可播放的媒体文件）</p>}
          {cursor && !loading && (
            <button className="c115-more" onClick={() => loadList(crumbs[crumbs.length - 1].fid, cursor, false, active)}>
              加载更多
            </button>
          )}
        </>
      )}
    </div>
  );
}
