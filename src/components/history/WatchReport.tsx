/* [lc-1631] 年度观影报告 —— 照原项目 Electron 版 `watchReport.ts`（Spotify Wrapped 式 5 页翻页
   报告）搬到 React：①封面(年份/总时长/部数) ②月度节奏(12 月条形) ③观影时刻(24 小时分布+夜猫指数)
   ④年度 TOP5(按观看时长) ⑤总结(连续天数/看完率/评分)。←/→ 键与按钮翻页，年份下拉切换，
   可导出长图（canvas 程序化绘制，无外链图片 → 不污染画布）。

   数据源分三级（与原项目同口径，逐级降级并明示）：
     ① 本机播放事件台账（每次起播一条真实墙钟时间；播放结束回填真实观看时长）—— 时长/日期全真实；
     ② 无真实时长的事件用**历史平均单次时长**折算（而不是硬编码 30 分钟）；
     ③ 记录被滚动淘汰的年份，退回**日级台账 × 平均单次时长**估算（封面明示为估算）。
   作品级事实（部数/看完率/片单）来自 get_watch_history 的作品级 items（进度 × 总时长，真实）。 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { HistoryItem } from "./WatchHistoryView";

/** 本机播放事件（Rust report_data.events） */
type ReportEvent = {
  ts: number;
  guid: string;
  title: string;
  type?: string;
  source?: string;
  /** 真实观看时长（ms；0 = 未知，回填失败/未播完） */
  ms?: number;
  /** 条目总时长（ms；0 = 未知） */
  durationMs?: number;
  progress?: number;
  finished?: boolean;
};
/** 日级台账桶（月份 0 基） */
type ReportDay = { y: number; m0: number; d: number; count: number };
type ReportResp = { years?: number[]; events?: ReportEvent[]; days?: ReportDay[] };

type ReportItem = { name: string; ms: number; prog: number; rating: number; type: string; finished: boolean };
type YearReport = {
  year: number;
  totalMs: number;
  titles: number;
  finished: number;
  rated: number;
  monthMs: number[]; // 12
  hourMs: number[]; // 24
  activeDays: number;
  maxStreak: number;
  nightRatio: number; // 0-1，00:00-05:00 开始的会话占比
  top: ReportItem[];
  /** 'events' = 真实事件台账；'ledger' = 日级台账估算 */
  source: "events" | "ledger";
  estimated: number; // 用平均时长补齐的事件条数
};

const MIN_VALID_TS = 946_684_800_000; // 2000-01-01，脏时间戳阈值（对齐 Rust MIN_VALID_MS）
const FALLBACK_SESSION_MS = 30 * 60000; // 从未有带时长事件时的最后手段

/** 事件日期 → 本地日历日 key（避免 UTC 偏移串日） */
function dayKey(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}
/** 一组日历日 key 的最长连续天数 */
function maxStreakOf(dayKeys: string[]): number {
  const days = Array.from(new Set(dayKeys)).sort();
  let maxStreak = 0;
  let run = 0;
  let prev = "";
  for (const dk of days) {
    if (prev) {
      const [py, pm, pd] = prev.split("-").map(Number);
      const [cy, cm, cd] = dk.split("-").map(Number);
      const gap = Math.round((new Date(cy, cm, cd).getTime() - new Date(py, pm, pd).getTime()) / 86400000);
      run = gap === 1 ? run + 1 : 1;
    } else run = 1;
    if (run > maxStreak) maxStreak = run;
    prev = dk;
  }
  return maxStreak;
}
/** 无时长记录的事件兜底：优先用历史真实均值，完全没有历史才退回 30 分钟 */
function avgSessionMs(events: ReportEvent[]): number {
  let sum = 0;
  let n = 0;
  for (const e of events) if (e.ms && e.ms > 0) { sum += e.ms; n++; }
  return n > 0 ? sum / n : FALLBACK_SESSION_MS;
}
/** 小时数（原项目口径：≥1 万小时折「万」） */
function fmtHours(ms: number): string {
  const h = Math.round(ms / 3600000);
  return h >= 10000 ? (ms / 3600000 / 10000).toFixed(1) + " 万" : String(h);
}

