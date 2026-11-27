# 社区 v2 施工文档

> 客户端：`src-tauri/src/community*.rs` + `src/components/community/`
> 服务端：`inbox.690075.xyz`（Cloudflare Worker + D1 + R2）。**服务端源码不入库**
> （与 `bgm-mirror` / `tmdb-mirror` / `iptv-mirror` 同一规则），本文档即完整实现依据，
> 可据此从零重建整个服务端。
>
> 前置协议：`docs/community-comments-api.md`（v1 作品级评论，**保持不动**，老客户端继续可用）

---

## 0. 目标与边界

### 0.1 做什么

| 模块 | 内容 |
|---|---|
| 账号 | 邮箱+密码注册（邮箱验证）、登录、设备会话、设备管理、注销 |
| 图片 | 多格式上传、客户端压缩、1MB 硬上限、30 天云端保留、本地缓存 |
| 反馈 | 现有工单体系 + 附图 |
| 社区 | 发帖（影评分享）、信息流、作品讨论、通知、举报 |
| 管理 | 管理员登录后界面不同：撤回 / 删除 / 封设备 / 封账号 / 举报队列 / 审计日志 |
| 审计 | 全量正文快照，90 天，Cron 清理 |

### 0.2 明确的边界（不是遗漏，是取舍）

| 不做 | 原因 |
|---|---|
| 密码找回 | 用户明确不要。忘密码 = 用同一邮箱重新注册（旧账号封存） |
| 手机号 / 实名 | 不做备案，仅 GitHub 发布 |
| AVIF 上传 | 需要引入 C 依赖 `ravif`，与单 exe 纯 Rust 构建冲突 |
| 有损 WebP 编码 | `image-webp` 只支持 VP8L 无损编码（源码 `encoder.rs:631`），对照片无意义 |
| 集级讨论（`scope=episode`） | 第一版只到作品级；表结构已预留，不实现 |
| WebSocket 实时 | 桌面端几千在线无必要，60s 轮询足够 |

### 0.3 一句话数据保留策略

> **文字永久，图片 30 天，审计 90 天。图片过期后帖子还在，只是配图裂。**

---

## 1. 容量核算（已核实，不是估算）

### 1.1 云资源额度（Cloudflare Workers Paid，$5/月）

| 资源 | 额度 | 本方案用量 |
|---|---|---|
| Workers 请求 | 1000 万/月 | 图片+读写，1 万/天 ≈ 30 万/月 |
| Workers CPU | 3000 万 ms/月 | 单请求 1–3ms（PBKDF2 登录 40–60ms） |
| D1 读行 | 250 亿/月 | 1 万页/天 ≈ 2700 万/月 |
| D1 写行 | 5000 万/月 | 5000 条/天 ≈ 15 万/月 |
| D1 存储 | 5GB 含，**单库硬上限 10GB** | 文字 1000 万条 ≈ 5.5GB |
| R2 存储 | 10GB 免费 | 自设 2GB 上限 |
| R2 出口 | **免费** | 图片全靠它 |
| Email Sending | Paid 可发任意收件人 | 注册验证码 |

### 1.2 文字能存多少（含核算过程）

单条发言 ≈ **570 字节**：

```
id 36 + account_id 29 + body 300（100 个中文×3） + title 60
+ media_type/tmdb_id 20 + flags 8 + 时间戳 16 + SQLite 行开销 30
+ 主索引 40 + 作者索引 30  =  约 570 字节
```

| 发言总条数 | D1 占用 | 费用 |
|---|---|---|
| 10 万 | 55MB | 0 |
| 100 万 | 550MB | 0 |
| 500 万 | 2.7GB | 0 |
| 1000 万 | 5.5GB | 约 $0.4/月（超出 5GB 部分 $0.75/GB·月） |
| 1800 万 | 10GB | **D1 单库硬上限，不能再存** |

**结论：文字永久保存的成本可以忽略**，真正的墙是 10GB 单库上限（约 1800 万条）。正常规模永远碰不到。

### 1.3 审计日志能存多少

单条（B 方案含正文快照）≈ **445 字节**。

| 日活 | 每天动作 | 90 天累计 | 占用 |
|---|---|---|---|
| 100 | 1000 | 9 万 | 40MB |
| 500 | 5000 | 45 万 | 200MB |
| 2000 | 2 万 | 180 万 | 800MB |

90 天全量正文快照在 2000 日活下不到 1GB，可与文字表共用 5GB 免费额度。

### 1.4 图片能存多少

| 项 | 值 |
|---|---|
| 单图压缩后 | 硬上限 1MB，典型 250–400KB（1600px q88） |
| 缩略图 | 400px q80，典型 30–45KB |
| 单张合计 | 约 335KB |
| 2GB 能存 | 约 **6100 张** |
| 30 天 × 每天上传 | 约 200 张/天 |
| 换算 | 约 2000 活跃用户，每人 3 天传 1 张 |

超过这个量就会在 30 天到期前触发 2GB 兜底淘汰。要扩容时改两个数：`IMAGE_BUDGET_BYTES` 和 R2 生命周期天数。

---

## 2. 服务端：数据模型

### 2.1 完整 schema（`inbox-mirror/schema-v2.sql`）

