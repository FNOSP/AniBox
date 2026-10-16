/* [lc-1589] 「总表查询」弹窗：选年份 + 季度 → 看该季真实番表。
   数据源 = **AniList 官方 API**（Rust 命令 `anilist_season`，与「当日放送表」同一条链路；
   往季基本定型、按季度缓存 24h）。外壳复用仓库既有的 `.ddet-mask` 遮罩模式（同详情页简介弹窗）。 */
import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CalendarDays as IcoCalL, ChevronDown as IcoChevD, X as IcoXL } from "lucide-react";
import { Dsk } from "./shared";

type SeasonItem = {
  anilistId: number;
  /** [native, romaji, english]（有的为空） */
  titles: string[];
  cover: string;
  episodes: number;
  format: string;
  status: string;
  /** AniList 百分制（0 = 无） */
  score: number;
  genres: string[];
};

type SeasonKey = "WINTER" | "SPRING" | "SUMMER" | "FALL";

const SEASONS: { k: SeasonKey; label: string }[] = [
  { k: "WINTER", label: "1月番" },
  { k: "SPRING", label: "4月番" },
  { k: "SUMMER", label: "7月番" },
  { k: "FALL", label: "10月番" },
];

/** 按当前月份推「当前季」（1-3 月 = 1月番，以此类推） */
function currentSeason(d = new Date()): SeasonKey {
  const m = d.getMonth() + 1;
  return m <= 3 ? "WINTER" : m <= 6 ? "SPRING" : m <= 9 ? "SUMMER" : "FALL";
}

const FORMAT_CN: Record<string, string> = {
  TV: "TV",
  TV_SHORT: "TV短篇",
  MOVIE: "剧场版",
  SPECIAL: "特别篇",
  OVA: "OVA",
  ONA: "网络动画",
  MUSIC: "音乐",
};
const STATUS_CN: Record<string, string> = {
  FINISHED: "已完结",
  RELEASING: "连载中",
  NOT_YET_RELEASED: "未开播",
  CANCELLED: "已取消",
  HIATUS: "休止",
};

export function SeasonArchive({ onClose }: { onClose: () => void }) {
  const now = new Date();
  const [year, setYear] = useState(now.getFullYear());
  const [season, setSeason] = useState<SeasonKey>(() => currentSeason(now));
  const [items, setItems] = useState<SeasonItem[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  // [lc-1591] 年份用**自定义面板**（原生 <select> 的展开列表没法美化）；打开时点别处/Esc 收起
  const [yearOpen, setYearOpen] = useState(false);

  const years = useMemo(() => {
    const out: number[] = [];
    for (let y = now.getFullYear(); y >= 2000; y--) out.push(y);
    return out;
    // 弹窗打开期间年份列表不需要变
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    let alive = true;
    setBusy(true);
    setErr("");
    invoke<{ items?: SeasonItem[] }>("anilist_season", { year, season })
      .then((d) => {
        if (alive) setItems(Array.isArray(d?.items) ? d.items : []);
      })
      .catch((e) => {
        if (alive) {
          setItems([]);
          setErr(String(e));
        }
      })
      .finally(() => {
        if (alive) setBusy(false);
      });
    return () => {
      alive = false;
    };
  }, [year, season]);

  // Esc：先收年份面板，再关弹窗（与其它弹窗一致）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (yearOpen) setYearOpen(false);
      else onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, yearOpen]);

  return (
    <div className="ddet-mask" onClick={onClose}>
      <div className="arc-modal" onClick={(e) => e.stopPropagation()}>
        {/* 年份面板打开时的透明遮罩：点卡片空白处收起面板（选择行 z-index 更高，不受影响） */}
        {yearOpen && <div className="arc-yback" onClick={() => setYearOpen(false)} />}
        <div className="arc-hd">
          <IcoCalL size={16} strokeWidth={2} />
          <h3>往季番表</h3>
          <button className="ddet-close arc-x" onClick={onClose} title="关闭">
            <IcoXL size={15} strokeWidth={2.2} />
          </button>
        </div>
        <div className="arc-sel">
          {/* [lc-1591] 自定义年份面板（原生 select 的下拉列表没法美化）：按钮 + 4 列年份网格 */}
          <div className="arc-yearwrap">
            <button
              type="button"
              className={`arc-ybtn${yearOpen ? " on" : ""}`}
              onClick={() => setYearOpen((v) => !v)}
              aria-expanded={yearOpen}
              aria-haspopup="listbox"
            >
              {year} 年
              <IcoChevD size={14} strokeWidth={2} className="arc-ychev" />
            </button>
            {yearOpen && (
              <div className="arc-ypanel" role="listbox">
                {years.map((y) => (
                  <button
                    key={y}
                    type="button"
                    role="option"
                    aria-selected={y === year}
                    className={`arc-yitem${y === year ? " on" : ""}`}
                    onClick={() => {
                      setYear(y);
                      setYearOpen(false);
                    }}
                  >
                    {y}
                  </button>
                ))}
              </div>
            )}
          </div>
          <div className="arc-seasons">
            {SEASONS.map((s) => (
              <button
                key={s.k}
                className={`arc-sbtn${season === s.k ? " on" : ""}`}
                onClick={() => setSeason(s.k)}
              >
                {s.label}
              </button>
            ))}
          </div>
          <span className="arc-cnt">{busy ? "查询中…" : `${items.length} 部`}</span>
        </div>
        <div className="arc-list">
          {busy && !items.length ? (
            <div className="arc-grid">
              {[0, 1, 2, 3, 4, 5, 6, 7].map((i) => (
                <div key={i} className="arc-card">
                  <div className="arc-cv">
                    <Dsk cls="dsk-poster" />
                  </div>
                  <Dsk cls="dsk-line" />
                  <Dsk cls="dsk-line short" />
                </div>
              ))}
            </div>
          ) : err ? (
            <div className="dstate">{err}</div>
          ) : !items.length ? (
            <div className="dstate">该季度没有查到条目</div>
          ) : (
            <div className="arc-grid">
              {items.map((it) => {
                const title = it.titles[0] || "（无标题）";
                const sub = it.titles.find((t) => t !== title) || "";
                return (
                  <div
                    className="arc-card"
                    key={it.anilistId}
                    title={[title, sub, it.genres.join(" / ")].filter(Boolean).join("\n")}
                  >
                    <div className="arc-cv">
                      {it.cover ? (
                        <img src={it.cover} alt="" loading="lazy" draggable={false} />
                      ) : (
                        <span className="arc-noimg">无图</span>
                      )}
                      {it.score > 0 && (
                        <span className="arc-score" title="AniList 评分（百分制）">
                          {it.score}
                        </span>
                      )}
                    </div>
                    <div className="arc-t">{title}</div>
                    <div className="arc-meta">
                      <span>{FORMAT_CN[it.format] || it.format || "—"}</span>
                      <span>·</span>
                      <span>{it.episodes > 0 ? `${it.episodes} 话` : "话数未知"}</span>
                      <span>·</span>
                      <span>{STATUS_CN[it.status] || it.status || "—"}</span>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
