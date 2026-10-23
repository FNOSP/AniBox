/* [lc-1629] 飞牛「每集/条目自定义元数据」操作弹层。
   背景：飞牛有时把集数/条目识别错，用户在飞牛网页端可「编辑元数据 / 手动匹配影片 /
   解除匹配影片 / 刷新元数据」，改完飞牛侧会重新匹配成正确元数据。本弹层把这几件事搬进 App。
   接口形状见 src-tauri/src/lib.rs 顶部注释（路由来自飞牛 Web bundle 的路由表 + 只读探针实测）。
   [lc-1632] 「手动匹配影片」已按飞牛网页端 F12 **实测请求**逐字段对齐（不再是推断）：
     POST /scrap/rescrap body = {item_guid, source_id, source, type, media_guids,
                                 season_number, episode_number, scrap_tv_type}
     关键：`source` 实测为 "trim_id"、`source_id` 即 trim id（如 tt220542）——
     两者都取自**选中那条搜索结果自带的字段原样透传**，不做映射、不套自造枚举。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Check as IcoCheckL, Link2Off as IcoUnlinkL, RefreshCw as IcoRefreshL, Search as IcoSearchL, X as IcoXL } from "lucide-react";
import { showAppToast, writeMetadataOverride } from "../../lib/metaApply";

/** 操作目标：一集、一个季、或整部剧/电影条目 */
export type MetaTarget = {
  guid: string;
  /** 展示用标题 */
  title: string;
  /** rescrap 用的资源类型（飞牛枚举 `Movie` / `TV`） */
  type: "Movie" | "TV";
  /** [lc-1728] 本地元数据覆盖的键（剧/电影 guid）。集/季目标必传父剧 guid；
      缺省回落到 guid（剧/电影根条目时两者本就相同）。 */
  seriesGuid?: string;
  /** 集目标才有：用于 rescrap 的 season/episode 号与 scrap_tv_type */
  seasonNumber?: number;
  episodeNumber?: number;
  isEpisode?: boolean;
};

/** 飞牛**搜索**源枚举（bundle `el`：title/tmdb_id/imdb_id）—— 仅用于 `scrap/search` 的入参，
    与 rescrap 请求体里的 `source` 无关（后者实测为 "trim_id"，取自搜索结果原样透传）。 */
type ScrapSource = "title" | "tmdb_id" | "imdb_id";

type ScrapHit = {
  title?: string;
  source?: string;
  source_id?: string;
  type?: string;
  first_air_date?: string;
  last_air_date?: string;
  number_of_seasons?: number;
};

const str = (v: unknown): string => (typeof v === "string" ? v : "");