```sql
PRAGMA foreign_keys = ON;

-- ═══════════════════ 账号 ═══════════════════
-- role 不落库：管理员身份由 wrangler secret ANIBOX_ADMIN_EMAILS 每次请求现算。
-- 这样改 secret 立即生效，数据库被篡改也无法提权。
CREATE TABLE IF NOT EXISTS accounts (
  id            TEXT PRIMARY KEY,                  -- 'u_' + 26 位 base32
  email         TEXT NOT NULL,                     -- 小写归一
  pwd_hash      TEXT NOT NULL,                     -- pbkdf2_sha256$100000$<salt_b64>$<hash_b64>
  nick          TEXT NOT NULL,                     -- 1–24 字
  nick_suffix   TEXT NOT NULL,                     -- 4 位十六进制，重名时显示「小明 @a3f2」
  avatar_seed   TEXT NOT NULL,                     -- 确定性头像色的种子，不存图
  status        TEXT NOT NULL DEFAULT 'active',    -- active | muted | banned | deleted
  mute_until    INTEGER NOT NULL DEFAULT 0,        -- 毫秒；> now 时只拦写
  created_at    INTEGER NOT NULL,                  -- 毫秒
  last_login_at INTEGER NOT NULL,
  CONSTRAINT uq_email UNIQUE (email)
);
CREATE INDEX IF NOT EXISTS idx_acc_status ON accounts(status, created_at DESC);

-- 注册进行中：验证码通过前不建账号，避免「注册一半放弃」留垃圾
CREATE TABLE IF NOT EXISTS pending_regs (
  id         TEXT PRIMARY KEY,
  email      TEXT NOT NULL,
  pwd_hash   TEXT NOT NULL,                        -- 提前算好，验证通过直接搬进 accounts
  nick       TEXT NOT NULL,
  code_hash  TEXT NOT NULL,                        -- sha256(code)，不存明文
  tries      INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER NOT NULL,                     -- 创建 + 10 分钟
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pending_email ON pending_regs(email, created_at DESC);

-- ═══════════════════ 设备会话 ═══════════════════
-- 台数不限制（用户要求）。device_code 是物理机标识，用于跨账号封禁。
CREATE TABLE IF NOT EXISTS devices (
  id           TEXT PRIMARY KEY,                   -- 'd_' + uuid
  account_id   TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  tok_hash     TEXT NOT NULL,                      -- sha256(token)
  device_code  TEXT NOT NULL,                      -- 32 位 hex，sha256(机器标识+盐)
  os           TEXT NOT NULL,                      -- Windows / macOS / Linux
  screen       TEXT NOT NULL,                      -- '3840x2160'（物理像素）
  scale        INTEGER NOT NULL,                   -- 150
  ip_net       TEXT NOT NULL,                      -- '183.36.*.*'（网段，不存全 IP）
  created_at   INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  revoked_at   INTEGER,                            -- 被登出/踢下线
  CONSTRAINT uq_tok UNIQUE (tok_hash)
);
CREATE INDEX IF NOT EXISTS idx_dev_tok  ON devices(tok_hash);
CREATE INDEX IF NOT EXISTS idx_dev_code ON devices(device_code);
CREATE INDEX IF NOT EXISTS idx_dev_acct ON devices(account_id, last_seen_at DESC);

-- 设备封禁：按 device_code 而不是 devices.id ——
-- 否则用户登出再登入就换了 devices.id，封禁失效。
CREATE TABLE IF NOT EXISTS device_bans (
  device_code TEXT PRIMARY KEY,
  reason      TEXT NOT NULL,
  created_by  TEXT NOT NULL,                       -- 管理员账号 id
  created_at  INTEGER NOT NULL
);

-- 邮箱验证码发送冷却（防轰炸别人邮箱）
CREATE TABLE IF NOT EXISTS email_sends (
  email      TEXT NOT NULL,
  ip_net     TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_es_email ON email_sends(email, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_es_ip    ON email_sends(ip_net, created_at DESC);

-- 登录失败计数（§3.4）。key = 'acc:<id>' 或 'ip:<ip_net>'
CREATE TABLE IF NOT EXISTS login_fails (
  key          TEXT PRIMARY KEY,
  count        INTEGER NOT NULL,
  first_at     INTEGER NOT NULL,
  locked_until INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_lf_first ON login_fails(first_at);

-- ═══════════════════ 图片 ═══════════════════
CREATE TABLE IF NOT EXISTS images (
  id           TEXT PRIMARY KEY,                   -- 'img_' + 26 位 base32
  owner_id     TEXT NOT NULL,
  scope        TEXT NOT NULL,                      -- post | feedback
  r2_bucket    TEXT NOT NULL,                      -- anibox-public | anibox-private
  r2_full      TEXT NOT NULL,                      -- 'c/<id>.jpg' 或 'f/<id>.jpg'
  r2_thumb     TEXT,                               -- 't/<id>.jpg'（GIF 无缩略图）
  w            INTEGER NOT NULL,
  h            INTEGER NOT NULL,
  bytes        INTEGER NOT NULL,                   -- full 字节数
  thumb_bytes  INTEGER NOT NULL DEFAULT 0,
  fmt          TEXT NOT NULL,                      -- 原始上传格式 jpeg|png|webp|bmp|tiff|heic|gif
  status       TEXT NOT NULL DEFAULT 'unused',     -- unused | attached | hidden | deleted
  ref_kind     TEXT,                               -- post | comment | feedback
  ref_id       TEXT,
  created_at   INTEGER NOT NULL,
  deleted_at   INTEGER,
  deleted_by   TEXT,
  -- 涉及违法内容时不能真删（要留证据）：置 takedown_at 后
  -- ① 前端一律不显示 ② 2GB 淘汰与生命周期 Rule 都跳过 ③ 只有管理员能看
  takedown_at     INTEGER,
  takedown_reason TEXT
);
CREATE INDEX IF NOT EXISTS idx_img_owner ON images(owner_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_img_ref   ON images(ref_kind, ref_id);
-- 2GB 兜底淘汰要按「最老优先」扫未删的图
CREATE INDEX IF NOT EXISTS idx_img_live  ON images(created_at ASC) WHERE deleted_at IS NULL;

-- ═══════════════════ 社区内容 ═══════════════════
-- 文字永久保存。tmdb_id 是唯一跨服务器稳定的作品键（同 v1）。
CREATE TABLE IF NOT EXISTS posts (
  id           TEXT PRIMARY KEY,                   -- 'p_' + 26 位 base32（= clientRequestId，幂等）
  account_id   TEXT NOT NULL,
  author_nick  TEXT NOT NULL DEFAULT '',           -- [lc-2445 补] 昵称快照：「改名只影响之后的发言」对帖子的兑现（光靠 accounts.nick 当前值会违反承诺）
  media_type   TEXT NOT NULL,                      -- movie | tv
  tmdb_id      INTEGER NOT NULL,
  work_title   TEXT NOT NULL DEFAULT '',           -- 冗余，便于后台排查（不是键）
  score        INTEGER,                            -- 0–5，NULL = 没打分
  title        TEXT NOT NULL DEFAULT '',           -- 长评标题（可空）
  body         TEXT NOT NULL,
  image_ids    TEXT NOT NULL DEFAULT '[]',         -- JSON 数组
  pinned       INTEGER NOT NULL DEFAULT 0,
  hidden       INTEGER NOT NULL DEFAULT 0,         -- 被举报自动隐藏 / 管理员隐藏
  report_count INTEGER NOT NULL DEFAULT 0,         -- 不同举报者人数
  retracted_at INTEGER,                            -- 管理员撤回（可恢复）
  retracted_by TEXT,
  retract_reason TEXT,
  deleted_at   INTEGER,                            -- 软删（不可恢复）
  deleted_by   TEXT,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_posts_work ON posts(tmdb_id, created_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_posts_feed ON posts(created_at DESC) WHERE deleted_at IS NULL AND hidden = 0;
CREATE INDEX IF NOT EXISTS idx_posts_acct ON posts(account_id, created_at DESC);

-- 短评沿用 v1 comments 表，只加三列（v1 行是 account_id IS NULL 的老数据）
ALTER TABLE comments ADD COLUMN account_id TEXT;
ALTER TABLE comments ADD COLUMN hidden     INTEGER NOT NULL DEFAULT 0;
ALTER TABLE comments ADD COLUMN report_count INTEGER NOT NULL DEFAULT 0;

-- ═══════════════════ 举报 ═══════════════════
CREATE TABLE IF NOT EXISTS reports (
  id          TEXT PRIMARY KEY,
  kind        TEXT NOT NULL,                       -- post | comment | image | account
  ref_id      TEXT NOT NULL,
  reason      TEXT NOT NULL,                       -- spam | abuse | porn | illegal | other
  detail      TEXT,                                -- ≤500 字
  reporter_id TEXT NOT NULL,
  status      TEXT NOT NULL DEFAULT 'open',         -- open | resolved | rejected
  handled_by  TEXT,
  handled_at  INTEGER,
  handle_note TEXT,
  created_at  INTEGER NOT NULL,
  -- 同一人对同一条只能报一次：否则一个人点 100 次就能触发自动隐藏
  CONSTRAINT uq_report UNIQUE (kind, ref_id, reporter_id)
);
CREATE INDEX IF NOT EXISTS idx_rep_status ON reports(status, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_rep_ref    ON reports(kind, ref_id);

-- ═══════════════════ 通知 ═══════════════════
CREATE TABLE IF NOT EXISTS notifications (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id TEXT NOT NULL,                        -- 收件人
  kind       TEXT NOT NULL,                        -- reply | mention | post_like | mod
  ref_kind   TEXT NOT NULL,
  ref_id     TEXT NOT NULL,
  media_type TEXT NOT NULL,
  tmdb_id    INTEGER NOT NULL,
  actor_nick TEXT NOT NULL,
  excerpt    TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  read_at    INTEGER
);
CREATE INDEX IF NOT EXISTS idx_notif ON notifications(account_id, created_at DESC);

-- ═══════════════════ 反馈（[lc-2442 补] §4.6 端点对应的表，P2 施工时补入）═══════════════════
-- 工单随社区账号；附图走 R2 私有桶 f/（90 天生命周期）；会话返回形状与旧 v1 同构。
CREATE TABLE IF NOT EXISTS feedback_tickets (
  id         TEXT PRIMARY KEY,                    -- 'fb_' + uuid
  account_id TEXT NOT NULL,
  message    TEXT NOT NULL,                       -- 1–4000 字
  contact    TEXT,
  log        TEXT,                                -- 脱敏日志尾部（≤2MB 文本）
  page       TEXT,
  aid        TEXT,
  v          TEXT NOT NULL DEFAULT '',
  os         TEXT NOT NULL DEFAULT '',
  arch       TEXT NOT NULL DEFAULT '',
  status     TEXT NOT NULL DEFAULT 'open',        -- open | confirmed | progress | resolved | closed
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_fb_acct ON feedback_tickets(account_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS feedback_messages (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  ticket_id  TEXT NOT NULL REFERENCES feedback_tickets(id) ON DELETE CASCADE,
  role       TEXT NOT NULL,                       -- user | admin
  text       TEXT NOT NULL,
  image_ids  TEXT NOT NULL DEFAULT '[]',          -- JSON 数组
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_fbm_ticket ON feedback_messages(ticket_id, created_at ASC);

-- ═══════════════════ 审计（B 方案：全量正文快照，90 天）═══════════════════
CREATE TABLE IF NOT EXISTS audit_log (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  ts          INTEGER NOT NULL,
  actor_id    TEXT,                                -- 账号 id
  actor_nick  TEXT,                                -- 昵称快照（改名后仍可追溯）
  actor_dev   TEXT,                                -- 设备码 ← 用户要求的「设备」
  ip_net      TEXT NOT NULL,                       -- '183.36.*.*' ← 用户要求的「ip」
  action      TEXT NOT NULL,
  target_kind TEXT,
  target_id   TEXT,
  body_snap   TEXT,                                -- 正文快照 ← B 方案核心
  image_refs  TEXT,                                -- JSON 数组
  detail      TEXT                                 -- 撤回原因等纯文本
);
CREATE INDEX IF NOT EXISTS idx_audit_ts    ON audit_log(ts DESC);
CREATE INDEX IF NOT EXISTS idx_audit_actor ON audit_log(actor_id, ts DESC);
CREATE INDEX IF NOT EXISTS idx_audit_dev   ON audit_log(actor_dev, ts DESC);
```

### 2.2 迁移注意

1. `ALTER TABLE comments ADD COLUMN` 在 D1 上支持；三列都有默认值，v1 老行自动填 `NULL/0`。
2. **不要改 v1 表的既有列名或语义**，v1 客户端仍在用。
3. `mine` 判定升级：`account_id = 当前账号` OR（`account_id IS NULL` AND `device_id = 当前设备码`）。老评论仍显示自己的「我」标。

---

## 3. 服务端：认证与授权

### 3.1 密码哈希

CF Workers 的 Web Crypto **只支持 PBKDF2**（Argon2/scrypt/bcrypt 均不在支持表）。因此：

```
算法：PBKDF2-HMAC-SHA256
迭代：100,000
盐：16 字节随机
输出：32 字节
格式：pbkdf2_sha256$100000$<salt_b64url>$<hash_b64url>
校验：等长常量时间比较（先比长度再逐字节 XOR 累加）
```

单次约 40–60ms，Workers Paid 默认 30s CPU 上限完全够。

**已声明的风险**：用户要求「8 位字母+数字即可」，该组合离线可破解。补偿手段只有两条，都落在**在线**面：

| 措施 | 值 |
|---|---|
| 登录失败冷却 | 同账号连续 5 次失败 → 锁 15 分钟（存 `accounts.mute_until` 之外单独一行，见 3.4） |
| 同网段限流 | 同 `ip_net` 60 分钟内最多 20 次登录尝试 |
| 弱密码黑名单 | 200 条（`12345678` / `abcd1234` / `qwerty12` / `a1234567` …），注册时拒绝 |
| 无密码找回 | 用户明确要求不做 → 减少「邮箱被盗 → 账号被接管」的攻击面 |

### 3.2 邮箱验证流程

