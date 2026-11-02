/* [lc-1907d] JavHub 主页面 —— 信息结构对照 JAVCloudHub 的应用侧栏：
   资料库（媒体库 / 演员库 / 厂商库 / 标签库）· 工作区（概览 / 工作台）· 系统（工具 / 关于）。
   原版左侧竖导航改为**顶部横排胶囊**：默认单图标，选中展开标签名（与设置弹层同一套交互）。
   数据：jav_library_all（网盘扫描分流 + 飞牛媒体库扫描都落在这里）；播放走 cloud_play / play_item。 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { imgUrl } from "../../lib/images";
import { JavSettingsModal } from "./JavSettingsModal";
import { JavDetailDrawer, type JavEntryT } from "./JavDetailDrawer";
import {
  Box as IcoWork,
  Building2 as IcoStudio,
  Clapperboard as IcoJavL,
  FolderOpen as IcoFolderL,
  LayoutDashboard as IcoDash,
  LayoutGrid as IcoGridL,
  Settings as IcoGearL,
  Tag as IcoTagL,
  Users as IcoUsersL,
} from "lucide-react";

type JavLibT = { provider: string; label?: string; scannedAtMs: number; count: number; entries: JavEntryT[] };
type ProviderInfo = { id: string; label: string; logged: boolean; userName: string };

type View = "library" | "actors" | "studios" | "tags" | "overview" | "workbench";

/* 分节导航（对照 JAVCloudHub 侧栏的三组：资料库 / 工作区 / 系统） */
const NAV: { group: string; items: { key: View; label: string; icon: typeof IcoGridL }[] }[] = [
  {
    group: "资料库",
    items: [
      { key: "library", label: "媒体库", icon: IcoGridL },
      { key: "actors", label: "演员库", icon: IcoUsersL },
      { key: "studios", label: "厂商库", icon: IcoStudio },
      { key: "tags", label: "标签库", icon: IcoTagL },
    ],
  },
  {
    group: "工作区",
    items: [
      { key: "overview", label: "概览", icon: IcoDash },
      { key: "workbench", label: "工作台", icon: IcoWork },
    ],
  },
];

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

