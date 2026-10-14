/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */


export function fmtSize(b: number): string {
  if (!isFinite(b) || b <= 0) return "0 MB";
  if (b >= 1024 ** 3) return `${(b / 1024 ** 3).toFixed(2)} GB`;
  if (b >= 1024 ** 2) return `${(b / 1024 ** 2).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(b / 1024))} KB`;
}


export function pick(o: Record<string, unknown>, keys: string[]): any {
  for (const k of keys) {
    const v = o?.[k];
    if (v !== undefined && v !== null && v !== "") return v;
  }
  return undefined;
}

/** 时长口语化（原版剧集行样式）：1420 → 「23 分钟 40 秒」 */

export function fmtDur(sec: number | null | undefined): string {
  if (!sec || !isFinite(sec) || sec <= 0) return "";
  const m = Math.floor(sec / 60);
  const s = Math.round(sec % 60);
  if (m <= 0) return `${s} 秒`;
  return s > 0 ? `${m} 分钟 ${s} 秒` : `${m} 分钟`;
}

/** 图片统一走鉴权代理（WebView 直连 NAS 不带 Cookie 会全裂图）
 *  Windows 自定义协议形态：http://fntvimg.localhost/<poster 路径>；mac: fntvimg://localhost/<path>
 *  [lc-128] 该协议同时承载 TMDB/Bangumi 图片（`/__tmdb/*`、`/__bgm/*`，见 imgproto.rs），
 *  三级缓存（内存 → 磁盘 → 网络）都在 Rust 侧，前端只拼路径。 */

export function fmtBgmTime(ts?: number | string | null): string {
  const n = Number(ts ?? 0);
  if (!Number.isFinite(n) || n <= 0) return "";
  const d = new Date(n * 1000);
  if (Number.isNaN(d.getTime())) return "";
  const diff = Date.now() - d.getTime();
  const min = Math.floor(diff / 60000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ${min % 60}m ago`;
  const day = Math.floor(hr / 24);
  if (day <= 7) return `${day}d ago`;
  const y = d.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  const thisYear = new Date().getFullYear();
  return y === thisYear ? `${mm}-${dd}` : `${y}-${mm}-${dd}`;
}

/** Bangumi /v0/* 端点返回**裸数组**（吐槽走 /p1/* 包 {data}）——统一在这里兜底取值 */
