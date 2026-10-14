import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode, type UIEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { createPortal } from "react-dom";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { IcoHome, IcoStar, IcoGrid, IcoMovie, IcoTv, IcoLive, IcoFolder } from "./icons";
// [lc-123-rev8] 图标统一用 Lucide（ISC 许可，https://lucide.dev）：stroke=currentColor、24 viewBox，
// 按需导入 tree-shake；仓库内不再保留手拷贝的 SVG path。
import {
  Menu as IcoMenuL,
  X as IcoXL,
  Film as IcoFilmL,
  Clapperboard as IcoClapL,
  Check as IcoCheckL,
  Settings as IcoGearL,
  User as IcoUserL,
  Clock as IcoClockL,
  MessageSquare as IcoMsgL,
  House as IcoHouseL,
  Search as IcoSearchL,
  Star as IcoStarL,
  Play as IcoPlayL,
} from "lucide-react";
import "./App.css";

type Config = {
  server: string;
  username: string;
  token: string;
  password: string;
  auto_login: boolean;
  mpv_path: string;
  proxy_path: string;
  nas_proxy_enabled: boolean;
  /** [lc-128] 图片/演员表落盘缓存上限（MB；0 = 不限）。设置面板「缓存」一栏读写。 */
  cache_limit_mb: number;
};

type Lib = { guid: string; title: string; category: string; posters?: string[] };

/** item/list（分页查询）返回的条目，snake_case */
type MediaItem = {
  guid: string;
  title: string;
  tv_title?: string;
  type?: string;
  poster?: string;
  poster_width?: number;
  poster_height?: number;
  is_favorite?: number;
  watched?: number;
  vote_average?: string;
  release_date?: string;
  overview?: string;
  season_number?: number;
  episode_number?: number;
  ts?: number;
  duration?: number;
  air_date?: string;
  media_stream?: unknown;
  /** [lc-1444] 发现页条目自带的 TMDB id（无飞牛 guid 时用它直连 TMDB 定位） */
  tmdb_id?: number;
  /** [lc-1444] 已知的 Bangumi subject id（发现页右栏新番表条目自带）：详情页直连，免标题匹配 */
  bgm_id?: number;
};

/** play/list（继续观看）返回的条目，camelCase 为主，读取时做双格式兼容 */
type PlayItem = Record<string, unknown>;

/** episode/list 返回的剧集，PlayListItem 形状（snake_case） */
type Episode = {
  guid: string;
  title: string;
  episode_number?: number;
  season_number?: number;
  poster?: string;
  ts?: number;
  duration?: number;
  air_date?: string;
  watched?: number;
  /** item/{guid} 详情回填（原版剧集行有单集简介） */
  overview?: string;
  /** media_stream.resolutions[0] 去 p 尾（如 1080），原版标题旁的清晰度小徽标 */
  quality?: string;
  media_stream?: { resolutions?: string[] } | null;
};

type Season = {
  guid: string;
  title: string;
  season_number?: number;
  number_of_episodes?: number;
};

/** [lc-119] Bangumi 匹配结果（src-tauri/src/bgm.rs::bgm_match；全线走自建镜像） */
type BgmMatch = {
  subject_id: number;
  name?: string;
  name_cn?: string;
  date?: string;
  score?: number | null;
  rank?: number | null;
  images?: Record<string, string> | null;
  matched_by?: string;
};

/** [lc-1412] Bangumi 评论（长影评）：/p1/subjects/{id}/reviews。
 *  与「吐槽」是两套数据：吐槽是短评（/comments），评论是长影评（/reviews）。
 *  封面在 entry.icon（**多数是 no_photo.png 占位图**，前端碰到要退成图标，不显示灰块）；
 *  entry.summary 是上游已截断的摘要；entry.replies 是回复数。 */
type BgmReview = {
  id: number | string;
  user?: {
    id?: number;
    username?: string;
    nickname?: string;
    avatar?: { small?: string; grid?: string; medium?: string; large?: string };
  };
  entry?: {
    id?: number;
    title?: string;
    icon?: string;
    summary?: string;
    replies?: number;
    createdAt?: number;
    updatedAt?: number;
  };
};

/** [lc-119] Bangumi 吐槽（/p1/subjects/{id}/comments）；分页 limit/offset */
type BgmComment = {
  id: number | string;
  user?: {
    id?: number;
    username?: string;
    nickname?: string;
    avatar?: { small?: string; grid?: string; large?: string };
  };
  rate?: number;
  comment?: string;
  /** 上游是 **Unix 秒**（数字）；类型放宽到 number|string —— 老代码里当字符串 slice 过，
   *  实际值是数字，所以展示前一律走 fmtBgmTime() 归一。 */
  updatedAt?: number | string;
  /** [lc-1412] 「喜欢」：上游形状 `[{users:[{...}], value:0}]`。
   *  **只有少数条目带这个字段**（实测 50 条里 4 条），且 users 是点赞者列表 ——
   *  计数 = 各 value 组的 users 长度之和。 */
  reactions?: { users?: { id?: number; username?: string; nickname?: string }[]; value?: number }[];
};

type PlayTarget = { guid: string; title: string; tvTitle?: string };

/** Bangumi 角色（/v0/subjects/{id}/characters，裸数组）：relation=主角/配角/客串；type=分类；
 *  actors=声优列表（取首位展示）；images 与吐槽头像同形（large/grid/medium/small，可相对路径）。 */
type BgmCharacter = {
  id?: number;
  name?: string;
  relation?: string;
  type?: number;
  images?: Record<string, string> | null;
  actors?: { id?: number; name?: string; images?: Record<string, string> | null }[];
};

/** Bangumi 制作人员（/v0/subjects/{id}/persons，裸数组）：relation=职位（导演/脚本/原作…）；
 *  career 是职业列表（无 relation 时兜底）。 */
type BgmPerson = {
  id?: number;
  name?: string;
  relation?: string;
  career?: string[];
  images?: Record<string, string> | null;
};

/** [lc-123] TMDB 发现页条目（src-tauri/src/tmdb.rs::tmdb_discover 归一化后的 items[]）。
 *  state 只有三态：airing（首播日已到）/ upcoming（首播日未到）/ unknown（缺日期）——
 *  discover 端点**不返回 status**，所以这里刻意没有「连载中/已完结」。
 */
type TmdbDiscoverItem = {
  tmdbId: number;
  title: string;
  originalTitle: string;
  year: string;
  date: string;
  state: "airing" | "upcoming" | "unknown";
  rating: number;
  votes: number;
  popularity: number;
  poster: string;
  backdropPath: string;
  overview: string;
};

/** [lc-123] Bangumi 每日新番表条目（src-tauri/src/bgm.rs::bgm_calendar 归一化后）。 */
type BgmCalendarItem = {
  id: number;
  type: number;
  name: string;
  name_cn: string;
  air_date: string;
  air_weekday?: number | null;
  score?: number | null;
  votes: number;
  doing: number;
  cover: string;
};

/** 星期分组（/calendar 的顶层元素：上游是**裸数组**，不是 /v0 的 {data,total}） */
type BgmCalendarDay = { id: number; cn: string; en: string; ja: string; count: number; items: BgmCalendarItem[] };

/** Bangumi 剧集行（/v0/episodes 的 data[]；只取渲染需要的字段） */
type BgmEpisodeRow = {
  id?: number;
  sort?: number;
  ep?: number;
  name?: string;
  name_cn?: string;
  airdate?: string;
  duration?: string;
  /** 单集简介（bgm_episodes 自带；详情页右栏选集每集小标题下的那段） */
  desc?: string;
};

/** [lc-1401] Bangumi 关联条目（/v0/subjects/{id}/subjects，**裸数组**）。
 *  relation 是上游中文关系词（片头曲/片尾曲/原声集/游戏/画集/广播剧/番外篇/前传…），
 *  type 是条目类型码（1 书籍 / 2 动画 / 3 音乐 / 4 游戏 / 6 三次元），images 可能是空对象。 */
type BgmRelated = {
  id?: number;
  type?: number;
  name?: string;
  name_cn?: string;
  relation?: string;
  images?: Record<string, string> | null;
};

/** [lc-1401] Bangumi 标签（/v0/subjects/{id} 的 tags[]）：count = 该标签被标次数（热度序） */
type BgmTag = { name?: string; count?: number };

/** [lc-128] TMDB 演员 / 制作人员（src-tauri/src/tmdb.rs::tmdb_credits 归一化后）。
 *  剧集带 roles[].episodes（aggregate_credits 的"演了几集"）；crew 带 job/jobLabel（中文职务）。 */
type TmdbCreditPerson = {
  id?: number;
  name?: string;
  character?: string;
  roles?: { character?: string; episodes?: number }[];
  episodes?: number;
  job?: string;
  jobLabel?: string;
  department?: string;
  profile?: string;
  order?: number;
};

/** [lc-128] TMDB 演员表响应（tmdb_credits）：matchedBy = 定位方式（trim_id/imdb/搜索相似度） */
type TmdbCredits = {
  tmdbId?: number;
  mediaType?: string;
  matchedBy?: string;
  cached?: boolean;
  cast?: TmdbCreditPerson[];
  crew?: TmdbCreditPerson[];
};

function fmtSize(b: number): string {
  if (!isFinite(b) || b <= 0) return "0 MB";
  if (b >= 1024 ** 3) return `${(b / 1024 ** 3).toFixed(2)} GB`;
  if (b >= 1024 ** 2) return `${(b / 1024 ** 2).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(b / 1024))} KB`;
}

function pick(o: Record<string, unknown>, keys: string[]): any {
  for (const k of keys) {
    const v = o?.[k];
    if (v !== undefined && v !== null && v !== "") return v;
  }
  return undefined;
}

/** 时长口语化（原版剧集行样式）：1420 → 「23 分钟 40 秒」 */
function fmtDur(sec: number | null | undefined): string {
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
function imgUrl(p?: string | null): string {
  if (!p) return "";
  // [lc-1433] 幂等：已经是本协议的完整 URL 就原样返回。
  //   为什么需要：`tmdbImg()` / `bgmImg()` 产出的就是完整代理 URL，而消费处一律写
  //   `imgUrl(poster)` —— 直接传进去会被再包一层（`.../http://fntvimg.localhost/...`）→ 404。
  //   上游（如详情页 poster 链路）现在是"TMDB 完整 URL 与飞牛相对路径混在一个变量里"，
  //   在这里兜住比在每个调用点判来源更可靠。
  if (p.startsWith("http://fntvimg.localhost") || p.startsWith("fntvimg://localhost")) return p;
  const path = p.startsWith("/") ? p : `/${p}`;
  if (navigator.userAgent.includes("Windows")) return `http://fntvimg.localhost${path}`;
  return `fntvimg://localhost${path}`;
}

/** [lc-119] Bangumi 头像/封面：走 fntvimg 代理的 `/__bgm/*`（**拉了即落盘**，下次零网络），
 *  兼容三种形态：完整 URL / 协议相对 `//lain.bgm.tv/...` / 相对路径 `/pic/user/l/...`。
 *  [lc-123] `/calendar` 的封面是**明文 http://lain.bgm.tv**（该端点的老式返回）——WebView 是 https 页面
 *  上下文，http 图会被当混合内容拦掉 ⇒ 整片裂图。这里统一升到 https（同一主机，站点支持）。
 *  [lc-128] 其它域名（理论上不该有）不代理、不改写：fntvimg 只认识 NAS / TMDB / Bangumi 三个上游。 */
function bgmImg(u?: string | null): string {
  const s = String(u ?? "").trim();
  if (!s) return "";
  let path = "";
  if (s.startsWith("http://lain.bgm.tv") || s.startsWith("https://lain.bgm.tv")) {
    const url = new URL(s.replace(/^http:/, "https:"));
    path = url.pathname + url.search;
  } else if (s.startsWith("//lain.bgm.tv")) {
    path = s.slice("//lain.bgm.tv".length);
  } else if (s.startsWith("http") || s.startsWith("//")) {
    return s;
  } else {
    path = s.startsWith("/") ? s : `/${s}`;
  }
  if (!path) return "";
  // 少数图带查询串：自定义协议下 `?` 会被当 query 丢掉，这里用 %3F 编进路径（Rust 侧原样透传）
  return imgUrl(`/__bgm${path.replace(/\?/g, "%3F")}`);
}

/** [lc-122] TMDB 图片：走 fntvimg 代理的 `/__tmdb/*`（Rust 反代镜像图片域并**落盘缓存**，
 *  免签名）。`path` 是 TMDB 的 file_path（形如 /abc.jpg），`size` 如 w500/w185/**original**。
 *  [lc-128] 不再让 WebView 直连 tmdb.690075.xyz：命中本地磁盘就**绝不重复请求**（用户要求
 *  「拉了就本地缓存不要多拉」），且镜像/域名变更只改 Rust 一处。 */
function tmdbImg(path: string, size: string): string {
  const p = path.startsWith("/") ? path : `/${path}`;
  return imgUrl(`/__tmdb/t/p/${size}${p}`);
}

/* ── [lc-123-rev4] 顶栏自适应反色（按 hero 海报顶部条带的亮度切 chrome 颜色） ──
   背景：顶栏零背景（lc-123-rev3）后，白字/白图标压在一张**亮**海报上会"消失"（用户截图：亮绿海报）。
   做法：采样 hero 图**顶部条带**（顶栏盖住的那一条，高度 8%）的平均感知亮度 0.2126R+0.7152G+0.0722B，
        > 阈值 → 深色 chrome；否则浅色 chrome；采样失败 → 兜底"白字 + 强 text-shadow"。
   跨域前置：图片走 fntvimg 自定义协议（src-tauri/src/imgproto.rs）——那里必须回 `Access-Control-Allow-Origin`，
        否则画布被跨域污染、getImageData 抛 SecurityError，永远落兜底（见 imgproto.rs 注释）。
   缓存：按图片 URL 记忆（同一张只采一次；换片才重采）。
   只对首页 hero 采样：发现/库/详情页顶栏底下没有满幅图（是应用底色），用默认色（见 App.css）。 */
type ChromeTone = "auto" | "dark" | "light" | "fallback";
/** [lc-1437] 设置面板的项（Rust 侧 `get_settings`/`set_settings` 的白名单子集）。
 *  放在 App 顶层持有：性能模式（html.perf 类）与轮播 Logo（CarouselView 入参）
 *  都要在面板之外生效，面板只是它们的编辑界面。 */
type AppSettings = {
  exitMode: "exit" | "tray" | "ask";
  perfMode: boolean;
  carouselLogo: boolean;
  launchPage: "home" | "continue";
  autoLogin: boolean;
};
const DEFAULT_SETTINGS: AppSettings = {
  exitMode: "ask",
  perfMode: false,
  carouselLogo: true,
  launchPage: "home",
  autoLogin: false,
};
/** 平均亮度阈值（0..255）：> 阈值 = 亮底 → 深色 chrome */
const CHROME_LUMA_THRESHOLD = 150;
/** 只取图片顶部这一条（顶栏 40px 在满幅 hero 上覆盖的大致比例） */
const CHROME_STRIP_RATIO = 0.08;
/** 采样画布的目标宽度（按比例缩，够算均值且省时间） */
const CHROME_SAMPLE_WIDTH = 64;
const chromeToneCache = new Map<string, ChromeTone>();

/** 采样一张图的顶部条带 → "dark"（亮底）/ "light"（暗底）；任何失败（跨域/解码/404）→ null（走兜底） */
async function sampleChromeTone(src: string): Promise<ChromeTone | null> {
  const hit = chromeToneCache.get(src);
  if (hit) return hit;
  try {
    const img = new Image();
    img.crossOrigin = "anonymous"; // 必须：不带这个，画布被污染 → getImageData 抛 SecurityError
    img.decoding = "async";
    img.src = src;
    await img.decode();
    const sw = img.naturalWidth;
    const sh = Math.max(1, Math.round(img.naturalHeight * CHROME_STRIP_RATIO));
    if (!sw || !sh) return null;
    const dw = Math.max(1, Math.min(CHROME_SAMPLE_WIDTH, sw));
    const dh = Math.max(1, Math.round(sh * (dw / sw)));
    const cv = document.createElement("canvas");
    cv.width = dw;
    cv.height = dh;
    const ctx = cv.getContext("2d", { willReadFrequently: true });
    if (!ctx) return null;
    ctx.drawImage(img, 0, 0, sw, sh, 0, 0, dw, dh); // 源矩形 = 原图顶部 8%（宽度按比例缩到 ≤64px）
    const { data } = ctx.getImageData(0, 0, dw, dh); // ← 跨域被拦时在这里抛错 → catch → null
    let sum = 0;
    let n = 0;
    for (let i = 0; i < data.length; i += 4) {
      sum += 0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2];
      n++;
    }
    if (!n) return null;
    const tone: ChromeTone = sum / n > CHROME_LUMA_THRESHOLD ? "dark" : "light";
    chromeToneCache.set(src, tone);
    return tone;
  } catch {
    return null; // 跨域（SecurityError）/解码失败/图片 404：一律交给调用方走兜底分支
  }
}

/** item 详情图片字段取值（对齐主仓库 carousel/images.ts pickImg：字符串直取；
 *  数组取首项的 path/url/字面量——新版 fnOS 实测为字符串，数组形态留作兼容） */
function pickImgStr(d: Record<string, unknown>, keys: string[]): string | undefined {
  for (const k of keys) {
    const v = d[k];
    if (v == null || v === "") continue;
    if (typeof v === "string") return v;
    if (Array.isArray(v) && v.length) {
      const first = v[0];
      if (typeof first === "string") return first;
      const obj = first as Record<string, unknown> | null;
      const p = obj?.path ?? obj?.url;
      if (typeof p === "string" && p) return p;
    }
  }
  return undefined;
}

/** [lc-1430] 从图片数组里取**面积最大**的那张（不是第一张）。
 *
 *  为什么必须这样：飞牛的 `backdrops`/`posters` 数组里**第一项经常是竖版小缩略图**，
 *  真正的大横版（1920×1080）排在后面。取第一张 → 首页轮播把一张几百像素的缩略图
 *  拉伸铺满整屏（用户反馈"海报太糊"）。
 *  主仓库 embyWall/carousel/images.ts 的 [lc-599] 早就踩过这个坑并修成"按 w*h 选最大"，
 *  这里是同一套逻辑的移植（那条注释原话："数组第一项经常是竖版小缩略图, 大横版通常在后面"）。
 *
 *  兼容三种元素形态：字符串 / {file_path|url|path|image|src} / {width,height}。
 *  没有任何尺寸信息时**回退到第一张**（不猜、不报错）。
 */
function pickLargestImg(v: unknown): string | undefined {
  const extract = (it: unknown): string => {
    if (typeof it === "string") return it;
    if (!it || typeof it !== "object") return "";
    const o = it as Record<string, unknown>;
    const p = o.file_path ?? o.url ?? o.path ?? o.image ?? o.src;
    return typeof p === "string" ? p : "";
  };
  if (typeof v === "string") return v || undefined;
  if (!Array.isArray(v) || !v.length) return undefined;
  let best = v[0];
  let bestSize = 0;
  for (const it of v) {
    const o = (it && typeof it === "object" ? it : null) as Record<string, unknown> | null;
    const w = Number(o?.width ?? o?.w ?? 0) || 0;
    const h = Number(o?.height ?? o?.h ?? 0) || 0;
    const sz = w * h;
    if (sz > bestSize) {
      bestSize = sz;
      best = it;
    }
  }
  const s = extract(best) || extract(v[0]);
  return s || undefined;
}

/** 库内容加载：库根=ancestor 查询（Episode 由服务端白名单滤掉）；文件夹钻取=parent 查询。
 *  形状依据见 lib.rs get_items 注释（实测：库根传 parent_guid=库guid 返回 0 条，勿兜底）。 */
async function fetchLibItems(libGuid: string, parentGuid: string | null): Promise<MediaItem[]> {
  const d = await invoke<{ list: MediaItem[]; total: number }>("get_items", {
    ancestorGuid: parentGuid === null ? libGuid : null,
    parentGuid,
    page: 1,
  });
  return d.list ?? [];
}

/* ────────────────────────── 标题栏 ────────────────────────── */

/** [lc-123-rev5] 用户是否要求"少动效"：三处动效的 **JS 侧守卫**
 *  （CSS 侧另有 @media (prefers-reduced-motion: reduce) 守卫，双保险）。 */
function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** [lc-123-rev2] 「首页 | 发现」胶囊：**从顶栏挪到内容区顶部的浮层**（.navbar = absolute，
 *  不占布局高度，直接浮在首页海报上；见 App.css）。顶栏恢复干净：左边只留当前页名
 *  （+侧栏/刷新/主题按钮），中间弱化品牌小字，右边窗口按钮。
 *  [lc-123-rev6] 文字 → **内联 SVG 图标**（房子/放大镜）；高亮不再是"直接跳过去"，而是
 *  一块会滑动的**水滴指示器**（等宽两槽 translateX(0|100%) + 过冲回弹缓动），见 App.css。
 *  图标按钮必须带 aria-label + title（无文字时读屏与悬停都要认得出）。
 *  胶囊仍带 `-webkit-app-region: no-drag`（见 App.css .nav-cap）：虽已不贴拖拽区，
 *  这道声明是"胶囊一定点得动"的保险丝（顶栏整条仍是 data-tauri-drag-region 拖拽区）。 */
function NavCapsule({
  page,
  onNav,
}: {
  page: "home" | "discover";
  onNav: (p: "home" | "discover") => void;
}) {
  return (
    <div className="nav-cap" data-side={page} role="tablist" aria-label="主导航">
      {/* [lc-123-rev6] 滑动指示器（"水滴"）：绝对定位、宽度 = 一个槽位，translateX(0|100%) 搬家；
          切换瞬间内部 scaleX(.92)→1 做出黏滞回弹（两个不同 animation-name 保证每次切换都重放） */}
      <span className="nav-cap-ind" aria-hidden="true">
        <i />
      </span>
      <button
        role="tab"
        aria-selected={page === "home"}
        aria-label="首页"
        title="首页"
        className={`nav-cap-l${page === "home" ? " on" : ""}`}
        onClick={() => onNav("home")}
      >
        <IcoHouseL size={18} strokeWidth={1.8} />
      </button>
      <button
        role="tab"
        aria-selected={page === "discover"}
        aria-label="发现"
        title="发现"
        className={`nav-cap-l${page === "discover" ? " on" : ""}`}
        onClick={() => onNav("discover")}
      >
        <IcoSearchL size={18} strokeWidth={1.8} />
      </button>
    </div>
  );
}

function TitleBar({
  title,
  nav,
  page,
  sidebarOpen,
  onToggleSidebar,
  onRefresh,
  refreshing,
  onToggleTheme,
  chrome,
}: {
  title: string;
  nav?: boolean;
  /** 当前"页"（首页/发现）：左上角那行字跟着它切（胶囊已挪到内容区浮层，见 NavCapsule） */
  page?: "home" | "discover";
  sidebarOpen?: boolean;
  onToggleSidebar?: () => void;
  onRefresh?: () => void;
  refreshing?: boolean;
  /** [lc-123-rev6] 传事件：深浅切换的圆形扩散要拿**按钮中心**做原点（getBoundingClientRect） */
  onToggleTheme?: (e: React.MouseEvent<HTMLButtonElement>) => void;
  /** [lc-123-rev4] 顶栏 chrome 色：auto=跟随主题（默认）；dark/light=按 hero 亮度采样；fallback=白字强阴影 */
  chrome?: ChromeTone;
}) {
  const win = getCurrentWindow();
  const crumb = page === "discover" ? "发现" : "首页";
  return (
    <div
      className="titlebar"
      data-tauri-drag-region
      data-chrome={chrome && chrome !== "auto" ? chrome : undefined}
    >
      <div className="tb-left" data-tauri-drag-region>
        {nav && onToggleSidebar && (
          <button
            className="tb-icon"
            title={sidebarOpen ? "收起侧栏" : "展开侧栏"}
            onClick={onToggleSidebar}
          >
            <IcoMenuL size={16} strokeWidth={1.8} />
          </button>
        )}
        {/* [lc-123] 左上角这行字 = **当前页名**（首页/发现），随胶囊切换（不再是"回首页"按钮） */}
        {nav && <span className="tb-crumb">{crumb}</span>}
        {nav && onRefresh && (
          <button
            className={`tb-icon${refreshing ? " spinning" : ""}`}
            title="刷新首页数据"
            onClick={onRefresh}
            disabled={refreshing}
          >
            ⟳
          </button>
        )}
        {onToggleTheme && (
          <button className="tb-icon" title="切换深浅模式" onClick={(e) => onToggleTheme(e)}>
            ◐
          </button>
        )}
        {!nav && <span className="tb-page">{title}</span>}
      </div>
      {/* [lc-123-rev] 中部只剩弱化的品牌小字（原来这里是「首页|发现」胶囊，已挪到 .navbar 行）。
          整条顶栏仍是拖拽区：中间的 span 不拦截拖拽，窗口拖动不受影响。 */}
      <div className="tb-center" data-tauri-drag-region>
        <span className="tb-brand">{nav ? "AniBox" : title}</span>
      </div>
      <div className="tb-right" data-tauri-drag-region>
        <div className="tb-buttons">
          <button className="tb-btn" title="最小化" onClick={() => win.minimize()}>
            ─
          </button>
          <button className="tb-btn" title="最大化/还原" onClick={() => win.toggleMaximize()}>
            ▢
          </button>
          <button className="tb-btn tb-close" title="关闭" onClick={() => win.close()}>
            <IcoXL size={16} strokeWidth={1.8} />
          </button>
        </div>
      </div>
    </div>
  );
}

/* ────────────────────────── 登录页 ────────────────────────── */

function LoginPage({ onDone }: { onDone: () => void }) {
  const [cfg, setCfg] = useState<Config | null>(null);
  const [server, setServer] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [remember, setRemember] = useState(false);
  const [autoLogin, setAutoLogin] = useState(false);
  const [mpvPath, setMpvPath] = useState("");
  const [proxyPath, setProxyPath] = useState("");
  const [showAdv, setShowAdv] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  // 自动登录一次性闸门：失败后停在本页报错，绝不循环重试
  const autoTried = useRef(false);

  useEffect(() => {
    invoke<Config>("get_config").then((c) => {
      setCfg(c);
      setServer(c.server);
      setUsername(c.username);
      setPassword(c.password || "");
      setRemember(!!c.password);
      setAutoLogin(c.auto_login && !!c.password);
      setMpvPath(c.mpv_path);
      setProxyPath(c.proxy_path);
    });
  }, []);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setErr("");
    try {
      await invoke("save_config", {
        config: {
          ...cfg,
          server: server.trim(),
          username,
          password: remember ? password : "",
          auto_login: remember && autoLogin,
          mpv_path: mpvPath,
          proxy_path: proxyPath,
        },
      });
      await invoke("login", { server: server.trim(), username, password });
      onDone();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }

  // 启动自动登录：token 已由 check_session 把过关，走到登录页即代表失效；
  // 记住了密码且开了自动登录 → 用记住的凭据静默重登一次（成功直接进首页）
  useEffect(() => {
    if (autoTried.current || !cfg) return;
    if (!cfg.auto_login || !cfg.password || !cfg.server || !cfg.username) return;
    autoTried.current = true;
    setBusy(true);
    invoke("login", { server: cfg.server, username: cfg.username, password: cfg.password })
      .then(() => onDone())
      .catch((e) => {
        setErr("自动登录失败：" + String(e));
        setBusy(false);
      });
  }, [cfg, onDone]);

  return (
    <div className="login-wrap">
      <div className="login-hero">
        <div className="login-hero-glow" />
        <h1>FN·TV</h1>
        <p>自绘 UI · 直调飞牛 API · 内嵌 mpv</p>
      </div>
      <form className="login-card" onSubmit={submit}>
        <h2>登录飞牛影视</h2>
        <label>服务器地址</label>
        <input
          value={server}
          onChange={(e) => setServer(e.target.value)}
          placeholder="如 http://192.168.31.170:18888 或 100.66.1.2:18888"
          required
        />
        <label>用户名</label>
        <input value={username} onChange={(e) => setUsername(e.target.value)} required />
        <label>密码</label>
        <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} required />
        <div className="login-opts">
          <label className="login-check">
            <input
              type="checkbox"
              checked={remember}
              onChange={(e) => {
                setRemember(e.target.checked);
                if (!e.target.checked) setAutoLogin(false);
              }}
            />
            记住密码
          </label>
          <label className="login-check">
            <input
              type="checkbox"
              checked={autoLogin}
              disabled={!remember}
              onChange={(e) => setAutoLogin(e.target.checked)}
            />
            自动登录
          </label>
        </div>
        <button type="button" className="linklike" onClick={() => setShowAdv(!showAdv)}>
          高级设置（mpv / 代理路径）
        </button>
        {showAdv && (
          <>
            <label>mpv.exe 路径</label>
            <input value={mpvPath} onChange={(e) => setMpvPath(e.target.value)} />
            <label>Go 代理路径</label>
            <input value={proxyPath} onChange={(e) => setProxyPath(e.target.value)} />
          </>
        )}
        {err && <div className="err">{err}</div>}
        <button className="primary" disabled={busy}>
          {busy ? "登录中…" : "登 录"}
        </button>
      </form>
    </div>
  );
}

/* ────────────────────────── 首页全屏海报轮播（复刻 emby-crx Misty banner） ────────────────────────── */

