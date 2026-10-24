/* [lc-1720] 剧集「手动获取元数据」弹窗。
   入口：海报卡「⋯ → 刷新元数据」。
   流程：默认填入当前媒体标题 → 用户可改 → 并行走 TMDB / Bangumi 镜像搜索 →
        两路结果分区展示 → 单选 → 确认后**只写本地**（metadata_set_override），
        不调用飞牛任何写接口。选定结果由 Rust 侧按剧 guid 叠加到后续所有展示。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Check as IcoCheckL, Search as IcoSearchL, Star as IcoStarL, Trash2 as IcoTrashL, X as IcoXL } from "lucide-react";
import { imgUrl } from "../../lib/images";
import { backfillFnosText, writeMetadataOverride } from "../../lib/metaApply";

export type MetadataPickTarget = {
  /** 覆盖键：剧/电影的飞牛 guid（集卡片请传父剧 guid） */
  seriesGuid: string;
  /** 搜索框默认值 */
  title: string;
  /** 提示 TMDB 搜索类型；缺省两路都搜 */
  mediaType?: "tv" | "movie";
};

type PickResult = {
  source: "tmdb" | "bgm";
  id: string;
  title: string;
  originalTitle?: string;
  year?: string;
  date?: string;
  overview?: string;
  rating?: number;
  poster?: string;
  backdrop?: string;
  seasons?: number;
  episodes?: number;
  subtitle?: string;
};

type SearchResp = {
  query?: string;
  /** [lc-1778] 原词 0 结果时 AI 清洗出的重搜标题（出现即代表已用它重搜） */
  refinedQuery?: string;
  tmdb?: PickResult[];
  bgm?: PickResult[];
  errors?: Record<string, string>;
};

const keyOf = (r: PickResult) => `${r.source}:${r.id}`;
const fmtYear = (r: PickResult) => (r.year || (r.date ?? "").slice(0, 4) || "").trim();