```
① POST /v2/auth/register { email, password, nick }
   ├ 校验：邮箱格式 / 密码 8–64 且含字母+数字 / 弱密码黑名单 / 昵称 1–24 字
   ├ 拒绝：accounts.email 已存在 → 409 EMAIL_TAKEN
   ├ 冷却：同邮箱 60 秒 1 次、1 小时 5 次；同 ip_net 1 小时 10 次 → 429
   ├ 写入 pending_regs（pwd_hash 提前算好，code_hash 存 sha256(code)）
   ├ 发信：6 位数字码，10 分钟有效
   └ 返回 { pendingId, cooldownSec: 60, maskedEmail: "ab***@qq.com" }

② POST /v2/auth/verify { pendingId, code }
   ├ 校验：未过期 / tries < 5
   ├ 错误 → 422 BAD_CODE（tries+1；到 5 次删行，要求重新注册）
   ├ 正确 → 建 accounts 行 → 建 devices 行（签发 token）→ 删 pending_regs
   └ 返回 { accountId, token, nick, nickSuffix, role }
```

**「验证前账号不存在」** 是刻意设计：不给「注册一半」的邮箱占位，也不留垃圾账号。

### 3.3 登录

```
POST /v2/auth/login { email, password, device: { code, os, screen, scale } }
  ├ device_bans 命中 → 403 DEVICE_BANNED（不透露密码对错）
  ├ 限流命中        → 429 TOO_MANY_ATTEMPTS
  ├ 密码错误        → 401 BAD_CREDENTIALS（计入失败次数）
  ├ accounts.status = banned  → 403 BANNED
  ├ accounts.status = deleted → 401
  └ 成功 → 新建 devices 行（每次登录一台新设备记录；同 device_code 已有未撤销行
            则复用并刷新 token，避免「同一台机器反复登录堆出 20 条记录」）
```

**设备软上限 8 台**：超了返回 409 `TOO_MANY_DEVICES`，提示「请先在旧设备登出」。
用户要求「不限制」，这是防滥用护栏，正常用户碰不到——**如需完全放开，把常量改成 0 即可**。

### 3.4 失败计数

表结构见 §2.1 的 `login_fails`（`key = 'acc:<id>'` 或 `'ip:<ip_net>'`）。

- 成功登录 → 删 `acc:` 行
- 60 分钟内失败 5 次 → `locked_until = now + 15min`
- `ip:` 行按小时窗口统计，超 20 次拒绝

### 3.5 统一鉴权入口（所有写接口的第一行）

**绑定名约定（全文统一，`wrangler.toml` 里就叫这些）**：

| 绑定 | 类型 | 变量名 |
|---|---|---|
| D1 | `[[d1_databases]]` | `DB` |
| R2 公开桶 | `[[r2_buckets]]` | `R2_PUBLIC` |
| R2 私有桶 | `[[r2_buckets]]` | `R2_PRIVATE` |
| 邮件 | `[[send_email]]` | `EMAIL` |
| 执行上下文 | — | `ctx`（未用：touchDevice 已内置节流，直接 await） |

```js
async function requireAuth(request, env, opts = {}) {  // opts.readOk：banned 放行（sess.banned=true，写接口再 403）——「读接口不拦封禁」的实现
  const tok = bearerToken(request);
  if (!tok) throw httpErr(401, 'UNAUTHORIZED', '请先登录');
  const dev = await env.DB.prepare(
    'SELECT * FROM devices WHERE tok_hash = ?'
  ).bind(await sha256Hex(tok)).first();
  if (!dev)              throw httpErr(401, 'UNAUTHORIZED', '登录已失效，请重新登录');
  if (dev.revoked_at)    throw httpErr(401, 'DEVICE_REVOKED', '该设备已被登出');
  if (await isDeviceBanned(env, dev.device_code))
                         throw httpErr(403, 'DEVICE_BANNED', '该设备已被封禁');

  const acc = await env.DB.prepare('SELECT * FROM accounts WHERE id = ?')
    .bind(dev.account_id).first();
  if (!acc || acc.status === 'deleted') throw httpErr(401, 'UNAUTHORIZED', '账号不存在');
  if (acc.status === 'banned')          throw httpErr(403, 'BANNED', '账号已被封禁');

  // 管理员身份每次现算，不落库（改 secret 立即生效，库被篡改也提不了权）
  const admins = (env.ANIBOX_ADMIN_EMAILS || '').toLowerCase().split(',')
    .map((s) => s.trim()).filter(Boolean);
  const role = admins.includes(acc.email.toLowerCase()) ? 'admin' : 'user';

  // 最后活跃：直接 await（touchDevice 内部 5 分钟节流，命中节流时是空 UPDATE）。
  // 实测不需要 ctx.waitUntil —— 节流后的 UPDATE 成本可忽略，少一层回调更好测。
  await touchDevice(env, dev.id);

  return { acc, dev, role, muted: acc.mute_until > Date.now() || acc.status === 'muted' };
}

/** 写操作额外过一道禁言闸 */
function requireNotMuted(sess) {
  if (sess.muted) throw httpErr(403, 'MUTED', '你已被禁言，暂时无法发言');
}

/** 管理接口额外过一道角色闸 —— 服务端判定，前端渲染只是表现 */
function requireAdmin(sess) {
  if (sess.role !== 'admin') throw httpErr(403, 'FORBIDDEN', '需要管理员权限');
}
```

**读接口不拦封禁**：被封的人还能看帖，只是不能发。整页 403 是最糟的体验。

**`sha256Hex` 是异步的**（Web Crypto `crypto.subtle.digest` 返回 Promise），所以上面写的是 `await sha256Hex(tok)`。**不能**像 Node 那样同步调用，这是 Workers 与 Node 环境最容易踩的一个差异。

### 3.5.1 `wrangler.toml` 完整绑定段

```toml
name = "anibox-inbox"
main = "worker.js"
compatibility_date = "2026-11-23"
workers_dev = true

routes = [{ pattern = "inbox.690075.xyz", custom_domain = true }]

[[d1_databases]]
binding = "DB"                     # v1 的 COMMENT_DB 已统一改名 DB（同库双名会写错库）
database_name = "anibox-comments"  # 沿用 v1 现有库（保住 v1 评论数据）；已有库跑一次 migrate-v1-to-v2.sql
database_id = "REPLACE_WITH_REAL_D1_ID"

[[r2_buckets]]
binding = "R2_PUBLIC"
bucket_name = "anibox-public"

[[r2_buckets]]
binding = "R2_PRIVATE"
bucket_name = "anibox-private"

[[send_email]]
name = "EMAIL"

[[kv_namespaces]]
binding = "MAP_KV"                 # v1 弹幕共享映射继续用 KV
id = "REPLACE_WITH_REAL_KV_ID"

[triggers]
crons = ["0 4 * * *"]
```

**注意**：v1 的评论段用的是 `COMMENT_DB`。迁移时不要让同一个 Worker 里出现两个 D1 binding 指向同一个库（容易写错库）——**统一改成 `DB`，v1 代码内部把 `COMMENT_DB` 替换成 `DB`**，这是唯一需要改动 v1 代码的地方。


### 3.6 邮件发送

```toml
# inbox-mirror/wrangler.toml
[[send_email]]
name = "EMAIL"
```

```js
await env.EMAIL.send({
  from: { email: 'noreply@690075.xyz', name: 'AniBox' },
  to: email,
  subject: `${code} 是你的 AniBox 验证码`,
  text: `你的验证码是 ${code}，10 分钟内有效。\n\n如果这不是你本人操作，请忽略本邮件。\n若未在收件箱找到，请检查垃圾邮件。`,
  html: `<p>你的验证码是 <b style="font-size:20px;letter-spacing:2px">${code}</b>，10 分钟内有效。</p>
         <p style="color:#888">如果这不是你本人操作，请忽略本邮件。若未在收件箱找到，请检查垃圾邮件。</p>`,
});
```

**前置条件（部署必做，缺一条 QQ/163 就会扔垃圾箱）**：

1. `690075.xyz` 必须在同一 Cloudflare 账户下托管
2. CF 后台 → Email → Email Sending → 添加域名并完成验证
3. DNS 三条记录由 CF 自动写入：`SPF`（`v=spf1 include:_spf.mx.cloudflare.net ~all`）、`DKIM`、`DMARC`（建议 `p=none` 起步）
4. 免费套餐只能发「已验证的目标地址」；**发任意收件人需要 Paid**（已有）

**发送失败降级**：

```
EMAIL.send 抛错
  ├ E_DAILY_LIMIT_EXCEEDED / E_EMAIL_SENDING_NOT_READY → 走 Resend 备用（RESEND_KEY secret）
  ├ 两者都失败 → 返回 503 MAIL_UNAVAILABLE，前端提示「稍后重试或换邮箱」
  └ 每次失败都写 audit_log（action='mail_fail'），便于排查
```

**Beta 风险**：Email Sending 目前是 Beta，新账号日配额是逐步放开的。所以：
- 注册页必须有「60 秒后可重发」+「检查垃圾邮件」提示
- `email_sends` 表记录每次发送，配额撞墙时能看出是「发太多」还是「发失败」

---

## 4. 服务端：API 清单

Base：`https://inbox.690075.xyz`
统一响应：`{ "data": ... }` / `{ "error": { code, message } }`
鉴权头：`Authorization: Bearer <token>`

### 4.1 认证

| 方法 | 路径 | 入参 | 返回 |
|---|---|---|---|
| POST | `/v2/auth/register` | `{email,password,nick}` | `{pendingId,cooldownSec,maskedEmail}` |
| POST | `/v2/auth/resend` | `{pendingId}` | `{cooldownSec}` |
| POST | `/v2/auth/verify` | `{pendingId,code,device}`（device 与 login 同构 —— **首次会话在验证时建立**，token 此时签发） | `{accountId,token,nick,nickSuffix,role}` |
| POST | `/v2/auth/login` | `{email,password,device}` | `{accountId,token,nick,nickSuffix,role}` |
| POST | `/v2/auth/logout` | —（带 token） | `{ok:true}` |

`device` 结构：

```json
{ "code": "050c9dd3a1b2...32hex", "os": "Windows", "screen": "3840x2160", "scale": 150 }
```