function CarouselView({
  items,
  bds,
  logos,
  onPlay,
  onDetail,
  onTone,
}: {
  items: MediaItem[];
  bds: Record<string, string>;
  logos: Record<string, string>;
  onPlay: (t: PlayTarget) => void;
  onDetail: (item: MediaItem) => void;
  /** [lc-123-rev4] 上报"当前这张 hero 图的顶栏 chrome 色"（App 透传给 TitleBar；仅首页用） */
  onTone?: (t: ChromeTone) => void;
}) {
  // [lc-1523] 真无限轮播：轨道 = [最后一张克隆, ...items, 第一张克隆]，
  //   pos 是**轨道下标**（1..n 为真实位；0 / n+1 为首尾克隆位）。
  //   跨接时先滑到克隆位（动画连续），transitionend 后再无动画瞬移回真实位。
  const [pos, setPos] = useState(1);
  const [armed, setArmed] = useState(false);
  // [lc-116] 鼠标靠近哪一侧："" | "prev" | "next"（用来淡入对应箭头）
  const [near, setNear] = useState<"" | "prev" | "next">("");
  // [lc-117] 首尾跨接的那一帧临时关掉 .mist-body 的 1.5s 横向过渡
  const [noAnim, setNoAnim] = useState(false);
  const n = items.length;

  const onBannerMove = (e: React.MouseEvent<HTMLDivElement>) => {
    const box = e.currentTarget;
    const w = box.clientWidth || 1;
    const x = e.clientX - box.getBoundingClientRect().left;
    const zone = Math.min(260, w * 0.22);
    setNear(x <= zone ? "prev" : x >= w - zone ? "next" : "");
  };

  /**
   * [lc-1523] 统一走这里切页，delta = -1 / +1。克隆轨道上直接 ±1：
   * 最后一张( pos=n )再 +1 → 滑到第一张克隆( n+1 )，动画连续；transitionend
   * 后 onTransEnd 无动画瞬移回真实第一张( 1 )。第一张( 1 )再 -1 同理走 last 克隆( 0 )。
   */
  const go = useCallback(
    (delta: number) => {
      if (n < 2) return;
      const target = pos + delta;
      if (target < 0 || target > n + 1) return;
      setPos(target);
    },
    [pos, n]
  );

  /** [lc-1523] 滑到克隆位后的瞬间重置：关过渡 → 瞬移回真实位 → 恢复过渡。 */
  const onTransEnd = useCallback((e: React.TransitionEvent) => {
    // 只认容器自身 transform 的过渡结束：子元素（图/遮罩/文字）的 transition 冒泡上来不理会
    if (e.target !== e.currentTarget || e.propertyName !== "transform") return;
    if (n < 2) return;
    if (pos === n + 1 || pos === 0) {
      setNoAnim(true);
      setPos(pos === n + 1 ? 1 : n);
      requestAnimationFrame(() => requestAnimationFrame(() => setNoAnim(false)));
    }
  }, [pos, n]);

  // items 数量变化（刷新/换库）时把轨道拉回真实首位，避免 pos 越界
  useEffect(() => {
    setPos(1);
  }, [n]);

  // 入场动画闸门：挂载后瞬间补 active，让首帧信息走 misty 同款上浮入场
  useEffect(() => {
    const t = setTimeout(() => setArmed(true), 80);
    return () => clearTimeout(t);
  }, []);

  // 8s 自动轮播（emby-crx 同款节奏；页面隐藏时不切，避免回前台跳帧）
  // [lc-117] 依赖当前片（pos）而不是常驻 interval：手动点箭头后重新起算 8s，
  // 否则刚点过去就被自动轮播顶走，看着像「点了没反应」。
  useEffect(() => {
    if (n < 2) return;
    const t = setTimeout(() => {
      if (!document.hidden) go(1);
    }, 8000);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [n, pos]);

  // [lc-123-rev4] 顶栏自适应反色：把**当前这一张** hero 图的 URL 采样成 chrome 色上报。
  // 换片（pos 变）才重采；同一张图有 URL 级缓存（sampleChromeTone 内部）。采样失败 → "fallback"。
  const seq = n >= 2 ? [items[n - 1], ...items, items[0]] : items;
  const activeItem = seq[Math.min(pos, seq.length - 1)];
  const activeSrc = activeItem
    ? bds[activeItem.guid]
      ? imgUrl(bds[activeItem.guid])
      : activeItem.poster
        ? imgUrl(activeItem.poster)
        : null
    : null;
  useEffect(() => {
    if (!onTone) return;
    if (!activeSrc) {
      onTone("auto"); // 这一张没有图（垫底色块）→ 交回默认色
      return;
    }
    let alive = true;
    sampleChromeTone(activeSrc).then((t) => {
      if (alive) onTone(t ?? "fallback");
    });
    return () => {
      alive = false;
    };
  }, [activeSrc, onTone]);

  if (n === 0) return null;
  const yearOf = (it: MediaItem) => String(it.release_date || it.air_date || "").slice(0, 4);

  return (
    <div
      className="mist-banner"
      onMouseMove={onBannerMove}
      onMouseLeave={() => setNear("")}
    >
      <div
        className={`mist-body${noAnim ? " no-anim" : ""}`}
        style={{ transform: `translateX(-${pos * 100}%)` }}
        onTransitionEnd={onTransEnd}
      >
        {seq.map((it, i) => {
          const bd = bds[it.guid];
          const meta = [
            yearOf(it),
            (it.type || "").toLowerCase() === "movie" ? "电影" : "剧集",
            it.vote_average && it.vote_average !== "0" ? `${it.vote_average}` : "",
          ]
            .filter(Boolean)
            .join(" · ");
          return (
            <div key={it.guid} className={`mist-item ${i === pos && armed ? "active" : ""}`}>
              {bd ? (
                <img
                  className="mist-cover"
                  src={imgUrl(bd)}
                  alt={it.title}
                  draggable={false}
                  loading={i <= 1 ? "eager" : "lazy"}
                />
              ) : (
                <div
                  className="mist-cover mist-cover-fallback"
                  style={
                    it.poster
                      ? { backgroundImage: `url(${imgUrl(it.poster)})` }
                      : undefined
                  }
                />
              )}
              <div className="mist-veil" />
              <div className="mist-info">
                <div className="mist-meta">{meta}</div>
                {logos[it.guid] ? (
                  <img
                    className="mist-logo"
                    src={imgUrl(logos[it.guid])}
                    alt={it.title}
                    draggable={false}
                    loading="lazy"
                  />
                ) : (
                  <h1>{it.title}</h1>
                )}
                <div>
                  <p>{it.overview || ""}</p>
                </div>
                <div className="mist-actions">
                  <button
                    className="mist-play"
                    onClick={() => onPlay({ guid: it.guid, title: it.title })}
                  >
                    ▶ 开始播放
                  </button>
                  <button className="mist-more" onClick={() => onDetail(it)}>
                    MORE
                  </button>
                </div>
              </div>
            </div>
          );
        })}
      </div>
      {/* [lc-116] 左右切换箭头：默认完全透明；鼠标移到对应侧的"附近区域"
          （左右各约 22% 宽，最多 260px）才淡入 —— 用 mousemove 判断靠近哪一侧，
          而不是 hover 整个 banner（hero 占满一屏，那样等于一直显示）。
          [lc-117] 三个坑：
          1) onMouseDown 里 preventDefault —— 原生 button 按下会抢焦点，浏览器把焦点元素
             滚进视口会让整页上下窜一下（第二个「跳动」来源）。
          2) 事件用 go(-1)/go(+1)，取模实现首尾反向回绕（第一张往左 = 最后一张）。
          3) 不再用 pointer-events:none 门控可点性 —— 鼠标静止时 mousemove 不刷新 near，
             按钮可能停在「看得见但点不动」的状态；透明时点到也只是切页，无副作用。 */}
      {n > 1 && (
        <>
          <button
            type="button"
            className={`mist-nav mist-nav-prev${near === "prev" ? " is-on" : ""}`}
            title="上一张"
            aria-label="上一张"
            onMouseEnter={() => setNear("prev")}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => go(-1)}
          >
            ‹
          </button>
          <button
            type="button"
            className={`mist-nav mist-nav-next${near === "next" ? " is-on" : ""}`}
            title="下一张"
            aria-label="下一张"
            onMouseEnter={() => setNear("next")}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => go(1)}
          >
            ›
          </button>
        </>
      )}
    </div>
  );
}

/* ────────────────────────── 海报卡 ────────────────────────── */

function PosterCard({
  item,
  onOpen,
  subtitle,
}: {
  item: MediaItem;
  onOpen: (item: MediaItem) => void;
  subtitle?: string;
}) {
  const isPlayable = item.type === "Movie" || item.type === "Episode";
  const progress =
    item.ts && item.duration && item.duration > 0 ? Math.min(100, (item.ts / item.duration) * 100) : 0;
  return (
    <div className="card" onClick={() => onOpen(item)} title={item.title}>
      <div className="poster-box">
        {item.poster ? (
          <>
            {/* [lc-114] 骨架屏放在图片**下层**（纯 CSS 层叠）。此前用 opacity 切换且依赖
                onLoad，而图片命中 WebView 缓存时常在 React 挂监听前就 complete ⇒ 回调不触发、
                图片永久 opacity:0 ⇒ "图片出不来"。层叠写法没有这个竞态。 */}
            <div className="poster-skeleton" />
            <img src={imgUrl(item.poster)} loading="lazy" alt={item.title} />
          </>
        ) : (
          <div className="poster-empty"><IcoFilmL size={22} strokeWidth={1.8} /></div>
        )}
        <div className="card-hover">
          <span className="play-pill">{isPlayable ? "▶ 播放" : "打开"}</span>
        </div>
        {item.vote_average && item.vote_average !== "0" && (
          <span className="rating"><IcoStarL size={12} strokeWidth={2} className="rating-ico fill" /> {item.vote_average}</span>
        )}
        {progress > 0 && (
          <div className="progress">
            <div style={{ width: `${progress}%` }} />
          </div>
        )}
        {item.type === "Episode" && item.season_number != null && (
          <span className="ep-badge">
            S{item.season_number}E{item.episode_number ?? ""}
          </span>
        )}
        {item.watched === 1 && <span className="watched-badge"><IcoCheckL size={11} strokeWidth={2.2} /></span>}
      </div>
      <div className="card-title">{subtitle || item.title}</div>
      {subtitle && item.title !== subtitle && <div className="card-sub">{item.title}</div>}
    </div>
  );
}

/** [lc-1407] 可横向翻页的一行（首页「继续观看」与各媒体库横排共用）。
 *
 *  为什么需要它：这两条横排是 `overflow-x: auto`，但本项目**全局隐藏了滚动条**
 *  （`* { scrollbar-width: none }`，WebView2 的 Fluent 滚动条无视 ::-webkit-scrollbar 才这么干的），
 *  而鼠标滚轮默认只滚纵向 —— 于是横排实际上"没有任何可用的滚动入口"
 *  （用户反馈：无法横向滑动）。这里改成**轮播同款雪佛龙箭头**翻页（用户指定）。
 *
 *  实现要点：
 *   - 翻页距离 = **可视宽度 × 0.9**（留一点重叠，让用户看得出是接着翻的），按 smooth 滚；
 *   - 到两端后对应箭头**自动淡出并禁用**（与轮播的 near 透明逻辑同一套观感）；
 *   - 箭头只在鼠标进入这一行时才显形（否则首页一屏里到处是箭头，很吵）；
 *   - 用 ResizeObserver 跟踪可视宽度，窗口缩放后翻页距离自动跟着变。
 */
function ScrollRow({
  children,
  boxClass,
  imgSel = ".poster-box",
  inSec = false,
}: {
  children: ReactNode;
  boxClass?: string;
  /** 量哪个元素的高度作为箭头垂直居中的基准（默认 .poster-box；剧照行传 img 选择器） */
  imgSel?: string;
  /** 这一行嵌在带内边距的卡片里（如 .tmdb-sec）→ 箭头左右内收到 14px，不压卡片边框 */
  inSec?: boolean;
}) {
  const boxRef = useRef<HTMLDivElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const [hover, setHover] = useState(false);
  const [atStart, setAtStart] = useState(true);
  const [atEnd, setAtEnd] = useState(false);

  const sync = useCallback(() => {
    const el = boxRef.current;
    if (!el) return;
    // 1px 容差：分数缩放（125%/150%）下 scrollLeft 是小数，等于阈值时不该误判成"还能滚"
    setAtStart(el.scrollLeft <= 1);
    setAtEnd(el.scrollLeft + el.clientWidth >= el.scrollWidth - 1);
    // [lc-1409] 箭头要**相对图片区域垂直居中**（用户要求），而不是相对整行 ——
    //   整行还含图片下方的标题/副标题，按整行居中会让箭头落到图片偏下的位置。
    //   这里量出第一张卡片的图片高度写进 CSS 变量（.poster-box 有 aspect-ratio，
    //   高度只取决于宽度，不依赖图片是否加载完成，所以量得到就是准的）。
    const poster = el.querySelector<HTMLElement>(imgSel);
    const wrap = wrapRef.current;
    if (wrap) {
      // 量不到图片高度时（首帧未布局/结构变化）退化为"整行高度" ——
      // 箭头就变成相对整行居中，比固定一个猜的像素值稳（继续观看与媒体库的图片高度差很多，
      // 任何写死的兜底值都必然对其中一种不准）。
      const imgH = poster && poster.offsetHeight > 0 ? poster.offsetHeight : el.clientHeight;
      if (imgH > 0) wrap.style.setProperty("--srow-img-h", `${imgH}px`);
    }
  }, [imgSel]);

  useEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    sync();
    el.addEventListener("scroll", sync, { passive: true });
    let ro: ResizeObserver | null = null;
    if (typeof ResizeObserver !== "undefined") {
      ro = new ResizeObserver(sync);
      ro.observe(el);
    }
    window.addEventListener("resize", sync);
    return () => {
      el.removeEventListener("scroll", sync);
      ro?.disconnect();
      window.removeEventListener("resize", sync);
    };
  }, [sync, children, imgSel]);

  const page = (dir: -1 | 1) => {
    const el = boxRef.current;
    if (!el) return;
    const step = Math.max(160, el.clientWidth * 0.9);
    el.scrollBy({ left: dir * step, behavior: "smooth" });
  };

  return (
    <div
      className={`srow${inSec ? " insec" : ""}`}
      ref={wrapRef}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <div className={boxClass ? `hscroll ${boxClass}` : "hscroll"} ref={boxRef}>
        {children}
      </div>
      <button
        type="button"
        className={`srow-nav srow-prev${hover && !atStart ? " is-on" : ""}`}
        title="向左翻"
        aria-label="向左翻"
        disabled={atStart}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => page(-1)}
      >
        ‹
      </button>
      <button
        type="button"
        className={`srow-nav srow-next${hover && !atEnd ? " is-on" : ""}`}
        title="向右翻"
        aria-label="向右翻"
        disabled={atEnd}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => page(1)}
      >
        ›
      </button>
    </div>
  );
}

/** [lc-1414] 头像兜底：**拉不到图就用内置图标**，绝不显示裂图（用户明确要求）。
 *
 *  为什么需要：Bangumi 头像 URL 基本都带 `?r=…&hd=1` 查询串，历史上前端把它编成 %3F、
 *  Rust 侧没还原 → 上游 404 → 整片裂图（根因已单独修）。但上游偶发 404/超时仍可能发生，
 *  所以渲染层也要兜住：onError 后换成图标，并按用户名做**稳定的颜色**（刷新不变）。
 *
 *  用内置图标而不是灰底/首字母：与项目里其它占位（.poster-empty 用 IcoFilmL）同一套语言，
 *  视觉上"这是个占位"比"这是个坏图"清楚。
 */
function AvatarSafe({
  src,
  name,
  size = 36,
  className = "dvtc-ava",
}: {
  src?: string;
  /** 用于取稳定配色与 title（同一个人每次都是同一个颜色） */
  name?: string;
  size?: number;
  className?: string;
}) {
  const [failed, setFailed] = useState(false);
  const showImg = !!src && !failed;
  // 稳定的色相：把名字哈希成 0..360（同一个人刷新前后颜色一致，不会"闪色"）
  const hue = (() => {
    const s = String(name || "");
    let h = 0;
    for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360;
    return h;
  })();
  if (showImg) {
    return (
      <img
        className={className}
        src={src}
        alt=""
        loading="lazy"
        width={size}
        height={size}
        onError={() => setFailed(true)}
      />
    );
  }
  return (
    <span
      className={`${className} avatar-fallback`}
      style={{ background: `hsl(${hue} 42% 62%)` }}
      title={name || ""}
      aria-hidden="true"
    >
      <IcoUserL size={Math.round(size * 0.5)} strokeWidth={1.9} />
    </span>
  );
}

/** [lc-1417] 详情页骨架屏（用户反馈"没做骨架屏"，进页面时是空等）。
 *
 *  形状按详情页真实布局摆：左列 = 海报 + 标签 chips + 简介行；右列 = 选集行（图 + 两行文字）。
 *  复用既有 `.dsk` 的 shimmer 语言（与首页/发现页同一套），不再另造一套动画。
 *
 *  为什么必须连"右栏选集"一起摆：详情页的等待感主要来自右侧——条目详情没回来时
 *  选集整个是空的（用户截图里右侧就是一片白），只给左列摆骨架的话右侧空白更显慢。
 */
function DetailSkeleton() {
  return (
    <div className="dv2">
      <div className="dv2-left">
        <div className="dv2-top">
          <div className="dv2-poster"><div className="dsk dsk-dposter" /></div>
          <div className="dv2-topinfo">
            <div className="dv2-itags">
              {[0, 1, 2, 3, 4, 5].map((i) => (
                <div key={i} className="dsk dsk-chip" />
              ))}
            </div>
            <div className="dv2-isum">
              {[0, 1, 2, 3].map((i) => (
                <div key={i} className={`dsk dsk-sline${i === 3 ? " short" : ""}`} />
              ))}
            </div>
          </div>
        </div>
        <div className="dseg dv2-seg">
          {[0, 1, 2, 3, 4].map((i) => (
            <div key={i} className="dsk dsk-tab" />
          ))}
        </div>
      </div>
      <aside className="dv2-right">
        {/* [lc-1421] 骨架与真实头部同构：播放按钮在左、标题与元信息跟其后（一排） */}
        <div className="dv2e-hd">
          <div className="dsk dsk-playbtn" />
          <div className="dsk dsk-hd" />
          <div className="dsk dsk-hd2" />
        </div>
        <div className="dv2e-list">
          {[0, 1, 2].map((i) => (
            <div className="dv2e-row" key={i}>
              <div className="dv2e-thumb"><div className="dsk dsk-dthumb" /></div>
              <div className="dv2e-main">
                <div className="dsk dsk-sline" />
                <div className={`dsk dsk-sline${i % 2 ? " short" : ""}`} />
                <div className="dsk dsk-sline short" />
              </div>
            </div>
          ))}
        </div>
      </aside>
    </div>
  );
}

/* ────────────────────────── 首页骨架屏 ────────────────────────── */

function HomeSkeleton() {
  return (
    <main className="content home-full">
      <div className="skel hero-skel" />
      <div className="skel-row">
        {Array.from({ length: 5 }).map((_, i) => (
          <div key={i} className="skel lsc-skel" />
        ))}
      </div>
      {[0, 1].map((r) => (
        <section className="home-sec" key={r}>
          <div className="skel sec-title-skel" />
          <div className="skel-row">
            {Array.from({ length: 6 }).map((_, i) => (
              <div key={i} className="skel card-skel" />
            ))}
          </div>
        </section>
      ))}
    </main>
  );
}

/* ────────────────────────── 首页 ────────────────────────── */

function HomeView({
  libs,
  hiddenLibs,
  onToggleHiddenLib,
  continueList,
  carousel,
  carouselBds,
  carouselLogos,
  carouselLogoOn,
  launchPage,
  libRows,
  onOpenLib,
  onPlay,
  onOpenItem,
  onHeroTone,
}: {
  libs: Lib[];
  hiddenLibs: string[];
  onToggleHiddenLib: (guid: string) => void;
  continueList: PlayItem[];
  carousel: MediaItem[];
  carouselBds: Record<string, string>;
  carouselLogos: Record<string, string>;
  /** [lc-1438] 「轮播透明标题」开关：关时传空表让 CarouselView 走文字标题。
   *  不在这里过滤数据（数据留着，下次打开开关无需重拉）。 */
  carouselLogoOn: boolean;
  /** [lc-1439] 启动页设置：`"continue"` = 首页加载后自动滚到「继续观看」 */
  launchPage: "home" | "continue";
  libRows: { lib: Lib; items: MediaItem[] }[];
  onOpenLib: (lib: Lib) => void;
  onPlay: (t: PlayTarget) => void;
  onOpenItem: (item: MediaItem) => void;
  /** [lc-123-rev4] hero 图亮度采样结果上报（App → TitleBar 自适应反色） */
  onHeroTone?: (t: ChromeTone) => void;
}) {
  const [manageOpen, setManageOpen] = useState(false);
  const shownLibs = libs.filter((l) => !hiddenLibs.includes(l.guid));
  // [lc-1439] 启动页 = 继续观看：等「继续观看」区块渲染出来后把它滚到视口顶部。
  //   用 ref + scrollIntoView 而不是算像素：hero 是 100vh，偏移量随窗口高度变，
  //   算像素就得维护"hero 高度 + 媒体库条高度"这套常量（改版式必忘）。
  //   只在**首次**挂载后执行一次（ref 清空），否则用户手动滚回顶部又会被拽下去。
  const homeRef = useRef<HTMLElement | null>(null);
  const launchScrolled = useRef(false);
  useEffect(() => {
    if (launchScrolled.current || launchPage !== "continue") return;
    if (!continueList.length) return; // 没有观看记录就没得滚（保持首页顶部）
    launchScrolled.current = true;
    // 下一帧再滚：等这一轮 DOM 提交完（区块可能刚随 continueList 出现）
    const t = window.setTimeout(() => {
      const box = homeRef.current;
      const sec = box?.querySelector<HTMLElement>(".home-sec");
      if (box && sec) box.scrollTo({ top: sec.offsetTop, behavior: "auto" });
    }, 60);
    return () => window.clearTimeout(t);
  }, [launchPage, continueList.length]);

  // [lc-1431] **首页左缘对齐自检**（仅 dev）：启动后量四个关键元素的实际左边缘，
  //   写进诊断日志（dev_log → libmpv::trace）。
  //   为什么加这个：本轮我按 CSS 规范推演左缘，前后翻了两次结论（把 .content/.home-full
  //   当成父子、又漏算负边距），最后靠重新读 DOM 才定案 —— 推演不可靠，实测才可靠。
  //   这段代码在**真实渲染**后取值，日志里直接给出差值，以后改布局不必再猜。
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const t = window.setTimeout(() => {
      const leftOf = (sel: string): number | null => {
        const el = document.querySelector(sel);
        return el ? Math.round(el.getBoundingClientRect().left) : null;
      };
      const rows: [string, number | null][] = [
        ["轮播标题", leftOf(".mist-info h1")],
        ["轮播简介", leftOf(".mist-info p")],
        ["开始播放", leftOf(".mist-play")],
        ["媒体库条卡", leftOf(".lib-strip-card")],
        ["区块标题", leftOf(".home-sec .section-head h3")],
        ["横排首卡", leftOf(".hscroll .card")],
      ];
      const vals = rows.map(([, v]) => v).filter((v): v is number => v != null);
      const spread = vals.length ? Math.max(...vals) - Math.min(...vals) : -1;
      const detail = rows.map(([k, v]) => `${k}=${v ?? "-"}`).join(" ");
      invoke("dev_log", {
        msg: `[align] ${detail} | 极差=${spread}px${spread === 0 ? " ✅对齐" : " ⚠️不齐"}`,
      }).catch(() => {});
    }, 1500);
    return () => window.clearTimeout(t);
  }, []);

  return (
    <main className="content home-full" ref={homeRef}>
      {/* 全屏 hero 轮播 + 底部媒体库条（Emby 布局） */}
      <div className="hero-zone">
        <CarouselView
          items={carousel}
          bds={carouselBds}
          logos={carouselLogoOn ? carouselLogos : {}}
          onPlay={onPlay}
          onDetail={onOpenItem}
          onTone={onHeroTone}
        />
        <div className="lib-strip">
          {shownLibs.map((lib) => (
            <div key={lib.guid} className="lib-strip-card" onClick={() => onOpenLib(lib)}>
              <div className="lsc-bg">
                {lib.posters && lib.posters[0] ? (
                  <img src={imgUrl(lib.posters[0])} loading="lazy" alt={lib.title} />
                ) : (
                  <div className="poster-empty"><IcoClapL size={22} strokeWidth={1.8} /></div>
                )}
              </div>
              <div className="lsc-shade" />
              <div className="lsc-title">{lib.title}</div>
              <div className="lsc-cat">{lib.category}</div>
              <button
                type="button"
                className="lsc-hide"
                title="首页不展示该库"
                onClick={(e) => {
                  e.stopPropagation();
                  onToggleHiddenLib(lib.guid);
                }}
              >
                <IcoXL size={13} strokeWidth={1.8} />
              </button>
            </div>
          ))}
          <div
            className={`lsc-manage ${shownLibs.length === 0 ? "always" : ""}`}
            title="管理首页媒体库展示"
            onClick={() => setManageOpen((v) => !v)}
          >
            <IcoGearL size={15} strokeWidth={1.8} /> 管理
          </div>
        </div>
        {manageOpen && (
          <>
            <div className="lsc-pop-mask" onClick={() => setManageOpen(false)} />
            <div className="lsc-pop">
              <div className="lsc-pop-head">首页媒体库展示</div>
              {libs.map((lib) => (
                <label key={lib.guid} className="lsc-pop-row">
                  <input
                    type="checkbox"
                    checked={!hiddenLibs.includes(lib.guid)}
                    onChange={() => onToggleHiddenLib(lib.guid)}
                  />
                  <span>{lib.title}</span>
                  <span className="lsc-pop-cat">{lib.category}</span>
                </label>
              ))}
            </div>
          </>
        )}
      </div>

      {/* 继续观看 */}
      {continueList.length > 0 && (
        <section className="home-sec">
          <div className="section-head">
            <h3>继续观看</h3>
          </div>
          <ScrollRow>
            {continueList.map((raw) => {
              const guid = String(pick(raw, ["guid"]) ?? "");
              const title = String(pick(raw, ["title"]) ?? "");
              const tvTitle = pick(raw, ["tvTitle", "tv_title"]) as string | undefined;
              const poster = pick(raw, ["poster"]) as string | undefined;
              const ts = Number(pick(raw, ["ts"]) ?? 0);
              const duration = Number(pick(raw, ["duration"]) ?? 0);
              const sn = pick(raw, ["seasonNumber", "season_number"]);
              const en = pick(raw, ["episodeNumber", "episode_number"]);
              const progress = duration > 0 ? Math.min(100, (ts / duration) * 100) : 0;
              return (
                <div
                  key={guid}
                  className="card continue-card"
                  onClick={() => onPlay({ guid, title, tvTitle })}
                >
                  <div className="poster-box wide">
                    {poster ? (
                      <img src={imgUrl(poster)} loading="lazy" alt={title} />
                    ) : (
                      <div className="poster-empty"><IcoFilmL size={22} strokeWidth={1.8} /></div>
                    )}
                    <div className="card-hover">
                      <span className="play-pill">▶ 续播</span>
                    </div>
                    {progress > 0 && (
                      <div className="progress">
                        <div style={{ width: `${progress}%` }} />
                      </div>
                    )}
                    {sn != null && (
                      <span className="ep-badge">
                        S{sn}E{en ?? ""}
                      </span>
                    )}
                  </div>
                  <div className="card-title">{tvTitle || title}</div>
                  {tvTitle && <div className="card-sub">{title}</div>}
                </div>
              );
            })}
          </ScrollRow>
        </section>
      )}

      {/* 按库顺序的剧集横排（隐藏的库不展示） */}
      {libRows
        .filter(({ lib }) => !hiddenLibs.includes(lib.guid))
        .map(({ lib, items }) =>
        items.length > 0 ? (
          <section className="home-sec" key={lib.guid}>
            <div className="section-head">
              <h3>{lib.title}</h3>
              <span className="muted">{items.length} 项</span>
              <span className="row-more" onClick={() => onOpenLib(lib)}>
                全部 ›
              </span>
            </div>
            <ScrollRow>
              {items.map((it) => (
                <PosterCard key={it.guid} item={it} onOpen={onOpenItem} />
              ))}
            </ScrollRow>
          </section>
        ) : null,
      )}
    </main>
  );
}

/* ────────────────────────── 媒体库浏览 ────────────────────────── */

function LibView({
  lib,
  onBackHome,
  onOpenItem,
}: {
  lib: Lib;
  onBackHome: () => void;
  onOpenItem: (item: MediaItem) => void;
}) {
  const [items, setItems] = useState<MediaItem[]>([]);
  const [total, setTotal] = useState(0);
  const [stack, setStack] = useState<{ folder: MediaItem; items: MediaItem[] }[]>([]);
  const [err, setErr] = useState("");
  const [loading, setLoading] = useState(true);

  const current = stack.length > 0 ? stack[stack.length - 1].folder : null;

  const load = useCallback(
    async (parentGuid: string | null) => {
      setLoading(true);
      setErr("");
      // 虚拟库：收藏 / 全局分类浏览
      if (lib.guid === "_fav") {
        try {
          const d = await invoke<{ total: number; list: MediaItem[] | null }>("get_favorites", {
            page: 1,
          });
          setItems(d.list ?? []);
          setTotal(d.total ?? 0);
          if (!d.list?.length) setErr("还没有收藏");
        } catch (e) {
          setErr(String(e));
        }
        setLoading(false);
        return;
      }
      if (lib.guid.startsWith("_cat:")) {
        try {
          const types = lib.guid.slice(5).split(",").filter(Boolean);
          const d = await invoke<{ list: MediaItem[]; total: number }>("get_items_global", {
            typeFilter: types,
            page: 1,
          });
          setItems(d.list ?? []);
          setTotal(d.total ?? 0);
          if (!d.list?.length) setErr("该分类暂无已识别条目");
        } catch (e) {
          setErr(String(e));
        }
        setLoading(false);
        return;
      }
      const list = await fetchLibItems(lib.guid, parentGuid);
      setItems(list);
      setTotal(list.length);
      if (list.length === 0) setErr("该库暂无已识别条目");
      setLoading(false);
    },
    [lib.guid],
  );

  useEffect(() => {
    load(null);
  }, [load]);

  async function openItem(item: MediaItem) {
    // 只有未识别文件夹（Directory）继续逐层钻取；
    // TV/Movie/Episode/Video 一律进详情页 —— 已识别剧集的「季/集」由详情页走
    // season/list + episode/list 获取，item/list 白名单里根本没有 Season/Episode。
    if (item.type !== "Directory") {
      onOpenItem(item);
      return;
    }
    if (current) setStack((s) => [...s.slice(0, -1), { folder: current, items }]);
    else setStack((s) => [...s, { folder: item, items }]);
    await load(item.guid);
  }

  async function pop() {
    const prev = stack[stack.length - 1];
    setStack((s) => s.slice(0, -1));
    setItems(prev.items);
  }

  return (
    <main className="content">
      <div className="page-head">
        <button className="back" onClick={stack.length > 0 ? pop : onBackHome}>
          ‹
        </button>
        <h3>{current ? current.title : lib.title}</h3>
        <span className="muted">{total || items.length} 项</span>
      </div>
      {err && <div className="err">{err}</div>}
      {loading && <div className="empty">加载中…</div>}
      {!loading && (
        <div className="grid">
          {items.map((it) => (
            <PosterCard key={it.guid} item={it} onOpen={openItem} />
          ))}
        </div>
      )}
      {!loading && items.length === 0 && !err && <div className="empty">暂无内容</div>}
    </main>
  );
}

/* ────────────────────────── 详情页（Kazumi 风：模糊海报头 + 信息卡 + 分段） ────────────────────────── */

/** [lc-118] genres 三形态归一：数组（[string] 或 [{name}]）/ 逗号串 —— 主仓库 images.ts lc-572 同款。
 *  item 详情的字段名各版本不一，按优先级多键兜底。 */
function pickGenres(d: Record<string, unknown> | null): string[] {
  const src = d ?? {};
  // [lc-1405] **不许再兜底到 `src.tags`**：飞牛 item 详情的 `tags` 是**元信息字典**
  //   （形如 {话数:"13", 放送开始:"…", 类型:[…]} 这类刮削字段），不是类型标签数组。
  //   之前把它当 genres 取，值里的 "13"（话数）就被渲染成一个标签 —— 用户看到的
  //   「标签数据错误显示成集数」正是这么来的。类型只认下面这几个真正的类型字段。
  const g = src.genres ?? src.genre ?? src.types ?? src.categories;
  if (Array.isArray(g)) {
    return g
      .map((x) =>
        typeof x === "string" ? x : String((x as Record<string, unknown> | null)?.name ?? ""),
      )
      .map((s) => s.trim())
      .filter(Boolean);
  }
  if (typeof g === "string" && g.trim()) {
    return g
      .split(/[,，/、|]/)
      .map((s) => s.trim())
      .filter(Boolean);
  }
  return [];
}

/** [lc-118] 分段栏标签（顺序即 tabIds 顺序）。
 *  [lc-1401] 按用户定稿版式改名：概览→详情、更多→演职人员，并新增「关联条目」。
 *  「选集」不再是标签页 —— 它按用户版式常驻右栏（每集小标题 + 简介的竖向列表）。 */
const TAB_LABEL: Record<string, string> = {
  // [lc-1413] 用户要求：标签名从「剧照剧集详情信息」缩短为「详情信息」
  overview: "详情信息",
  // [lc-1412] 原「吐槽评论」一个页拆成两页（用户要求"分开分别做成图片样式"），
  //   并把两页**交换位置**：评论（长影评）在前、吐槽（短评气泡）在后。
  reviews: "评论",
  comments: "吐槽",
  related: "关联条目",
  more: "演职人员",
};

/** [lc-119] 吐槽分页页大小（p1 上游默认 20；分页推进以**实际返回条数**为准，不假定 limit 生效） */
const BGM_PAGE = 20;

/** [lc-1412] Bangumi 时间戳 → 展示串。
 *  上游给的是 Unix 秒（comments.updatedAt / reviews.entry.createdAt）。
 *  规则：7 天内显示相对时间（气泡流的常规做法，用户设计稿就是 "1d 23h ago"），
 *  更早显示日期（同年不带年、跨年带年）。**解析不了就返回空串**（不显示比显示错的好）。 */
