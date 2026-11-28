# Trakt API 能力盘点（AniBox 集成参考）

> 整理日期：2026-11-28 · 数据来源：[developer.trakt.tv OpenAPI spec](https://developer.trakt.tv/openapi.json)（333 个端点实拉解析）+ 本轮实测
> 适用：AniBox fpk 网页端（`fpk/src/go/internal/bridge/bridge.go` Trakt 模块）与后续桌面端规划

---

## 0. 授权现状

- 应用：Trakt 开发者后台 **Production App 279547「AniBox」**（2026-11-28 审核即批；Development App 279494 为沙盒）
- 授权方式：OAuth **设备授权流**（`device/code` → `auth.trakt.tv/activate` 输码 → 轮询 `device/token`），仅需 Client ID（PKCE 口径不发放 Secret）
- Client ID 为公开标识符，编译内置（自填凭证优先，回退内置）
- **权限实测**：设备流授权即可读写用户数据（`sync/history` 写入已真机验证），无需额外 scope 申请；token 含 `offline_access`（refresh 可续期，refresh token 单次有效——每次刷新必须替换存储值）
- 令牌存用户 NAS 本地设置（`trakt_access_token` / `trakt_refresh_token` / `trakt_expires_at`），401 自动刷新一次重试

## 1. AniBox 已实现的端点

| 端点 | 用途 |
|---|---|
| `POST /oauth/device/code` | 申请设备码（仅需 client_id） |
| `POST /oauth/device/token` | 轮询换 token（400=待授权 429=放慢 410=过期 418=拒绝） |
| `POST /oauth/token` | refresh_token 续期 |
| `POST /oauth/revoke` | 断开连接吊销（required: token + client_id） |
| `POST /sync/history` | 库内已识别作品批量写入观影历史（电影 + 剧集 show 级，Trakt 自动展开全季集） |
| `POST /scrobble/start` `pause` `stop` | 播放实时打点：起播 / 暂停 / 看完≥80% 自动落历史，<80% 停下即留续播点（playback） |

## 2. 能从 Trakt 取的数据（读）

### 2.1 用户账号数据（授权后）

| 数据 | 端点 | AniBox 价值 |
|---|---|---|
| 全量已看（电影 + 每剧集数，含 last_watched_at） | `GET /sync/watched/{movies\|shows}` | ★★★ 反向回填：换 NAS / 首装把 Trakt 历史灌回飞牛「已观看」 |
| 观看历史明细（含观看时间戳，分页） | `GET /sync/history/{type}/{id}`、`GET /users/me/history/{type}/{item_id}` | ★★ 精确到哪天看的哪一集 |
| 剧集观看进度（看到第几集） | `GET /sync/progress/watched`、`GET /sync/progress/up_next`（up_next_nitro 为 VIP 增强） | ★★★ 「继续看」跨设备 |
| 未看完的续播点（秒级进度） | `GET /sync/playback`（分 `episodes` / `movies` 变体） | ★★★ scrobble pause/stop(<80%) 留下的即续播点 |
| 评分（1-10 分，分类型/分值过滤） | `GET /sync/ratings/{type}/{rating}`、`GET /users/me/ratings/*` | ★★★ 评分双向同步 |
| 待看清单 | `GET /sync/watchlist/{type}/{sort_by}/{sort_how}` | ★★ Trakt 想看 → 发现页 / 飞牛想看 |
| 媒体收藏 | `GET /sync/collection/{type}`（含 minimal 变体） | ★★ 与媒体库对照找缺 |
| 收藏夹 | `GET /sync/favorites/{type}/{sort_by}/{sort_how}` | ★ |
| 增量同步锚点 | `GET /sync/last_activities` | ★★★ 增量同步必用（ movies/shows/ratings/watchlist 等各自时间戳），免全量拉 |
| 个人清单 / 智能清单 | `GET /users/me/lists/*`、`/users/me/smart-lists/*` | ★ |
| 社交（关注/好友/动态） | `GET /users/{id}/followers、following、friends、{type}/activities` | 低优先 |
| 账号设置 | `GET /users/settings` | ★ |

### 2.2 公开社区数据（仅需 Client ID，无需用户授权）

| 数据 | 端点 | AniBox 价值 |
|---|---|---|
| Trakt 社区评分（含人数，季/集级也有） | `GET /movies\|shows/{id}/ratings`、`.../episodes/{n}/ratings` | ★★★ 详情页 / 首页 hero 显示 Trakt 评分（OMDb 评分同款模式） |
| 热门 / 流行 / 受期待 / 最多人看 / 收藏最多 / 最受喜爱 | `GET /movies\|shows/trending、popular、anticipated、watched、played、collected、favorited`（period 参数） | ★★ 发现页推荐位 |
| 相关作品 | `GET /movies\|shows/{id}/related` | ★★ 详情页「看了又看」 |
| 下一集 / 最新集 | `GET /shows/{id}/next_episode、last_episode` | ★★★ hero 标签行「更新至 X 集」（TMDB 进度滞后的互补权威源） |
| 放送日历（新番 / 首播 / 完结 / 流媒体上线 / DVD） | `GET /calendars/{target}/{kind}/{start_date}/{days}` | ★★ 日历页 |
| 多语言元数据（含简体中文） | `GET /movies\|shows/{id}/translations`（季/集级也有） | ★★ 中文标题/简介兜底 |
| 票房 | `GET /movies/boxoffice` | ★ |
| 正在实时观看某片的人 | `GET /movies\|shows/{id}/watching` | ★ |
| 人物作品 | `GET /people/{id}/movies\|shows` | 已有 TMDB/飞牛源 |
| JustWatch 观看渠道 | `GET .../watchnow/{country}`、`watchnow/justwatch_links/{country}` | ★ |
| 短评（trending/recent + 条目关联） | `GET /comments/*`、`/movies\|shows/{id}/comments/{sort}` | ★ |
| 搜索（文本 / ID 反查） | `GET /search/{type}`、`/search/{id_type}/{id}` | ★ 识别兜底 |
| 统计（投票/观看人数分布） | `GET /movies\|shows\|episodes/{id}/stats` | ★ |

## 3. 能上传的数据（写）

| 能力 | 端点 | AniBox 现状 |
|---|---|---|
| 观看历史（电影/剧集/集级，批量，含 watched_at 时间） | `POST /sync/history`（remove 同路径 `/sync/history/remove`） | ✅ 已实现 |
| 实时打点（起播/暂停/看完） | `POST /scrobble/start` `pause` `stop` | ✅ 已实现 |
| 评分（1-10，批量，含 rated_at） | `POST /sync/ratings`（remove 同名） | 可做：飞牛评分 → Trakt |
| 待看清单（加/删/排序） | `POST /sync/watchlist`（+remove、reorder） | 可做（限额见 §5） |
| 媒体收藏 | `POST /sync/collection`（remove） | 可做 |
| 收藏夹 | `POST /sync/favorites`（remove/reorder/PUT 单项） | 可做 |
| 续播点 | scrobble pause/stop 进度 <80% 自动写 playback；`DELETE /sync/playback/{id}` 清除 | scrobble 已接，回读未做 |
| 打卡 | `POST /checkin`（DELETE 取消） | 价值低 |
| 剧集进度重置 | `POST /shows/{id}/progress/watched/reset`（DELETE 撤销） | **VIP only** |
| 个人清单（建表/增删条目/排序） | `POST /users/me/lists`、`.../lists/{id}/items`（+remove/reorder/PUT 表属性） | 可做：飞牛想看 → Trakt 清单（限额见 §5） |
| 评论 / 点赞 / 关注 / 屏蔽 | `POST /comments/*`、`follow`、`block` 等 | 社交类，低优先 |
| 隐藏项（日历/进度不显示某剧） | `POST /users/hidden/*` | ★ |

## 4. VIP 限制（OpenAPI 实标，非会员返回 426）

### 4.1 VIP Only（11 个）

- `GET /users/{id}/stats` —— 账号统计
- `GET /users/{id}/yir/{year}`（+ `/all`、`/persona`、`/{year_end}`）—— **官方年度报告全系列**
- `GET /users/{id}/mir/{year}/{month}` —— 月度回顾
- `POST|DELETE /shows/{id}/progress/watched/reset` —— 剧集进度重置
- `POST /movies|shows/{id}/refresh/justwatch` —— JustWatch 渠道刷新
- `GET /users/saved_filters/{section}`

> 结论：Trakt 官方统计/年度报告要会员；AniBox 自己的年度报告用 `GET /sync/watched` + `/sync/history` 自行计算即可，不依赖这些。

### 4.2 VIP Enhanced（非会员可用但限额低，超限返回 420）

- `POST /sync/watchlist`
- `POST /users/{id}/lists`、`POST /users/{id}/lists/{list_id}/items`
- `POST /users/{id}/smart-lists`
- `GET /lists/{id}/items/{type}/{sort_by}/{sort_how}`

## 5. 工程注意事项

- **速率**：遵守官方 rate-limiting 规约（读多缓存、增量优先用 `last_activities`，别全量刷）；429=放慢
- **刷新令牌单次有效**：每次 refresh 返回新 access+refresh 对，必须立即替换存储的两个值；换新失败返回 400 `invalid_grant`（旧令牌不可用，需重新授权）
- **show 级提交**：`sync/history` 提交 show ids 会展开该季全部已播出集（实测生效）；集级控制需带 `seasons` 结构
- **错误约定**：公开 JSON 的 error 字段不得含镜像域名/URL（`mirrorUnavailableMsg` / `publicError` 既有约定同样适用）
- 响应解析：`/sync/history` 返回 `added.{movies,episodes,...}` + `not_found.{movies,shows,seasons,episodes}`（[lc-2453] 已修前端恒 0 的字段错位）

## 6. 规划优先级建议

1. **评分同步**（飞牛评分 ↔ Trakt 双向，`sync/ratings` 读写 + `last_activities` 增量）—— 数据源与界面现成
2. **Trakt 已看 → 飞牛已观看回填**（迁移刚需，`sync/watched` 即够）
3. **Trakt 社区评分显示**（详情页/hero，`ratings` + OMdb 同款缓存模式）
4. **续播跨设备**（`sync/playback` ↔ AniBox 播放进度）
5. **下一集/最新集状态**（`shows/{id}/next_episode` —— hero 标签行 TMDB 滞后问题的互补源）
6. 待看清单 / collection 对照（低频，可选）