### 4.2 账号

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/v2/account/me` | `{id,email(masked),nick,nickSuffix,role,createdAt,stats:{posts,comments}}` |
| PATCH | `/v2/account` | `{nick}` 改名（**只影响之后的发言**，历史保留快照） |
| PATCH | `/v2/account/password` | `{oldPassword,newPassword}`，改密码后撤销其他所有设备 |
| DELETE | `/v2/account` | 注销：`status='deleted'`、邮箱替换为 `deleted+<id>@invalid`、撤销全部设备、名下内容软删。审计日志保留（处置记录除外另行标注） |

### 4.3 设备

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/v2/devices` | `[{id,os,screen,scale,ipNet,deviceCode,lastSeenAt,createdAt,current}]` |
| DELETE | `/v2/devices/:id` | 撤销指定设备 |
| POST | `/v2/devices/revoke-others` | 撤销除当前外全部 |

`current` 由服务端比对当前 token 判定，不信任前端。

### 4.4 图片

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/v2/images` | `multipart/form-data`：`file`（full）、`thumb`（可选）、`scope` |
| GET | `/img/<key>` | **不经过 Worker**：R2 自定义域 `img.690075.xyz`，公开图 |
| GET | `/v2/images/:id/raw` | 私有图（反馈图）：Bearer token，仅 owner/admin |

上传响应：

```json
{ "data": { "id": "img_xxx", "url": "https://img.690075.xyz/c/img_xxx.jpg",
            "thumbUrl": "https://img.690075.xyz/t/img_xxx.jpg",
            "w": 1600, "h": 900, "bytes": 312005, "fmt": "jpeg" } }
```

**Worker 侧校验（客户端压缩不等于可信，全部重验）**：

```js
const MAX_FULL  = 1_048_576;   // 1MB
const MAX_THUMB = 122_880;     // 120KB
const ALLOWED_CT = new Set(['image/jpeg','image/gif']);
// full 只接受 jpeg 与 gif（gif 原样保留）；其余格式必须已被客户端转成 jpeg
// 二次确认魔数：FFD8FF = jpeg，GIF87a/GIF89a = gif；不符 → 400 BAD_IMAGE
```

### 4.5 社区

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/v2/posts` | `{mediaType,tmdbId,workTitle,score,title,body,imageIds,clientRequestId}`（UUID 作 id，同 id 重发返回原帖——与 v1 comments 同口径） |
| GET | `/v2/feed?sort=new\|hot&cursor=&limit=` | 信息流。**hot 的 P3 实现 = 置顶优先 + 新到旧**（点赞/评论数热度算法 P4 再加，不假装有）；文字不过滤时间（永久），30 天只是图片生命周期 |
| GET | `/v2/works/:mediaType/:tmdbId` | 作品聚合：`{score,count,ratingDist,posts,comments}` |
| GET | `/v2/posts/:id` | 单帖 + 全部评论 |
| DELETE | `/v2/posts/:id` | 删自己的 |
| POST | `/v2/comments` | 沿用 v1 形状 + 账号体系 |
| DELETE | `/v2/comments/:id` | 删自己的 |
| GET | `/v2/mine/posts?cursor=` | 我的发言（**永久可查，不受 30 天限制**） |
| GET | `/v2/notifications?cursor=` | 通知列表 |
| POST | `/v2/notifications/read` | `{ids:[]}` 或 `{all:true}` |
| POST | `/v2/reports` | `{kind,refId,reason,detail}` |

### 4.6 反馈（迁移到 v2）

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/v2/feedback` | `{message,contact,log,imageIds,page,aid,v,os,arch}` → `{id}` |
| GET | `/v2/feedback/mine` | 工单列表（含 `imageCount`） |
| GET | `/v2/feedback/:id` | 会话（`messages[]`，管理员可带图） |
| POST | `/v2/feedback/:id/reply` | `{text,imageIds}` |

**旧工单**：`/v1` 那批继续用 `feedback_thread` 指回 `stats.690075.xyz` 只读展示，自然消亡。

### 4.7 管理（全部要求 `role=admin`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/v2/admin/reports?status=open&cursor=` | 举报队列 |
| POST | `/v2/admin/posts/:id/retract` | `{reason}` 撤回（可恢复） |
| POST | `/v2/admin/posts/:id/unretract` | 恢复 |
| POST | `/v2/admin/posts/:id/delete` | 删除（不可恢复） |
| POST | `/v2/admin/comments/:id/retract\|delete` | 同上 |
| POST | `/v2/admin/images/:id/delete` | 删图（R2 + 行标记） |
| POST | `/v2/admin/accounts/:id/ban` | `{reason}` |
| POST | `/v2/admin/accounts/:id/unban` | — |
| POST | `/v2/admin/accounts/:id/mute` | `{minutes,reason}` |
| POST | `/v2/admin/devices/:id/ban` | `{reason}` 按 `device_code` 封物理机 |
| POST | `/v2/admin/devices/ban-code` | `{deviceCode,reason}` 直接按码封 |
| GET | `/v2/admin/audit?actorId=&deviceCode=&action=&from=&to=&cursor=` | 审计查询 |
| GET | `/v2/admin/stats` | `{users,posts,comments,images,storageUsed,storageCap}` |

**撤回 vs 删除（QQ 群语义）**：

| 动作 | 可见性 | 可恢复 | 审计 |
|---|---|---|---|
| 撤回 | 公开显示「该内容已被管理员撤回」，本人与管理员可见原文 | ✅ | `admin_retract` |
| 删除 | 前端完全不显示（软删，正文留在库） | ❌ | `admin_delete` |

---

## 5. 服务端：保留与清理

### 5.1 R2 桶与生命周期

| 桶 | 前缀 | 内容 | 生命周期 | 访问 |
|---|---|---|---|---|
| `anibox-public` | `c/` | 社区图 full | **30 天** | 自定义域 `img.690075.xyz`，`Cache-Control: public, max-age=31536000, immutable` |
| `anibox-public` | `t/` | 社区图缩略图 | **30 天** | 同上 |
| `anibox-private` | `f/` | 反馈图 | **90 天** | 无公开域，只能经 Worker 带 token 读 |
| `anibox-private` | `e/` | **证据留存**（takedown 内容） | **不设**（人工处置） | 无公开域，仅管理员 |

生命周期规则（R2 控制台或 API 设置，按前缀过滤，值以天为单位；**删除在到期后 24 小时内发生**）：

```json
[{ "ID": "community-30d", "Filter": { "Prefix": "c/" }, "Expiration": { "Days": 30 }, "Enabled": true },
 { "ID": "community-thumb-30d", "Filter": { "Prefix": "t/" }, "Expiration": { "Days": 30 }, "Enabled": true },
 { "ID": "feedback-90d", "Filter": { "Prefix": "f/" }, "Expiration": { "Days": 90 }, "Enabled": true }]
```

**为什么用 R2 生命周期而不是 Cron 扫表**：R2 内部操作零成本、CF 保证执行；Cron 扫表要烧 D1 读行 + Worker CPU，还要处理「cron 跑失败」的补偿。

**⚠ 生命周期规则不认识 D1 状态**——它只看对象键和年龄。所以「涉及违法内容要留证据」**不能靠跳过规则实现**，必须换桶：

```
管理员对某图执行 takedown
  ↓
① 从 anibox-public 复制到 anibox-private/e/<id>.jpg
② D1 置 takedown_at / takedown_reason，r2_full 改为 'e/<id>.jpg'，r2_bucket 改为 anibox-private
③ 从 anibox-public 删除原对象（公开域立刻 404，符合「必须下架」）
  ↓
30 天生命周期规则此时已管不到它（前缀不同 + 桶不同），证据随 D1 行长期保留
```

**代价如实说明**：证据图占用 `anibox-private` 的空间，**不计入 2GB 社区预算**（否则会被淘汰逻辑删掉）。它的量极小（只有真正违法的图才会进来），由管理员人工定期清理。


### 5.2 2GB 兜底淘汰（安全阀，正常不触发）

```js
const IMAGE_BUDGET_BYTES = 2 * 1024 ** 3;   // 2GB
const EVICT_TARGET_RATIO = 0.95;            // 淘汰到 95% 以下

async function ensureImageBudget(env, incomingBytes) {
  const used = await liveBytes(env);        // SUM(bytes + thumb_bytes) WHERE deleted_at IS NULL
  if (used + incomingBytes <= IMAGE_BUDGET_BYTES) return;

  const target = Math.floor(IMAGE_BUDGET_BYTES * EVICT_TARGET_RATIO);
  let need = used + incomingBytes - target;

  // 优先级：① 孤儿图（传了没发出去）② 已删除/隐藏内容的图 ③ 最老的非置顶
  // 注意：三段 SQL 的占位符个数不同（只有第一段有 ?），**不能共用一个 .bind()** ——
  // D1 对绑定参数个数与占位符不符会直接抛错。所以把参数一起放进数组。
  const stages = [
    {
      sql: `SELECT id, bytes, thumb_bytes FROM images
              WHERE deleted_at IS NULL AND takedown_at IS NULL
                AND status = 'unused' AND created_at < ?
              ORDER BY created_at ASC LIMIT 200`,
      args: [Date.now() - 86400000],
    },
    {
      sql: `SELECT i.id, i.bytes, i.thumb_bytes FROM images i
              LEFT JOIN posts p ON i.ref_kind='post' AND i.ref_id=p.id
              WHERE i.deleted_at IS NULL AND i.takedown_at IS NULL
                AND (p.hidden=1 OR p.deleted_at IS NOT NULL)
              ORDER BY i.created_at ASC LIMIT 200`,
      args: [],
    },
    {
      sql: `SELECT i.id, i.bytes, i.thumb_bytes FROM images i
              LEFT JOIN posts p ON i.ref_kind='post' AND i.ref_id=p.id
              WHERE i.deleted_at IS NULL AND i.takedown_at IS NULL
                AND COALESCE(p.pinned,0)=0
              ORDER BY i.created_at ASC LIMIT 200`,
      args: [],
    },
  ];

  for (const s of stages) {
    const rows = await env.DB.prepare(s.sql).bind(...s.args).all();
    for (const r of (rows.results || [])) {
      if (need <= 0) return;
      await deleteImageObject(env, r.id);
      need -= r.bytes + r.thumb_bytes;
    }
  }
}
```