/** 作品级事实（来自 get_watch_history 的 items）：进度 × 总时长折算观看量。 */
function itemFacts(items: HistoryItem[]): ReportItem[] {
  return items.map((it) => {
    const dur = it.durationMs ?? 0;
    const watched = Math.min(1, Math.max(0, it.progress ?? 0));
    // 未看完按真实进度折算；看完按全片；仅痕迹（progress=0）按 5% 估
    const ms = dur > 0 ? Math.round(dur * (watched > 0 ? watched : 0.05)) : 0;
    return {
      name: it.title || it.guid,
      ms,
      prog: it.progress ?? 0,
      rating: it.myRating ?? 0,
      type: it.type || "",
      finished: (it.finished ?? false) || (it.progress ?? 0) >= 1,
    };
  });
}

/**
 * 由「事件台账 + 日级台账 + 作品级事实」算出某年的报告。
 * 纯函数（数据全部由调用方注入），便于静态验证。
 */
function computeReport(
  events: ReportEvent[],
  days: ReportDay[],
  items: HistoryItem[],
  year: number,
): YearReport {
  const monthMs = new Array(12).fill(0) as number[];
  const hourMs = new Array(24).fill(0) as number[];
  const dayKeys: string[] = [];
  let totalMs = 0;
  let nightMs = 0;
  let estimated = 0;
  const fallback = avgSessionMs(events);
  const byKey = new Map<string, { name: string; ms: number; prog: number; finished: boolean }>();

  for (const e of events) {
    if (!e.ts || e.ts < MIN_VALID_TS) continue;
    const d = new Date(e.ts);
    if (d.getFullYear() !== year) continue;
    let ms = typeof e.ms === "number" && e.ms > 0 ? e.ms : 0;
    if (!ms) {
      ms = fallback;
      estimated++;
    }
    totalMs += ms;
    monthMs[d.getMonth()] += ms;
    hourMs[d.getHours()] += ms;
    if (d.getHours() < 5) nightMs += ms;
    dayKeys.push(dayKey(e.ts));
    const gk = e.guid || e.title || "unknown";
    const cur = byKey.get(gk);
    if (cur) {
      cur.ms += ms;
      cur.prog = Math.max(cur.prog, e.progress ?? 0);
      cur.finished = cur.finished || !!e.finished;
    } else {
      byKey.set(gk, { name: e.title || "未知作品", ms, prog: e.progress ?? 0, finished: !!e.finished });
    }
  }

  // 作品级元数据（评分/类型/看完）用 items 补齐（事件只带标题）
  const metaByName = new Map(items.map((it) => [it.title || it.guid, it]));
  const facts = itemFacts(items);
  const factByName = new Map(facts.map((f) => [f.name, f]));

  if (totalMs > 0) {
    const top: ReportItem[] = [];
    byKey.forEach((v) => {
      const m = metaByName.get(v.name);
      const f = factByName.get(v.name);
      top.push({
        name: v.name,
        ms: v.ms,
        prog: v.prog || (m?.progress ?? 0),
        rating: m?.myRating ?? 0,
        type: m?.type || "",
        finished: v.finished || f?.finished || (m?.progress ?? 0) >= 1,
      });
    });
    top.sort((a, b) => b.ms - a.ms);
    return {
      year,
      totalMs,
      titles: top.length,
      finished: top.filter((t) => t.finished || t.prog >= 1).length,
      rated: top.filter((t) => t.rating > 0).length,
      monthMs,
      hourMs,
      activeDays: new Set(dayKeys).size,
      maxStreak: maxStreakOf(dayKeys),
      nightRatio: totalMs > 0 ? nightMs / totalMs : 0,
      top: top.slice(0, 5),
      source: "events",
      estimated,
    };
  }

  // ③ 兜底：本年事件为空（记录被裁掉/只同步了飞牛）→ 用日级台账估算月度节奏，
  //    作品级事实仍用 items（真实进度 × 总时长），片单/看完率不因缺日期而空白。
  const ledMonth = new Array(12).fill(0) as number[];
  const ledDays = new Set<string>();
  let ledTotal = 0;
  for (const b of days) {
    if (b.y !== year) continue;
    const ms = Math.round(b.count * fallback);
    ledTotal += ms;
    ledMonth[b.m0] += ms;
    ledDays.add(`${b.y}-${b.m0}-${b.d}`);
  }
  const top = facts
    .filter((f) => f.ms > 0 || f.prog > 0)
    .sort((a, b) => b.ms - a.ms || b.prog - a.prog || b.rating - a.rating);
  return {
    year,
    totalMs: ledTotal,
    titles: top.length,
    finished: top.filter((t) => t.finished || t.prog >= 1).length,
    rated: top.filter((t) => t.rating > 0).length,
    monthMs: ledMonth,
    hourMs, // 日级台账没有小时维度 → 全 0，页面据此显示「暂无时刻数据」
    activeDays: ledDays.size,
    maxStreak: maxStreakOf(Array.from(ledDays)),
    nightRatio: 0,
    top: top.slice(0, 5),
    source: "ledger",
    estimated: ledTotal > 0 ? ledDays.size : 0,
  };
}

