/* [lc-2078] 发现页短剧栏（占位转真）—— 数据来自 guoapp「红果鉴」Go 核心（duanju_core.dll，
   GPL-3.0 与本仓库同许可；Rust 侧 FFI 桥见 src-tauri/src/duanju.rs）。
   UI：站源 chips（红果/剧果/禁果短剧/野果/黑桃/黄豆）+ 分类 chips + 搜索 + 竖版海报网格；
   点卡片 → 详情浮层（简介 + 选集 30/组）→ 点集 duanju_play（核心本地流直链 → mpv）。
   NOT IMPLEMENTED：推荐/榜单 Tab、弹幕、追剧、下载、多清晰度切换、Linux/macOS 核心。 */

import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DState } from "./shared";
import { djImg } from "../../lib/images";
import { X as IcoXL } from "lucide-react";

type DuanjuDrama = {
  id: string;
  source?: string;
  sourceId?: string;
  title: string;
  description?: string;
  cover?: string;
  episodes?: number;
  category?: string;
  vip?: boolean | null;
  heat?: string;
  views?: string;
  score?: string;
  tags?: string[];
  releaseStatus?: string;
  badge?: string;
  [k: string]: unknown;
};

type DuanjuChapter = {
  id: string;
  title?: string;
  videoUrl?: string;
  currentEpisode?: unknown;
  mediaSize?: number;
  [k: string]: unknown;
};

type CatalogResp = {
  items?: DuanjuDrama[];
  hasMore?: boolean;
  page?: number;
  warning?: string;
  localSearch?: boolean;
};
type DetailResp = { drama?: DuanjuDrama; chapters?: DuanjuChapter[]; warning?: string };

/** 站源（id 与上游 Go sourceXxx 常量一致；顺序即展示顺序） */
const SOURCES: { id: string; label: string }[] = [
  { id: "hongguo", label: "红果" },
  { id: "jinguo", label: "剧果" },
  { id: "jinguoduanju", label: "禁果短剧" },
  { id: "yeguo", label: "野果" },
  { id: "heitao", label: "黑桃" },
  { id: "huangdou", label: "黄豆" },
];
const EP_GROUP = 30; // 选集分组（与上游详情页同口径）

