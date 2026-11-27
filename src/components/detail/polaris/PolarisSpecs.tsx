/* [lc-2396] Polaris 详情页 · 技术规格（4 列玻璃网格）
 *
 * 逐条对照 `Polaris main.css`：
 *   · `.pw-detail-info-panel`  margin-top 48px、背景/阴影/模糊全 none（网格自己带玻璃）—— 6878
 *   · `.pw-detail-specs-grid`  grid-template-columns: repeat(4,1fr)、gap 20、align-items stretch —— 6885
 *   · `.pw-detail-spec-group`  flex column、justify-content center、gap 8、padding 24px 28px、
 *                              hover 提白 + translateY(-2px) —— 6891
 *   · 玻璃板配方（三层阴影）：inset 0 .7px 0 white.9 + inset 0 0 0 .7px black.10 + 0 14px 30px black.18 —— 7005
 *   · `.pw-detail-spec-head`   13px/700/大写/1px 字距/rgba(0,0,0,.55) —— 6912
 *   · `.pw-detail-spec-val`    20px/600/rgba(0,0,0,.88) —— 6924
 *   · `.pw-detail-spec-lbl`    13px/500/rgba(0,0,0,.55) —— 6932
 *   · `.pw-detail-tech-badge`  10px/700、黑 10% 底、radius 4 —— 6940
 *
 * 数据来源：`get_media_versions` 的同一次 stream/list 响应（`specs` + 当前选中的 video_stream）。
 * 取不到的格显示「—」（不编数字）——这是与 Polaris 一致的做法（它也是 `--` 占位）。
 *
 * [lc-2406] 用户澄清：「上面原本的四个不动啊」—— 4 列网格**必须保留**。
 *   lc-2405 我误删了整块，现在恢复（代码与数值全部回到 lc-2404 的状态）。
 *   同时按用户要求：
 *     · 删掉展开按钮与展开区（视频/音频两个大块）
 *     · 原长条位置改为「资源详情」行，显示完整路径
 *   —— 即"四个方块 + 一条路径行"，不再有展开交互。
 */
import type { MediaSpecs, MediaVersion } from "../../../types";
import { fmtBitrate, fmtBytes, fmtChannels } from "./shared";
import { PolarisFilePath } from "./PolarisFilePath";

/** 一格显示值：空则「—」。 */
function v(x?: string | number | null): string {
  const s = String(x ?? "").trim();
  return s || "—";
}

export function PolarisSpecs({
  version,
  specs,
  filePath,
}: {
  /** 当前选中的视频流（没有则整块不渲染） */
  version: MediaVersion | null;
  specs: MediaSpecs | null;
  /** 当前版本的完整路径（资源详情行；空则不渲染该行） */
  filePath?: string;
}) {
  if (!version) return null;

  /* 音频：优先 play/info 记住的 audio_guid，其次第一条（与 Polaris 同口径） */
  const audioList = specs?.audio_streams ?? [];
  const audio =
    audioList.find((a) => a.guid && a.guid === specs?.selected_audio_guid) ?? audioList[0] ?? null;

  const resText =
    Number(version.width ?? 0) > 0 && Number(version.height ?? 0) > 0
      ? `${version.width}×${version.height}`
      : v(String(version.resolution_type ?? "").toUpperCase());

  const colorRaw = String(version.color_range_type ?? "").trim();
  const hdrText = colorRaw === "DolbyVision" ? "杜比视界" : colorRaw || "";

  const codec = String(version.codec_name ?? "").trim().toUpperCase();
  const bitDepth = Number(version.bit_depth ?? 0);
  const sizeText = fmtBytes(Number(version.file_size ?? 0));
  const bitrateText = fmtBitrate(Number(version.bps ?? 0));
  const channelsText = fmtChannels(audio?.channels);
  const audioCodec = String(audio?.codec_name ?? "").trim().toUpperCase();
  const audioBitrate = fmtBitrate(Number(audio?.bit_rate ?? 0));
  const container = String(specs?.container ?? "").trim().toUpperCase();
  const subCount = Number(specs?.subtitle_count ?? 0);

  return (
    <div className="pdt-specs">
      <div className="pdt-specs-grid">
        <div className="pdt-spec-group">
          <div className="pdt-spec-head">文件信息</div>
          <div className="pdt-spec-item">
            <span className="pdt-spec-val">{v(sizeText)}</span>
            <span className="pdt-spec-lbl">文件大小</span>
          </div>
          <div className="pdt-spec-item">
            <span className="pdt-spec-val">{v(bitrateText)}</span>
            <span className="pdt-spec-lbl">平均码率</span>
          </div>
        </div>

        <div className="pdt-spec-group">
          <div className="pdt-spec-head">视频流</div>
          <div className="pdt-spec-item">
            <span className="pdt-spec-val">{resText}</span>
            <span className="pdt-spec-lbl">分辨率</span>
          </div>
          <div className="pdt-spec-item">
            <span className="pdt-spec-val">{hdrText || "SDR"}</span>
            <span className="pdt-tech-badge">
              {[codec, bitDepth > 0 ? `${bitDepth}bit` : ""].filter(Boolean).join(" ") || "—"}
            </span>
          </div>
        </div>

        <div className="pdt-spec-group">
          <div className="pdt-spec-head">音频流</div>
          <div className="pdt-spec-item">
            <span className="pdt-spec-val">{v(audioCodec)}</span>
            <span className="pdt-spec-lbl">{v(audioBitrate)}</span>
          </div>
          <div className="pdt-spec-item">
            <span className="pdt-spec-val">{v(channelsText)}</span>
            <span className="pdt-spec-lbl">声道配置</span>
          </div>
        </div>

        <div className="pdt-spec-group">
          <div className="pdt-spec-head">封装</div>
          <div className="pdt-spec-item">
            <span className="pdt-spec-val">{v(container)}</span>
            <span className="pdt-spec-lbl">容器格式</span>
          </div>
          <div className="pdt-spec-item">
            <span className="pdt-spec-val">{subCount > 0 ? `${subCount} 条` : "—"}</span>
            <span className="pdt-spec-lbl">字幕流</span>
          </div>
        </div>
      </div>

      {/* [lc-2406] 资源详情：原展开按钮的位置改成完整路径行 */}
      <PolarisFilePath filePath={filePath} fileName={version.file_name} />
    </div>
  );
}

/* 未使用的格式化函数在 lc-2405 被删过一次，这里重新需要（规格网格恢复了）——
   已确认 shared.ts 里四个函数都还在。 */