const MEDALS = ["🥇", "🥈", "🥉", "④", "⑤"];

export function WatchReport({
  items,
  onClose,
}: {
  /** 作品级事实（已在观影记录页加载，直接复用，避免报告再拉一次飞牛） */
  items: HistoryItem[];
  onClose: () => void;
}) {
  const [raw, setRaw] = useState<{ events: ReportEvent[]; days: ReportDay[]; years: number[] } | null>(null);
  const [err, setErr] = useState("");
  const [year, setYear] = useState<number>(new Date().getFullYear());
  const [page, setPage] = useState(0);
  const stageRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let alive = true;
    invoke<ReportResp>("get_watch_report")
      .then((r) => {
        if (!alive) return;
        setRaw({
          events: Array.isArray(r?.events) ? r.events : [],
          days: Array.isArray(r?.days) ? r.days : [],
          years: Array.isArray(r?.years) ? r.years : [],
        });
      })
      .catch((e) => alive && setErr(String(e)));
    return () => {
      alive = false;
    };
  }, []);

  /** 可选年份 = 数据里出现过的年份（降序）+ 当前年 */
  const yearList = useMemo(() => {
    const set = new Set<number>([new Date().getFullYear()]);
    for (const y of raw?.years ?? []) set.add(y);
    for (const e of raw?.events ?? []) if (e.ts >= MIN_VALID_TS) set.add(new Date(e.ts).getFullYear());
    for (const b of raw?.days ?? []) if (b.y >= 2000 && b.y <= new Date().getFullYear()) set.add(b.y);
    return Array.from(set).sort((a, b) => b - a);
  }, [raw]);

  const report = useMemo<YearReport | null>(
    () => (raw ? computeReport(raw.events, raw.days, items, year) : null),
    [raw, items, year],
  );

  // 打开时默认选「有数据的最近一年」（当前年没看东西时不该展示全 0 封面）
  const initedRef = useRef(false);
  useEffect(() => {
    if (!raw || initedRef.current || yearList.length === 0) return;
    initedRef.current = true;
    const hit = yearList.find((y) => computeReport(raw.events, raw.days, items, y).totalMs > 0);
    setYear(hit ?? yearList[0]);
  }, [raw, yearList, items]);

  const PAGES = 5;
  const turn = useCallback(
    (dir: number) => setPage((p) => Math.max(0, Math.min(PAGES - 1, p + dir))),
    [PAGES],
  );

  // ←/→ 翻页，Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowRight") turn(1);
      else if (e.key === "ArrowLeft") turn(-1);
      else if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [turn, onClose]);

  const exportLongImage = useCallback(() => {
    if (!report) return;
    exportReportImage(report);
  }, [report]);

  return (
    <div className="wr-overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="wr-stage" ref={stageRef}>
        {!report && <div className="wr-loading">{err ? `加载失败：${err}` : "生成报告中…"}</div>}
        {report && <ReportPage r={report} page={page} />}
      </div>

      <div className="wr-ctrl">
        <select
          className="wr-year"
          value={year}
          onChange={(e) => {
            setYear(Number(e.target.value));
            setPage(0);
          }}
          aria-label="选择年份"
        >
          {yearList.map((y) => (
            <option key={y} value={y}>
              {y} 年
            </option>
          ))}
        </select>
        <button className="wr-nav" onClick={() => turn(-1)} aria-label="上一页">
          ‹
        </button>
        <div className="wr-dots">
          {Array.from({ length: PAGES }).map((_, i) => (
            <i key={i} className={i === page ? "on" : ""} />
          ))}
        </div>
        <button className="wr-nav" onClick={() => turn(1)} aria-label="下一页">
          ›
        </button>
        <button className="wr-close" onClick={onClose}>
          ✕ 关闭
        </button>
      </div>

      {report && (
        <button className="wr-export" onClick={exportLongImage}>
          ⬇ 导出长图
        </button>
      )}
    </div>
  );
}

