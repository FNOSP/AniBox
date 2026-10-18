/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */


export type Config = {
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
  /** [lc-1630] Bangumi 访问令牌（敏感，仅用于本地恢复；不展示） */
  bgm_token?: string;
  /** [lc-1630] 上次绑定成功的 Bangumi 账号名（缓存展示用） */
  bgm_username?: string;
  /** [lc-1630] 记住上次账号：下次启动自动恢复 */
  bgm_remember?: boolean;
  /** 弹幕引擎：`"node"`（默认）| `"rust"`（测试版）。与 Rust 侧 `danmu_engine` 同名（snake_case） */
  danmu_engine?: string;
};

/**
 * [lc-1630] Bangumi 账号状态（`bgm_account_status` / `bgm_account_bind` /
 * `bgm_account_set_remember` 的统一返回结构）。
 *
 * 关键：`bound` 是"是否已绑定"的唯一依据；`verified` 只表示"本次网络校验是否成功"。
 * 网络/镜像失败时可能是 `bound=true, verified=false`，**不得**据此显示为未连接。
 */
export type BgmAccountState = {
  bound: boolean;
  /** 本次网络校验是否成功（缺省视为 true，兼容旧后端） */
  verified?: boolean;
  /** 本次校验拿到的账号名（可能为空，展示时回退缓存） */
  username?: string;
  /** 校验失败时的错误说明（仅提示用） */
  error?: string;
  /** 是否记住此账号（下次启动自动恢复） */
  remember?: boolean;
};


export type Lib = { guid: string; title: string; category: string; posters?: string[] };

/** item/list（分页查询）返回的条目，snake_case */