function fmtBgmTime(ts?: number | string | null): string {
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
function bgmList<T>(v: unknown): T[] {
  if (Array.isArray(v)) return v as T[];
  const d = (v as { data?: unknown } | null)?.data;
  return Array.isArray(d) ? (d as T[]) : [];
}

/** /characters 的 type 是分类（1 角色 / 2 机体 / 3 舰船 / 4 组织）；relation 才是主角/配角/客串 */
const CHAR_TYPE_LABEL: Record<number, string> = { 1: "角色", 2: "机体", 3: "舰船", 4: "组织" };

/** Bangumi 列表骨架屏（吐槽 / 更多 共用；与既有 .poster-skeleton 同风格；行宽错开避免机械感） */
function BgmListSkeleton() {
  return (
    <ul className="bgmc-list bgmc-skel" aria-hidden="true">
      {[0, 1, 2, 3].map((i) => (
        <li key={i} className="bgmc-item">
          <span className="bgmc-sk-avatar" />
          <div className="bgmc-main">
            <span className="bgmc-sk-line" style={{ width: i % 2 ? "40%" : "56%" }} />
            <span className="bgmc-sk-line" style={{ width: "94%" }} />
            <span className="bgmc-sk-line" style={{ width: i % 2 ? "72%" : "62%" }} />
          </div>
        </li>
      ))}
    </ul>
  );
}

/** Bangumi 角色/制作人员行（「更多」分段共用）：头像 + 名称 + 右侧角色/职位标签 + 可选副行（声优）。
 *  结构与「吐槽」列表同款（.bgmc-item 一族）；头像默认走 bgmImg（lain.bgm.tv 三级缓存代理），
 *  [lc-128] `img` 可传**已解析好的完整 URL**（TMDB 头像走 tmdbImg、飞牛兜底头像走 imgUrl），
 *  这样 TMDB / Bangumi / 飞牛三处演员表共用同一个展示组件，不新增一套。 */
function BgmCastRow({
  name,
  tag,
  sub,
  avatar,
  img,
  fallback,
}: {
  name?: string;
  tag?: string;
  sub?: string;
  avatar?: string;
  /** 已解析的完整图片 URL（优先于 avatar） */
  img?: string;
  fallback: string;
}) {
  const src = img || bgmImg(avatar);
  // [lc-1403] 图片加载失败 → 退回占位图标（否则 .bgmc-avatar 的圆形底色会留一个灰圆，
  // 在演员表里看着像"背景上有个大圆圈"）。onError 只触发一次，不重试、不打日志（避免刷屏）。
  const [failed, setFailed] = useState(false);
  const showImg = !!src && !failed;
  return (
    <li className="bgmc-item">
      {showImg ? (
        <img
          className="bgmc-avatar"
          src={src}
          alt=""
          loading="lazy"
          width={40}
          height={40}
          onError={() => setFailed(true)}
        />
      ) : (
        // [lc-1414] 兜底也用同一套配色（按名字取色），与吐槽页的 AvatarSafe 观感一致
        <span
          className="bgmc-avatar avatar-fallback"
          style={{ background: `hsl(${((): number => {
            const t = String(name || "");
            let h = 0;
            for (let i = 0; i < t.length; i++) h = (h * 31 + t.charCodeAt(i)) % 360;
            return h;
          })()} 42% 62%)` }}
          aria-hidden="true"
        >
          <IcoUserL size={15} strokeWidth={1.8} />
        </span>
      )}
      <div className="bgmc-main">
        <div className="bgmc-head">
          <span className="bgmc-nick">{name || fallback}</span>
          {tag && <span className="bgmc-role">{tag}</span>}
        </div>
        {sub && <div className="bgmc-sub">{sub}</div>}
      </div>
    </li>
  );
}

/** [lc-128] 飞牛侧演员表 —— **只做兜底**（TMDB 拿不到时才用）。飞牛的 item 详情是否带演员字段
 *  依版本/刮削源而定：有就用（兼容字符串数组与对象数组、多种键名），**没有就是空数组**，
 *  不报错、不渲染空区块。 */
function fnosCast(d: Record<string, unknown> | null): { name: string; role: string; avatar: string }[] {
  if (!d) return [];
  const str = (o: Record<string, unknown>, keys: string[]): string => {
    for (const k of keys) {
      const v = o[k];
      if (typeof v === "string" && v.trim()) return v.trim();
    }
    return "";
  };
  const raw = d.actors ?? d.cast ?? d.persons ?? d.actor_list ?? d.actors_list;
  const arr: unknown[] = Array.isArray(raw)
    ? raw
    : typeof raw === "string" && raw.trim()
      ? raw.split(/[,，、/]/).map((s) => s.trim()).filter(Boolean)
      : [];
  const out: { name: string; role: string; avatar: string }[] = [];
  for (const it of arr) {
    if (typeof it === "string") {
      if (it.trim()) out.push({ name: it.trim(), role: "", avatar: "" });
      continue;
    }
    const o = it as Record<string, unknown> | null;
    if (!o || typeof o !== "object") continue;
    const name = str(o, ["name", "actor", "actor_name", "person_name", "title"]);
    if (!name) continue;
    const role = str(o, ["role", "character", "character_name", "roles"]);
    let avatar = str(o, ["avatar", "image", "head", "profile", "poster", "url", "path"]);
    if (!avatar) {
      const imgs = o.images;
      if (typeof imgs === "string") avatar = imgs.trim();
      else if (imgs && typeof imgs === "object") {
        const im = imgs as Record<string, unknown>;
        avatar = str(im, ["large", "grid", "medium", "small", "url"]);
      }
    }
    out.push({ name, role, avatar });
  }
  return out.slice(0, 24);
}

/* ══════════════════════ [lc-123] 发现页（左 TMDB / 右 Bangumi） ══════════════════════
   取数**一律走 Rust 命令**（`tmdb_discover` / `bgm_calendar`）：镜像要准入签名，WebView 里签不了，
   所以 src/ 下**不允许**出现直连 bgm.690075.xyz / tmdb.690075.xyz 的 fetch。图片也不再直连图片域：
   [lc-128] 统一走 fntvimg 自定义协议的 `/__tmdb/*`、`/__bgm/*`（Rust 反代 + **落盘三级缓存**）。
   两栏各有**自己的滚动容器**（左栏滚到第五屏，右栏位置不能跟着跳）与**各自**的骨架屏/失败重试/空态。
   数据只在**首次激活**时拉，切回首页再回来不重拉（与 HomeView 常驻挂载同一套理由）。 */

/** 周几（1=周一 .. 7=周日）—— 与 Bangumi `weekday.id` / `air_weekday` **同一套编号**，
 *  对齐一律按这个 id，**不按数组下标猜**（上游给的是 7 组数组，顺序变了也不会串天）。 */
const WEEKDAYS: { id: number; cn: string }[] = [
  { id: 1, cn: "周一" },
  { id: 2, cn: "周二" },
  { id: 3, cn: "周三" },
  { id: 4, cn: "周四" },
  { id: 5, cn: "周五" },
  { id: 6, cn: "周六" },
  { id: 7, cn: "周日" },
];

/** 本机时区的今天是周几（1..7）；右栏默认选中它、并在胶囊上打"今天"点 */
function localWeekdayId(): number {
  const d = new Date().getDay(); // 0=周日
  return d === 0 ? 7 : d;
}

/** [lc-123-rev5] 本机日期 YYYY-MM-DD：日历缓存的**跨天分桶键**
 *  （本地零点一过就换键 ⇒ 必然重取；不会把昨天的表当今天用）。 */
function localDay(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** 发现页骨架块（两栏共用；shimmer 语言对齐已有的 .poster-skeleton / .bgmc-sk-*） */
function Dsk({ cls }: { cls: string }) {
  return <div className={`dsk ${cls}`} aria-hidden="true" />;
}

/* ───────────────────────── 左栏：TMDB 当季热门 + 即将播出 ───────────────────────── */

/** [lc-123-rev] 左栏「即将播出」的追加页响应（Rust tmdb_discover 带 total/totalPages 回传）。 */
type TmdbUpResp = { items?: TmdbDiscoverItem[]; total?: number; totalPages?: number };

/** 「加载更多」的页数上限：与 Rust `tmdb_discover` 的 `page.clamp(1,25)` 对齐（20 条/页 × 25 = 500） */
const UP_MAX_PAGE = 25;

/** 失败重试 / 空态（两块共用；骨架由各块自己的形状渲染） */
function DState({ err, emptyText, onRetry }: { err: string; emptyText: string; onRetry: () => void }) {
  if (err) {
    return (
      <div className="dstate">
        <div className="dstate-msg">{err}</div>
        <button className="dretry" onClick={onRetry}>
          重试
        </button>
      </div>
    );
  }
  return <div className="dstate">{emptyText}</div>;
}

/** [lc-123-rev9] 主推区（**用户权威设计稿**：紧凑左右分栏 —— 左竖版海报 140×210，右信息列：
 *  大标题 / 评分徽章+年份·状态·N 人评分一行 / 3 行简介 / 「查看详情」按钮）。**没有大 backdrop、
 *  没有文字压图** —— 文字永远在实底信息列里。主推数据 = 当季第 1 条（热度第一），**不轮播不翻页**。
 *  海报 = poster（2:3，w500）；缺图走占位块（Film 图标），不破格。
 *  「查看详情」：TMDB 条目没有飞牛 guid —— [lc-1444] 改为**直接进主详情页**
 *  （复用 DetailView，选集信息占位），不再走 bgm_match + 轻量弹层的旧链路。 */
function TmdbFeatured({
  it,
  isMovie,
  onOpenDetail,
}: {
  it: TmdbDiscoverItem;
  isMovie: boolean;
  onOpenDetail: (it: TmdbDiscoverItem) => void;
}) {
  const state = it.state === "upcoming" ? "未开播" : it.state === "airing" ? "已开播" : "";
  return (
    <div className="dfeat">
      <div className="dfeat-poster">
        <Dsk cls="dsk-poster" />
        {it.poster ? (
          <img src={tmdbImg(it.poster, "w500")} alt="" draggable={false} />
        ) : (
          <span className="dfeat-poster-empty" aria-hidden="true">
            <IcoFilmL size={28} strokeWidth={1.8} />
          </span>
        )}
      </div>
      <div className="dfeat-info">
        <div className="dfeat-title">{it.title}</div>
        <div className="dfeat-meta">
          {it.rating > 0 && (
            <span className="dfeat-score">
              <IcoStarL size={14} strokeWidth={2} className="rating-ico fill" /> {it.rating.toFixed(1)}
            </span>
          )}
          <span>{it.year || "—"}</span>
          {state && <span>· {state}</span>}
          {it.votes > 0 && <span>· {it.votes.toLocaleString("zh-CN")} 人评分</span>}
          <span className="dfeat-kind">{isMovie ? "电影" : "剧集"}</span>
        </div>
        {it.overview ? <p className="dfeat-desc">{it.overview}</p> : null}
        <button className="dfeat-btn" onClick={() => onOpenDetail(it)}>
          查看详情
        </button>
      </div>
    </div>
  );
}

/** [lc-123-rev9] 标准海报卡（设计稿 .tmdb-card）：**2:3 竖版海报**（不再裁 16:9）+
 *  右上评分角标 + 下方两行（标题 / 年份·状态 pill）。`minmax(140px,1fr)` 自适应 2~4 列。
 *  缺 poster → 占位块（Film 图标），不破格。 */
function TmdbPC({ it, onOpen }: { it: TmdbDiscoverItem; onOpen: (it: TmdbDiscoverItem) => void }) {
  const tip = [it.title, it.overview].filter(Boolean).join("\n\n");
  const state = it.state === "upcoming" ? "未开播" : it.state === "airing" ? "已开播" : "";
  return (
    <div className="dpc" title={tip} onClick={() => onOpen(it)}>
      <div className="dpc-img">
        <Dsk cls="dsk-poster" />
        {it.poster ? (
          <img src={tmdbImg(it.poster, "w342")} alt="" loading="lazy" draggable={false} />
        ) : (
          <span className="dpc-empty" aria-hidden="true">
            <IcoFilmL size={22} strokeWidth={1.8} />
          </span>
        )}
        {it.rating > 0 && <span className="dpc-rate">{it.rating.toFixed(1)}</span>}
      </div>
      <div className="dpc-bd">
        <div className="dpc-t">{it.title}</div>
        <div className="dpc-s">
          <span>{it.year || "—"}</span>
          {state && <span className="dpill">{state}</span>}
        </div>
      </div>
    </div>
  );
}

/** [lc-123-rev9] 新形状骨架：主推分栏 + 海报网格卡（与真内容一致，避免加载完成跳动） */
function TmdbSeasonSkeleton() {
  return (
    <>
      <div className="dfeat dfeat-sk">
        <div className="dfeat-poster">
          <Dsk cls="dsk-poster" />
        </div>
        <div className="dfeat-info">
          <Dsk cls="dsk-line" />
          <Dsk cls="dsk-line short" />
          <Dsk cls="dsk-line" />
          <Dsk cls="dsk-line short" />
        </div>
      </div>
      <div className="dpcgrid">
        {[0, 1, 2, 3, 4, 5].map((i) => (
          <div key={i} className="dpc">
            <div className="dpc-img">
              <Dsk cls="dsk-poster" />
            </div>
            <div className="dpc-bd">
              <Dsk cls="dsk-line" />
              <Dsk cls="dsk-line short" />
            </div>
          </div>
        ))}
      </div>
    </>
  );
}

/** [lc-123-rev] 「即将播出」紧凑行：日期徽章 + 一行标题 + 类型小标签（纯展示，无跳转）。 */
function UpRow({ it, isMovie }: { it: TmdbDiscoverItem; isMovie: boolean }) {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(it.date || "");
  const tip = [it.title, it.date, it.overview].filter(Boolean).join("\n\n");
  return (
    <li className="dup" title={tip}>
      <span className="dup-d">
        {m ? (
          <>
            <b>{String(Number(m[3]))}</b>
            {String(Number(m[2]))} 月
          </>
        ) : (
          "待定"
        )}
      </span>
      <span className="dup-t">{it.title}</span>
      <span className="dpill">{isMovie ? "电影" : "剧集"}</span>
    </li>
  );
}

/** 左栏：TMDB。`active` = 首次激活闸门（懒加载）。取数走 tmdb_discover（一条命令带 mode 参数）。 */
function TmdbPane({
  active,
  onOpenDetail,
}: {
  active: boolean;
  /** [lc-1444] 「查看详情」→ 直接进主详情页（复用 DetailView；父级合成 MediaItem） */
  onOpenDetail: (it: TmdbDiscoverItem, mediaType: "tv" | "movie") => void;
}) {
  const [mediaType, setMediaType] = useState<"tv" | "movie">("tv");
  const [season, setSeason] = useState<"current" | "next">("current");
  const [items, setItems] = useState<TmdbDiscoverItem[]>([]);
  const [range, setRange] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [upcoming, setUpcoming] = useState<TmdbDiscoverItem[]>([]);
  const [upBusy, setUpBusy] = useState(false);
  const [upErr, setUpErr] = useState("");
  const [upMoreErr, setUpMoreErr] = useState(""); // 追加页失败：只提示，不动已显示的列表
  const [upPage, setUpPage] = useState(1); // 已载入页数（1..UP_MAX_PAGE）
  const [upTotal, setUpTotal] = useState(0); // 上游总条数（0=未知）
  const [upTotalPages, setUpTotalPages] = useState(0);
  const [upMoreBusy, setUpMoreBusy] = useState(false);
  const [nonce, setNonce] = useState(0); // 两块共用的**失败重试**触发器（force + noEtag：强制全量）
  const [probe, setProbe] = useState(0); // [lc-123-rev5] 10 分钟后台探测（force=false：命令层先读缓存判 TTL）
  // 失败重试与"到期探测"走**不同**的 force 语义：重试 = force + noEtag（强制全量，别拿坏缓存反复撞）
  const retryRef = useRef(false);
  const hardRetry = useCallback(() => {
    retryRef.current = true;
    setNonce((n) => n + 1);
  }, []);
  // changed=false（缓存命中/304/内容未变）时**不碰 UI**：用 ref 记"已经渲染过哪一份数据"
  const appliedKeyRef = useRef("");
  const itemsRef = useRef<TmdbDiscoverItem[]>([]);
  const upCountRef = useRef(0);
  useEffect(() => {
    itemsRef.current = items;
  }, [items]);
  useEffect(() => {
    upCountRef.current = upcoming.length;
  }, [upcoming]);

  // 当季/下季（切换 剧集/电影 或 本季/下季 → 重取；active 才取）
  useEffect(() => {
    if (!active) return;
    let alive = true;
    setBusy(true);
    setErr("");
    const hard = retryRef.current; // 失败重试：绕过本地缓存且**不带 ETag**（强制全量）
    retryRef.current = false;
    invoke<{ items?: TmdbDiscoverItem[]; gte?: string; lte?: string; changed?: boolean }>(
      "tmdb_discover",
      // 常规（进页面/10 分钟探测）：force=false ⇒ 命令层「先读 12h 磁盘缓存 → 未过期直接返回（零网络）
      // → 过期才带 ETag 探测（304 就只顺延时间戳）」
      { arg: { mode: "season", mediaType, season, force: hard, noEtag: hard } },
    )
      .then((d) => {
        if (!alive) return;
        const key = `${mediaType}|${season}`;
        // changed=false（缓存命中 / 304 / 200 但内容未变）且这份数据已经渲染过 → **一个 state 都不碰**
        // （零 re-render、滚动位置不动）。只有 内容变了 / 首次 / 换了 key 才落 state。
        if (d?.changed === false && appliedKeyRef.current === key && itemsRef.current.length) return;
        appliedKeyRef.current = key;
        setItems(Array.isArray(d?.items) ? d.items : []);
        setRange(d?.gte && d?.lte ? `${d.gte} ~ ${d.lte}` : "");
      })
      .catch((e) => {
        if (alive) setErr(String(e));
      })
      .finally(() => {
        if (alive) setBusy(false);
      });
    return () => {
      alive = false;
    };
  }, [active, mediaType, season, nonce, probe]);

  // [lc-123-rev5] 后台探测：每 10 分钟一次 —— **304 极便宜**（未变只传几十字节），
  // 且命令层只在 TTL 过期时才真发请求；未变时 changed=false ⇒ 上面那个分支一个 state 都不碰。
  // 只探当季列表：即将播出是"加载更多"累加出来的列表（用户翻过页就不该被后台缩回去）。
  useEffect(() => {
    if (!active) return;
    const t = window.setInterval(() => setProbe((n) => n + 1), 10 * 60 * 1000);
    return () => window.clearInterval(t);
  }, [active]);

  // 即将播出（同一 mediaType；**不带季度窗口**）：先取第一页，后续页由「加载更多」追
  useEffect(() => {
    if (!active) return;
    let alive = true;
    setUpBusy(true);
    setUpErr("");
    setUpMoreErr("");
    setUpPage(1);
    const hard = retryRef.current;
    retryRef.current = false;
    invoke<TmdbUpResp & { changed?: boolean }>("tmdb_discover", {
      // 常规 force=false：先读磁盘缓存（即将播出 TTL 24h）；失败重试才 force+noEtag 强制全量
      arg: { mode: "upcoming", mediaType, page: 1, force: hard, noEtag: hard },
    })
      .then((d) => {
        if (!alive) return;
        if (d?.changed === false && upCountRef.current > 0) return; // 内容没变 → 不碰 UI
        setUpcoming(Array.isArray(d?.items) ? d.items : []);
        setUpTotal(typeof d?.total === "number" ? d.total : 0);
        setUpTotalPages(typeof d?.totalPages === "number" ? d.totalPages : 0);
      })
      .catch((e) => {
        if (alive) setUpErr(String(e));
      })
      .finally(() => {
        if (alive) setUpBusy(false);
      });
    return () => {
      alive = false;
    };
  }, [active, mediaType, nonce]);

  const featured = items[0]; // 设计稿：主推 = 热度第一，无轮播
  const gridItems = items.slice(1);
  const upLastPage = Math.min(upTotalPages > 0 ? upTotalPages : UP_MAX_PAGE, UP_MAX_PAGE);
  const upMoreAvail =
    upcoming.length > 0 && upPage < upLastPage && (upTotal === 0 || upcoming.length < upTotal);

  // 「加载更多」：**只追加**、不重置已显示内容（先出列表，追加失败只在尾部提示）
  async function loadMoreUpcoming() {
    if (upMoreBusy) return;
    const next = upPage + 1;
    setUpMoreBusy(true);
    setUpMoreErr("");
    try {
      const d = await invoke<TmdbUpResp>("tmdb_discover", {
        arg: { mode: "upcoming", mediaType, page: next, force: false },
      });
      const more = Array.isArray(d?.items) ? d.items : [];
      setUpcoming((prev) => {
        const seen = new Set(prev.map((x) => `${x.tmdbId}-${x.date}`));
        return [...prev, ...more.filter((x) => !seen.has(`${x.tmdbId}-${x.date}`))];
      });
      setUpPage(next);
      if (typeof d?.total === "number" && d.total > 0) setUpTotal(d.total);
      if (typeof d?.totalPages === "number" && d.totalPages > 0) setUpTotalPages(d.totalPages);
    } catch (e) {
      setUpMoreErr(String(e));
    } finally {
      setUpMoreBusy(false);
    }
  }

  // [lc-1444] 「查看详情」：**直接进主详情页**（复用 DetailView，选集信息占位）。
  //   TMDB 条目没有飞牛 guid —— 由父级合成 MediaItem（guid 用 discover-<tmdbId> 占位标识）。
  function openDetail(it: TmdbDiscoverItem) {
    onOpenDetail(it, mediaType);
  }

  const isMovie = mediaType === "movie";
  return (
    <section className="dpane">
      <header className="dpane-hd">
        <h2>{isMovie ? "当季热门电影" : "当季热门剧集"}</h2>
        <span className="dn">{items.length} 部</span>
        <span className="dsp" />
        <div className="dchips">
          <button className={`dchip${!isMovie ? " on" : ""}`} onClick={() => setMediaType("tv")}>
            剧集
          </button>
          <button className={`dchip${isMovie ? " on" : ""}`} onClick={() => setMediaType("movie")}>
            电影
          </button>
          <button
            className={`dchip${season === "current" ? " on" : ""}`}
            onClick={() => setSeason("current")}
          >
            本季
          </button>
          <button className={`dchip${season === "next" ? " on" : ""}`} onClick={() => setSeason("next")}>
            下季
          </button>
        </div>
      </header>
      <div className="dscroll">
        {range && (
          <div className="dcap">
            {season === "next" ? "下季" : "本季"} {range} · 共 {items.length} 部
          </div>
        )}
        {busy && !items.length ? (
          <TmdbSeasonSkeleton />
        ) : err && !items.length ? (
          <DState err={err} emptyText="" onRetry={hardRetry} />
        ) : !items.length ? (
          <DState
            err=""
            emptyText={`${season === "next" ? "下季" : "本季"}暂无数据`}
            onRetry={hardRetry}
          />
        ) : (
          <>
            {featured && (
              <TmdbFeatured
                it={featured}
                isMovie={isMovie}
                onOpenDetail={openDetail}
              />
            )}
            {gridItems.length > 0 && (
              <>
                <h3 className="dsub-h">
                  {season === "next" ? "下季看点" : "本季新剧"}
                  <span>按热度排序</span>
                </h3>
                <div className="dpcgrid">
                  {gridItems.map((it) => (
                    <TmdbPC key={`${it.tmdbId}-${it.date}`} it={it} onOpen={openDetail} />
                  ))}
                </div>
              </>
            )}
          </>
        )}
        <h3 className="dsub-h">
          {isMovie ? "即将上映" : "即将播出"}
          <span>按首播日排序</span>
        </h3>
        {upBusy && !upcoming.length ? (
          <ul className="duplist">
            {[0, 1, 2, 3].map((i) => (
              <li key={i} className="dup">
                <Dsk cls="dsk-date" />
                <div className="dup-t">
                  <Dsk cls="dsk-line" />
                </div>
              </li>
            ))}
          </ul>
        ) : upErr && !upcoming.length ? (
          <DState err={upErr} emptyText="" onRetry={hardRetry} />
        ) : !upcoming.length ? (
          <div className="dstate">暂无即将播出的条目</div>
        ) : (
          <>
            <ul className="duplist">
              {upcoming.map((it) => (
                <UpRow key={`${it.tmdbId}-${it.date}`} it={it} isMovie={isMovie} />
              ))}
            </ul>
            {/* [lc-1416] 按钮类名 .dmore → **.dmore-btn**：原来它与「演职人员」页的**容器** .dmore 撞名，
                而 `.dmore` 那条选择器同样命中那个 block 级、撑满宽度的 div →
                容器套上了胶囊样式（border-radius:999px：上下半圆 + 中间矩形）与 :hover 高亮。
                用户看到的"背景有个大椭圆、鼠标放上去还高亮"就是这个**类名冲突**，不是几何绘制。 */}
            {upMoreAvail && (
              <button className="dmore-btn" onClick={loadMoreUpcoming} disabled={upMoreBusy}>
                {upMoreBusy ? "加载中…" : "加载更多"}
              </button>
            )}
            {/* 上游页数被后端夹在上限内：这里如实说明"只显示到第 N 部"，不谎称"已全部加载" */}
            {!upMoreAvail && upPage >= upLastPage && upTotal > upcoming.length && (
              <div className="dup-hint">
                已显示前 {upcoming.length} 部 · 共 {upTotal} 部
              </div>
            )}
            {upMoreErr && <div className="dup-hint">加载失败：{upMoreErr}</div>}
          </>
        )}
      </div>
    </section>
  );
}

/* ───────────────────────── 右栏：Bangumi 每日新番 ───────────────────────── */

/* ── [lc-123-rev] 在库匹配（best-effort；宁可少标，绝不谎报） ──
   取数方式：复用**已有的** get_items_global（typeFilter 白名单 Movie/TV，与主仓库「库索引」同一口径），
   按页拉标题集合。为什么建索引：本仓库**没有**"按标题查库"的命令（fnapi.rs 只有 item/list 等既有端点），
   逐条网络匹配（一天 10~26 条、一周 105 条）会把 NAS 打爆 —— 一次性分页索引是唯一省的方式。
   诚实边界：索引**分页到底**才算完整 ⇒ "ok"（可分组：在库/不在库）；
   拉满上限还没到底 ⇒ "partial"（只给命中项打「已在库」标，**不分组、不标"不在库"**）；
   拉失败或空库 ⇒ "none"（整个分组退场）。三种状态下列表都照常渲染（匹配在后台补，不阻塞）。 */
const LIB_INDEX_PAGE = 60; // get_items_global 的 page_size 在 Rust 侧固定 60
const LIB_INDEX_MAX_PAGES = 8; // 上限 480 部；不足一页才算"到底"
const LIB_WHITELIST_TYPES = ["Movie", "TV"]; // 只要已识别的电影/剧集（排除文件夹/未识别视频）

/** 归一化标题：NFKC（全角→半角）+ 小写 + 去掉空白/标点/符号（只留字母数字与汉字） */
function normTitle(raw?: string | null): string {
  return String(raw ?? "")
    .normalize("NFKC")
    .toLowerCase()
    .replace(/[\s\p{P}\p{S}]/gu, "");
}

/** 标题是否命中库索引：先全等，再前缀兜底（「我推的孩子」↔「我推的孩子 第三季」这类季/篇后缀差异） */
function libHasTitle(titles: Set<string>, raw?: string | null): boolean {
  const n = normTitle(raw);
  if (n.length < 2) return false;
  if (titles.has(n)) return true;
  for (const t of titles) {
    if (t.length >= 3 && n.length >= 3 && (t.startsWith(n) || n.startsWith(t))) return true;
  }
  return false;
}

type LibIndexState = "loading" | "ok" | "partial" | "none";

/** 会话级缓存：拉到的索引切页回来不再重拉（最多 8 次请求只花一次；失败不缓存，下次进来可重试） */
let libIndexCache: { state: LibIndexState; titles: Set<string> } | null = null;

/** 库标题索引（active 即拉，与日历并行；不阻塞列表渲染） */
function useLibIndex(active: boolean): { state: LibIndexState; titles: Set<string> } {
  const [idx, setIdx] = useState<{ state: LibIndexState; titles: Set<string> }>(
    () => libIndexCache ?? { state: "loading", titles: new Set() },
  );
  useEffect(() => {
    if (!active || libIndexCache) return;
    let alive = true;
    (async () => {
      const titles = new Set<string>();
      let complete = false;
      try {
        for (let p = 1; p <= LIB_INDEX_MAX_PAGES; p++) {
          const d = await invoke<{ list?: MediaItem[]; total?: number }>("get_items_global", {
            typeFilter: LIB_WHITELIST_TYPES,
            page: p,
          });
          const list = Array.isArray(d?.list) ? d.list : [];
          for (const it of list) {
            const n = normTitle(it?.title);
            if (n) titles.add(n);
          }
          if (list.length < LIB_INDEX_PAGE) {
            complete = true; // 不足一页 = 已到底（空页也是）
            break;
          }
        }
      } catch {
        if (alive) setIdx({ state: "none", titles: new Set() }); // 失败不写缓存（下次进来重试）
        return;
      }
      if (titles.size === 0) {
        if (alive) setIdx({ state: "none", titles: new Set() });
        return;
      }
      const next = { state: (complete ? "ok" : "partial") as LibIndexState, titles };
      libIndexCache = next;
      if (alive) setIdx(next);
    })();
    return () => {
      alive = false;
    };
  }, [active]);
  return idx;
}

/** [lc-123-rev] 单个更新条目行：左侧强调竖条 + 封面(46×62，左上角该天序号) + 标题 +
 *  评分色阶标签(≥8 绿 / 7–8 黄 / <7 灰；无评分不显示) + 年份 + 今日更新标。
 *  `/calendar` **没有集号与播出时刻**（只有 air_date 首播日 + air_weekday），所以这里
 *  刻意**不显示**「第 N 话 / 今晚 22:30」——那两列无数据可依，宁可不要也不编。 */
function BgmCalRow({
  it,
  today,
  rank,
  libState,
  onOpen,
}: {
  it: BgmCalendarItem;
  today: boolean;
  /** 该天列表内的序号（1 起，按上游顺序，就是封面左上角那个角标） */
  rank: number;
  /** "in" = 已匹配到飞牛库（打「已在库」标）；null = 未匹配或索引不可用（不标） */
  libState: "in" | null;
  onOpen: (it: BgmCalendarItem) => void;
}) {
  const title = it.name_cn || it.name || "（无标题）";
  const year = (it.air_date || "").slice(0, 4);
  const score = typeof it.score === "number" && isFinite(it.score) && it.score > 0 ? it.score : null;
  const tier = score == null ? null : score >= 8 ? "hi" : score >= 7 ? "mid" : "lo";
  const tip = [title, year ? `${year} 年` : "", score != null ? `评分 ${score.toFixed(1)}` : ""]
    .filter(Boolean)
    .join(" · ");
  return (
    <li
      className="ditem"
      role="button"
      tabIndex={0}
      title={`${tip} · 点击查看 Bangumi 详情`}
      onClick={() => onOpen(it)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen(it);
        }
      }}
    >
      <div className={`dcv${it.cover ? "" : " empty"}`}>
        {it.cover ? (
          <img src={bgmImg(it.cover)} alt="" loading="lazy" width={46} height={62} draggable={false} />
        ) : (
          <span aria-hidden="true"><IcoFilmL size={18} strokeWidth={1.8} /></span>
        )}
        <span className="drk" aria-hidden="true">
          {rank}
        </span>
      </div>
      <div className="dtx">
        <div className="dt1">{title}</div>
        <div className="dt2">
          {tier && score != null && <span className={`dsc ${tier}`}>{score.toFixed(1)}</span>}
          <span>{year ? `${year} 年` : "年份未知"}</span>
          {libState === "in" && <span className="dlib">已在库</span>}
          {today && <span className="ditem-tag">今日更新</span>}
        </div>
      </div>
    </li>
  );
}

/** 右栏：Bangumi 新番更新（7 个星期胶囊<迷你占比条> + 当天汇总条 + 当日列表）。
 *  点击条目 → 打开该 Bangumi 条目的轻量详情。
 *  [lc-123-rev] 在库分组：只对**当前选中这天**的条目做匹配（不逐条打网络），在库在前、组标题带数量；
 *  索引不完整（partial）时退化成"只给命中项打标"，索引不可用时整个分组退场（见 useLibIndex 注释）。 */
/* ════════════ [lc-1516] 发现页右栏顶部：Bangumi 追番同步卡 ════════════
   版式按用户设计稿：左侧 = 图标 +「Bangumi / 追番同步」标题组 + 描述 + 状态枚举
   + 大按钮（连接 / 立即同步）；右侧 = 观看统计（追番状态 donut + 分项计数）。
   数据源：Rust bgm_account 模块（走自建镜像，见 bgm_account.rs）。 */
type BgmAccountState = { bound: boolean; username: string };
type BgmCollectStats = { total: number; byType: Record<string, number> };
/** [lc-1534] 「正在追」封面条条目（Rust bgm_watching_covers：本地表 type=3 按最近更新倒序） */
type BgmWatchCover = { id: number; name: string; cover: string };

/** 追番状态五元组（Bangumi type 值 / 标签 / 统计配色） */
const BGAC_TYPES: { k: number; label: string; color: string }[] = [
  { k: 1, label: "想看", color: "#E9A23C" },
  { k: 3, label: "在看", color: "#37A75F" },
  { k: 2, label: "看过", color: "#4A87F0" },
  { k: 4, label: "搁置", color: "#9AA1A8" },
  { k: 5, label: "放弃", color: "#ED5B52" },
];

