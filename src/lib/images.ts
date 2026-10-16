/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */

export function imgUrl(p?: string | null): string {
  if (!p) return "";
  // [lc-1433] 幂等：已经是本协议的完整 URL 就原样返回。
  //   为什么需要：`tmdbImg()` / `bgmImg()` 产出的就是完整代理 URL，而消费处一律写
  //   `imgUrl(poster)` —— 直接传进去会被再包一层（`.../http://fntvimg.localhost/...`）→ 404。
  //   上游（如详情页 poster 链路）现在是"TMDB 完整 URL 与飞牛相对路径混在一个变量里"，
  //   在这里兜住比在每个调用点判来源更可靠。
  if (p.startsWith("http://fntvimg.localhost") || p.startsWith("fntvimg://localhost")) return p;
  // [lc-1600] 其它绝对 URL（http/https）同样直通：AniList 封面/横图（s4.anilist.co 等）不在
  //   fntvimg 的三个上游（NAS / TMDB / Bangumi）里，包一层只会变成 `.../https://...` 的 404 路径。
  //   与 CastTab 的 `avatar.startsWith("http") ? avatar : imgUrl(avatar)` 是同一语义。
  if (p.startsWith("http://") || p.startsWith("https://")) return p;
  const path = p.startsWith("/") ? p : `/${p}`;
  if (navigator.userAgent.includes("Windows")) return `http://fntvimg.localhost${path}`;
  return `fntvimg://localhost${path}`;
}

/** [lc-119] Bangumi 头像/封面：走 fntvimg 代理的 `/__bgm/*`（**拉了即落盘**，下次零网络），
 *  兼容三种形态：完整 URL / 协议相对 `//lain.bgm.tv/...` / 相对路径 `/pic/user/l/...`。
 *  [lc-123] `/calendar` 的封面是**明文 http://lain.bgm.tv**（该端点的老式返回）——WebView 是 https 页面
 *  上下文，http 图会被当混合内容拦掉 ⇒ 整片裂图。这里统一升到 https（同一主机，站点支持）。
 *  [lc-128] 其它域名（理论上不该有）不代理、不改写：fntvimg 只认识 NAS / TMDB / Bangumi 三个上游。 */

export function bgmImg(u?: string | null): string {
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

export function tmdbImg(path: string, size: string): string {
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

export function pickImgStr(d: Record<string, unknown>, keys: string[]): string | undefined {
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

export function pickLargestImg(v: unknown): string | undefined {
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