export function MetadataPickerModal({
  target,
  onClose,
  onDone,
}: {
  target: MetadataPickTarget;
  onClose: () => void;
  /** 保存/清除成功后调用：关闭弹窗并刷新当前视图 */
  onDone: () => void;
}) {
  const [query, setQuery] = useState(target.title);
  const [tmdb, setTmdb] = useState<PickResult[]>([]);
  const [bgm, setBgm] = useState<PickResult[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [err, setErr] = useState("");
  const [notice, setNotice] = useState("");
  const [selected, setSelected] = useState<string>("");
  const [hasOverride, setHasOverride] = useState(false);
  const alive = useRef(true);
  const autoSearched = useRef("");

  const search = useCallback(
    async (q: string) => {
      const term = q.trim();
      if (!term) return;
      setLoading(true);
      setErr("");
      setNotice("");
      setSelected("");
      try {
        const r = await invoke<SearchResp>("metadata_search", {
          query: term,
          mediaType: target.mediaType ?? null,
        });
        if (!alive.current) return;
        const t = Array.isArray(r?.tmdb) ? r.tmdb : [];
        const b = Array.isArray(r?.bgm) ? r.bgm : [];
        setTmdb(t);
        setBgm(b);
        const errs = r?.errors ?? {};
        // [lc-1778] AI 兜底提示：后端在 0 结果时用 AI 清洗标题重搜过（refinedQuery 出现即代表重搜生效）
        const refinedNote = r?.refinedQuery ? `原词无结果，已按 AI 识别标题「${r.refinedQuery}」重搜` : "";
        if (!t.length && !b.length) {
          setErr(refinedNote ? `${refinedNote}，仍无结果，换个关键词再试` : "没有搜到匹配结果，换个关键词再试");
        } else if (Object.keys(errs).length) {
          const parts = [refinedNote, `部分来源失败：${Object.entries(errs)
            .map(([k, v]) => `${k === "tmdb" ? "TMDB" : "Bangumi"}（${v}）`)
            .join("；")}`].filter(Boolean);
          setNotice(parts.join("；"));
        } else if (refinedNote) {
          setNotice(refinedNote);
        }
      } catch (e) {
        if (alive.current) setErr(String(e));
      } finally {
        if (alive.current) setLoading(false);
      }
    },
    [target.mediaType],
  );

  // 打开即读当前是否有本地覆盖 + 自动按默认标题搜一次（用户仍可改词重搜）
  useEffect(() => {
    alive.current = true;
    invoke<Record<string, unknown> | null>("metadata_get_override", { seriesGuid: target.seriesGuid })
      .then((v) => {
        if (alive.current) setHasOverride(!!v);
      })
      .catch(() => {});
    if (autoSearched.current !== target.seriesGuid) {
      autoSearched.current = target.seriesGuid;
      void search(target.title);
    }
    return () => {
      alive.current = false;
    };
  }, [target.seriesGuid, target.title, search]);

  // Esc 关闭（与详情页其它弹层一致）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !saving) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, saving]);

  const chosen = [...tmdb, ...bgm].find((r) => keyOf(r) === selected) ?? null;

  const onConfirm = useCallback(async () => {
    if (!chosen || saving) return;
    setSaving(true);
    setErr("");
    try {
      // [lc-1728] ① 本地覆盖先行（关键步骤）：写成功立即关闭弹窗并刷新当前视图；
      //   失败则留在弹窗内报红（不允许静默无反应）。
      await writeMetadataOverride(target.seriesGuid, chosen);
      onDone();
      // [lc-1728] ② best-effort 回填飞牛（标题/简介带 *_locked，防下次刮削覆盖）：
      //   失败只 console.warn + 轻提示，绝不影响本地已生效的覆盖。
      void backfillFnosText(target.seriesGuid, chosen.title, chosen.overview ?? "");
    } catch (e) {
      setErr(String(e));
    } finally {
      setSaving(false);
    }
  }, [chosen, saving, target.seriesGuid, onDone]);

  const onClear = useCallback(async () => {
    if (clearing) return;
    setClearing(true);
    setErr("");
    try {
      await invoke("metadata_clear_override", { seriesGuid: target.seriesGuid });
      onDone();
    } catch (e) {
      setErr(String(e));
    } finally {
      setClearing(false);
    }
  }, [clearing, target.seriesGuid, onDone]);

  const renderCard = (r: PickResult) => {
    const k = keyOf(r);
    const on = selected === k;
    const year = fmtYear(r);
    return (
      <button
        key={k}
        type="button"
        className={`mp-card${on ? " is-on" : ""}`}
        onClick={() => setSelected(k)}
        title={r.title}
      >
        <div className="mp-poster">
          {r.poster ? (
            <img src={imgUrl(r.poster)} alt="" loading="lazy" draggable={false} />
          ) : (
            <span className="mp-noimg">无封面</span>
          )}
          {on && (
            <span className="mp-pick">
              <IcoCheckL size={13} strokeWidth={2.6} />
            </span>
          )}
        </div>
        <div className="mp-body">
          <div className="mp-title">{r.title}</div>
          {r.originalTitle && r.originalTitle !== r.title && (
            <div className="mp-orig">{r.originalTitle}</div>
          )}
          <div className="mp-meta">
            {year && <span>{year}</span>}
            {typeof r.rating === "number" && r.rating > 0 && (
              <span className="mp-rating">
                <IcoStarL size={11} strokeWidth={2} className="fill" /> {r.rating.toFixed(1)}
              </span>
            )}
            {r.subtitle && <span>{r.subtitle}</span>}
            {typeof r.episodes === "number" && r.episodes > 0 && <span>{r.episodes} 集</span>}
          </div>
          {r.overview && <p className="mp-overview">{r.overview}</p>}
        </div>
      </button>
    );
  };

  return createPortal(
    <div className="meta-mask" onClick={() => !saving && onClose()}>
      <div className="meta-modal mp-modal" onClick={(e) => e.stopPropagation()}>
        <div className="meta-h">
          <h3>手动获取元数据</h3>
          <span className="meta-sub" title={target.title}>
            {target.title}
          </span>
          <button className="meta-x" onClick={onClose} title="关闭（Esc）">
            <IcoXL size={15} strokeWidth={1.8} />
          </button>
        </div>

        <div className="meta-b">
          <div className="meta-row mp-search">
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !loading && search(query)}
              placeholder="输入剧集 / 电影名称"
              autoFocus
            />
            <button className="meta-btn" disabled={loading || !query.trim()} onClick={() => search(query)}>
              <IcoSearchL size={13} strokeWidth={2} /> {loading ? "搜索中…" : "搜索"}
            </button>
          </div>
          <p className="meta-dim">
            搜索走自建镜像（TMDB / Bangumi）。选定后先写本地覆盖（该剧/电影的所有展示立即生效），
            再尽力把标题/简介回填飞牛（带锁定防下次刮削覆盖；回填失败不影响本地结果）。
          </p>

          {err && <p className="meta-err">{err}</p>}
          {notice && <p className="meta-dim mp-notice">{notice}</p>}

          {loading ? (
            <p className="meta-dim">正在搜索…</p>
          ) : (
            <>
              <section className="meta-sec">
                <h4>TMDB{countSuffix(tmdb.length)}</h4>
                {tmdb.length ? <div className="mp-grid">{tmdb.map(renderCard)}</div> : <p className="meta-dim">无结果</p>}
              </section>
              <section className="meta-sec">
                <h4>Bangumi{countSuffix(bgm.length)}</h4>
                {bgm.length ? <div className="mp-grid">{bgm.map(renderCard)}</div> : <p className="meta-dim">无结果</p>}
              </section>
            </>
          )}
        </div>

        <div className="mp-foot">
          <div className="mp-foot-left">
            {hasOverride && (
              <button className="meta-btn danger" disabled={clearing || saving} onClick={onClear}>
                <IcoTrashL size={13} strokeWidth={2} /> {clearing ? "清除中…" : "恢复飞牛原始元数据"}
              </button>
            )}
          </div>
          <div className="mp-foot-right">
            <button className="meta-btn" onClick={onClose} disabled={saving}>
              取消
            </button>
            <button className="meta-btn primary" disabled={!chosen || saving} onClick={onConfirm}>
              {saving ? "保存中…" : "使用此元数据"}
            </button>
          </div>
        </div>
      </div>
    </div>,
    document.body,
  );
}

/** 小工具：给分区标题补条数（0 时不显示）。 */
function countSuffix(n: number): string {
  return n > 0 ? ` · ${n}` : "";
}