**绝不删**：`pinned=1`、举报待审、`takedown_at IS NOT NULL`（证据已挪到私有桶，见 §5.1）的图。

### 5.2.1 本节引用的辅助函数（实现时必须有，签名在此固定）

```js
/** 当前存活的图片总字节（full + thumb），2GB 预算的唯一口径 */
async function liveBytes(env) {
  const r = await env.DB.prepare(
    `SELECT COALESCE(SUM(bytes + thumb_bytes), 0) AS n
       FROM images WHERE deleted_at IS NULL AND takedown_at IS NULL`
  ).first();
  return r ? r.n : 0;
}

/** 删对象 + 行标记（顺序：先 R2 后 D1；R2 已不存在也算成功，保证重试幂等） */
async function deleteImageObject(env, imageId) {
  const row = await env.DB.prepare('SELECT * FROM images WHERE id = ?').bind(imageId).first();
  if (!row || row.deleted_at) return;
  const bucket = env[row.r2_bucket === 'anibox-private' ? 'R2_PRIVATE' : 'R2_PUBLIC'];
  for (const k of [row.r2_full, row.r2_thumb].filter(Boolean)) {
    try { await bucket.delete(k); } catch { /* 已不存在：继续 */ }
  }
  await env.DB.prepare(
    'UPDATE images SET deleted_at = ?, deleted_by = ? WHERE id = ?'
  ).bind(Date.now(), 'evict', imageId).run();
}

async function isDeviceBanned(env, deviceCode) {
  const r = await env.DB.prepare('SELECT 1 FROM device_bans WHERE device_code = ?')
    .bind(deviceCode).first();
  return !!r;
}

function bearerToken(request) {
  const h = request.headers.get('authorization') || '';
  return h.startsWith('Bearer ') ? h.slice(7).trim() : '';
}

/** 最后活跃：节流写（同一设备 5 分钟内只写一次），否则每个请求都写 D1 会烧写行配额 */
async function touchDevice(env, deviceId) {
  await env.DB.prepare(
    'UPDATE devices SET last_seen_at = ? WHERE id = ? AND last_seen_at < ?'
  ).bind(Date.now(), deviceId, Date.now() - 5 * 60 * 1000).run();
}

/** IP 只存网段：IPv4 保留前三段，IPv6 保留前四组 */
function ipNet(ip) {
  if (!ip) return 'unknown';
  if (ip.includes(':')) return ip.split(':').slice(0, 4).join(':') + ':*';
  const p = ip.split('.');
  return p.length === 4 ? `${p[0]}.${p[1]}.${p[2]}.*` : ip;
}
```

**`touchDevice` 的节流是必须的**：社区每 180 秒轮询通知，若不节流，1000 在线用户 = 每天 48 万次 D1 写行，白烧配额。


### 5.3 Cron（`wrangler.toml`）

```toml
[triggers]
crons = ["0 4 * * *"]   # 每天 04:00 UTC
```

```js
async function scheduled(env) {
  const now = Date.now();

  // ① 审计日志 90 天。**管理员动作不删**（处置证据，与 §7.8 对用户的承诺一致）
  //    注意：这里保留的是全部 admin_* 动作，不只是 takedown —— 处置动作量极小
  //    （一个管理员一辈子点不了几千次），留全量才能回答「谁在什么时候因为什么封了谁」。
  await env.DB.prepare(
    `DELETE FROM audit_log WHERE ts < ? AND action NOT LIKE 'admin_%'`
  ).bind(now - 90 * 86400000).run();

  // ② 过期临时表
  await env.DB.prepare('DELETE FROM pending_regs WHERE expires_at < ?').bind(now).run();
  await env.DB.prepare('DELETE FROM email_sends WHERE created_at < ?').bind(now - 7 * 86400000).run();
  await env.DB.prepare('DELETE FROM login_fails WHERE first_at < ?').bind(now - 86400000).run();

  // ③ 图片行标记：R2 对象已被生命周期删掉，这里只把 D1 行标成 deleted。
  //    takedown_at 的行跳过（证据在私有桶、不设期，见 §5.1）
  await env.DB.prepare(
    `UPDATE images SET deleted_at = ?, deleted_by = 'lifecycle'
      WHERE deleted_at IS NULL AND takedown_at IS NULL
        AND scope = 'post' AND created_at < ?`
  ).bind(now, now - 30 * 86400000).run();

  // ④ 通知 60 天
  await env.DB.prepare('DELETE FROM notifications WHERE created_at < ?')
    .bind(now - 60 * 86400000).run();
}
```

**第 ① 条排除管理员动作是一个刻意的例外**，必须写清楚原因：普通用户的发言/动作日志按承诺 90 天清掉；**管理员自己的处置动作（撤回、删除、封禁、封设备）及其关联的证据永远保留**——这是「处置有没有做过、谁做的、什么理由」的唯一凭证。用户告知文案里已明确「已被举报并处置的内容记录会继续保留」。


### 5.4 正文永久保存的配套

- 帖子/评论行**没有任何 TTL**
- 用户删自己的 → 软删（`deleted_at`），正文留在库里（审计需要），前端显示「该内容已删除」
- 「我的发言」接口**不过滤时间**，用户永远能翻到自己的历史

---

## 6. 客户端：Rust

### 6.1 新增/改动文件

```
src-tauri/src/
  community.rs           改：保留 v1 命令，新增 v2 命令入口
  community_auth.rs      新：注册/登录/验证/登出/设备会话（token 加密落盘）
  community_images.rs    新：选图 → 校验 → 压缩 → 上传（multipart）
  community_admin.rs     新：举报/撤回/封禁/审计查询（复用同一 HTTP 层）
  config.rs              改：新增通用 seal/unseal（enc:v1 那套密钥复用）
  feedback.rs            改：提交时带 imageIds；日志附件逻辑不变
  Cargo.toml             改：image +png/gif/bmp/tiff/webp；reqwest +multipart
  lib.rs                 改：注册新命令
```

### 6.2 依赖改动

```toml
# multipart 上传（reqwest 的 feature，不新增 crate）
reqwest = { version = "0.12", default-features = false,
            features = ["json", "rustls-tls", "stream", "multipart"] }

# 多格式解码（AVIF 明确不开：会引入 C 依赖 ravif）
# [lc-2442 勘误] 项目在 lc-1811 前后已为 image 开过 jpeg/webp/png（heic.rs 等在用），
# 实际只需补 gif/bmp/tiff —— 体积影响同量级。
image = { version = "0.25", default-features = false,
          features = ["jpeg", "png", "webp", "gif", "bmp", "tiff"] }
```

体积影响：png ≈ +40KB、webp ≈ +60KB、gif ≈ +30KB、bmp/tiff 可忽略。

### 6.2.1 [lc-2442 施工补正] 服务端图片对象的三个硬约束（写代码前必读）

这三条是 P2 实测抓出来的，服务端实现与任何自建实现都必须遵守：

1. **R2 `put` 必须带 `httpMetadata.contentType`**：R2 缺省回 `application/octet-stream`，
   浏览器 `<img>` 对非 `image/*` 一律拒渲染（imgproto.rs lc-2091 同款教训）——
   不带 contentType 等于公开域全裂图。
2. **GIF 对象键用 `.gif` 后缀**（JPEG 用 `.jpg`）：键名要诚实，调试时一眼分辨格式；
   content-type 由 fmt 决定，但键后缀不说谎能省掉所有「为什么这张是乱码」的排查。
3. **`image` 0.25 的 `ImageDecoder` 没有 `decode()` 方法**：正确路径是
   `into_decoder() → orientation()/dimensions() → DynamicImage::from_decoder(decoder)`
   （from_decoder by value，内部自己 read_image）。网上大量旧教程的
   `decoder.decode()` 写法在 0.25 编译不过。


### 6.3 token 落盘

复用 `config.rs` 的 AES-256-GCM（`enc:v1:` 前缀那套）。新增通用函数：

```rust
pub fn seal(plain: &str) -> String;     // → "enc:v1:<base64(nonce||ct)>"
pub fn unseal(stored: &str) -> String;  // 非 enc: 前缀原样返回（兼容旧值）
// 原 encrypt_access_code / decrypt_access_code 改为调这两个，行为不变
```

文件：`app_data_dir/community-session.json`

```json
{
  "v": 2,
  "accountId": "u_xxx",
  "email": "ab***@qq.com",
  "nick": "小明",
  "nickSuffix": "a3f2",
  "token": "enc:v1:....",
  "deviceId": "d_xxx"
}
```

**绝不写日志**：`token` 与密码原文一律不进 `libmpv::trace`，也不进诊断包。唯一的例外是「是否已登录」布尔值。

### 6.4 设备信息采集

```rust
fn device_info(app: &AppHandle) -> DeviceInfo {
    let os = match std::env::consts::OS { "windows" => "Windows", "macos" => "macOS", _ => "Linux" };
    // 物理像素 + 缩放：Tauri 的 Monitor 给物理像素与 scale_factor
    let (screen, scale) = app.get_webview_window("main")
        .and_then(|w| w.current_monitor().ok().flatten())
        .map(|m| {
            let s = m.size();
            (format!("{}x{}", s.width, s.height), (m.scale_factor() * 100.0).round() as i32)
        })
        .unwrap_or_else(|| ("unknown".into(), 100));
    let code = crate::community::derive_device_id(&crate::usage_stats::machine_raw_id_pub());
    DeviceInfo { code, os: os.into(), screen, scale }
}
```

`derive_device_id` 与 v1 完全相同（盐 `anibox-community-v1:`，取 sha256 前 16 字节 → 32 位 hex），**已有 4 个单测守着，不要改盐**。

