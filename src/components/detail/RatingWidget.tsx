/* [lc-1570] 详情页「评分组件」（用户设计稿接入）：放送开始 / 评分+星级 / Ranked 排名+复制 /
   收藏状态同步按钮（下拉）/ 评分透视柱状图。挂在首屏海报右侧空位（.dv2-topinfo）。
   数据：BGM 优先（bgm_subject：date / rating.rank·total·score·count），TMDB 兜底
   （airDate / rating·votes）；收藏状态走 bgm_collect_get/set（本地表 + 已绑定则同步 BGM）。 */
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";

type Props = {
  /** BGM subject id（收藏读写用；null = 未匹配到条目，按钮不可用） */
  subjectId: number | null;
  /** 放送开始（BGM date 优先，TMDB airDate 兜底；空 = 显示「—」） */
  date: string;
  /** 评分 0-10（null = 无数据） */
  score: number | null;
  /** 评分来源：bgm / tmdb / none（决定「N 人评分」的措辞） */
  scoreSource: "bgm" | "tmdb" | "none";
  /** 评分人数（bgm: rating.total / tmdb: votes） */
  total: number;
  /** BGM 排名（>0 有效；null = 未上榜） */
  rank: number | null;
  /** TMDB 编号（BGM 无排名时的兜底显示；"" = 无） */
  tmdbId: string;
  /** 评分分布（BGM rating.count 或 [lc-1576] TMDB histogram 转成的 {"1":..,"10":..}；
   *  null = 无数据 → 全部画矮柱） */
  counts: Record<string, number> | null;
  /** 分布来源：bgm / tmdb / none（[lc-1576] tmdb 时图表标题标注「· TMDB」，与「TMDB 编号」同口径） */
  countsSource: "bgm" | "tmdb" | "none";
  /** [lc-1598] 该条目**有观看记录**（详情页按条目 watched/ts 或任意一集有进度算出）：
   *  为 true 且当前状态是「未追 / 想看」时，自动改标「在看」（见下方取状态的 effect）。 */
  autoWatching?: boolean;
  /** [lc-1651] 条目本身（无 BGM 条目时，按钮从「同步状态」改成「收藏」，写本地收藏）。
   *  guid 必填，其余字段用于侧栏收藏卡片渲染。 */
  favItem?: Record<string, unknown> | null;
};

/** 收藏状态项（kind 与 BGM type 同号：0=未追（不在收藏）/1 想看 /2 看过 /3 在看 /4 搁置 /5 抛弃）。
 *  顺序沿用用户设计稿（未追/在看/想看/搁置/看过/抛弃），别"顺手改成标准序"。 */
const STATUS_ITEMS: { kind: number; label: string }[] = [
  { kind: 0, label: "未追" },
  { kind: 3, label: "在看" },
  { kind: 1, label: "想看" },
  { kind: 4, label: "搁置" },
  { kind: 2, label: "看过" },
  { kind: 5, label: "抛弃" },
];
const statusLabel = (kind: number) => STATUS_ITEMS.find((s) => s.kind === kind)?.label ?? "同步状态";

