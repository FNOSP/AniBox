/* [lc-1615] 观影记录页 —— 样式照抄原项目（Electron 版 watchHistory.ts 的 .wh-* 面板：
   大标题 + 筛选胶囊 + 「观影活跃度」统计卡 + GitHub 风热力图 + 「已看完/在观看」双栏海报墙），
   但配色/材质落到本仓库 tokens（深浅主题通用，见 src/styles/37-history.css）。
   数据 = get_watch_history（飞牛 play/list ∪ 本机播放历史，Rust 侧合并去重）。 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Clock as IcoClockL, RefreshCw as IcoRefreshL } from "lucide-react";
import { imgUrl } from "../../lib/images";
import { MediaItem } from "../../types";

/** get_watch_history 返回的统一条目（camelCase；Rust history.rs 组装） */
export type HistoryItem = {
  guid: string;
  title: string;
  /** 集号标签（"S2E1"；电影为空） */
  episode?: string;
  /** 海报（NAS 相对路径；空 = 用渐变兜底） */
  poster?: string;
  tmdbId?: string;
  bgmId?: string;
  /** Movie / Episode / ...；在线源固定 "webrs" */
  type?: string;
  /** 播放时刻（ms；飞牛条目无时间戳时为 0 → 显示「未记录时间」） */
  playedAt?: number;
  /** "fnos"（库内）| "webrs"（在线源） */
  source?: string;
  /** 在线源 = 规则名 */
  sourceName?: string;
  parentGuid?: string;
  /** 剧 guid（富化补全；点击进详情用） */
  seriesGuid?: string;
  /** 观看进度 0-1（飞牛 play/list 有；本机记录为 0） */
  progress?: number;
};

type HistoryResp = { items?: HistoryItem[]; fnosOk?: boolean; fnosError?: string };

type Filter = "全部" | "电影" | "剧集" | "在线源";

const FILTERS: Filter[] = ["全部", "电影", "剧集", "在线源"];

/** 条目 → 筛选分类（在线源按 source；其余按类型：Movie=电影，其它归剧集） */
function catOf(it: HistoryItem): Exclude<Filter, "全部"> {
  if (it.source === "webrs") return "在线源";
  return it.type === "Movie" ? "电影" : "剧集";
}

/** 相对时间（口径照抄原项目 formatAgo；无时间戳 → 未记录时间） */
function fmtAgo(ts?: number): string {
  if (!ts || ts <= 0) return "未记录时间";
  const mins = Math.floor((Date.now() - ts) / 60000);
  if (mins < 1) return "刚刚";
  if (mins < 60) return `${mins} 分钟前`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs} 小时前`;
  const days = Math.floor(hrs / 24);
  if (days < 30) return `${days} 天前`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months} 个月前`;
  return `${Math.floor(months / 12)} 年前`;
}

/** 无海报时的渐变兜底（照抄原项目 ART_PALETTE：按名称 hash 稳定取色） */
const ART_PALETTE = [
  "#3a2a1a,#120c08", "#3a1a2a,#140810", "#1a2a3a,#081018",
  "#2a1a3a,#100818", "#2a2a1a,#101008", "#10283a,#04101c",
  "#2a2410,#100c04", "#3a2010,#140a04", "#3a1010,#140404",
  "#1a2a2a,#080810", "#2a1a1a,#100808", "#102a2a,#04100c",
];
function artForName(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = ((h << 5) - h + name.charCodeAt(i)) | 0;
  const [a, b] = ART_PALETTE[Math.abs(h) % ART_PALETTE.length].split(",");
  return `linear-gradient(145deg, ${a}, ${b})`;
}

/* ── 热力图（GitHub 贡献图口径，照抄原项目：列=周、行=星期，53 周 = 过去一年） ── */

type HeatCell = { date: Date; count: number; future: boolean };

const HEAT_CELL = 12; // 与 37-history.css 的 --hist-hcell 同步
const HEAT_GAP = 4;
const HEAT_WEEKS = 53;

