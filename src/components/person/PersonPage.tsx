/* [lc-1700] 演员页（TMDB / Bangumi 人物）。
   - 顶部横幅：左竖版头像 + 右姓名/标签 chips/简介
   - 中部筛选：全部 / 剧集 / 电影 + 作品数徽章
   - 下方自适应海报墙：PersonWorkCard（不复用 PosterCard，避免触发飞牛播放/收藏）
   三态：骨架 / 错误重试 / 空态。入场错峰动画由 CSS（--pw-i）驱动，
   prefers-reduced-motion 下自动关闭（见 43-person.css）。 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { bgmImg, tmdbImg } from "../../lib/images";
import type { PersonInfo, PersonSource, PersonTarget, PersonWork } from "../../types";
import { PersonWorkCard } from "./PersonWorkCard";

type Filter = "all" | "series" | "movie" | "stock";

/** [lc-1701] 作品稳定键：与去重口径一致，用于把后端匹配结果映射回卡片 */
function workKey(w: PersonWork): string {
  return w.tmdbId ? `t${w.tmdbId}` : w.bgmId ? `b${w.bgmId}` : `${w.title}|${w.year ?? ""}`;
}

/** TMDB 部门名 → 中文（未知原样展示） */
const DEPARTMENT_CN: Record<string, string> = {
  Acting: "演员",
  Directing: "导演",
  Writing: "编剧",
  Production: "制片",
  Camera: "摄影",
  Editing: "剪辑",
  Art: "美术",
  Sound: "音效",
  "Costume & Make-Up": "服装化妆",
  "Visual Effects": "特效",
  Lighting: "灯光",
  Crew: "制作",
};

/** 把 TMDB / Bangumi 的媒体类型归到筛选桶。
 *  - tmdb：movie → 电影；tv → 剧集
 *  - bgm：anime → 剧集；real → subjectType 6（三次元）且标题不含集数信息时归电影，否则剧集；
 *         other 无法判断 → 只在「全部」里出现（卡片仍带类型标签）。 */
function bucketOf(w: PersonWork): Filter {
  if (w.mediaType === "movie") return "movie";
  if (w.mediaType === "tv" || w.mediaType === "anime") return "series";
  if (w.mediaType === "real") {
    const t = String(w.title || "");
    const hasEpisodeInfo = /第\s*\d+\s*[集话話期]|S\d+\s*E\d+|\bEP?\s*\d+|\b\d+\s*集/i.test(t);
    return Number(w.subjectType) === 6 && !hasEpisodeInfo ? "movie" : "series";
  }
  return "all";
}

function profileUrl(source: PersonSource, p?: string): string {
  const s = String(p ?? "").trim();
  if (!s) return "";
  if (source === "tmdb") return s.startsWith("http") ? s : tmdbImg(s, "w500");
  return bgmImg(s);
}

/** 作品去重（TMDB 同一 id / BGM 同一 id 只保留一次；无 id 时用标题+年份兜底） */
function dedupeWorks(works: PersonWork[]): PersonWork[] {
  const seen = new Set<string>();
  const out: PersonWork[] = [];
  for (const w of works) {
    const key = workKey(w);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(w);
  }
  return out;
}

/** 后端返回 → PersonInfo（TMDB / Bangumi 两种形状归一） */
function normalizeTmdb(raw: Record<string, unknown>): PersonInfo {
  const works = Array.isArray(raw.works) ? (raw.works as PersonWork[]) : [];
  return {
    source: "tmdb",
    id: Number(raw.tmdbId ?? raw.id ?? 0) || 0,
    name: String(raw.name ?? ""),
    originalName: String(raw.originalName ?? ""),
    profile: profileUrl("tmdb", String(raw.profile ?? "")),
    biography: String(raw.biography ?? ""),
    birthday: String(raw.birthday ?? ""),
    deathday: String(raw.deathday ?? ""),
    placeOfBirth: String(raw.placeOfBirth ?? ""),
    knownFor: String(raw.knownFor ?? ""),
    works: dedupeWorks(works.filter((w) => w && w.title)),
  };
}

function normalizeBgm(
  raw: Record<string, unknown>,
  worksFromList: PersonWork[] | null,
): PersonInfo {
  const imgs = (raw.images ?? {}) as Record<string, string>;
  const rawWorks = worksFromList && worksFromList.length
    ? worksFromList
    : Array.isArray(raw.works)
      ? (raw.works as PersonWork[])
      : [];
  return {
    source: "bgm",
    id: Number(raw.bgmId ?? raw.id ?? 0) || 0,
    name: String(raw.name ?? ""),
    originalName: String(raw.originalName ?? ""),
    profile: profileUrl("bgm", imgs.large || imgs.medium || imgs.grid || imgs.small),
    summary: String(raw.summary ?? ""),
    birthday: String(raw.birth ?? ""),
    bloodType: String(raw.bloodType ?? ""),
    career: Array.isArray(raw.career) ? (raw.career as string[]) : [],
    works: dedupeWorks(rawWorks.filter((w) => w && w.title)),
  };
}

