/* [lc-2247] 电影详情页「杂志风」版式（editorial）。
 *
 * 与经典版（MovieHero.tsx）的分工：
 *   · 经典版 = 满幅头图 + 自适应反色，视觉重心是背景图；
 *   · 本版 = 左文右图双栏，视觉重心是大标题 + 层叠海报，没有满幅头图。
 * 两者由设置项 movieDetailStyle 切换（默认 classic），**目前只对电影生效**。
 *
 * 数据来源与经典版完全一致（都来自 DetailView 的同一批 props），
 * 因此切换版式不会丢数据、也不额外发请求。
 *
 * 布局（与用户的静态稿一致）：
 *   左栏  ① 艺术字大标题（kicker / 标题 / 原名）
 *         ② 数据格（评分·片长·年份·地区）+ 补充小字 + 题材标签
 *         ③ 简介（吃掉剩余高度，超长内部滚动）
 *   右栏  层叠海报轮播 + 播放/收藏/元数据 + 版本条 + 主创 + 配音
 */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Star as IcoStarL } from "lucide-react";
import { imgUrl, tmdbImg } from "../../lib/images";
import { prefersReducedMotion } from "../../lib/motion";
import { TmdbCreditPerson } from "../../types";

/** 数据格里的一项：大数字 + 小标签。 */
type StatItem = { value: string; label: string; unit?: string; rating?: boolean };

type Props = {
  title: string;
  originalTitle: string;
  /** 头图（已是完整 URL；空则退回海报） */
  backdrop: string;
  poster: string;
  rating: number | null;
  votes: number;
  year: string;
  runtimeMin: number;
  genres: string[];
  overview: string;
  cast: TmdbCreditPerson[];
  /** 4K 剧照 file_path 列表（与经典版共用，用作层叠卡片） */
  stills: string[];
  /** TMDB 详情（取 images.logos 用） */
  tmdb: Record<string, unknown> | null;
  /** 上映日期（YYYY-MM-DD，可空） */
  releaseDate?: string;
  resumeAt: boolean;
  playing: boolean;
  onPlay: () => void;
  onBack: () => void;
  onMeta?: () => void;
  onOpenPerson?: (c: TmdbCreditPerson) => void;
  versions?: { mediaGuid: string; label: string; meta?: string }[];
  selectedVersion?: string;
  onVersionChange?: (mediaGuid: string) => void;
  favItem: Record<string, unknown> | null;
};

/** 从 tmdb 详情里取作品标题 logo（后端 [lc-2247] 已按「中文优先 + 面积最大」挑好）。
 *  返回 TMDB file_path（形如 /abc.png）与一张备选。
 *
 *  为什么要两张：TMDB 的 zh logo 常见是**白色字**（配深色海报用的），
 *  直接放浅色底上等于隐形。所以：
 *    · 浅色主题 → 用 alt（通常是英文深色版）
 *    · 深色主题 → 用 primary（中文白版）
 *  两者都缺时返回空串，区块不渲染。 */
function pickLogos(tmdb: Record<string, unknown> | null): { light: string; dark: string } {
  const p = String(tmdb?.titleLogo ?? "").trim();
  const a = String(tmdb?.titleLogoAlt ?? "").trim();
  const ok = (s: string) => (s.startsWith("/") ? s : "");
  return { light: ok(a) || ok(p), dark: ok(p) || ok(a) };
}

/** 国家代码 → 中文名（够用即可，未收录的原样显示）。 */
const COUNTRY_CN: Record<string, string> = {
  JP: "日本",
  CN: "中国",
  HK: "中国香港",
  TW: "中国台湾",
  KR: "韩国",
  US: "美国",
  GB: "英国",
  FR: "法国",
  DE: "德国",
  CA: "加拿大",
  AU: "澳大利亚",
  IN: "印度",
  TH: "泰国",
  SG: "新加坡",
};

/** 语言代码 → 中文名。 */
const LANG_CN: Record<string, string> = {
  ja: "日语",
  zh: "中文",
  cn: "粤语",
  en: "英语",
  ko: "韩语",
  fr: "法语",
  de: "德语",
  es: "西班牙语",
  th: "泰语",
};

