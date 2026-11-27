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

/** 集号 → 展示文本（polaris 版固定「第 N 集」；短标题用于卡片角标）。 */
export function epLabel(num: number): string {
  return `第 ${num} 集`;
}

/** 秒 → 图上角标用的**极短**时长（`24m` / `1h24m`）。
 *  为什么不复用 `fmtDur`：那个返回「24 分钟」（中文全称），而图上角标是压在剧照上的
 *  浮字，宽一点就会盖住画面。Polaris 源码（strings.json idx 12a0 一带）与用户给的
 *  截图用的都是 `24m` 这种紧凑写法，这里特意分开一个函数，两个消费点互不影响。 */
export function epDurationBadge(sec?: number | null): string {
  const s = Number(sec ?? 0);
  if (!s || !isFinite(s) || s <= 0) return "";
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const rest = m % 60;
  return rest > 0 ? `${h}h${rest}m` : `${h}h`;
}