### 6.5 图片压缩管线（`community_images.rs`）

```rust
pub const MAX_ORIGINAL: u64 = 50 * 1024 * 1024;  // 原文件 50MB
pub const MAX_FULL_OUT: usize = 1024 * 1024;      // 压缩后 1MB 硬上限
pub const MAX_THUMB_OUT: usize = 120 * 1024;      // 缩略图 120KB
pub const MAX_EDGE: u32 = 12_000;                 // 解码尺寸上限（防炸弹）

/// 压缩阶梯：先降质量，再降尺寸。命中即返回。
const LADDER: &[(u32, u8)] = &[
    (1600, 88), (1600, 84), (1600, 80), (1600, 76), (1600, 72),
    (1400, 80), (1400, 76), (1200, 80), (1200, 76), (1080, 76),
];

pub struct Prepared {
    pub full: Vec<u8>,        // JPEG 或原样 GIF
    pub thumb: Option<Vec<u8>>,
    pub w: u32, pub h: u32,
    pub fmt: String,          // 原始格式名
}
```

处理顺序（**顺序写错就糊图**）：

```
① 读文件；len > 50MB → Err("图片过大（最大 50MB）")
② 魔数判定格式：
     FFD8FF → jpeg       89504E47 → png
     GIF87a/89a → gif    RIFF....WEBP → webp
     BM → bmp            II*/MM* → tiff
     ftyp 含 heic/heix/hevc/hevx → heic
     含 ftypavif/avis → Err("不支持 AVIF，请转成 JPG 或 PNG")
     其他 → Err("不支持的文件类型")
③ heic → crate::heic::heic_to_jpeg(app, &bytes)?
     libde265 缺失/解码失败 → Err("HEIC 解码失败")，绝不静默
④ gif 特殊分支：
     len <= 1MB → 原样保留（不重编码，保住动画），thumb = None，返回
     len >  1MB → Err("动图需 ≤1MB")
⑤ 其余格式：ImageReader + Limits { max_image_width: 12000,
     max_image_height: 12000, max_alloc: 512MiB }   ← 防解压炸弹，必须有
⑥ orientation() → DynamicImage::apply_orientation   ← 手机竖拍转正
⑦ 透明通道：PNG/WebP 有 alpha → composite 到**白底**（不是黑底，深色 UI 上像破图）
⑧ 按 LADDER 逐档：resize(Lanczos3, 长边<=edge，短边等比；**绝不放大**)
     → JpegEncoder::new_with_quality(q) → 体积 <= 1MB 则命中
⑨ 十档全不中 → Err("图片细节过多，压缩后仍超过 1MB，请换一张")
⑩ thumb：长边 400, q80；> 120KB 则 q72；> 120KB 则长边 320, q72
```

**已知编码约束（如实记录）**：

- `image` 0.25 的 `JpegEncoder` 固定 **4:2:2** 子采样（源码 `encoder.rs:473`），没有 4:4:4 选项。对动漫硬边会有轻微色度渗出，只在 1:1 放大时可见。
- 1MB 上限意味着少数超高细节图会被降到 1200px q76。**这是上限的必然代价**，UI 上必须显示「已压缩到 XXX KB（原 X.X MB）」让用户知情。

### 6.6 Rust 命令清单

```rust
// 账号
community_v2_register(email, password, nick) -> PendingReg
community_v2_resend(pending_id) -> Cooldown
community_v2_verify(pending_id, code) -> Session
community_v2_login(email, password) -> Session     // 内部自动采设备信息
community_v2_logout() -> ()
community_v2_session() -> Option<Session>
community_v2_set_nick(nick) -> Session
community_v2_change_password(old, new) -> ()
community_v2_delete_account(password) -> ()

// 设备
community_v2_devices() -> Vec<DeviceRow>
community_v2_revoke_device(id) -> ()
community_v2_revoke_others() -> ()

// 图片
community_pick_and_prepare(scope) -> PreparedView  // 选图 + 压缩，返回预览与体积对比
community_upload_image(prepared_id) -> ImageRef    // 按 id 上传暂存的压缩结果
community_discard_prepared(prepared_id) -> ()      // 用户移除/换图时释放
community_v2_image_bytes(id) -> (ct, bytes)        // 私有图（反馈）取字节

// 社区
community_v2_post(req) -> Post
community_v2_feed(sort, cursor) -> Page
community_v2_work(media_type, tmdb_id) -> WorkDetail
community_v2_post_detail(id) -> PostDetail
community_v2_delete_post(id) -> ()
community_v2_comment(req) -> Comment
community_v2_delete_comment(id) -> ()
community_v2_mine(cursor) -> Page
community_v2_notifications(cursor) -> Page
community_v2_read_notifications(ids, all) -> ()
community_v2_report(kind, ref_id, reason, detail) -> ()

// 管理
admin_reports(status, cursor) -> Page
admin_retract_post(id, reason) -> ()
admin_unretract_post(id) -> ()
admin_delete_post(id) -> ()
admin_retract_comment(id, reason) -> ()
admin_delete_comment(id) -> ()
admin_delete_image(id) -> ()
admin_ban_account(id, reason) -> ()
admin_unban_account(id) -> ()
admin_mute_account(id, minutes, reason) -> ()
admin_ban_device_code(code, reason) -> ()
admin_audit(query) -> Page
admin_stats() -> Stats

// 反馈（附图）
feedback_submit_v2(message, contact, include_log, page, image_ids) -> {id}
```

---

## 7. 客户端：前端

### 7.1 文件结构（按功能拆，不堆大文件）

```
src/components/community/
  CommunityPage.tsx      页面壳：登录态分流（未登录→AuthPanel / 已登录→信息流）
  AuthPanel.tsx          注册（邮箱+密码+验证码三段）/ 登录
  FeedView.tsx           信息流列表（新到旧 / 热门）
  PostCard.tsx           单帖卡片（含管理员操作菜单）
  PostComposer.tsx       发帖（影评一键带入 + 图片）
  PostDetail.tsx         帖子详情 + 评论
  WorkDiscussion.tsx     作品讨论聚合（评分摘要 + 长评 + 短评）
  DeviceManager.tsx      设备管理（照用户给的那张截图的信息结构）
  AdminPanel.tsx         管理台壳（三个页签）
  AdminReports.tsx       举报队列
  AdminAudit.tsx         审计日志
  AdminUsers.tsx         用户与设备封禁
  ReportDialog.tsx       举报弹窗
  NotificationsView.tsx  通知列表
  ImageUploader.tsx      选图 / 预览 / 压缩结果 / 上传进度
  CommunityImage.tsx     图片容器：加载失败 → 「配图已过期」占位

src/lib/community.ts     类型定义 + invoke 封装（唯一网络入口）
src/lib/communityUnread.ts  通知未读快照（照 feedbackUnread.ts 的模式）
src/styles/55-community.css 新
```

### 7.2 与现有代码的接线点

| 位置 | 改动 |
|---|---|
| `App.tsx` | `view` 联合类型加 `"community"`；条件挂载 `<CommunityPage/>`；`activeGuid` 加 `"_community"` |
| `Sidebar.tsx` | 加「社区」入口（图标 `Users`），带未读角标 |
| `DetailView.tsx` | 「讨论」tab 保留；新增「分享到社区」按钮（读 `get_my_rating` + `tmdbId`） |
| `WatchHistoryView.tsx` | 详情浮层「保存我的评价」旁加「同时分享到社区」勾选 |
| `FeedbackPanel.tsx` | 新建反馈区加 `ImageUploader` |
| `config.rs` / 设置面板 | 新增「社区」区块：当前账号 / 设备管理 / 注销 / 退出登录 |

### 7.3 管理员界面差异（`role === 'admin'` 时）

| 区域 | 普通用户 | 管理员 |
|---|---|---|
| 侧栏 | 社区 · 反馈 · 观影记录 · 设置 | 同左 + **管理台** |
| 帖子/评论菜单 | 回复 · 举报 | 回复 · 举报 · **撤回** · **删除** |
| 图片右上角 | — | **删图** |
| 被隐藏内容 | 不可见 | 半透明 + 「已隐藏（N 人举报）」 |
| 设备管理页 | 只看自己 | 每个账号下列出全部设备 + 「封禁此设备」 |

**前端判断只是渲染**，每个管理动作服务端都会再过一次 `requireAdmin`。

### 7.4 图片显示（含过期占位）

```tsx
function CommunityImage({ src, thumb, alt }: Props) {
  const [failed, setFailed] = useState(false);
  if (failed) {
    return (
      <div className="cimg-expired" role="img" aria-label="配图已过期">
      <IcoImageOff size={16} />
      <span>配图已过期</span>
    </div>
    );
  }
  return <img src={thumb ?? src} alt={alt} loading="lazy"
              onError={() => setFailed(true)} />;
}
```

30 天后 R2 对象已删 → URL 404 → `onError` → 显示灰底占位。**比浏览器的碎图标好，实现成本一样。**

### 7.5 图片上传交互

```
[选择图片] → Rust 压缩 → 返回 {原大小, 压缩后, 尺寸, 预览 base64}
   ↓
卡片显示：「已压缩 2.8MB → 312KB · 1600×900」，右侧「移除」
   ↓
点「发送」→ multipart 上传 → 拿到 imageId → 随正文提交
   ↓
上传失败 → 保留预览 + 错误提示 + 「重试」（只重传这张，不重发正文）
```

压缩在**选择时**完成（而不是发送时），用户能先看到「压缩到什么程度」再决定要不要换图。

### 7.6 未读通知

照 `src/lib/feedbackUnread.ts` 的现成模式：本地已读快照 + 60 秒轮询 + `sig` 去重。
**不同点**：社区通知轮询频率必须低于工单，取 **180 秒**（社区不如工单紧急，且要控 D1 读行）。

### 7.7 图片本地缓存的落地路径（必须明确，否则会绕开 disk_cache）

