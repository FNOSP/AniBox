/* [lc-1907e] JavHub 详情抽屉 —— 布局对照 JAVCloudHub 的 entry.js 抽屉：
   右侧滑出面板：大封面 → 状态 chips + 看过/日期 → 番号（复制/翻译）+ 标题 →
   立即播放（体积）+ 重刮 → 错误横幅 → 演员/厂商系列卡 → 标签 chips → 评分/身材信息。
   动作全部接 AniBox 既有链路：播放 cloud_play/play_item、单条重刮 jav_entry_scrape、
   单条翻译 jav_ai_translate。Esc / 点遮罩关闭。 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { imgUrl } from "../../lib/images";
import { Clipboard as IcoCopy, Languages as IcoLang, RefreshCw as IcoRefreshL, X as IcoX } from "lucide-react";

type JavFileT = { provider: string; fileId: string; name: string; size: number; part: number };
export type JavEntryT = {
  code: string;
  codeKey: string;
  category: string;
  categoryLabel: string;
  status: number;
  title: string;
  titleZh: string;
  actors: string[];
  tags: string[];
  releaseDate: string;
  studio: string;
  series: string;
  summary: string;
  runtime: number;
  director: string;
  rating: number;
  votes: number;
  userRating: number;
  cup: string;
  age: number;
  height: number;
  coverUrl: string;
  lastError: string;
  files: JavFileT[];
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

export function JavDetailDrawer({
  provider,
  entry,
  onClose,
  onPlay,
  onFilter,
  onUpdated,
}: {
  provider: string;
  entry: JavEntryT;
  onClose: () => void;
  onPlay: (e: JavEntryT) => void;
  onFilter: (name: string) => void;
  onUpdated: (e: JavEntryT) => void;
}) {
  const [e, setE] = useState<JavEntryT>(entry);
  const [busy, setBusy] = useState("");
  const [note, setNote] = useState("");
  useEffect(() => setE(entry), [entry]);
  useEffect(() => {
    const h = (ev: KeyboardEvent) => ev.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);

  const totalSize = e.files.reduce((a, f) => a + (f.size || 0), 0);
  const title = e.titleZh || e.title || e.code;

  const copyCode = async () => {
    try {
      await navigator.clipboard.writeText(e.code);
      setNote("番号已复制");
      window.setTimeout(() => setNote(""), 1600);
    } catch {
      /* 剪贴板权限失败静默 */
    }
  };

  const rescrape = async () => {
    if (busy) return;
    setBusy("scrape");
    setNote("");
    try {
      const r = await invoke<{ ok: boolean; entry: JavEntryT }>("jav_entry_scrape", {
        provider,
        codeKey: e.codeKey,
      });
      setE({ ...e, ...r.entry });
      setNote(r.ok ? "刮削成功" : "刮削失败（可再试或换站点）");
      onUpdated(r.entry);
    } catch (ex) {
      setNote(String(ex));
    } finally {
      setBusy("");
    }
  };

  const translate = async () => {
    if (busy) return;
    setBusy("translate");
    setNote("");
    try {
      const r = await invoke<{ translated: number }>("jav_ai_translate", {
        provider,
        codeKey: e.codeKey,
      });
      if (r.translated > 0) {
        // 重拉一次条目（翻译结果在后端库里）
        const rr = await invoke<{ libs: { provider: string; entries: JavEntryT[] }[] }>("jav_library_all");
        const lib = rr.libs.find((l) => l.provider === provider);
        const ne = lib?.entries.find((x) => x.codeKey === e.codeKey);
        if (ne) {
          setE({ ...e, ...ne });
          onUpdated(ne);
        }
        setNote("已翻译");
      } else {
        setNote("AI 没有返回译名，稍后再试");
      }
    } catch (ex) {
      setNote(String(ex));
    } finally {
      setBusy("");
    }
  };

  const rate = async (v: number) => {
    const next = Math.round(e.userRating) === v ? 0 : v; // 再点同一档 = 清除
    setE({ ...e, userRating: next });
    try {
      await invoke("jav_entry_rate", { provider, codeKey: e.codeKey, value: next });
    } catch (ex) {
      setNote(String(ex));
    }
  };

  const filterBy = (name: string) => {
    onFilter(name);
    onClose();
  };

  return (
    <div className="jvd-scrim" onClick={onClose}>
      <aside className="jvd" onClick={(ev) => ev.stopPropagation()}>
        <button className="jvd-x" onClick={onClose} aria-label="关闭">
          <IcoX size={17} strokeWidth={2.2} />
        </button>

        {/* 大封面 */}
        <div className="jvd-cover">
          {e.coverUrl ? (
            <img src={imgUrl(e.coverUrl)} alt="" />
          ) : (
            <div className="jvd-cover-ph">暂无封面</div>
          )}
        </div>

        <div className="jvd-body">
          {/* chips 行 */}
          <div className="jvd-chips">
            <span className="jvd-chip cat">{e.categoryLabel}</span>
            {e.status === 0 && <span className="jvd-chip">待刮削</span>}
            {e.status === 2 && <span className="jvd-chip err">刮削失败</span>}
            {e.status === 1 && <span className="jvd-chip ok">已刮削</span>}
            <span className="jvd-spacer" />
            <span className="jvd-dim">
              {e.runtime > 0 ? `${e.runtime} 分钟` : ""}
              {e.releaseDate ? ` ${e.releaseDate}` : ""}
            </span>
          </div>

          {/* 番号 + 标题 */}
          <div className="jvd-coderow">
            <b className="jvd-code">{e.code}</b>
            <button className="jvd-icobtn" title="复制番号" onClick={() => void copyCode()}>
              <IcoCopy size={14} strokeWidth={1.9} />
            </button>
            <button className="jvd-icobtn" title="AI 翻译标题" disabled={busy !== ""} onClick={() => void translate()}>
              <IcoLang size={14} strokeWidth={1.9} />
            </button>
          </div>
          {title !== e.code && <p className="jvd-title">{title}</p>}

          {/* 播放大按钮 */}
          <button className="jvd-play" disabled={e.files.length === 0} onClick={() => onPlay(e)}>
            ▶ 立即播放 {totalSize > 0 ? fmtSize(totalSize) : ""}
          </button>
          {e.files.length > 1 && (
            <div className="jvd-parts">
              {e.files.map((f) => (
                <button key={f.fileId} className="jvd-part" onClick={() => onPlay({ ...e, files: [f] })}>
                  第 {f.part} 段 · {f.name}
                </button>
              ))}
            </div>
          )}

          {/* 错误横幅 */}
          {e.status === 2 && e.lastError && <div className="jvd-err">⚠ {e.lastError}</div>}

          {/* 演员 / 厂商系列 */}
          <div className="jvd-grid2">
            <div className="jvd-card">
              <div className="jvd-k">出演演员</div>
              <div className="jvd-chips2">
                {e.actors.length ? (
                  e.actors.map((a) => (
                    <button key={a} className="jvd-chip link" onClick={() => filterBy(a)}>{a}</button>
                  ))
                ) : (
                  <span className="jvd-dim">—</span>
                )}
              </div>
            </div>
            <div className="jvd-card">
              <div className="jvd-k">制片厂商 / 系列</div>
              <div className="jvd-chips2">
                {e.studio && <button className="jvd-chip link" onClick={() => filterBy(e.studio)}>{e.studio}</button>}
                {e.series && <span className="jvd-chip">{e.series}</span>}
                {!e.studio && !e.series && <span className="jvd-dim">—</span>}
              </div>
            </div>
          </div>

          {/* 标签 */}
          {e.tags.length > 0 && (
            <div className="jvd-card">
              <div className="jvd-k">影片标签</div>
              <div className="jvd-chips2">
                {e.tags.map((t) => (
                  <button key={t} className="jvd-chip" onClick={() => filterBy(t)}>{t}</button>
                ))}
              </div>
            </div>
          )}

          {/* 我的评分（10 星，点一下打分，再点同档清除） */}
          <div className="jvd-card">
            <div className="jvd-krow">
              <span className="jvd-k">我的评分</span>
              <span className="jvd-stars">
                {[1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map((v) => (
                  <button key={v} type="button" title={`${v} 分`}
                    className={`jvd-star${Math.round(e.userRating) >= v ? " on" : ""}`}
                    onClick={() => void rate(v)}>
                    ★
                  </button>
                ))}
                <span className="jvd-dim" style={{ marginLeft: 6 }}>
                  {e.userRating > 0 ? `${e.userRating} 分` : "点一下打分"}
                </span>
              </span>
            </div>
          </div>

          {/* 媒体文件 */}
          {e.files.length > 0 && (
            <div className="jvd-card">
              <div className="jvd-k">媒体文件</div>
              {e.files.map((f) => (
                <div key={f.fileId} className="jvd-file">
                  <span className="jvd-fname" title={f.name}>{f.name}</span>
                  <span className="jvd-dim">{fmtSize(f.size)}</span>
                </div>
              ))}
              {e.releaseDate && (
                <div className="jvd-k" style={{ marginTop: 6 }}>发行 {e.releaseDate}</div>
              )}
            </div>
          )}

          {/* 简介 / 身材 / 评分 */}
          {(e.summary || e.rating > 0 || e.cup || e.height > 0) && (
            <div className="jvd-card">
              {e.rating > 0 && (
                <div className="jvd-krow">
                  <span className="jvd-k">站点评分</span>
                  <span className="jvd-rate">
                    ★ {e.rating.toFixed(1)}
                    {e.votes > 0 ? `（${e.votes} 人评）` : ""}
                  </span>
                </div>
              )}
              {(e.cup || e.height > 0 || e.age > 0) && (
                <div className="jvd-krow">
                  <span className="jvd-k">身材信息</span>
                  <span className="jvd-dim">
                    {[e.cup ? `${e.cup} 罩杯` : "", e.height > 0 ? `${e.height}cm` : "", e.age > 0 ? `${e.age} 岁` : ""]
                      .filter(Boolean)
                      .join(" · ")}
                  </span>
                </div>
              )}
              {e.summary && <p className="jvd-summary">{e.summary}</p>}
            </div>
          )}

          {note && <div className="jvd-note">{note}</div>}
          {busy && <div className="jvd-note">{busy === "scrape" ? "正在重新刮削…" : "正在翻译…"}</div>}
        </div>

        {/* 悬浮重刮按钮（右下） */}
        <button className="jvd-rescrape" title="重新刮削" disabled={busy !== ""} onClick={() => void rescrape()}>
          <IcoRefreshL size={16} strokeWidth={2} />
        </button>
      </aside>
    </div>
  );
}
