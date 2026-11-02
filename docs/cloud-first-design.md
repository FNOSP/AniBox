# 网盘优先（Cloud-first）改造设计

> 背景：产品方向从「以飞牛为主」转向「多来源接入」，网盘/主流云盘成为一等公民。
> 参考对象：LumiPlayer 的多云盘资源库逻辑（原生网盘直连 + 扫码 + 本地续签代理 + WebDAV/Emby/采集/STRM）。

---

## 〇、现状盘点（先说清楚哪些已经有了）

**传输层已经齐了**（lc-1810/1811 照 LumiPlayer 抄好，`cloud.rs` / `cloud_proxy.rs` / `webdav.rs` / `cloud115.rs`）：

| 能力 | 现状 |
|---|---|
| 网盘接入 | 夸克 / 百度 / 阿里 / 123（扫码，`cloud_qr_start/poll`）+ 115 + WebDAV（账号表单） |
| 目录浏览 | `cloud_list` 统一 cursor 分页（五家响应形状已实测修正） |
| 取直链 | 夸克 video_list m3u8 / download 302；百度 dlink；阿里 cdn_url；123 download_info |
| 播放 | `cloud_play` 三档：**直链（续签代理）** / **服务端转码 m3u8** / **本地 ffmpeg remux HLS** |
| 续签代理 | 稳定本地地址 + TTL 提前续 + 403 强制重签（`cloud_proxy.rs`） |
| 凭据 | 只落本机（`cloud115.json` / `direct-cloud-auth.json` / `webdav-accounts.json`），日志脱敏（feedback.rs redact） |

**真正的缺口是三个**：
1. **登录页仍以飞牛为中心**——网盘藏在侧栏「网盘接入」里，是个文件浏览器，不是登录入口；
2. **网盘媒体没有库化、没有刮削**——`cloud.rs` 里没有任何 TMDB/元数据逻辑，用户面对的是裸文件树；
3. **详情页 / 进度 / 观看记录全部绑死飞牛 guid**——网盘条目进不了「继续观看」和详情页。

所以这不是「从零接入网盘」，而是「把已有的网盘传输层接到产品骨架上」。

---

## 一、登录页重构：从「登录飞牛」变成「选择内容来源」

### 信息架构
- 页面语义改掉：标题不再是「登录飞牛影视」，而是「选择你的内容来源」。
- **三个来源并列，全部可选、可多选、可跳过**：
  1. **飞牛 NAS**——保留现有单框自适应（IP / 域名 / FN ID，lc-1884/1885 已做好），整卡折叠成一行摘要；
  2. **网盘账号**——夸克 / 百度 / 阿里 / 123 / 115，点开**内嵌二维码**（复用 `cloud_qr_start/poll`，不再跳独立面板），扫码成功即打勾；
  3. **WebDAV**——地址 + 账号 + 密码（复用现有连通性验证命令）。
- 底部主按钮改语义：「开始使用」。任一来源配好即可进；一个都没配 = 游客态（现有 guest 通道）。
- 「记住密码 / 自动登录」只属于飞牛卡；网盘凭据天然持久化，不需要这两个勾。

### 启动路由跟着改
- `check_session` 失败**不再无条件停登录页**：只要 `hasAnySource()`（config.json 有飞牛 token，或任一网盘/WebDAV 已配）就直接进首页，首页数据按可用来源聚合。
- 飞牛会话失效 + 有网盘来源 → 照常进首页，飞牛区显示「重新登录」入口（设置 → 账号，lc-1886 已有直达）。
- 完全没有来源 → 才落登录页。

### 实现边界
- 登录页只做**编排**（状态机 `sources: { fnos?, clouds[], webdav? }`），各来源的落盘沿用现有存储，不新造存储。
- `LoginPage.tsx` 拆成 `FnosCard` / `CloudCard` / `WebdavCard` 三个子组件；扫码轮询逻辑从 CloudPanel 抽成 hook 复用。

---

## 二、网盘媒体库化 + 刮削（最大的一块）

参考 LumiPlayer 的核心思想：**网盘不是文件树，是可刮削的资源库**。分两步走，别把「识别」和「刮削」混在一起。

