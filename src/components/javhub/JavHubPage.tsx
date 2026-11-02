/* [lc-1907] JavHub 独立页：番号识别 + 番号刮削的条目都在这里展示。
   数据来自 jav_library_all（jav-library.json，由「网盘接入」页扫描时自动分流）；
   播放复用 cloud_play 直链续签；封面走 fntvimg /__cover/ 本机协议。
   网格/卡片样式与 CloudPanel 媒体库同套（c115-*），保证观感一致。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { imgUrl } from "../../lib/images";
import {
  Clapperboard as IcoJavL,
  FolderOpen as IcoFolderL,
  Play as IcoPlayL,
  RefreshCw as IcoRefreshL,
  Settings as IcoGearL,
} from "lucide-react";
import { JavSettingsModal } from "./JavSettingsModal";

type JavFileT = { provider: string; fileId: string; name: string; size: number; part: number };
type JavEntryT = {
  code: string;
  codeKey: string;
  category: string;
  categoryLabel: string;
  status: number; // 0 待刮削 1 已刮削 2 失败
  title: string;
  titleZh: string;
  releaseDate: string;
  studio: string;
  coverUrl: string;
  files: JavFileT[];
};
type JavLibT = { provider: string; scannedAtMs: number; count: number; entries: JavEntryT[] };
type ProviderInfo = { id: string; label: string; logged: boolean; userName: string };

export function JavHubPage({ onBack }: { onBack: () => void }) {
  const [libs, setLibs] = useState<JavLibT[]>([]);
  const [labels, setLabels] = useState<Record<string, string>>({});
  const [active, setActive] = useState<string>("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState("");
  const [err, setErr] = useState("");
  const [setOpen, setSetOpen] = useState(false);
  const alive = useRef(true);

  const load = useCallback(async () => {
    try {
      const r = await invoke<{ libs: JavLibT[] }>("jav_library_all");
      if (!alive.current) return;
      setLibs(r.libs);
      // 默认落在第一个有内容的盘
      setActive((prev) => (prev && r.libs.some((l) => l.provider === prev) ? prev : (r.libs[0]?.provider ?? "")));
    } catch (e) {
      if (alive.current) setErr(String(e));
    } finally {
      if (alive.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void load();
    // 盘的显示名（夸克/阿里/115…）从 cloud_providers 拿
    invoke<ProviderInfo[]>("cloud_providers")
      .then((ps) => {
        if (!alive.current) return;
        const m: Record<string, string> = {};
        for (const p of ps) m[p.id] = p.label;
        setLabels(m);
      })
      .catch(() => {});
    return () => {
      alive.current = false;
    };
  }, [load]);

  const cur = libs.find((l) => l.provider === active);
  const total = libs.reduce((a, l) => a + l.count, 0);

  const scrape = async () => {
    if (!active || busy) return;
    setBusy(true);
    setNote("正在按番号刮削（r18.dev / jav321 / FC2，mgstage 与 heyzo 需要代理）…");
    setErr("");
    try {
      const r = await invoke<{ scraped: number; failed: number; skipped: number }>("jav_library_scrape", {
        provider: active,
        force: false,
      });
      setNote(`刮削完成：成功 ${r.scraped}，失败 ${r.failed}（失败的可再点一次重试）`);
      await load();
    } catch (ex) {
      setNote("");
      setErr(String(ex));
    } finally {
      setBusy(false);
    }
  };

  const play = async (e: JavEntryT) => {
    const f = e.files[0];
    if (!f) return;
    setErr("");
    try {
      await invoke("cloud_play", { provider: f.provider, fileId: f.fileId, name: f.name, mode: null });
    } catch (ex) {
      setErr(String(ex));
    }
  };

  return (
    <main className="c115">
      <div className="page-head">
        <button className="back" onClick={onBack}>‹ 返回</button>
        <h3 className="c115-title">
          <span className="c115-title-ico">
            <IcoJavL size={16} strokeWidth={2.2} />
          </span>
          JavHub
        </h3>
        <button
          className="c115-mini"
          style={{ marginLeft: "auto" }}
          title="扫描规则 / 定时扫描 / 网络与代理 / 刮削站点 / 内置 AI"
          onClick={() => setSetOpen(true)}
        >
          <IcoGearL size={13} strokeWidth={2} /> 设置
        </button>
      </div>

      {/* 盘 tab（只显示已有 JAV 库的盘） */}
      {libs.length > 0 && (
        <div className="c115-tabs">
          {libs.map((l) => (
            <button
              key={l.provider}
              type="button"
              className={`c115-tab${l.provider === active ? " on" : ""}`}
              onClick={() => setActive(l.provider)}
            >
              {labels[l.provider] || l.provider}
              <span className="c115-dot ok" title={`${l.count} 部`} />
            </button>
          ))}
        </div>
      )}

      {cur && (
        <div className="c115-bar">
          <span className="c115-user">共 {cur.count} 部（全库 {total}）</span>
          <button className="c115-mini" disabled={busy} title="按番号刮削元数据与封面（增量：已成功的跳过）" onClick={() => void scrape()}>
            {busy ? "刮削中…" : "刮削本盘"}
          </button>
        </div>
      )}
      {note && <p className="c115-dim" style={{ margin: "4px 0 0" }}>{note}</p>}
      {err && <p className="c115-err">{err}</p>}

      {loading && <div className="c115-loading">加载中…</div>}

      {!loading && cur && (
        <div className="c115-grid">
          {cur.entries.map((e) => (
            <div key={e.codeKey} className="c115-item" onClick={() => e.files[0] && void play(e)}>
              {e.coverUrl ? (
                <img className="c115-libposter" src={imgUrl(e.coverUrl)} alt="" loading="lazy" />
              ) : (
                <span className="c115-ico video">
                  <IcoJavL size={15} strokeWidth={1.8} />
                </span>
              )}
              <span className="c115-name" title={e.titleZh || e.title || e.code}>
                {e.titleZh || e.title || e.code}
              </span>
              <span className="c115-size">
                {e.categoryLabel}
                {e.releaseDate ? ` ${e.releaseDate.slice(0, 7)}` : ""}
                {` ${e.files.length} 段`}
                {e.status === 0 ? " 待刮削" : e.status === 2 ? " 刮削失败" : ""}
              </span>
              <button
                className="c115-play"
                title="播放（直链自动续签）"
                onClick={(ev) => {
                  ev.stopPropagation();
                  if (e.files[0]) void play(e);
                }}
              >
                <IcoPlayL size={13} strokeWidth={2.2} />
              </button>
            </div>
          ))}
          {cur.entries.length === 0 && (
            <div className="c115-empty">
              <IcoRefreshL size={34} strokeWidth={1.4} />
              <p>这个盘还没有识别到番号 —— 回「网盘接入」重新扫描</p>
            </div>
          )}
        </div>
      )}

      {!loading && libs.length === 0 && (
        <div className="c115-empty">
          <IcoFolderL size={34} strokeWidth={1.4} />
          <p>还没有 JAV 库 —— 去「网盘接入」进到媒体目录点「加入媒体库」，扫描时番号文件会自动归到这里</p>
        </div>
      )}
      {setOpen && (
        <JavSettingsModal
          onClose={() => setSetOpen(false)}
          onSaved={() => {
            void load();
          }}
        />
      )}
    </main>
  );
}