async function loadPerson(target: PersonTarget): Promise<PersonInfo> {
  if (target.source === "tmdb") {
    const raw = await invoke<Record<string, unknown>>("tmdb_person", { arg: { id: target.id } });
    return normalizeTmdb(raw ?? {});
  }
  const [info, worksRes] = await Promise.all([
    invoke<Record<string, unknown>>("bgm_person", { arg: { id: target.id } }),
    invoke<Record<string, unknown>>("bgm_person_works", { arg: { id: target.id } }).catch(() => null),
  ]);
  const list = worksRes && Array.isArray(worksRes.works) ? (worksRes.works as PersonWork[]) : null;
  return normalizeBgm(info ?? {}, list);
}

export function PersonPage({
  target,
  onBack,
  onOpenWork,
}: {
  target: PersonTarget;
  onBack: () => void;
  /** 点击作品：由 App 用现有 openTmdbRecommendation / openDiscoverBgm 链路打开详情 */
  onOpenWork?: (work: PersonWork) => void;
}) {
  const [info, setInfo] = useState<PersonInfo | null>(null);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const [nonce, setNonce] = useState(0);
  const [filter, setFilter] = useState<Filter>("all");
  const [bioOpen, setBioOpen] = useState(false);
  // [lc-1701] 入库状态：ok 才会显示徽章/「已入库」筛选；失败或空库静默降级
  const [stock, setStock] = useState<{ state: "idle" | "loading" | "ok" | "fail"; owned: Set<string> }>(
    { state: "idle", owned: new Set() },
  );

  useEffect(() => {
    let alive = true;
    setBusy(true);
    setErr("");
    setInfo(null);
    setBioOpen(false);
    setFilter("all");
    loadPerson(target)
      .then((p) => {
        if (!alive) return;
        setInfo(p);
        setBusy(false);
      })
      .catch((e) => {
        if (!alive) return;
        setErr(String(e));
        setBusy(false);
      });
    return () => {
      alive = false;
    };
  }, [target.source, target.id, nonce]);

  const works = info?.works ?? [];

  /* [lc-1701] 作品入库匹配：人物资料到位后一次性批量请求（后端拉一次库、本地比对），
     失败/无库数据不阻塞页面，仅不显示徽章。 */
  useEffect(() => {
    const list = info?.works ?? [];
    if (!info || list.length === 0) {
      setStock({ state: "idle", owned: new Set() });
      return;
    }
    let alive = true;
    setStock({ state: "loading", owned: new Set() });
    invoke<{ owned?: boolean[]; guids?: (string | null)[] }>("match_library_works", {
      arg: {
        works: list.map((w) => ({
          tmdbId: w.tmdbId ?? null,
          bgmId: w.bgmId ?? null,
          title: w.title ?? "",
          originalTitle: w.originalTitle ?? "",
          year: w.year ?? null,
          mediaType: w.mediaType,
        })),
      },
    })
      .then((r) => {
        if (!alive) return;
        const owned = new Set<string>();
        const flags = Array.isArray(r?.owned) ? r.owned : [];
        list.forEach((w, i) => {
          if (flags[i]) owned.add(workKey(w));
        });
        setStock({ state: "ok", owned });
      })
      .catch(() => {
        if (alive) setStock({ state: "fail", owned: new Set() });
      });
    return () => {
      alive = false;
    };
  }, [info]);

  const counts = useMemo(() => {
    let movie = 0;
    let series = 0;
    let stocked = 0;
    for (const w of works) {
      const b = bucketOf(w);
      if (b === "movie") movie += 1;
      else if (b === "series") series += 1;
      if (stock.owned.has(workKey(w))) stocked += 1;
    }
    return { total: works.length, movie, series, stock: stocked };
  }, [works, stock]);

  const shown = useMemo(() => {
    if (filter === "all") return works;
    if (filter === "stock") return works.filter((w) => stock.owned.has(workKey(w)));
    return works.filter((w) => bucketOf(w) === filter);
  }, [works, filter, stock]);

  const displayName = info?.name || target.name;
  const avatar = info?.profile || profileUrl(target.source, target.profile);
  const bio = info?.biography || info?.summary || "";
  const bioLong = bio.length > 220;

  const tags: string[] = [];
  if (info?.knownFor) tags.push(DEPARTMENT_CN[info.knownFor] || info.knownFor);
  if (info?.career?.length) tags.push(...info.career.slice(0, 4));
  if (info?.birthday) tags.push(`生日 ${info.birthday}`);
  if (info?.placeOfBirth) tags.push(info.placeOfBirth);
  if (info?.bloodType) tags.push(`血型 ${info.bloodType}`);

  const tabs: { key: Filter; label: string; n: number }[] = [
    { key: "all", label: "全部", n: counts.total },
    { key: "series", label: "剧集", n: counts.series },
    { key: "movie", label: "电影", n: counts.movie },
  ];
  // [lc-1701] 库数据可用时才出现「已入库」筛选（失败/无库不显示空标签）
  if (stock.state === "ok") tabs.push({ key: "stock", label: "已入库", n: counts.stock });

  const retry = useCallback(() => setNonce((n) => n + 1), []);

  return (
    <main className="content pp-page">
      <div className="pp-topbar">
        <button className="back" onClick={onBack} title="返回上一页">
          ‹ 返回
        </button>
      </div>

      {/* ① 骨架 */}
      {busy && !info && !err && (
        <div className="pp-skel" aria-busy="true" aria-label="加载中">
          <div className="pp-skel-banner">
            <div className="pp-skel-avatar" />
            <div className="pp-skel-lines">
              <div className="pp-skel-line w40" />
              <div className="pp-skel-line w70" />
              <div className="pp-skel-line w90" />
              <div className="pp-skel-line w80" />
            </div>
          </div>
          <div className="pp-skel-grid">
            {Array.from({ length: 12 }).map((_, i) => (
              <div className="pp-skel-card" key={i} />
            ))}
          </div>
        </div>
      )}

      {/* ② 错误重试 */}
      {err && !info && (
        <div className="pp-state">
          <div className="pp-state-t">人物资料加载失败</div>
          <div className="pp-state-d">{err}</div>
          <button className="pp-state-btn" onClick={retry}>
            重试
          </button>
        </div>
      )}

      {/* ③ 数据 */}
      {info && (
        <>
          <section className="pp-banner">
            <div className="pp-avatar">
              {avatar ? (
                <img src={avatar} alt="" loading="lazy" draggable={false} />
              ) : (
                <span className="pp-avatar-fb" aria-hidden="true">
                  {String(displayName || "?").slice(0, 1)}
                </span>
              )}
            </div>
            <div className="pp-info">
              <h1 className="pp-name">
                {displayName || "未知人物"}
                {info.source === "tmdb" && <span className="pp-src">TMDB</span>}
                {info.source === "bgm" && <span className="pp-src bgm">Bangumi</span>}
              </h1>
              {info.originalName && info.originalName !== displayName && (
                <div className="pp-orig">{info.originalName}</div>
              )}
              {tags.length > 0 && (
                <div className="pp-chips">
                  {tags.map((t, i) => (
                    <span className="pp-chip" key={`${t}-${i}`}>
                      {t}
                    </span>
                  ))}
                </div>
              )}
              <div className="pp-stats">
                <span>
                  <b>{counts.total}</b> 部作品
                </span>
                <span className="sep" aria-hidden="true" />
                <span>
                  <b>{counts.movie}</b> 部电影
                </span>
                <span className="sep" aria-hidden="true" />
                <span>
                  <b>{counts.series}</b> 部剧集
                </span>
                {stock.state === "ok" && (
                  <>
                    <span className="sep" aria-hidden="true" />
                    <span>
                      <b>{counts.stock}</b> 部已入库
                    </span>
                  </>
                )}
              </div>
              {bio && (
                <div className={`pp-bio${bioOpen ? " open" : ""}`}>
                  <p>{bio}</p>
                  {bioLong && (
                    <button className="pp-bio-toggle" onClick={() => setBioOpen((v) => !v)}>
                      {bioOpen ? "收起" : "展开"}
                    </button>
                  )}
                </div>
              )}
            </div>
          </section>

          <div className="pp-toolbar">
            <div className="pp-tabs" role="tablist">
              {tabs.map((t) => (
                <button
                  key={t.key}
                  role="tab"
                  aria-selected={filter === t.key}
                  className={`pp-tab${filter === t.key ? " on" : ""}`}
                  onClick={() => setFilter(t.key)}
                >
                  {t.label}
                  <span className="pp-tab-n">{t.n}</span>
                </button>
              ))}
            </div>
            <div className="pp-total">{counts.total} 部作品</div>
          </div>

          {works.length === 0 ? (
            <div className="pp-state">
              <div className="pp-state-t">暂无作品信息</div>
              <div className="pp-state-d">这位人物在 {info.source === "tmdb" ? "TMDB" : "Bangumi"} 暂无收录作品</div>
            </div>
          ) : shown.length === 0 ? (
            <div className="pp-state">
              <div className="pp-state-t">该分类暂无作品</div>
              <div className="pp-state-d">试试「全部」</div>
            </div>
          ) : (
            <div className="pp-grid">
              {shown.map((w, i) => (
                <PersonWorkCard
                  key={`${w.tmdbId ?? w.bgmId ?? w.title}-${i}`}
                  work={w}
                  index={i}
                  owned={stock.owned.has(workKey(w))}
                  onOpen={onOpenWork}
                />
              ))}
            </div>
          )}
        </>
      )}
    </main>
  );
}
