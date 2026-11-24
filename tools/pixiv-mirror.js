/**
 * [lc-2356] pixiv-mirror —— 自建 Pixiv 图片镜像（Cloudflare Worker）。
 *
 * ## 为什么需要它
 *
 * `i.pximg.net` 有 Referer 防盗链（无 Referer → 403），而公共镜像
 * `i.pixiv.cat` / `i.pixiv.nl` 在大陆时通时不通。自建一个就永远可控。
 *
 * ## 协议（与项目现有 tmdb/bgm 镜像同一套风格）
 *
 * ```
 *   GET /img/<rest>   →  https://i.pximg.net/<rest>      （原图）
 *   GET /c/<rest>     →  https://i.pximg.net/c/<rest>    （缩放版，pixiv 自己的 /c/ 路由）
 * ```
 *
 * 统一注入 `Referer: https://www.pixiv.net/` 绕过防盗链 —— 这是唯一必需的伪装，
 * 不做任何 UA 伪造（pixiv 的图片站不校验 UA）。
 *
 * ## 边缘缓存
 *
 * 图片是不可变资源（同一个 URL 永远同一张图），缓存 30 天，`immutable`。
 * 用 `caches.default` 而不是 KV/R2：零成本、零配置、全球边缘。
 *
 * ## 部署
 *
 * ```bash
 * npx wrangler deploy pixiv-mirror.js --name anibox-pixiv-mirror
 * # 然后在 Cloudflare 里绑自定义域：pixiv.690075.xyz
 * ```
 *
 * ## 不做的事（如实标注）
 *
 * - **不代理 `app-api.pixiv.net`**：那需要 OAuth，且每用户 token 不同。
 *   客户端直连（实测可达，只是缺 auth），不经过镜像。
 * - **不代理 `www.pixiv.net`**：网页站的 ranking/ajax 实测直连可用，无需镜像。
 * - **不缓存搜索/排行结果**：那些是动态数据，缓存会给出过期榜单。
 */

/** 上游图片域（pixiv 官方 CDN）。 */
const UPSTREAM = "https://i.pximg.net";

/** 上游需要的 Referer（防盗链唯一要求）。 */
const REFERER = "https://www.pixiv.net/";

/** 边缘缓存时长：图片不可变，给满。 */
const CACHE_SECONDS = 30 * 24 * 3600;

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);

    // 健康检查（部署后自测用）
    if (url.pathname === "/" || url.pathname === "/health") {
      return new Response(
        JSON.stringify({ ok: true, service: "anibox-pixiv-mirror", version: 1 }),
        { headers: { "content-type": "application/json" } },
      );
    }

    // 只接受 /img/ 与 /c/ 两条路径（对应原图与缩放版）
    const ok = url.pathname.startsWith("/img/") || url.pathname.startsWith("/c/");
    if (!ok) {
      return new Response(
        JSON.stringify({
          error: "unsupported path",
          hint: "只支持 /img/* 与 /c/*（对应 i.pximg.net 的两个路由）",
        }),
        { status: 404, headers: { "content-type": "application/json" } },
      );
    }

    const upstreamUrl = UPSTREAM + url.pathname + url.search;

    // 边缘缓存优先：命中直接回，不碰上游
    const cache = caches.default;
    const cacheKey = new Request(upstreamUrl, { method: "GET" });
    const hit = await cache.match(cacheKey);
    if (hit) {
      // 加一个头便于确认「这次是缓存命中」，排查时不用抓包
      const r = new Response(hit.body, hit);
      r.headers.set("x-anibox-cache", "hit");
      return r;
    }

    let upstream;
    try {
      upstream = await fetch(upstreamUrl, {
        headers: {
          Referer: REFERER,
          // 不伪造 UA，也不带 Cookie —— 只解防盗链，不做身份伪装
          Accept: "image/avif,image/webp,image/png,image/jpeg,*/*",
        },
        // 图片站可能 302 到 CDN
        redirect: "follow",
      });
    } catch (e) {
      return new Response(
        JSON.stringify({ error: "upstream fetch failed", detail: String(e) }),
        { status: 502, headers: { "content-type": "application/json" } },
      );
    }

    if (!upstream.ok) {
      // 404（图不存在）原样透传；其它错误也透传状态码，不吞掉
      return new Response(upstream.body, {
        status: upstream.status,
        statusText: upstream.statusText,
        headers: {
          "content-type": upstream.headers.get("content-type") || "application/octet-stream",
          "x-anibox-cache": "miss",
        },
      });
    }

    const ct = upstream.headers.get("content-type") || "";
    // 只缓存真图片（上游有时返回 HTML 错误页但状态 200）
    const isImage = ct.startsWith("image/");
    const headers = new Headers();
    headers.set("content-type", isImage ? ct : "application/octet-stream");
    headers.set(
      "cache-control",
      isImage ? `public, max-age=${CACHE_SECONDS}, immutable` : "no-store",
    );
    // 允许任意来源取图（我们用自定义协议 fntvimg 或直连，跨源是常态）
    headers.set("access-control-allow-origin", "*");

    const resp = new Response(upstream.body, { status: 200, headers });

    // 只把真图片写进边缘缓存（HTML 错误页缓存了会长期脏）
    if (isImage) {
      const toCache = resp.clone();
      const cacheHeaders = new Headers(toCache.headers);
      cacheHeaders.set("x-anibox-cache", "miss");
      ctx.waitUntil(
        cache.put(cacheKey, new Response(toCache.body, { status: 200, headers: cacheHeaders })),
      );
    }

    return resp;
  },
};
