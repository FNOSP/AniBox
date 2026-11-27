/* [lc-2445] 发帖表单：作品搜索（tmdb_search，剧集/电影切换）+ 评分星 + 正文 + 附图。
 * 附图复用反馈的压缩管线组件（FeedbackImages，scope=post 公开桶）。
 * 提交流程：先逐张上传拿 imageIds，再 createPost（幂等键 Rust 侧生成）。 */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Search as IcoSearch, Send as IcoSend, Star as IcoStar } from "lucide-react";
import {
  createPost,
  uploadImage,
  type CommunityPost,
  type PreparedImage,
} from "../../lib/community";
import { FeedbackImages } from "../feedback/FeedbackImages";

type SearchItem = {
  id: number;
  title?: string;
  name?: string;
  original_title?: string;
  original_name?: string;
  release_date?: string;
  first_air_date?: string;
};

type SearchResp = { items?: SearchItem[] };

export function PostComposer({
  initial,
  onPosted,
  onCancel,
}: {
  /** 从详情页带过来的预填（tmdbId 已定位，跳过搜索） */
  initial?: {
    mediaType: "movie" | "tv";
    tmdbId: number;
    workTitle: string;
    score?: number | null;
    body?: string;
  };
  onPosted: (p: CommunityPost) => void;
  onCancel?: () => void;
}) {
  const [mediaType, setMediaType] = useState<"movie" | "tv">(initial?.mediaType ?? "tv");
  const [picked, setPicked] = useState<{ tmdbId: number; workTitle: string } | null>(
    initial ? { tmdbId: initial.tmdbId, workTitle: initial.workTitle } : null,
  );
  const [term, setTerm] = useState("");
  const [results, setResults] = useState<SearchItem[]>([]);
  const [searching, setSearching] = useState(false);
  const [score, setScore] = useState<number | null>(initial?.score ?? null);
  const [body, setBody] = useState(initial?.body ?? "");
  const [images, setImages] = useState<PreparedImage[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const seqRef = useRef(0);

  // 作品搜索（防抖 400ms；并行搜当前类型的第 1 页）
  useEffect(() => {
    if (picked || term.trim().length < 1) {
      setResults([]);
      return;
    }
    const t = window.setTimeout(() => {
      const seq = ++seqRef.current;
      setSearching(true);
      invoke<SearchResp>("tmdb_search", { arg: { query: term.trim(), mediaType, page: 1 } })
        .then((d) => {
          if (seq !== seqRef.current) return;
          setResults(Array.isArray(d?.items) ? d.items.slice(0, 8) : []);
        })
        .catch(() => {
          if (seq !== seqRef.current) return;
          setResults([]);
        })
        .finally(() => {
          if (seq === seqRef.current) setSearching(false);
        });
    }, 400);
    return () => window.clearTimeout(t);
  }, [term, mediaType, picked]);

  const itemTitle = (it: SearchItem) => it.title || it.name || it.original_title || it.original_name || "（无标题）";
  const itemYear = (it: SearchItem) =>
    (it.release_date || it.first_air_date || "").slice(0, 4);

  const submit = async () => {
    const text = body.trim();
    if (busy) return;
    if (!picked) {
      setErr("先搜索并选择作品");
      return;
    }
    if (text.length < 3) {
      setErr("正文至少 3 个字");
      return;
    }
    setBusy(true);
    setErr("");
    try {
      // 先传图（任一失败即中止，正文不发）
      const imageIds: string[] = [];
      for (const img of images) {
        const up = await uploadImage(img.preparedId, "post");
        imageIds.push(up.id);
      }
      const r = await createPost({
        mediaType,
        tmdbId: picked.tmdbId,
        workTitle: picked.workTitle,
        score: score ?? null,
        body: text,
        imageIds,
      });
      setBody("");
      setImages([]);
      setScore(null);
      onPosted(r.post);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="cv2-composer">
      {!picked ? (
        <>
          <div className="cv2-composer-mt">
            {(["tv", "movie"] as const).map((mt) => (
              <button
                key={mt}
                type="button"
                className={mediaType === mt ? "on" : ""}
                onClick={() => {
                  setMediaType(mt);
                  setResults([]);
                }}
              >
                {mt === "tv" ? "剧集" : "电影"}
              </button>
            ))}
          </div>
          <div className="cv2-composer-search">
            <IcoSearch size={14} strokeWidth={2} aria-hidden="true" />
            <input
              className="cv2-input"
              value={term}
              onChange={(e) => setTerm(e.target.value)}
              placeholder="搜索作品（TMDB）…"
            />
          </div>
          {(results.length > 0 || searching) && (
            <div className="cv2-composer-results">
              {searching && <div className="cv2-sr-loading">搜索中…</div>}
              {results.map((it) => (
                <button
                  key={it.id}
                  type="button"
                  className="cv2-sr-item"
                  onClick={() => {
                    setPicked({ tmdbId: it.id, workTitle: itemTitle(it) });
                    setResults([]);
                  }}
                >
                  <span>{itemTitle(it)}</span>
                  {itemYear(it) && <span className="cv2-sr-year">{itemYear(it)}</span>}
                </button>
              ))}
            </div>
          )}
        </>
      ) : (
        <div className="cv2-composer-picked">
          <span className="cv2-work-chip">{mediaType === "movie" ? "电影" : "剧集"}</span>
          <b>{picked.workTitle}</b>
          {!initial && (
            <button type="button" className="cmlink" onClick={() => setPicked(null)}>
              重选
            </button>
          )}
        </div>
      )}

      <div className="cv2-composer-stars">
        {[1, 2, 3, 4, 5].map((i) => (
          <button
            key={i}
            type="button"
            className={`cv2-star${score && i <= score ? " on" : ""}`}
            onClick={() => setScore(score === i ? null : i)}
            title={`${i} 星`}
          >
            <IcoStar size={16} strokeWidth={2} aria-hidden="true" />
          </button>
        ))}
        <span className="cv2-composer-starh">{score ? `${score} 星` : "不打分也可以"}</span>
      </div>

      <textarea
        className="cv2-input cv2-ta"
        rows={4}
        value={body}
        maxLength={5000}
        onChange={(e) => setBody(e.target.value)}
        placeholder="说点什么…（3–5000 字，Ctrl+Enter 发送）"
        onKeyDown={(e) => {
          if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
            e.preventDefault();
            void submit();
          }
        }}
      />

      <FeedbackImages images={images} onChange={setImages} />

      <div className="cv2-composer-ft">
        <span className="cv2-counter">{body.trim().length} / 5000</span>
        {onCancel && (
          <button type="button" className="cv2-btn-ghost" onClick={onCancel}>
            取消
          </button>
        )}
        <button type="button" className="pl-solid pl-h-sm" onClick={() => void submit()} disabled={busy}>
          <IcoSend size={13} strokeWidth={2} />
          {busy ? "发布中…" : "发布"}
        </button>
      </div>

      {err && <div className="set-warn cv2-warn">{err}</div>}
    </div>
  );
}