/** 拼「国家 · 语言」展示串；两者都空则返回空串（前端此时不显示该数据格）。 */
function regionText(country?: string, language?: string): string {
  const c = (country ?? "").trim().toUpperCase();
  const l = (language ?? "").trim().toLowerCase();
  const cn = c ? COUNTRY_CN[c] ?? c : "";
  const ln = l ? LANG_CN[l] ?? l : "";
  return [cn, ln].filter(Boolean).join(" · ");
}

/** 数据格的构造：缺项的条目自动跳过（不占位、不留空 label）。 */
function buildStats(
  rating: number | null,
  votes: number,
  runtimeMin: number,
  year: string,
  regionLabel?: string,
): StatItem[] {
  const out: StatItem[] = [];
  if (rating != null && rating > 0) {
    out.push({ value: rating.toFixed(1), label: votes > 0 ? `${votes} 人评分` : "评分", rating: true });
  }
  if (runtimeMin > 0) out.push({ value: String(runtimeMin), unit: " 分钟", label: "片长" });
  if (year) out.push({ value: year, label: "上映年份" });
  const region = (regionLabel ?? "").split(" · ")[0]?.trim();
  if (region) out.push({ value: region, label: regionLabel ?? "" });
  return out;
}

export function MovieHeroEditorial({
  title,
  originalTitle,
  backdrop,
  poster,
  rating,
  votes,
  year,
  runtimeMin,
  genres,
  overview,
  cast,
  stills,
  tmdb,
  releaseDate,
  resumeAt,
  playing,
  onPlay,
  onBack,
  onMeta,
  onOpenPerson,
  versions,
  selectedVersion,
  onVersionChange,
  favItem,
}: Props) {
  const favGuid = String((favItem?.guid as string) ?? "").trim();
  const [fav, setFav] = useState(false);
  const [favBusy, setFavBusy] = useState(false);
  const [note, setNote] = useState("");

  useEffect(() => {
    if (!favGuid) return;
    let alive = true;
    invoke<boolean>("favorite_get", { guid: favGuid })
      .then((v) => alive && setFav(!!v))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [favGuid]);

  const toggleFav = () => {
    if (!favGuid || favBusy) return;
    setFavBusy(true);
    invoke<{ favorited?: boolean }>("favorite_toggle", { item: favItem })
      .then((r) => {
        const on = !!r?.favorited;
        setFav(on);
        setNote(on ? "已收藏，可在侧边栏「收藏」里找到" : "已取消收藏");
        window.dispatchEvent(new Event("anibox-favorites-changed"));
      })
      .catch((e) => setNote(String(e)))
      .finally(() => setFavBusy(false));
  };

  /* ── 层叠海报：backdrop / 海报 / 剧照 一起去重后叠放 ──
     经典版用同一批图做背景轮播，这里改成卡片层叠（位置 0/1/2，其余隐藏）。 */
  /* ── 层叠海报：卡片是 **16:9 横版**（[lc-2248] 用户要求），
     所以优先用横图：背板 → 剧照（都是 16:9/2.35:1 原生横图），
     竖版海报（2:3）排最后兜底 —— 它被 cover 裁进 16:9 会损失大半画面，
     只在完全没有横图时才用它，避免空白。 */
  const stillUrls = stills.map((p) => (p.startsWith("http") ? p : tmdbImg(p, "original"))).filter(Boolean);
  const posterUrl = poster ? imgUrl(poster) : "";
  const slides = Array.from(new Set([backdrop, ...stillUrls, posterUrl].filter(Boolean)));
  const slideKey = slides.join("|");
  const [slideIdx, setSlideIdx] = useState(0);
  const [hovered, setHovered] = useState(false);
  const autoRef = useRef<number | null>(null);
  const reduce = prefersReducedMotion();
  /* [lc-2249] 入场动画开关：挂载后下一帧才置真，
     这样 `.is-in` 是从"无 → 有"的**状态变化**，动画必然触发（同一帧加上不会播）。 */
  const [fanIn, setFanIn] = useState(false);
  useEffect(() => {
    if (reduce) return;
    const id = requestAnimationFrame(() => setFanIn(true));
    return () => cancelAnimationFrame(id);
  }, [reduce]);

  useEffect(() => setSlideIdx(0), [slideKey]);

  // 自动轮换：4s 一张；悬停暂停；尊重「减弱动态效果」
  useEffect(() => {
    if (slides.length <= 1 || reduce) return;
    if (hovered) return;
    const t = window.setInterval(() => setSlideIdx((i) => (i + 1) % slides.length), 4000);
    autoRef.current = t;
    return () => window.clearInterval(t);
  }, [slides.length, slideKey, hovered, reduce]);

  const topCast = cast.filter((c) => c?.name).slice(0, 4);

  /* ── 数据格：评分 / 片长 / 年份 / 地区（缺项自动跳过） ──
     地区与语言从 tmdb 的原始代码取，组件内映射中文（后端 [lc-2247] 新增字段）。 */
  const regionLabel = regionText(
    String(tmdb?.originCountry ?? ""),
    String(tmdb?.originalLanguage ?? ""),
  );
  const stats = buildStats(rating, votes, runtimeMin, year, regionLabel);

  // 作品 logo：两张（浅色主题用 light、深色主题用 dark），见 pickLogos 注释
  const logos = pickLogos(tmdb);
  const logoLight = logos.light ? tmdbImg(logos.light, "w500") : "";
  const logoDark = logos.dark ? tmdbImg(logos.dark, "w500") : "";

  return (
    <div className="mved">
      <div className="mved-inner">
        <button className="mved-back" onClick={onBack} title="返回上一页">
          ‹ 返回
        </button>

        <header className="mved-header">
          {/* ══ 左栏：标题 / 数据 / 简介 ══ */}
          <div className="mved-left">
            <div className="mved-block mved-block-title">
              <div className="mved-kicker">
                {[year, "动画长片"].filter(Boolean).join(" · ")}
              </div>
              <h1 className="mved-title">{title || "（无标题）"}</h1>
              {originalTitle && originalTitle !== title && (
                <div className="mved-subtitle">{originalTitle}</div>
              )}
            </div>

            <div className="mved-block mved-block-data">
              <div className="mved-data-main">
                {stats.length > 0 && (
                  <div className="mved-stats">
                    {stats.map((s, i) => (
                      <div className="mved-stat" key={`${s.label}-${i}`}>
                        <span className={`mved-stat-v${s.rating ? " is-rating" : ""}`}>
                          {s.rating && <IcoStarL size={14} strokeWidth={0} aria-hidden="true" />}
                          {s.value}
                          {s.unit && <span className="mved-stat-unit">{s.unit}</span>}
                        </span>
                        <span className="mved-stat-k">{s.label}</span>
                      </div>
                    ))}
                  </div>
                )}

                {(releaseDate || genres.length > 0) && (
                  <div className="mved-meta-row">
                    {releaseDate && <span>{String(releaseDate).slice(0, 10)} 上映</span>}
                    {genres.slice(0, 3).map((g) => (
                      <span key={g}>{g}</span>
                    ))}
                  </div>
                )}

                {genres.length > 0 && (
                  <div className="mved-itags" role="list" aria-label="标签">
                    {genres.slice(0, 12).map((g, i) => (
                      <span className="mved-tag" role="listitem" key={`${g}-${i}`}>
                        {g}
                      </span>
                    ))}
                  </div>
                )}
              </div>

              {/* 作品 logo：两张按主题切换（见 pickLogos），都没有则整块不渲染 */}
              {(logoLight || logoDark) && (
                <div className={`mved-data-logo${logoLight && logoDark ? "" : " mved-data-logo--single"}`}>
                  {logoLight && <img className="mved-logo-light" src={logoLight} alt="" aria-hidden="true" />}
                  {logoDark && <img className="mved-logo-dark" src={logoDark} alt="" aria-hidden="true" />}
                </div>
              )}
            </div>

            {overview && (
              <div className="mved-block mved-block-desc">
                <p className="mved-lead">{overview}</p>
              </div>
            )}
          </div>

          {/* ══ 右栏：海报 / 操作 / 版本 / 主创 / 配音 ══ */}
          <div className="mved-media">
            <div
              className={`mved-stack${fanIn ? " is-in" : ""}`}
              role="group"
              aria-label="海报轮播"
              onMouseEnter={() => setHovered(true)}
              onMouseLeave={() => setHovered(false)}
              onClick={(e) => {
                const el = (e.target as HTMLElement).closest<HTMLElement>(".mved-card");
                if (el?.dataset.pos === "0") return; // 点最前那张不切换
                if (slides.length > 1) setSlideIdx((i) => (i + 1) % slides.length);
              }}
            >
              {slides.map((url, i) => {
                const pos = (i - slideIdx + slides.length) % slides.length;
                return (
                  <div
                    key={url}
                    className="mved-card"
                    data-pos={String(pos > 2 ? 3 : pos)}
                    /* [lc-2249] --i 给入场动画做错峰延迟（0/1/2 → 0/90/180ms） */
                    style={{ backgroundImage: `url("${url}")`, ["--i" as string]: String(pos) }}
                  />
                );
              })}
            </div>

            <div className="mved-actions">
              <button className="mved-btn-primary" onClick={onPlay} disabled={playing}>
                ▶ {playing ? "打开中…" : resumeAt ? "继续播放" : "立即播放"}
              </button>
              <button
                className={`mved-btn-ghost${fav ? " on" : ""}`}
                onClick={toggleFav}
                disabled={!favGuid || favBusy}
              >
                <svg viewBox="0 0 24 24" fill={fav ? "currentColor" : "none"} stroke="currentColor" strokeWidth={fav ? 0 : 1.8}>
                  <path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" />
                </svg>
                {favBusy ? "处理中…" : fav ? "已收藏" : "收藏"}
              </button>
              {onMeta && (
                <button className="mved-btn-square" onClick={onMeta} title="元数据操作（编辑 / 手动匹配 / 刷新）" aria-label="元数据操作">
                  ⋯
                </button>
              )}
            </div>
            {note && <div className="mved-note">{note}</div>}

            {versions && versions.length > 1 && (
              <div className="mved-versions" role="group" aria-label="选择版本">
                {versions.map((v) => (
                  <button
                    key={v.mediaGuid}
                    type="button"
                    className={`mved-version${v.mediaGuid === selectedVersion ? " active" : ""}`}
                    title={v.label}
                    aria-pressed={v.mediaGuid === selectedVersion}
                    onClick={() => onVersionChange?.(v.mediaGuid)}
                  >
                    {v.label}
                  </button>
                ))}
              </div>
            )}

            {/* 配音阵容：4 等分，人多时换行；没有演职员数据时整块不渲染 */}
            {topCast.length > 0 && (
              <div className="mved-cast-row" role="list" aria-label="主要配音">
                {topCast.map((c, i) => {
                  const av = c.profile
                    ? c.profile.startsWith("/")
                      ? tmdbImg(c.profile, "w185")
                      : imgUrl(c.profile)
                    : "";
                  const clickable = !!onOpenPerson && Number(c.id ?? 0) > 0;
                  const role = c.character ? `饰 ${c.character}` : c.jobLabel || c.job || "";
                  return (
                    <div
                      className={`mved-cast${clickable ? " clickable" : ""}`}
                      key={`${c.id ?? i}`}
                      role="listitem"
                      tabIndex={clickable ? 0 : undefined}
                      onClick={clickable ? () => onOpenPerson?.(c) : undefined}
                      onKeyDown={
                        clickable
                          ? (e) => {
                              if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                onOpenPerson?.(c);
                              }
                            }
                          : undefined
                      }
                    >
                      <div className="mved-cast-avatar" style={av ? { backgroundImage: `url("${av}")` } : undefined} />
                      <div className="mved-cast-meta">
                        <div className="mved-cast-name">{c.name}</div>
                        {role && <div className="mved-cast-role">{role}</div>}
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </header>
      </div>
    </div>
  );
}
