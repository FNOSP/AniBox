/* [lc-2077] 发现页 IPTV 栏（占位转真）—— 数据直接用开源聚合项目 vbskycn/iptv 的现成 m3u
   （GPL-3.0，服务端每 6 小时更新；Rust 侧取/解析/缓存/回退见 src-tauri/src/iptv.rs）。
   UI：分组 chips（全部/央视/卫视/电影/…）+ 频道格子（台标 + 台名）；点频道 → iptv_play
   直链起播（mpv 直连，不经 Go 代理，与飞牛直播同口径）。
   NOT IMPLEMENTED：EPG 节目单 / 自定义订阅地址 / 频道搜索 —— 后续接入。 */

import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DState } from "./shared";

type IptvChan = { name: string; logo: string; url: string };
type IptvGroup = { name: string; channels: IptvChan[] };
/** stale=true = 全部数据源拉取失败，回吐的是过期缓存（旧数据比报错强，但要如实说明） */
type IptvResp = { updated?: number; groups?: IptvGroup[]; stale?: boolean };

/** 数据更新时间（HH:MM；直播源新鲜度有意义，与来源注明同排栏底右侧） */
function timeLabel(ts: number | undefined): string {
  if (!ts) return "";
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

export function IptvPane({ active }: { active: boolean }) {
  const [groups, setGroups] = useState<IptvGroup[]>([]);
  const [stale, setStale] = useState(false);
  const [updated, setUpdated] = useState(0);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [nonce, setNonce] = useState(0);
  const [group, setGroup] = useState(""); // 当前分组（"" = 全部）
  const [playing, setPlaying] = useState(""); // 正在起播的频道名（点击反馈 + 防重复点击）
  const [playErr, setPlayErr] = useState(""); // 起播失败：内联提示，**不**顶掉已加载的频道列表

  useEffect(() => {
    if (!active) return;
    let alive = true;
    setBusy(true);
    setErr("");
    invoke<IptvResp>("iptv_channels_get")
      .then((d) => {
        if (!alive) return;
        setGroups(Array.isArray(d?.groups) ? d.groups : []);
        setStale(d?.stale === true);
        setUpdated(typeof d?.updated === "number" ? d.updated : 0);
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
  }, [active, nonce]);

  const rows = useMemo(() => {
    const pick = group ? groups.filter((g) => g.name === group) : groups;
    return pick.flatMap((g) => g.channels);
  }, [groups, group]);

  async function play(ch: IptvChan) {
    if (playing) return;
    setPlayErr("");
    setPlaying(ch.name);
    try {
      await invoke("iptv_play", { title: ch.name, url: ch.url });
    } catch (e) {
      setPlayErr(String(e));
    } finally {
      setPlaying("");
    }
  }

  if (busy && !groups.length) {
    return (
      <div className="dlib-stub">
        <div className="dlib-stub-t">加载中…</div>
      </div>
    );
  }
  if (err) {
    return <DState err={err} emptyText="" onRetry={() => setNonce((n) => n + 1)} />;
  }
  if (!groups.length) {
    return <DState err="" emptyText="暂无频道数据" onRetry={() => setNonce((n) => n + 1)} />;
  }

  return (
    <div className="dip">
      <div className="dip-chips">
        <button type="button" className={`dip-chip${group === "" ? " on" : ""}`} onClick={() => setGroup("")}>
          全部
        </button>
        {groups.map((g) => (
          <button
            key={g.name}
            type="button"
            className={`dip-chip${group === g.name ? " on" : ""}`}
            onClick={() => setGroup(group === g.name ? "" : g.name)}
          >
            {g.name}
          </button>
        ))}
      </div>
      {/* 内联提示（过期缓存 / 起播失败）：一行带过，不抢内容区 */}
      {stale && <div className="dip-hint">直播源暂时拉不到最新，显示的是缓存数据</div>}
      {playErr && <div className="dip-hint">{playErr}</div>}
      <div className="dip-grid">
        {rows.map((ch, i) => (
          <button
            key={`${i}-${ch.url}`}
            type="button"
            className="dip-card"
            title={ch.name}
            onClick={() => play(ch)}
            disabled={playing === ch.name}
            /* 入场错峰：照抄 TMDB 网格（dlib-gcard）的 animationDelay 手法 */
            style={{ animationDelay: `${Math.min(i, 15) * 0.02}s` }}
          >
            <span className="dip-logo">
              <b aria-hidden="true">{ch.name.slice(0, 1)}</b>
              {ch.logo ? (
                <img
                  src={ch.logo}
                  alt=""
                  loading="lazy"
                  draggable={false}
                  onError={(e) => e.currentTarget.remove()}
                />
              ) : null}
            </span>
            <span className="dip-name">{ch.name}</span>
          </button>
        ))}
      </div>
      {/* 底行：GPL-3.0 来源注明（vbskycn/iptv 要求引用注明来源）+ 数据更新时间 */}
      <div className="dip-src">
        <span>直播源数据 vbskycn/iptv（GPL-3.0）</span>
        {updated > 0 && <span>更新于 {timeLabel(updated)}</span>}
      </div>
    </div>
  );
}