function BgmAccountCard({ onOpen }: { onOpen?: (it: BgmCalendarItem) => void }) {
  const [acct, setAcct] = useState<BgmAccountState>({ bound: false, username: "" });
  const [stats, setStats] = useState<BgmCollectStats>({ total: 0, byType: {} });
  const [watch, setWatch] = useState<BgmWatchCover[]>([]); // [lc-1534] 「正在追」封面条
  const [season, setSeason] = useState<{ total: number; seasonTotal: number; byType: Record<string, number> }>(
    { total: 0, seasonTotal: 0, byType: {} },
  );
  const [bindOpen, setBindOpen] = useState(false); // 连接表单展开态
  // [lc-1528] 同步状态图联动高亮：null = 无；数字 = BGAC_TYPES 下标
  const [hl, setHl] = useState<number | null>(null);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [syncBusy, setSyncBusy] = useState(false);
  const [err, setErr] = useState("");
  const [note, setNote] = useState("");

  const refresh = useCallback(async () => {
    try {
      const s = await invoke<BgmAccountState>("bgm_account_status");
      setAcct(s);
    } catch {
      /* 状态查询失败不打扰用户 */
    }
    try {
      const st = await invoke<BgmCollectStats>("bgm_collect_stats");
      setStats(st);
    } catch {
      /* ignore */
    }
    try {
      const ss = await invoke<{ total: number; seasonTotal: number; byType: Record<string, number> }>(
        "bgm_collect_season",
      );
      setSeason(ss);
    } catch {
      /* ignore */
    }
    // [lc-1534] 「正在追」封面条（本地优先；缺封面时命令层会现拉一次并回填）
    try {
      const wc = await invoke<{ items: BgmWatchCover[] }>("bgm_watching_covers", { limit: 12 });
      setWatch(Array.isArray(wc?.items) ? wc.items : []);
    } catch {
      /* ignore */
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const bind = async () => {
    const t = token.trim();
    if (!t) return;
    setBusy(true);
    setErr("");
    setNote("");
    try {
      const s = await invoke<BgmAccountState>("bgm_account_bind", { token: t });
      setAcct(s);
      setToken("");
      setBindOpen(false);
      setNote(`已连接 Bangumi 账号「${s.username}」`);
      refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const unbind = async () => {
    if (!window.confirm(`解绑 Bangumi 账号「${acct.username}」？本地追番状态会保留。`)) return;
    setErr("");
    setNote("");
    try {
      await invoke("bgm_account_unbind");
      setAcct({ bound: false, username: "" });
      setNote("已解绑");
    } catch (e) {
      setErr(String(e));
    }
  };

  const syncNow = async () => {
    setSyncBusy(true);
    setErr("");
    setNote("");
    try {
      const r = await invoke<{ uploaded: number; downloaded: number; remoteTotal: number }>(
        "bgm_sync_now",
        { priority: "local" },
      );
      setNote(`同步完成 · 下载 ${r.downloaded} · 上传 ${r.uploaded}`);
      refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setSyncBusy(false);
    }
  };

  // ── 右侧观看统计：donut（五状态占比）+ 分项计数 ──
  const total = Math.max(0, Number(stats.total ?? 0));
  const counts = (k: number) => Number(stats.byType?.[String(k)] ?? 0);
  let acc = 0;
  const segs = BGAC_TYPES.map((t) => {
    const n = counts(t.k);
    const frac = total > 0 ? n / total : 0;
    const seg = { ...t, n, frac, a0: acc, a1: acc + frac };
    acc += frac;
    return seg;
  });

  // [lc-1528] 同步状态图几何（按用户设计稿重算，适配对半列宽；viewBox 200×150）
  const VB_W = 206; // 右侧多留 6 单位给胶囊右缘内边距（数字不再贴边）
  const VB_H = 150;
  const CX = 58;
  const CY = 86;
  const R_OUT = 46;
  const R_IN = 32;
  const R_LEADER = 58;
  const ROW_GAP = 26;
  const DOT_X = 126;
  const LABEL_X = 140;
  const VALUE_X = 194;
  const ROW_BG_X = DOT_X - 8; // 胶囊左缘（用户要求：胶囊背景矩形整体右移，贴近圆点）
  const LEADER_END_X = ROW_BG_X - 2; // 引线终点停在胶囊左缘外 2px，可见段更长
  const ROW_N = BGAC_TYPES.length;
  const rowY = (i: number) => CY - ((ROW_N - 1) * ROW_GAP) / 2 + i * ROW_GAP;
  const pt = (r: number, deg: number) => {
    const rad = (deg * Math.PI) / 180;
    return [CX + r * Math.sin(rad), CY - r * Math.cos(rad)] as const;
  };
  /** 环形扇形路径（外弧正向 + 内弧反向） */
  const sectorPath = (a0: number, a1: number) => {
    const d0 = -90 + a0 * 360;
    const d1 = -90 + a1 * 360;
    const [x0, y0] = pt(R_OUT, d0);
    const [x1, y1] = pt(R_OUT, d1);
    const [x2, y2] = pt(R_IN, d1);
    const [x3, y3] = pt(R_IN, d0);
    const large = d1 - d0 > 180 ? 1 : 0;
    return `M${x0.toFixed(2)},${y0.toFixed(2)}A${R_OUT},${R_OUT} 0 ${large} 1 ${x1.toFixed(2)},${y1.toFixed(2)}L${x2.toFixed(2)},${y2.toFixed(2)}A${R_IN},${R_IN} 0 ${large} 0 ${x3.toFixed(2)},${y3.toFixed(2)}Z`;
  };
  /** 引线：扇形边缘 → 沿 R_LEADER 圆弧绕到图例行方向 → 直线连到行（不穿过环本体） */
  const leaderPath = (mid: number, ry: number) => {
    const [x0, y0] = pt(R_OUT + 2, -90 + mid * 360);
    const [x1, y1] = pt(R_LEADER, -90 + mid * 360);
    const dx = LEADER_END_X - CX;
    const dy = ry - CY;
    const phi = (Math.atan2(dx, -dy) * 180) / Math.PI;
    const [x2, y2] = pt(R_LEADER, phi);
    const d = ((phi - (-90 + mid * 360) + 540) % 360) - 180;
    const large = Math.abs(d) > 180 ? 1 : 0;
    const sweep = d > 0 ? 1 : 0;
    return `M${x0.toFixed(2)},${y0.toFixed(2)}L${x1.toFixed(2)},${y1.toFixed(2)}A${R_LEADER},${R_LEADER} 0 ${large} ${sweep} ${x2.toFixed(2)},${y2.toFixed(2)}L${LEADER_END_X},${ry.toFixed(2)}`;
  };

  return (
    <div className={`bgmac${acct.bound ? " on" : ""}`}>
      {/* 左侧：标题组 + 按钮（图标按用户要求删除） */}
      <div className="bgmac-main">
        <div className="bgmac-text">
          <div className="bgmac-kicker">Bangumi</div>
          <div className="bgmac-title-row">
            <span className="bgmac-title">追番同步</span>
            <span className={`bgmac-badge${acct.bound ? " on" : ""}`} title={acct.bound ? `已连接：${acct.username}` : "未连接 Bangumi 账号"}>
              {acct.bound ? (
                <>
                  <svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M4 12.5 9 17.5 20 6.5" />
                  </svg>
                  <span className="bgmac-badge-name">{acct.username}</span>
                </>
              ) : (
                "未连接"
              )}
            </span>
          </div>
          <div className="bgmac-desc">与 Bangumi 保持相同的追番状态。</div>
          <div className="bgmac-types">想看 · 在看 · 看过 · 搁置 · 抛弃</div>
        </div>
        {/* [lc-1534] 「正在追」封面条：填面板中部留白；点封面 → 该条目详情 */}
        {acct.bound && watch.length > 0 && (
          <div className="bgmac-watch">
            <div className="bgmac-watch-hd">
              正在追 <b>{Math.max(counts(3), watch.length)}</b> 部
            </div>
            <div className="bgmac-watch-row">
              {watch.slice(0, 5).map((w) => (
                <button
                  key={w.id}
                  className={`bgmac-wcover${w.cover ? "" : " empty"}`}
                  title={w.name}
                  onClick={() =>
                    onOpen?.({
                      id: w.id,
                      type: 2,
                      name: w.name,
                      name_cn: w.name,
                      air_date: "",
                      votes: 0,
                      doing: 0,
                      cover: w.cover,
                    })
                  }
                >
                  {w.cover ? (
                    <img src={bgmImg(w.cover)} alt="" loading="lazy" draggable={false} />
                  ) : (
                    <IcoFilmL size={16} strokeWidth={1.8} />
                  )}
                </button>
              ))}
            </div>
          </div>
        )}
        {acct.bound ? (
          <div className="bgmac-acts">
            <button className="bgmac-act" disabled={syncBusy} onClick={syncNow}>
              {syncBusy ? "同步中…" : "立即同步"}
            </button>
            <button className="bgmac-link" onClick={unbind}>
              解绑
            </button>
            {note && !err && <span className="bgmac-inline-note">{note}</span>}
          </div>
        ) : bindOpen ? (
          <div className="bgmac-bind">
            <input
              value={token}
              onChange={(e) => setToken(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !busy && bind()}
              placeholder="粘贴 Bangumi Access Token"
              autoFocus
            />
            <button className="bgmac-cta sm" disabled={busy || !token.trim()} onClick={bind}>
              {busy ? "连接中…" : "连接"}
            </button>
          </div>
        ) : (
          <button className="bgmac-cta" onClick={() => setBindOpen(true)}>
            → 连接 Bangumi
          </button>
        )}
      </div>

      {/* 右上：同步状态图（标题在 SVG 内，用户设计稿） */}
      <div className="bgmac-stats">
        {acct.bound ? (
          <div className="bgmac-stats-body">
            {/* [lc-1528] 同步状态图（用户设计稿移植）：环形图 + 引线联动图例行。
                悬停扇形/图例行 → 该段高亮其余变暗 + 引线连到该行。 */}
            <svg className="bgmac-chart" viewBox={`0 0 ${VB_W} ${VB_H}`} role="img" aria-label="同步状态">
              <text className="bgmac-chart-title" x={VB_W / 2} y="13" textAnchor="middle" dominantBaseline="central">
                同步状态
              </text>
              {total > 0 ? (
                segs
                  .filter((s) => s.n > 0)
                  .map((s) => {
                    const i = segs.indexOf(s);
                    return (
                      <path
                        key={s.k}
                        className="bgmac-sector"
                        d={sectorPath(s.a0, s.a1)}
                        fill={s.color}
                        opacity={hl == null || hl === i ? 1 : 0.32}
                        onMouseEnter={() => setHl(i)}
                        onMouseLeave={() => setHl(null)}
                      >
                        <title>{`${s.label} ${s.n}`}</title>
                      </path>
                    );
                  })
              ) : (
                <path className="bgmac-sector" d={sectorPath(0, 1)} fill="var(--ph-bg)" />
              )}
              {total > 0 && (
                <>
                  <text className="bgmac-chart-num" x={CX} y={CY - 7} textAnchor="middle" dominantBaseline="central">
                    {total}
                  </text>
                  <text className="bgmac-chart-sub" x={CX} y={CY + 11} textAnchor="middle" dominantBaseline="central">
                    部番剧
                  </text>
                </>
              )}
              {/* 引线：当前高亮段的连线 */}
              {hl != null && (
                <path
                  className="bgmac-leader show"
                  d={leaderPath(segs[hl].frac > 0 ? (segs[hl].a0 + segs[hl].a1) / 2 : 0, rowY(hl))}
                  stroke={segs[hl].color}
                />
              )}
              {/* 图例行 */}
              {segs.map((s, i) => (
                <g
                  key={s.k}
                  className={`bgmac-chart-row${hl === i ? " hl" : ""}`}
                  onMouseEnter={() => setHl(i)}
                  onMouseLeave={() => setHl(null)}
                >
                  <rect className="bgmac-row-bg" x={ROW_BG_X} y={rowY(i) - 10} width={VALUE_X + 8 - ROW_BG_X} height={20} rx="6" />
                  <circle cx={DOT_X} cy={rowY(i)} r="4.5" fill={s.color} />
                  <text className="bgmac-chart-label" x={LABEL_X} y={rowY(i)} dominantBaseline="central">
                    {s.label}
                  </text>
                  <text className="bgmac-chart-value" x={VALUE_X} y={rowY(i)} textAnchor="end" dominantBaseline="central">
                    {s.n}
                  </text>
                </g>
              ))}
            </svg>
          </div>
        ) : (
          <div className="bgmac-stats-empty">
            <span className="bgmac-block-t">同步状态</span>
            <svg viewBox="0 0 100 100" width="76" height="76">
              <circle cx="50" cy="50" r="42" fill="none" stroke="var(--line-strong)" strokeWidth="7" strokeDasharray="4 6" />
            </svg>
            <span>连接后展示追番统计</span>
          </div>
        )}
      </div>

      {/* 底部通栏：当季追番状态（本季放送 ∩ 本地追番表） */}
      <div className="bgmac-season">
        <div className="bgmac-block-t">当季追番状态</div>
        {acct.bound ? (
          season.total > 0 ? (
            <>
              <div className="bgmac-season-bar">
                {BGAC_TYPES.map((t) => {
                  const n = Number(season.byType?.[String(t.k)] ?? 0);
                  if (n <= 0) return null;
                  return (
                    <i
                      key={t.k}
                      className="bgmac-season-seg"
                      style={{ width: `${(n / season.total) * 100}%`, background: t.color }}
                    />
                  );
                })}
              </div>
              <div className="bgmac-season-legend">
                {BGAC_TYPES.map((t) => {
                  const n = Number(season.byType?.[String(t.k)] ?? 0);
                  if (n <= 0) return null;
                  return (
                    <span key={t.k} className="bgmac-legend-row">
                      <i className="bgmac-dot" style={{ background: t.color }} />
                      {t.label}
                      <b>{n}</b>
                    </span>
                  );
                })}
                <span className="bgmac-season-total">
                  当季 {season.seasonTotal} 部中已标记 {season.total} 部
                </span>
              </div>
            </>
          ) : (
            <div className="bgmac-season-empty">
              当季 {season.seasonTotal > 0 ? `${season.seasonTotal} 部番剧` : "番剧"}暂无追番标记
            </div>
          )
        ) : (
          <div className="bgmac-season-empty">连接 Bangumi 后展示当季追番分布</div>
        )}
      </div>

      {/* 错误/提示收在左列内（不再整卡宽灰条）；已连接的成功提示走行内小字 */}
      {err && <div className="bgmac-err">{err}</div>}
      {!acct.bound && note && !err && <div className="bgmac-note">{note}</div>}
      {!acct.bound && bindOpen && (
        <div className="bgmac-tip">在 bangumi.tv 个人设置 → 访问令牌 生成，粘贴后即可同步</div>
      )}
    </div>
  );
}

function BgmPane({ active, onOpen }: { active: boolean; onOpen: (it: BgmCalendarItem) => void }) {
  const todayId = localWeekdayId();
  const [days, setDays] = useState<BgmCalendarDay[]>([]);
  const [wd, setWd] = useState<number>(todayId); // 默认今天（设计稿的"今天"带点，也默认展开）
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [nonce, setNonce] = useState(0);
  const [probe, setProbe] = useState(0); // [lc-123-rev5] 10 分钟后台探测
  // 手动 ⟳ / 失败重试的语义不同（见下）：⟳ = force（**仍带条件头**）；失败重试 = force + noEtag（全量）
  const forceRef = useRef(false);
  const hardRef = useRef(false);
  const daysRef = useRef<BgmCalendarDay[]>([]);
  useEffect(() => {
    daysRef.current = days;
  }, [days]);
  // 库标题索引：与日历**并行**拉（不阻塞列表；没回来就先按普通列表渲染，回来再补标签/分组）
  const lib = useLibIndex(active);

  useEffect(() => {
    if (!active) return;
    let alive = true;
    const force = forceRef.current; // 手动 ⟳：跳过本地 TTL 检查（但条件头照带）
    const noEtag = hardRef.current; // 失败重试：连条件头都不带（强制全量）
    forceRef.current = false;
    hardRef.current = false;
    setBusy(true);
    setErr("");
    invoke<{ days?: BgmCalendarDay[]; changed?: boolean }>("bgm_calendar", {
      force,
      noEtag,
      day: localDay(), // 跨天分桶键：本地零点一过就换键 ⇒ 必然重取
    })
      .then((d) => {
        if (!alive) return;
        // changed=false（命中缓存 / 304 / 内容未变）且已有数据 → **一个 state 都不碰**（零 re-render）
        if (d?.changed === false && daysRef.current.length) return;
        setDays(Array.isArray(d?.days) ? d.days : []);
      })
      .catch((e) => {
        if (alive) setErr(String(e));
      })
      .finally(() => {
        if (alive) setBusy(false);
      });
    return () => {
      alive = false;
    };
  }, [active, nonce, probe]);

  // 后台探测：每 10 分钟一次；命令层只在 TTL（3h）过期时才真发请求，而且带 ETag —— 未变就 304，
  // changed=false ⇒ 上面一个 state 都不碰（不 re-render、不重置滚动）。
  useEffect(() => {
    if (!active) return;
    const t = window.setInterval(() => setProbe((n) => n + 1), 10 * 60 * 1000);
    return () => window.clearInterval(t);
  }, [active]);

  // 按 **weekday.id** 取组（不是数组下标）；找不到就当这天空（不拿别的天凑）
  const day = days.find((g) => g.id === wd) ?? null;
  const dayItems = day?.items ?? [];
  // 星期胶囊的迷你占比条：以当周最大值（至少 1）为 100%
  const maxCount = Math.max(1, ...days.map((g) => g.count || 0));
  const weekTotal = days.reduce((n, g) => n + (g.count || 0), 0);
  // 当天汇总：部数 + 8.0+ 数量 + 热度最高（doing=在看人数最大者；没有 doing 就不写这段，不编）
  const hot = dayItems.reduce<BgmCalendarItem | null>(
    (a, b) => (a == null || (b.doing ?? 0) > (a.doing ?? 0) ? b : a),
    null,
  );
  const hotName = hot ? hot.name_cn || hot.name : "";
  const hiCount = dayItems.filter((x) => typeof x.score === "number" && (x.score ?? 0) >= 8).length;
  const wdCn = WEEKDAYS.find((w) => w.id === wd)?.cn ?? "";
  // 逐条匹配：loading/partial 只给"命中"打标；只有 ok（索引完整）才敢分组和标"不在库"
  const withLib = lib.state === "ok" || lib.state === "partial";
  const rows = dayItems.map((it, i) => ({
    it,
    rank: i + 1, // 序号 = 该天列表内的位置（分组重排后也保持原序）
    inLib: withLib ? libHasTitle(lib.titles, it.name_cn || it.name) : false,
  }));
  const inRows = rows.filter((r) => r.inLib);
  const outRows = rows.filter((r) => !r.inLib);
  const renderRow = (r: (typeof rows)[number]) => (
    <BgmCalRow
      key={r.it.id}
      it={r.it}
      today={wd === todayId}
      rank={r.rank}
      libState={r.inLib ? "in" : null}
      onOpen={onOpen}
    />
  );

  return (
    <section className="dpane">
      <header className="dpane-hd">
        <h2>新番更新</h2>
        <span className="dn">{weekTotal > 0 ? `${weekTotal} 部 / 周` : ""}</span>
        <span className="dsp" />
        <button
          className="dicon"
          title="刷新新番列表"
          disabled={busy}
          onClick={() => {
            forceRef.current = true;
            setNonce((n) => n + 1);
          }}
        >
          ⟳
        </button>
      </header>
      {/* [lc-1516] Bangumi 账号面板：紧跟栏头（避开顶部 56px 的胶囊让位区） */}
      <BgmAccountCard onOpen={onOpen} />
      <div className="dweeks">
        {WEEKDAYS.map((w) => {
          const g = days.find((x) => x.id === w.id);
          const pct = g && g.count > 0 ? Math.max(4, Math.round((g.count / maxCount) * 100)) : 0;
          return (
            <button
              key={w.id}
              className={`dweek${wd === w.id ? " on" : ""}${todayId === w.id ? " today" : ""}`}
              onClick={() => setWd(w.id)}
              title={g ? `${w.cn}共 ${g.count} 部` : w.cn}
            >
              {w.cn}
              <span className="dweek-bar" aria-hidden="true">
                <b style={{ width: `${pct}%` }} />
              </span>
              <i>{g ? `${g.count} 部` : "…"}</i>
            </button>
          );
        })}
      </div>
      <div className="dscroll">
        {busy && !days.length ? (
          <ul className="dlist">
            {[0, 1, 2, 3].map((i) => (
              <li key={i} className="ditem">
                <Dsk cls="dsk-cv" />
                <div className="dtx">
                  <Dsk cls="dsk-line" />
                  <Dsk cls="dsk-line short" />
                </div>
              </li>
            ))}
          </ul>
        ) : err ? (
          <div className="dstate">
            <div className="dstate-msg">{err}</div>
            <button
              className="dretry"
              onClick={() => {
                // 失败重试：force + noEtag（强制全量重拉，别拿坏缓存反复撞）
                hardRef.current = true;
                setNonce((n) => n + 1);
              }}
            >
              重试
            </button>
          </div>
        ) : !day || !day.items.length ? (
          <div className="dstate">
            {wd === todayId ? "今天没有新番更新" : `${wdCn}没有新番更新`}
          </div>
        ) : (
          <>
            <div className="ddaybar">
              <b>
                {wdCn}
                {wd === todayId ? "（今天）" : ""}
              </b>
              <span>
                {wd === todayId ? `今日 ${dayItems.length} 部新番更新` : `${dayItems.length} 部更新`}
                {hiCount > 0 ? ` · ${hiCount} 部评分 8.0+` : ""}
                {hotName ? ` · 热度最高：${hotName}` : ""}
              </span>
            </div>
            <ul className="dlist">
              {lib.state === "ok" ? (
                <>
                  {inRows.length > 0 && <li className="dgrp">已在飞牛库 · {inRows.length}</li>}
                  {inRows.map(renderRow)}
                  {outRows.length > 0 && <li className="dgrp">不在库 · {outRows.length}</li>}
                  {outRows.map(renderRow)}
                </>
              ) : (
                rows.map(renderRow)
              )}
            </ul>
          </>
        )}
      </div>
    </section>
  );
}

function DiscoverView({
  active,
  onOpenDetail,
  onOpenBgm,
}: {
  active: boolean;
  /** [lc-1444] 左栏 TMDB 条目 → 主详情页（父级合成 MediaItem 并切 view） */
  onOpenDetail: (it: TmdbDiscoverItem, mediaType: "tv" | "movie") => void;
  /** [lc-1444] 右栏新番表条目 → 主详情页（条目自带 Bangumi subject id） */
  onOpenBgm: (it: BgmCalendarItem) => void;
}) {
  return (
    <div className="discover">
      <TmdbPane active={active} onOpenDetail={onOpenDetail} />
      <BgmPane active={active} onOpen={onOpenBgm} />
    </div>
  );
}

/* ════════════ [lc-1502] 在线资源面板（发现页无源条目 · Kazumi「我的规则」式）════════════
   源由**用户勾选启用**（⚙ 设置面板，localStorage 持久化），搜索只跑勾选的源：
   · ⚙ 源管理：复选框启用/停用、全选/清空、＋粘贴 Kazumi 规则 JSON 添加、× 删除
   · 结果按源分组：N 个结果（绿）/ 无结果 / 检索失败（红），点条目 → 线路/选集 → mpv
   · 解析直链：HTTP 正则直解 → 隐藏 WebView 嗅探兜底 */
type WebrsSearchItem = { name: string; url: string };
type WebrsRoad = { name: string; index: number; episodes: WebrsSearchItem[] };
type WebrsGroup = { name: string; status: "ok" | "empty" | "error"; error: string; items: WebrsSearchItem[] };
type WebrsRuleInfo = { name: string; mode?: string };

const WEBRS_ENABLED_KEY = "anibox.webrs.enabledSources";

/** 已勾选源名单（localStorage 持久化）。null = 从未改过 → 视为全启用。 */
function loadEnabled(): string[] | null {
  try {
    const raw = localStorage.getItem(WEBRS_ENABLED_KEY);
    if (raw == null) return null;
    const v = JSON.parse(raw);
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return null;
  }
}

function WebrsPanel({ title }: { title: string }) {
  const [groups, setGroups] = useState<WebrsGroup[] | null>(null); // null = 尚未搜索
  const [kw, setKw] = useState(title);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  // 源管理
  const [srcs, setSrcs] = useState<WebrsRuleInfo[]>([]);
  const [enabled, setEnabled] = useState<string[] | null>(null); // null = 全启用
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [addText, setAddText] = useState("");
  const [addErr, setAddErr] = useState("");
  // [lc-1503] 一键测试源：对全部源各发一次真实搜索，行内显示 ✓ n / 无结果 / ✗
  const [tests, setTests] = useState<Record<string, { status: string; count: number; error: string }>>({});
  const [testing, setTesting] = useState(false);
  // 二级视图：选集
  const [sel, setSel] = useState<{ rule: string; item: WebrsSearchItem } | null>(null);
  const [roads, setRoads] = useState<WebrsRoad[]>([]);
  const [roadIdx, setRoadIdx] = useState(0);
  const [subBusy, setSubBusy] = useState("");

  const isEnabled = useCallback(
    (name: string) => enabled == null || enabled.includes(name),
    [enabled],
  );
  const enabledNames = useMemo(() => srcs.filter((r) => isEnabled(r.name)).map((r) => r.name), [srcs, isEnabled]);

  const refreshRules = useCallback(async () => {
    try {
      const r = await invoke<WebrsRuleInfo[]>("webrs_rules");
      setSrcs(r);
      return r;
    } catch (e) {
      setErr(String(e));
      return [];
    }
  }, []);

  // [lc-1508] 引导提示（中性样式，区别于真错误 err）：如"还没勾选任何源"
  const [srcHint, setSrcHint] = useState("");
  // [lc-1513] 二级视图（选集）返回时先播退出动画再卸载
  const [subMounted, subClosing] = useDelayedUnmount(!!sel);
  const doSearch = useCallback(
    async (k: string) => {
      const key = k.trim();
      if (!key) return;
      const picked = srcs.filter((r) => isEnabled(r.name)).map((r) => r.name);
      if (picked.length === 0) {
        setSrcHint("还没有勾选任何源，点右侧 ⚙ 选择要用的源");
        return;
      }
      setSrcHint("");
      setBusy(true);
      setErr("");
      setSel(null);
      setRoads([]);
      try {
        const r = await invoke<WebrsGroup[]>("webrs_search_multi", { keyword: key, rules: picked });
        setGroups(r);
        const first = r.find((g) => g.status === "ok");
        setExpanded(first ? { [first.name]: true } : {});
      } catch (e) {
        setErr(String(e));
        setGroups([]);
      } finally {
        setBusy(false);
      }
    },
    [srcs, isEnabled],
  );

  // 进面板：拉源列表 + 自动按标题搜一次（只搜勾选的）
  const autoDone = useRef(false);
  useEffect(() => {
    if (autoDone.current) return;
    autoDone.current = true;
    setEnabled(loadEnabled());
    refreshRules().then((r) => {
      const en = loadEnabled();
      const picked = r.filter((x) => en == null || en.includes(x.name)).map((x) => x.name);
      if (r.length > 0 && picked.length > 0) doSearch(title);
      else setSrcHint(r.length > 0 ? "还没有勾选任何源，点右侧 ⚙ 选择要用的源" : "还没有可用源，点 ⚙ 添加规则");
    });
  }, []);

  const toggleSource = (name: string) => {
    const cur = srcs.filter((r) => isEnabled(r.name)).map((r) => r.name);
    const next = cur.includes(name) ? cur.filter((x) => x !== name) : [...cur, name];
    setEnabled(next);
    localStorage.setItem(WEBRS_ENABLED_KEY, JSON.stringify(next));
  };
  const setAllSources = (on: boolean) => {
    const next = on ? srcs.map((r) => r.name) : [];
    setEnabled(next);
    localStorage.setItem(WEBRS_ENABLED_KEY, JSON.stringify(next));
  };

  const openChapters = async (rule: string, item: WebrsSearchItem) => {
    setSel({ rule, item });
    setRoads([]);
    setRoadIdx(0);
    setSubBusy("拉取选集…");
    try {
      const r = await invoke<{ roads: WebrsRoad[] }>("webrs_chapters", {
        ruleName: rule,
        url: item.url,
      });
      setRoads(r.roads ?? []);
    } catch (e) {
      setErr(String(e));
      setSel(null);
    } finally {
      setSubBusy("");
    }
  };

  const playEp = async (ep: WebrsSearchItem) => {
    if (!sel) return;
    setSubBusy("解析视频直链…（页面藏得深时会拉起嗅探，最多等 40s）");
    setErr("");
    try {
      const r = await invoke<{ url: string; referer: string; ua: string; via: string }>(
        "webrs_resolve",
        { ruleName: sel.rule, url: ep.url },
      );
      setSubBusy("正在拉起独立播放器…");
      await invoke("webrs_play", { url: r.url, referer: r.referer, ua: r.ua });
      setSubBusy(`已在独立播放器窗口播放 · ${sel.item.name} · ${ep.name}（${r.via === "webview" ? "嗅探直链" : "页面直解"}）`);
    } catch (e) {
      setErr(String(e));
      setSubBusy("");
    }
  };

  const addRule = async () => {
    setAddErr("");
    try {
      const name = await invoke<string>("webrs_add_rule", { jsonText: addText });
      setAddOpen(false);
      setAddText("");
      await refreshRules();
      setErr("");
      setSrcHint(`已添加源「${name}」，记得在列表里勾选它`);
    } catch (e) {
      setAddErr(String(e));
    }
  };

  // [lc-1503] 一键测试：全部源（不管勾没勾）各跑一次搜索，结果挂到每行徽标。
  // 测试词用搜索框关键词（空则用通用词「少女」——各站收录差异最小的探针）。
  const testAll = async () => {
    if (testing) return;
    const key = kw.trim() || "少女";
    setTesting(true);
    setAddErr("");
    const loading: Record<string, { status: string; count: number; error: string }> = {};
    for (const r of srcs) loading[r.name] = { status: "loading", count: 0, error: "" };
    setTests(loading);
    try {
      const r = await invoke<WebrsGroup[]>("webrs_search_multi", {
        keyword: key,
        rules: srcs.map((s) => s.name),
      });
      const next: Record<string, { status: string; count: number; error: string }> = {};
      for (const g of r) next[g.name] = { status: g.status, count: g.items.length, error: g.error };
      setTests(next);
    } catch (e) {
      setAddErr(String(e));
      setTests({});
    } finally {
      setTesting(false);
    }
  };

  const removeRule = async (name: string) => {
    if (!window.confirm(`删除源「${name}」？（内置源会在下次启动时恢复）`)) return;
    try {
      await invoke("webrs_remove_rule", { ruleName: name });
      await refreshRules();
      const next = (enabled ?? srcs.map((r) => r.name)).filter((x) => x !== name);
      setEnabled(next);
      localStorage.setItem(WEBRS_ENABLED_KEY, JSON.stringify(next));
      setGroups((g) => (g ? g.filter((x) => x.name !== name) : g));
    } catch (e) {
      setErr(String(e));
    }
  };

  const badge = (g: WebrsGroup) =>
    g.status === "ok" ? `${g.items.length} 个结果` : g.status === "empty" ? "无结果" : "检索失败";

  return (
    <div className="webrs">
      <div className="webrs-bar">
        <input
          value={kw}
          onChange={(e) => setKw(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !busy && doSearch(kw)}
          placeholder="搜索资源关键词"
        />
        <button className="webrs-go" disabled={busy} onClick={() => doSearch(kw)}>
          {busy ? "搜索中" : "搜索"}
        </button>
        <button
          className="webrs-gear"
          title={`源管理（已勾选 ${enabledNames.length}/${srcs.length}）`}
          onClick={() => {
            setSettingsOpen(true);
            setAddErr("");
          }}
        >
          ⚙
        </button>
      </div>
      {busy && <div className="webrs-note">正在搜索 {enabledNames.length} 个已勾选源…</div>}
      {err && <div className="webrs-err">{err}</div>}
      {srcHint && !err && <div className="webrs-hint">{srcHint}</div>}
      {subBusy && <div className="webrs-note">{subBusy}</div>}

      {/* 二级视图：选集（返回时播退出动画；roads 在动画期间保留，内容不闪空） */}
      {(sel || subMounted) && (
        <div className={`webrs-sub${subClosing ? " view-out" : " view-in"}`}>
          <div className="webrs-crumbs">
            <span className="webrs-cur">{sel?.item.name ?? ""}</span>
            <span className="webrs-src">来源：{sel?.rule ?? ""}</span>
            <button
              className="webrs-back"
              onClick={() => setSel(null)}
              title="返回搜索结果"
            >
              ‹ 返回结果
            </button>
          </div>
          {roads.length > 1 && (
            <div className="webrs-roads">
              {roads.map((rd, i) => (
                <button key={`${rd.name}-${i}`} className={i === roadIdx ? "on" : ""} onClick={() => setRoadIdx(i)}>
                  {rd.name}
                </button>
              ))}
            </div>
          )}
          <div className="webrs-eps">
            {(roads[roadIdx]?.episodes ?? []).map((ep) => (
              <button key={ep.url} disabled={subBusy.startsWith("解析")} onClick={() => playEp(ep)} title={ep.url}>
                {ep.name}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* 一级视图：按源分组的结果 */}
      {!sel && groups !== null && (
        <div className="webrs-groups">
          {groups.map((g) => (
            <div key={g.name} className={`webrs-g${g.status === "ok" ? " ok" : ""}`}>
              <button
                className="webrs-g-hd"
                onClick={() => setExpanded((m) => ({ ...m, [g.name]: !m[g.name] }))}
                title={g.error || g.name}
              >
                <span className="webrs-g-name">{g.name}</span>
                <span className={`webrs-g-badge ${g.status}`}>{badge(g)}</span>
                <span className="webrs-g-arrow">{expanded[g.name] ? "⌃" : "⌄"}</span>
              </button>
              {expanded[g.name] && g.status === "ok" && (
                <div className="webrs-g-items">
                  {g.items.map((it) => (
                    <button key={it.url} onClick={() => openChapters(g.name, it)} title={it.url}>
                      {it.name}
                    </button>
                  ))}
                </div>
              )}
            </div>
          ))}
          {groups.every((g) => g.status !== "ok") && (
            <div className="webrs-note">勾选的源都没有结果：换个关键词、换个源，或 ⚙ 里添加规则</div>
          )}
        </div>
      )}

      {/* ⚙ 源管理设置面板。portal 挂 body：弹窗留在 .content 里会被 sticky 的
          .dseg（z30 + backdrop-filter）合成穿透 —— 玻璃层画到遮罩上面成"白条"
          （[lc-1504] 用户截图实锤）。portal 后与 .dseg 同在顶层上下文，
          mask z40 > dseg z30 正常压制；顶栏 z50 仍在其上，窗口按钮不受影响。 */}
      {settingsOpen &&
        createPortal(
          <div className="ddet-mask" onClick={() => setSettingsOpen(false)}>
          <div className="sum-modal webrs-add-modal" onClick={(e) => e.stopPropagation()}>
            <div className="sum-modal-h">
              <h3>
                源管理 <span className="webrs-mgr-count">已勾选 {enabledNames.length} / {srcs.length}</span>
              </h3>
              <button className="ddet-close sum-modal-x" onClick={() => setSettingsOpen(false)}>
                ×
              </button>
            </div>
            <div className="webrs-mgr-body">
              <div className="webrs-mgr-acts">
                <button onClick={testAll} disabled={testing} title="对全部源各发一次真实搜索，检验源是否可用">
                  {testing ? "测试中…" : "⚡ 一键测试"}
                </button>
                <button onClick={() => setAllSources(true)}>全选</button>
                <button onClick={() => setAllSources(false)}>清空</button>
                <button className="webrs-mgr-addbtn" onClick={() => setAddOpen((v) => !v)}>
                  {addOpen ? "收起" : "＋ 添加源"}
                </button>
              </div>
            {addOpen && (
              <>
                <textarea
                  className="webrs-add-ta"
                  value={addText}
                  onChange={(e) => setAddText(e.target.value)}
                  placeholder="粘贴 Kazumi 规则 JSON（可从 KazumiRules 仓库复制规则文件全文），须包含 name 字段"
                  rows={10}
                />
                {addErr && <div className="webrs-err">{addErr}</div>}
                <div className="webrs-add-acts">
                  <button className="webrs-go" disabled={!addText.trim()} onClick={addRule}>
                    导入
                  </button>
                </div>
              </>
            )}
            <div className="webrs-mgr-list">
              {srcs.map((r) => {
                const t = tests[r.name];
                return (
                  <div key={r.name} className={`webrs-mgr-row${isEnabled(r.name) ? " on" : ""}`}>
                    <label className="webrs-mgr-main">
                      <input type="checkbox" checked={isEnabled(r.name)} onChange={() => toggleSource(r.name)} />
                      <span className="webrs-mgr-name">{r.name}</span>
                      {r.mode === "api" && <span className="webrs-mgr-mode">API</span>}
                    </label>
                    {t && (
                      <span
                        className={`webrs-mgr-test ${t.status}`}
                        title={t.error || (t.status === "ok" ? `搜索正常，${t.count} 条结果` : "")}
                      >
                        {t.status === "loading"
                          ? "…"
                          : t.status === "ok"
                            ? `✓ ${t.count} 条`
                            : t.status === "empty"
                              ? "无结果"
                              : "✗ 失败"}
                      </span>
                    )}
                    <button className="webrs-mgr-x" title={`删除源 ${r.name}`} onClick={() => removeRule(r.name)}>
                      ×
                    </button>
                  </div>
                );
              })}
              {srcs.length === 0 && <div className="webrs-note">没有可用源 —— 「＋ 添加源」导入一条规则</div>}
              </div>
              <div className="webrs-mgr-tip">
                勾选后关闭本面板，搜索只查已勾选的源；内置源删除后下次启动会恢复。一键测试用搜索框关键词（空则「少女」）。
              </div>
            </div>
          </div>
        </div>,
          document.body,
        )}
    </div>
  );
}

/**
 * [lc-1513] 延迟卸载：active 变 false 时先保留渲染 closing 时长（给退出动画留时间），
 * 到点再真正卸载。用于「返回上一级」的转场 —— 否则路由一切换旧视图瞬间消失。
 */
function useDelayedUnmount(active: boolean, ms = 200) {
  const [mounted, setMounted] = useState(active);
  const [closing, setClosing] = useState(false);
  useEffect(() => {
    if (active) {
      setMounted(true);
      setClosing(false);
      return;
    }
    if (!mounted) return;
    setClosing(true);
    const t = window.setTimeout(() => {
      setMounted(false);
      setClosing(false);
    }, ms);
    return () => window.clearTimeout(t);
  }, [active, mounted, ms]);
  return [mounted, closing] as const;
}

function DetailView({ base, onBack }: { base: MediaItem; onBack: () => void }) {
  const [data, setData] = useState<Record<string, unknown> | null>(null);
  const [seasons, setSeasons] = useState<Season[]>([]);
  const [seasonIdx, setSeasonIdx] = useState(0);
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  // TMDB 完整详情（剧照/相似/更多/演职人员），详情页「更多」分段的 TMDB 侧数据源
  const [tmdb, setTmdb] = useState<Record<string, unknown> | null>(null);
  // [lc-1507] TMDB 请求是否已 settle（成败都置位）：区分「加载中」（骨架）与「已失败」
  // （不再转骨架，改显示 Bangumi 简介兜底 / 空态）—— 否则失败后骨架永远转
  const [tmdbDone, setTmdbDone] = useState(false);
  const [err, setErr] = useState("");
  // [lc-118] 详情页分段栏当前项
  const [tab, setTab] = useState("overview");
  // [lc-1423] 重新引入简介展开态：true = 弹窗打开（看完整简介）。
  //   注意语义与 lc-1410 之前不同 —— 那时是"就地铺开"（会撑高容器、拉长海报），
  //   现在是"**弹出弹窗**看全文"，容器高度不受影响。
  const [introFull, setIntroFull] = useState(false);
  // [lc-119] 吐槽（Bangumi 评论）：懒加载 —— 用户切到该分段时才匹配+拉评论（避免拖慢首屏）
  const [bgm, setBgm] = useState<BgmMatch | null>(null);
  const bgmRef = useRef<BgmMatch | null>(null); // 供 legacy 回填任务/翻页读最新值（不进依赖数组）
  // [lc-1401] 匹配是否已尝试（成败都置位）。旧版用它给「吐槽」分段做入口闸门；
  //   新版式标签条恒在（不增删项），这个标志只作诊断用途 —— 保留 setter 以沿用同一条匹配路径。
  const [, setBgmProbed] = useState(false);
  const [comments, setComments] = useState<BgmComment[]>([]);
  const [commentsErr, setCommentsErr] = useState("");
  const [commentsBusy, setCommentsBusy] = useState(false); // 首屏拉取（骨架屏）
  const [commentsMoreBusy, setCommentsMoreBusy] = useState(false); // 翻页中（列表尾部提示）
  const [commentsDone, setCommentsDone] = useState(false); // 已翻到底
  const [commentsNonce, setCommentsNonce] = useState(0); // 失败重试触发器
  const commentsOffset = useRef(0); // 下一页 offset = 已累计的**实际返回条数**
  // [lc-1412] 评论（长影评）状态：与吐槽同款一套（懒加载 / 翻页 / 到底 / 重试），但数据源是 /reviews
  const [reviews, setReviews] = useState<BgmReview[]>([]);
  const [reviewsErr, setReviewsErr] = useState("");
  const [reviewsBusy, setReviewsBusy] = useState(false);
  const [reviewsMoreBusy, setReviewsMoreBusy] = useState(false);
  const [reviewsDone, setReviewsDone] = useState(false);
  const [reviewsNonce, setReviewsNonce] = useState(0);
  const reviewsOffset = useRef(0);
  const reviewsBusyRef = useRef(false);
  const reviewsDoneRef = useRef(false);
  const commentsBusyRef = useRef(false);
  const commentsDoneRef = useRef(false);
  // 更多分段（Bangumi 角色/制作人员）：懒加载 —— 与吐槽同套路（骨架屏 / 失败重试 / 空态）
  const [chars, setChars] = useState<BgmCharacter[]>([]);
  const [persons, setPersons] = useState<BgmPerson[]>([]);
  const [moreErr, setMoreErr] = useState("");
  const [moreBusy, setMoreBusy] = useState(false); // 首屏拉取（骨架屏）
  const [moreNonce, setMoreNonce] = useState(0); // 失败重试触发器
  const [moreProbed, setMoreProbed] = useState(false); // 拉取是否已尝试（成败都置位；空壳分段靠它退场）
  const moreLoadedRef = useRef(false); // 成功拉到过 → 不重复拉（切走再回来不重打请求）
  // [lc-128] 演员表（**TMDB 主源**）：与「更多」同套路懒加载；Rust 侧按 guid + tmdb id 落盘缓存，
  // 命中本地就零请求。Bangumi/飞牛侧只在 TMDB 无数据时兜底展示。
  const [credits, setCredits] = useState<TmdbCredits | null>(null);
  const [creditsErr, setCreditsErr] = useState("");
  const [creditsBusy, setCreditsBusy] = useState(false);
  const [creditsNonce, setCreditsNonce] = useState(0);
  const creditsLoadedRef = useRef(false);
  // [lc-128] 剧照点击放大（original 尺寸：走同一套三级缓存，拉了即落盘）
  const [zoom, setZoom] = useState("");
  // [lc-1401] 关联条目（Bangumi /subjects/{id}/subjects）：与「更多」同套懒加载骨架
  const [related, setRelated] = useState<BgmRelated[]>([]);
  const [relatedErr, setRelatedErr] = useState("");
  const [relatedBusy, setRelatedBusy] = useState(false);
  const [relatedNonce, setRelatedNonce] = useState(0);
  const relatedLoadedRef = useRef(false);
  // [lc-1401] 右栏选集：本机条目集数 + Bangumi 集号对齐表（desc/标题回填）
  const [bgmEps, setBgmEps] = useState<BgmEpisodeRow[]>([]);
  // [lc-1401] Bangumi 条目详情（顶部标签框用它的 tags[]；与 bgm 匹配分开存，语义不同）
  const [bgmSub, setBgmSub] = useState<Record<string, unknown> | null>(null);
  // 注意：与吐槽一样**不做 busy 重入闸门** —— StrictMode 的双调用会「首跑 abort + 二跑早退」，
  // 骨架屏会永久卡住；靠 alive 标志丢弃过期结果即可（最坏情况是 dev 下多发一次请求）。

  // [lc-1513] 标签页切换淡入（详情/评论/吐槽/关联/演职人员）：class 加一下即触发
  const [tabAnim, setTabAnim] = useState(false);
  useEffect(() => {
    if (prefersReducedMotion()) return;
    setTabAnim(true);
    const t = window.setTimeout(() => setTabAnim(false), 260);
    return () => window.clearTimeout(t);
  }, [tab]);

  const isSeries = base.type === "TvSeries" || base.type === "TV";
  // [lc-1444] 发现页条目形态：guid 是 `discover-<tmdbId>` 占位标识（不是真飞牛 guid），
  //   没有飞牛条目可查 —— 详情主体靠 TMDB（条目自带 tmdb_id 直接定位）+ Bangumi（标题匹配），
  //   选集区显示占位。dataReady = 主体可以渲染（真条目等 data 到位；发现页条目立即渲染）。
  const noFnOS = /^discover-/.test(base.guid);
  const dataReady = !!data || noFnOS;

  // [lc-1444] Bangumi 定位统一入口：发现页右栏条目自带 bgm_id → **直连**（省一次标题匹配，
  //   也不会匹配错同名番）；否则 bgm_match(标题+年)。结果写 bgmRef + setBgm，供
  //   首屏标签框 / 影评 / 吐槽 / 更多 / 关联五个懒加载点共用同一条路径。
  const resolveBgm = useCallback(async (): Promise<BgmMatch | null> => {
    const cached = bgmRef.current;
    if (cached) return cached;
    let m: BgmMatch | null = null;
    if (base.bgm_id) {
      m = { subject_id: base.bgm_id };
    } else {
      const title = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
      const r = await invoke<BgmMatch | null>("bgm_match", {
        query: title,
        year: String(base.release_date || "").slice(0, 4) || null,
        mediaType: base.type ?? null,
        cacheKey: base.guid,
      });
      m = r && r.subject_id ? r : null;
    }
    bgmRef.current = m;
    setBgm(m);
    return m;
  }, [base.bgm_id, base.guid, base.title, base.type, base.release_date, data]);

  /** [lc-128] TMDB 精确锚点：飞牛条目 `trim_id` 形如 `tm63578`（TMDB id）/ `tt325158`（IMDb id）/
   *  `bg456080`（Bangumi，无 TMDB id）；另有显式 `tmdb_id`/`imdb_id` 字段时优先。有锚点就**直接定位**，
   *  没有才回落标题搜索（Rust 侧搜索带相似度门槛，并回传 matched_by 说明"这条是谁给的 id"）。 */
  const tmdbAnchor = (() => {
    const d = data ?? {};
    const str = (v: unknown): string => (v == null ? "" : String(v).trim());
    const directTmdb = str(pick(d, ["tmdbId", "tmdb_id"]));
    const imdb = str(pick(d, ["imdb_id", "imdbId"]));
    const trim = str(pick(d, ["trim_id", "trimId"]));
    let tmdbId = /^\d+$/.test(directTmdb) ? directTmdb : "";
    if (!tmdbId) {
      const m = /^(?:tm)(\d+)$/i.exec(trim);
      if (m) tmdbId = m[1];
      else if (/^\d+$/.test(trim)) tmdbId = trim; // 纯数字：老口径当 TMDB id
    }
    const imdbId = imdb || (/^tt\d+$/i.test(trim) ? trim.toLowerCase() : "");
    return { trimId: trim, tmdbId, imdbId };
  })();

  useEffect(() => {
    setData(null);
    setSeasons([]);
    setSeasonIdx(0);
    setEpisodes([]);
    if (noFnOS) {
      // 发现页条目：无飞牛条目，跳过详情/季/集三条飞牛请求；清掉可能残留的详情错误
      setErr("");
      return;
    }
    invoke<Record<string, unknown>>("get_item_detail", { guid: base.guid })
      .then((d) => setData(d))
      .catch((e) => setErr(String(e)));
    // 层级：剧 → 季 → 集。episode/list 的入参是季 guid（传剧 guid 实测 data:null），
    // 所以先 season/list/{剧guid}，选中季后再 episode/list/{季guid}。
    if (isSeries) {
      invoke<Season[]>("get_seasons", { guid: base.guid })
        .then((ss) => setSeasons(Array.isArray(ss) ? ss : []))
        .catch(() => {});
    }
  }, [base.guid, isSeries, noFnOS]);

  useEffect(() => {
    const s = seasons[seasonIdx];
    if (!s) return;
    setEpisodes([]);
    invoke<Episode[]>("get_episodes", { guid: s.guid })
      .then((eps) =>
        setEpisodes(
          (Array.isArray(eps) ? eps : []).map((e) => ({
            ...e,
            quality:
              Array.isArray(e?.media_stream?.resolutions) && e.media_stream.resolutions[0]
                ? String(e.media_stream.resolutions[0]).replace(/p$/i, "")
                : undefined,
          })),
        ),
      )
      .catch(() => {});
  }, [seasons, seasonIdx]);

  // TMDB 完整详情（剧照/相似/更多/演职人员）：主仓库 fetchShowDetails 同款链路，失败静默降级
  // [lc-128] 额外带上 trim_id / tmdb_id / imdb_id 锚点：能精确定位就不靠标题搜索（matchedBy 见日志）
  // [lc-1507] 发现页 Bangumi 条目（无 tmdb 锚点）的标题搜索失败率很高 —— TMDB 对新番/第二季
  //   经常没有条目或没有中文译名（实测「转生就是剑 第二季」sim=0.17 只有第一季 0.22）。
  //   两级兜底：① 首搜失败 → 用 Bangumi **日文原名**（bgmSub.name）重搜一次（TMDB 收录日文
  //   原名远多于中文译名，且不带年份防错标）；② 日文重搜也失败 → 详情信息 tab 显示
  //   Bangumi 简介而不是永久骨架。
  const tmdbTitle = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
  const bgmNameJp = String(pick(bgmSub ?? {}, ["name"]) ?? "").trim();
  useEffect(() => {
    setTmdb(null);
    setTmdbDone(false);
    // 等条目详情到位再发：锚点（trim_id 等）就来自它，先发一次纯标题搜索再重发纯属浪费。
    // [lc-1444] 发现页条目例外：条目自带 tmdb_id，直接精确定位，不等（也没有）飞牛详情。
    if (!dataReady || !tmdbTitle) return;
    let alive = true;
    const fetchDetails = (title: string, year: string) =>
      invoke<Record<string, unknown>>("tmdb_show_details", {
        arg: {
          title,
          year,
          mediaType: base.type === "Movie" ? "movie" : "tv",
          trimId: tmdbAnchor.trimId || undefined,
          tmdbId: noFnOS
            ? base.tmdb_id
              ? String(base.tmdb_id)
              : undefined // 右栏新番条目没有 TMDB id → 走标题搜索
            : tmdbAnchor.tmdbId || undefined,
          imdbId: tmdbAnchor.imdbId || undefined,
        },
      });
    (async () => {
      let d: Record<string, unknown> | null = null;
      try {
        const r = await fetchDetails(tmdbTitle, String(base.release_date || "").slice(0, 4));
        if (r && !r.error) d = r;
      } catch {
        /* 静默：走日文重搜 */
      }
      if (!alive) return;
      // ① 首搜失败（或没搜到）→ 日文原名重搜（不带年份：TMDB/飞牛的年份错标很常见）
      if (!d && bgmNameJp && bgmNameJp !== tmdbTitle) {
        try {
          const r2 = await fetchDetails(bgmNameJp, "");
          if (r2 && !r2.error) d = r2;
        } catch {
          /* 静默：兜底空态 */
        }
      }
      if (!alive) return;
      if (d) setTmdb(d);
      setTmdbDone(true);
    })();
    return () => {
      alive = false;
    };
  }, [
    base.guid,
    tmdbTitle,
    bgmNameJp,
    dataReady,
    noFnOS,
    base.tmdb_id,
    base.release_date,
    base.type,
    tmdbAnchor.trimId,
    tmdbAnchor.tmdbId,
    tmdbAnchor.imdbId,
  ]);

  // [lc-119] 切换条目（本组件不随 base 重挂载）时清空 Bangumi 状态，避免把上一部的吐槽/匹配带过来
  useEffect(() => {
    bgmRef.current = null;
    commentsOffset.current = 0;
    commentsDoneRef.current = false;
    commentsBusyRef.current = false;
    setBgm(null);
    setBgmProbed(false);
    setComments([]);
    setCommentsErr("");
    setCommentsBusy(false);
    setCommentsMoreBusy(false);
    setCommentsDone(false);
    // [lc-1412] 评论（长影评）同样重置
    reviewsOffset.current = 0;
    reviewsDoneRef.current = false;
    reviewsBusyRef.current = false;
    setReviews([]);
    setReviewsErr("");
    setReviewsBusy(false);
    setReviewsMoreBusy(false);
    setReviewsDone(false);
    moreLoadedRef.current = false;
    setChars([]);
    setPersons([]);
    setMoreErr("");
    setMoreBusy(false);
    setMoreProbed(false);
    creditsLoadedRef.current = false;
    setCredits(null);
    setCreditsErr("");
    setCreditsBusy(false);
    relatedLoadedRef.current = false;
    setRelated([]);
    setRelatedErr("");
    setRelatedBusy(false);
    setBgmEps([]);
    setBgmSub(null);
    setZoom("");
  }, [base.guid]);

  // [lc-1401] 条目详情（bgm_subject）：新版式顶部的「标签框」用它（tags[]），**必须一开始就有**，
  // 所以不再等用户点某个标签页 —— 匹配 + 详情两次请求在进页面时就发（都走镜像 + 落盘缓存：
  // match 30 天、subject 6h），首屏不等它们（标签框自己显示骨架屏）。
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        let m = bgmRef.current;
        if (!m) {
          m = await resolveBgm();
          if (!alive) return;
          setBgmProbed(true);
        }
        if (!m) return;
        const s = await invoke<Record<string, unknown>>("bgm_subject", { id: m.subject_id });
        if (alive && s && !(s as { error?: unknown }).error) setBgmSub(s);
      } catch {
        /* 标签框拉不到就空着（下面的各标签页有各自的失败重试），绝不阻塞首屏 */
      }
    })();
    return () => {
      alive = false;
    };
  }, [base.guid, data]);

  // [lc-1412] 评论（长影评）懒加载：与吐槽同款（切到该页才发、复用 bgmRef 匹配、nonce 重试），
  // 数据源换成 /p1/.../reviews。与吐槽**互相独立**：各自有自己的 offset/到底/错误态。
  useEffect(() => {
    if (tab !== "reviews") return;
    if (reviewsOffset.current > 0) return;
    let alive = true;
    (async () => {
      reviewsBusyRef.current = true;
      setReviewsBusy(true);
      setReviewsErr("");
      try {
        let m = bgmRef.current;
        if (!m) {
          m = await resolveBgm();
          if (!alive) return;
          setBgmProbed(true);
        }
        if (!m) throw new Error("未匹配到 Bangumi 条目");
        const r = await invoke<{ data?: BgmReview[] }>("bgm_reviews", {
          id: m.subject_id,
          offset: 0,
          limit: BGM_PAGE,
        });
        const list = Array.isArray(r?.data) ? r.data : [];
        if (!alive) return;
        reviewsOffset.current = list.length;
        setReviews(list);
        if (list.length === 0) {
          reviewsDoneRef.current = true;
          setReviewsDone(true);
        }
      } catch (e) {
        if (alive) {
          setBgmProbed(true);
          setReviewsErr(String(e));
        }
      } finally {
        reviewsBusyRef.current = false;
        if (alive) setReviewsBusy(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [tab, reviewsNonce, base.guid]);

  // [lc-119] 吐槽懒加载（只在切到「吐槽」分段时才开始）：先 bgm_match（结果按 base.guid 落盘缓存），
  // 再拉第一页 /p1/.../comments。重试按钮通过 commentsNonce 重新走这段。已有数据则不重复拉。
  useEffect(() => {
    if (tab !== "comments") return;
    if (commentsOffset.current > 0) return; // 已拉到过数据（含翻页）→ 不重拉
    let alive = true;
    (async () => {
      commentsBusyRef.current = true;
      setCommentsBusy(true);
      setCommentsErr("");
      try {
        let m = bgmRef.current;
        if (!m) {
          m = await resolveBgm();
          if (!alive) return;
          setBgmProbed(true);
        }
        if (!m) throw new Error("未匹配到 Bangumi 条目");
        const r = await invoke<{ data?: BgmComment[] }>("bgm_comments", {
          id: m.subject_id,
          offset: 0,
          limit: BGM_PAGE,
        });
        const list = Array.isArray(r?.data) ? r.data : [];
        if (!alive) return;
        commentsOffset.current = list.length; // ← 实际返回条数（不假定 limit 生效）
        setComments(list);
        if (list.length === 0) {
          commentsDoneRef.current = true;
          setCommentsDone(true);
        }
      } catch (e) {
        if (alive) {
          setBgmProbed(true);
          setCommentsErr(String(e));
        }
      } finally {
        commentsBusyRef.current = false;
        if (alive) setCommentsBusy(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [tab, commentsNonce, base.guid]);

  // 更多分段懒加载（只在切到「更多」时才开始）：复用 bgm 匹配结果（bgmRef）——
  // 吐槽先点过就直接用，两边都没匹配过才发 bgm_match（同 title/年份/type/cacheKey 参数）。
  // 注意：这里**不**置 bgmProbed —— 那个标志的语义是「吐槽已探测」，是「吐槽」分段的入口闸门。
  // 角色/制作人员一次拉两条（同 subject）：任一失败整体失败，重试按钮走 moreNonce 重走本段。
  useEffect(() => {
    if (tab !== "more") return;
    if (moreLoadedRef.current) return; // 已拉到过 → 不重拉
    let alive = true;
    (async () => {
      setMoreBusy(true);
      setMoreErr("");
      try {
        let m = bgmRef.current;
        if (!m) {
          m = await resolveBgm();
          if (!alive) return;
        }
        if (!m) throw new Error("未匹配到 Bangumi 条目");
        const [cr, pr] = await Promise.all([
          invoke<BgmCharacter[]>("bgm_characters", { id: m.subject_id }),
          invoke<BgmPerson[]>("bgm_persons", { id: m.subject_id }),
        ]);
        if (!alive) return;
        setChars(bgmList<BgmCharacter>(cr));
        setPersons(bgmList<BgmPerson>(pr));
        moreLoadedRef.current = true;
      } catch (e) {
        if (alive) setMoreErr(String(e));
      } finally {
        if (alive) {
          setMoreBusy(false);
          setMoreProbed(true);
        }
      }
    })();
    return () => {
      alive = false;
    };
  }, [tab, moreNonce, base.guid]);

  /** 失败重试：清掉错误从头发一次（数据成功前 moreLoadedRef 恒为 false，无需额外重置） */
  const retryMore = () => setMoreNonce((n) => n + 1);

  // [lc-1401] 关联条目懒加载：切到「关联条目」才发。复用 bgmRef（吐槽/更多已匹配过就直接用），
  // 都没匹配过才发 bgm_match —— 三条懒加载路径共用同一份匹配结果的模式，与「更多」一致。
  useEffect(() => {
    if (tab !== "related") return;
    if (relatedLoadedRef.current) return;
    let alive = true;
    (async () => {
      setRelatedBusy(true);
      setRelatedErr("");
      try {
        let m = bgmRef.current;
        if (!m) {
          m = await resolveBgm();
          if (!alive) return;
        }
        if (!m) throw new Error("未匹配到 Bangumi 条目");
        const r = await invoke<BgmRelated[]>("bgm_related", { id: m.subject_id });
        if (!alive) return;
        setRelated(bgmList<BgmRelated>(r));
        relatedLoadedRef.current = true;
      } catch (e) {
        if (alive) setRelatedErr(String(e));
      } finally {
        if (alive) setRelatedBusy(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [tab, relatedNonce, base.guid]);

  /** 失败重试（关联条目）：与 retryMore 同款 —— 成功前 relatedLoadedRef 恒为 false */
  const retryRelated = () => setRelatedNonce((n) => n + 1);

  // [lc-128] 演员表（**TMDB 主源**）：切到「更多」分段时懒加载一次；锚点优先（trim_id/tmdbId/imdbId），
  // 锚不到由 Rust 侧回落标题搜索（带相似度门槛）。结果在 Rust 侧落盘缓存 7 天 → 重复进详情页零请求。
  // 失败不影响 Bangumi/飞牛兜底展示（各自独立状态机）。
  useEffect(() => {
    if (tab !== "more") return;
    if (creditsLoadedRef.current) return;
    let alive = true;
    (async () => {
      setCreditsBusy(true);
      setCreditsErr("");
      try {
        const title = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
        const r = await invoke<TmdbCredits>("tmdb_credits", {
          arg: {
            title,
            year: String(base.release_date || "").slice(0, 4),
            mediaType: base.type === "Movie" ? "movie" : "tv",
            trimId: tmdbAnchor.trimId || undefined,
            tmdbId: noFnOS
              ? base.tmdb_id
                ? String(base.tmdb_id)
                : undefined // 右栏新番条目没有 TMDB id → 走标题搜索
              : tmdbAnchor.tmdbId || undefined,
            imdbId: tmdbAnchor.imdbId || undefined,
            cacheKey: base.guid,
          },
        });
        if (!alive) return;
        setCredits(r);
        creditsLoadedRef.current = true;
      } catch (e) {
        if (alive) setCreditsErr(String(e));
      } finally {
        if (alive) setCreditsBusy(false);
      }
    })();
    return () => {
      alive = false;
    };
    // data 到位后才拿得到 trim_id 锚点：进「更多」早于详情返回时，数据一到会带着锚点重跑一次
  }, [tab, creditsNonce, base.guid, data, tmdbAnchor.trimId, tmdbAnchor.tmdbId, tmdbAnchor.imdbId]);

  /** [lc-128] 失败重试：清掉错误从头发一次（成功前 creditsLoadedRef 恒为 false） */
  const retryCredits = () => setCreditsNonce((n) => n + 1);

  /** [lc-1412] 评论翻页：与吐槽同款（offset += 实际返回条数，0 条即到底） */
  const loadMoreReviews = async () => {
    const m = bgmRef.current;
    if (!m || reviewsBusyRef.current || reviewsDoneRef.current) return;
    reviewsBusyRef.current = true;
    setReviewsMoreBusy(true);
    setReviewsErr("");
    try {
      const r = await invoke<{ data?: BgmReview[] }>("bgm_reviews", {
        id: m.subject_id,
        offset: reviewsOffset.current,
        limit: BGM_PAGE,
      });
      const list = Array.isArray(r?.data) ? r.data : [];
      reviewsOffset.current += list.length;
      setReviews((prev) => [...prev, ...list]);
      if (list.length === 0) {
        reviewsDoneRef.current = true;
        setReviewsDone(true);
      }
    } catch (e) {
      setReviewsErr(String(e));
    } finally {
      reviewsBusyRef.current = false;
      setReviewsMoreBusy(false);
    }
  };

  /** [lc-1412] 评论重试：已有列表续拉，空列表从头（含重新匹配） */
  const retryReviews = () => {
    if (reviews.length > 0) {
      setReviewsErr("");
      void loadMoreReviews();
    } else {
      setReviewsNonce((n) => n + 1);
    }
  };

  /** [lc-119] 翻下一页：offset += 实际返回条数；返回 0 条即到底（不再假定 limit/总数） */
  const loadMoreComments = async () => {
    const m = bgmRef.current;
    if (!m || commentsBusyRef.current || commentsDoneRef.current) return;
    commentsBusyRef.current = true;
    setCommentsMoreBusy(true);
    setCommentsErr("");
    try {
      const r = await invoke<{ data?: BgmComment[] }>("bgm_comments", {
        id: m.subject_id,
        offset: commentsOffset.current,
        limit: BGM_PAGE,
      });
      const list = Array.isArray(r?.data) ? r.data : [];
      commentsOffset.current += list.length; // ← 实际返回条数
      setComments((prev) => [...prev, ...list]);
      if (list.length === 0) {
        commentsDoneRef.current = true;
        setCommentsDone(true);
      }
    } catch (e) {
      setCommentsErr(String(e));
    } finally {
      commentsBusyRef.current = false;
      setCommentsMoreBusy(false);
    }
  };

  /** [lc-119] 失败重试：已有列表 → 从当前 offset 续拉；空列表 → 重新从头（含重新匹配） */
  const retryComments = () => {
    if (comments.length > 0) {
      setCommentsErr("");
      void loadMoreComments();
    } else {
      setCommentsNonce((n) => n + 1);
    }
  };

  /** [lc-119] 滚动到底自动加载下一页：extentAfter = scrollHeight - scrollTop - clientHeight < 200 */
  const onDetailScroll = (e: UIEvent<HTMLElement>) => {
    // [lc-1412] 评论 / 吐槽两页各自触底翻页（互不影响）
    const el = e.currentTarget;
    if (el.scrollHeight - el.scrollTop - el.clientHeight >= 200) return;
    if (activeTab === "comments") {
      if (!bgm || commentsDone || commentsErr || commentsMoreBusy) return;
      void loadMoreComments();
    } else if (activeTab === "reviews") {
      if (!bgm || reviewsDone || reviewsErr || reviewsMoreBusy) return;
      void loadMoreReviews();
    }
  };

  // [lc-119] 单集简介回填（替换旧版 —— 唯一允许改动的既有逻辑）：
  // 旧版对整季每集 **无并发上限** 地并发 invoke("get_item_detail")（Promise.allSettled(episodes.map(...))），
  // 长篇（几十集）会瞬间打出几十个请求把 NAS/代理打爆 —— 与本项目 lc-114 图片并发风暴同源。
  // 新逻辑（按规格）：
  //   (1) 已匹配到 Bangumi（bgm 就绪）→ 用一次 bgm_episodes 的 desc 按 episode_number 对齐回填（单请求）；
  //   (2) 未匹配/尚未匹配（吐槽是懒加载，可能一直没匹配）→ 保留原 item 详情回填，但加 **并发上限 3**（worker 池）；
  //      worker 每轮先看 bgmRef：匹配一到位就退场，把补齐工作交给 (1) 的 desc（不再堆 NAS 请求）。
  useEffect(() => {
    if (!episodes.length) return;
    let alive = true;
    const missing = episodes.filter((ep) => !ep.overview);
    if (!missing.length) return;

    if (bgm?.subject_id) {
      invoke<{ data?: BgmEpisodeRow[] }>("bgm_episodes", {
        id: bgm.subject_id,
        limit: 200,
      })
        .then((r) => {
          if (!alive) return;
          const rows = Array.isArray(r?.data) ? r.data : [];
          if (rows.length) setBgmEps(rows); // [lc-1401] 右栏选集用它拿「第N话 小标题 + 单集简介」
          const byNum = new Map<number, string>();
          for (const e of rows) {
            // Bangumi 集号：sort 是数字序号（ep 是展示号，可能带小数/字母），优先 sort
            const num = Number.isInteger(e?.sort)
              ? (e.sort as number)
              : Number.isInteger(e?.ep)
                ? (e.ep as number)
                : NaN;
            const desc = String(e?.desc ?? "").trim();
            if (Number.isFinite(num) && desc) byNum.set(num, desc);
          }
          if (!byNum.size) return;
          setEpisodes((prev) =>
            prev.map((ep) => {
              if (ep.overview || ep.episode_number == null) return ep;
              const d = byNum.get(ep.episode_number);
              return d ? { ...ep, overview: d } : ep;
            }),
          );
        })
        .catch(() => {});
      return () => {
        alive = false;
      };
    }

    const queue = [...missing];
    const worker = async () => {
      while (alive && queue.length) {
        if (bgmRef.current) return; // (1) 就绪 → legacy 任务立即退场
        const ep = queue.shift()!;
        try {
          const d = await invoke<Record<string, unknown>>("get_item_detail", { guid: ep.guid });
          const ov = pick(d, ["overview"]);
          if (alive && ov) {
            setEpisodes((prev) =>
              prev.map((x) => (x.guid === ep.guid && !x.overview ? { ...x, overview: String(ov) } : x)),
            );
          }
        } catch {
          /* 单集失败忽略（与原 allSettled 语义一致） */
        }
      }
    };
    void Promise.all([worker(), worker(), worker()]); // 并发上限 3
    return () => {
      alive = false;
    };
  }, [episodes.length, bgm?.subject_id]);

  const title = String(pick(data ?? {}, ["tvTitle", "title"]) ?? base.title);
  // [lc-1433] 竖版海报：**TMDB 高清优先**（用户要求"直接取 tmdb 的或者 bgm 的高清大图"）。
  //   取值顺序：TMDB images.posters 面积最大的那张（Rust 的 heroPoster，拼 t/p/original）
  //   → 飞牛 posters（数组取最大 / 字符串直取）→ 列表页带来的 base.poster → 空（再兜 BGM 封面）。
  //   为什么换源：飞牛的 posters 常是**单个字符串**（它自己抓的小 webp），选无可选、放大必糊。
  //   图片仍走 fntvimg 代理，**拉了即落盘**，不会因为换档位而重复下载。
  const tmdbHeroPoster = String((tmdb ?? {}).heroPoster ?? "");
  // 档位选 w500 而不是 original：本海报的**显示尺寸只有 132×198**（.dv2-poster），
  //   w500 已是 3.8 倍密度、放大到极限也看不出差别，而 original（2000×3000，实测 ~260KB）
  //   纯属浪费 —— 多出的解码时间与内存都花在把 2000px 缩到 132px 上。
  //   （轮播 hero 那边是满屏宽度，才必须用 original。）
  const poster =
    (tmdbHeroPoster ? tmdbImg(tmdbHeroPoster, "w500") : "") ||
    pickLargestImg((data ?? {}).posters) ||
    (pick(data ?? {}, ["posters", "poster"]) as string) ||
    base.poster ||
    "";
  // [lc-1507] 简介兜底链：飞牛详情 → 飞牛条目自带 → **Bangumi subject.summary**（发现页
  //   Bangumi 条目 TMDB 匹配不上时，简介不至于空白 —— bgmSub 进页面就拉了）
  const bgmSummary = String(pick(bgmSub ?? {}, ["summary"]) ?? "").trim();
  const overview = (() => {
    const a = String(pick(data ?? {}, ["overview"]) ?? "").trim();
    if (a) return a;
    const b = String(base.overview ?? "").trim();
    if (b) return b;
    return bgmSummary;
  })();
  // [lc-1401] 原题名（有就别名不显示）：优先条目详情，其次 Bangumi 匹配结果
  const originalTitle = String(
    pick(data ?? {}, ["originalTitle", "original_title", "originalName"]) ?? bgm?.name ?? "",
  ).trim();
  // [lc-1401] Bangumi 封面（本机没海报时的兜底；走 fntvimg 的 /__bgm 代理，拉了即落盘）
  const bgmCover = bgmImg(
    bgm?.images?.large || bgm?.images?.common || bgm?.images?.medium || bgm?.images?.grid || "",
  );
  const resumeAt = base.ts && base.duration && base.ts > 0 && base.ts < base.duration;

  // [lc-1401] 新版式删掉了通栏头图（backdrop 不再用于详情页背景），
  //   但「剧照」一节仍取 TMDB 的 backdrops —— 那是 tmdb.images.backdrops，与本机条目的 backdrop 不是一回事。
  // [lc-1408] 原「年份 / 季数 / 集数 / 类型 / 评分 / 播放按钮」那组派生值随红框一起删掉了
  //   （用户定稿：那块的原有内容全部不要，改放标签与简介）。播放按钮挪到海报上，仍用 resumeAt。
  const tmdbGenres: string[] = Array.isArray(tmdb?.genres)
    ? (tmdb?.genres as unknown[]).map((x) => String(x))
    : [];
  const genres = Array.from(
    new Set((tmdbGenres.length ? tmdbGenres : pickGenres(data)).filter(Boolean)),
  ).slice(0, 6);
  // [lc-1404] 剧照列表不再在这里派生：它只被 TmdbExtra 内部用（那边自己从 tmdb.backdrops 取），
  //   留着一份"渲染过一次的剧照数组"正是上版**两组剧照**的来源。
  // [lc-128] 演员/制作人员三级来源（**TMDB 主源**）：
  //   (1) tmdb_credits（专用命令，锚点优先 + 落盘缓存 7 天）
  //   (2) tmdb_show_details 里同源的 cast/crew（(1) 失败时兜底，已在内存里，零额外请求）
  //   (3) 飞牛 item 详情里的 actors/cast（有则用、没有不报错）
  const creditsCast = Array.isArray(credits?.cast) ? (credits?.cast as TmdbCreditPerson[]) : [];
  const creditsCrew = Array.isArray(credits?.crew) ? (credits?.crew as TmdbCreditPerson[]) : [];
  const showCast: TmdbCreditPerson[] = Array.isArray(tmdb?.cast)
    ? (tmdb?.cast as TmdbCreditPerson[])
    : [];
  const showCrew: TmdbCreditPerson[] = Array.isArray(tmdb?.crew)
    ? (tmdb?.crew as TmdbCreditPerson[])
    : [];
  const cast = creditsCast.length ? creditsCast : showCast;
  const crew = creditsCrew.length ? creditsCrew : showCrew;
  const castFrom: "credits" | "details" | "none" = creditsCast.length
    ? "credits"
    : showCast.length
      ? "details"
      : "none";
  const fbCast = fnosCast(data);
  // [lc-1401] 分段栏（用户定稿版式：四个标签页，顺序固定）
  //   剧照剧集详情信息 / 吐槽评论 / 关联条目 / 演职人员 —— 全部**恒在**：
  //   版式要求标签条是固定的一条（不是按"有没有数据"增删），所以不再自适应增删项；
  //   无数据的分段自己显示空态（各自都有骨架屏/失败重试/空态三态）。
  //   「选集」已移出标签条 —— 按版式常驻右栏（每集小标题 + 简介的竖向列表）。
  const tabIds: string[] = ["overview", "reviews", "comments", "related", "more"];
  // [lc-1423] 简介是否需要"展开"：与之前同一套字数近似（>96 字≈4~5 行）
  const longIntro = overview.length > 96;
  // Esc 关简介弹窗（与项目其它弹层同一套键盘约定）
  useEffect(() => {
    if (!introFull) return;
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") setIntroFull(false);
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [introFull]);
  const activeTab = tabIds.includes(tab) ? tab : "overview";
  // [lc-1405] 标签框数据源：**Bangumi tags[] 优先，TMDB genres 兜底**（用户要求"直接取 bgm 或 tmdb 的"）。
  //   ⚠ 不再用飞牛 item 的 tags —— 那是刮削元信息字典（含"话数"这类），不是题材标签，
  //   之前它经 pickGenres 混进来，就把 "13"（话数）当标签显示了（用户反馈的现象）。
  const bgmRawTags: BgmTag[] = Array.isArray((bgmSub as Record<string, unknown> | null)?.tags)
    ? ((bgmSub as Record<string, unknown>).tags as BgmTag[])
    : [];
  // TMDB 的题材（tmdb.genres 已在上面解析成 genres: string[]）；两边都空 → 标签框整块不渲染
  const bgmTags: BgmTag[] = bgmRawTags.length
    ? bgmRawTags
    : genres.map((g) => ({ name: g }));
  // [lc-1401] 关联条目按 relation 分组（上游原序即热度序，分组内不重排）
  const relatedGroups: { relation: string; items: BgmRelated[] }[] = (() => {
    const map = new Map<string, BgmRelated[]>();
    for (const r of related) {
      const k = String(r.relation || "其它").trim() || "其它";
      const arr = map.get(k);
      if (arr) arr.push(r);
      else map.set(k, [r]);
    }
    return [...map.entries()].map(([relation, items]) => ({ relation, items }));
  })();
  // [lc-1419] 选集头部的「集数 / 完结状态 / 上次更新」三项（用户要求）。
  //   数据源与判据：
  //   · 总集数：Bangumi subject 的 eps/total_episodes（实测 13），本机集数作为回退；
  //   · 是否完结：infobox 里**有「放送结束」即视为已完结**（实测已完结条目有该键、在播没有）——
  //     比"集数是否相等"稳（有些番本机只有部分集，拿本机数判会误报"未完结"）；
  //   · 上次更新：本机集里最新的一集播出日期（air_date），没有就回退 Bangumi subject.date。
  const subjectEps = Number(
    (bgmSub as Record<string, unknown> | null)?.eps ??
      (bgmSub as Record<string, unknown> | null)?.total_episodes ??
      0,
  );
  const totalEps = subjectEps > 0 ? subjectEps : episodes.length;
  const infoBox = ((bgmSub as Record<string, unknown> | null)?.infobox ?? []) as {
    key?: string;
    value?: unknown;
  }[];
  const finished = infoBox.some((b) => String(b?.key ?? "").includes("放送结束"));
  const airDates = episodes.map((e) => String(e.air_date ?? "")).filter(Boolean).sort();
  const lastAir = airDates.length ? airDates[airDates.length - 1] : String(pick(bgmSub ?? {}, ["date"]) ?? "");

  // [lc-1419] 续播目标：有播放记录（ts>0 且未看完）就跳那一集并带进度，
  //   否则从第 1 集开始。原「海报上的播放按钮」用的就是 episodes[0]，这里升级为"上次的位置"。
  const resumeEp =
    episodes.find((e) => (e.ts ?? 0) > 0 && !(e.duration && e.ts! >= e.duration)) ?? episodes[0];
  const resumeTarget = resumeEp ?? null;
  // 进度百分比（给按钮上的小字用）
  const resumePct =
    resumeTarget && resumeTarget.ts && resumeTarget.duration && resumeTarget.duration > 0
      ? Math.min(100, Math.round((resumeTarget.ts / resumeTarget.duration) * 100))
      : 0;

  // [lc-1401] 右栏选集：本机集号 → Bangumi 那集的 {小标题, 简介}（集号对齐 sort 优先，与回填逻辑一致）
  const bgmEpByNum = new Map<number, BgmEpisodeRow>();
  for (const e of bgmEps) {
    const num = Number.isInteger(e?.sort) ? (e.sort as number) : Number.isInteger(e?.ep) ? (e.ep as number) : NaN;
    if (Number.isFinite(num) && !bgmEpByNum.has(num)) bgmEpByNum.set(num, e);
  }


  // [lc-106] 独立 mpv 窗口模式：点播放直接拉起独立 mpv（与主仓库一致），页面留在详情页
  const [playNote, setPlayNote] = useState("");
  const [playing, setPlaying] = useState(false);
  const [playingGuid, setPlayingGuid] = useState<string | null>(null);
  const startPlay = async () => {
    const ep = episodes[0];
    const guid = isSeries && ep ? ep.guid : base.guid;
    setPlaying(true);
    setPlayNote("正在打开独立播放器…");
    try {
      await invoke("play_item", { guid });
      setPlayNote("已在独立播放器窗口播放（可回到本页继续浏览）");
    } catch (e) {
      setPlayNote(String(e));
    } finally {
      setPlaying(false);
    }
  };

  return (
    <main className="content detail-full" onScroll={onDetailScroll}>
      {/* [lc-1401] 详情页按**用户定稿版式**重排（两栏）：
            左列：竖版封面 + 标题信息 → 标签框（Bangumi tags）→ 剧集简介框 → 四标签页 → 该页内容
            右列：集数据（本机集数 × Bangumi 集号对齐）的**整条竖向列表**
          ⚠ 上一版的模糊大头图（.dhero）**已按新稿删除**：稿里首屏就是"封面 + 两块信息框"，没有通栏头图。
          数据 effect 全部留在本组件顶层（不下沉到面板里，否则单集简介回填失效）。 */}
      <div className="dv2-rail">
        <button className="back" onClick={onBack} title="返回上一页">
          ‹ 返回
        </button>
        {/* [lc-1403] 标题挪到返回键后面（用户要求）；原名（日文/英文）放标题后的括号里。
            原名优先 TMDB 的 original_name/original_title，其次 Bangumi 的 name —— 与当前显示名
            完全相同（或为空）时不显示括号，避免出现「进击的巨人（进击的巨人）」。 */}
        <h1 className="dv2-rail-title">
          {title || "（无标题）"}
          {originalTitle && originalTitle !== title && (
            <span className="dv2-rail-orig">（{originalTitle}）</span>
          )}
        </h1>
      </div>

      {err && <div className="err">{err}</div>}

      {/* [lc-1417] 首屏骨架屏：条目详情（data）没到位之前先摆骨架，别让用户对着空白等。
          判据用 `!dataReady && !err`：出错时走上面的 .err，已有数据时立即换成真内容。
          [lc-1444] 发现页条目没有飞牛详情，dataReady 立即为真 —— 不等、不摆骨架。
          ⚠ 只包住"两栏主体"—— 返回键与标题（rail）是从 base 直接来的，不需要等网络。 */}
      {!dataReady && !err && <DetailSkeleton />}

      {/* ══ 两栏主体（用户版式）══ */}
      {dataReady && (
      <div className="dv2">
        {/* ── 左列：封面 / 标签框 / 简介框 / 标签页 / 内容 ── */}
        <div className="dv2-left">
          {/* 竖版封面 + 标题（海报来自本机条目，无则用 Bangumi 的） */}
          <div className="dv2-top">
            <div className="dv2-poster">
              {poster || bgmCover ? (
                <img src={poster ? imgUrl(poster) : bgmCover} alt={title} draggable={false} />
              ) : (
                <div className="poster-empty"><IcoFilmL size={22} strokeWidth={1.8} /></div>
              )}
              {/* [lc-1420] 海报上的播放按钮已删（用户要求）。
                  播放入口现在统一在**选集头部**（剧集），或右栏为空时的下方按钮（电影）——
                  见 aside 里的 .dv2e-play-btn / .dv2-solo-play。 */}
            </div>

            {/* [lc-1408] 海报右侧信息框 = **标签 + 简介**（用户定稿：这一块原来的
                年份/季数/集数/类型/评分/播放按钮全部不要）。 */}
            <div className="dv2-topinfo">
              {bgmTags.length > 0 && (
                <div className="dv2-itags">
                  {bgmTags.slice(0, 12).map((t, i) => (
                    <span key={`${t.name}-${i}`} className="dtag" title={t.count ? `${t.count} 人标记` : ""}>
                      {t.name}
                    </span>
                  ))}
                </div>
              )}
              {overview && (
                <div className="dv2-isum">
                  {/* [lc-1423] 改回"截断 + 展开"（用户要求），但这次**不再动容器高度**：
                      lc-1410 曾按"不要收起按钮、默认全展示"把简介全铺开，后果是长简介把
                      `.dv2-top` 撑高 —— 而海报是 `aspect-ratio:2/3` 的定宽块，
                      在 `align-items:stretch` 下被**拉长变形**（用户截图里海报就是被扯长的）。
                      现在：简介区**固定高度**、超出部分渐隐截断，右下角一个「展开」按钮；
                      点击 → 弹窗展示完整简介（不动海报、不撑容器）。 */}
                  <p className={`dfold${longIntro && !introFull ? " clip" : ""}`}>{overview}</p>
                  {longIntro && (
                    <button className="dv2-expand" onClick={() => setIntroFull(true)}>
                      展开
                    </button>
                  )}
                </div>
              )}
              {!bgmTags.length && !overview && <div className="empty">暂无标签与简介</div>}
              {playNote && <div className="play-note">{playNote}</div>}
            </div>
          </div>

          {/* 四标签页（固定一条，不随数据增删） */}
          <nav className="dseg dv2-seg" role="tablist">
            {tabIds.map((id) => (
              <button
                key={id}
                role="tab"
                aria-selected={activeTab === id}
                className={`dseg-btn${activeTab === id ? " active" : ""}`}
                onClick={() => setTab(id)}
              >
                {TAB_LABEL[id]}
              </button>
            ))}
          </nav>

          {/* 标签页内容 */}
          <div className={`dbody dv2-body${tabAnim ? " view-in" : ""}`}>
        {activeTab === "overview" && (
          /* [lc-1420] `.dpanel-single`：本页所有节包进**一个**容器、节间用浅横线分隔（用户要求）。
             见 App.css 里同名类的注释（为什么不用 :has）。 */
          <div className="dpanel dpanel-single">
            {/* [lc-1405] 原本这里还有一节「类型标签」渲染 genres —— 与左列顶部那个标签框**同一份数据**，
                重复；且用户已要求标签框直接取 BGM/TMDB，这一节就失去存在意义。故删除。 */}
            {/* [lc-1404] 剧照 / 分级 / 在线观看 / 相似 / 预告 / 别名关键词：**全部由 TmdbExtra 一处渲染**。
                此前这里是"自己一条 .dstill-strip + TmdbExtra 里再来一条 .tmdb-stills"，
                结果是同一批 TMDB 剧照出现**两组**（用户反馈）。现在只留 TmdbExtra 那一份。 */}
            {/* [lc-1420] 分块骨架：TMDB 详情也是一条独立请求，没回来之前这块是空的。
                摆"剧照 + 文本行"的骨架（形状与 TmdbExtra 的首节一致），到位时无跳动。
                [lc-1507] 骨架只在请求**进行中**（!tmdbDone）显示 —— 失败后不再永久转圈。 */}
            {!tmdb && !tmdbDone && (
              <>
                <div className="tmdb-sec">
                  <div className="dsk dsk-sec-t" />
                  {/* [lc-1505] 骨架容器用专用类：.dvstill-row 本身没有 flex（真实剧照行的
                      横排布局由 ScrollRow 提供），裸 div 会把骨架块竖着堆成一列 */}
                  <div className="dsk-still-row">
                    {[0, 1, 2, 3].map((i) => (
                      <div key={i} className="dsk dsk-dstill" />
                    ))}
                  </div>
                </div>
                <div className="tmdb-sec">
                  <div className="dsk dsk-sec-t" />
                  <div className="dsk dsk-sline" />
                  <div className="dsk dsk-sline short" />
                </div>
              </>
            )}
            {tmdb && <TmdbExtra tmdb={tmdb} onZoom={setZoom} />}
            {/* [lc-1507] TMDB 失败后的兜底：有简介（Bangumi）就显示简介节，
                什么都没有才显示空态 —— 原先骨架与「暂无详情信息」会同时出现 */}
            {!tmdb && tmdbDone && (
              <>
                {overview && (
                  <div className="tmdb-sec">
                    <div className="tmdb-sec-t">简介</div>
                    <p className="dfold">{overview}</p>
                  </div>
                )}
                {!overview && genres.length === 0 && (
                  <div className="empty">TMDB 与 Bangumi 都没有该条目的详情信息</div>
                )}
              </>
            )}
          </div>
        )}

        {/* [lc-1401] 关联条目（Bangumi /subjects/{id}/subjects）：**按 relation 分组**渲染
            （上游没有分组，只有每条的 relation 词）。组序 = 上游首次出现顺序（热度序）。 */}
        {activeTab === "related" && (
          <div className="dpanel">
            {relatedBusy && !relatedErr && <BgmListSkeleton />}
            {relatedErr && (
              <div className="bgmc-state">
                <div className="bgmc-state-t">关联条目加载失败</div>
                <div className="bgmc-state-d">{relatedErr}</div>
                <button className="bgmc-retry" onClick={retryRelated}>
                  重试
                </button>
              </div>
            )}
            {!relatedBusy && !relatedErr && related.length === 0 && (
              <div className="empty">暂无关联条目</div>
            )}
            {/* [lc-1411] 按用户定稿版式重做：**横向分栏**（一列 = 一个关系），
                  列内竖向排卡片，列间竖分隔线；整条横向可翻页（复用 ScrollRow 的箭头）。
                  与旧版的区别：旧版是"每个关系一节、节内网格"，改成一列一节后，
                  同一屏能同时看到「续集 / 书籍 / 游戏 / 原声集 / 片头曲…」的对照关系。 */}
            <ScrollRow boxClass="drel-cols">
              {relatedGroups.map((g) => (
                <section className="drel-col" key={g.relation}>
                  <h3 className="drel-col-h">
                    {g.relation}
                    <span className="drel-col-n">{g.items.length}</span>
                  </h3>
                  {g.items.map((r, i) => {
                    const cover = r.images?.large || r.images?.common || r.images?.medium || r.images?.grid || "";
                    const nm = String(r.name_cn || r.name || "").trim() || "（无标题）";
                    return (
                      <div className="drel-card" key={`${r.id ?? i}`} title={String(r.name || "")}>
                        <div className="drel-cv">
                          {cover ? (
                            <img src={bgmImg(cover)} alt="" loading="lazy" draggable={false} />
                          ) : (
                            <div className="poster-empty"><IcoFilmL size={16} strokeWidth={1.8} /></div>
                          )}
                        </div>
                        <div className="drel-nm">{nm}</div>
                      </div>
                    );
                  })}
                </section>
              ))}
            </ScrollRow>
          </div>
        )}

        {/* 演职人员：**TMDB 主源**（tmdb_credits 落盘缓存）→ Bangumi 角色/制作人员 → 飞牛演员字段兜底。
            [lc-1404] 本页**不含** TmdbExtra（分级/在线观看/相似/别名那些属于「剧照剧集详情信息」）——
            用户反馈"演职人员下面又接了一个 tmdb 数据卡"，就是它原来挂在这里造成的重复。 */}
        {activeTab === "more" && (
          <div className="dpanel dmore">
            {/* (1) 演员表（TMDB 主源）：骨架 / 失败重试 / 数据 */}
            {creditsBusy && !credits && !creditsErr && <BgmListSkeleton />}
            {creditsErr && !creditsCast.length && !showCast.length && (
              <div className="bgmc-state">
                <div className="bgmc-state-t">演员表（TMDB）加载失败</div>
                <div className="bgmc-state-d">{creditsErr}</div>
                <button className="bgmc-retry" onClick={retryCredits}>
                  重试
                </button>
              </div>
            )}
            {cast.length > 0 && (
              <section className="dsec">
                <h2 className="dsec-t">
                  演职人员
                  <span className="dsec-src" title={credits?.matchedBy || (castFrom === "details" ? "TMDB 详情接口" : "")}>
                    TMDB{credits?.tmdbId ? ` #${credits.tmdbId}` : ""}
                    {credits?.cached ? " · 本地缓存" : ""}
                  </span>
                </h2>
                <ul className="bgmc-list">
                  {cast.map((c, i) => (
                    <BgmCastRow
                      key={`${c.id ?? c.name}-${i}`}
                      name={c.name}
                      tag={c.character ? `饰 ${c.character}` : ""}
                      sub={Number(c.episodes ?? 0) > 0 ? `出演 ${c.episodes} 集` : ""}
                      img={c.profile ? tmdbImg(String(c.profile), "w185") : ""}
                      fallback="未知演员"
                    />
                  ))}
                </ul>
              </section>
            )}
            {crew.length > 0 && (
              <section className="dsec">
                <h2 className="dsec-t">
                  制作
                  <span className="dsec-src">TMDB</span>
                </h2>
                <ul className="bgmc-list">
                  {crew.map((c, i) => (
                    <BgmCastRow
                      key={`${c.id ?? c.name}-${i}`}
                      name={c.name}
                      tag={c.jobLabel || c.job || ""}
                      sub={Number(c.episodes ?? 0) > 0 ? `${c.episodes} 集` : ""}
                      img={c.profile ? tmdbImg(String(c.profile), "w185") : ""}
                      fallback="未知"
                    />
                  ))}
                </ul>
              </section>
            )}
            {/* (2) 飞牛演员字段（**只兜底**：TMDB 完全没数据时才显示；没有该字段则整块不出现） */}
            {!creditsBusy && !cast.length && !crew.length && fbCast.length > 0 && (
              <section className="dsec">
                <h2 className="dsec-t">
                  演职人员
                  <span className="dsec-src">飞牛</span>
                </h2>
                <ul className="bgmc-list">
                  {fbCast.map((c, i) => (
                    <BgmCastRow
                      key={`${c.name}-${i}`}
                      name={c.name}
                      tag={c.role ? `饰 ${c.role}` : ""}
                      img={c.avatar ? (c.avatar.startsWith("http") ? c.avatar : imgUrl(c.avatar)) : ""}
                      fallback="未知演员"
                    />
                  ))}
                </ul>
              </section>
            )}
            {moreBusy && !moreErr && <BgmListSkeleton />}
            {moreErr && (
              <div className="bgmc-state">
                <div className="bgmc-state-t">角色与制作人员加载失败</div>
                <div className="bgmc-state-d">{moreErr}</div>
                <button className="bgmc-retry" onClick={retryMore}>
                  重试
                </button>
              </div>
            )}
            {!moreBusy &&
              !moreErr &&
              moreProbed &&
              chars.length === 0 &&
              persons.length === 0 &&
              !creditsBusy &&
              cast.length === 0 &&
              crew.length === 0 &&
              fbCast.length === 0 && <div className="empty">暂无角色与制作人员信息</div>}
            {chars.length > 0 && (
              <section className="dsec">
                <h2 className="dsec-t">
                  角色
                  <span className="dsec-src">Bangumi</span>
                </h2>
                <ul className="bgmc-list">
                  {chars.map((c, i) => (
                    <BgmCastRow
                      key={String(c.id ?? i)}
                      name={c.name}
                      tag={c.relation || CHAR_TYPE_LABEL[Number(c.type)] || ""}
                      sub={c.actors?.[0]?.name ? `CV：${c.actors[0].name}` : ""}
                      avatar={c.images?.large || c.images?.grid || c.images?.medium || c.images?.small}
                      fallback="未知角色"
                    />
                  ))}
                </ul>
              </section>
            )}
            {/* 制作人员（Bangumi）：TMDB 已有 crew 时不再重复展示同一批人 */}
            {persons.length > 0 && crew.length === 0 && (
              <section className="dsec">
                <h2 className="dsec-t">
                  制作人员
                  <span className="dsec-src">Bangumi</span>
                </h2>
                <ul className="bgmc-list">
                  {persons.map((p, i) => (
                    <BgmCastRow
                      key={String(p.id ?? i)}
                      name={p.name}
                      tag={p.relation || (Array.isArray(p.career) ? p.career.join(" / ") : "")}
                      avatar={p.images?.large || p.images?.grid || p.images?.medium || p.images?.small}
                      fallback="未知"
                    />
                  ))}
                </ul>
              </section>
            )}
          </div>
        )}

        {/* [lc-1412] 评论（长影评，用户设计稿版式）：
              封面 + 标题（链接色）+ 摘要（3 行截断）+ 作者 · 日期 · N 回复。
              数据源 /p1/.../reviews；封面在 entry.icon —— **多数是 no_photo.png 占位图**，
              所以碰到 no_photo 就退成图标（否则整列都是灰方块，很难看）。 */}
        {activeTab === "reviews" && (
          <div className="dpanel dvrev">
            {reviewsBusy && reviews.length === 0 && !reviewsErr && <BgmListSkeleton />}
            {reviewsErr && (
              <div className="bgmc-state">
                <div className="bgmc-state-t">评论加载失败</div>
                <div className="bgmc-state-d">{reviewsErr}</div>
                <button className="bgmc-retry" onClick={retryReviews}>
                  重试
                </button>
              </div>
            )}
            {!reviewsBusy && !reviewsErr && reviews.length === 0 && (
              <div className="empty">暂无评论</div>
            )}
            {reviews.map((r) => {
              const icon = String(r.entry?.icon ?? "");
              const hasCover = !!icon && !/no_photo/i.test(icon);   // 占位图不当封面用
              const date = fmtBgmTime(r.entry?.createdAt ?? r.entry?.updatedAt);
              const replies = Number(r.entry?.replies ?? 0) || 0;
              return (
                <article className="dvrev-item" key={String(r.id)}>
                  <div className="dvrev-cv">
                    {hasCover ? (
                      <img src={bgmImg(icon)} alt="" loading="lazy" draggable={false} />
                    ) : (
                      <span className="dvrev-cv-ph" aria-hidden="true">
                        <IcoMsgL size={20} strokeWidth={1.7} />
                      </span>
                    )}
                  </div>
                  <div className="dvrev-main">
                    <h3 className="dvrev-t">{String(r.entry?.title || "（无标题）")}</h3>
                    {r.entry?.summary && <p className="dvrev-sum">{String(r.entry.summary)}</p>}
                    <div className="dvrev-meta">
                      <span className="dvrev-user">{r.user?.nickname || r.user?.username || "匿名"}</span>
                      {date && <><span className="dvrev-dot">·</span><span>{date}</span></>}
                      <span className="dvrev-dot">·</span>
                      <span>{replies} 回复</span>
                    </div>
                  </div>
                </article>
              );
            })}
            {reviews.length > 0 && reviewsMoreBusy && <div className="bgmc-more">加载中…</div>}
            {reviews.length > 0 && reviewsDone && <div className="bgmc-more">没有更多评论了</div>}
            {!reviewsDone && reviews.length > 0 && !reviewsMoreBusy && (
              <div className="dvrev-more" onClick={() => void loadMoreReviews()}>
                更多影评 ›
              </div>
            )}
          </div>
        )}

        {/* [lc-119] 吐槽（Bangumi 短评，走自建镜像）：骨架屏 / 失败重试 / 空态 / 滚到底翻页
            [lc-1412] 版式按用户设计稿改成**气泡流**（原为头像+昵称+正文的列表行）：
            每条是一张带边框的气泡卡，右上角心形（reactions 有数据时点亮并显示人数），
            昵称/星级/时间一行在气泡上方。
            ⚠ **做不出嵌套回复**：Bangumi 的 /comments 不提供回复链（50 条样本无 parent 字段，
              parent_id 参数无效、三个候选回复端点全 404），所以这里只有平铺的评论流。 */}
        {activeTab === "comments" && (
          <div className="dpanel bgmc-panel">
            {commentsBusy && comments.length === 0 && !commentsErr && <BgmListSkeleton />}
            {commentsErr && (
              <div className="bgmc-state">
                <div className="bgmc-state-t">吐槽加载失败</div>
                <div className="bgmc-state-d">{commentsErr}</div>
                <button className="bgmc-retry" onClick={retryComments}>
                  重试
                </button>
              </div>
            )}
            {!commentsBusy && !commentsErr && comments.length === 0 && (
              <div className="empty">暂无吐槽</div>
            )}
            {comments.length > 0 && (
              <div className="dvtc-list">
                {comments.map((c) => {
                  // 星级：Bangumi rate 是 0~10，展示成 5 星（半星用 CSS 宽度裁）
                  const rate = Number(c.rate ?? 0) || 0;
                  const stars = rate > 0 ? Math.round(rate / 2) : 0;
                  // reactions：上游只有 4/50 条带，且形状是 [{users:[...],value:0}]
                  const rLike = (c.reactions ?? []).reduce<number>(
                    (n, x) => n + (Array.isArray(x?.users) ? x.users.length : 0),
                    0,
                  );
                  return (
                    <div className="dvtc-item" key={String(c.id)}>
                      <AvatarSafe
                        className="dvtc-ava"
                        size={36}
                        name={c.user?.nickname || c.user?.username || "匿名"}
                        src={bgmImg(c.user?.avatar?.large || c.user?.avatar?.grid || c.user?.avatar?.small)}
                      />
                      <div className="dvtc-body">
                        <div className="dvtc-head">
                          <span className="dvtc-nick">{c.user?.nickname || c.user?.username || "匿名"}</span>
                          {stars > 0 && (
                            <span className="dvtc-stars" title={`${rate} 分`}>
                              {[0, 1, 2, 3, 4].map((i) => (
                                <IcoStarL
                                  key={i}
                                  size={12}
                                  strokeWidth={1.8}
                                  className={`rating-ico${i < stars ? " fill" : ""}`}
                                />
                              ))}
                            </span>
                          )}
                          <span className="dvtc-time">{fmtBgmTime(c.updatedAt)}</span>
                        </div>
                        <div className="dvtc-bubble">
                          <p className="dvtc-text">{c.comment}</p>
                          <span className={`dvtc-like${rLike > 0 ? " on" : ""}`} aria-hidden="true">
                            {rLike > 0 ? `♥ ${rLike}` : "♡"}
                          </span>
                        </div>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
            {comments.length > 0 && commentsMoreBusy && <div className="bgmc-more">加载中…</div>}
            {comments.length > 0 && commentsDone && <div className="bgmc-more">没有更多吐槽了</div>}
          </div>
        )}
          </div>
        </div>

        {/* ── 右列：集数据（整条竖向列表）──
            每集 = 集号徽标 + 小标题（本机集名 / Bangumi name_cn 兜底）+ 该集简介 + 时长。
            「本机集数 × Bangumi 集号」对齐：本机集按 episode_number 取 Bangumi 同号那条的
            小标题与 desc（Bangumi 只有一季时 ep 是**全季连号**，所以按号取，不按季内序号）。 */}
        <aside className="dv2-right">
          {/* [lc-1420] 非剧集（电影等）没有选集可点，海报上的播放按钮又已删 —— 
              这里给一个独立的播放入口，避免"能看不能播"。
              [lc-1444] 发现页条目没有飞牛 guid，播不了 —— 不显示播放入口。 */}
          {!isSeries && !noFnOS && (
            <button
              className="dv2-solo-play"
              disabled={playing}
              onClick={playing ? undefined : startPlay}
            >
              <IcoPlayL size={16} strokeWidth={2.4} />
              <span>{playing ? "打开中…" : resumeAt ? "继续播放" : "播放"}</span>
            </button>
          )}
          {/* [lc-1421] 头部改成**一整行**（用户要求："做成一排显示，播放按钮在左边"）。
              顺序 = 播放按钮 → 选集标题 → 集数/完结/更新 → 季切换。
              原来按钮独占右侧、标题与信息在上面一行，一窄就分成两行（用户截图就是那样）。
              `.dv2e-hd` 用 flex + nowrap + 允许横向溢出，保证"一排"；窄到实在放不下时才换行。 */}
          <div className="dv2e-hd">
            {/* 播放入口（原来在右侧，现移到最左）。语义：
                有播放记录 → 「观看至 第 N 集 · N%」（[lc-1424] 文案由"继续观看"改为"观看至"：
                更贴合"已经看到第 N 集的第 N%"这个事实）；没有记录 → 「播放 第 1 集」。 */}
            {isSeries && resumeTarget && (
              <button
                className="dv2e-play-btn"
                disabled={playing}
                onClick={() => {
                  setPlayingGuid(resumeTarget.guid);
                  invoke("play_item", { guid: resumeTarget.guid }).catch((e) => setErr(String(e)));
                }}
                title={
                  resumePct > 0
                    ? `从第 ${resumeTarget.episode_number ?? 1} 集的 ${resumePct}% 继续播放`
                    : `播放 第 ${resumeTarget.episode_number ?? 1} 集`
                }
              >
                <IcoPlayL size={14} strokeWidth={2.4} />
                <span>
                  {playing
                    ? "打开中…"
                    : resumePct > 0
                      ? `观看至 第 ${resumeTarget.episode_number ?? 1} 集`
                      : `播放 第 ${resumeTarget.episode_number ?? 1} 集`}
                </span>
                {resumePct > 0 && <i className="dv2e-play-pct">{resumePct}%</i>}
              </button>
            )}
            <h3 className="dv2e-hd-h">选集</h3>
            {/* [lc-1419] 集数/完结/上次更新（"一眼确认状态"） */}
            {isSeries && (
              <span className="dv2e-meta">
                <span className="dv2e-meta-n">
                  {/* [lc-1444] 发现页条目没有本机集数：显示 Bangumi 总集数（若有），否则 "—" */}
                  {noFnOS
                    ? totalEps > 0
                      ? totalEps
                      : "—"
                    : `${episodes.length}${totalEps > 0 && totalEps !== episodes.length ? ` / ${totalEps}` : ""}`}{" "}
                  集
                </span>
                <span className={`dv2e-state${finished ? " done" : " airing"}`}>
                  {finished ? "已完结" : "连载中"}
                </span>
                {lastAir && <span className="dv2e-meta-d">更新至 {String(lastAir).slice(0, 10)}</span>}
              </span>
            )}
            {seasons.length > 1 && (
              <div className="season-tabs">
                {seasons.map((sn, i) => (
                  <button
                    key={sn.guid}
                    className={`season-tab ${i === seasonIdx ? "active" : ""}`}
                    onClick={() => setSeasonIdx(i)}
                  >
                    {sn.title || `第 ${sn.season_number ?? i + 1} 季`}
                  </button>
                ))}
              </div>
            )}
          </div>
          <div className="dv2e-list">
            {!isSeries ? (
              <div className="empty">本条目不是剧集</div>
            ) : episodes.length === 0 && seasons.length === 0 && !noFnOS ? (
              /* [lc-1420] **分块骨架**：条目详情（data）到得很快，所以整页骨架一闪就没了；
                 但季/集是另外两条请求 —— 它们没回来之前右栏是空的，用户看着就是"还在等数据"。
                 这里单独给右栏摆骨架（用真实行高，数据到位时不跳）。 */
              [0, 1, 2, 3].map((i) => (
                <div className="dv2e-row" key={`sk${i}`}>
                  <div className="dv2e-thumb"><div className="dsk dsk-dthumb" /></div>
                  <div className="dv2e-main">
                    <div className="dsk dsk-sline" />
                    <div className={`dsk dsk-sline${i % 2 ? " short" : ""}`} />
                    <div className="dsk dsk-sline short" />
                  </div>
                </div>
              ))
            ) : episodes.length === 0 ? (
              /* [lc-1444] 发现页条目没有飞牛选集数据。
                 [lc-1500] 无源条目的"看"法 = 在线资源面板（Kazumi 规则源 → mpv），
                 直接替换掉原来的"选集信息暂未接入"占位。 */
              noFnOS ? (
                <WebrsPanel title={title || base.title} />
              ) : (
                <div className="empty">暂无剧集</div>
              )
            ) : (
              episodes.map((ep, i) => {
                const num = ep.episode_number ?? i + 1;
                const bgmEp = bgmEpByNum.get(num);
                const sub = String(bgmEp?.name_cn || bgmEp?.name || "").trim();
                const desc = String(ep.overview || bgmEp?.desc || "").trim();
                const progress =
                  ep.ts && ep.duration && ep.duration > 0 ? Math.min(100, (ep.ts / ep.duration) * 100) : 0;
                const seen = Number(ep.watched) === 1 || progress >= 95;
                const playing = playingGuid === ep.guid;
                return (
                  <div
                    key={ep.guid}
                    className={`dv2e-row${playing ? " playing" : ""}${seen ? " seen" : ""}`}
                    onClick={() => {
                      setPlayingGuid(ep.guid);
                      invoke("play_item", { guid: ep.guid }).catch((e) => setErr(String(e)));
                    }}
                    title={`播放 第 ${num} 集`}
                  >
                    <div className="dv2e-thumb">
                      {ep.poster ? (
                        <img src={imgUrl(ep.poster)} loading="lazy" alt="" draggable={false} />
                      ) : (
                        <div className="poster-empty">
                          <IcoClapL size={18} strokeWidth={1.8} />
                        </div>
                      )}
                      <span className="dv2e-play">{playing ? "◉" : "▶"}</span>
                      {seen && <span className="ep-mark seen"><IcoCheckL size={11} strokeWidth={2.2} /> 已看</span>}
                      {!seen && progress > 0 && <span className="ep-mark now">看到 {Math.round(progress)}%</span>}
                      {progress > 0 && (
                        <div className="progress">
                          <div style={{ width: `${progress}%` }} />
                        </div>
                      )}
                    </div>
                    <div className="dv2e-main">
                      <div className="dv2e-t">
                        <span className="dv2e-name">第 {num} 集{ep.title ? `. ${ep.title}` : sub ? `. ${sub}` : ""}</span>
                        {ep.quality && <span className="ep-quality">{ep.quality}</span>}
                      </div>
                      {desc && <p className="dv2e-desc">{desc}</p>}
                      {fmtDur(ep.duration) && <div className="dv2e-dur">{fmtDur(ep.duration)}</div>}
                    </div>
                  </div>
                );
              })
            )}
          </div>
        </aside>
      </div>
      )}

      {/* [lc-1423] 简介弹窗：完整正文（容器固定高度 + 截断后，全文在这里看）。
          Esc / 点背景关闭；正文可滚动（长简介比一屏更长）。 */}
      {introFull &&
        createPortal(
          <div className="ddet-mask" onClick={() => setIntroFull(false)}>
          <div className="sum-modal" onClick={(e) => e.stopPropagation()}>
            <div className="sum-modal-h">
              <h3>简介</h3>
              <button
                className="ddet-close sum-modal-x"
                title="关闭（Esc）"
                onClick={() => setIntroFull(false)}
              >
                <IcoXL size={15} strokeWidth={1.8} />
              </button>
            </div>
            <div className="sum-modal-b">
              <p className="dfold">{overview}</p>
            </div>
          </div>
        </div>,
          document.body,
        )}

      {/* [lc-128] 剧照原图（`original` 尺寸）：点击任意剧照放大。走同一套三级缓存
          （拉了即落盘，下次点开零网络），点背景/Esc… 这里只需点背景关闭。 */}
      {zoom &&
        createPortal(
          <div className="ddet-mask zoom-mask" onClick={() => setZoom("")}>
            <img className="zoom-img" src={tmdbImg(zoom, "original")} alt="" />
          </div>,
          document.body,
        )}
    </main>
  );
}


/* ────────────────────────── TMDB 扩展信息（剧照 / 相似 / 更多） ──────────────────────────
   剧照/头像走 fntvimg 代理的 TMDB 图片域（Rust 反代镜像 + **落盘三级缓存**，见 tmdbImg() 与 imgproto.rs）。
   [lc-128] 演职人员一节搬去「演职人员」页（TMDB 主源 + 落盘缓存 + 飞牛兜底），这里不再重复渲染。
   [lc-1413] 按用户要求**删掉「预告片」与「分级 / 在线观看」两节**：
     前者是 YouTube 外链（国内打不开，点了也是白点），后者对国内用户基本是空数据；
     连带删掉 TmdbRegions 组件、videos 派生变量与相关类型用法。 */

function TmdbExtra({
  tmdb,
  onZoom,
}: {
  tmdb: Record<string, unknown>;
  /** [lc-128] 点剧照 → 以 original 尺寸放大（原图走同一套落盘缓存，拉了就不再拉） */
  onZoom?: (path: string) => void;
}) {
  const arr = (k: string): Record<string, unknown>[] =>
    Array.isArray(tmdb[k]) ? (tmdb[k] as Record<string, unknown>[]) : [];
  const strList = (k: string): string[] => (Array.isArray(tmdb[k]) ? (tmdb[k] as string[]) : []);
  const stills = arr("backdrops").slice(0, 6);
  const recs = arr("recommendations").slice(0, 8);
  const aliases = strList("aliases");
  const keywords = strList("keywords");
  const popularity = Number(tmdb.popularity ?? 0);
  const tmdbId = String(tmdb.tmdbId ?? "");
  const isMovie = String(tmdb.mediaType ?? "") === "movie";
  const imdb = String((tmdb.externalIds as Record<string, unknown> | undefined)?.imdb ?? "");
  const wiki = String((tmdb.externalIds as Record<string, unknown> | undefined)?.wikidata ?? "");
  const hasMore = aliases.length || keywords.length || popularity > 0;
  const links = [
    { label: "TMDB", url: tmdbId ? `https://www.themoviedb.org/${isMovie ? "movie" : "tv"}/${tmdbId}` : "" },
    { label: "IMDb", url: imdb ? `https://www.imdb.com/title/${imdb}` : "" },
    { label: "Wikidata", url: wiki ? `https://www.wikidata.org/wiki/${wiki}` : "" },
  ].filter((l) => l.url);
  return (
    <>
      {stills.length > 0 && (
        <div className="tmdb-sec">
          <div className="tmdb-sec-t">剧照</div>
          {/* [lc-1413] 用户反馈"剧照太多显示不完全，加同款雪佛龙按钮（不要背景色）" ——
              原来是一条 overflow-x:auto 的裸横排（项目全局隐藏了滚动条，滚轮又只滚纵向，
              所以多出来的剧照根本看不到）。这里改走 ScrollRow：复用首页那套**无底色**雪佛龙，
              smooth 翻页 + 两端自动隐藏。箭头垂直居中的基准改成剧照高度（.tmdb-stills img）。 */}
          <ScrollRow boxClass="dvstill-row" imgSel=".tmdb-stills img" inSec>
            {stills.map((st) => (
              <img
                key={String(st.file_path ?? st)}
                src={tmdbImg(String(st.file_path ?? st), "w500")}
                loading="lazy"
                alt=""
                title="点击查看原图"
                onClick={() => onZoom?.(String(st.file_path ?? st))}
              />
            ))}
          </ScrollRow>
        </div>
      )}
      {recs.length > 0 && (
        <div className="tmdb-sec">
          <div className="tmdb-sec-t">相似剧集</div>
          <p className="tmdb-inline">
            {recs.map((r, i) => (
              <span key={String(r.tmdbId ?? i)}>
                {i > 0 && <i> · </i>}
                {String(r.title)}
                {r.year ? ` (${r.year})` : ""}
              </span>
            ))}
          </p>
        </div>
      )}
      {/* [lc-1413] 用户要求删掉「预告片」与「分级 / 在线观看」两个模块 ——
          前者是外链 YouTube（国内打不开，点了也是白点），后者对国内用户基本是空的。 */}
      {hasMore && (
        <div className="tmdb-sec">
          <div className="tmdb-sec-t">更多</div>
          {aliases.length > 0 && (
            <div className="tmdb-mrow"><span className="tmdb-mk">别名</span><span className="tmdb-inline">{aliases.join(" · ")}</span></div>
          )}
          {keywords.length > 0 && (
            <div className="tmdb-mrow"><span className="tmdb-mk">关键词</span><span className="tmdb-inline">{keywords.join(" · ")}</span></div>
          )}
          {popularity > 0 && (
            <div className="tmdb-mrow"><span className="tmdb-mk">热度</span><span className="tmdb-inline">{Math.round(popularity).toLocaleString("zh-CN")}</span></div>
          )}
          <div className="tmdb-mrow">
            <span className="tmdb-mk">编号</span>
            <span className="tmdb-inline">
              {tmdbId && `TMDB ${tmdbId}`}
              {imdb && ` · Q${wiki.replace(/^Q/, "")}` && wiki && ` · ${wiki}`}
              {imdb && ` · ${imdb}`}
            </span>
          </div>
          {links.length > 0 && (
            <div className="tmdb-links">
              {links.map((l) => (
                <a key={l.label} href={l.url} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>
                  {l.label}
                </a>
              ))}
            </div>
          )}
        </div>
      )}
    </>
  );
}

/* ────────────────────────── 侧栏（保留现有样式） ────────────────────────── */

const SIDEBAR_CATS = [
  { key: "全部", types: [] as string[], ico: <IcoGrid />, sumKey: "total" },
  { key: "电影", types: ["Movie"], ico: <IcoMovie />, sumKey: "movie" },
  { key: "电视节目", types: ["TV"], ico: <IcoTv />, sumKey: "tv" },
  { key: "电视直播", types: ["Live"], ico: <IcoLive />, sumKey: "live" },
  { key: "其他", types: ["Video"], ico: <IcoFolder />, sumKey: "video" },
];

function Sidebar({
  activeGuid,
  collapsed,
  username,
  server,
  sum,
  favTotal,
  onOpenFav,
  onOpenCat,
  onHome,
  onOpenSettings,
}: {
  activeGuid: string | null;
  collapsed?: boolean;
  username?: string;
  server?: string;
  sum: Record<string, number>;
  favTotal: number | null;
  onOpenFav: () => void;
  onOpenCat: (cat: { key: string; types: string[] }) => void;
  onHome: () => void;
  /** [lc-128] 打开设置面板（缓存上限 / 占用 / 清理 + 关于） */
  onOpenSettings: () => void;
}) {
  const [toast, setToast] = useState("");
  const toastTimer = useRef<number | null>(null);
  // 快捷操作卡折叠态：持久化键与主仓库 embyWall.ts 相同（'1'=收起成图标行，缺省=展开）
  const [cardFold, setCardFold] = useState(
    () => localStorage.getItem("fntvSidebarActions.collapsed") === "1",
  );
  const [appVer, setAppVer] = useState("0.1.0");
  useEffect(() => {
    getVersion().then(setAppVer).catch(() => {});
  }, []);
  function setFold(v: boolean) {
    setCardFold(v);
    try {
      localStorage.setItem("fntvSidebarActions.collapsed", v ? "1" : "0");
    } catch {
      /* ignore */
    }
  }
  function showToast(msg: string) {
    setToast(msg);
    if (toastTimer.current) clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(""), 2200);
  }
  return (
    <aside className={`sidebar ${collapsed ? "collapsed" : ""}`}>
      <div className={`side-item ${activeGuid === null ? "active" : ""}`} onClick={onHome}>
        <span className="side-ico">
          <IcoHome />
        </span>{" "}
        首页
      </div>
      <div className={`side-item ${activeGuid === "_fav" ? "active" : ""}`} onClick={onOpenFav}>
        <span className="side-ico">
          <IcoStar />
        </span>{" "}
        收藏
        <span className="side-count">{favTotal ?? ""}</span>
      </div>
      <div className="side-head">分类</div>
      {SIDEBAR_CATS.map((c) => {
        const cnt = sum[c.sumKey];
        return (
          <div key={c.key} className="side-item" onClick={() => onOpenCat(c)}>
            <span className="side-ico">{c.ico}</span> {c.key}
            <span className="side-count">{typeof cnt === "number" ? cnt : ""}</span>
          </div>
        );
      })}
      {/* 底部：用户行 + 快捷操作卡 —— 结构与样式逐行复刻主仓库 embyWall.ts injectSettingsUI */}
      <div className="side-userrow">
        <div className="side-avatar">{(username || "U").slice(0, 1).toUpperCase()}</div>
        <div className="side-usermeta">
          <div className="side-user-name">{username || "未登录"}</div>
          <div className="side-user-server">{server ? server.replace(/^https?:\/\//, "") : "未连接"}</div>
        </div>
      </div>
      <div className="side-actions">
        {cardFold ? (
          <div className="sa-icons">
            <button type="button" className="sa-ico" title="设置" onClick={onOpenSettings}>
              <IcoGearL size={15} strokeWidth={1.8} />
            </button>
            <button type="button" className="sa-ico" title="观影记录" onClick={() => showToast("观影记录页在阶段3接入")}>
              <IcoClockL size={15} strokeWidth={1.8} />
            </button>
            <button type="button" className="sa-ico" title="软件反馈建议" onClick={() => showToast("反馈渠道在阶段3接入")}>
              <IcoMsgL size={15} strokeWidth={1.8} />
            </button>
            <button type="button" className="sa-ico sa-expand" title="展开快捷操作" onClick={() => setFold(false)}>
              ∧
            </button>
          </div>
        ) : (
          <>
            <div className="sa-head" title="收起快捷操作" onClick={() => setFold(true)}>
              收起
              <span className="sa-chev" />
            </div>
            <button type="button" className="sa-btn" onClick={onOpenSettings}>
              <IcoGearL size={14} strokeWidth={1.8} /> 设置
            </button>
            <button type="button" className="sa-btn" onClick={() => showToast("观影记录页在阶段3接入")}>
              <IcoClockL size={14} strokeWidth={1.8} /> 观影记录
            </button>
            <button type="button" className="sa-btn" onClick={() => showToast("反馈渠道在阶段3接入")}>
              <IcoMsgL size={14} strokeWidth={1.8} /> 软件反馈建议
            </button>
            <div className="sa-ver">v{appVer}</div>
            {/* [lc-122] 数据来源署名（关于区=版本行下）：TMDB 条款强制原文 + Bangumi 署名。
                链接与详情页外链同款（<a target=_blank>），由 Tauri 交给系统浏览器打开。 */}
            <div className="sa-attrib">
              <span>
                This product uses the TMDB API but is not endorsed or certified by TMDB.
              </span>
              <span>
                <a href="https://www.themoviedb.org/" target="_blank" rel="noreferrer">
                  themoviedb.org
                </a>
              </span>
              <span>
                数据与部分内容来自{" "}
                <a href="https://bgm.tv/" target="_blank" rel="noreferrer">
                  Bangumi 番组计划
                </a>
              </span>
              {/* [lc-123-rev8] 图标库开源致谢（Lucide，ISC） */}
              <span>
                Icons by{" "}
                <a href="https://lucide.dev" target="_blank" rel="noreferrer">
                  Lucide (ISC)
                </a>
              </span>
            </div>
          </>
        )}
      </div>
      {toast && <div className="side-toast">{toast}</div>}
    </aside>
  );
}

/* ────────────────────────── 设置面板（缓存 + 关于） ────────────────────────── */

/** [lc-128] 设置面板：目前两栏 —— **缓存**（上限 / 占用 / 清理）与**关于**（版本 + 署名，与侧栏
 *  关于区同一份文案）。外壳复用 Bangumi 弹层的 .ddet-mask/.ddet/.ddet-close/.ddet-hd 与 .ddet-body；
 *  内部 .set-* 全部走既有 CSS 变量（同一套深色亚克力玻璃、字体与圆角），不引入新配色。
 *  三要素对应 Rust 的三条命令：cache_stats / cache_set_limit / cache_clear
 *  （清理**同时清内存层与磁盘层**，不会出现"清了盘还留着旧图"）。 */
/** [lc-1437] 设置面板：通用 / 外观 / 账号 / 缓存 / 诊断 / 关于 六组。
 *  与 Rust 侧的分工：`get_settings`/`set_settings` 是**白名单子集**（不含密码/token），
 *  缓存那三条命令是独立的一小组；诊断三按钮走 `open_path`。 */
function SettingsPanel({
  onClose,
  onLogout,
  settings,
  onSettings,
}: {
  onClose: () => void;
  /** 退出登录：由 App 负责清会话与切页（面板只发意图，不自己改全局状态） */
  onLogout: () => void;
  /** 设置项（App 持有：轮播 Logo / 性能模式都要在面板外生效） */
  settings: AppSettings;
  onSettings: (patch: Partial<AppSettings>) => void;
}) {
  type Stats = { bytes: number; files: number; limitMb: number; dir: string };
  const [stats, setStats] = useState<Stats | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState("");
  const [ver, setVer] = useState("0.1.0");
  const [user, setUser] = useState("");
  const [server, setServer] = useState("");
  const [confirmLogout, setConfirmLogout] = useState(false);
  const [autostart, setAutostart] = useState(
    () => localStorage.getItem("fntv-autostart") === "1",
  );

  const refresh = useCallback(() => {
    invoke<Stats>("cache_stats")
      .then(setStats)
      .catch((e) => setNote(String(e)));
  }, []);

  useEffect(() => {
    refresh();
    getVersion().then(setVer).catch(() => {});
    invoke<Config>("get_config")
      .then((c) => {
        setUser(c.username);
        setServer(c.server);
      })
      .catch(() => {});
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose, refresh]);

  const setLimit = async (mb: number) => {
    setBusy(true);
    setNote("");
    try {
      const r = await invoke<Stats>("cache_set_limit", { limitMb: mb });
      setStats(r);
      setNote(mb === 0 ? "已设为不限制" : `上限已设为 ${fmtSize(mb * 1024 * 1024)}`);
    } catch (e) {
      setNote(String(e));
    } finally {
      setBusy(false);
    }
  };

  const clearCache = async () => {
    setBusy(true);
    setNote("");
    try {
      const r = await invoke<Stats>("cache_clear");
      setStats(r);
      setNote("缓存已清理（内存 + 磁盘）");
    } catch (e) {
      setNote(String(e));
    } finally {
      setBusy(false);
    }
  };

  /** [lc-1437] 自启开关：真值在系统注册表里，前端只在 localStorage 记一份 UI 状态。
   *  写入失败（权限/杀软拦截）时**回滚开关**并把错误显示出来 —— 不能出现
   *  "界面显示已开启、实际没写进注册表"这种静默不一致。 */
  const toggleAutostart = async () => {
    const next = !autostart;
    setBusy(true);
    setNote("");
    try {
      await invoke("set_autostart", { enabled: next });
      setAutostart(next);
      localStorage.setItem("fntv-autostart", next ? "1" : "0");
      setNote(next ? "已设为开机自启" : "已取消开机自启");
    } catch (e) {
      setNote(String(e));
    } finally {
      setBusy(false);
    }
  };

  const openPath = async (kind: "log" | "cache" | "app") => {
    setNote("");
    try {
      await invoke("open_path", { kind });
    } catch (e) {
      setNote(String(e));
    }
  };

  const limitMb = stats?.limitMb ?? 0;
  // 下拉里没有的当前值（手改过 config.json）也要显示出来，别让 UI 与真实配置不一致
  const known = [0, 1024, 2048, 5120, 10240];
  const extras = limitMb && !known.includes(limitMb) ? [limitMb] : [];

  return (
    <div className="ddet-mask" onClick={onClose}>
      <div className="ddet set-card" onClick={(e) => e.stopPropagation()}>
        <button className="ddet-close" title="关闭（Esc）" onClick={onClose}>
          <IcoXL size={15} strokeWidth={1.8} />
        </button>
        <header className="ddet-hd">
          <div className="ddet-hdm">
            <h2>设置</h2>
            <div className="ddet-meta">
              <span className="ddet-chip dim">v{ver}</span>
            </div>
          </div>
        </header>
        <div className="ddet-body set-body">
          {/* ── 通用 ── */}
          <h3 className="ddet-h3">通用</h3>
          <div className="set-row">
            <div>
              <span className="set-k">启动页</span>
              <div className="set-desc">登录成功后进入的页面</div>
            </div>
            <select
              className="set-select"
              value={settings.launchPage}
              disabled={busy}
              onChange={(e) => onSettings({ launchPage: e.target.value as AppSettings["launchPage"] })}
            >
              <option value="home">首页</option>
              <option value="continue">继续观看</option>
            </select>
          </div>
          <div className="set-row">
            <div>
              <span className="set-k">开机自启</span>
              <div className="set-desc">登录系统后自动启动本程序</div>
            </div>
            <button
              type="button"
              className={`set-sw${autostart ? " on" : ""}`}
              disabled={busy}
              aria-pressed={autostart}
              aria-label="开机自启"
              onClick={toggleAutostart}
            />
          </div>
          <div className="set-row">
            <div>
              <span className="set-k">关闭按钮</span>
              <div className="set-desc">点窗口右上角关闭时的行为</div>
            </div>
            <select
              className="set-select"
              value={settings.exitMode}
              disabled={busy}
              onChange={(e) => onSettings({ exitMode: e.target.value as AppSettings["exitMode"] })}
            >
              <option value="ask">每次询问</option>
              <option value="tray">最小化到托盘</option>
              <option value="exit">直接退出</option>
            </select>
          </div>

          {/* ── 外观 ── */}
          <h3 className="ddet-h3">外观</h3>
          <div className="set-row">
            <div>
              <span className="set-k">性能模式</span>
              <div className="set-desc">关闭动画与磨砂效果，低配机更流畅</div>
            </div>
            <button
              type="button"
              className={`set-sw${settings.perfMode ? " on" : ""}`}
              aria-pressed={settings.perfMode}
              aria-label="性能模式"
              onClick={() => onSettings({ perfMode: !settings.perfMode })}
            />
          </div>
          <div className="set-row">
            <div>
              <span className="set-k">轮播透明标题</span>
              <div className="set-desc">首页轮播用影片透明 Logo 代替文字标题（无 Logo 时自动用文字）</div>
            </div>
            <button
              type="button"
              className={`set-sw${settings.carouselLogo ? " on" : ""}`}
              aria-pressed={settings.carouselLogo}
              aria-label="轮播透明标题"
              onClick={() => onSettings({ carouselLogo: !settings.carouselLogo })}
            />
          </div>

          {/* ── 账号 ── */}
          <h3 className="ddet-h3">账号</h3>
          <div className="set-row">
            <span className="set-k">当前账号</span>
            <span className="set-v">{user || "—"}</span>
          </div>
          <div className="set-row">
            <span className="set-k">服务器</span>
            <span className="set-v">{server || "—"}</span>
          </div>
          <div className="set-row">
            <span className="set-k">退出登录</span>
            {confirmLogout ? (
              <div className="set-confirm">
                <span className="set-confirm-q">退出后需重新登录，确定？</span>
                <button className="set-btn" disabled={busy} onClick={() => setConfirmLogout(false)}>
                  取消
                </button>
                <button
                  className="set-btn danger"
                  disabled={busy}
                  onClick={() => {
                    setConfirmLogout(false);
                    onLogout();
                  }}
                >
                  确定退出
                </button>
              </div>
            ) : (
              <button className="set-btn danger" onClick={() => setConfirmLogout(true)}>
                退出登录
              </button>
            )}
          </div>

          {/* ── 缓存 ── */}
          <h3 className="ddet-h3">缓存</h3>
          <div className="set-row">
            <span className="set-k">占用上限</span>
            <select
              className="set-select"
              value={limitMb}
              disabled={busy}
              onChange={(e) => setLimit(Number(e.target.value))}
            >
              {extras.map((mb) => (
                <option key={mb} value={mb}>
                  {fmtSize(mb * 1024 * 1024)}
                </option>
              ))}
              <option value={1024}>1 GB</option>
              <option value={2048}>2 GB</option>
              <option value={5120}>5 GB</option>
              <option value={10240}>10 GB</option>
              <option value={0}>不限制</option>
            </select>
          </div>
          <div className="set-row">
            <span className="set-k">当前占用</span>
            <span className="set-v">
              {stats ? `${fmtSize(stats.bytes)} · ${stats.files} 个文件` : "读取中…"}
            </span>
          </div>
          <div className="set-row">
            <span className="set-k">清理缓存</span>
            <button className="set-btn" onClick={clearCache} disabled={busy || !stats?.bytes}>
              立即清理
            </button>
          </div>
          <div className="set-hint">
            图片与演员表缓存在软件数据目录（{stats?.dir || "…"}）：命中本地就不再请求，超上限按最后访问
            时间自动淘汰；清理会同时清掉内存与磁盘。
          </div>

          {/* ── 诊断 ── */}
          <h3 className="ddet-h3">诊断</h3>
          <div className="set-row">
            <span className="set-k">打开目录</span>
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap", justifyContent: "flex-end" }}>
              <button className="set-btn" onClick={() => openPath("log")}>
                诊断日志
              </button>
              <button className="set-btn" onClick={() => openPath("cache")}>
                缓存目录
              </button>
              <button className="set-btn" onClick={() => openPath("app")}>
                数据目录
              </button>
            </div>
          </div>
          <div className="set-hint">
            诊断日志在系统临时目录（anibox.log），记录了镜像请求、图片缓存命中与播放链路；
            反馈问题时把最后一段贴出来即可。
          </div>

          {/* ── 关于 ── */}
          <h3 className="ddet-h3">关于</h3>
          <div className="set-attrib">
            <span>This product uses the TMDB API but is not endorsed or certified by TMDB.</span>
            <span>
              <a href="https://www.themoviedb.org/" target="_blank" rel="noreferrer">
                themoviedb.org
              </a>
            </span>
            <span>
              数据与部分内容来自{" "}
              <a href="https://bgm.tv/" target="_blank" rel="noreferrer">
                Bangumi 番组计划
              </a>
            </span>
          </div>
          {note && <div className="set-note">{note}</div>}
        </div>
      </div>
    </div>
  );
}

/* ────────────────────────── 根组件 ────────────────────────── */

function App() {
  const [view, setView] = useState<"login" | "home" | "discover" | "lib" | "detail">("login");
  // [lc-123] 「页」= 顶栏胶囊的两个面（首页/发现）。与 view 分开：在库里/详情页时胶囊仍要
  // 记住用户是从哪一页进去的（发现页点条目进 Bangumi 详情，左上角应仍显示「发现」）。
  const [page, setPage] = useState<"home" | "discover">("home");
  const [libs, setLibs] = useState<Lib[]>([]);
  const [continueList, setContinueList] = useState<PlayItem[]>([]);
  const [carousel, setCarousel] = useState<MediaItem[]>([]);
  const [carouselBds, setCarouselBds] = useState<Record<string, string>>({});
  const [carouselLogos, setCarouselLogos] = useState<Record<string, string>>({});
  const [libRows, setLibRows] = useState<{ lib: Lib; items: MediaItem[] }[]>([]);
  // 首页不展示的媒体库（仅影响首页媒体库条与按库横排；localStorage 持久化）
  const [hiddenLibs, setHiddenLibs] = useState<string[]>(() => {
    try {
      const v = JSON.parse(localStorage.getItem("fntv-hidden-libs") || "[]");
      return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
    } catch {
      return [];
    }
  });
  const toggleHiddenLib = useCallback((guid: string) => {
    setHiddenLibs((prev) => {
      const next = prev.includes(guid) ? prev.filter((g) => g !== guid) : [...prev, guid];
      try {
        localStorage.setItem("fntv-hidden-libs", JSON.stringify(next));
      } catch {
        /* ignore */
      }
      return next;
    });
  }, []);
  const [activeLib, setActiveLib] = useState<Lib | null>(null);
  const [detailItem, setDetailItem] = useState<MediaItem | null>(null);
  // [lc-1444] 进详情的来源（返回键的目标）：lib = 库里进，discover = 发现页进，home = 首页进。
  // 不用 view 现查 —— 用户从发现页进详情后 view 已是 "detail"，现查只能靠猜。
  const [detailFrom, setDetailFrom] = useState<"lib" | "discover" | "home">("home");
  const [err, setErr] = useState("");
  const [sidebarOpen, setSidebarOpen] = useState(
    () => localStorage.getItem("fntv-sidebar-open") !== "0",
  );
  // 深浅主题：默认浅色；html.dark 类由本状态驱动，首帧前 main.tsx 已按存储值预置防闪烁
  const [theme, setTheme] = useState<"light" | "dark">(() =>
    localStorage.getItem("fntv-theme") === "dark" ? "dark" : "light",
  );
  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
    localStorage.setItem("fntv-theme", theme);
    // SWCA 模糊染色随主题切换（浅色近白/深色近黑）
    invoke("set_acrylic_tint", { light: theme === "light" }).catch(() => {});
  }, [theme]);

  /* ── [lc-1437] 设置项（性能模式 / 轮播 Logo / 退出行为 / 启动页）──
     状态放 App 顶层：前两项要在面板之外生效（html.perf 类、CarouselView 入参），
     后两项是"下一次动作"才生效的（关闭按钮、登录后落点）。
     真值在 Rust 的 config.json，这里只是它的镜像 + 一份 localStorage 兜底：
     **启动瞬间就用本地值**（不等 IPC 回来，避免性能模式用户先看到一帧动画），
     IPC 回来后再以配置为准校正一次。 */
  const [settings, setSettings] = useState<AppSettings>(() => {
    let cached: Partial<AppSettings> = {};
    try {
      const raw = localStorage.getItem("fntv-settings");
      if (raw) cached = JSON.parse(raw) as Partial<AppSettings>;
    } catch {
      /* 损坏就当没有 */
    }
    return { ...DEFAULT_SETTINGS, ...cached };
  });
  // 性能模式：html.perf 类驱动全局降级（CSS 见 App.css [lc-1437]）
  useEffect(() => {
    document.documentElement.classList.toggle("perf", settings.perfMode);
  }, [settings.perfMode]);
  // 启动时拉一次真实配置（校正本地镜像；失败就继续用本地值）
  useEffect(() => {
    invoke<AppSettings>("get_settings")
      .then((s) => {
        if (!s) return;
        setSettings((prev) => ({ ...prev, ...s }));
        try {
          localStorage.setItem("fntv-settings", JSON.stringify(s));
        } catch {
          /* ignore */
        }
      })
      .catch(() => {});
  }, []);
  /** 改设置：**先本地生效**（开关点下去立刻有反应），再落盘；落盘失败则重拉真实配置覆盖回来。 */
  const patchSettings = useCallback(async (patch: Partial<AppSettings>) => {
    setSettings((prev) => ({ ...prev, ...patch }));
    try {
      const saved = await invoke<AppSettings>("set_settings", { arg: patch });
      setSettings((prev) => ({ ...prev, ...saved }));
      try {
        localStorage.setItem("fntv-settings", JSON.stringify(saved));
      } catch {
        /* ignore */
      }
    } catch {
      // 回滚：不能让界面停在一个"没写进配置"的值上。重拉最稳
      // （逐个键反推"改之前"容易出错，且 Rust 侧可能只拒了其中一个键）
      invoke<AppSettings>("get_settings")
        .then((s) => s && setSettings((prev) => ({ ...prev, ...s })))
        .catch(() => {});
    }
  }, []);

  /** [lc-1439] 关闭询问：原生关闭按钮被 Rust 拦下后 emit close-ask，这里弹框。
   *  仅桌面端有该事件（Web 预览下 listen 不可用 → 静默跳过）。 */
  const [closeAsk, setCloseAsk] = useState(false);
  // [lc-1514] 「记住我的选择」勾选态：每次弹问重置（不把上次的勾选带给下一次）
  const [rememberClose, setRememberClose] = useState(false);
  useEffect(() => {
    if (closeAsk) setRememberClose(false);
  }, [closeAsk]);
  useEffect(() => {
    let un: (() => void) | undefined;
    listen("close-ask", () => setCloseAsk(true))
      .then((f) => {
        un = f;
      })
      .catch(() => {
        /* 非 Tauri 环境（如 vite preview）：没有该事件 */
      });
    return () => un?.();
  }, []);

  // 首页加载中（首次/手动刷新）：无数据时渲染骨架屏；已有数据时静默刷新不闪骨架
  const [homeLoading, setHomeLoading] = useState(false);
  // [lc-123-rev4] 顶栏 chrome 色（首页 hero 亮度采样结果；CarouselView 上报）。
  // 只在首页生效：发现/库/详情页顶栏底下是应用底色 → 交回 "auto"（跟随主题）。
  const [heroTone, setHeroTone] = useState<ChromeTone>("auto");
  const onHeroTone = useCallback((t: ChromeTone) => setHeroTone(t), []);
  // 启动会话检查期：不渲染任何视图，避免带有效会话时闪登录页
  const [booting, setBooting] = useState(true);
  const [username, setUserName] = useState("");
  const [server, setServer] = useState("");
  // [lc-128] 设置面板（缓存 + 关于）开关；入口在侧栏快捷操作卡（设置）
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [sum, setSum] = useState<Record<string, number>>({});
  const [favTotal, setFavTotal] = useState<number | null>(null);

  useEffect(() => {
    // 用户名 / 服务器地址：侧栏底部卡片展示用
    invoke<Config>("get_config")
      .then((c) => {
        setUserName(c.username);
        setServer(c.server);
      })
      .catch(() => {});
  }, []);

  // 首页数据完整加载（启动无缓存/手动刷新/登录后）：拉全量并回填
  const loadHome = useCallback(async () => {
    setErr("");
    setHomeLoading(true);
    try {
      const [libData, cont, car, sumData, favData] = await Promise.all([
        invoke<Lib[]>("get_media_libs"),
        invoke<PlayItem[]>("get_continue_watching").catch(() => [] as PlayItem[]),
        invoke<{ list: MediaItem[]; total: number }>("get_carousel_items").catch(() => ({
          list: [],
          total: 0,
        })),
        invoke<Record<string, number>>("get_media_sum").catch(() => ({})),
        invoke<{ total: number }>("get_favorites", { page: 1 }).catch(() => ({ total: 0 })),
      ]);
      setSum(sumData ?? {});
      setFavTotal(typeof favData?.total === "number" ? favData.total : 0);
      const libsArr = Array.isArray(libData) ? libData : [];
      const carList = (car?.list ?? []).filter((it) => it && it.guid && it.title).slice(0, 8);
      const contArr = Array.isArray(cont) ? cont : [];
      setLibs(libsArr);
      setContinueList(contArr);
      setCarousel(carList);
      setLibRows(libsArr.map((l) => ({ lib: l, items: [] })));
      setPage("home");
      setView("home");

      // 轮播条目详情补齐（缓存到 App 层，只拉一次）：横版主图 + 简介/评分/年份回填
      Promise.allSettled(
        carList.map((it) => invoke<Record<string, unknown>>("get_item_detail", { guid: it.guid })),
      ).then((rs) => {
        const map: Record<string, string> = {};
        const lmap: Record<string, string> = {};
        const enrich: Record<string, Partial<MediaItem>> = {};
        const fb: { guid: string; it: MediaItem; arg: Record<string, unknown> }[] = [];
        rs.forEach((r, i) => {
          if (r.status === "fulfilled") {
            const d = r.value;
            // [lc-1433] 飞牛这张**只作兜底**（见下）：它是飞牛自己抓的单张 webp，
            //   实测缓存里大量只有 507×338 —— 整屏 hero 拉伸必糊，而且 `backdrops` 是
            //   **字符串**（不是数组），所以 pickLargestImg 在这里根本无从选择。
            const bd = pickLargestImg(d.backdrops) || pickImgStr(d, ["backdrops", "backdrop"]);
            if (bd) map[carList[i].guid] = bd;
            // 无论飞牛有没有图都进候选：TMDB 有图就用 TMDB 的（飞牛没有时更是唯一来源）
            // [lc-1433] **带锚点**：飞牛条目详情里就带 trim_id（tm*/tt*/bg*）与 tmdb_id/imdb_id，
            //   传进去 Rust 侧就能精确定位、**完全不做标题搜索** —— 否则"同名不同剧"会取错图
            //   （标题搜索有相似度门槛，但近似标题仍可能命中别的条目，那比糊更糟）。
            const str = (v: unknown): string => (v == null ? "" : String(v).trim());
            const trim = str(pick(d, ["trim_id", "trimId"]));
            const directTmdb = str(pick(d, ["tmdbId", "tmdb_id"]));
            let tmdbId = /^\d+$/.test(directTmdb) ? directTmdb : "";
            if (!tmdbId) {
              const m = /^(?:tm)(\d+)$/i.exec(trim);
              if (m) tmdbId = m[1];
              else if (/^\d+$/.test(trim)) tmdbId = trim;
            }
            const imdbId = str(pick(d, ["imdb_id", "imdbId"])) || (/^tt\d+$/i.test(trim) ? trim.toLowerCase() : "");
            fb.push({
              guid: carList[i].guid,
              it: carList[i],
              arg: {
                // 标题：条目详情里的（tvTitle/title）优先 —— 它与锚点同源，最不容易与
                //   列表页的简称错位（锚点缺失时这是唯一的匹配依据）
                title: String(pick(d, ["tvTitle", "title"]) ?? carList[i].title),
                year: String(carList[i].release_date || carList[i].air_date || "").slice(0, 4),
                mediaType: (carList[i].type || "").toLowerCase() === "movie" ? "movie" : "tv",
                trimId: trim || undefined,
                tmdbId: tmdbId || undefined,
                imdbId: imdbId || undefined,
              },
            });
            // [lc-570 同源] 飞牛自带透明 logo（item API data.logos，详情页 hero 同一字段）
            const lg = pickImgStr(d, ["logos", "logo"]);
            if (lg) lmap[carList[i].guid] = lg;
            const ov = pick(d, ["overview"]);
            const va = pick(d, ["vote_average"]);
            const rd = pick(d, ["release_date", "air_date"]);
            const patch: Partial<MediaItem> = {};
            if (ov && !carList[i].overview) patch.overview = String(ov);
            if (va && (!carList[i].vote_average || carList[i].vote_average === "0"))
              patch.vote_average = String(va);
            if (rd && !carList[i].release_date) patch.release_date = String(rd);
            if (Object.keys(patch).length) enrich[carList[i].guid] = patch;
          }
        });
        setCarouselBds(map);
        setCarouselLogos(lmap);
        if (Object.keys(enrich).length) {
          setCarousel((prev) => prev.map((it) => (enrich[it.guid] ? { ...it, ...enrich[it.guid] } : it)));
        }

        // [lc-1433] **高清横图：我们自己向 TMDB 要**（用户要求）——飞牛那张只当兜底。
        //   为什么必须换源：飞牛的 backdrops 是它抓的单张小图（实测多为 507×338），
        //   hero 是满屏宽度（1400~1900px），拉伸必糊。TMDB 的 images.backdrops 里有
        //   3840×2160 的原图，`heroBackdrop` 就是**按面积挑出的最大那张**（Rust 侧挑好）。
        //   取 `t/p/original` 档：走 fntvimg 代理**拉了即落盘**（下次零网络），
        //   与剧照放大用的是同一套缓存，不会多存一份。
        //   定位靠锚点优先（trim_id=tm*/tt*、tmdbId、imdbId），没有才标题搜索 —— 与详情页
        //   完全同一条链路（tmdb_show_details），命中同一份落盘缓存，所以这里几乎零额外成本。
        //   失败（TMDB 没匹配上/没图）就**保留飞牛那张**，绝不出现空 hero。
        void Promise.allSettled(
          fb.map(({ guid, arg }) =>
            invoke<Record<string, unknown>>("tmdb_show_details", { arg })
              .then((d) => ({ guid, path: String(d?.heroBackdrop ?? "") }))
          )
        ).then((trs) => {
          const hi: Record<string, string> = {};
          for (const t of trs) {
            if (t.status === "fulfilled" && t.value.path) hi[t.value.guid] = t.value.path;
          }
          if (!Object.keys(hi).length) return;
          setCarouselBds((prev) => {
            const next = { ...prev };
            for (const [guid, path] of Object.entries(hi)) next[guid] = tmdbImg(path, "original");
            return next;
          });
        });
      });

      // 每个库的第一页剧集（并行，按库顺序填回）
      // [lc-112] 原来只有 .then() 没有 .catch()：任何一个库取数失败都会被静默吞掉，
      // 该库 items 永远为空 ⇒ 整排不渲染（首页看起来"少了几个库"），且没有任何报错提示。
      libsArr.forEach((lib, idx) => {
        fetchLibItems(lib.guid, null)
          .then((items) => {
            setLibRows((rows) => {
              const next = [...rows];
              if (next[idx]) next[idx] = { lib, items };
              return next;
            });
          })
          .catch((e) => setErr(`加载媒体库「${lib.title}」失败：${String(e)}`));
      });
    } catch (e) {
      setErr(String(e));
      setView("home");
    } finally {
      setHomeLoading(false);
    }
  }, []);

  // 回首页：数据还在内存就直接切视图（进详情/库再回来不重拉），否则完整加载
  const goHome = useCallback(() => {
    setPage("home");
    if (view === "home") return;
    if (carousel.length > 0) setView("home");
    else loadHome();
  }, [view, carousel.length, loadHome]);

  // [lc-123] 去发现页：**不预取**（DiscoverView 的首拉闸门是 active=view==="discover"），
  // 首页数据原样留在内存（切回来不重拉）
  const goDiscover = useCallback(() => {
    setPage("discover");
    setView("discover");
  }, []);

  // 顶栏胶囊：首页走 goHome（有数据不重拉），发现走 goDiscover
  // [lc-123-rev6] 页面切换动效（首页 ↔ 发现）：两条视图都是常驻挂载，切换时给**进入/离开各一套**
  // 动画类（出去的 opacity→0 + 反方向位移，进来的反之）；进入侧 animationend 摘掉动画类
  // （另配 400ms 安全兜底：reduced-motion 下没有动画事件，不能留退场层盖在上面）。
  const [paneAnim, setPaneAnim] = useState<{
    dir: "to-discover" | "to-home";
    leaving: "home" | "discover";
  } | null>(null);
  const animTimer = useRef<number | null>(null);
  // [lc-1513] 子页（库/详情）转场：进出都播动画；返回时目标 pane 补一次淡入
  const [pageAnim, setPageAnim] = useState<string | null>(null);
  const [libEnterAnim, setLibEnterAnim] = useState(false);
  const [detailMounted, detailClosing] = useDelayedUnmount(view === "detail" && !!detailItem);
  const [libMounted, libClosing] = useDelayedUnmount(view === "lib" && !!activeLib);
  const clearPaneAnim = useCallback(() => {
    if (animTimer.current) {
      window.clearTimeout(animTimer.current);
      animTimer.current = null;
    }
    setPaneAnim(null);
  }, []);
  const onNav = useCallback(
    (p: "home" | "discover") => {
      if (p !== page) {
        if (prefersReducedMotion()) {
          clearPaneAnim(); // 少动效：瞬时切换
        } else {
          setPaneAnim({ dir: p === "discover" ? "to-discover" : "to-home", leaving: page });
          if (animTimer.current) window.clearTimeout(animTimer.current);
          animTimer.current = window.setTimeout(() => setPaneAnim(null), 400);
        }
      }
      if (p === "discover") goDiscover();
      else goHome();
    },
    [page, goDiscover, goHome, clearPaneAnim],
  );

  // [lc-123-rev7] 深浅模式切换：**「外部扩散」= 镂空反向扩散**（用户否掉 v6 的实色圆吞屏：
  // 「从按钮处往外扩散」：主题**立即切换**（DOM/类名马上换），随后铺一层**新主题的底色**
  // （CSS 变量按层次铺，见 App.css .theme-veil），clip-path: circle() 从**按钮处 0 长到全屏** ——
  // 圆内是新主题底色、圆外是尚未被盖住的旧界面，圆边向外推进 = 新主题从按钮荡开盖满屏。
  // 印象层**不放截图**（WebView2 无页面截图通道），也不用纯 #fff/#000。
  const [veil, setVeil] = useState<{ x: number; y: number } | null>(null);
  const toggleTheme = useCallback(
    (e: React.MouseEvent<HTMLButtonElement>) => {
      const next = theme === "dark" ? "light" : "dark";
      // [lc-1437] 性能模式/少动效偏好：跳过扩散遮罩（那是一次全屏重绘，正是性能模式要省的）
      if (prefersReducedMotion() || settings.perfMode) {
        setVeil(null);
        setTheme(next);
        return;
      }
      const r = e.currentTarget.getBoundingClientRect();
      // 主题先切（新主题立刻生效），遮罩再铺**新主题底色**从按钮处长出
      setTheme(next);
      setVeil({ x: r.left + r.width / 2, y: r.top + r.height / 2 });
    },
    [theme, settings.perfMode],
  );
  // [lc-1406] 遮罩移除的**双保险**：
  //   ① 正常路径：animationend（动画结束）
  //   ② 兜底：定时器（动画时长 420ms → 700ms 后无条件清掉）
  //   为什么必须有 ②：animationend 并非总能到达 —— 窗口失焦/最小化/后台时 Chromium 会节流甚至
  //   暂停动画，事件可能永不触发；此遮罩是 z-index:80 的**固定全屏层**，一旦残留就永久盖着页面
  //   （用户反馈的"演职人员背景有个大圆还在"就是它的形状：clip-path 圆停在中途，边缘是一段大圆弧）。
  //   定时器以 veil 的 x/y 为依赖：每次切换都会重置，所以它只兜"这一次动画没结束"，不会误杀下一次。
  const onVeilEnd = useCallback(() => setVeil(null), []);
  useEffect(() => {
    if (!veil) return;
    const t = window.setTimeout(() => setVeil(null), 700);
    return () => window.clearTimeout(t);
  }, [veil]);

  // [lc-1406] 切换视图/页面时清掉遮罩：遮罩属于"主题切换这一瞬间"的视觉，不该跨视图存活。
  useEffect(() => {
    setVeil(null);
  }, [view, page]);

  // 首页数据落盘（30 分钟 TTL）：重启/会话恢复时零请求直出；数据落定 1.5s 后快照
  useEffect(() => {
    if (view !== "home" || !carousel.length) return;
    const t = setTimeout(() => {
      try {
        localStorage.setItem(
          "fntv-home-cache",
          JSON.stringify({
            t: Date.now(),
            libs,
            continueList,
            carousel,
            carouselBds,
            carouselLogos,
            sum,
            favTotal,
            libRows,
          }),
        );
      } catch {
        /* 配额满等忽略 */
      }
    }, 1500);
    return () => clearTimeout(t);
  }, [view, libs, continueList, carousel, carouselBds, carouselLogos, sum, favTotal, libRows]);

  // [lc-118] 首页轮播的「开始播放」= **直接拉起独立 mpv 窗口**（与详情页同一个入口）。
  const [playMsg, setPlayMsg] = useState("");
  const play = useCallback((t: PlayTarget) => {
    setPlayMsg("");
    invoke("play_item", { guid: t.guid })
      .then(() => {
        setPlayMsg(`已在独立播放器窗口打开${t.title ? `：${t.title}` : ""}`);
        window.setTimeout(() => setPlayMsg(""), 4000);
      })
      .catch((e) => {
        setPlayMsg(`播放失败：${String(e)}`);
        window.setTimeout(() => setPlayMsg(""), 6000);
      });
  }, []);

  const openItem = useCallback(
    (item: MediaItem) => {
      setDetailItem(item);
      setDetailFrom(view === "lib" ? "lib" : "home");
      setView("detail");
    },
    [view],
  );

  // [lc-1444] 发现页条目 → 主详情页：合成 MediaItem。guid 用 `discover-<tmdbId>` 占位标识
  // （DetailView 借此进入「无飞牛条目」形态：跳过飞牛请求、选集区占位、
  //  TMDB 定位直接用条目自带的 tmdb_id，不用等飞牛详情的锚点）。
  const openDiscoverDetail = useCallback(
    (it: TmdbDiscoverItem, mediaType: "tv" | "movie") => {
      setDetailItem({
        guid: `discover-${it.tmdbId}`,
        title: it.title,
        overview: it.overview,
        release_date: it.date || (it.year ? `${it.year}-01-01` : ""),
        tmdb_id: it.tmdbId,
        type: mediaType === "movie" ? "Movie" : "TV",
      });
      setDetailFrom("discover");
      setView("detail");
    },
    [],
  );

  // [lc-1444] 发现页右栏（新番表）条目 → 主详情页：条目自带 Bangumi subject id，
  //   guid 用 `discover-bgm-<id>` 占位标识；封面直接带（代理 URL）；TMDB 侧没有 id，走标题搜索兜底。
  const openDiscoverBgm = useCallback((it: BgmCalendarItem) => {
    setDetailItem({
      guid: `discover-bgm-${it.id}`,
      title: it.name_cn || it.name,
      release_date: it.air_date,
      poster: bgmImg(it.cover),
      bgm_id: it.id,
      type: "TV",
    });
    setDetailFrom("discover");
    setView("detail");
  }, []);

  const openFav = useCallback(() => {
    setActiveLib({ guid: "_fav", title: "收藏", category: "" });
    setPage("home");
    setView("lib");
  }, []);

  const openCat = useCallback((cat: { key: string; types: string[] }) => {
    setActiveLib({ guid: `_cat:${cat.types.join(",")}`, title: cat.key, category: "" });
    setPage("home");
    setView("lib");
  }, []);

  useEffect(() => {
    invoke("check_session")
      .then(() => {
        // 30 分钟内的落盘快照直接恢复（重启零请求直出）；否则完整加载（骨架屏）
        let restored = false;
        try {
          const raw = localStorage.getItem("fntv-home-cache");
          const c = raw ? JSON.parse(raw) : null;
          if (
            c &&
            typeof c.t === "number" &&
            Date.now() - c.t <= 30 * 60 * 1000 &&
            Array.isArray(c.carousel) &&
            c.carousel.length &&
            Array.isArray(c.libs)
          ) {
            setLibs(c.libs);
            setContinueList(Array.isArray(c.continueList) ? c.continueList : []);
            setCarousel(c.carousel);
            setCarouselBds(c.carouselBds ?? {});
            setCarouselLogos(c.carouselLogos ?? {});
            setSum(c.sum ?? {});
            setFavTotal(typeof c.favTotal === "number" ? c.favTotal : 0);
            const rows0 =
              Array.isArray(c.libRows) && c.libRows.length
                ? c.libRows
                : c.libs.map((l: Lib) => ({ lib: l, items: [] }));
            setLibRows(rows0);
            // [lc-112] 快照直出后**仍要把各库第一页补齐**：快照里 items 常是空的
            // （写快照那一刻往往还没填完），空 items ⇒ 整排不渲染；而 restored=true
            // 意味着不会再走完整加载 —— 结果就是"首页只剩继续观看"，且 30 分钟内反复出现。
            rows0.forEach((row: { lib: Lib; items: MediaItem[] }, idx: number) => {
              if (row.items.length) return;
              fetchLibItems(row.lib.guid, null)
                .then((items) => {
                  setLibRows((prev) => {
                    const next = [...prev];
                    if (next[idx]) next[idx] = { lib: next[idx].lib, items };
                    return next;
                  });
                })
                .catch((e) => setErr(`加载媒体库「${row.lib.title}」失败：${String(e)}`));
            });
            setPage("home");
            setView("home");
            restored = true;
          }
        } catch {
          /* 缓存损坏 → 走完整加载 */
        }
        if (!restored) loadHome();
        setBooting(false);
      })
      .catch(() => {
        setBooting(false);
        setView("login");
      });
  }, [loadHome]);

  // [lc-123-rev6] 常驻视图的动画类：进入侧挂 anim-*、退场侧额外加 leaving（退场层 position:absolute 叠在上面，
  // pointer-events:none —— 动画中照样能点，不锁交互）。动画类只在这 ~260ms 里存在，结束即摘。
  const paneCls = (which: "home" | "discover") => {
    const active = view === which;
    const entering = paneAnim && active && paneAnim.leaving !== which;
    const leaving = paneAnim && !active && paneAnim.leaving === which;
    return (
      `view-pane${active ? " active" : ""}` +
      (paneAnim && (entering || leaving) ? ` anim-${paneAnim.dir}` : "") +
      (leaving ? " leaving" : "") +
      // [lc-1513] 从详情页返回：目标主页补一次进入淡入（class 添加即触发）
      (pageAnim && active ? " view-in" : "")
    );
  };

  let body: ReactNode;
  if (booting) {
    body = null; // 会话检查期不出任何视图
  } else if (view === "login") {
    body = <LoginPage onDone={() => loadHome()} />;
  } else {
    // 首页常驻挂载：切到库/详情只隐藏不卸载（图片零重请求、滚动位置保留）
    const homeReady = carousel.length > 0;
    body = (
      <div className="main-row">
        {/* [lc-1427] 侧栏改为覆盖式浮层后的配套交互：展开时铺一层**透明遮罩**，
            点击它收起侧栏（覆盖式侧栏的标准行为）。它必须：
              · 在侧栏之下（z-index 44 < 侧栏 45）→ 不挡侧栏自己的点击；
              · 在顶栏之下（< 50）→ 汉堡键仍可点（否则点不开/关不上）；
              · 完全透明（不加暗化）—— 用户要的是"覆盖上层"而不是"模态弹窗"，
                加暗化会让首页变暗、显得像对话框。 */}
        {sidebarOpen && (
          <div
            className="sidebar-scrim"
            aria-hidden="true"
            onClick={() => {
              setSidebarOpen(false);
              localStorage.setItem("fntv-sidebar-open", "0");
            }}
          />
        )}
        <Sidebar
          activeGuid={view === "home" ? null : activeLib?.guid ?? null}
          collapsed={!sidebarOpen}
          username={username}
          server={server}
          sum={sum}
          favTotal={favTotal}
          onOpenFav={openFav}
          onOpenCat={openCat}
          onHome={goHome}
          onOpenSettings={() => setSettingsOpen(true)}
        />
        <div className="view-stack">
          {/* [lc-123-rev2] 「首页 | 发现」胶囊是**浮层**（.navbar = absolute，不占布局高度）：
              浮在内容区（.view-stack，侧栏右侧）顶部居中、盖在海报上；首页 hero 保持满幅紧贴标题栏。
              只在两个"主页"上出现：库/详情是带返回入口的子页，且详情页的分段栏（.dseg，sticky top 40px）
              正好在胶囊的落点上，浮上去会压住中间的 tab（见 App.css 注释）。 */}
          {(view === "home" || view === "discover") && (
            <div className="navbar">
              <NavCapsule page={page} onNav={onNav} />
            </div>
          )}
          <div
            className={paneCls("home")}
            onAnimationEnd={() => {
              clearPaneAnim();
              setPageAnim(null);
            }}
          >
            {homeReady ? (
              <HomeView
                libs={libs}
                continueList={continueList}
                carousel={carousel}
                carouselBds={carouselBds}
                carouselLogos={carouselLogos}
                carouselLogoOn={settings.carouselLogo}
                launchPage={settings.launchPage}
                libRows={libRows}
                hiddenLibs={hiddenLibs}
                onToggleHiddenLib={toggleHiddenLib}
                onOpenLib={(l) => {
                  setActiveLib(l);
                  setView("lib");
                }}
                onPlay={play}
                onOpenItem={openItem}
                onHeroTone={onHeroTone}
              />
            ) : homeLoading ? (
              <HomeSkeleton />
            ) : null}
          </div>
          {/* [lc-123] 发现页：与首页同款「常驻挂载」——两栏的滚动位置/已取数据在切页后保留；
              首次拉取的闸门是 active={view === "discover"}（不进这一页就不发任何请求） */}
          <div
            className={paneCls("discover")}
            onAnimationEnd={() => {
              clearPaneAnim();
              setPageAnim(null);
            }}
          >
            <DiscoverView
              active={view === "discover"}
              onOpenDetail={openDiscoverDetail}
              onOpenBgm={openDiscoverBgm}
            />
          </div>
          {(view === "lib" || libMounted) && activeLib && (
            <div
              className={`view-pane active${libClosing ? " view-out" : libEnterAnim ? " view-in" : ""}`}
              onAnimationEnd={() => setLibEnterAnim(false)}
            >
              <LibView lib={activeLib} onBackHome={goHome} onOpenItem={openItem} />
            </div>
          )}
          {/* [lc-1513] 详情/库页：进入播 view-in；返回时（view 切走）先播 view-out
              再延迟卸载；返回目标（home/discover/lib）同时补一次 view-in 淡入，
              不会出现「返回后旧视图瞬间闪回、新视图硬切」。 */}
          {(view === "detail" || detailMounted) && detailItem && (
            <div className={`view-pane active${detailClosing ? " view-out" : " view-in"}`}>
              {/* [lc-1444] 返回目标按进详情的来源（detailFrom）决定，不用 view/activeLib 现猜 */}
              <DetailView
                base={detailItem}
                onBack={() => {
                  // 返回动画：目标 pane 补一次进入淡入（class 添加即触发，animationend 后移除）
                  if (detailFrom === "lib") setLibEnterAnim(true);
                  else setPageAnim("view-in");
                  setView(detailFrom === "discover" ? "discover" : detailFrom === "lib" ? "lib" : "home");
                }}
              />
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="app-shell">
      {/* [lc-118] 播放反馈浮标：轮播/首页没有 err 区域，播放失败必须有可见反馈 */}
      {playMsg && (
        <div
          style={{
            position: "fixed",
            left: "50%",
            bottom: 28,
            transform: "translateX(-50%)",
            padding: "10px 18px",
            borderRadius: 12,
            zIndex: 9999,
            background: "rgba(20,22,28,.92)",
            color: "#fff",
            fontSize: 13,
            boxShadow: "0 8px 28px rgba(0,0,0,.45)",
            pointerEvents: "none",
          }}
        >
          {playMsg}
        </div>
      )}
      {/* [lc-094] 无边框窗口的缩放热区：tao 自带热区只有窗口外侧 SM_CXFRAME（175% 缩放 ≈ 7 物理像素
          ≈ 4 CSS px），那几像素又常被 WebView2 子窗吃掉 → 手感"不灵敏、有时拖不动"。
          这里在四边/四角铺透明拖拽条，由 WebView 收指针后调 startResizeDragging，热区稳定。 */}
      {(
        [
          ["North", "rsz rsz-n"],
          ["South", "rsz rsz-s"],
          ["West", "rsz rsz-w"],
          ["East", "rsz rsz-e"],
          ["NorthWest", "rsz rsz-nw"],
          ["SouthWest", "rsz rsz-sw"],
          ["SouthEast", "rsz rsz-se"],
        ] as const
      ).map(([dir, cls]) => (
        <div
          key={dir}
          className={cls}
          onPointerDown={(e) => {
            e.preventDefault();
            getCurrentWindow()
              .startResizeDragging(dir)
              .catch(() => {});
          }}
        />
      ))}
      {/* [lc-123-rev7] 深浅切换的**镂空反向扩散**：旧主题印象层（CSS 变量底色，无截图通道）
          用 clip-path: circle() 从 150% 收缩到 0 —— 圆内是旧界面、圆外已是新界面，"透明的外部扩散"。
          原点来自按钮 rect（--tx/--ty）；z-index 80 > 顶栏 50；pointer-events:none；
          animationend 后从 DOM 移除。 */}
      {veil && (
        <div
          className="theme-veil"
          style={
            {
              "--tx": `${veil.x}px`,
              "--ty": `${veil.y}px`,
            } as React.CSSProperties
          }
          onAnimationEnd={onVeilEnd}
          aria-hidden="true"
        />
      )}
      <TitleBar
        title="AniBox"
        nav={view !== "login"}
        page={page}
        sidebarOpen={sidebarOpen}
        onToggleSidebar={() =>
          setSidebarOpen((v) => {
            localStorage.setItem("fntv-sidebar-open", v ? "0" : "1");
            return !v;
          })
        }
        onRefresh={() => loadHome()}
        refreshing={homeLoading}
        // [lc-123-rev3] 顶栏现在**一律全透明**（无底色/描边/渐变），可读性靠 text-shadow；
        // overlay/ghost 两个"底色模式"修饰类随之取消（App.css 里的 .titlebar.overlay/.ghost 已删）。
        // [lc-123-rev4] 首页 hero 是满幅图 → chrome 色按**图上条带亮度采样**自适应；
        // 其余页（发现/库/详情）顶栏底下是应用底色 → "auto" 跟随主题（浅色主题深字 / 深色主题浅字）。
        chrome={view === "home" ? heroTone : "auto"}
        onToggleTheme={toggleTheme}
      />
      {err && view !== "login" && <div className="err global-err">{err}</div>}
      {settingsOpen && (
        <SettingsPanel
          onClose={() => setSettingsOpen(false)}
          settings={settings}
          onSettings={(patch) => void patchSettings(patch)}
          onLogout={() => {
            // [lc-1437] 退出登录：清会话 + 清首页快照（那是上一个账号的数据，
            // 留着会让下一个账号先看到别人的片单）+ 回登录页
            setSettingsOpen(false);
            invoke("logout").catch(() => {});
            try {
              localStorage.removeItem("fntv-home-cache");
            } catch {
              /* ignore */
            }
            setCarousel([]);
            setCarouselBds({});
            setCarouselLogos({});
            setLibRows([]);
            setLibs([]);
            setContinueList([]);
            setView("login");
          }}
        />
      )}
      {/* [lc-1439] 关闭询问框：原生关闭按钮被 Rust 拦下后弹（三选一）。
          用 fixed 浮层而非复用设置面板 —— 它必须在任何页面状态下都能弹出来。
          [lc-1514] 玻璃化（半透明 + 动态模糊）+「记住我的选择」：勾上则把
          exit_mode 写进 settings，之后 Rust 的 CloseRequested 直接按记忆执行，不再问。 */}
      {closeAsk && (
        <div className="close-ask" onClick={() => setCloseAsk(false)}>
          <div className="close-ask-box" onClick={(e) => e.stopPropagation()}>
            <h3>关闭窗口</h3>
            <p>要退出程序，还是最小化到系统托盘继续在后台运行？</p>
            <label className="close-ask-remember">
              <input
                type="checkbox"
                checked={rememberClose}
                onChange={(e) => setRememberClose(e.target.checked)}
              />
              <span>记住我的选择，下次不再询问</span>
            </label>
            <div className="close-ask-acts">
              <button className="set-btn" onClick={() => setCloseAsk(false)}>
                取消
              </button>
              <button
                className="set-btn"
                onClick={() => {
                  setCloseAsk(false);
                  if (rememberClose) patchSettings({ exitMode: "tray" });
                  invoke("resolve_close", { action: "hide" }).catch(() => {});
                }}
              >
                最小化到托盘
              </button>
              <button
                className="set-btn danger"
                onClick={() => {
                  setCloseAsk(false);
                  if (rememberClose) patchSettings({ exitMode: "exit" });
                  invoke("resolve_close", { action: "exit" }).catch(() => {});
                }}
              >
                退出程序
              </button>
            </div>
          </div>
        </div>
      )}
      {body}
    </div>
  );
}

export default App;
