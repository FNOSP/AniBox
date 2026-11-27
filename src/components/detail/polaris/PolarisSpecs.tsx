/* [lc-2396] Polaris 详情页 · 技术规格（4 列玻璃网格 + 展开区）
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
 *   · `.pw-detail-expand-btn`  整宽按钮、min-height 52、38px 下箭头（展开时 rotate 180） —— 6953
 *   · `.pw-detail-expanded-area` max-height 0→800 + cb(.25,1,.5,1) —— 6984
 *   · `.pw-detail-expanded-grid` 1fr 1fr / gap 24；行两端对齐 —— 7044
 *
 * 数据来源：`get_media_versions` 的同一次 stream/list 响应（`specs` + 当前选中的 video_stream）。
 * 取不到的格显示「—」（不编数字）——这是与 Polaris 一致的做法（它也是 `--` 占位）。
 */
import { useState } from "react";
import { ChevronDown as IcoChevronDownL } from "lucide-react";
import type { MediaSpecs, MediaVersion } from "../../../types";
import { fmtBitrate, fmtBytes, fmtChannels, fmtFrameRate } from "./shared";

/** 一格显示值：空则「—」。 */
function v(x?: string | number | null): string {
  const s = String(x ?? "").trim();
  return s || "—";
}

export function PolarisSpecs({
  version,
  specs,
}: {
  /** 当前选中的视频流（没有则整块不渲染） */
  version: MediaVersion | null;
  specs: MediaSpecs | null;
}) {
  const [expanded, setExpanded] = useState(false);
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
  const frameText = fmtFrameRate(Number(version.frame_rate ?? 0));
  const channelsText = fmtChannels(audio?.channels);
  const audioCodec = String(audio?.codec_name ?? "").trim().toUpperCase();
  const audioBitrate = fmtBitrate(Number(audio?.bit_rate ?? 0));
  const container = String(specs?.container ?? "").trim().toUpperCase();
  const subCount = Number(specs?.subtitle_count ?? 0);

  /* 展开区：逐行 key/value（Polaris 的「视频」「音频」两块） */
  const videoRows: [string, string][] = [
    ["标题", v(version.title)],
    ["分辨率", resText],
    ["帧率", v(frameText)],
    ["编码", v(codec)],
    ["配置", v(version.profile)],
    ["基色", v(version.color_primaries)],
    ["色彩空间", v(version.color_space)],
    ["转移特性", v(version.color_transfer)],
    ["色彩范围", v(version.color_range || colorRaw)],
    ["位深", bitDepth > 0 ? `${bitDepth} bit` : "—"],
    ["平均码率", v(bitrateText)],
    ["时长", Number(version.duration ?? 0) > 0 ? `${Math.round(Number(version.duration) / 60)} 分钟` : "—"],
  ];
  const audioRows: [string, string][] =
    audioList.length > 0
      ? audioList.map((a, i) => {
          const parts = [
            String(a.codec_name ?? "").trim().toUpperCase(),
            fmtChannels(a.channels),
            fmtBitrate(Number(a.bit_rate ?? 0)),
            Number(a.sample_rate ?? 0) > 0 ? `${Number(a.sample_rate) / 1000} kHz` : "",
            String(a.language ?? "").trim(),
          ].filter(Boolean);
          const label = String(a.title ?? "").trim();
          return [
            label || (audioList.length > 1 ? `音轨 ${i + 1}` : "音轨"),
            parts.join(" / ") || "—",
          ] as [string, string];
        })
      : [["音轨", "—"]];

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
            <span className="pdt-spec-val">
              {hdrText || "SDR"}
            </span>
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

      <button
        type="button"
        className={`pdt-expand-btn${expanded ? " expanded" : ""}`}
        aria-expanded={expanded}
        aria-label={expanded ? "收起详情" : "展开详情"}
        title={expanded ? "收起详情" : "展开详情"}
        onClick={() => setExpanded((x) => !x)}
      >
        <IcoChevronDownL className="pdt-expand-icon" size={38} strokeWidth={3.4} />
      </button>

      <div className={`pdt-expanded-area${expanded ? " show" : ""}`}>
        <div className="pdt-expanded-grid">
          <div className="pdt-expanded-section">
            <div className="pdt-expanded-title">视频</div>
            <div className="pdt-expanded-list">
              {videoRows.map(([k, val]) => (
                <div className="pdt-expanded-row" key={k}>
                  <span className="pdt-expanded-label">{k}</span>
                  <span className="pdt-expanded-value" title={val}>{val}</span>
                </div>
              ))}
            </div>
          </div>
          <div className="pdt-expanded-section">
            <div className="pdt-expanded-title">音频</div>
            <div className="pdt-expanded-list">
              {audioRows.map(([k, val], i) => (
                <div className="pdt-expanded-row" key={`${k}-${i}`}>
                  <span className="pdt-expanded-label">{k}</span>
                  <span className="pdt-expanded-value" title={val}>{val}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
