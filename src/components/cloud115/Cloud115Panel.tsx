/* [lc-1779] 115 网盘面板：扫码登录 → 文件浏览 → 播放。
   · 登录：Rust 侧 cloud115_qr_start 出二维码图片 URL（<img> 直显），
     cloud115_qr_poll 2s 轮询；Cookie 只落本机 app_data_dir/cloud115.json。
   · 浏览：cloud115_list 分页列目录（文件夹 + 视频/音频）。
   · 播放：cloud115_play —— 默认直链；「转码播放」走 115 服务端转码 HLS
     （编码不兼容 / 不想占本地 CPU 时用）。播放同直播口径：不回传飞牛进度。 */
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

type Entry = {
  fileId: string;
  name: string;
  isDir: boolean;
  size: number;
  pickcode: string;
  kind: "dir" | "video" | "audio";
};

type ListResp = {
  cid: string;
  count: number;
  offset: number;
  hasMore: boolean;
  items: Entry[];
};

type Crumb = { cid: string; name: string };

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

export function Cloud115Panel({ onBack }: { onBack: () => void }) {
  const [logged, setLogged] = useState<boolean | null>(null);
  const [userName, setUserName] = useState("");
  const [qr, setQr] = useState<{ qrcode: string; uid: string; time: string; sign: string } | null>(null);
  const [qrMsg, setQrMsg] = useState("打开 115 生活 App（或 115 App）扫一扫登录");
  const [crumbs, setCrumbs] = useState<Crumb[]>([{ cid: "", name: "根目录" }]);
  const [items, setItems] = useState<Entry[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState("");
  const [hls, setHls] = useState(false);
  const alive = useRef(true);
  const pollTimer = useRef<number | null>(null);

  const stopPoll = () => {
    if (pollTimer.current !== null) {
      window.clearInterval(pollTimer.current);
      pollTimer.current = null;
    }
  };

  const loadList = useCallback(async (cid: string, offset: number, replace: boolean) => {
    setLoading(true);
    setErr("");
    try {
      const r = await invoke<ListResp>("cloud115_list", { cid: cid || null, offset });
      if (!alive.current) return;
      setItems((prev) => (replace ? r.items : [...prev, ...r.items]));
      setHasMore(!!r.hasMore);
    } catch (e) {
      if (alive.current) setErr(String(e));
    } finally {
      if (alive.current) setLoading(false);
    }
  }, []);

  const startQr = useCallback(async () => {
    setErr("");
    setQrMsg("正在获取二维码…");
    stopPoll();
    try {
      const r = await invoke<{ qrcode: string; uid: string; time: string; sign: string }>(
        "cloud115_qr_start",
      );
      if (!alive.current) return;
      setQr(r);
      setQrMsg("打开 115 生活 App（或 115 App）扫一扫登录");
      pollTimer.current = window.setInterval(async () => {
        try {
          const p = await invoke<{ status: number; ok: boolean; userName?: string }>(
            "cloud115_qr_poll",
            { uid: r.uid, time: r.time, sign: r.sign },
          );
          if (!alive.current) return stopPoll();
          if (p.ok) {
            stopPoll();
            setLogged(true);
            setUserName(p.userName || "");
            void loadList("", 0, true);
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
  }, [loadList]);

  useEffect(() => {
    alive.current = true;
    invoke<{ logged: boolean; userName: string }>("cloud115_status")
      .then((s) => {
        if (!alive.current) return;
        setLogged(s.logged);
        setUserName(s.userName || "");
        if (s.logged) void loadList("", 0, true);
        else void startQr();
      })
      .catch((e) => {
        if (alive.current) {
          setLogged(false);
          setErr(String(e));
        }
      });
    return () => {
      alive.current = false;
      stopPoll();
    };
  }, [startQr, loadList]);

  const openDir = (e: Entry) => {
    setCrumbs((prev) => [...prev, { cid: e.fileId, name: e.name }]);
    void loadList(e.fileId, 0, true);
  };

  const jumpCrumb = (i: number) => {
    const next = crumbs.slice(0, i + 1);
    setCrumbs(next);
    void loadList(next[next.length - 1].cid, 0, true);
  };

  const play = async (e: Entry) => {
    setErr("");
    try {
      await invoke("cloud115_play", { pickcode: e.pickcode, name: e.name, hls });
    } catch (ex) {
      setErr(String(ex));
    }
  };

  const logout = async () => {
    await invoke("cloud115_logout").catch(() => {});
    setLogged(false);
    setUserName("");
    setItems([]);
    setCrumbs([{ cid: "", name: "根目录" }]);
    void startQr();
  };

  return (
    <div className="c115 view-scroll">
      <div className="c115-hd">
        <button className="c115-back" onClick={onBack} title="返回">
          <IcoBackL size={16} strokeWidth={2} />
        </button>
        <h2 className="c115-title">
          <IcoCloudL size={17} strokeWidth={2} /> 115 网盘
        </h2>
        {logged && (
          <>
            <span className="c115-user">{userName ? `已登录：${userName}` : "已登录"}</span>
            <button className="c115-mini" onClick={logout} title="退出登录（清除本机 Cookie）">
              <IcoLogoutL size={13} strokeWidth={2} /> 退出
            </button>
          </>
        )}
        {logged && (
          <div className="c115-mode" title="编码不兼容或不想占本地 CPU 时用转码播放（115 服务端转 HLS）">
            <button
              type="button"
              className={`c115-seg${!hls ? " on" : ""}`}
              onClick={() => setHls(false)}
            >
              直链播放
            </button>
            <button
              type="button"
              className={`c115-seg${hls ? " on" : ""}`}
              onClick={() => setHls(true)}
            >
              转码播放
            </button>
          </div>
        )}
      </div>

      {err && <p className="c115-err">{err}</p>}

      {logged === false && (
        <div className="c115-login">
          {qr ? (
            <>
              <img className="c115-qr" src={qr.qrcode} alt="115 登录二维码" />
              <p className="c115-qrmsg">{qrMsg}</p>
            </>
          ) : (
            <p className="c115-qrmsg">二维码加载中…</p>
          )}
          <button className="c115-mini" onClick={startQr}>
            <IcoRefreshL size={13} strokeWidth={2} /> 刷新二维码
          </button>
          <p className="c115-dim">
            登录凭据只保存在本机（app_data_dir/cloud115.json），不会上传到任何服务器。
          </p>
        </div>
      )}

      {logged && (
        <>
          <div className="c115-crumbs">
            {crumbs.map((c, i) => (
              <span key={`${c.cid}-${i}`} className="c115-crumb">
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
                {!e.isDir && (
                  <button
                    className="c115-play"
                    title={hls ? "转码播放（115 服务端转 HLS）" : "直链播放"}
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
          {hasMore && !loading && (
            <button className="c115-more" onClick={() => loadList(crumbs[crumbs.length - 1].cid, items.length, false)}>
              加载更多
            </button>
          )}
        </>
      )}
    </div>
  );
}
