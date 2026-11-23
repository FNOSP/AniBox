# 弹幕共享映射协议（v1）

> 实现：`src-tauri/src/danmaku/shared_map.rs`（客户端）
> 服务端：自托管 Cloudflare Worker（`inbox.690075.xyz`）。**服务端源码不入库**
> （与 `bgm-mirror` / `iptv-mirror` 同一规则），本文档即完整协议，可据此自建。

## 解决什么

一季动画的**弹幕库集号经常和本地文件集数错位**（季合并、特别篇、先行放映偏移）。
现在用户每集都要手动选一遍正确的弹弹play 剧集，而且只在本机记住
（`episode_offset` 是纯本地 JSON）。

共享映射把它变成协作资产：**一个人校准一次，同组设备以后自动命中**。

## 归属模型

- **一机一账号**（产品决策）：身份是设备码 `sha256(机器标识)`，不做账号体系。
  飞牛是私有部署、不同 NAS 的 `user_id` 互不相通，用设备码换来的是「换 NAS 不丢数据」。
  代价：同一个人两台机器算两个身份。
- 映射按 **作品 + 季** 存，不是按用户存。同组设备（甚至跨设备）只要作品标题+季
  对得上，后来的设备直接复用别人的校准结果。
- **读取不需要令牌，写入需要**。读放开才能让共享真正生效。

## 接口

Base：`https://inbox.690075.xyz`（可在客户端留空以完全关闭该功能）

所有响应都是 `{ "data": ... }` 或 `{ "error": { code, message } }`。

### `GET /health`

```json
{ "data": { "status": "ok", "service": "anibox-inbox", "version": 1 } }
```

### `POST /v1/map/resolve` — 读取

```json
{ "title": "葬送的芙莉莲", "season": 1, "episode": 4 }
```

- 命中 → `200 { "data": { "title", "season", "episode", "selection": { "bangumiId", "episodeId", "episodeTitle"? } } }`
- 未命中 → `404 { "error": { "code": "NOT_FOUND" } }`
- 省略 `episode` → 返回整季 `{ "title", "season", "episodes": { "3": {...}, "5": {...} } }`

### `POST /v1/map/save` — 写入

请求头：

| 头 | 必需 | 含义 |
|---|---|---|
| `X-Anibox-Token` | 是 | 写入准入令牌（部署时 `wrangler secret put ANIBOX_INBOX_TOKEN`） |
| `X-Anibox-Device` | 否 | 设备码，仅记录「谁校准的」 |

```json
{
  "title": "葬送的芙莉莲",
  "season": 1,
  "episode": 3,
  "selection": { "bangumiId": "17617", "episodeId": "176170003", "episodeTitle": "第3话" }
}
```

- 成功 → `200 { "data": { "saved": true, "episode": 3, "selection": {...} } }`
- 无/错令牌 → `401 UNAUTHORIZED`

### `GET /v1/map/list?title=...&season=...`

返回该作品该季全部映射（调试/整季预取用）。

## 字段约束

| 字段 | 约束 |
|---|---|
| `title` | 1–200 字符，不含控制字符。归一化见下 |
| `season` | 整数 1–1000 |
| `episode` | 整数 1–10000 |
| `selection.bangumiId` | ≤64 字符，支持 `tmdb-12345` 形式 |
| `selection.episodeId` | **必须十进制数字串**（弹弹play 约定），1–20 位 |
| `selection.episodeTitle` | 可选，≤200 字符 |

## 标题归一化（跨语言契约，最容易出 bug 的地方）

服务端（JS）与客户端（Rust）的归一化**必须逐条一致** —— 不一致就会「写得进、查不到」，
而且静默失败。两边实现的顺序是契约的一部分：

1. 全角 ASCII（U+FF01..U+FF5E）→ 半角；全角空格（U+3000）→ 普通空格
2. 分隔标点 → 空格：`· ・ : ： - — – _ / \ | ; ；`
3. 连续空白折叠成一个，去首尾
4. 小写

**不做**语义归一化（去括号、去季后缀等）—— 那会把不同作品混成一条。

服务端键：`map:<sha256(norm_title + "\0" + season) 的前 16 位十六进制>`
（标题含 `/` `?` 等字符，不能直接进键）。

验证：`node tests/verify_norm_title_parity_2330.cjs`
（从 worker.js 抽出实现、跑 30 条刁钻用例、注入 Rust 测试比对）

## 客户端行为约定

| 约定 | 原因 |
|---|---|
| **读取失败一律当作「没有共享映射」** | 共享映射是加速层，不能成为起播延迟来源（lc-2296「连播等 8s」血训） |
| 解析超时 2.5s | 同上 |
| `episodeId` 非十进制 → 当没命中 | 绝不把脏值传给下游请求 |
| **只在用户明确手动选择时写入** | 自动命中/哈希匹配/历史复用都不能冒充手动选择，否则污染所有人的映射 |
| 无令牌 → `Ok(false)` 不发请求 | 协议允许无令牌客户端只读 |
| 写入失败只记日志 | 这次校准没共享出去，但用户的操作不能崩 |

## 部署

```bash
cd inbox-mirror
npx wrangler kv namespace create MAP_KV     # 把返回的 id 填进 wrangler.toml
npx wrangler secret put ANIBOX_INBOX_TOKEN  # 写入准入令牌
npx wrangler deploy
```

存储用 KV：映射是「一机一条」的稀疏小数据，读（含边缘缓存）远比 D1 便宜，
写延迟无所谓（用户点完选集等 100ms 完全可以接受）。TTL 一年。

将来公共评论/一起看要事务和查询，再加 D1，不冲突。

## 验证

```bash
# Worker 逻辑（内存 KV 替身，36 项）
node tests/verify_inbox_mapping_2330.cjs

# 跨语言归一化一致性（30 条用例 + Rust 比对）
node tests/verify_norm_title_parity_2330.cjs

# Rust 客户端端到端（独立进程 mock，16 项）
node tests/verify_shared_map_client_2330.cjs
```