/* ── 单页渲染（照原项目 pageHtml 的 5 页结构） ── */

function ReportPage({ r, page }: { r: YearReport; page: number }) {
  if (page === 0) return <CoverPage r={r} />;
  if (page === 1) return <MonthPage r={r} />;
  if (page === 2) return <HourPage r={r} />;
  if (page === 3) return <TopPage r={r} />;
  return <SummaryPage r={r} />;
}

function Kicker({ text }: { text: string }) {
  return <div className="wr-kicker">{text}</div>;
}

function CoverPage({ r }: { r: YearReport }) {
  return (
    <div className="wr-pg wr-pg-cover">
      <Kicker text={`ANIBOX · ${r.year} 年度观影报告`} />
      <div className="wr-cover-lead">这一年，你在 AniBox 看了</div>
      <div className="wr-cover-big">
        <span className="wr-num">{fmtHours(r.totalMs)}</span>
        <span className="wr-unit">小时</span>
      </div>
      <div className="wr-cover-row">
        <div>
          <b>{r.titles}</b>
          <span>部作品</span>
        </div>
        <div>
          <b>{r.activeDays}</b>
          <span>天有观影</span>
        </div>
        <div>
          <b>{r.finished}</b>
          <span>部看完</span>
        </div>
      </div>
      {r.source === "ledger" && (
        <div className="wr-note">
          📊 播放时段按每日台账估算（每日记录 × 你的平均单次时长）；本机播放记录已被滚动淘汰，
          播一段后重开即逐步精确。片单/看完率为全库累计值。
        </div>
      )}
      {r.source === "events" && r.estimated > 0 && (
        <div className="wr-note">📊 有 {r.estimated} 次播放未回填真实时长，按你的平均单次时长折算。</div>
      )}
      <div className="wr-foot">← → 翻页 · Esc 关闭 · 可导出长图</div>
    </div>
  );
}

