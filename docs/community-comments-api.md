# 公共评论协议（v1）

> 实现：`inbox.690075.xyz`（Cloudflare Worker + D1）。**服务端源码不入库**
> （与 `bgm-mirror` / `iptv-mirror` 同一规则），本文档即完整协议，可据此自建。
> 客户端模块：`src-tauri/src/community.rs`

## 定位

按 TMDB 作品关联的跨服务器讨论。同一部片，不管你连的是哪个飞牛 NAS，看到的是同一串评论。

**作品级，不分集**（第一版）。用 `(mediaType, tmdbId)` 作作品键 —— TMDB ID 是我们
唯一跨服务器稳定的标识（飞牛的 item guid 只在本 NAS 内有效）。

## 明确不做权限校验（重要，别误解）

Tigerest 那套靠 Emby token 由服务端反查「这个人能不能看这部片」。**我们做不到**：

- 飞牛没有对外的权限查询接口
- 客户端自报「我能看」是可伪造的，做了也是假防护

所以本协议**不假装有防护**：评论区公开，任何持有设备码的客户端都能评论任何 TMDB 作品。
真正的约束只有下面「安全边界」里那几条。这是取舍，不是遗漏。

## 身份：设备码 + 可改昵称

| 头 | 必需 | 说明 |
|---|---|---|
| `X-Anibox-Device` | 是（写操作） | `sha256(机器标识 + 盐)`，32–64 位十六进制。**服务端只存哈希，不存原始标识** |
| `X-Anibox-Nick` | 否 | 显示昵称，UTF-8 ≤24 字，**须百分号编码**（HTTP header 只能放 ASCII；服务端会 `decodeURIComponent`） |

不传昵称时服务端生成 `观众-xxxx`（`xxxx` = 设备码前 4 位），**确定性**：同一设备永远
得到同一个默认名，不会看起来像换了一个人。

为什么是设备码而不是账号：飞牛是私有部署，不同 NAS 的 user_id 互不相通；设备码换来
「换 NAS 不丢数据」。代价：同一人两台机器算两个身份。

## 接口

Base：`https://inbox.690075.xyz`

响应统一 `{ "data": ... }` / `{ "error": { code, message } }`。

### `GET /v1/comments?mediaType=movie|tv&tmdbId=N&cursor=0&limit=20`

- `cursor` 偏移量（默认 0）；`limit` 1–50（默认 20）
- 主评论**按新到旧**；每条主评论内嵌最多 **3 条回复**（按旧到新）
- 带 `X-Anibox-Device` 时，自己的评论 `mine: true`

```json
{
  "data": {
    "items": [
      {
        "id": "UUID", "parentId": null, "rootId": null,
        "nick": "小明", "body": "很好看", "deleted": false,
        "createdAt": 1732400000000, "mine": false,
        "replies": [
          { "id": "UUID", "parentId": "主评论UUID", "rootId": "主评论UUID",
            "nick": "小红", "body": "同感", "deleted": false,
            "createdAt": 1732400100000, "mine": false }
        ]
      }
    ],
    "nextCursor": 20,
    "total": 137
  }
}
```

`nextCursor: null` 表示没有更多。`total` 只数**主评论**。

### `POST /v1/comments`

```json
{
  "mediaType": "movie",
  "tmdbId": 12345,
  "title": "作品名（仅便于后台排查，不是键）",
  "body": "正文",
  "clientRequestId": "UUID",
  "parentId": "可选，回复某条评论时填"
}
```

- 成功 → `201 { "data": { "comment": {...}, "replayed": false } }`
- **同一 `clientRequestId` 重复提交 → `200` + `replayed: true` + 原来那条**
  （网络重试不该产生两条评论；内容不会被改写）
- 回复的回复：`parentId` 填某条回复的 id，服务端把 `rootId` 归到同一个主评论

### `DELETE /v1/comments/{id}`

- 只能删**自己的**（按 `device_id` 比对，**不是**按昵称）
- 别人的评论返回 `404`（与「不存在」同一个响应 —— 不泄露存在性）
- **软删除**：`body` 置 `NULL`、`deleted=1`，行保留（回复链不断、列表显示「该评论已删除」）
- 正文必须真的从库里清掉，不能只靠前端不显示（已有测试专门守这条）

### `GET /v1/comments/mine?cursor=0&limit=20`

自己的全部发言（含回复），按新到旧，每条带 `work: { mediaType, tmdbId, title }` 可跳转。

## 安全边界（真实生效的部分）

| 约束 | 值 |
|---|---|
| 写操作必须带设备码 | 缺失 → `401` |
| 正文长度 | 3–2000 字（按字符，不按字节） |
| 单设备限流 | 60 秒内最多 5 条，两条间隔 ≥3 秒 → 超了 `429 RATE_LIMITED` |
| 删除归属 | 按 `device_id`，不按昵称 |
| SQL | 全部参数化查询（有测试验证注入无效） |
| HTML | **原样存储、不转义** —— 渲染时必须由客户端转义（不存 HTML 是刻意的：存了就得考虑清洗，不存最安全） |

限流按设备而不是 IP：同一 NAT 后多台设备互不影响；换 IP 也绕不过（伪造设备码可行，
但代价是丢掉自己的全部历史）。

## 错误码

| HTTP | code | 含义 |
|---|---|---|
| 400 | `INVALID_REQUEST` / `INVALID_BODY` / `INVALID_JSON` | 参数不合法 |
| 401 | `UNAUTHORIZED` | 缺设备码 |
| 404 | `NOT_FOUND` | 评论不存在，或不是你的 |
| 409 | `PARENT_DELETED` | 要回复的评论已删除 |
| 429 | `RATE_LIMITED` | 触发限流 |

## 存储

D1 三张表（`schema.sql`）：

- `works(id, media_type, tmdb_id, title, created_at)` — `UNIQUE(media_type, tmdb_id)`
- `comments(id, work_id, parent_id, root_id, device_id, nick, body, deleted, created_at, updated_at)`
- 三个索引：主评论分页、回复分页、我的发言（都带 `parent_id IS NULL` 部分索引）

为什么用 D1 而不是 KV：评论要 `ORDER BY` / `LIMIT OFFSET` / `COUNT`，KV 做不了
（只能全量读出来在 Worker 里排序，数据一多就崩）。

## 部署

```bash
cd inbox-mirror
npx wrangler d1 create anibox-comments    # 把返回的 database_id 填进 wrangler.toml
npx wrangler d1 execute anibox-comments --file=./schema.sql --remote
npx wrangler deploy
```

## 验证

```bash
# Worker 逻辑：node:sqlite 内存库跑真实 worker.js（54 项）
node tests/verify_inbox_comments_2331.cjs
```

覆盖：空列表、创建、幂等（同 clientRequestId 返回原条）、5 类参数校验、限流、
回复嵌套（rootId 归一）、删除（归属/软删除/正文清空/不能回复已删除）、
三页分页（新到旧、nextCursor）、我的发言、XSS+SQL 注入。