/** [lc-1651] 收藏图标（无 BGM 条目时的按钮）：实心 = 已收藏 */
function FavIcon({ filled }: { filled: boolean }) {
  return (
    <svg
      className="btn-icon"
      viewBox="0 0 24 24"
      fill={filled ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth={filled ? 0 : 1.8}
    >
      <path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" />
    </svg>
  );
}

/** 状态图标（设计稿内联 SVG 全量搬运；kind=-1 = 初始「同步状态」的刷新图标） */
function StatusIcon({ kind }: { kind: number }) {
  if (kind === 0)
    return (
      <svg className="btn-icon" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><path d="M12 6v6l4 2" /></svg>
    );
  if (kind === 3)
    return (
      <svg className="btn-icon" viewBox="0 0 24 24"><polygon points="5 3 19 12 5 21 5 3" /></svg>
    );
  if (kind === 1)
    return (
      <svg className="btn-icon" viewBox="0 0 24 24"><path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" /></svg>
    );
  if (kind === 4)
    return (
      <svg className="btn-icon" viewBox="0 0 24 24"><rect x="6" y="4" width="4" height="16" /><rect x="14" y="4" width="4" height="16" /></svg>
    );
  if (kind === 2)
    return (
      <svg className="btn-icon" viewBox="0 0 24 24"><polyline points="20 6 9 17 4 12" /></svg>
    );
  if (kind === 5)
    return (
      <svg className="btn-icon" viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" /></svg>
    );
  return (
    <svg className="btn-icon sync-icon" viewBox="0 0 24 24">
      <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
      <path d="M3 3v5h5" />
      <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
      <path d="M16 21h5v-5" />
    </svg>
  );
}

export function RatingWidget({ subjectId, date, score, scoreSource, total, rank, tmdbId, counts, countsSource, autoWatching, favItem }: Props) {
  // 收藏状态：-1 = 尚未取到（按钮显示「同步状态」+ 刷新图标）
  const [status, setStatus] = useState<number>(-1);
  // [lc-1651] 无 BGM 条目时的本地收藏态
  const [fav, setFav] = useState(false);
  const [favBusy, setFavBusy] = useState(false);
  const favGuid = String((favItem?.guid as string) ?? "").trim();
  const noSync = subjectId == null;
  const [menuOpen, setMenuOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const [changing, setChanging] = useState(false);
  const [bounce, setBounce] = useState(false);
  const [note, setNote] = useState("");
  // [lc-1572] 下拉浮层位置（portal 到 body + fixed）：开菜单时按按钮 rect 现算
  const [menuPos, setMenuPos] = useState<{ top: number; left: number } | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const timers = useRef<number[]>([]);
  const swapTimer = useRef<number | null>(null);
  const addTimer = (t: number) => timers.current.push(t);
  useEffect(
    () => () => {
      timers.current.forEach((t) => window.clearTimeout(t));
    },
    [],
  );

  // 取当前收藏状态（本地表，快；切条目重取）
  useEffect(() => {
    setStatus(-1);
    setNote("");
    if (subjectId == null) return;
    let alive = true;
    invoke<{ type?: number }>("bgm_collect_get", { subjectId })
      .then((r) => {
        if (!alive) return;
        const t = Number(r?.type ?? 0);
        setStatus(t);
        // [lc-1598] **有观看记录 → 默认把状态改成「在看」**（用户要求）。只在「未追(0) / 想看(1)」时改：
        //   在看/看过/搁置/抛弃 都是用户明确设过的，尊重原值、不覆盖。
        //   bgm_collect_set 内部：本地表落库 + 已绑定则顺手推一份到 Bangumi。
        if (autoWatching && (t === 0 || t === 1)) {
          invoke("bgm_collect_set", { subjectId, kind: 3 })
            .then(() => {
              if (alive) {
                setStatus(3);
                setNote("已按观看记录标记为「在看」");
              }
            })
            .catch(() => {
              /* 失败静默：保持原状态，不打扰 */
            });
        }
      })
      .catch((e) => {
        if (alive) setNote(String(e));
      });
    return () => {
      alive = false;
    };
  }, [subjectId, autoWatching]);

  // [lc-1651] 无 BGM 条目（按钮原本永远停在「同步状态」）→ 改读本地收藏态
  useEffect(() => {
    if (!noSync || !favGuid) return;
    let alive = true;
    invoke<boolean>("favorite_get", { guid: favGuid })
      .then((v) => {
        if (alive) setFav(!!v);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [noSync, favGuid]);

  const toggleFav = () => {
    if (!favGuid || favBusy) return;
    setFavBusy(true);
    invoke<{ favorited?: boolean }>("favorite_toggle", { item: favItem })
      .then((r) => {
        const on = !!r?.favorited;
        setFav(on);
        setNote(on ? "已收藏，可在侧边栏「收藏」里找到" : "已取消收藏");
      })
      .catch((e) => setNote(String(e)))
      .finally(() => setFavBusy(false));
  };

  // 下拉：点组件外关闭（设计稿同款）
  useEffect(() => {
    if (!menuOpen) return;
    const h = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("click", h);
    return () => document.removeEventListener("click", h);
  }, [menuOpen]);

  /** 选状态：先做 200ms 的淡出换字动画（设计稿节奏），落库走 bgm_collect_set；
   *  失败回滚（重查真实状态）并在按钮下显示错误。 */
  const pickStatus = (kind: number) => {
    if (subjectId == null || changing) return;
    setMenuOpen(false);
    setChanging(true);
    swapTimer.current = window.setTimeout(() => {
      setStatus(kind);
      setChanging(false);
      setBounce(true);
      addTimer(window.setTimeout(() => setBounce(false), 500));
    }, 200);
    invoke("bgm_collect_set", { subjectId, kind })
      .then(() => setNote(""))
      .catch((e) => {
        if (swapTimer.current) window.clearTimeout(swapTimer.current);
        setChanging(false);
        setNote(String(e));
        invoke<{ type?: number }>("bgm_collect_get", { subjectId })
          .then((r) => setStatus(Number(r?.type ?? 0)))
          .catch(() => {});
      });
  };

  // [lc-1575] 排名块判据：BGM 排名有效 → #rank；BGM 没有排名数据时改用 TMDB 编号顶上（用户要求）；
  //   两者都没有 → 保持「未上榜」（不显示复制按钮）。复制内容 = 当前显示的值。
  const rankOk = rank != null && rank > 0;
  const idFallback = !rankOk && tmdbId !== "";

  const copyRank = () => {
    const text = rankOk ? `#${rank}` : idFallback ? tmdbId : "";
    if (!text) return;
    const done = () => {
      setCopied(true);
      addTimer(window.setTimeout(() => setCopied(false), 1500));
    };
    const fallback = () => {
      const ta = document.createElement("textarea");
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
      done();
    };
    if (navigator.clipboard) navigator.clipboard.writeText(text).then(done).catch(fallback);
    else fallback();
  };

  // 放送开始：YYYY-MM-DD → [年] - [月-日]
  const dm = /^(\d{4})-(\d{2})-(\d{2})/.exec(date);
  // 评分/星级：0-10 分 → 5 星（round；无半星）
  const scoreText = score == null ? "—" : score.toFixed(1);
  const starFill = score == null ? 0 : Math.max(0, Math.min(5, Math.round(score / 2)));
  const totalLabel = total > 0 ? `${total} 人评分` : scoreSource === "tmdb" ? "TMDB 评分" : "暂无评分";
  // 柱状图：counts 的 "1".."10"；无数据全部画 2% 矮柱（设计稿的"小柱子也在"语言）
  const bars = Array.from({ length: 10 }, (_, i) => Number(counts?.[String(i + 1)] ?? 0) || 0);
  const barMax = Math.max(1, ...bars);
  const barH = (v: number) => (v > 0 ? Math.max(4, Math.round((v / barMax) * 100)) : 2);

  return (
    <div className="drw" ref={rootRef}>
      {/* SVG 渐变定义（星级用）—— id 固定 drwStarGradient，单页同一时刻只有一个组件 */}
      <svg style={{ width: 0, height: 0, position: "absolute" }} aria-hidden="true" focusable="false">
        <defs>
          <linearGradient id="drwStarGradient" x1="0%" y1="0%" x2="0%" y2="100%">
            <stop offset="0%" stopColor="var(--drw-star-1)" />
            <stop offset="100%" stopColor="var(--drw-star-2)" />
          </linearGradient>
        </defs>
      </svg>
      <div className="left-section">
        <div>
          <div className="info-group">
            <div className="label label-start">放送开始</div>
            <div className="date-inline">
              <svg className="calendar-icon" viewBox="0 0 24 24">
                <rect x="3" y="4" width="18" height="18" rx="2" ry="2" />
                <line x1="16" y1="2" x2="16" y2="6" />
                <line x1="8" y1="2" x2="8" y2="6" />
                <line x1="3" y1="10" x2="21" y2="10" />
              </svg>
              {dm ? (
                <>
                  <span className="date-part year">{dm[1]}</span>
                  <span className="date-part divider">-</span>
                  <span className="date-part month-day">{`${dm[2]}-${dm[3]}`}</span>
                </>
              ) : (
                <span className="date-part month-day">—</span>
              )}
            </div>
          </div>
          <div className="info-group">
            <div className="label label-rating">{totalLabel}</div>
            <div className="rating-display">
              <span className="rating-number">{scoreText}</span>
              <div className="rating-stars">
                {[0, 1, 2, 3, 4].map((i) => (
                  <svg key={i} className={`star ${i < starFill ? "filled" : "empty"}`} viewBox="0 0 24 24">
                    <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
                  </svg>
                ))}
              </div>
            </div>
          </div>
          <div className="info-group">
            <div className="label label-rank">{idFallback ? "TMDB 编号" : "Bangumi Ranked"}</div>
            <div className="rank-display">
              <div className="value rank">{rankOk ? `#${rank}` : idFallback ? tmdbId : "未上榜"}</div>
              {(rankOk || idFallback) && (
                <button
                  className={`copy-btn${copied ? " copied" : ""}`}
                  title={rankOk ? "复制排名" : "复制编号"}
                  onClick={copyRank}
                >
                  <svg className="icon-copy" viewBox="0 0 24 24">
                    <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                  </svg>
                  <svg className="icon-check" viewBox="0 0 24 24">
                    <polyline points="20 6 9 17 4 12" />
                  </svg>
                </button>
              )}
              <div className={`copy-tooltip${copied ? " show" : ""}`}>已复制</div>
            </div>
          </div>
        </div>
        {noSync ? (
          /* [lc-1651] 没有 BGM 条目 → 原按钮永远停在「同步状态」，改成真正可用的「收藏」：
             写本地收藏（favorites.json），侧边栏「收藏」立刻能看到。 */
          <div className={`watch-component${fav ? " active" : ""}`}>
            <button
              className="watch-btn"
              data-status={fav ? "已收藏" : "收藏"}
              disabled={!favGuid || favBusy}
              title={favGuid ? (fav ? "取消收藏" : "收藏（可在侧边栏「收藏」里找到）") : "该条目无法收藏"}
              onClick={(e) => {
                e.stopPropagation();
                toggleFav();
              }}
            >
              <div className="btn-content">
                <FavIcon filled={fav} />
                <span>{favBusy ? "处理中…" : fav ? "已收藏" : "收藏"}</span>
              </div>
            </button>
          </div>
        ) : (
          <div className={`watch-component${menuOpen ? " active" : ""}`}>
            <button
              className={`watch-btn${changing ? " changing" : ""}${bounce ? " bounce" : ""}`}
              data-status={status >= 0 ? statusLabel(status) : undefined}
              disabled={subjectId == null}
              title={subjectId == null ? "未匹配到 Bangumi 条目，无法同步收藏" : "设置收藏状态（已绑定账号则同步到 Bangumi）"}
              onClick={(e) => {
                e.stopPropagation();
                if (menuOpen) {
                  setMenuOpen(false);
                  return;
                }
                // [lc-1572] 浮层 portal 到 body：避开 .content 内的层叠/合成怪癖
                // （lc-1504 同款坑——详情页里浮层与玻璃/滚动层在 WebView2 会画错顺序，
                //  曾表现为下拉被标签条半透明"穿透"）。
                const r = e.currentTarget.getBoundingClientRect();
                setMenuPos({ top: r.bottom + 8, left: r.left });
                setMenuOpen(true);
              }}
            >
              <div className="btn-content">
                <StatusIcon kind={status} />
                <span>{status >= 0 ? statusLabel(status) : "同步状态"}</span>
              </div>
            </button>
            {createPortal(
              <ul
                className={`drw-drop${menuOpen ? " open" : ""}`}
                style={menuPos ? { top: menuPos.top, left: menuPos.left } : undefined}
              >
                {STATUS_ITEMS.map((it) => (
                  <li
                    key={it.kind}
                    data-value={it.label}
                    className={status === it.kind ? "selected" : ""}
                    onClick={() => pickStatus(it.kind)}
                  >
                    <StatusIcon kind={it.kind} />
                    <span>{it.label}</span>
                  </li>
                ))}
              </ul>,
              document.body,
            )}
          </div>
        )}
        {note && <div className="drw-note">{note}</div>}
      </div>
      <div className="right-section">
        {/* [lc-1576] 分布来自 TMDB 时在标题上标注来源（与「TMDB 编号 / TMDB 评分」同一口径） */}
        <div className="chart-title">{countsSource === "tmdb" ? "评分透视 · TMDB" : "评分透视"}</div>
        <div className="chart">
          {bars.map((v, i) => (
            <div className="bar-wrapper" key={i} title={`${i + 1} 分 · ${v} 人`}>
              <div className="bar" style={{ height: `${barH(v)}%` }} />
              <div className="x-label">{i + 1}</div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