社区图是 `https://img.690075.xyz/...` 的**外部 HTTP 图**，`<img src>` 直连时**不走** `fntvimg` 自定义协议，也就享受不到 `disk_cache` 的落盘与 LRU。这跟 NAS 海报的机制不同，必须专门接一条路：

```
现状（海报）：<img src="http://fntvimg.localhost/__tmdb/...">  → imgproto 代理 → disk_cache
社区图现状：  <img src="https://img.690075.xyz/c/xxx.jpg">   → 浏览器直连，无本地缓存
             ✗ 30 天后云端删了，本地连缩略图都没有 → 完全没法显示
```

**做法**：给 `imgproto.rs` 加第四条前缀 `/__cm/<id>.<ext>`，转发到 `img.690075.xyz`：

```rust
if let Some(rest) = path.strip_prefix("/__cm/") {
    // rest 形如 "c/img_xxx.jpg" / "t/img_xxx.jpg"
    if !safe_rel(rest) { return Err((400, "bad community path".into())); }
    return Ok(Target::Public {
        // key 带 cm 段：分包统计与淘汰用（见下）
        key: format!("img:cm:{rest}"),
        url: format!("https://img.690075.xyz/{rest}"),
    });
}
```

前端 `communityImageUrl(id, { thumb })`：

```ts
export function communityImageUrl(path: string): string {
  // path 是服务端返回的 r2 key（'c/img_x.jpg' 或 't/img_x.jpg'）
  return imgUrl(`/__cm/${path}`);
}
```

**收益**：
1. 走 `disk_cache` → 30 天后云端删了，**本地看过的还能显示**（这是意外的好处，不用额外做）
2. 统一受 256MB 子限额与 LRU 管理
3. 图片加载失败时 `onError` 才真正触发（直连时 CDN 的 404 也会触发，但没本地兜底）

**`communityImageUrl` 的输入必须是服务端返回的 `r2_full` / `r2_thumb` 字段**，不要在前端拼 UUID——桶前缀（`c/` `t/`）是服务端知识。

### 7.8 注册页隐私告知（原文，逐字使用）

注册表单底部必须有勾选框，**默认不勾**，不勾时「注册」按钮 disabled：

```
☐ 我已阅读并同意以下内容

使用社区功能需要注册账号。为了社区安全与违规追责，我们会记录你的
发言内容、操作动作、账号、设备标识与 IP 网段，保留 90 天。这些记录
仅用于处理举报与违规，不对外提供。

你可以在设置中注销账号。注销后我们会删除你的账号信息与 90 天内的
审核日志；已被举报并处置的内容记录会继续保留，用于应对必要的核查。
```

**文案里每一句都必须是真的**（对照 §5.3 与 §4.2 的注销实现）：

| 承诺 | 代码依据 |
|---|---|
| 记录发言/动作/账号/设备/IP 网段 | `audit_log` 六列全覆盖 |
| 保留 90 天 | Cron 删 `ts < now-90d` |
| 不对外提供 | 无任何导出接口；管理端点需 `role=admin` |
| 注销删除账号信息 | `DELETE /v2/account` 置 `deleted` + 邮箱替换 |
| 处置记录继续保留 | `takedown_at` 的内容与对应审计行 **不删** |

**不要在这段文案里加「我们非常重视您的隐私」这类空话**——用户级规范禁止冗余解释性文案，且空话会让人怀疑具体承诺。

### 7.9 disk_cache 分组限额（引用现有实现位置）

`src-tauri/src/disk_cache.rs` 现状：单一 `LIMIT_BYTES`（默认 2GB）+ 全局 LRU。

新增「分组」概念，最小改动方案（约 60 行）：

```rust
/// 分组 = key 的第二个冒号段。'img:cm:xxx' → "cm"；'img:tmdb:xxx' → "tmdb"；无第二段 → "misc"
fn group_of(key: &str) -> &str {
    let mut it = key.split(':');
    it.next();                        // 跳过 "img" / "json"
    it.next().unwrap_or("misc")
}

/// 每组独立限额（MB）。0 或缺省 = 不限（走全局 LRU）。
/// 社区图 256MB：超出只淘汰 img:cm:*，绝不动海报。
const GROUP_LIMIT_MB: &[(&str, u64)] = &[("cm", 256)];
```

- `put` 后：先按组限额淘汰该组最老，再走全局 `enforce_limit`
- 全局限额仍是 `config.cache_limit_mb`（默认 2048），**总占用口径不变**
- 设置面板「缓存」一栏显示分组占用：`海报与元数据 812MB / 社区图片 47MB`

**写失败的处理（接 §4.1 隐患二）**：

```rust
// 现状：if std::fs::write(&path, bytes).is_err() { return; }  ← 静默
let Err(e) = std::fs::write(&path, bytes) else { /* ok */ };
crate::libmpv::trace(&format!("[cache] 写盘失败 {key}: {e}（本次仅内存层有效）"));
note_write_failure();   // 连续 3 次 → 熔断 10 分钟：put 只进内存层，不再尝试磁盘
```

熔断状态用 `AtomicU64` 存「失败次数 + 熔断截止时间」，不引入新依赖。

---

## 8. 错误码

| HTTP | code | 中文提示 |
|---|---|---|
| 400 | `INVALID_REQUEST` | 参数不合法 |
| 400 | `BAD_IMAGE` | 图片格式不受支持 |
| 400 | `IMAGE_TOO_LARGE` | 图片过大，压缩后仍超过 1MB |
| 400 | `WEAK_PASSWORD` | 密码需 8 位以上且含字母和数字 |
| 401 | `UNAUTHORIZED` | 请先登录 |
| 401 | `BAD_CREDENTIALS` | 邮箱或密码错误 |
| 401 | `DEVICE_REVOKED` | 该设备已被登出 |
| 403 | `BANNED` | 账号已被封禁 |
| 403 | `DEVICE_BANNED` | 该设备已被封禁 |
| 403 | `MUTED` | 你已被禁言 |
| 403 | `FORBIDDEN` | 需要管理员权限 |
| 404 | `NOT_FOUND` | 内容不存在 |
| 409 | `EMAIL_TAKEN` | 该邮箱已注册 |
| 409 | `TOO_MANY_DEVICES` | 设备数量已达上限，请先在旧设备登出 |
| 409 | `PARENT_DELETED` | 该内容已删除，无法回复 |
| 422 | `BAD_CODE` | 验证码错误或已过期 |
| 429 | `RATE_LIMITED` | 操作太频繁，请稍后再试 || 429 | `TOO_MANY_ATTEMPTS` | 尝试次数过多，请 15 分钟后再试 |
| 503 | `MAIL_UNAVAILABLE` | 验证邮件发送失败，请稍后重试 |

**失败一律给人话**：`community_post` 已经在做「取服务端 `error.message` 直接展示」，v2 保持这个做法。

---

## 9. 限额汇总（可调常量集中在一处）

```js
// inbox-mirror/worker.js 顶部
const LIMITS = {
  PWD_MIN: 8, PWD_MAX: 64,
  NICK_MAX: 24,
  CODE_TTL_MS: 10 * 60 * 1000,
  CODE_RESEND_SEC: 60,
  CODE_EMAIL_PER_HOUR: 5,
  CODE_IP_PER_HOUR: 10,
  LOGIN_FAIL_LOCK: 5, LOGIN_LOCK_MS: 15 * 60 * 1000, LOGIN_IP_PER_HOUR: 20,
  DEVICES_MAX: 8,                       // 0 = 不限
  POST_MIN: 3, POST_MAX: 5000,
  COMMENT_MIN: 3, COMMENT_MAX: 2000,
  WRITE_PER_MIN: 5, WRITE_MIN_GAP_MS: 3000,
  IMAGE_FULL_MAX: 1024 * 1024,
  IMAGE_THUMB_MAX: 120 * 1024,
  IMAGE_PER_HOUR: 10, IMAGE_PER_DAY: 30,
  NEW_ACCOUNT_HOURS: 24,                // 新号窗口
  NEW_ACCOUNT_POSTS: 5, NEW_ACCOUNT_IMAGES: 3,
  IMAGE_BUDGET_BYTES: 2 * 1024 ** 3,
  REPORT_AUTOHIDE: 3,                   // 3 个不同人 → 自动隐藏
  AUDIT_KEEP_DAYS: 90,
  IMAGE_KEEP_DAYS: 30,
  FEEDBACK_IMAGE_KEEP_DAYS: 90,
};
```

客户端对应常量集中在 `src-tauri/src/community_images.rs` 与 `src/lib/community.ts` 顶部，**改动时必须两边同步**（写进注释）。

---

## 10. 安全边界（真实生效的，不是声称的）

| 约束 | 实现 |
|---|---|
| 密码 | PBKDF2-HMAC-SHA256 10 万次，存哈希不存明文 |
| 验证码 | 存 sha256，10 分钟过期，5 次错误作废 |
| token | 存 sha256，客户端 AES-256-GCM 加密落盘 |
| SQL | 全部参数化查询 |
| XSS | 正文原样存储、**前端一律 React 文本插值**（`{text}`），禁 `dangerouslySetInnerHTML` |
| 上传 | 服务端重验魔数 + 字节上限，不信客户端 |
| 越权 | 删改一律校验 `owner_id`/`account_id`；管理操作过 `requireAdmin` |
| 管理身份 | 由 secret 现算，不落库；数据库篡改无法提权 |
| 设备封禁 | 按 `device_code`（物理机），重登无效 |
| 举报滥用 | 同人去重 + 3 人阈值（不是 3 次） |

**明确做不到的**：无法验证「这个用户是否拥有这部片」（飞牛没有对外权限接口，客户端自报可伪造）。评论区公开——这是取舍，不是遗漏，与 v1 文档口径一致。

---

## 11. 施工顺序（P1–P4）

每个阶段独立可验证，**不通过不进下一阶段**。