export type MediaItem = {
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

export type PlayItem = Record<string, unknown>;

/** episode/list 返回的剧集，PlayListItem 形状（snake_case） */

export type Episode = {
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


export type Season = {
  guid: string;
  title: string;
  season_number?: number;
  number_of_episodes?: number;
};

/** [lc-119] Bangumi 匹配结果（src-tauri/src/bgm.rs::bgm_match；全线走自建镜像） */

export type BgmMatch = {
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

export type BgmReview = {
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

export type BgmComment = {
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


export type PlayTarget = { guid: string; title: string; tvTitle?: string };

/** Bangumi 角色（/v0/subjects/{id}/characters，裸数组）：relation=主角/配角/客串；type=分类；
 *  actors=声优列表（取首位展示）；images 与吐槽头像同形（large/grid/medium/small，可相对路径）。 */

export type BgmCharacter = {
  id?: number;
  name?: string;
  relation?: string;
  type?: number;
  images?: Record<string, string> | null;
  actors?: { id?: number; name?: string; images?: Record<string, string> | null }[];
};

/** Bangumi 制作人员（/v0/subjects/{id}/persons，裸数组）：relation=职位（导演/脚本/原作…）；
 *  career 是职业列表（无 relation 时兜底）。 */

export type BgmPerson = {
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

export type TmdbDiscoverItem = {
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
  /** [lc-1577] TMDB 中文题材名（列表端点只有 genre_ids，Rust 侧用官方表映射；未知 id 丢弃 ⇒ 可能空数组） */
  genres: string[];
};

/** [lc-123] Bangumi 每日新番表条目（src-tauri/src/bgm.rs::bgm_calendar 归一化后）。 */

export type BgmCalendarItem = {
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

export type BgmCalendarDay = { id: number; cn: string; en: string; ja: string; count: number; items: BgmCalendarItem[] };

/** Bangumi 剧集行（/v0/episodes 的 data[]；只取渲染需要的字段） */

export type BgmEpisodeRow = {
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

export type BgmRelated = {
  id?: number;
  type?: number;
  name?: string;
  name_cn?: string;
  relation?: string;
  images?: Record<string, string> | null;
};

/** [lc-1401] Bangumi 标签（/v0/subjects/{id} 的 tags[]）：count = 该标签被标次数（热度序） */

export type BgmTag = { name?: string; count?: number };

/** [lc-128] TMDB 演员 / 制作人员（src-tauri/src/tmdb.rs::tmdb_credits 归一化后）。
 *  剧集带 roles[].episodes（aggregate_credits 的"演了几集"）；crew 带 job/jobLabel（中文职务）。 */

export type TmdbCreditPerson = {
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

export type TmdbCredits = {
  tmdbId?: number;
  mediaType?: string;
  matchedBy?: string;
  cached?: boolean;
  cast?: TmdbCreditPerson[];
  crew?: TmdbCreditPerson[];
};

/* ────────────────── [lc-1700] 演员页（TMDB / Bangumi 人物） ────────────────── */

/** 人物数据来源：TMDB = 全球影视库；bgm = Bangumi（动画/三次元） */
export type PersonSource = "tmdb" | "bgm";

/** 从详情页点进演员页的目标（只带最少字段，详情由演员页自己拉） */
export type PersonTarget = {
  source: PersonSource;
  id: number;
  name: string;
  /** 头像：TMDB 为 file_path（/xxx.jpg），BGM 为 images.grid 之类的相对/完整路径 */
  profile?: string;
};

/** 演员页的一部作品（兼容 TMDB / Bangumi 两种返回；缺失字段按 source 解释） */
export type PersonWork = {
  /** TMDB 作品 id（source = tmdb） */
  tmdbId?: number;
  /** Bangumi 条目 id（source = bgm） */
  bgmId?: number;
  mediaType: "movie" | "tv" | "anime" | "real" | "other";
  title: string;
  originalTitle?: string;
  year?: string | number;
  poster?: string;
  /** TMDB 横图 file_path（BGM 无） */
  backdrop?: string;
  /** TMDB：饰演角色 */
  character?: string;
  /** TMDB：职务（导演/编剧…） */
  job?: string;
  /** BGM：职位 */
  staff?: string;
  rating?: number | null;
  votes?: number;
  /** TMDB 剧集：出演集数 */
  episodeCount?: number;
  /** BGM 条目类型码（2 动画 / 6 三次元 …） */
  subjectType?: number;
};

/** 演员页的人物资料（TMDB / Bangumi 归一后的统一形状） */
export type PersonInfo = {
  source: PersonSource;
  /** tmdbId / bgmId 归一后的 id */
  id: number;
  name: string;
  originalName?: string;
  /** 已解析为可直接使用的图片 URL */
  profile?: string;
  /** TMDB 传记 */
  biography?: string;
  /** Bangumi 简介 */
  summary?: string;
  birthday?: string;
  deathday?: string;
  placeOfBirth?: string;
  /** TMDB：known_for_department */
  knownFor?: string;
  /** Bangumi：职业列表 */
  career?: string[];
  bloodType?: string;
  works: PersonWork[];
};

/** [更新与维护] 后端 `app_info()`：软件名 / 版本 / 仓库地址 / 简介。
 *  前端优先用它填「关于」，字段为空时回退到占位常量。 */
export type AppInfo = {
  name: string;
  version: string;
  repoUrl: string;
  description: string;
};

/** [更新与维护] 更新类型：hotfix = 热补丁（无需重装）；full = 全量安装包 */
export type UpdateType = "hotfix" | "full";

/** [更新与维护] 后端 `update_check()` 的返回。字段按固定契约（camelCase）；
 *  前端在读取时兼容 snake_case（见 SettingsPanel 的归一化函数）。 */
export type UpdateCheckResult = {
  hasUpdate: boolean;
  currentVersion: string;
  latestVersion: string;
  updateType: UpdateType;
  notes: string;
  publishedAt: string;
  downloadUrl: string;
  patchUrl: string;
  htmlUrl: string;
};

/** [更新与维护] 后端 `update_download()` 的返回：落盘路径与字节数 */
export type UpdateDownloadResult = { path: string; size: number };

/** [更新与维护] 后端 `patch_check()` 的返回 */
export type PatchCheckResult = { hasPatch: boolean; version: string; notes: string };

/** [更新与维护] `update-progress` / `patch-progress` 事件 payload。
 *  percent 可能缺省（旧后端），此时前端用 loaded/total 现算。 */
export type UpdateProgress = {
  phase: string;
  loaded: number;
  total: number;
  percent: number;
  message: string;
};

/** [player] 播放器类型：mpv = 随包自带（默认）；potplayer = 本机安装的 PotPlayer */
export type PlayerKind = "mpv" | "potplayer";

export type ChromeTone = "auto" | "dark" | "light" | "fallback";
/** [lc-1437] 设置面板的项（Rust 侧 `get_settings`/`set_settings` 的白名单子集）。
 *  放在 App 顶层持有：性能模式（html.perf 类）与轮播 Logo（CarouselView 入参）
 *  都要在面板之外生效，面板只是它们的编辑界面。 */

export type AppSettings = {
  exitMode: "exit" | "tray" | "ask";
  perfMode: boolean;
  carouselLogo: boolean;
  launchPage: "home" | "discover";
  autoLogin: boolean;
  /** [player] 播放器选择（默认 mpv = 随包自带）。后端始终返回，旧缓存可能缺省。 */
  playerKind?: PlayerKind;
  /** [player] PotPlayer 可执行文件路径；空串 = 播放时自动探测常见安装路径 */
  potplayerPath?: string;
  /** [lc-1561] 自建弹幕接口（danmu_api）开关 —— 对齐主仓库同名设置 */
  danmuApiEnabled: boolean;
  /** 多地址列表（故障转移，≤4 条；danmuApiBase 恒等于第一条） */
  danmuApiBases: string[];
  danmuApiBase: string;
  /** 弹幕引擎（camelCase 映射 Rust `danmu_engine`）："node" = 默认/稳定，"rust" = 测试版。
   *  仅在「自建弹幕接口」启用时生效；旧缓存可能缺省 → 读取时回退 "node"。 */
  danmuEngine?: "node" | "rust";
  /** 手柄：开关与高级参数（真值与处理均在 Rust；前端只做编辑与显示） */
  gamepadEnabled?: boolean;
  gamepadDeadzone?: number;
  gamepadDirectionThreshold?: number;
  gamepadRepeatDelay?: number;
  gamepadRepeatInterval?: number;
};