function MonthPage({ r }: { r: YearReport }) {
  const max = Math.max(...r.monthMs, 1);
  const peakIdx = r.monthMs.indexOf(Math.max(...r.monthMs));
  return (
    <div className="wr-pg wr-pg-pad">
      <Kicker text="PAGE 2 · 月度节奏" />
      <div className="wr-h1">
        {r.totalMs > 0 ? (
          <>
            你观影最猛的一个月是 <span className="wr-grad">{peakIdx + 1} 月</span>
          </>
        ) : (
          "月度节奏"
        )}
      </div>
      <div className="wr-bars">
        {r.monthMs.map((ms, i) => {
          const h = Math.max(3, Math.round((ms / max) * 150));
          const hot = ms === max && ms > 0;
          return (
            <div className="wr-bcol" key={i}>
              <div className={`wr-bpeak${hot ? " on" : ""}`}>峰值</div>
              <div className={`wr-bar${hot ? " hot" : ""}`} style={{ height: `${h}px` }} />
              <div className="wr-blab">{i + 1}月</div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function HourPage({ r }: { r: YearReport }) {
  const hourTotal = r.hourMs.reduce((a, b) => a + b, 0);
  if (hourTotal <= 0) {
    return (
      <div className="wr-pg wr-pg-pad">
        <Kicker text="PAGE 3 · 观影时刻" />
        <div className="wr-h1">观影时刻</div>
        <div className="wr-muted-blk">
          暂无可用的观看时刻数据。
          <br />
          本机播放记录尚未覆盖本年，或记录已被滚动淘汰。
          <br />
          <span className="wr-accent">用本应用播放一段后重开本报告，即可看到真实的 24 小时分布与夜猫指数。</span>
        </div>
      </div>
    );
  }
  const max = Math.max(...r.hourMs, 1);
  const owl = r.nightRatio > 0.25 ? "重度夜猫 🦉" : r.nightRatio > 0.1 ? "轻度夜猫 🌙" : "养生作息 ☀️";
  return (
    <div className="wr-pg wr-pg-pad">
      <Kicker text="PAGE 3 · 观影时刻" />
      <div className="wr-h1">
        你是 <span className="wr-grad">{owl}</span>
      </div>
      <div className="wr-sub">深夜时段(00-05点)观看占比 {(r.nightRatio * 100).toFixed(1)}%</div>
      <div className="wr-hbars">
        {r.hourMs.map((ms, h) => (
          <div className="wr-hcol" key={h} title={`${h}:00`}>
            <div
              className={`wr-hbar${h < 5 ? " night" : ""}`}
              style={{ height: `${Math.max(3, Math.round((ms / max) * 110))}px` }}
            />
            <div className="wr-hlab">{h % 3 === 0 ? String(h).padStart(2, "0") : ""}</div>
          </div>
        ))}
      </div>
    </div>
  );
}

function TopPage({ r }: { r: YearReport }) {
  const max = r.top.length ? r.top[0].ms : 1;
  return (
    <div className="wr-pg wr-pg-pad">
      <Kicker text="PAGE 4 · 年度片单" />
      <div className="wr-h1">你的年度 TOP5</div>
      {r.top.length === 0 ? (
        <div className="wr-muted-blk">今年暂无观看时长数据</div>
      ) : (
        <div className="wr-tops">
          {r.top.map((t, i) => (
            <div className="wr-top" key={`${t.name}-${i}`}>
              <div className="wr-medal">{MEDALS[i] ?? ""}</div>
              <div className="wr-topbody">
                <div className="wr-tophead">
                  <span className="wr-topname">{t.name}</span>
                  <span className="wr-topmeta">
                    {fmtHours(t.ms)} 小时{t.finished || t.prog >= 1 ? " · 已看完" : ""}
                  </span>
                </div>
                <div className="wr-topbar">
                  <div className="wr-topfill" style={{ width: `${Math.max(4, Math.round((t.ms / max) * 100))}%` }} />
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function SummaryPage({ r }: { r: YearReport }) {
  const doneRate = r.titles ? Math.round((r.finished / r.titles) * 100) : 0;
  return (
    <div className="wr-pg wr-pg-cover">
      <Kicker text="PAGE 5 · 总结" />
      <div className="wr-sum-h">
        {r.year} 年，最大连续观影 <span className="wr-grad">{r.maxStreak} 天</span>
      </div>
      <div className="wr-sum-row">
        <div>
          <b>{doneRate}%</b>
          <span>看完率</span>
        </div>
        <div>
          <b>{r.rated}</b>
          <span>打过分</span>
        </div>
        <div>
          <b>{r.activeDays}</b>
          <span>观影天数</span>
        </div>
      </div>
      <div className="wr-sub">期待 {r.year + 1} 年继续与你相伴 🎬</div>
    </div>
  );
}

/* ── 导出长图（canvas 程序化绘制；纯文字/图形，无外链图片 → 画布不被污染） ── */

function exportReportImage(r: YearReport): void {
  const W = 720;
  const H = 1500;
  const cv = document.createElement("canvas");
  cv.width = W;
  cv.height = H;
  const ctx = cv.getContext("2d");
  if (!ctx) return;
  const bg = ctx.createLinearGradient(0, 0, 0, H);
  bg.addColorStop(0, "#fafbfe");
  bg.addColorStop(1, "#eef1fa");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, W, H);
  const ink = "#2f3550";
  const sub = "#5a6480";
  const accent = (x: number, y: number, w: number, h: number) => {
    const g = ctx.createLinearGradient(x, y, x + w, y + h);
    g.addColorStop(0, "#6d7ff2");
    g.addColorStop(1, "#8a63e8");
    ctx.fillStyle = g;
    ctx.fillRect(x, y, w, h);
  };
  const text = (t: string, x: number, y: number, size: number, weight: string, color: string, align: CanvasTextAlign = "left") => {
    ctx.fillStyle = color;
    ctx.font = `${weight} ${size}px "Microsoft YaHei",sans-serif`;
    ctx.textAlign = align;
    ctx.fillText(t, x, y);
  };

  text(`ANIBOX · ${r.year} 年度观影报告`, W / 2, 70, 13, "800", "#8a93ad", "center");
  text("这一年，你在 AniBox 看了", W / 2, 118, 15, "700", sub, "center");
  accent(W / 2 - 92, 138, 184, 62);
  text(fmtHours(r.totalMs), W / 2, 182, 52, "900", "#ffffff", "center");
  text("小时", W / 2 + 104, 182, 18, "800", ink, "center");
  const c3: [string, string][] = [
    ["部作品", String(r.titles)],
    ["天有观影", String(r.activeDays)],
    ["部看完", String(r.finished)],
  ];
  c3.forEach(([l, v], i) => {
    const x = W / 2 + (i - 1) * 150;
    text(v, x, 246, 26, "900", ink, "center");
    text(l, x, 268, 11, "600", sub, "center");
  });

  // 月度
  let y0 = 360;
  text("月度节奏", 46, y0, 20, "900", ink);
  const maxMonth = Math.max(...r.monthMs, 1);
  const peak = r.monthMs.indexOf(Math.max(...r.monthMs));
  if (r.totalMs > 0) text("最猛的一个月：" + (peak + 1) + " 月", W - 46, y0, 13, "700", "#4a5fd0", "right");
  const bw = 34;
  const gap = (W - 92 - bw * 12) / 11;
  for (let i = 0; i < 12; i++) {
    const h = Math.max(4, Math.round((r.monthMs[i] / maxMonth) * 130));
    const x = 46 + i * (bw + gap);
    if (i === peak) accent(x, y0 + 30 + 130 - h, bw, h);
    else {
      ctx.fillStyle = "rgba(109,127,242,.30)";
      ctx.fillRect(x, y0 + 30 + 130 - h, bw, h);
    }
    text(i + 1 + "月", x + bw / 2, y0 + 30 + 148, 10, "600", sub, "center");
  }

  // 时刻
  y0 += 240;
  const hourTotal = r.hourMs.reduce((a, b) => a + b, 0);
  if (hourTotal > 0) {
    const owl = r.nightRatio > 0.25 ? "重度夜猫 🦉" : r.nightRatio > 0.1 ? "轻度夜猫 🌙" : "养生作息 ☀️";
    text(`观影时刻 · ${owl}（深夜占比 ${(r.nightRatio * 100).toFixed(1)}%）`, 46, y0, 20, "900", ink);
  } else {
    text("观影时刻 · 暂无可用的观看时刻数据", 46, y0, 20, "900", ink);
  }
  const maxHour = Math.max(...r.hourMs, 1);
  const hw = (W - 92) / 24 - 4;
  for (let h = 0; h < 24; h++) {
    const hh = Math.max(3, Math.round((r.hourMs[h] / maxHour) * 90));
    const x = 46 + h * (hw + 4);
    ctx.fillStyle = h < 5 ? "#6d7ff2" : "rgba(109,127,242,.35)";
    ctx.fillRect(x, y0 + 24 + 90 - hh, hw, hh);
    if (h % 6 === 0) text(String(h).padStart(2, "0"), x + hw / 2, y0 + 24 + 108, 9, "600", sub, "center");
  }

  // TOP5
  y0 += 200;
  text("年度 TOP5", 46, y0, 20, "900", ink);
  const maxTop = r.top.length ? r.top[0].ms : 1;
  r.top.forEach((t, i) => {
    const yy = y0 + 34 + i * 52;
    text(MEDALS[i], 46, yy + 16, 17, "700", ink);
    text(t.name.slice(0, 22), 78, yy + 14, 14, "700", ink);
    text(`${fmtHours(t.ms)} 小时${t.finished || t.prog >= 1 ? " · 已看完" : ""}`, W - 46, yy + 14, 11, "600", sub, "right");
    ctx.fillStyle = "rgba(109,127,242,.14)";
    ctx.fillRect(78, yy + 22, W - 78 - 46, 7);
    accent(78, yy + 22, Math.max(6, Math.round((t.ms / maxTop) * (W - 78 - 46))), 7);
  });

  // 总结
  y0 += 330;
  const doneRate = r.titles ? Math.round((r.finished / r.titles) * 100) : 0;
  accent(W / 2 - 150, y0, 300, 2);
  text(`${r.year} 年，最大连续观影 ${r.maxStreak} 天`, W / 2, y0 + 52, 24, "900", ink, "center");
  const c5: [string, string][] = [
    ["看完率", doneRate + "%"],
    ["打过分", String(r.rated)],
    ["观影天数", String(r.activeDays)],
  ];
  c5.forEach(([l, v], i) => {
    const x = W / 2 + (i - 1) * 150;
    text(v, x, y0 + 108, 26, "900", ink, "center");
    text(l, x, y0 + 130, 11, "600", sub, "center");
  });
  text("AniBox 生成 · 数据来自本机播放记录", W / 2, H - 30, 10.5, "600", "#8a93ad", "center");

  const a = document.createElement("a");
  a.href = cv.toDataURL("image/png");
  a.download = `AniBox_年度观影报告_${r.year}.png`;
  a.click();
}
