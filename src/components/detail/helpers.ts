/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */

export function pickGenres(d: Record<string, unknown> | null): string[] {
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

export const TAB_LABEL: Record<string, string> = {
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

export const BGM_PAGE = 20;

/** [lc-1412] Bangumi 时间戳 → 展示串。
 *  上游给的是 Unix 秒（comments.updatedAt / reviews.entry.createdAt）。
 *  规则：7 天内显示相对时间（气泡流的常规做法，用户设计稿就是 "1d 23h ago"），
 *  更早显示日期（同年不带年、跨年带年）。**解析不了就返回空串**（不显示比显示错的好）。 */

export function bgmList<T>(v: unknown): T[] {
  if (Array.isArray(v)) return v as T[];
  const d = (v as { data?: unknown } | null)?.data;
  return Array.isArray(d) ? (d as T[]) : [];
}

/** /characters 的 type 是分类（1 角色 / 2 机体 / 3 舰船 / 4 组织）；relation 才是主角/配角/客串 */

export const CHAR_TYPE_LABEL: Record<number, string> = { 1: "角色", 2: "机体", 3: "舰船", 4: "组织" };

/** Bangumi 列表骨架屏（吐槽 / 更多 共用；与既有 .poster-skeleton 同风格；行宽错开避免机械感） */

export function fnosCast(d: Record<string, unknown> | null): { name: string; role: string; avatar: string }[] {
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
