/* [lc-2396] Polaris 详情页 · 版本条（资源横排）
 *
 * 逐条对照 `Polaris main.css`：
 *   · `.pw-detail-source-scroller`  flex、gap 16、padding 20px 20px 40px、margin -20px -20px 0、
 *                                   scroll-snap-type: x mandatory —— 6548
 *   · `.pw-detail-source-card`      最终形态：白玻璃 scrim .5 + inset 0 .7px 高光、
 *                                   radius 14、min-width 160 / max-width 240、padding 14px 16px —— 6826 + 6642
 *   · 卡内文字统一深色 rgba(0,0,0,.85)（底是 50% 白玻璃）—— 6845
 *   · hover/active 只提白色不透明度（.5 → .58 → .66），不加位移 —— 6826
 *
 * 与原有实现的关系：数据全部来自 DetailView 已取好的 mediaVersions（同一批 video_streams），
 * 只是从「一排小胶囊」换成 Polaris 的资源卡。选择逻辑（写本地记忆 + 更新选中）完全复用。
 */
import { MediaVersion } from "../../../types";
import { versionLabels, versionMeta } from "./shared";

export function PolarisVersions({
  versions,
  selected,
  onSelect,
  filePath,
}: {
  versions: MediaVersion[];
  selected: string;
  onSelect: (mediaGuid: string) => void;
  /** 当前选中版本的完整路径（空则不渲染该行） */
  filePath?: string;
}) {
  if (versions.length === 0) return null;
  const labels = versionLabels(versions);

  return (
    <>
      <div className="pdt-section-head">
        <span className="pdt-section-label">版本</span>
        <span className="pdt-section-hint">{versions.length} 个文件</span>
      </div>
      <div className="pdt-source-scroller" role="list" aria-label="选择版本">
        {versions.map((v, i) => {
          const active = v.media_guid === selected;
          const meta = versionMeta(v);
          const color = String(v.color_range_type ?? "").trim();
          const hdr = color && color.toLowerCase() !== "sdr";
          return (
            <button
              key={v.media_guid}
              type="button"
              role="listitem"
              className={`pdt-source-card${active ? " active" : ""}`}
              aria-pressed={active}
              title={labels[i]}
              onClick={() => onSelect(v.media_guid)}
            >
              <span className="pdt-source-tag">{labels[i]}</span>
              {hdr && (
                <span className="pdt-source-hdr">{color === "DolbyVision" ? "杜比视界" : color}</span>
              )}
              {meta && <span className="pdt-source-info">{meta}</span>}
              {v.is_not_exist && <span className="pdt-source-warn">占位流</span>}
            </button>
          );
        })}
      </div>
      {filePath && (
        <div className="pdt-filepath" title={filePath}>
          <span className="pdt-filepath-k">文件位置</span>
          <span className="pdt-filepath-v">{filePath}</span>
        </div>
      )}
    </>
  );
}