### P1 账号（4–5 天）

| # | 任务 | 产出 | 验证 |
|---|---|---|---|
| 1 | `schema-v2.sql` 落库 | 表全部建立 | `wrangler d1 execute --file` 无报错 |
| 2 | Worker 认证路由（register/resend/verify/login/logout） | 5 个端点 | `tests/verify_community_auth.cjs` |
| 3 | 邮件发送 + Resend 降级 | 真发一封到 QQ 邮箱 | 手工确认收到、不在垃圾箱 |
| 4 | `community_auth.rs` | 命令可用 | `cargo test community_auth` |
| 5 | `AuthPanel.tsx` + `DeviceManager.tsx` | 注册/登录/设备页 | 手工走查完整流程 |
| 6 | `Sidebar` + `App` 接线 | 侧栏可见「社区」 | 点击进入登录页 |
| 7 | **注册页隐私告知 + 勾选框**（§7.8 文案） | 不勾不能注册 | 手工点一遍 |

**P1 验收**：新邮箱能注册、收到验证码、登录成功、设备管理页显示本机（Windows · 3840×2160 · 150% · 本机）、能登出、不勾隐私告知时注册按钮禁用。

### P2 图片（4–5 天）

| # | 任务 | 验证 |
|---|---|---|
| 1 | R2 两个桶 + 四条生命周期规则（c/ t/ f/；e/ 不设期） | 控制台确认规则已启用 |
| 2 | `POST /v2/images` 上传 + 重验 + 记账 | `tests/verify_community_images.cjs` |
| 3 | `community_images.rs` 压缩管线 | `tests/verify_image_pipeline.cjs`（见 §12） |
| 4 | `ImageUploader.tsx` + `CommunityImage.tsx` | 手工：选 3 种格式、看压缩结果 |
| 5 | **`imgproto.rs` 加 `/__cm/` 前缀**（§7.7） | 社区图走本地缓存；断网后看过的图仍显示 |
| 6 | 反馈附图（`/v2/feedback` + `FeedbackPanel`） | 提交带图工单，会话里能看到图 |
| 7 | `disk_cache` 分组限额 + 写失败日志/熔断（§7.9） | `tests/verify_cache_group.cjs` |

**P2 验收**：PNG 2.8MB → 312KB 上传成功；AVIF 被拒；5MB GIF 被拒；2MB GIF 通过；1600px 图在列表中加载为缩略图；拔网线后已看过的社区图仍能显示。

### P3 社区（5–6 天）

| # | 任务 | 验证 |
|---|---|---|
| 1 | posts/comments 写入 + 信息流 + 作品聚合 | `tests/verify_community_posts.cjs` |
| 2 | 影评分享（历史页/详情页按钮） | 手工：评分+评语 → 分享 → 信息流可见 |
| 3 | `FeedView` / `PostCard` / `PostDetail` | 手工走查 |
| 4 | 通知（写入 + 轮询 + 角标） | 别人回复我 → 180s 内角标亮 |
| 5 | 「我的发言」永久可翻 | 造 40 天前数据，仍可查到 |

### P4 举报 + 管理 + 审计（3–4 天）

| # | 任务 | 验证 |
|---|---|---|
| 1 | 举报写入 + 去重 + 3 人自动隐藏 | 3 个账号报同一帖 → `hidden=1` |
| 2 | 管理端点（撤回/删除/封禁/禁言/删图） | `tests/verify_community_admin.cjs` |
| 3 | `AdminPanel` 三个页签 | 手工：管理员登录看到「管理台」，普通用户看不到 |
| 4 | 审计写入全动作 | 每条动作在 `audit_log` 有对应行，含 body_snap |
| 5 | Cron 清理 | 手工触发 `scheduled()`，90 天前的行被删 |
| 6 | 注册页隐私告知文案 + 勾选框 | 不勾不能注册 |

**P4 是上线前置**：没有举报和管理工具就开放注册，第一天出事你没有任何处置手段。

---

## 12. 测试计划

服务端测试（`node:sqlite` 内存库跑真实 `worker.js`，沿用 `tests/verify_inbox_comments_2331.cjs` 的模式，**不入库**）：

| 脚本 | 覆盖 |
|---|---|
| `verify_community_auth.cjs` | 注册/重复邮箱/弱密码/验证码错误 5 次/过期/登录成功/密码错/锁定 15 分钟/设备封禁/超 8 台 |
| `verify_community_images.cjs` | JPEG/PNG(透明)/GIF/AVIF 拒/超 1MB 拒/孤儿图淘汰/2GB 兜底淘汰/私有图越权 |
| `verify_community_posts.cjs` | 发帖/幂等/分页/删除自己/删别人 404/隐藏内容不可见/「我的发言」不过滤时间 |
| `verify_community_admin.cjs` | 非管理员 403/撤回可见性/删除不可恢复/封账号/封设备码/审计行完整性/3 人举报自动隐藏 |
| `verify_image_pipeline.cjs` | 压缩阶梯（见下） |

**压缩管线双向验证**（照项目既有做法：未修复版本必须 FAIL、修复版本 PASS）：

1. 取一张真实 3840×2160 JPEG（约 3–5MB）
2. 断言：输出 ≤ 1MB、长边 ≤ 1600、输出是 JPEG 魔数
3. 取 1×1 像素 PNG：断言不放大（输出仍 1×1）
4. 取带 EXIF Orientation=6 的 JPEG：断言输出宽高互换（已转正）
5. 取 20000×20000 构造 PNG：断言被 `Limits` 拒绝而不是 OOM
6. 取 2MB GIF：断言原样返回且 `thumb == None`
7. 取纯随机噪声 PNG 12000×12000：断言最终返回 `Err`（十档都超 1MB）

> 跑测试遵守全局纪律：**一轮只跑一个浏览器/重脚本**，其余用 `node --check` 与纯函数单测替代。

---

## 13. 部署清单（一次性）

```bash
# ① D1（**沿用现有 anibox-comments 库**，保住 v1 评论数据 —— 不新建库）
npx wrangler d1 execute anibox-comments --file=./schema-v2.sql --remote   # v2 表 IF NOT EXISTS 新建、v1 表 no-op
npx wrangler d1 execute anibox-comments --file=./migrate-v1-to-v2.sql --remote  # 只跑一次：comments 补三列
# （仅当从零重建时才 d1 create；全新库只跑 schema-v2.sql，migrate 不要跑 —— ALTER 会报重复列）

# ② R2
npx wrangler r2 bucket create anibox-public
npx wrangler r2 bucket create anibox-private
# 自定义域：anibox-public → img.690075.xyz
# 生命周期三条规则（§5.1）在控制台或用 wrangler 的 lifecycle 配置设置

# ③ 密钥
npx wrangler secret put ANIBOX_ADMIN_EMAILS       # "your@mail.com"
npx wrangler secret put ANIBOX_INBOX_TOKEN        # 弹幕映射写入令牌（已有）
npx wrangler secret put RESEND_KEY                # 邮件备用通道（可选）

# ④ 邮件域名
# CF 控制台 → Email → Email Sending → 添加 690075.xyz → 按提示写 SPF/DKIM/DMARC

# ⑤ 部署
npx wrangler deploy
```

**验证**：`curl https://inbox.690075.xyz/health` → `{"data":{"status":"ok","version":2}}`

---

## 14. 已知限制与风险（如实列出）

| # | 限制 | 影响 | 缓解 |
|---|---|---|---|
| 1 | 8 位字母+数字密码离线可破解 | 拖库后多数密码可还原 | 登录失败锁定 + 无密码找回（减少连带面） |
| 2 | PBKDF2 抗 GPU 弱于 Argon2 | 同上 | CF Workers 只支持 PBKDF2，无替代 |
| 3 | JPEG 固定 4:2:2 | 动漫硬边 1:1 放大有轻微色度渗出 | 1600px 下肉眼不可见 |
| 4 | 1MB 上限会牺牲超高细节图 | 少数图降到 1200px q76 | UI 明示压缩结果 |
| 5 | Email Sending 是 Beta | 日配额逐步放开，可能偶发失败 | Resend 降级 + 重发按钮 |
| 6 | R2 生命周期「到期后 24 小时内」删 | 图片可能多留一天 | 无实际影响 |
| 7 | 30 天过期后图片裂 | 老帖观感变差（**用户明确接受**） | 灰底「配图已过期」占位 |
| 8 | 无法验证用户是否拥有该片 | 评论区任何人都能评论任何作品 | 与 v1 同口径，文档已声明 |
| 9 | 没有实名 | 有刷子成本门槛但无法清零 | 邮件唯一 + 新号限额 + 设备封禁 |
| 10 | D1 单库 10GB 硬顶 | 约 1800 万条发言后无解 | 届时按季度归档到 R2 冷存 |

---

## 15. 变更记录

| 日期 | 内容 |
|---|---|
| 2026-11-27 | 初版：定稿账号（邮箱+密码+设备）、图片（多格式/1MB/30 天/2GB 兜底）、社区（文字永久）、举报、管理员、90 天审计 |
| 2026-11-28 | P1 施工（lc-2441）：verify 补 device 参数；部署沿用现有库 + migrate-v1-to-v2.sql；requireAuth 免 ctx |
| 2026-11-28 | P3 施工（lc-2445）：posts 加 author_nick 快照列；/v2/posts 幂等（clientRequestId）；requireAuth readOk 语义；hot=P3 置顶+新到旧；**limit 解析坑**：`Number(null)=0` 不是 NaN——「无 limit 参数」会走 isInteger 分支被 clamp 成 1（feed/mine/notifications 三处中招，照 v1 readLimit 口径修复） |
| 2026-11-28 | P2 施工（lc-2442）：schema 补 feedback_tickets/messages（§4.6 端点原本没有表）；§6.2.1 服务端图片三硬约束（R2 contentType / GIF 键后缀 / ImageDecoder 无 decode）；Cargo 勘误（webp/png 原已开启） |
