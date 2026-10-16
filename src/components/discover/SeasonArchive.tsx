/* [lc-1589] 「总表查询」弹窗：选年份 + 季度 → 看该季真实番表。
   [lc-1605] 数据源 **AniList → Bangumi**：AniList 标题只有日语原生（用户投诉「名称全是日语」），
   换成 BGM（中文库 name_cn + 每季当独立条目 + 条目自带 subject id 可复用现有 BGM 详情链路）。
   Rust 命令 `bgm_season`（走镜像 /v0/subjects?year&month，落盘 7 天）。
   外壳复用仓库既有的 `.ddet-mask` 遮罩模式（同详情页简介弹窗），关闭动画沿用 lc-1592。 */
import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CalendarDays as IcoCalL, ChevronDown as IcoChevD, X as IcoXL } from "lucide-react";
import { BgmCalendarItem } from "../../types";
import { Dsk } from "./shared";

/** [lc-1605] bgm_season 归一化后的条目（Rust 侧字段，见 bgm.rs normalize_season） */
type SeasonItem = {
  id: number;
  /** 日文原名 */
  name: string;
  /** 中文名（BGM name_cn，可能为空） */
  name_cn: string;
  /** 首播日期 YYYY-MM-DD（可能为空） */
  date: string;
  /** Bangumi 评分 **10 分制**（null = 无评分） */
  score: number | null;
  /** 排名（越小越好，null = 无） */
  rank: number | null;
  /** 话数（0 = 未知） */
  eps: number;
  cover: string;
};

type SeasonKey = "WINTER" | "SPRING" | "SUMMER" | "FALL";

const SEASONS: { k: SeasonKey; label: string }[] = [
  { k: "WINTER", label: "1月番" },
  { k: "SPRING", label: "4月番" },
  { k: "SUMMER", label: "7月番" },
  { k: "FALL", label: "10月番" },
];

/** [lc-1605] 季度 → BGM 的 month 参数（前端 SEASONS 已固定这四个月） */
const SEASON_MONTH: Record<SeasonKey, number> = {
  WINTER: 1,
  SPRING: 4,
  SUMMER: 7,
  FALL: 10,
};

/** 按当前月份推「当前季」（1-3 月 = 1月番，以此类推） */
function currentSeason(d = new Date()): SeasonKey {
  const m = d.getMonth() + 1;
  return m <= 3 ? "WINTER" : m <= 6 ? "SPRING" : m <= 9 ? "SUMMER" : "FALL";
}

export function SeasonArchive({
  onClose,
  onOpenItem,
}: {
  onClose: () => void;
  /** [lc-1605] 点卡片 → 跳详情页。与「每日新番表」同一条链路（App 的 openDiscoverBgm）：
      这里把 SeasonItem 合成为 BgmCalendarItem（air_date 用 date），复用同一条详情合成逻辑。 */
  onOpenItem: (it: BgmCalendarItem) => void;
}) {
  const now = new Date();
  const [year, setYear] = useState(now.getFullYear());
  const [season, setSeason] = useState<SeasonKey>(() => currentSeason(now));
  const [items, setItems] = useState<SeasonItem[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  // [lc-1591] 年份用**自定义面板**（原生 <select> 的展开列表没法美化）；打开时点别处/Esc 收起
  const [yearOpen, setYearOpen] = useState(false);
  // [lc-1592] 关闭动画：先挂 `.arc-mask-closing` 让遮罩淡出 + 卡片缩下沉，**动画走完再真卸载**
  const [closing, setClosing] = useState(false);
  // [lc-1605] after：退场动画走完后要接着做的事（点卡片跳详情时用）。
  // 与 CSS 动画时长（170ms）对齐；先关弹窗、再执行 after（跳详情自带 view-in，接力不硬切）。
  const requestClose = (after?: () => void) => {
    if (closing) return;
    setClosing(true);
    window.setTimeout(() => {
      onClose();
      after?.();
    }, 170);
  };

  // [lc-1605] 点卡片：复用退场动画，动画走完再跳详情（避免「弹窗瞬没 + 详情硬切」）
  const openItem = (it: SeasonItem) => {
    requestClose(() =>
      onOpenItem({
        id: it.id,
        type: 2,
        name: it.name,
        name_cn: it.name_cn,
        air_date: it.date,
        air_weekday: null,
        score: it.score,
        votes: 0,
        doing: 0,
        cover: it.cover,
      }),
    );
  };

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
    // [lc-1605] 数据源换成 BGM：季度 → month（1/4/7/10）
    invoke<{ items?: SeasonItem[] }>("bgm_season", { year, month: SEASON_MONTH[season] })
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
      else requestClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // requestClose 每次渲染都是新函数，但只读 closing —— 依赖里放 closing 语义等价
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [yearOpen, closing]);

  return (
    <div className={`ddet-mask${closing ? " arc-mask-closing" : ""}`} onClick={() => requestClose()}>
      <div className="arc-modal" onClick={(e) => e.stopPropagation()}>
        {/* 年份面板打开时的透明遮罩：点卡片空白处收起面板（选择行 z-index 更高，不受影响） */}
        {yearOpen && <div className="arc-yback" onClick={() => setYearOpen(false)} />}
        <div className="arc-hd">
          <IcoCalL size={16} strokeWidth={2} />
          <h3>往季番表</h3>
          <button className="ddet-close arc-x" onClick={() => requestClose()} title="关闭">
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
              {items.map((it, i) => {
                // [lc-1605] 中文优先（BGM name_cn），缺则回落日文原名
                const title = it.name_cn || it.name || "（无标题）";
                const tip = [title, it.rank ? `排名 #${it.rank}` : "", it.date]
                  .filter(Boolean)
                  .join("\n");
                return (
                  <button
                    type="button"
                    className="arc-card"
                    key={it.id}
                    title={tip}
                    // 错峰进场（克制）：按 index 给一点 animation-delay，上限防最后一排等太久
                    style={{ animationDelay: `${Math.min(i, 11) * 22}ms` }}
                    onClick={() => openItem(it)}
                  >
                    <div className="arc-cv">
                      {it.cover ? (
                        <img src={it.cover} alt="" loading="lazy" draggable={false} />
                      ) : (
                        <span className="arc-noimg">无图</span>
                      )}
                      {it.score != null && it.score > 0 && (
                        <span className="arc-score" title="Bangumi 评分（10 分制）">
                          {it.score}
                        </span>
                      )}
                    </div>
                    <div className="arc-t">{title}</div>
                    <div className="arc-meta">
                      <span>{it.date || "日期未知"}</span>
                      <span>·</span>
                      <span>{it.eps > 0 ? `${it.eps} 话` : "话数未知"}</span>
                    </div>
                  </button>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