export function JavHubPage({ onBack }: { onBack: () => void }) {
  const [libs, setLibs] = useState<JavLibT[]>([]);
  const [labels, setLabels] = useState<Record<string, string>>({});
  const [active, setActive] = useState<string>("");
  const [view, setView] = useState<View>("library");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState("");
  const [err, setErr] = useState("");
  const [filter, setFilter] = useState(""); // 演员库/厂商库/标签库点卡片 → 媒体库过滤
  const [setOpen, setSetOpen] = useState(false);
  const [detail, setDetail] = useState<JavEntryT | null>(null);
  const [fnosBusy, setFnosBusy] = useState(false);
  const [fnosNote, setFnosNote] = useState("");
  const alive = useRef(true);

  const load = useCallback(async () => {
    try {
      const r = await invoke<{ libs: JavLibT[] }>("jav_library_all");
      if (!alive.current) return;
      setLibs(r.libs);
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

  /* 聚合：演员 / 厂商 / 标签（跨全部盘，从条目统计） */
  const agg = useMemo(() => {
    const actors = new Map<string, { count: number; covers: string[] }>();
    const studios = new Map<string, { count: number; covers: string[] }>();
    const tags = new Map<string, { count: number; covers: string[] }>();
    let size = 0;
    let scraped = 0;
    let pending = 0;
    let failed = 0;
    for (const l of libs) {
      for (const e of l.entries) {
        size += e.files.reduce((a, f) => a + (f.size || 0), 0);
        if (e.status === 1) scraped += 1;
        else if (e.status === 2) failed += 1;
        else pending += 1;
        const cover = e.coverUrl || "";
        for (const a of e.actors) {
          const x = actors.get(a) || { count: 0, covers: [] };
          x.count += 1;
          if (cover && x.covers.length < 4) x.covers.push(cover);
          actors.set(a, x);
        }
        if (e.studio.trim()) {
          const x = studios.get(e.studio) || { count: 0, covers: [] };
          x.count += 1;
          if (cover && x.covers.length < 4) x.covers.push(cover);
          studios.set(e.studio, x);
        }
        for (const t of e.tags) {
          const x = tags.get(t) || { count: 0, covers: [] };
          x.count += 1;
          if (cover && x.covers.length < 4) x.covers.push(cover);
          tags.set(t, x);
        }
      }
    }
    const sortAgg = (m: Map<string, { count: number; covers: string[] }>) =>
      [...m.entries()].sort((a, b) => b[1].count - a[1].count);
    return { actors: sortAgg(actors), studios: sortAgg(studios), tags: sortAgg(tags), size, scraped, pending, failed };
  }, [libs]);

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

  const scrapeAll = async () => {
    if (busy) return;
    setBusy(true);
    setNote("正在按番号刮削全部盘…");
    setErr("");
    try {
      let ok = 0;
      let bad = 0;
      for (const l of libs) {
        try {
          const r = await invoke<{ scraped: number; failed: number }>("jav_library_scrape", { provider: l.provider, force: false });
          ok += r.scraped;
          bad += r.failed;
        } catch {
          /* 单盘失败不挡其余盘 */
        }
      }
      setNote(`全部刮削完成：成功 ${ok}，失败 ${bad}`);
      await load();
    } finally {
      setBusy(false);
    }
  };

  const scanFnos = async () => {
    if (fnosBusy) return;
    setFnosBusy(true);
    setNote("正在扫描启用的飞牛媒体库（翻页读取 + 番号识别）…");
    try {
      const r = await invoke<{ results: { title?: string; error?: string; items?: number; javDetected?: number }[] }>("jav_fnos_scan");
      const parts = r.results.map((x) => (x.error ? `${x.title ?? ""} 失败` : `${x.title} ${x.javDetected}/${x.items} 条命中番号`));
      setFnosNote(`扫完了：${parts.join("，")}`);
      await load();
    } catch (e) {
      setFnosNote(String(e));
    } finally {
      setFnosBusy(false);
    }
  };

  const scanLocal = async () => {
    if (fnosBusy) return;
    setFnosBusy(true);
    setNote("正在扫描本地文件夹（遍历 + 番号识别）…");
    try {
      const r = await invoke<{ results: { dir: string; videos: number; javDetected: number }[] }>("jav_local_scan");
      const parts = r.results.map((x) => `${x.dir.split(/[\/]/).pop()} ${x.javDetected}/${x.videos} 条命中番号`);
      setNote(`扫完了：${parts.join("，")}（刮削在媒体库页点「刮削本盘」）`);
      await load();
    } catch (e) {
      setNote(String(e));
    } finally {
      setFnosBusy(false);
    }
  };

  const play = async (e: JavEntryT) => {
    const f = e.files[0];
    if (!f) return;
    setErr("");
    try {
      if (f.provider.startsWith("fnos:")) {
        // 飞牛媒体库来源：走既有 NAS 播放链路（play/info → mpv，带进度回传）
        await invoke("play_item", { guid: f.fileId });
      } else if (f.provider.startsWith("local:")) {
        // 本地文件夹来源：绝对路径直投 mpv
        await invoke("jav_local_play", { path: f.fileId });
      } else {
        await invoke("cloud_play", { provider: f.provider, fileId: f.fileId, name: f.name, mode: null });
      }
    } catch (ex) {
      setErr(String(ex));
    }
  };

  const matchFilter = (e: JavEntryT) => {
    if (!filter) return true;
    const q = filter.toLowerCase();
    return (
      e.actors.some((a) => a.toLowerCase() === q) ||
      e.studio.toLowerCase() === q ||
      e.tags.some((t) => t.toLowerCase() === q)
    );
  };
  const entries = (cur?.entries || []).filter((e) => e.category !== "unknown").filter(matchFilter);

  // 海报墙卡片（与媒体库海报墙同观感：竖版海报 + 标题 + meta）
  const mediaCards = entries.length
    ? entries.map((e) => (
        <div key={e.codeKey} className="jvh-card" onClick={() => setDetail(e)}>
          <div className="jvh-poster">
            {e.coverUrl ? (
              <img src={imgUrl(e.coverUrl)} alt="" loading="lazy" />
            ) : (
              <span className="jvh-ph">
                <IcoJavL size={26} strokeWidth={1.4} />
              </span>
            )}
            {e.status === 0 && <i className="jvh-badge">待刮削</i>}
            {e.status === 2 && <i className="jvh-badge warn">失败</i>}
          </div>
          <p className="jvh-title" title={e.titleZh || e.title || e.code}>
            {e.titleZh || e.title || e.code}
          </p>
          <p className="jvh-meta">
            {e.categoryLabel}
            {e.releaseDate ? ` ${e.releaseDate.slice(0, 7)}` : ""}
            {e.studio ? ` ${e.studio}` : ""}
          </p>
        </div>
      ))
    : null;

  /* 聚合卡片（演员 / 厂商 / 标签通用）：点击 → 媒体库按该项过滤 */
  const aggCards = (list: [string, { count: number; covers: string[] }][], emptyText: string) =>
    list.length ? (
      <div className="jvh-grid">
        {list.map(([name, x]) => (
          <div key={name} className="jvh-card" onClick={() => { setFilter(name); setView("library"); }}>
            <div className="jvh-poster">
              {x.covers[0] ? (
                <img src={imgUrl(x.covers[0])} alt="" loading="lazy" />
              ) : (
                <span className="jvh-ph">
                  <IcoJavL size={26} strokeWidth={1.4} />
                </span>
              )}
            </div>
            <p className="jvh-title" title={name}>{name}</p>
            <p className="jvh-meta">{x.count} 部</p>
          </div>
        ))}
      </div>
    ) : (
      <div className="c115-empty">
        <IcoFolderL size={34} strokeWidth={1.4} />
        <p>{emptyText}</p>
      </div>
    );

  const stat = (label: string, v: string) => (
    <div className="jvh-stat">
      <b>{v}</b>
      <span>{label}</span>
    </div>
  );

  return (
    <main className="content">
      {/* 头部与媒体库页同款：圆返回 + 标题 + 计数 */}
      <div className="page-head">
        <button className="back" onClick={onBack}>‹</button>
        <h3>JavHub</h3>
        <span className="muted">{total} 部</span>
      </div>
      {/* 第二段：分节胶囊 —— 直接复用库页筛选条原生类（libf-bar / libf-open），样式与「筛选/最近添加」完全一致 */}
      <div className="libf-bar">
        {NAV.flatMap((g) => g.items).map((x) => {
          const Ico = x.icon;
          const on = view === x.key;
          return (
            <button key={x.key} type="button" title={x.label}
              className={`libf-open${on ? " on" : ""}`}
              onClick={() => {
                setView(x.key);
                setFilter("");
              }}>
              <Ico size={13} strokeWidth={2} />
              {x.label}
            </button>
          );
        })}
        <button type="button" title="设置" className="libf-open" onClick={() => setSetOpen(true)}>
          <IcoGearL size={13} strokeWidth={2} />
          设置
        </button>
      </div>

      {err && <p className="c115-err">{err}</p>}
      {note && <p className="c115-dim" style={{ margin: "4px 0 0" }}>{note}</p>}

      {loading && <div className="c115-loading">加载中…</div>}

      {!loading && view === "library" && (
        <>
          {libs.length > 0 && (
            <div className="c115-tabs">
              {libs.map((l) => (
                <button key={l.provider} type="button" className={`c115-tab${l.provider === active ? " on" : ""}`}
                  onClick={() => setActive(l.provider)}>
                  {l.label || labels[l.provider] || l.provider}
                  <span className="c115-dot ok" title={`${l.count} 部`} />
                </button>
              ))}
              <button className="c115-mini" disabled={busy} style={{ marginLeft: "auto" }} onClick={() => void scrape()}>
                {busy ? "刮削中…" : "刮削本盘"}
              </button>
              {filter && (
                <button className="c115-mini" onClick={() => setFilter("")}>清除筛选：{filter} ×</button>
              )}
            </div>
          )}
          {mediaCards ? (
            <div className="jvh-grid">{mediaCards}</div>
          ) : (
            <div className="c115-empty">
              <IcoFolderL size={34} strokeWidth={1.4} />
              <p>
                {filter
                  ? `没有 ${filter} 的条目`
                  : "还没有条目 —— 扫描/刮削后，识别到番号的视频会以海报墙展示在这里"}
              </p>
            </div>
          )}
        </>
      )}

      {!loading && view === "actors" && aggCards(agg.actors, "还没有演员数据 —— 先在媒体库把条目刮削好")}
      {!loading && view === "studios" && aggCards(agg.studios, "还没有厂商数据 —— 先在媒体库把条目刮削好")}
      {!loading && view === "tags" && aggCards(agg.tags, "还没有标签数据 —— 先在媒体库把条目刮削好")}

      {!loading && view === "overview" && (
        <>
          <div className="jvh-stats">
            {stat("条目", String(total))}
            {stat("已刮削", String(agg.scraped))}
            {stat("待刮削", String(agg.pending))}
            {stat("失败", String(agg.failed))}
            {stat("总大小", fmtSize(agg.size))}
            {stat("来源盘", String(libs.length))}
          </div>
          <div className="javset-ro">
            {libs.map((l) => `${l.label || labels[l.provider] || l.provider}：${l.count} 部`).join("，") || "还没有数据"}
          </div>
        </>
      )}

      {!loading && view === "workbench" && (
        <>
          <div className="javset-item boolrow" style={{ cursor: "default" }}>
            <label><span className="bl">扫描飞牛媒体库</span></label>
            <span>
              <button className="javset-btn ghost" disabled={fnosBusy} onClick={() => void scanFnos()}>
                {fnosBusy ? "扫描中…" : "扫描启用的媒体库"}
              </button>
              <div className="hint">对设置 → 数据源里启用的飞牛媒体库做番号识别（元数据只叠加在本机，不写飞牛）。</div>
              {fnosNote && <div className="hint">{fnosNote}</div>}
            </span>
          </div>
          <div className="javset-item boolrow" style={{ cursor: "default" }}>
            <label><span className="bl">扫描本地文件夹</span></label>
            <span>
              <button className="javset-btn ghost" disabled={fnosBusy} onClick={() => void scanLocal()}>
                {fnosBusy ? "扫描中…" : "扫描本地文件夹"}
              </button>
              <div className="hint">对设置 → 数据源里添加的本地视频文件夹做番号识别（只读遍历，不改本地文件）。</div>
              {fnosNote && <div className="hint">{fnosNote}</div>}
            </span>
          </div>
          <div className="javset-item boolrow" style={{ cursor: "default" }}>
            <label><span className="bl">批量刮削</span></label>
            <span>
              <button className="javset-btn ghost" disabled={busy} onClick={() => void scrapeAll()}>
                {busy ? "刮削中…" : "刮削全部盘"}
              </button>
              <div className="hint">逐盘跑增量刮削（已成功的跳过）；单盘失败不影响其余盘。走镜像轨免梯子，mgstage / heyzo 需要代理。</div>
            </span>
          </div>
        </>
      )}

      {detail && (
        <JavDetailDrawer
          provider={active}
          entry={detail}
          onClose={() => setDetail(null)}
          onPlay={(e) => void play(e)}
          onFilter={(name) => {
            setFilter(name);
            setView("library");
          }}
          onUpdated={(ne) => {
            // 单条刮削/翻译后原地同步（列表 + 当前抽屉）
            setLibs((prev) =>
              prev.map((l) =>
                l.provider === active
                  ? { ...l, entries: l.entries.map((x) => (x.codeKey === ne.codeKey ? ne : x)) }
                  : l,
              ),
            );
          }}
        />
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
