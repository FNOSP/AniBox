/* [lc-2077] 发现页 IPTV 栏（占位转真）—— 数据直接用开源聚合项目 vbskycn/iptv 的现成 m3u
   （GPL-3.0，服务端每 6 小时更新；Rust 侧取/解析/缓存/回退见 src-tauri/src/iptv.rs）。
   UI：分组 chips（全部/央视/卫视/电影/…）+ 频道格子（台标 + 台名）；点频道 → iptv_play
   直链起播（mpv 直连，不经 Go 代理，与飞牛直播同口径）。
   NOT IMPLEMENTED：EPG 节目单 / 自定义订阅地址 / 频道搜索 —— 后续接入。 */

import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Star as IcoStar, Tv as IcoTv } from "lucide-react";
import { DState } from "./shared";

type IptvChan = { name: string; logo: string; url: string };
type IptvGroup = { name: string; channels: IptvChan[] };
/** stale=true = 全部数据源拉取失败，回吐的是过期缓存（旧数据比报错强，但要如实说明） */
type IptvResp = { updated?: number; groups?: IptvGroup[]; stale?: boolean };

/** ── 频道收藏（本地持久化）：点击卡片即起播，收藏走卡片右上角的星标（不占点击）。
    键 = 频道 url（同源多频道极少见，重复 url 视为同一频道）。存 localStorage ——
    与项目里「源管理勾选 / 季度缓存」同一套前端持久化惯例。 ── */
const FAV_KEY = "anibox.iptv.favs";

function loadFavs(): IptvChan[] {
  try {
    const v = JSON.parse(localStorage.getItem(FAV_KEY) || "[]");
    return Array.isArray(v) ? v.filter((x) => x && typeof x.url === "string") : [];
  } catch {
    return [];
  }
}

/** 数据更新时间（HH:MM；直播源新鲜度有意义，与来源注明同排栏底右侧） */
function timeLabel(ts: number | undefined): string {
  if (!ts) return "";
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** 台标：只显示图（透明 PNG，底下不垫字 —— 垫字会透出来）；缺图/挂图退化为居中 Tv 图标
    占位（真实数据 434 条全带 tvg-logo，占位是罕见路径；首字方案已废 —— 单字太丑）。
    referrerPolicy=no-referrer：部分台标站挡热链 referer，去掉能救回一批。 */
function DipLogo({ logo }: { logo: string }) {
  const [dead, setDead] = useState(!logo);
  return dead ? (
    <IcoTv className="dip-phico" size={26} strokeWidth={1.6} aria-hidden="true" />
  ) : (
    <img
      src={logo}
      alt=""
      loading="lazy"
      decoding="async"
      referrerPolicy="no-referrer"
      draggable={false}
      onError={() => setDead(true)}
    />
  );
}

export function IptvPane({ active }: { active: boolean }) {
  const [groups, setGroups] = useState<IptvGroup[]>([]);
  const [stale, setStale] = useState(false);
  const [updated, setUpdated] = useState(0);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [nonce, setNonce] = useState(0);
  const [group, setGroup] = useState<string>(""); // 当前分组（"" = 全部；"__fav" = 收藏）
  const [term, setTerm] = useState(""); // 频道名过滤（本地即时过滤；Esc 清空）
  const [favs, setFavs] = useState<IptvChan[]>(loadFavs);
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

  const favUrls = useMemo(() => new Set(favs.map((f) => f.url)), [favs]);

  /** 收藏/取消（点星标，不触发起播）；localStorage 同步写，失败静默（下次会话丢这次勾选）。 */
  function toggleFav(ch: IptvChan) {
    setFavs((prev) => {
      const next = prev.some((f) => f.url === ch.url)
        ? prev.filter((f) => f.url !== ch.url)
        : [...prev, { name: ch.name, logo: ch.logo, url: ch.url }];
      try {
        localStorage.setItem(FAV_KEY, JSON.stringify(next));
      } catch {
        /* 存储满/被禁：不影响本次会话内的收藏态 */
      }
      return next;
    });
  }

  const rows = useMemo(() => {
    const all =
      group === "__fav" ? favs : (group ? groups.filter((g) => g.name === group) : groups).flatMap((g) => g.channels);
    const t = term.trim().toLowerCase();
    return t ? all.filter((c) => c.name.toLowerCase().includes(t)) : all;
  }, [groups, group, term, favs]);

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
        <button
          type="button"
          className={`dip-chip${group === "__fav" ? " on" : ""}`}
          onClick={() => setGroup(group === "__fav" ? "" : "__fav")}
        >
          收藏
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
        {/* 频道搜索（同短剧栏的玻璃胶囊输入框；频道全在本地 → 输入即过滤，Esc 清空） */}
        <input
          className="dip-search"
          placeholder="搜索频道"
          value={term}
          onChange={(e) => setTerm(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") setTerm("");
          }}
        />
      </div>
      {/* 内联提示（过期缓存 / 起播失败）：一行带过，不抢内容区 */}
      {stale && <div className="dip-hint">直播源暂时拉不到最新，显示的是缓存数据</div>}
      {playErr && <div className="dip-hint">{playErr}</div>}
      {rows.length === 0 ? (
        <DState
          err=""
          emptyText={
            group === "__fav" && !favs.length
              ? "还没有收藏的频道（点频道右上角的星标收藏）"
              : term.trim()
                ? `未找到与「${term.trim()}」相关的频道`
                : "该分组暂无频道"
          }
          onRetry={() => {}}
        />
      ) : (
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
              <DipLogo logo={ch.logo} />
              {/* 收藏星标：压在台标上的角标（与 .dlib-badge 同一「固定深底」做法）；
                  点它不触发起播（stopPropagation），卡片本体点击仍是直接播放 */}
              <span
                role="button"
                title={favUrls.has(ch.url) ? "取消收藏" : "收藏"}
                className={`dip-fav${favUrls.has(ch.url) ? " on" : ""}`}
                onClick={(e) => {
                  e.stopPropagation();
                  toggleFav(ch);
                }}
              >
                <IcoStar size={13} strokeWidth={2} className={`rating-ico${favUrls.has(ch.url) ? " fill" : ""}`} />
              </span>
            </span>
            <span className="dip-name">{ch.name}</span>
          </button>
          ))}
        </div>
      )}
      {/* 底行：GPL-3.0 来源注明（vbskycn/iptv 要求引用注明来源）+ 数据更新时间 */}
      <div className="dip-src">
        <span>直播源数据 vbskycn/iptv（GPL-3.0）</span>
        {updated > 0 && <span>更新于 {timeLabel(updated)}</span>}
      </div>
    </div>
  );
}
