/* [lc-1725] 顶栏全局聚合搜索（对齐 FlyNarwhal 的信息架构）：
 *   · 顶栏胶囊入口 + Ctrl/Cmd+K 聚焦、Esc 关闭（main_layout.dart:52,91）
 *   · 输入 300ms 防抖 + 过期响应丢弃（search_view_model.dart:45,66）
 *   · 分类 Tab + 结果下拉/覆盖层（capsule_search_box.dart / search_result_dropdown.dart）
 *
 * 与参考实现的差异（按 AniBox 现状降级）：
 *   · 飞牛 search/list 无分类参数，分类过滤在结果上完成（同 FlyNarwhal _filterItems）；
 *   · 人物结果暂无可跳转的飞牛人物页 → 行内标注「暂不支持」，不阻断其它结果；
 *   · 结果覆盖层用 fixed 面板而非顶栏内联下拉，避免挤占品牌 logo 与拖拽区。
 */
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { Film as IcoFilm, Search as IcoSearch, X as IcoX } from "lucide-react";
import { searchItems } from "../../lib/api";
import { imgUrl } from "../../lib/images";
import type { MediaItem, SearchResultItem } from "../../types";

type Bucket = "movie" | "tv" | "live" | "person" | "other";
type TabKey = "all" | Bucket;

const TAB_DEFS: { key: TabKey; label: string }[] = [
  { key: "all", label: "全部" },
  { key: "movie", label: "电影" },
  { key: "tv", label: "电视剧" },
  { key: "live", label: "电视直播" },
  { key: "person", label: "人物" },
  { key: "other", label: "其他" },
];

const TYPE_LABEL: Record<Bucket, string> = {
  movie: "电影",
  tv: "电视剧",
  live: "电视直播",
  person: "人物",
  other: "其他",
};

/** 飞牛类型 → 分类桶（大小写宽容；未知类型归「其他」） */
function bucketOf(type?: string): Bucket {
  const t = (type ?? "").trim();
  if (t === "Movie" || t === "movie") return "movie";
  if (t === "TV" || t === "TvSeries" || t === "tv") return "tv";
  if (t === "LiveChannel" || t === "live") return "live";
  if (t === "Person" || t === "person") return "person";
  return "other";
}

/** 现有详情页可承接的类型（人物/未知类型不跳转） */
const OPENABLE = new Set(["Movie", "TV", "TvSeries", "Video", "LiveChannel", "Episode", "Season"]);
function canOpen(item: SearchResultItem): boolean {
  return OPENABLE.has((item.type ?? "").trim());
}

function metaParts(it: SearchResultItem): string[] {
  const parts: string[] = [];
  if (it.year) parts.push(it.year);
  parts.push(TYPE_LABEL[bucketOf(it.type)]);
  if (it.rating) parts.push(`★ ${it.rating}`);
  if (it.episodes && it.episodes > 0) parts.push(`共 ${it.episodes} 集`);
  else if (it.seasons && it.seasons > 0) parts.push(`${it.seasons} 季`);
  if (it.works && it.works > 0) parts.push(`${it.works} 部作品`);
  return parts;
}