### 1. 库模型（新增 `cloud_library.rs` + 前端聚合）
```
CloudLibrary {
  provider: String,          // quark/baidu/aliyun/tianyi/123/115/webdav
  roots: Vec<String>,        // 用户指定的媒体根目录（fid 或路径），每盘可多个
  items: Vec<CloudItem>,     // 扫描结果索引
}
CloudItem {
  provider, file_id, parent_dir,
  name, size, mtime,
  kind: Movie | Episode,     // 由文件名解析定
  parse: TitleParse,         // 标题/年份/SxxEyy/分辨率 的解析产物
  meta: Option<CloudMeta>,   // 刮削结果
}
```
- **扫描策略**：只扫用户指定的「媒体目录」，**懒扫描 + 分页 + 本地索引缓存 + 手动刷新**（对齐现有「库索引可刷新、失败保留旧索引」机制）。**严禁全量递归**——五家网盘都有 API 限额，全量扫必被封。
- 只收视频扩展名：mp4 / mkv / avi / rmvb / flv / ts / webm / m2ts / wmv / mov。
- 电视/电影目录约定：目录名即剧名（`剧名 (2024)` / `剧名 S01`），目录内文件按集解析；单文件按电影解析。

### 2. 识别（本地，零请求）
- Rust 侧正则解析：`标题 + (年份) + SxxEyy + 集名 + 分辨率 + 来源组`，兼容中文命名主流形态（`第x季` / `第xx集` / `EPxx`）。
- 归一化复用现有 `libmatch.rs` 的思路（异体字 / 全半角 / 季后缀），保证和飞牛库同套匹配口径。

### 3. 刮削（复用现有 tmdb.rs，零新增依赖）
- 剧集：`search/tv?query=标题` → 年份过滤取 best → 条目落 `{tmdb_id, season, episode}`；海报/简介走现有详情缓存。
- 电影：`search/movie` 同理。
- 结果落 `cloud_metadata.json`（`provider+file_id → tmdb 元数据`），**与索引分离**——重新扫描不丢刮削结果。
- 匹配失败项进 `unmatched` 队列，详情页/设置里提供「手动匹配」（复用 lc-1872 的手动匹配 UI 与 rescrap 交互模式）。
- **网盘不做飞牛侧 rescrap**：网盘条目的元数据只来自 TMDB + 本地解析，飞牛刮削接口（fnos-metadata）不适用。

### 4. 呈现
- 首页/媒体库新增「网盘库」分区，行结构对齐现有 lib rows；点开进**现有 DetailView**（数据源换成 cloud item + TMDB 元数据），海报 / 简介 / 选集 / 继续观看全部白嫖现有 UI。
- 来源角标：卡片角落标 provider 图标（夸克/阿里/…），用户能分清这条在哪个盘。

---

## 三、直链播放：传输层已齐，补「两端」

LumiPlayer 的播放三档已经完整移植（`cloud_play`）：
- **直链**：`cloud_proxy::register_direct` → 稳定本地地址，TTL 提前续签 + 403 强制重签（播放中途换链无感）；
- **服务端转码**：CDN m3u8 直连，长效不续签；
- **本地 remux**：`register_hls` → 本地 ffmpeg 转 HLS 喂 mpv。

要补的只有两端：
1. **详情页接入**：`DetailView.startPlay` 分流——fnOS 条目走现有逻辑；cloud 条目 `invoke("cloud_play", { provider, fileId, name, mode })`。画质档位映射：默认直链，转码不可用/清晰度不足时给「原画/转码」切换。
2. **进度与已看**：新增 cloud 进度键 `cloud:{provider}:{file_id}`，复用 mpv 退出回写 + `watched.rs` 落盘；「继续观看」聚合飞牛 + 网盘两个来源，条目带来源角标。

---

## 四、实施顺序（按改动量从小到大）

| 步骤 | 内容 | 量级 |
|---|---|---|
| 1 | 登录页重构（来源编排 + 内嵌扫码 + 路由放宽） | 1 轮 |
| 2 | `cloud_play` 接详情页 + cloud 进度键 + 继续观看聚合 | 1 轮 |
| 3 | `cloud_library.rs` 扫描 + 文件名解析 + 索引缓存 | 1~2 轮 |
| 4 | TMDB 刮削 + unmatched 手动匹配 + 网盘库分区 UI | 1~2 轮 |

## 五、风险与边界

- **API 限额**：扫描必须懒 + 分页 + 缓存，绝不全量递归；刷新给节流（同一目录 10 分钟内不重复拉）。
- **直链时效**：续签代理已兜底；前端与详情页**绝不缓存直链 URL**，每次播放现取。
- **风控**：部分网盘扫码入口会被服务端拦截（指纹风控）——登录页扫码失败要给明确文案与「改用 WebDAV/直连」降级路径，不要无限转圈。天翼（`22001 非正确来源`）与光鸭（`device/code` 404）已确认上游不可用，lc-1903 起整体移除。
- **凭据安全**：沿用「只落本机 + 日志脱敏」，新增的 `cloud_library.json` / `cloud_metadata.json` 不得含 token。
- **不做**：Emby/Jellyfin/Plex 接入、drpy 采集、STRM（LumiPlayer 有但本轮不做，接口留扩展位）。
