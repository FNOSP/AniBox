/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Film as IcoFilmL } from "lucide-react";
import { bgmImg } from "../../lib/images";
import { BgmCalendarItem } from "../../types";

export type BgmAccountState = { bound: boolean; username: string };

export type BgmCollectStats = { total: number; byType: Record<string, number> };
/** [lc-1534] 「正在追」封面条条目（Rust bgm_watching_covers：本地表 type=3 按最近更新倒序） */

export type BgmWatchCover = { id: number; name: string; cover: string };

/** 追番状态五元组（Bangumi type 值 / 标签 / 统计配色） */

export const BGAC_TYPES: { k: number; label: string; color: string }[] = [
  { k: 1, label: "想看", color: "#E9A23C" },
  { k: 3, label: "在看", color: "#37A75F" },
  { k: 2, label: "看过", color: "#4A87F0" },
  { k: 4, label: "搁置", color: "#9AA1A8" },
  { k: 5, label: "放弃", color: "#ED5B52" },
];


export function BgmAccountCard({ onOpen }: { onOpen?: (it: BgmCalendarItem) => void }) {
  const [acct, setAcct] = useState<BgmAccountState>({ bound: false, username: "" });
  const [stats, setStats] = useState<BgmCollectStats>({ total: 0, byType: {} });
  const [watch, setWatch] = useState<BgmWatchCover[]>([]); // [lc-1534] 「正在追」封面条
  const [season, setSeason] = useState<{ total: number; seasonTotal: number; byType: Record<string, number> }>(
    { total: 0, seasonTotal: 0, byType: {} },
  );
  const [bindOpen, setBindOpen] = useState(false); // 连接表单展开态
  // [lc-1528] 同步状态图联动高亮：null = 无；数字 = BGAC_TYPES 下标
  const [hl, setHl] = useState<number | null>(null);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [syncBusy, setSyncBusy] = useState(false);
  const [err, setErr] = useState("");
  const [note, setNote] = useState("");

  const refresh = useCallback(async () => {
    try {
      const s = await invoke<BgmAccountState>("bgm_account_status");
      setAcct(s);
    } catch {
      /* 状态查询失败不打扰用户 */
    }
    try {
      const st = await invoke<BgmCollectStats>("bgm_collect_stats");
      setStats(st);
    } catch {
      /* ignore */
    }
    try {
      const ss = await invoke<{ total: number; seasonTotal: number; byType: Record<string, number> }>(
        "bgm_collect_season",
      );
      setSeason(ss);
    } catch {
      /* ignore */
    }
    // [lc-1534] 「正在追」封面条（本地优先；缺封面时命令层会现拉一次并回填）
    try {
      const wc = await invoke<{ items: BgmWatchCover[] }>("bgm_watching_covers", { limit: 12 });
      setWatch(Array.isArray(wc?.items) ? wc.items : []);
    } catch {
      /* ignore */
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const bind = async () => {
    const t = token.trim();
    if (!t) return;
    setBusy(true);
    setErr("");
    setNote("");
    try {
      const s = await invoke<BgmAccountState>("bgm_account_bind", { token: t });
      setAcct(s);
      setToken("");
      setBindOpen(false);
      setNote(`已连接 Bangumi 账号「${s.username}」`);
      refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const unbind = async () => {
    if (!window.confirm(`解绑 Bangumi 账号「${acct.username}」？本地追番状态会保留。`)) return;
    setErr("");
    setNote("");
    try {
      await invoke("bgm_account_unbind");
      setAcct({ bound: false, username: "" });
      setNote("已解绑");
    } catch (e) {
      setErr(String(e));
    }
  };

  const syncNow = async () => {
    setSyncBusy(true);
    setErr("");
    setNote("");
    try {
      const r = await invoke<{ uploaded: number; downloaded: number; remoteTotal: number }>(
        "bgm_sync_now",
        { priority: "local" },
      );
      setNote(`同步完成 · 下载 ${r.downloaded} · 上传 ${r.uploaded}`);
      refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setSyncBusy(false);
    }
  };

  // ── 右侧观看统计：donut（五状态占比）+ 分项计数 ──
  const total = Math.max(0, Number(stats.total ?? 0));
  const counts = (k: number) => Number(stats.byType?.[String(k)] ?? 0);
  let acc = 0;
  const segs = BGAC_TYPES.map((t) => {
    const n = counts(t.k);
    const frac = total > 0 ? n / total : 0;
    const seg = { ...t, n, frac, a0: acc, a1: acc + frac };
    acc += frac;
    return seg;
  });

  // [lc-1528] 同步状态图几何（按用户设计稿重算，适配对半列宽；viewBox 200×150）
  const VB_W = 206; // 右侧多留 6 单位给胶囊右缘内边距（数字不再贴边）
  const VB_H = 150;
  const CX = 58;
  const CY = 86;
  const R_OUT = 46;
  const R_IN = 32;
  const R_LEADER = 58;
  const ROW_GAP = 26;
  const DOT_X = 126;
  const LABEL_X = 140;
  const VALUE_X = 194;
  const ROW_BG_X = DOT_X - 8; // 胶囊左缘（用户要求：胶囊背景矩形整体右移，贴近圆点）
  const LEADER_END_X = ROW_BG_X - 2; // 引线终点停在胶囊左缘外 2px，可见段更长
  const ROW_N = BGAC_TYPES.length;
  const rowY = (i: number) => CY - ((ROW_N - 1) * ROW_GAP) / 2 + i * ROW_GAP;
  const pt = (r: number, deg: number) => {
    const rad = (deg * Math.PI) / 180;
    return [CX + r * Math.sin(rad), CY - r * Math.cos(rad)] as const;
  };
  /** 环形扇形路径（外弧正向 + 内弧反向） */
  const sectorPath = (a0: number, a1: number) => {
    const d0 = -90 + a0 * 360;
    const d1 = -90 + a1 * 360;
    const [x0, y0] = pt(R_OUT, d0);
    const [x1, y1] = pt(R_OUT, d1);
    const [x2, y2] = pt(R_IN, d1);
    const [x3, y3] = pt(R_IN, d0);
    const large = d1 - d0 > 180 ? 1 : 0;
    return `M${x0.toFixed(2)},${y0.toFixed(2)}A${R_OUT},${R_OUT} 0 ${large} 1 ${x1.toFixed(2)},${y1.toFixed(2)}L${x2.toFixed(2)},${y2.toFixed(2)}A${R_IN},${R_IN} 0 ${large} 0 ${x3.toFixed(2)},${y3.toFixed(2)}Z`;
  };
  /** 引线：扇形边缘 → 沿 R_LEADER 圆弧绕到图例行方向 → 直线连到行（不穿过环本体） */
  const leaderPath = (mid: number, ry: number) => {
    const [x0, y0] = pt(R_OUT + 2, -90 + mid * 360);
    const [x1, y1] = pt(R_LEADER, -90 + mid * 360);
    const dx = LEADER_END_X - CX;
    const dy = ry - CY;
    const phi = (Math.atan2(dx, -dy) * 180) / Math.PI;
    const [x2, y2] = pt(R_LEADER, phi);
    const d = ((phi - (-90 + mid * 360) + 540) % 360) - 180;
    const large = Math.abs(d) > 180 ? 1 : 0;
    const sweep = d > 0 ? 1 : 0;
    return `M${x0.toFixed(2)},${y0.toFixed(2)}L${x1.toFixed(2)},${y1.toFixed(2)}A${R_LEADER},${R_LEADER} 0 ${large} ${sweep} ${x2.toFixed(2)},${y2.toFixed(2)}L${LEADER_END_X},${ry.toFixed(2)}`;
  };

  return (
    <div className={`bgmac${acct.bound ? " on" : ""}`}>
      {/* 左侧：标题组 + 按钮（图标按用户要求删除） */}
      <div className="bgmac-main">
        <div className="bgmac-text">
          <div className="bgmac-kicker">Bangumi</div>
          <div className="bgmac-title-row">
            <span className="bgmac-title">追番同步</span>
            <span className={`bgmac-badge${acct.bound ? " on" : ""}`} title={acct.bound ? `已连接：${acct.username}` : "未连接 Bangumi 账号"}>
              {acct.bound ? (
                <>
                  <svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M4 12.5 9 17.5 20 6.5" />
                  </svg>
                  <span className="bgmac-badge-name">{acct.username}</span>
                </>
              ) : (
                "未连接"
              )}
            </span>
          </div>
          <div className="bgmac-desc">与 Bangumi 保持相同的追番状态。</div>
          <div className="bgmac-types">想看 · 在看 · 看过 · 搁置 · 抛弃</div>
        </div>
        {/* [lc-1534] 「正在追」封面条：填面板中部留白；点封面 → 该条目详情 */}
        {acct.bound && watch.length > 0 && (
          <div className="bgmac-watch">
            <div className="bgmac-watch-hd">
              正在追 <b>{Math.max(counts(3), watch.length)}</b> 部
            </div>
            <div className="bgmac-watch-row">
              {watch.slice(0, 5).map((w) => (
                <button
                  key={w.id}
                  className={`bgmac-wcover${w.cover ? "" : " empty"}`}
                  title={w.name}
                  onClick={() =>
                    onOpen?.({
                      id: w.id,
                      type: 2,
                      name: w.name,
                      name_cn: w.name,
                      air_date: "",
                      votes: 0,
                      doing: 0,
                      cover: w.cover,
                    })
                  }
                >
                  {w.cover ? (
                    <img src={bgmImg(w.cover)} alt="" loading="lazy" draggable={false} />
                  ) : (
                    <IcoFilmL size={16} strokeWidth={1.8} />
                  )}
                </button>
              ))}
            </div>
          </div>
        )}
        {acct.bound ? (
          <div className="bgmac-acts">
            <button className="bgmac-act" disabled={syncBusy} onClick={syncNow}>
              {syncBusy ? "同步中…" : "立即同步"}
            </button>
            <button className="bgmac-link" onClick={unbind}>
              解绑
            </button>
            {note && !err && <span className="bgmac-inline-note">{note}</span>}
          </div>
        ) : bindOpen ? (
          <div className="bgmac-bind">
            <input
              value={token}
              onChange={(e) => setToken(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !busy && bind()}
              placeholder="粘贴 Bangumi Access Token"
              autoFocus
            />
            <button className="bgmac-cta sm" disabled={busy || !token.trim()} onClick={bind}>
              {busy ? "连接中…" : "连接"}
            </button>
          </div>
        ) : (
          <button className="bgmac-cta" onClick={() => setBindOpen(true)}>
            → 连接 Bangumi
          </button>
        )}
      </div>

      {/* 右上：同步状态图（标题在 SVG 内，用户设计稿） */}
      <div className="bgmac-stats">
        {acct.bound ? (
          <div className="bgmac-stats-body">
            {/* [lc-1528] 同步状态图（用户设计稿移植）：环形图 + 引线联动图例行。
                悬停扇形/图例行 → 该段高亮其余变暗 + 引线连到该行。 */}
            <svg className="bgmac-chart" viewBox={`0 0 ${VB_W} ${VB_H}`} role="img" aria-label="同步状态">
              <text className="bgmac-chart-title" x={VB_W / 2} y="13" textAnchor="middle" dominantBaseline="central">
                同步状态
              </text>
              {total > 0 ? (
                segs
                  .filter((s) => s.n > 0)
                  .map((s) => {
                    const i = segs.indexOf(s);
                    return (
                      <path
                        key={s.k}
                        className="bgmac-sector"
                        d={sectorPath(s.a0, s.a1)}
                        fill={s.color}
                        opacity={hl == null || hl === i ? 1 : 0.32}
                        onMouseEnter={() => setHl(i)}
                        onMouseLeave={() => setHl(null)}
                      >
                        <title>{`${s.label} ${s.n}`}</title>
                      </path>
                    );
                  })
              ) : (
                <path className="bgmac-sector" d={sectorPath(0, 1)} fill="var(--ph-bg)" />
              )}
              {total > 0 && (
                <>
                  <text className="bgmac-chart-num" x={CX} y={CY - 7} textAnchor="middle" dominantBaseline="central">
                    {total}
                  </text>
                  <text className="bgmac-chart-sub" x={CX} y={CY + 11} textAnchor="middle" dominantBaseline="central">
                    部番剧
                  </text>
                </>
              )}
              {/* 引线：当前高亮段的连线 */}
              {hl != null && (
                <path
                  className="bgmac-leader show"
                  d={leaderPath(segs[hl].frac > 0 ? (segs[hl].a0 + segs[hl].a1) / 2 : 0, rowY(hl))}
                  stroke={segs[hl].color}
                />
              )}
              {/* 图例行 */}
              {segs.map((s, i) => (
                <g
                  key={s.k}
                  className={`bgmac-chart-row${hl === i ? " hl" : ""}`}
                  onMouseEnter={() => setHl(i)}
                  onMouseLeave={() => setHl(null)}
                >
                  <rect className="bgmac-row-bg" x={ROW_BG_X} y={rowY(i) - 10} width={VALUE_X + 8 - ROW_BG_X} height={20} rx="6" />
                  <circle cx={DOT_X} cy={rowY(i)} r="4.5" fill={s.color} />
                  <text className="bgmac-chart-label" x={LABEL_X} y={rowY(i)} dominantBaseline="central">
                    {s.label}
                  </text>
                  <text className="bgmac-chart-value" x={VALUE_X} y={rowY(i)} textAnchor="end" dominantBaseline="central">
                    {s.n}
                  </text>
                </g>
              ))}
            </svg>
          </div>
        ) : (
          <div className="bgmac-stats-empty">
            <span className="bgmac-block-t">同步状态</span>
            <svg viewBox="0 0 100 100" width="76" height="76">
              <circle cx="50" cy="50" r="42" fill="none" stroke="var(--line-strong)" strokeWidth="7" strokeDasharray="4 6" />
            </svg>
            <span>连接后展示追番统计</span>
          </div>
        )}
      </div>

      {/* 底部通栏：当季追番状态（本季放送 ∩ 本地追番表） */}
      <div className="bgmac-season">
        <div className="bgmac-block-t">当季追番状态</div>
        {acct.bound ? (
          season.total > 0 ? (
            <>
              <div className="bgmac-season-bar">
                {BGAC_TYPES.map((t) => {
                  const n = Number(season.byType?.[String(t.k)] ?? 0);
                  if (n <= 0) return null;
                  return (
                    <i
                      key={t.k}
                      className="bgmac-season-seg"
                      style={{ width: `${(n / season.total) * 100}%`, background: t.color }}
                    />
                  );
                })}
              </div>
              <div className="bgmac-season-legend">
                {BGAC_TYPES.map((t) => {
                  const n = Number(season.byType?.[String(t.k)] ?? 0);
                  if (n <= 0) return null;
                  return (
                    <span key={t.k} className="bgmac-legend-row">
                      <i className="bgmac-dot" style={{ background: t.color }} />
                      {t.label}
                      <b>{n}</b>
                    </span>
                  );
                })}
                <span className="bgmac-season-total">
                  当季 {season.seasonTotal} 部中已标记 {season.total} 部
                </span>
              </div>
            </>
          ) : (
            <div className="bgmac-season-empty">
              当季 {season.seasonTotal > 0 ? `${season.seasonTotal} 部番剧` : "番剧"}暂无追番标记
            </div>
          )
        ) : (
          <div className="bgmac-season-empty">连接 Bangumi 后展示当季追番分布</div>
        )}
      </div>

      {/* 错误/提示收在左列内（不再整卡宽灰条）；已连接的成功提示走行内小字 */}
      {err && <div className="bgmac-err">{err}</div>}
      {!acct.bound && note && !err && <div className="bgmac-note">{note}</div>}
      {!acct.bound && bindOpen && (
        <div className="bgmac-tip">在 bangumi.tv 个人设置 → 访问令牌 生成，粘贴后即可同步</div>
      )}
    </div>
  );
}
