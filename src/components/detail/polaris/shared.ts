/* [lc-2396] Polaris 详情页（`detail/polaris/`）共享常量与纯函数。
 *
 * 为什么单独成文件：一个页面要拆成 Header / Episodes / Specs / Stills / Cast 几个区块，
 * 每块都要用「版本标签文案」「已看判定」「进度百分比」「文件大小格式化」这几条规则。
 * 写在各区块里必然出现「列表显示已看、网格没显示」这类漂移，所以集中在这里，
 * 与 `DetailView.tsx` 里原有实现的**口径完全一致**（判定阈值、优先级都照抄）。
 */
import { Episode, MediaVersion } from "../../../types";

/** 进度百分比（0–100 整数）。与 DetailView 的 resumePct / 选集进度同一公式。 */
export function progressPct(ts?: number, duration?: number): number {
  if (!ts || !duration || duration <= 0) return 0;
  return Math.min(100, Math.round((ts / duration) * 100));
}

/** 「已看」判定：本地进度 ≥80% 即视为已看（lc-1596 用户口径），飞牛 watched=1 优先。
 *  手动覆盖（epWatchedManual / localWatched）由调用方先行处理 —— 与 DetailView 同序。 */
export function isSeen(ep: Episode, pct: number): boolean {
  return Number(ep.watched) === 1 || pct >= 80;
}

/** 去掉文件扩展名（仅取文件名，不含目录）。与 DetailView `stripExt` 同一实现。 */
export function stripExt(name: string): string {
  const base = String(name ?? "").trim().split(/[\\/]/).pop() ?? "";
  return base.replace(/\.[A-Za-z0-9]{1,5}$/, "").trim();
}

/** 版本盒子文案（与 DetailView `versionLabel` 同口径）：
 *  分辨率+色域 → 高度P+编码+位深 → title → 文件名去扩展名 → 「默认版本」。 */
export function versionLabel(v: MediaVersion): string {
  const res = String(v.resolution_type ?? "").trim().toUpperCase();
  const colorRaw = String(v.color_range_type ?? "").trim();
  const color = colorRaw === "DolbyVision" ? "杜比视界" : colorRaw;
  const primary = [res, color].filter(Boolean).join(" ").trim();
  if (primary) return primary;

  const h = Number(v.height ?? 0);
  const spec = [
    h > 0 ? `${h}P` : "",
    String(v.codec_name ?? "").trim(),
    Number(v.bit_depth ?? 0) > 0 ? `${v.bit_depth}bit` : "",
  ]
    .filter(Boolean)
    .join(" ");
  if (spec) return spec;

  const title = String(v.title ?? "").trim();
  if (title) return title;

  const file = stripExt(String(v.file_name ?? ""));
  return file || "默认版本";
}

/** 批量生成版本标签：重复时追加 ` · 文件名`，仍重复则追加 ` · 序号`。与 DetailView 同一实现。 */
export function versionLabels(versions: MediaVersion[]): string[] {
  const base = versions.map((v) => versionLabel(v));
  const count = new Map<string, number>();
  for (const b of base) count.set(b, (count.get(b) ?? 0) + 1);

  const withFile = base.map((b, i) => {
    if ((count.get(b) ?? 0) <= 1) return b;
    const file = stripExt(String(versions[i]?.file_name ?? ""));
    return file ? `${b} · ${file}` : b;
  });
  const count2 = new Map<string, number>();
  for (const b of withFile) count2.set(b, (count2.get(b) ?? 0) + 1);
  return withFile.map((b, i) => ((count2.get(b) ?? 0) > 1 ? `${b} · ${i + 1}` : b));
}

/** 版本卡第二行：**大小 · 编码 · 位深**（缺项自动跳过）。与 DetailView `versionMeta` 同口径：
 *  真实字节数优先，其次 bps × duration 估算，都没有就不显示体积（绝不编数字）。 */
export function versionMeta(v: MediaVersion): string {
  const codec = String(v.codec_name ?? "").trim();
  const depth = Number(v.bit_depth ?? 0);

  const rawSize = Number(v.file_size ?? 0);
  const bps = Number(v.bps ?? 0);
  const dur = Number(v.duration ?? 0);
  const bytes = rawSize > 0 ? rawSize : bps > 0 && dur > 0 ? (bps * dur) / 8 : 0;
  const size =
    bytes >= 1024 ** 3
      ? `${(bytes / 1024 ** 3).toFixed(1)} GB`
      : bytes >= 1024 ** 2
        ? `${Math.round(bytes / 1024 ** 2)} MB`
        : "";

  return [size, codec, depth > 0 ? `${depth}bit` : ""].filter(Boolean).join(" · ");
}

/** 字节数 → 人类可读（GB/MB）。规格区的「文件大小」格用它。 */
export function fmtBytes(bytes: number): string {
  if (!bytes || bytes <= 0) return "";
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** bps → 人类可读码率（Mbps / Kbps）。0 = 上游没给，返回空串。 */
export function fmtBitrate(bps: number): string {
  if (!bps || bps <= 0) return "";
  if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(1)} Mbps`;
  return `${Math.round(bps / 1000)} Kbps`;
}

/** 帧率 → 展示串（整数不带小数、非整数保留三位）。0 = 没给，返回空串。 */
export function fmtFrameRate(fps: number): string {
  if (!fps || fps <= 0) return "";
  return Number.isInteger(fps) ? `${fps} fps` : `${fps.toFixed(3)} fps`;
}

/** 声道数 → 展示串（2 → 2.0、6 → 5.1、8 → 7.1）。与 fnOS web 前端同口径。 */
export function fmtChannels(ch?: number): string {
  const n = Number(ch ?? 0);
  if (!n || n <= 0) return "";
  if (n === 1) return "1.0";
  if (n === 2) return "2.0";
  if (n === 6) return "5.1";
  if (n === 8) return "7.1";
  if (n === 4) return "4.0";
  return `${n} ch`;
}

/** 集号 → 展示文本（polaris 版固定「第 N 集」；短标题用于卡片角标）。 */
export function epLabel(num: number): string {
  return `第 ${num} 集`;
}
