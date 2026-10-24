/* [lc-1783] 应用内「下载字幕」弹窗（补齐：此前字幕搜索只能进 mpv 窗口的 uosc 菜单）。
   入口：详情页标题行「字」按钮。流程：默认填剧名 → Rust subtitle_search 扇出
   （迅雷/assrt/OpenSubtitles，同一份 subtitle-sources.json 配置）→ 列表选择 →
   subtitle_download 下载到 %TEMP%/anibox-subtitles（与 NAS 自动挂载/shim 菜单互享缓存）
   → mpv 在播时自动 sub-add select 挂载。样式复用 .meta-* 弹层骨架。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Captions as IcoCaptionsL, Download as IcoDlL, Search as IcoSearchL, X as IcoXL } from "lucide-react";

type SubItem = {
  kind: "xunlei" | "assrt" | "osub";
  id: string;
  title: string;
  hint: string;
};

type DownloadResp = { path?: string; title?: string; mounted?: boolean };

const KIND_LABEL: Record<string, string> = {
  xunlei: "迅雷",
  assrt: "assrt",
  osub: "OpenSubs",
};

export function SubtitlePickerModal({
  defaultQuery,
  onClose,
}: {
  defaultQuery: string;
  onClose: () => void;
}) {
  const [query, setQuery] = useState(defaultQuery);
  const [items, setItems] = useState<SubItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [downloading, setDownloading] = useState<string>("");
  const [err, setErr] = useState("");
  const [notice, setNotice] = useState("");
  const alive = useRef(true);
  const autoSearched = useRef("");

  const search = useCallback(async (q: string) => {
    const term = q.trim();
    if (!term) return;
    setLoading(true);
    setErr("");
    setNotice("");
    try {
      const r = await invoke<{ items: SubItem[] }>("subtitle_search", { query: term });
      if (!alive.current) return;
      setItems(Array.isArray(r?.items) ? r.items : []);
      if (!r?.items?.length) setErr("没有搜到字幕，换个关键词再试");
    } catch (e) {
      if (alive.current) setErr(String(e));
    } finally {
      if (alive.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    if (autoSearched.current !== defaultQuery) {
      autoSearched.current = defaultQuery;
      void search(defaultQuery);
    }
    return () => {
      alive.current = false;
    };
  }, [defaultQuery, search]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const download = async (it: SubItem) => {
    if (downloading) return;
    setDownloading(it.id);
    setErr("");
    setNotice("");
    try {
      const r = await invoke<DownloadResp>("subtitle_download", {
        kind: it.kind,
        id: it.id,
        filename: it.title,
      });
      if (!alive.current) return;
      if (r?.mounted) {
        setNotice(`已下载并挂载到播放器：${r.title ?? it.title}`);
      } else {
        setNotice(
          `已下载到本地缓存（${r.title ?? it.title}）。当前没有正在播放的媒体，播放时会自动加载同名字幕；也可在 mpv「加载字幕」菜单手动选择。`,
        );
      }
    } catch (e) {
      if (alive.current) setErr(String(e));
    } finally {
      if (alive.current) setDownloading("");
    }
  };

  return createPortal(
    <div className="meta-mask" onClick={onClose}>
      <div className="meta-modal sp-modal" onClick={(e) => e.stopPropagation()}>
        <div className="meta-h">
          <h3>
            <IcoCaptionsL size={15} strokeWidth={2} /> 下载字幕
          </h3>
          <span className="meta-sub" title={defaultQuery}>
            {defaultQuery}
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
              placeholder="输入剧名 / 电影名"
              autoFocus
            />
            <button className="meta-btn" disabled={loading || !query.trim()} onClick={() => search(query)}>
              <IcoSearchL size={13} strokeWidth={2} /> {loading ? "搜索中…" : "搜索"}
            </button>
          </div>
          <p className="meta-dim">
            来源：迅雷（手雷官方接口）+ assrt.net + OpenSubtitles（配置见 设置→播放器→字幕源）。
            下载到本机缓存；正在播放时自动挂载为字幕轨。
          </p>

          {err && <p className="meta-err">{err}</p>}
          {notice && <p className="meta-dim">{notice}</p>}

          {loading ? (
            <p className="meta-dim">正在搜索…</p>
          ) : (
            <div className="sp-list">
              {items.map((it) => (
                <div className="sp-item" key={`${it.kind}:${it.id}`}>
                  <span className={`sp-badge sp-${it.kind}`}>{KIND_LABEL[it.kind] ?? it.kind}</span>
                  <span className="sp-name" title={it.title}>
                    {it.title}
                  </span>
                  <span className="sp-hint">{it.hint}</span>
                  <button
                    className="meta-btn"
                    disabled={!!downloading}
                    title="下载（播放中会自动挂载）"
                    onClick={() => void download(it)}
                  >
                    <IcoDlL size={12} strokeWidth={2} />
                    {downloading === it.id ? "下载中…" : "下载"}
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
