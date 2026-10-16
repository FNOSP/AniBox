/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CalendarDays as IcoCalL } from "lucide-react";
import { BgmAccountCard } from "./BgmAccountCard";
import { BgmCalRow } from "./BgmCalRow";
import { SeasonArchive } from "./SeasonArchive";
import { Dsk, WEEKDAYS, localDay, localWeekdayId } from "./shared";
import { libHasTitle, normTitle, resolveLibItem, useLibIndex } from "../../hooks/useLibIndex";
import { BgmCalendarDay, BgmCalendarItem, MediaItem } from "../../types";


export function BgmPane({ active, onOpen, onOpenLib }: { active: boolean; onOpen: (it: BgmCalendarItem) => void; onOpenLib: (item: MediaItem) => void }) {
  const todayId = localWeekdayId();
  const [days, setDays] = useState<BgmCalendarDay[]>([]);
  const [wd, setWd] = useState<number>(todayId); // 默认今天（设计稿的"今天"带点，也默认展开）
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [nonce, setNonce] = useState(0);
  const [probe, setProbe] = useState(0); // [lc-123-rev5] 10 分钟后台探测
  const [collect, setCollect] = useState<{ id: string; kind: number; name: string }[]>([]); // [lc-1581] 追番表（含名字）
  // [lc-1582] 精确放送时刻（AniList 官方 API）：条目 id → { ep, at }（一次拿"选中那天"整张表，按标题匹配）
  const [airMap, setAirMap] = useState<Record<string, { ep: number; at: number }>>({});
  // [lc-1589] 「总表查询」弹窗（往季番表，数据走 AniList）
  const [arcOpen, setArcOpen] = useState(false);
  // 手动 ⟳ / 失败重试的语义不同（见下）：⟳ = force（**仍带条件头**）；失败重试 = force + noEtag（全量）
  const forceRef = useRef(false);
  const hardRef = useRef(false);
  const daysRef = useRef<BgmCalendarDay[]>([]);
  useEffect(() => {
    daysRef.current = days;
  }, [days]);
  // 库标题索引：与日历**并行**拉（不阻塞列表；没回来就先按普通列表渲染，回来再补标签/分组）
  const lib = useLibIndex(active);
  // [lc-1563] 点开统一入口：先查库索引 —— 命中（已在库）→ 开库内条目（真 guid，可播）；
  // 未命中 → 保持原链路（discover-* 占位详情，无源时右栏为在线资源面板）。
  const openBgm = (it: BgmCalendarItem) => {
    const hit = resolveLibItem(lib.byTitle, it.name_cn || it.name);
    // [lc-1564] onOpenLib 兜底：极端混版（热更半套）下缺回调也不让点击抛错，退原链路
    if (hit && onOpenLib) onOpenLib(hit);
    else onOpen(it);
  };

  useEffect(() => {
    if (!active) return;
    let alive = true;
    const force = forceRef.current; // 手动 ⟳：跳过本地 TTL 检查（但条件头照带）
    const noEtag = hardRef.current; // 失败重试：连条件头都不带（强制全量）
    forceRef.current = false;
    hardRef.current = false;
    setBusy(true);
    setErr("");
    invoke<{ days?: BgmCalendarDay[]; changed?: boolean }>("bgm_calendar", {
      force,
      noEtag,
      day: localDay(), // 跨天分桶键：本地零点一过就换键 ⇒ 必然重取
    })
      .then((d) => {
        if (!alive) return;
        // changed=false（命中缓存 / 304 / 内容未变）且已有数据 → **一个 state 都不碰**（零 re-render）
        if (d?.changed === false && daysRef.current.length) return;
        setDays(Array.isArray(d?.days) ? d.days : []);
      })
      .catch((e) => {
        if (alive) setErr(String(e));
      })
      .finally(() => {
        if (alive) setBusy(false);
      });
    return () => {
      alive = false;
    };
  }, [active, nonce, probe]);

  // [lc-1580] 本地追番表（subject_id → 收藏类型）：算「今日已追订阅更新」用。
  // 与日历**同频**刷新（进页面 / ⟳ / 10 分钟探测）—— 表是本地文件、读取近乎零成本；
  // 在详情页改了收藏、切回发现页（active 变化）就会重跑本 effect ⇒ 提醒实时跟上。
  useEffect(() => {
    if (!active) return;
    let alive = true;
    invoke<{ list?: { id: string; kind: number; name: string }[] }>("bgm_collect_stats")
      .then((d) => {
        if (alive) setCollect(Array.isArray(d?.list) ? d.list : []);
      })
      .catch(() => {
        /* 未绑定 / 读取失败：保持空表（提醒显示「暂无更新」，不报错） */
      });
    return () => {
      alive = false;
    };
  }, [active, nonce, probe]);

  // 后台探测：每 10 分钟一次；命令层只在 TTL（3h）过期时才真发请求，而且带 ETag —— 未变就 304，
  // changed=false ⇒ 上面一个 state 都不碰（不 re-render、不重置滚动）。
  useEffect(() => {
    if (!active) return;
    const t = window.setInterval(() => setProbe((n) => n + 1), 10 * 60 * 1000);
    return () => window.clearInterval(t);
  }, [active]);

  // 按 **weekday.id** 取组（不是数组下标）；找不到就当这天空（不拿别的天凑）
  const day = days.find((g) => g.id === wd) ?? null;
  const dayItems = day?.items ?? [];
  // 星期胶囊的迷你占比条：以当周最大值（至少 1）为 100%
  const maxCount = Math.max(1, ...days.map((g) => g.count || 0));
  const weekTotal = days.reduce((n, g) => n + (g.count || 0), 0);
  // 当天汇总：部数 + 8.0+ 数量 + [lc-1580] 已追订阅更新数（原「热度最高」已按用户要求替换）
  const hiCount = dayItems.filter((x) => typeof x.score === "number" && (x.score ?? 0) >= 8).length;
  // [lc-1581] 「已追订阅」口径 = 本地追番表里 在看(3) / 想看(1) 的条目（排除 看过/搁置/放弃）。
  // ⚠ **必须 id + 标题双匹配**：BGM 把每一季算独立条目 —— 日历里「药屋少女的呢喃 第三季」(568244)、
  // 追番表里「药屋少女的呢喃」(420628)，只按 id 求交集会漏（用户实测「今天应该有一部更新」）。
  // 标题匹配复用库索引那套（normTitle + 全等→前缀兜底，能吃掉「 第三季」这类季/篇后缀）。
  const subIds = useMemo(() => {
    const s = new Set<string>();
    for (const e of collect) if (e.kind === 3 || e.kind === 1) s.add(e.id);
    return s;
  }, [collect]);
  const subNames = useMemo(() => {
    const s = new Set<string>();
    for (const e of collect) if ((e.kind === 3 || e.kind === 1) && e.name) s.add(normTitle(e.name));
    return s;
  }, [collect]);
  const subUpdated = dayItems.filter(
    (it) => subIds.has(String(it.id)) || libHasTitle(subNames, it.name_cn || it.name),
  ).length;

  // [lc-1582] 行的「第 N 话 · HH:MM」角标：**BGM 没有"几点更新"**（infobox 只有 放送星期/放送开始/
  // 电视台），改从 **AniList 官方 API** 的当日放送表拿（`airingSchedules` 带精确到分钟的 `airingAt`）。
  // 一次拿"选中那天"的整张表（Rust 侧 1~4 页 + 按天落盘缓存）⇒ 比"逐条打 bgm_episodes（14~26 次）"
  // 省得多，还多了时刻。匹配按**归一化标题**（全等 → 前缀兜底，与库索引同口径）；匹配不到就不显示，
  // 失败也静默（不影响列表本身）。
  const dayRange = (() => {
    const d = new Date();
    d.setDate(d.getDate() + (wd - todayId)); // 所选星期在本周的那一天（今天 = todayId）
    d.setHours(0, 0, 0, 0);
    const start = Math.floor(d.getTime() / 1000);
    return { start, end: start + 86400 };
  })();
  useEffect(() => {
    if (!active || !dayItems.length) return;
    let alive = true;
    invoke<{ items?: { episode: number; airingAt: number; titles: string[] }[] }>(
      "anilist_day_schedule",
      { start: dayRange.start, end: dayRange.end },
    )
      .then((d) => {
        if (!alive) return;
        // 归一化标题索引（AniList 给 native/romaji/english 三种，全进）
        const idx = new Map<string, { ep: number; at: number }>();
        for (const s of d?.items ?? []) {
          for (const t of s.titles ?? []) {
            const n = normTitle(t);
            if (n.length >= 2 && !idx.has(n)) idx.set(n, { ep: s.episode, at: s.airingAt });
          }
        }
        const next: Record<string, { ep: number; at: number }> = {};
        for (const it of dayItems) {
          const keys = [normTitle(it.name_cn || it.name), normTitle(it.name)].filter((k) => k.length >= 2);
          let hit = keys.map((k) => idx.get(k)).find(Boolean);
          if (!hit) {
            const kk = keys.filter((k) => k.length >= 3);
            for (const [t, v] of idx) {
              if (t.length >= 3 && kk.some((k) => t.startsWith(k) || k.startsWith(t))) {
                hit = v;
                break;
              }
            }
          }
          if (hit) next[String(it.id)] = hit;
        }
        setAirMap(next);
      })
      .catch(() => {
        /* 拿不到就静默：行上不显示时刻（列表照常） */
      });
    return () => {
      alive = false;
    };
    // 依赖刻意用 days/wd（dayItems 每次渲染都是新数组，进依赖会每帧重跑）；数据变了才重算。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, wd, days]);

  // 行上显示：有集号 → 「第 N 话 · HH:MM」，没集号 → 只给时刻（本地时区）
  const epLabelOf = (id: number): string | undefined => {
    const a = airMap[String(id)];
    if (!a) return undefined;
    const d = new Date(a.at * 1000);
    const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
    return a.ep > 0 ? `第 ${a.ep} 话 · ${hm}` : hm;
  };
  const wdCn = WEEKDAYS.find((w) => w.id === wd)?.cn ?? "";
  // 逐条匹配：loading/partial 只给"命中"打标；只有 ok（索引完整）才敢分组和标"不在库"
  const withLib = lib.state === "ok" || lib.state === "partial";
  const rows = dayItems.map((it, i) => ({
    it,
    rank: i + 1, // 序号 = 该天列表内的位置（分组重排后也保持原序）
    inLib: withLib ? libHasTitle(lib.titles, it.name_cn || it.name) : false,
  }));
  const inRows = rows.filter((r) => r.inLib);
  const outRows = rows.filter((r) => !r.inLib);
  const renderRow = (r: (typeof rows)[number]) => (
    <BgmCalRow
      key={r.it.id}
      it={r.it}
      today={wd === todayId}
      rank={r.rank}
      libState={r.inLib ? "in" : null}
      epLabel={epLabelOf(r.it.id)}
      onOpen={openBgm}
    />
  );

  return (
    <section className="dpane">
      <header className="dpane-hd">
        <h2>新番更新</h2>
        <span className="dn">{weekTotal > 0 ? `${weekTotal} 部 / 周` : ""}</span>
        <span className="dsp" />
        <button
          className="dicon"
          title="刷新新番列表"
          disabled={busy}
          onClick={() => {
            forceRef.current = true;
            setNonce((n) => n + 1);
          }}
        >
          ⟳
        </button>
      </header>
      {/* [lc-1516] Bangumi 账号面板：紧跟栏头（避开顶部 56px 的胶囊让位区）
          [lc-1538] 底部通栏改「今日状态条」：当前选中日的汇总由这里算好传进去
          （原先那条 .ddaybar 已从列表上方移入卡片，列表不再重复显示） */}
      <BgmAccountCard onOpen={openBgm} />
      {/* [lc-1583] 「今日状态条」**独立成容器**（原先嵌在追番卡里当底部通栏）—— 用户要求：
          它汇总的是**当前选中日**，属于日历区，不该塞在账号卡内部。 */}
      {dayItems.length > 0 && (
        <div className="dtoday">
          <b>
            {wdCn}
            {wd === todayId ? "（今天）" : ""}
          </b>
          <span>
            {wd === todayId ? `今日 ${dayItems.length} 部新番更新` : `${dayItems.length} 部更新`}
            {hiCount > 0 ? ` · ${hiCount} 部评分 8.0+` : ""}
            {subUpdated > 0 ? ` · 已追订阅 ${subUpdated} 部更新` : " · 已追订阅暂无更新"}
          </span>
        </div>
      )}
      <div className="dweeks">
        {WEEKDAYS.map((w) => {
          const g = days.find((x) => x.id === w.id);
          // [lc-1539] 柱高 = 该天部数 / 当周最大值（占比 ≥6% 才可见，0 部不画）
          const pct = g && g.count > 0 ? Math.max(6, Math.round((g.count / maxCount) * 100)) : 0;
          return (
            <button
              key={w.id}
              className={`dweek${wd === w.id ? " on" : ""}${todayId === w.id ? " today" : ""}`}
              onClick={() => setWd(w.id)}
              title={g ? `${w.cn}共 ${g.count} 部` : w.cn}
            >
              {/* [lc-1539] 底色占比填充（迷你柱状图，替代原独立细条） */}
              <span className="dweek-fill" style={{ height: `${pct}%` }} aria-hidden="true" />
              <span className="dweek-d">{w.cn}</span>
              <i>{g ? `${g.count} 部` : "…"}</i>
            </button>
          );
        })}
        {/* [lc-1589] 星期七格右侧的「总表查询」：打开往季番表弹窗（选年份 + 季度） */}
        <button className="darc-btn" onClick={() => setArcOpen(true)} title="查往季番表（选年份 + 季度）">
          <IcoCalL size={14} strokeWidth={2} />
          总表查询
        </button>
      </div>
      <div className="dscroll">
        {busy && !days.length ? (
          <ul className="dlist">
            {[0, 1, 2, 3].map((i) => (
              <li key={i} className="ditem">
                <Dsk cls="dsk-cv" />
                <div className="dtx">
                  <Dsk cls="dsk-line" />
                  <Dsk cls="dsk-line short" />
                </div>
              </li>
            ))}
          </ul>
        ) : err ? (
          <div className="dstate">
            <div className="dstate-msg">{err}</div>
            <button
              className="dretry"
              onClick={() => {
                // 失败重试：force + noEtag（强制全量重拉，别拿坏缓存反复撞）
                hardRef.current = true;
                setNonce((n) => n + 1);
              }}
            >
              重试
            </button>
          </div>
        ) : !day || !day.items.length ? (
          <div className="dstate">
            {wd === todayId ? "今天没有新番更新" : `${wdCn}没有新番更新`}
          </div>
        ) : (
          <>
            <ul className="dlist">
              {lib.state === "ok" ? (
                <>
                  {inRows.length > 0 && <li className="dgrp">已在飞牛库 · {inRows.length}</li>}
                  {inRows.map(renderRow)}
                  {outRows.length > 0 && <li className="dgrp">不在库 · {outRows.length}</li>}
                  {outRows.map(renderRow)}
                </>
              ) : (
                rows.map(renderRow)
              )}
            </ul>
          </>
        )}
      </div>
      {/* [lc-1589] 「总表查询」弹窗（往季番表，数据走 AniList 官方 API） */}
      {arcOpen && <SeasonArchive onClose={() => setArcOpen(false)} />}
    </section>
  );
}