function localKey(d: Date): string {
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}
const pad2 = (n: number) => (n < 10 ? `0${n}` : `${n}`);
const fmtDay = (d: Date) => `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
/** 颜色档位（照抄原项目）：0=空 1=1条 2=2~3 3=4~5 4=6+ */
const heatLevel = (c: number) => (c <= 0 ? 0 : c === 1 ? 1 : c <= 3 ? 2 : c <= 5 ? 3 : 4);

function buildHeat(items: HistoryItem[]) {
  const counts = new Map<string, number>();
  for (const it of items) {
    const ts = it.playedAt ?? 0;
    if (ts <= 0) continue;
    const k = localKey(new Date(ts));
    counts.set(k, (counts.get(k) ?? 0) + 1);
  }
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const end = new Date(today);
  end.setDate(end.getDate() + (6 - end.getDay())); // 对齐到周六（本周尾）
  const start = new Date(end);
  start.setDate(start.getDate() - (HEAT_WEEKS * 7 - 1)); // 起点 = 周日
  const weeks: HeatCell[][] = [];
  const cursor = new Date(start);
  for (let w = 0; w < HEAT_WEEKS; w++) {
    const col: HeatCell[] = [];
    for (let dow = 0; dow < 7; dow++) {
      const future = cursor > today;
      col.push({ date: new Date(cursor), count: future ? 0 : counts.get(localKey(cursor)) ?? 0, future });
      cursor.setDate(cursor.getDate() + 1);
    }
    weeks.push(col);
  }
  // 月份标签：月份变化时打一个，隔一个打（避免 53 周挤满，照抄原项目口径）
  const months: { left: number; text: string }[] = [];
  let lastMonth = -1;
  let skip = 0;
  weeks.forEach((wk, wi) => {
    const m = wk[0].date.getMonth();
    if (m !== lastMonth) {
      if (skip % 2 === 0) months.push({ left: wi * (HEAT_CELL + HEAT_GAP), text: `${m + 1}月` });
      lastMonth = m;
      skip++;
    }
  });
  const total = [...counts.values()].reduce((s, n) => s + n, 0);
  return { weeks, months, total };
}

const WEEKDAY_LABELS = ["日", "一", "二", "三", "四", "五", "六"];

export function WatchHistoryView({
  onBack,
  onOpenItem,
}: {
  onBack: () => void;
  /** 库内条目点击 → 走现有详情链路（App.tsx 负责 setDetailItem + view 切换） */
  onOpenItem: (item: MediaItem) => void;
}) {
  const [data, setData] = useState<{ items: HistoryItem[]; fnosOk: boolean; fnosError: string } | null>(null);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const [filter, setFilter] = useState<Filter>("全部");
  const [note, setNote] = useState("");
  const noteTimer = useRef<number | null>(null);
  const heatWrapRef = useRef<HTMLDivElement | null>(null);
  const [tip, setTip] = useState<{ x: number; y: number; text: string } | null>(null);

  function showNote(msg: string) {
    setNote(msg);
    if (noteTimer.current) window.clearTimeout(noteTimer.current);
    noteTimer.current = window.setTimeout(() => setNote(""), 2600);
  }

  const load = useCallback(async (isRefresh = false) => {
    if (isRefresh) setBusy(true);
    setErr("");
    try {
      const r = await invoke<HistoryResp>("get_watch_history");
      setData({
        items: Array.isArray(r?.items) ? r.items : [],
        fnosOk: !!r?.fnosOk,
        fnosError: String(r?.fnosError ?? ""),
      });
      if (isRefresh) {
        showNote(`已刷新：共 ${Array.isArray(r?.items) ? r.items.length : 0} 条记录`);
      }
    } catch (e) {
      setErr(String(e));
    } finally {
      if (isRefresh) setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // 卸载时清掉 toast 定时器（防泄漏）
  useEffect(() => () => {
    if (noteTimer.current) window.clearTimeout(noteTimer.current);
  }, []);

  const items = data?.items ?? [];
  const counts = useMemo(() => {
    const c: Record<Filter, number> = { 全部: items.length, 电影: 0, 剧集: 0, 在线源: 0 };
    for (const it of items) c[catOf(it)]++;
    return c;
  }, [items]);

  const filtered = useMemo(
    () => (filter === "全部" ? items : items.filter((it) => catOf(it) === filter)),
    [items, filter],
  );

  // 双栏拆分（照抄原项目：已看完 = 进度≥1；其余 = 在观看）
  const done = useMemo(() => filtered.filter((it) => (it.progress ?? 0) >= 1), [filtered]);
  const watching = useMemo(() => filtered.filter((it) => (it.progress ?? 0) < 1), [filtered]);

  const stats = useMemo(() => {
    const now = new Date();
    const monthStart = new Date(now.getFullYear(), now.getMonth(), 1).getTime();
    const dayMs = 86400000;
    const thirtyAgo = now.getTime() - 30 * dayMs;
    const activeDays = new Set<string>();
    let monthCount = 0;
    for (const it of items) {
      const ts = it.playedAt ?? 0;
      if (ts <= 0) continue;
      if (ts >= monthStart) monthCount++;
      if (ts >= thirtyAgo) activeDays.add(localKey(new Date(ts)));
    }
    const total = items.length;
    const doneN = items.filter((it) => (it.progress ?? 0) >= 1).length;
    return {
      activeDays: activeDays.size,
      monthCount,
      total,
      doneRate: total ? Math.round((doneN / total) * 100) : 0,
      doneN,
      watchingN: total - doneN,
    };
  }, [items]);

  const heat = useMemo(() => buildHeat(items), [items]);

  /** 卡片点击：库内 → 详情链路；在线源 → 提示（没有可打开的详情页） */
  const handleOpen = (it: HistoryItem) => {
    if (it.source === "webrs") {
      showNote("在线源条目暂无可打开的详情页 —— 可在对应作品的「在线资源」面板中继续播放");
      return;
    }
    onOpenItem({
      guid: it.seriesGuid || it.guid,
      title: it.title,
      poster: it.poster || undefined,
      // 剧集有剧 guid → TV（完整季/集详情）；缺剧 guid 时退化为集条目形态（仍能打开，不会空等选集）
      type: it.type === "Movie" ? "Movie" : it.seriesGuid ? "TV" : "Episode",
    });
  };

  const loading = data === null && !err;

  return (
    <main className="content hist-full">
      <div className="page-head hist-head">
        <button className="back" onClick={onBack} title="返回首页">
          ‹
        </button>
        <h3 className="hist-title">观影记录</h3>
        <span className="muted">{data ? `${items.length} 条` : ""}</span>
        <button className="hist-sync" onClick={() => void load(true)} disabled={busy || loading}>
          <IcoRefreshL size={13} strokeWidth={2.2} className={busy ? "hist-sync-spin" : ""} />
          {busy ? "同步中…" : "立即同步"}
        </button>
      </div>

      {err && (
        <div className="err">
          加载失败：{err}
          <button className="hist-retry" onClick={() => void load()}>
            重试
          </button>
        </div>
      )}

      {loading && (
        <>
          <div className="hist-skel-stats" />
          <div className="hist-grid hist-skel-grid">
            {Array.from({ length: 10 }).map((_, i) => (
              <div className="hist-skel" key={i} />
            ))}
          </div>
        </>
      )}

      {!loading && data && (
        <>
          {/* 筛选胶囊（照抄原项目右上角「全部/电影/剧集/动漫」→ 本仓库分类：+在线源；计数随数据刷新） */}
          <div className="hist-pills">
            {FILTERS.map((f) => (
              <button
                key={f}
                className={`hist-pill${filter === f ? " active" : ""}`}
                onClick={() => setFilter(f)}
              >
                {f} ({counts[f]})
              </button>
            ))}
          </div>

          {!data.fnosOk && (
            <div className="hist-warn">
              飞牛记录暂不可用{data.fnosError ? `（${data.fnosError}）` : ""} —— 当前仅显示本机播放记录
            </div>
          )}

          {items.length === 0 ? (
            <div className="hist-empty">
              <IcoClockL size={26} strokeWidth={1.6} />
              <div className="hist-empty-t">暂无观影记录</div>
              <div className="hist-empty-s">
                {data.fnosOk ? "播放过的内容会自动记录在这里（库内 + 在线源）" : "登录后还会并入飞牛影视的观看记录"}
              </div>
            </div>
          ) : (
            <>
              {/* 顶部两栏：左=观影活跃度统计卡，右=热力图卡（照抄原项目 wh-chart-split 版式） */}
              <section className="hist-chart-split">
                <div className="hist-stats">
                  <div className="hist-stats-title">观影活跃度</div>
                  <div className="hist-stats-sub">
                    共 {stats.total} 条 · 已看完 {stats.doneN} · 在看 {stats.watchingN} · {stats.activeDays} 天/30天活跃
                  </div>
                  <div className="hist-stat-row">
                    <div>
                      <div className="n"><b>{stats.activeDays}</b><i>天</i></div>
                      <span>近30天</span>
                    </div>
                    <div>
                      <div className="n"><b>{stats.monthCount}</b><i>条</i></div>
                      <span>本月</span>
                    </div>
                    <div>
                      <div className="n"><b>{stats.total}</b><i>条</i></div>
                      <span>累计</span>
                    </div>
                    <div>
                      <div className="n"><b>{stats.doneRate}</b><i>%</i></div>
                      <span>看完率</span>
                    </div>
                  </div>
                </div>

                <div className="hist-heat">
                  <div className="hist-heat-head">
                    <div className="hist-heat-title-wrap">
                      <div className="hist-heat-title">观影活动热力图</div>
                      <div className="hist-heat-range">过去一年 · {heat.total} 条记录</div>
                    </div>
                    <div className="hist-heat-legend">
                      少
                      <span className="hist-hcell l1" />
                      <span className="hist-hcell l2" />
                      <span className="hist-hcell l3" />
                      <span className="hist-hcell l4" />
                      多
                    </div>
                  </div>
                  <div className="hist-heat-body" ref={heatWrapRef}>
                    <div className="hist-heat-days">
                      {WEEKDAY_LABELS.map((d, i) => (
                        <span key={d} className={i === 1 || i === 3 || i === 5 ? "show" : ""}>
                          {d}
                        </span>
                      ))}
                    </div>
                    <div className="hist-heat-scroll">
                      <div className="hist-heat-inner">
                        <div className="hist-heat-months">
                          {heat.months.map((m) => (
                            <span key={m.left} style={{ left: m.left }}>
                              {m.text}
                            </span>
                          ))}
                        </div>
                        <div className="hist-heat-cols">
                          {heat.weeks.map((wk, wi) => (
                            <div className="hist-heat-week" key={wi}>
                              {wk.map((cell) => {
                                const lv = cell.future ? -1 : heatLevel(cell.count);
                                return (
                                  <div
                                    key={fmtDay(cell.date)}
                                    className={`hist-hcell${cell.future ? " future" : lv > 0 ? ` l${lv}` : ""}`}
                                    data-cnt={cell.count}
                                    onMouseEnter={(e) => {
                                      const wrap = heatWrapRef.current;
                                      if (!wrap) return;
                                      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                                      const wr = wrap.getBoundingClientRect();
                                      setTip({
                                        x: r.left - wr.left + r.width / 2,
                                        y: r.top - wr.top - 6,
                                        text: `${fmtDay(cell.date)} · ${cell.count > 0 ? `${cell.count} 条记录` : "未观看"}`,
                                      });
                                    }}
                                    onMouseLeave={() => setTip(null)}
                                  />
                                );
                              })}
                            </div>
                          ))}
                        </div>
                      </div>
                    </div>
                    {tip && (
                      <div className="hist-heat-tip show" style={{ left: tip.x, top: tip.y }}>
                        {tip.text}
                      </div>
                    )}
                  </div>
                </div>
              </section>

              {/* 影视清单：已看完 / 在观看 双栏（照抄原项目 wh-split 版式） */}
              <section className="hist-section">
                <div className="hist-section-head">
                  <div className="hist-section-title">影视清单</div>
                  <div className="hist-section-hint">点击查看详情</div>
                </div>
                <div className="hist-split">
                  <div className="hist-col done">
                    <div className="hist-col-head">
                      <span className="hist-col-title">已看完</span>
                      <span className="hist-col-count">{done.length}</span>
                    </div>
                    <div className="hist-grid">
                      {done.length > 0 ? (
                        done.map((it) => <HistoryCard key={it.guid} it={it} onOpen={handleOpen} />)
                      ) : (
                        <div className="hist-col-empty">暂无已看完的作品</div>
                      )}
                    </div>
                  </div>
                  <div className="hist-col watching">
                    <div className="hist-col-head">
                      <span className="hist-col-title">在观看</span>
                      <span className="hist-col-count">{watching.length}</span>
                    </div>
                    <div className="hist-grid">
                      {watching.length > 0 ? (
                        watching.map((it) => <HistoryCard key={it.guid} it={it} onOpen={handleOpen} />)
                      ) : (
                        <div className="hist-col-empty">暂无在观看的作品</div>
                      )}
                    </div>
                  </div>
                </div>
              </section>
            </>
          )}
        </>
      )}

      {note && <div className="hist-toast show">{note}</div>}
    </main>
  );
}

/* ── 海报卡（照抄原项目 .wh-card：2:3 封面 + 底部渐隐 + 名称/副行 + 进度条 + 来源角标） ── */

function HistoryCard({ it, onOpen }: { it: HistoryItem; onOpen: (it: HistoryItem) => void }) {
  const pct = Math.round(Math.min(1, Math.max(0, it.progress ?? 0)) * 100);
  const sub = [it.episode, fmtAgo(it.playedAt)].filter(Boolean).join(" · ");
  const isWebrs = it.source === "webrs";
  return (
    <div
      className="hist-card"
      onClick={() => onOpen(it)}
      title={isWebrs && it.sourceName ? `在线源：${it.sourceName}` : it.title}
    >
      <div
        className="hist-art"
        style={
          it.poster
            ? { backgroundImage: `url(${imgUrl(it.poster)})` }
            : { background: artForName(it.title || it.guid) }
        }
      />
      <div className="hist-scrim" />
      <span className={`hist-badge${isWebrs ? " webrs" : ""}`}>{isWebrs ? "在线源" : "飞牛"}</span>
      {pct > 0 && pct < 100 && (
        <div className="hist-pbar">
          <div className="hist-pfill" style={{ width: `${pct}%` }} />
        </div>
      )}
      <div className="hist-meta">
        <div className="hist-name">{it.title || "未知作品"}</div>
        <div className="hist-sub">{sub}</div>
      </div>
    </div>
  );
}
