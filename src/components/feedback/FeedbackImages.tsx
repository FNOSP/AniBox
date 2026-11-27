/* [lc-2442] 反馈附图上传（与社区共用压缩管线；scope=feedback → 私有桶，仅本人与管理员可见）。
 * 选图（plugin-dialog）→ Rust 压缩（长边 1600 / q88 → 1MB 十档阶梯 / EXIF 剥除+转正 /
 * 透明合白底 / HEIC→JPEG / GIF ≤1MB 原样）→ 卡片预览 → 提交工单时逐张上传。 */
import { useCallback, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ImagePlus as IcoImgPlus, X as IcoX } from "lucide-react";
import { discardPrepared, prepareImage, type PreparedImage } from "../../lib/community";

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)}KB`;
  return `${(n / (1024 * 1024)).toFixed(1)}MB`;
}

export const FEEDBACK_MAX_IMAGES = 6;

export function FeedbackImages({
  images,
  onChange,
}: {
  images: PreparedImage[];
  onChange: (next: PreparedImage[]) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  const pick = useCallback(async () => {
    if (busy || images.length >= FEEDBACK_MAX_IMAGES) return;
    setBusy(true);
    setErr("");
    try {
      const path = await openDialog({
        multiple: false,
        filters: [
          {
            name: "图片",
            extensions: ["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "heic"],
          },
        ],
      });
      if (!path || typeof path !== "string") return;
      const prepared = await prepareImage(path);
      onChange([...images, prepared]);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }, [busy, images, onChange]);

  const remove = async (preparedId: string) => {
    onChange(images.filter((x) => x.preparedId !== preparedId));
    try {
      await discardPrepared(preparedId);
    } catch {
      /* 暂存清理失败无碍（24h 后自动清理） */
    }
  };

  const full = images.length >= FEEDBACK_MAX_IMAGES;

  return (
    <div className="fb2imgs">
      <div className="fb2imgs-row">
        {images.map((img) => (
          <div className="fb2imgs-card" key={img.preparedId}>
            {img.preview ? (
              <img className="fb2imgs-thumb" src={img.preview} alt="" />
            ) : (
              <div className="fb2imgs-thumb ph">{img.fmt.toUpperCase()}</div>
            )}
            <div className="fb2imgs-meta">
              <span className="fb2imgs-shrink">
                {fmtSize(img.originalBytes)} → {fmtSize(img.finalBytes)}
              </span>
              <span className="fb2imgs-dim">
                {img.w}×{img.h}
              </span>
            </div>
            <button
              type="button"
              className="fb2imgs-x"
              onClick={() => void remove(img.preparedId)}
              title="移除"
            >
              <IcoX size={12} strokeWidth={2} />
            </button>
          </div>
        ))}
        {!full && (
          <button
            type="button"
            className="fb2imgs-add"
            onClick={() => void pick()}
            disabled={busy}
            title={busy ? "压缩中…" : "添加图片"}
          >
            <IcoImgPlus size={18} strokeWidth={1.7} aria-hidden="true" />
            <span>{busy ? "压缩中…" : "添加图片"}</span>
          </button>
        )}
      </div>
      {err && <p className="fb2imgs-err">{err}</p>}
    </div>
  );
}