export function SearchLauncher({
  onOpenItem,
  disabled,
}: {
  /** 选中结果 → 复用现有详情链路（App.openSearchItem） */
  onOpenItem: (item: MediaItem) => void;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResultItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [searched, setSearched] = useState(false);
  const [error, setError] = useState("");
  const [tab, setTab] = useState<TabKey>("all");
  const [selected, setSelected] = useState(0);
  const [retryTick, setRetryTick] = useState(0);

  const inputRef = useRef<HTMLInputElement | null>(null);
  const rowRefs = useRef<(HTMLButtonElement | null)[]>([]);
  /** 请求令牌：每次输入/重试自增，旧响应回来时令牌不符直接丢弃 */
  const reqId = useRef(0);

  const shortcut = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent) ? "⌘ K" : "Ctrl K";

  const close = useCallback(() => setOpen(false), []);

  /* 全局快捷键：Ctrl/Cmd+K 开合；Esc 关闭（焦点在面板任意位置都生效） */
  useEffect(() => {
    if (disabled) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && (e.key === "k" || e.key === "K")) {
        e.preventDefault();
        setOpen((v) => !v);
        return;
      }
      if (e.key === "Escape") {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [disabled]);

  /* 打开后聚焦输入框 */
  useEffect(() => {
    if (!open) return;
    const id = window.setTimeout(() => inputRef.current?.focus(), 0);
    return () => window.clearTimeout(id);
  }, [open]);

  /* 关闭时清空会话状态 */
  useEffect(() => {
    if (open) return;
    setQuery("");
    setResults([]);
    setLoading(false);
    setSearched(false);
    setError("");
    setTab("all");
    setSelected(0);
  }, [open]);

  /* 300ms 防抖查询；每次输入立刻自增令牌，旧响应作废 */
  useEffect(() => {
    if (!open) return;
    const q = query.trim();
    if (!q) {
      reqId.current += 1;
      setResults([]);
      setLoading(false);
      setSearched(false);
      setError("");
      setSelected(0);
      return;
    }
    setLoading(true);
    setError("");
    const token = ++reqId.current;
    const timer = window.setTimeout(() => {
      searchItems(q)
        .then((r) => {
          if (token !== reqId.current) return; // 过期响应：丢弃
          setResults(Array.isArray(r?.list) ? r.list : []);
          setSearched(true);
          setLoading(false);
          setSelected(0);
        })
        .catch((e) => {
          if (token !== reqId.current) return;
          setResults([]);
          setError(String(e));
          setSearched(true);
          setLoading(false);
        });
    }, 300);
    return () => window.clearTimeout(timer);
  }, [query, open, retryTick]);

  /* 分类 Tab：无结果时展示完整分类（引导可搜类型）；有结果后按实际命中类型裁剪 */
  const tabs = useMemo(() => {
    if (results.length === 0) return TAB_DEFS;
    const present = new Set(results.map((r) => bucketOf(r.type)));
    return TAB_DEFS.filter((t) => t.key === "all" || present.has(t.key as Bucket));
  }, [results]);

  /* 当前分类消失（换关键词后无该类型）→ 回「全部」 */
  useEffect(() => {
    if (!tabs.some((t) => t.key === tab)) setTab("all");
  }, [tabs, tab]);

  const filtered = useMemo(
    () => (tab === "all" ? results : results.filter((r) => bucketOf(r.type) === tab)),
    [results, tab],
  );

  /* 结果变化后夹住选中下标 */
  useEffect(() => {
    setSelected((i) => (filtered.length === 0 ? 0 : Math.min(i, filtered.length - 1)));
  }, [filtered]);

  /* 键盘移动选中项时滚动到可视区 */
  useEffect(() => {
    rowRefs.current[selected]?.scrollIntoView({ block: "nearest" });
  }, [selected, filtered]);

  const handleOpen = useCallback(
    (item: SearchResultItem) => {
      if (!canOpen(item)) return;
      onOpenItem(item.item);
      setOpen(false);
    },
    [onOpenItem],
  );

  const onInputKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelected((i) => (filtered.length ? (i + 1) % filtered.length : 0));
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelected((i) => (filtered.length ? (i - 1 + filtered.length) % filtered.length : 0));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const it = filtered[selected];
      if (it) handleOpen(it);
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      const idx = tabs.findIndex((t) => t.key === tab);
      const next = tabs[(idx + (e.shiftKey ? -1 : 1) + tabs.length) % tabs.length];
      if (next) {
        setTab(next.key);
        setSelected(0);
      }
    }
  };

  if (disabled) return null;

  return (
    <>
      <button type="button" className="tb-search" title={`全局搜索（${shortcut}）`} onClick={() => setOpen(true)}>
        <IcoSearch size={14} strokeWidth={2} />
        <span className="tb-search-label">搜索</span>
        <kbd className="tb-search-kbd">{shortcut}</kbd>
      </button>

      {open &&
        createPortal(
          <div className="search-overlay" onMouseDown={close}>
          <div
            className="search-panel"
            role="dialog"
            aria-modal="true"
            aria-label="全局搜索"
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div className="search-bar">
              <IcoSearch size={16} strokeWidth={2} className="search-bar-ico" />
              <input
                ref={inputRef}
                className="search-input"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={onInputKeyDown}
                placeholder="搜索片名、演员…"
                spellCheck={false}
                autoComplete="off"
                aria-label="搜索关键词"
              />
              {query && (
                <button
                  type="button"
                  className="search-clear"
                  title="清空"
                  onClick={() => {
                    setQuery("");
                    inputRef.current?.focus();
                  }}
                >
                  <IcoX size={14} />
                </button>
              )}
              <kbd className="search-esc">Esc</kbd>
            </div>

            <div className="search-tabs" role="tablist" aria-label="搜索结果分类">
              {tabs.map((t) => (
                <button
                  key={t.key}
                  type="button"
                  role="tab"
                  aria-selected={tab === t.key}
                  className={`search-tab${tab === t.key ? " on" : ""}`}
                  onClick={() => {
                    setTab(t.key);
                    setSelected(0);
                    inputRef.current?.focus();
                  }}
                >
                  {t.label}
                </button>
              ))}
            </div>

            <div className="search-body">
              {loading && results.length === 0 ? (
                <div className="search-state">
                  <span className="search-spinner" aria-hidden="true" />
                  正在搜索…
                </div>
              ) : error ? (
                <div className="search-state err">
                  <span className="search-err-text">{error}</span>
                  <button type="button" className="search-retry" onClick={() => setRetryTick((n) => n + 1)}>
                    重试
                  </button>
                </div>
              ) : !query.trim() ? (
                <div className="search-state">输入片名或演员开始搜索</div>
              ) : filtered.length === 0 ? (
                <div className="search-state">{searched ? "没有找到相关内容" : "正在搜索…"}</div>
              ) : (
                <div className="search-list">
                  {filtered.map((it, i) => {
                    const openable = canOpen(it);
                    return (
                      <button
                        key={`${it.guid}-${i}`}
                        type="button"
                        ref={(el) => {
                          rowRefs.current[i] = el;
                        }}
                        className={`search-row${i === selected ? " on" : ""}${openable ? "" : " disabled"}`}
                        onMouseEnter={() => setSelected(i)}
                        onClick={() => handleOpen(it)}
                        aria-disabled={!openable}
                        title={openable ? it.title : "人物详情暂未支持"}
                      >
                        <span className="search-thumb">
                          {it.poster ? <img src={imgUrl(it.poster)} alt="" loading="lazy" /> : <IcoFilm size={18} />}
                        </span>
                        <span className="search-info">
                          <span className="search-title">{it.title || "未命名"}</span>
                          <span className="search-meta">{metaParts(it).join(" · ")}</span>
                        </span>
                        {!openable && <span className="search-badge">暂不支持</span>}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
