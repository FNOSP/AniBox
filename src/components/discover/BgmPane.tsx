/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BgmAccountCard } from "./BgmAccountCard";
import { BgmCalRow } from "./BgmCalRow";
import { Dsk, WEEKDAYS, localDay, localWeekdayId } from "./shared";
import { libHasTitle, resolveLibItem, useLibIndex } from "../../hooks/useLibIndex";
import { BgmCalendarDay, BgmCalendarItem, MediaItem } from "../../types";


export function BgmPane({ active, onOpen, onOpenLib }: { active: boolean; onOpen: (it: BgmCalendarItem) => void; onOpenLib: (item: MediaItem) => void }) {
  const todayId = localWeekdayId();
  const [days, setDays] = useState<BgmCalendarDay[]>([]);
  const [wd, setWd] = useState<number>(todayId); // 默认今天（设计稿的"今天"带点，也默认展开）
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [nonce, setNonce] = useState(0);
  const [probe, setProbe] = useState(0); // [lc-123-rev5] 10 分钟后台探测
  const [collectKinds, setCollectKinds] = useState<Record<string, number>>({}); // [lc-1580] id → 收藏类型
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
    invoke<{ byId?: Record<string, number> }>("bgm_collect_stats")
      .then((d) => {
        if (alive) setCollectKinds(d?.byId ?? {});
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
  // [lc-1580] 「已追订阅」口径 = 本地追番表里 type 在看(3) / 想看(1) 的条目（排除 看过/搁置/放弃）。
  // 用当天日历条目的 id 直接查表 ⇒ 交集在本地算，实时跟着追番表变（在详情页改了收藏、回到本页即重算）。
  const subUpdated = dayItems.filter((it) => {
    const k = collectKinds[String(it.id)];
    return k === 3 || k === 1;
  }).length;
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
      <BgmAccountCard
        onOpen={openBgm}
        today={{
          wdCn,
          isToday: wd === todayId,
          count: dayItems.length,
          hiCount,
          subCount: subUpdated,
        }}
      />
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
    </section>
  );
}