export function DuanjuPane() {
  const [source, setSource] = useState("hongguo");
  const [cats, setCats] = useState<{ id: string; name: string }[]>([]);
  const [cat, setCat] = useState("");
  const [query, setQuery] = useState("");
  const [term, setTerm] = useState(""); // 已提交的搜索词（回车触发）
  const [items, setItems] = useState<DuanjuDrama[]>([]);
  const [page, setPage] = useState(1);
  const [hasMore, setHasMore] = useState(false);
  const [busy, setBusy] = useState(false);
  const [moreBusy, setMoreBusy] = useState(false);
  const [err, setErr] = useState("");
  const [nonce, setNonce] = useState(0);

  // 详情浮层
  const [detail, setDetail] = useState<DetailResp | null>(null);
  const [dBusy, setDBusy] = useState(false);
  const [dErr, setDErr] = useState("");
  const [epGroup, setEpGroup] = useState(0);
  const [playingIdx, setPlayingIdx] = useState(-1);
  const [playErr, setPlayErr] = useState("");

  // 分类（随站源重取）
  useEffect(() => {
    let alive = true;
    setCats([]);
    setCat("");
    invoke<{ id: string; name: string }[]>("duanju_categories", { source })
      .then((d) => {
        if (alive && Array.isArray(d)) setCats(d);
      })
      .catch(() => {
        /* 分类拉取失败不阻塞目录（上游会回「全部」） */
      });
    return () => {
      alive = false;
    };
  }, [source]);

  // 目录第一页（站源/分类/搜索词变化即重取）
  useEffect(() => {
    if (!source) return;
    let alive = true;
    setBusy(true);
    setErr("");
    setItems([]);
    setPage(1);
    setHasMore(false);
    invoke<CatalogResp>("duanju_catalog", {
      source,
      category: cat,
      page: 1,
      query: term,
    })
      .then((d) => {
        if (!alive) return;
        setItems(Array.isArray(d?.items) ? d.items : []);
        setHasMore(d?.hasMore === true);
        setPage(d?.page || 1);
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
  }, [source, cat, term, nonce]);

  async function loadMore() {
    if (moreBusy || !hasMore) return;
    setMoreBusy(true);
    try {
      const next = page + 1;
      const d = await invoke<CatalogResp>("duanju_catalog", {
        source,
        category: cat,
        page: next,
        query: term,
      });
      const fresh = Array.isArray(d?.items) ? d.items : [];
      const seen = new Set(items.map((x) => x.id));
      setItems((prev) => [...prev, ...fresh.filter((x) => !seen.has(x.id))]);
      setHasMore(d?.hasMore === true);
      setPage(d?.page || next);
    } catch (e) {
      setErr(String(e));
    } finally {
      setMoreBusy(false);
    }
  }

  async function openDetail(d: DuanjuDrama) {
    setDetail(null);
    setDBusy(true);
    setDErr("");
    setEpGroup(0);
    setPlayErr("");
    setPlayingIdx(-1);
    try {
      const resp = await invoke<DetailResp>("duanju_detail", { drama: d });
      setDetail({ drama: resp?.drama || d, chapters: resp?.chapters || [] });
    } catch (e) {
      setDErr(String(e));
    } finally {
      setDBusy(false);
    }
  }

  async function play(idx: number, chapter: DuanjuChapter, name: string) {
    if (playingIdx >= 0 || !detail?.drama) return;
    setPlayErr("");
    setPlayingIdx(idx);
    try {
      await invoke("duanju_play", {
        drama: detail.drama,
        chapter,
        index: idx,
        quality: null,
        title: `${detail.drama.title || name} ${chapter.title || `第${idx + 1}集`}`,
      });
    } catch (e) {
      setPlayErr(String(e));
    } finally {
      setPlayingIdx(-1);
    }
  }

  const chapters = detail?.chapters || [];
  const epGroups = useMemo(() => {
    const out: DuanjuChapter[][] = [];
    for (let i = 0; i < chapters.length; i += EP_GROUP) out.push(chapters.slice(i, i + EP_GROUP));
    return out;
  }, [chapters]);

  return (
    <div className="dj">
      {/* 站源行 */}
      <div className="dj-sources">
        {SOURCES.map((s) => (
          <button
            key={s.id}
            type="button"
            className={`dj-chip${source === s.id ? " on" : ""}`}
            onClick={() => {
              setSource(s.id);
              setTerm("");
              setQuery("");
            }}
          >
            {s.label}
          </button>
        ))}
      </div>

      {/* 分类 + 搜索 */}
      <div className="dj-row">
        <div className="dj-chips dj-cats">
          <button
            type="button"
            className={`dj-chip sm${cat === "" ? " on" : ""}`}
            onClick={() => setCat("")}
          >
            全部
          </button>
          {cats.map((c) => (
            <button
              key={c.id}
              type="button"
              className={`dj-chip sm${cat === c.id ? " on" : ""}`}
              onClick={() => setCat(c.id)}
            >
              {c.name}
            </button>
          ))}
        </div>
        <input
          className="dj-search"
          placeholder="搜索短剧"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") setTerm(query.trim());
          }}
        />
      </div>

      {busy ? (
        <div className="dlib-stub">
          <div className="dlib-stub-t">加载中…</div>
        </div>
      ) : err ? (
        <DState err={err} emptyText="" onRetry={() => setNonce((n) => n + 1)} />
      ) : items.length === 0 ? (
        <DState
          err=""
          emptyText={term ? `未找到与「${term}」相关的短剧` : "暂无内容"}
          onRetry={() => setNonce((n) => n + 1)}
        />
      ) : (
        <>
          <div className="dj-grid">
            {items.map((d) => (
              <button key={d.id} type="button" className="dj-card" onClick={() => openDetail(d)}>
                <span className="dj-poster">
                  {d.cover ? (
                    <img src={djImg(d.cover)} alt="" loading="lazy" draggable={false} />
                  ) : (
                    <span className="dj-ph" aria-hidden="true" />
                  )}
                  {d.vip === true && <span className="dj-vip">VIP</span>}
                </span>
                <span className="dj-title">{d.title}</span>
                <span className="dj-meta">
                  {[d.episodes ? `${d.episodes}集` : "", d.score ? `${d.score}分` : "", d.heat || d.views || ""]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
              </button>
            ))}
          </div>
          {hasMore && (
            <div className="dj-more">
              <button type="button" className="dj-morebtn" disabled={moreBusy} onClick={loadMore}>
                {moreBusy ? "加载中…" : "加载更多"}
              </button>
            </div>
          )}
        </>
      )}

      {/* 详情浮层（面板内覆盖，不占独立路由） */}
      {(dBusy || detail || dErr) && (
        <div className="dj-mask" onClick={() => !dBusy && setDetail(null)}>
          <div className="dj-detail" onClick={(e) => e.stopPropagation()}>
            {dBusy ? (
              <div className="dj-detail-tip">加载中…</div>
            ) : dErr ? (
              <>
                <div className="dj-detail-tip">{dErr}</div>
                <button type="button" className="dj-morebtn" onClick={() => setDetail(null)}>
                  关闭
                </button>
              </>
            ) : (
              detail && (
                <>
                  <button
                    type="button"
                    className="dj-close"
                    aria-label="关闭详情"
                    onClick={() => setDetail(null)}
                  >
                    <IcoXL size={16} strokeWidth={1.8} />
                  </button>
                  <div className="dj-dhead">
                    {detail.drama?.cover && (
                      <img className="dj-dposter" src={djImg(detail.drama.cover)} alt="" draggable={false} />
                    )}
                    <div className="dj-dinfo">
                      <div className="dj-dtitle">{detail.drama?.title}</div>
                      <div className="dj-dmeta">
                        {[
                          detail.drama?.episodes ? `${detail.drama.episodes}集` : "",
                          detail.drama?.releaseStatus === "finished"
                            ? "已完结"
                            : detail.drama?.releaseStatus === "ongoing"
                              ? "连载中"
                              : "",
                          detail.drama?.category || "",
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </div>
                      {(detail.drama?.tags || []).length > 0 && (
                        <div className="dj-dtags">
                          {detail.drama!.tags!.slice(0, 6).map((t) => (
                            <span key={t} className="dj-tag">
                              {t}
                            </span>
                          ))}
                        </div>
                      )}
                      {detail.drama?.description && (
                        <div className="dj-ddesc">{detail.drama.description}</div>
                      )}
                    </div>
                  </div>
                  {epGroups.length > 1 && (
                    <div className="dj-epgroups">
                      {epGroups.map((_, gi) => (
                        <button
                          key={gi}
                          type="button"
                          className={`dj-chip sm${epGroup === gi ? " on" : ""}`}
                          onClick={() => setEpGroup(gi)}
                        >
                          {gi * EP_GROUP + 1}-{Math.min((gi + 1) * EP_GROUP, chapters.length)}集
                        </button>
                      ))}
                    </div>
                  )}
                  <div className="dj-eps">
                    {(epGroups[epGroup] || []).map((ch, i) => {
                      const idx = epGroup * EP_GROUP + i;
                      return (
                        <button
                          key={ch.id || idx}
                          type="button"
                          className="dj-ep"
                          disabled={playingIdx >= 0}
                          onClick={() => play(idx, ch, detail.drama?.title || "")}
                        >
                          {playingIdx === idx ? "起播中…" : ch.title || `第${idx + 1}集`}
                        </button>
                      );
                    })}
                  </div>
                  {playErr && <div className="dj-hint">{playErr}</div>}
                  <div className="dj-src">站源数据 guoapp（红果鉴，GPL-3.0）</div>
                </>
              )
            )}
          </div>
        </div>
      )}
    </div>
  );
}