export function MetaEditModal({
  target,
  onClose,
  onDone,
}: {
  target: MetaTarget;
  onClose: () => void;
  /** 任一写操作成功后调用：让详情页带 force 重取，立刻反映飞牛侧的新数据 */
  onDone: () => void;
}) {
  // ── 编辑元数据 ──
  const [edit, setEdit] = useState<Record<string, unknown> | null>(null);
  const [title, setTitle] = useState("");
  const [overview, setOverview] = useState("");
  const [loadErr, setLoadErr] = useState("");
  const [saving, setSaving] = useState(false);

  // ── 手动匹配 ──
  const [source, setSource] = useState<ScrapSource>("title");
  const [keyword, setKeyword] = useState("");
  const [hits, setHits] = useState<ScrapHit[] | null>(null);
  const [searching, setSearching] = useState(false);
  // [lc-1632] 集级匹配的季节/集号：对齐飞牛网页端弹层里的两个输入框（rescrap 实测会回传这两个值）。
  // 默认取当前集自己的季/集号，用户可在此改成正确值 —— 手动匹配本就是为了纠错。
  const [season, setSeason] = useState(0);
  const [episode, setEpisode] = useState(0);

  // ── 其它操作 ──
  const [busy, setBusy] = useState("");
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");

  // 打开即拉一次 getEditDetail 作为编辑初始值（全量对象回传，避免飞牛把未传字段清空）
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    setEdit(null);
    setLoadErr("");
    invoke<Record<string, unknown>>("get_edit_detail", { guid: target.guid })
      .then((d) => {
        if (!alive.current) return;
        setEdit(d ?? {});
        setTitle(str(d?.title));
        setOverview(str(d?.overview));
      })
      .catch((e) => alive.current && setLoadErr(String(e)));
    return () => {
      alive.current = false;
    };
  }, [target.guid]);

  useEffect(() => {
    setKeyword(target.title);
  }, [target.title]);

  // 目标集切换时重置季/集号输入框（默认取当前集的真实季/集号）
  useEffect(() => {
    setSeason(target.seasonNumber ?? 0);
    setEpisode(target.episodeNumber ?? 0);
  }, [target.seasonNumber, target.episodeNumber]);

  // Esc 关闭（与详情页其它弹层一致）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const flash = (s: string) => {
    setErr("");
    setMsg(s);
    window.setTimeout(() => setMsg(""), 2600);
  };
  const fail = (e: unknown) => {
    setMsg("");
    setErr(String(e));
  };

  // 保存编辑元数据：仅改 title/overview 并置 *_locked（防飞牛下次刮削覆盖），其余原样回传。
  const onSave = useCallback(async () => {
    if (!edit) return;
    setSaving(true);
    setErr("");
    try {
      const body: Record<string, unknown> = { ...edit };
      body.title = title;
      body.title_locked = true;
      body.overview = overview;
      body.overview_locked = true;
      await invoke("save_edit_detail", { payload: body });
      onDone();
      flash("已保存，飞牛侧将按新信息更新");
    } catch (e) {
      fail(e);
    } finally {
      setSaving(false);
    }
  }, [edit, title, overview, onDone]);

  const onSearch = useCallback(async () => {
    if (!keyword.trim()) return;
    setSearching(true);
    setErr("");
    setHits(null);
    try {
      const r = await invoke<ScrapHit[] | { list?: ScrapHit[] }>("scrap_search", {
        source,
        keyword: keyword.trim(),
        lan: "zh-CN",
        itemGuid: target.guid,
      });
      const list = Array.isArray(r) ? r : Array.isArray(r?.list) ? r.list : [];
      setHits(list);
      if (!list.length) flash("没有搜到匹配结果，换个关键词或搜索方式");
    } catch (e) {
      fail(e);
    } finally {
      setSearching(false);
    }
  }, [keyword, source, target.guid]);

  // 应用匹配。[lc-1728] 改为「本地覆盖先行」——此前只走飞牛 rescrap：NAS 侧失败/无可见
  // 变化时整个动作就"没反应"（错误只有弹层底部一行小字）。现在：
  //   ① 先把选中结果写进**本地元数据覆盖**（键 = 剧 guid，字段与 metadata_override 对齐：
  //     标题/首播日期；overview/海报等命中里没有的字段留空 = 不覆盖），成功后立即关闭弹层
  //     并让详情页带 force 重取 —— 覆盖当场可见；失败则留在弹层内报红。
  //   ② 再 best-effort 让飞牛 rescrap 重刮（请求体仍按 [lc-1632] 实测对齐）；失败只
  //     console.warn + 轻提示，**不影响本地已生效的结果**。
  const onApply = useCallback(
    async (hit: ScrapHit) => {
      const seriesGuid = String(target.seriesGuid || target.guid || "").trim();
      const hitTitle = String(hit.title ?? "").trim() || target.title;
      const hitDate = String(hit.first_air_date ?? "").trim();
      setBusy("match");
      setErr("");
      try {
        // ① 本地覆盖（关键步骤，失败必须在弹层内可见）
        await writeMetadataOverride(seriesGuid, {
          source: String(hit.source ?? "").trim() || "trim_id",
          id: String(hit.source_id ?? "").trim() || seriesGuid,
          title: hitTitle,
          date: hitDate,
          year: hitDate.slice(0, 4),
        });
      } catch (e) {
        fail(e);
        setBusy("");
        return;
      }
      setBusy("");
      onDone(); // 详情页带 force 重取，覆盖立即生效
      onClose(); // 匹配已应用，关闭弹层
      // ② best-effort：飞牛按选中结果重新刮削（rescrap 请求体逐字段照 [lc-1632] 实测）。
      //    弹层此时已卸载，这里不得再碰任何组件状态。
      try {
        const args: Record<string, unknown> = {
          itemGuid: target.guid,
          sourceId: hit.source_id ?? "",
          source: hit.source ?? "",
          type: hit.type || target.type,
          mediaGuids: [],
        };
        if (target.isEpisode) {
          args.seasonNumber = season;
          args.episodeNumber = episode;
          args.scrapTvType = "SingleEpisode";
        }
        await invoke("scrap_rescrap", args);
      } catch (e) {
        console.warn("[MetaEditModal] 飞牛 rescrap 回填失败（本地覆盖已生效）:", e);
        showAppToast("本地匹配已生效；飞牛重新匹配失败（可稍后重试）");
      }
    },
    [target, season, episode, onDone, onClose],
  );

  const onRefresh = useCallback(async () => {
    setBusy("refresh");
    setErr("");
    try {
      await invoke("refresh_item", { guid: target.guid, mode: "immediate" });
      onDone();
      flash("已请求刷新元数据");
    } catch (e) {
      fail(e);
    } finally {
      setBusy("");
    }
  }, [target.guid, onDone]);

  const onUnscrap = useCallback(async () => {
    if (!window.confirm("解除匹配影片？该条目会与当前刮削结果解除关联（文件不受影响）。")) return;
    setBusy("unscrap");
    setErr("");
    try {
      await invoke("scrap_unscrap", { guid: target.guid });
      onDone();
      flash("已解除匹配");
    } catch (e) {
      fail(e);
    } finally {
      setBusy("");
    }
  }, [target.guid, onDone]);

  return createPortal(
    <div className="meta-mask" onClick={onClose}>
      <div className="meta-modal" onClick={(e) => e.stopPropagation()}>
        <div className="meta-h">
          <h3>元数据{target.isEpisode ? " · 本集" : ""}</h3>
          <span className="meta-sub" title={target.title}>
            {target.title}
          </span>
          <button className="meta-x" onClick={onClose} title="关闭（Esc）">
            <IcoXL size={15} strokeWidth={1.8} />
          </button>
        </div>

        <div className="meta-b">
          {/* ① 编辑元数据 */}
          <section className="meta-sec">
            <h4>编辑元数据</h4>
            {loadErr ? (
              <p className="meta-err">读取当前元数据失败：{loadErr}</p>
            ) : !edit ? (
              <p className="meta-dim">读取中…</p>
            ) : (
              <>
                <label className="meta-f">
                  <span>标题</span>
                  <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="留空则用飞牛自动刮削结果" />
                </label>
                <label className="meta-f">
                  <span>简介</span>
                  <textarea
                    value={overview}
                    rows={4}
                    onChange={(e) => setOverview(e.target.value)}
                    placeholder="自定义简介（会覆盖飞牛自动刮削的简介）"
                  />
                </label>
                <button className="meta-btn primary" disabled={saving} onClick={onSave}>
                  {saving ? "保存中…" : "保存到飞牛"}
                </button>
                <p className="meta-dim">保存后标题/简介会被「锁定」，飞牛下次自动刮削不会覆盖。</p>
              </>
            )}
          </section>

          {/* ② 手动匹配影片 */}
          <section className="meta-sec">
            <h4>手动匹配影片</h4>
            <div className="meta-row">
              <select value={source} onChange={(e) => setSource(e.target.value as ScrapSource)}>
                <option value="title">按标题</option>
                <option value="tmdb_id">按 TMDB ID</option>
                <option value="imdb_id">按 IMDB ID</option>
              </select>
              <input
                value={keyword}
                onChange={(e) => setKeyword(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && onSearch()}
                placeholder={source === "title" ? "片名" : source === "tmdb_id" ? "如 12345" : "如 tt1234567"}
              />
              <button className="meta-btn" disabled={searching} onClick={onSearch}>
                <IcoSearchL size={13} strokeWidth={2} /> {searching ? "搜索中…" : "搜索"}
              </button>
            </div>
            {/* [lc-1632] 集级匹配：对齐飞牛网页端的季/集号输入（rescrap 会把这两个值回传给飞牛）。
                默认是当前集自己的季/集号，识别错时在此改成正确的。 */}
            {target.isEpisode && (
              <div className="meta-row">
                <span className="meta-dim">季</span>
                <input
                  type="number"
                  min={0}
                  value={season}
                  onChange={(e) => setSeason(Number(e.target.value) || 0)}
                />
                <span className="meta-dim">集</span>
                <input
                  type="number"
                  min={0}
                  value={episode}
                  onChange={(e) => setEpisode(Number(e.target.value) || 0)}
                />
              </div>
            )}
            {hits && (
              <ul className="meta-hits">
                {hits.map((h, i) => (
                  <li key={`${h.source_id}-${i}`}>
                    <div className="meta-hit-t">
                      <span className="meta-hit-name">{h.title || "(无标题)"}</span>
                      <span className="meta-hit-meta">
                        {[h.first_air_date?.slice(0, 4), h.type, h.source_id].filter(Boolean).join(" · ")}
                      </span>
                    </div>
                    <button className="meta-btn sm" disabled={busy === "match"} onClick={() => onApply(h)}>
                      用此匹配
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <p className="meta-dim">
              点「用此匹配」先把选中结果写入本地覆盖（立即生效并刷新详情），再让飞牛按该结果重新刮削
              （rescrap 请求体按飞牛网页端实测对齐：source/source_id 取自选中结果、media_guids 为空、
              集级匹配带季/集号与 scrap_tv_type=SingleEpisode；飞牛失败不影响本地）。
            </p>
          </section>

          {/* ③ 刷新 / 解除匹配 */}
          <section className="meta-sec">
            <h4>其它</h4>
            <div className="meta-row">
              <button className="meta-btn" disabled={busy === "refresh"} onClick={onRefresh}>
                <IcoRefreshL size={13} strokeWidth={2} /> {busy === "refresh" ? "请求中…" : "刷新元数据"}
              </button>
              <button className="meta-btn danger" disabled={busy === "unscrap"} onClick={onUnscrap}>
                <IcoUnlinkL size={13} strokeWidth={2} /> 解除匹配
              </button>
            </div>
          </section>
        </div>

        {(msg || err) && (
          <div className={`meta-foot${err ? " err" : ""}`}>
            {err ? err : (
              <>
                <IcoCheckL size={13} strokeWidth={2.4} /> {msg}
              </>
            )}
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
}
