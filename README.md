# AniBox

**飞牛影视（fnOS）桌面客户端 · 自绘界面 + 直连 API + 独立 mpv 播放内核**

<div align="center">

![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)
![React](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black)
![Rust](https://img.shields.io/badge/Rust-2024-000000?logo=rust&logoColor=white)
![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-blue)
![License](https://img.shields.io/badge/license-GPL--3.0-green)

</div>

> **关于仓库名**：本仓库目录仍为 `Fntv-Plus-Tauri2`（历史遗留），**软件产品名统一为 AniBox**。
> 本文档中的「AniBox」均指本仓库构建的桌面客户端。

---

## 目录

- [1. AniBox 简介](#1-anibox-简介)
- [2. 功能一览](#2-功能一览)
- [3. 技术栈与架构](#3-技术栈与架构)
- [4. 项目结构](#4-项目结构)
- [5. 开发与构建](#5-开发与构建)
- [6. 配置说明](#6-配置说明)
- [7. 更新机制](#7-更新机制)
- [8. 版权与合规](#8-版权与合规)
- [9. 已知限制](#9-已知限制)
- [10. 反馈与贡献](#10-反馈与贡献)
- [11. 免责声明与隐私](#11-免责声明与隐私)
- [12. 致谢](#12-致谢)

---

## 1. AniBox 简介

AniBox 是一个面向 **飞牛影视（fnOS）** 的 Windows 桌面客户端。它不再把网页注入到浏览器里，
而是用 **React 自绘界面**直接调用飞牛影视 API，并用**独立 `mpv.exe` 进程**承担播放：

- **媒体库直连**：登录你的 NAS，浏览媒体库、继续观看、分类、收藏、详情与选集。
- **元数据聚合**：Bangumi 番组数据、TMDB 影视元数据、AniList 放送时刻，统一呈现在详情页与发现页。
- **原生播放体验**：mpv 独立窗口 + 命名管道 IPC，保留 uosc 底栏、右键菜单、字幕、着色器与弹幕脚本。
- **可选外部播放器**：可切换为本机安装的 PotPlayer，并通过本地 relay 处理 Referer/UA 与进度。
- **在线资源聚合**：按公开社区规则检索第三方站点，解析直链后交给 mpv / PotPlayer 播放。
- **长期可维护**：自建镜像、磁盘缓存、匿名统计、反馈工单、全量更新与前端热补丁。

> AniBox **提供在线资源聚合与播放能力，但不存储、不分发任何影视内容**。
> 库内内容来自你自己的 NAS；在线资源仅按你自行添加的公开规则聚合，
> 内容版权归原站 / 权利人所有。详见 [第 8 节](#8-版权与合规)。

---

## 2. 功能一览

### 2.1 登录与连接

- NAS 地址 + 用户名 + 密码登录，地址支持裸 IP、无协议头、全角冒号等宽容输入。
- 登录态（token）与可选「记住密码 / 自动登录」保存在本机配置中。
- 首次启动展示**免责声明**，同意后方可使用；设置 → 关于可再次查看。
- 数据目录自动迁移：旧 identifier `com.fntv.plus.tauri2` 的配置、缓存与 WebView 数据会迁移到 `com.anibox.app`。

### 2.2 首页

- 全屏 Hero 轮播（无限循环、首尾克隆位无缝衔接），支持影片透明 Logo 或文字标题。
- 「继续观看」横排、按媒体库横排、首页媒体库展示管理（可隐藏指定库）。
- 左侧栏：首页、收藏、全部 / 电影 / 电视节目 / 电视直播 / 其他分类；底部为用户信息与快捷操作卡。

### 2.3 发现

- **TMDB 面板**：热门 / 发现 / 搜索，条目可直接进入详情。
- **Bangumi 面板**：每日放送日历、当季番表、往季番表查询、番组详情、评论 / 吐槽 / 关联 / 角色。
- **AniList 补充**：按天获取番剧的精确放送时刻（BGM 只有星期与首播日）。
- **Bangumi 账号**：手动粘贴 Access Token 绑定，支持追番收藏状态同步与冲突策略。

### 2.4 媒体库与收藏

- 媒体库网格、分类浏览、文件夹层级、虚拟「收藏」入口。
- 海报卡支持「已看」切换（写回飞牛，接口不可用时降级本地标记）与「编辑元数据」。
- 本地收藏：飞牛收藏接口不可用时由客户端落盘，列表与飞牛收藏合并展示。

### 2.5 详情页

- 两栏沉浸式布局：左侧选集，右侧详情；全屏背景海报。
- 标签页：**详情 / 评论 / 吐槽 / 关联 / 角色 / 更多**（无 Bangumi 条目 id 时隐藏评论与吐槽）。
- 演职人员、角色、关联条目、TMDB 剧照与预告、剧照灯箱。
- 我的评分 / 评语、Bangumi 评分组件、简介展开、元数据编辑。

### 2.6 观影记录

- 数据 = 飞牛 `play/list` ∪ 本机播放历史，Rust 侧合并去重并按「作品」聚合。
- 已看完 / 在观看双栏海报墙、按年 / 季 / 月 / 周切换的活跃度热力图。
- 年度观影报告：总时长、部数、月度节奏、观影时刻、年度 TOP5、连续天数、看完率，可导出 PNG 长图。

### 2.7 在线资源

- 兼容公开社区规则 JSON：搜索、条目、线路、选集、直链解析。
- 双通道解析：先走纯 HTTP 正则；未命中时用隐藏 WebView 注入钩子嗅探 `m3u8 / mp4`。
- 多源并发搜索、源启用 / 停用、规则新增 / 删除、在线规则目录安装与更新。
- 反爬验证：图片验证码、自动点击、规则自定义脚本三种模式。
- 解析出的直链交给 mpv（`referrer` / `user-agent`）或 PotPlayer（本地 relay）。

### 2.8 播放与弹幕

- **mpv（默认）**：独立进程 + 命名管道 IPC，uosc 底栏、右键菜单、字幕、着色器原生可用。
- **PotPlayer（可选）**：命令行起播 + `potctl.exe` 轮询进度；Referer/UA 由本地 relay 代转。
- **弹幕**：本地 `127.0.0.1:22347` shim 为 uosc_danmaku 提供 B 站弹幕接口；
  可选自建 `danmu_api` 作为优选源（最多 4 个地址故障转移），未命中自动降级内置 B 站。
- **播放历史**：起播即记录到本机 `history.json`，与飞牛记录合并展示。

### 2.9 手柄

- 原生全局手柄监听（gilrs / XInput），mpv 或 PotPlayer 在前台时同样可用。
- Xbox 布局标准映射：方向键焦点导航、A 确认 / 暂停、B 返回 / 停止、LB/RB 切页或 ±5s、
  LT/RT 调倍速、X 收藏、Start 打开设置。
- 可调摇杆死区、方向阈值、长按首次延迟、连跳间隔。

### 2.10 更新与维护

- **热补丁**：只覆盖前端静态资源，下载 → sha256 校验 → 原子切换 → 可回滚。
- **全量更新**：后台下载安装包（Range 续传），完成后拉起 NSIS 安装器，不跳转 GitHub。
- 设置面板内置检查更新、更新说明、下载进度、补丁应用 / 回滚。

### 2.11 反馈与诊断

- 应用内工单：新建反馈、我的工单、会话回复、20 秒轮询状态。
- 日志与诊断：诊断日志、mpv 日志、调试日志开关、组件级过滤、诊断信息导出。
- 反馈内容与日志在提交前做脱敏处理。

---

## 3. 技术栈与架构

### 3.1 总览

| 层 | 技术 / 实现 |
| --- | --- |
| 桌面壳 | **Tauri 2**（Rust 2024 edition） |
| 前端 | **React 19 + TypeScript + Vite 8**，Lucide 图标 |
| 样式 | 原生 CSS 分层（`src/styles/*.css`），浅色 / 深色主题 |
| 播放内核 | **独立 `mpv.exe` 进程** + `--input-ipc-server` 命名管道（JSON 行协议） |
| 播放器 UI | mpv 便携配置中的 **uosc** 与弹幕脚本 |
| 外部播放器 | 本机 **PotPlayer**（可选）+ 随包 `potctl.exe` 进度读取 |
| 本地代理 | **Go `proxy.exe`**（`127.0.0.1:22346`），负责 NAS 拉流中转 |
| 弹幕 shim | 系统 Node 运行 `danmaku-shim.cjs`（`127.0.0.1:22347`） |
| 图片协议 | 自定义 `fntvimg` 协议（NAS / TMDB / Bangumi 图片代理 + 缓存） |
| 热补丁协议 | 自定义 `fntvweb` 协议（前端资源覆盖层） |
| 元数据 | Bangumi 自建镜像、TMDB 自建镜像、AniList 官方 GraphQL |
| 在线资源 | 公开社区规则 JSON + HTTP / 隐藏 WebView 嗅探 |
| 手柄 | Rust **gilrs** 原生监听，事件推送到前端 |
| 缓存 | 内存 → 磁盘 → 网络三级缓存，磁盘 LRU 淘汰 |

### 3.2 运行时拓扑

```text
┌──────────────────────────────────────────────┐
│ WebView2                                     │
│ React 19 + TypeScript                        │
│ 首页 / 发现 / 媒体库 / 详情 / 观影记录 / ... │
└───────────────┬──────────────────────────────┘
                │ Tauri IPC（invoke / event）
┌───────────────▼──────────────────────────────┐
│ Rust 核心（Tauri 2）                          │
│ config · fnapi/authx · mpv · player · update │
│ bgm · tmdb · webrs · history · feedback ...  │
└───┬───────────┬───────────┬───────────┬──────┘
    │           │           │           │
 mpv.exe     proxy.exe    node shim   WebView 嗅探
 命名管道     :22346       :22347      :22348+
    │
 PotPlayer（可选）+ potctl.exe；需要请求头时走 relay :22349
```

### 3.3 mpv 播放架构

- **独立进程，不进程内嵌**：`mpv.exe` 自带消息循环，鼠标 / 键盘 / uosc / 右键菜单 /
  缩略图 / 弹幕 Lua 脚本全部原生可用。
- **控制与事件**：`--input-ipc-server=\\.\pipe\anibox-mpv-<pid>`，前端仍接收
  `mpv-event { event: "property-change", id, data }`。
- **会话复用**：同一 mpv 进程内 `loadfile replace` 切换剧集，避免反复启动。
- **请求头**：在线源通过 `referrer` / `user-agent` / `http-header-fields` 传入；库内播放会清空残留头。
- **便携配置**：uosc、input.conf、脚本、着色器来自 `mpv.exe` 同目录的 `portable_config`，
  由用户自行提供（本仓库不分发 mpv 二进制）。

### 3.4 自建镜像与降级

客户端默认优先访问自建镜像（可用环境变量覆盖）：

| 用途 | 默认端点（代码内） | 说明 |
| --- | --- | --- |
| Bangumi API / 图片 | `bgm.690075.xyz` | 带准入签名；缺密钥时降级直连官方域 |
| TMDB API / 图片 | `tmdb.690075.xyz` | 镜像注入服务端 key；缺密钥时降级用户自己的 key |
| GitHub 下载 | `gh.690075.xyz` | 更新包解析 `/app/resolve` |
| 匿名统计 / 反馈 | `stats.690075.xyz` | `/ping`、`/feedback` 等 |

- 镜像准入签名：`x-fntv-app` + `x-fntv-ts` + `x-fntv-sign`，密钥**只从运行时环境或编译期
  `option_env!` 读取，绝不写入仓库**。
- 缺密钥 / 网络错误 / 超时 / 5xx / 429 会按模块既定策略降级；401 与其他 4xx 属确定性拒绝，不降级。
- AniList 官方 GraphQL 直连，按天缓存，遵守其非商业与限流条款。

### 3.5 磁盘缓存

- 位置：`<app_data_dir>/cache/`（`img/` 与 `json/` 两池）。
- 行为：内存命中 → 磁盘命中 → 网络；命中磁盘不回源。
- LRU：超过 `cache_limit_mb` 后按最后访问时间淘汰到上限的 90%，内存与磁盘同步清理。
- 设置面板可查看占用、修改上限（1/2/5/10 GB 或不限）、立即清理。

---

## 4. 项目结构

```text
Fntv-Plus-Tauri2/                  # 仓库目录名（产品名 AniBox）
├── src/                           # 前端（React + TypeScript）
│   ├── App.tsx                    # 应用外壳：视图路由、登录态、主题、全局事件
│   ├── main.tsx                   # 入口：主题预置、挂载
│   ├── types.ts                   # 前后端共享类型
│   ├── icons.tsx                  # 图标封装（Lucide）
│   ├── components/
│   │   ├── home/                  # 首页：Hero 轮播、继续观看、媒体库横排
│   │   ├── discover/              # 发现：TMDB / Bangumi / 番表 / 账号
│   │   ├── library/               # 媒体库网格与分类
│   │   ├── detail/                # 详情页、标签页、评分、元数据编辑、剧照灯箱
│   │   ├── history/               # 观影记录与年度报告
│   │   ├── webrs/                 # 在线资源面板（搜索 / 选集 / 规则管理）
│   │   ├── feedback/              # 反馈工单与日志诊断
│   │   ├── settings/              # 设置面板
│   │   ├── sidebar/ shell/        # 侧栏、标题栏、导航胶囊
│   │   ├── login/                 # 登录页
│   │   ├── disclaimer/            # 首次启动免责声明
│   │   └── common/                # 海报卡、滚动行、头像兜底等
│   ├── hooks/                     # useGamepad、useLibIndex、useRefresh 等
│   ├── lib/                       # api、焦点导航、图片、格式化、主题等
│   ├── styles/                    # 分层 CSS（tokens、shell、各页面）
│   └── assets/                    # 内置字体与静态资源
├── src-tauri/                     # Rust 桌面端
│   ├── src/
│   │   ├── lib.rs                 # Tauri 命令注册与应用启动
│   │   ├── main.rs                # 二进制入口
│   │   ├── config.rs              # config.json 读写与迁移兼容
│   │   ├── fnapi.rs / authx.rs    # 飞牛 API 客户端与 Authx 签名
│   │   ├── mpv.rs                 # mpv 进程、命名管道 IPC、事件
│   │   ├── player.rs              # mpv / PotPlayer 统一启动 + 本地 relay
│   │   ├── proxy.rs               # Go 代理守护（22346）
│   │   ├── danmaku_shim.rs        # 弹幕 shim（22347）
│   │   ├── webrs.rs               # 在线资源规则、解析、嗅探、规则目录
│   │   ├── bgm.rs / bgm_account.rs# Bangumi 数据与账号同步
│   │   ├── tmdb.rs / anilist.rs   # TMDB 元数据 / AniList 放送时刻
│   │   ├── mirror_sign.rs         # 镜像准入签名（BGM / TMDB 共用）
│   │   ├── imgproto.rs            # fntvimg 图片协议
│   │   ├── disk_cache.rs          # 三级缓存与 LRU
│   │   ├── history.rs             # 本机播放历史、合并、年度报告数据
│   │   ├── watched.rs / favorites.rs
│   │   ├── feedback.rs            # 反馈工单、日志与诊断
│   │   ├── usage_stats.rs         # 匿名统计
│   │   ├── update.rs              # 全量更新 + 热补丁 + fntvweb 协议
│   │   ├── migrate.rs             # 数据目录迁移
│   │   └── libmpv.rs              # 诊断日志与崩溃记录
│   ├── assets/                    # 弹幕 shim 的 Node 源码（include_str!）
│   ├── resources/potctl.exe       # PotPlayer 进度读取工具（随包）
│   ├── capabilities/              # Tauri 权限声明
│   ├── icons/                     # 应用图标
│   ├── Cargo.toml                 # Rust 依赖与 release 配置
│   └── tauri.conf.json            # 窗口、打包、资源与 identifier
├── public/                        # Vite 公共静态资源
├── dev.cmd                        # 开发启动脚本（清残留 → tauri dev）
├── index.html                     # 前端入口 HTML
├── package.json                   # 前端依赖与脚本
├── vite.config.ts                 # Vite 配置（端口 1420）
├── tsconfig*.json                 # TypeScript 配置
└── LICENSE                        # GNU GPL-3.0
```

> 说明：`mpv.exe`、`proxy.exe` 与 `portable_config` **不在本仓库内**，由用户自行提供并通过配置指定路径。
> `release.cmd` 等发布 / 运维脚本按仓库规则不入库（本地脚本）。

---

## 5. 开发与构建

### 5.1 环境要求

| 依赖 | 版本 / 说明 |
| --- | --- |
| 操作系统 | Windows 10 / 11 x64（当前代码大量使用 Win32 API） |
| Node.js | 18+（开发与弹幕 shim 运行时都需要） |
| Rust | stable + `cargo`（edition 2024） |
| MSVC | Visual Studio Build Tools（C++ 桌面开发） |
| WebView2 | Windows 10/11 通常自带；缺失需安装 Evergreen Runtime |
| mpv | 独立 `mpv.exe` + `portable_config`（uosc、弹幕脚本、着色器） |
| Go 代理 | 配套 `proxy.exe`（监听 `127.0.0.1:22346`） |
| PotPlayer | 可选，本机安装；仅调用，不随包分发 |

### 5.2 常用命令

```bash
# 安装前端依赖
npm install

# 开发模式（Vite + Tauri，首次编译较慢）
npm run tauri dev
# 或使用仓库脚本（会先清理残留 anibox.exe / proxy.exe）
dev.cmd

# 仅构建前端（tsc + vite build）
npm run build

# 构建安装包（NSIS）
npm run tauri build

# Rust 单元测试
cargo test --manifest-path src-tauri/Cargo.toml
```

> **提示**：`tauri dev` 默认使用 `http://localhost:1420`。端口被占用时 Windows 可能返回
> `EACCES` 而非 `EADDRINUSE`，`dev.cmd` 已包含清理逻辑。

### 5.3 外部资源准备

1. 准备 `mpv.exe` 及其 `portable_config`（含 uosc、`uosc_danmaku` 等）。
2. 准备 Go 代理 `proxy.exe`。
3. 在登录页「高级设置」或 `config.json` 中填写 `mpv_path` 与 `proxy_path`。
4. 若需要镜像（BGM / TMDB），在启动进程的环境中提供准入密钥（见下节）；**不要写入仓库**。

### 5.4 环境变量

| 变量 | 作用 |
| --- | --- |
| `FNTV_BGM_SECRET` | Bangumi 镜像准入密钥；缺失时降级直连官方域 |
| `FNTV_TMDB_SECRET` | TMDB 镜像准入密钥；缺失时降级用户自己的 key |
| `FNTV_BGM_ENDPOINT` | Bangumi 端点列表（逗号分隔） |
| `FNTV_TMDB_ENDPOINT` | TMDB 端点列表（逗号分隔） |
| `FNTV_TMDB_KEY` | 用户自己的 TMDB API Key（镜像不可用时直连） |
| `FNTV_STATS_ENDPOINT` | 匿名统计端点（逗号分隔） |
| `FNTV_STATS_FORCE` | 开发模式下强制上报（`1`） |
| `FNTV_UPDATE_MANIFEST` | 更新清单地址覆盖 |
| `FNTV_GH_MIRROR` | GitHub 下载镜像覆盖 |
| `FNTV_DEVTOOLS` | `1` 时开发模式自动打开 DevTools |
| `FNTV_NO_ACRYLIC` | `1` 时关闭亚克力底板 |
| `FNTV_AUTOPLAY_TEST` | 开发期无人值守自动播放（传条目 guid） |
| `FNTV_EVAL_JS` | 开发期启动后在 WebView 执行 JS |

> 密钥只在本机进程环境或编译环境中存在，仓库与构建产物均不包含明文密钥文件。

---

## 6. 配置说明

配置落盘位置：

```text
%APPDATA%\com.anibox.app\config.json
```

缓存位于同级的 `cache/`，播放历史为 `history.json`，收藏为 `favorites.json`，
已看降级标记为 `watched.json`，评分与评语为 `ratings.json`。

| 字段 | 默认值 | 用途 |
| --- | --- | --- |
| `server` | 空 | NAS 地址，如 `http://192.168.31.170:18888` |
| `username` / `token` | 空 | 登录账号与会话 token |
| `password` | 空 | 勾选「记住密码」时保存（**本机明文，勿分享该文件**） |
| `auto_login` | `false` | 启动时用记住的密码静默重登 |
| `mpv_path` | 开发机示例路径 | `mpv.exe` 完整路径 |
| `player_kind` | `"mpv"` | `mpv`（默认）或 `potplayer` |
| `potplayer_path` | 空 | PotPlayer 路径；空 = 起播时自动探测 |
| `proxy_path` | 开发机示例路径 | Go 代理 `proxy.exe` 路径 |
| `nas_proxy_enabled` | `false` | 是否走 NAS 代理播放链路 |
| `cache_limit_mb` | `2048` | 磁盘缓存上限（MB）；`0` = 不限 |
| `exit_mode` | `"ask"` | 关闭按钮行为：`exit` / `tray` / `ask` |
| `perf_mode` | `false` | 性能模式：关闭动画与磨砂 |
| `carousel_logo` | `true` | 首页轮播优先使用影片透明 Logo |
| `launch_page` | `"home"` | 登录后进入 `home` 或 `discover` |
| `bgm_token` | 空 | Bangumi 个人 Access Token |
| `bgm_remember` | `true` | 是否把 `bgm_token` 落盘 |
| `bgm_username` | 空 | 最近校验成功的 Bangumi 用户名（缓存） |
| `danmu_api_enabled` | `false` | 是否启用自建 `danmu_api` 弹幕源 |
| `danmu_api_base` / `danmu_api_bases` | 空 | 自建弹幕地址（单地址 / 多地址，最多 4 条） |
| `feedback_aid` | 空 | 反馈工单匿名设备 ID（首次提交时生成） |
| `log_debug` | `true` | 调试日志总开关（WARN/ERROR 始终写入独立错误日志） |
| `log_modules` | 空 | 组件级日志开关；空 = 全开 |
| `usage_stats_enabled` | `true` | 匿名使用统计开关 |
| `usage_stats_anon_id` | 空 | 机器标识 SHA-256 前 31 位（不可逆）或随机 hex |
| `usage_stats_last_ping_day` / `_ok` / `_pending_days` | 空 / `false` / 空 | 上报状态与欠报日期 |
| `gamepad_enabled` | `false` | 手柄总开关 |
| `gamepad_deadzone` | `0.30` | 左摇杆死区（0.10–0.50） |
| `gamepad_direction_threshold` | `0.40` | 方向触发阈值（0.20–0.80） |
| `gamepad_repeat_delay` | `320` | 长按首次重复延迟（ms） |
| `gamepad_repeat_interval` | `130` | 长按重复间隔（ms） |

> 所有新增字段均带 `serde(default)`，旧配置文件无需迁移脚本。
> 设置面板只读写白名单子集，不会暴露密码 / token。
> **敏感信息（密码、token、镜像密钥）不应提交到仓库，也不应随日志分享。**

---

## 7. 更新机制

AniBox 使用「**热补丁优先、全量兜底**」的双通道更新：

### 7.1 更新清单

- 默认读取公开更新清单（`update-check.json`），可用 `FNTV_UPDATE_MANIFEST` 覆盖。
- 清单包含版本、日期、更新说明、全量包地址、热补丁地址与项目页地址。

### 7.2 热补丁（前端资源）

1. 拉取补丁清单与文件列表。
2. 下载补丁，逐文件 **sha256 校验**。
3. 写入暂存目录，校验通过后**原子切换**到 `app_data_dir/web/<version>/`。
4. 记录 `active.json`（含 `previous` 指针），启动时切换到 `fntvweb` 自定义协议读取覆盖层。
5. 失败时保留原覆盖层；可一键回滚到上一版本。
6. 覆盖层缺失或未打补丁时，自动回退内置 `tauri://` 资源。

> 热补丁**只覆盖前端静态资源**。Rust、插件、原生依赖的改动必须发布全量包。
> 开发模式（Vite）下覆盖层不生效。

### 7.3 全量更新

1. 通过自建 GitHub 镜像解析安装包（失败回退清单中的下载地址）。
2. 流式下载，支持 Range 断点续传与进度事件。
3. 下载完成后**直接拉起 NSIS 安装器**（GUI 安装），不跳转 GitHub。
4. 用户可在设置面板取消下载、查看进度、立即安装。

---

## 8. 版权与合规

> 本节是发布与二次分发前必须保留的内容。若你修改并分发 AniBox，请同时保留本节与 `LICENSE`。

### 8.1 开源许可证（GPL-3.0）

- AniBox 基于 **GNU General Public License v3.0（GPL-3.0）** 开源，完整条款见仓库根目录 [`LICENSE`](LICENSE)。
- 你可以运行、研究、修改和再分发本软件；**衍生分发必须以 GPL-3.0 开放对应源代码**，
  并保留原始版权与许可声明。
- 本软件按「现状」提供，**不提供任何明示或默示担保**。

### 8.2 TMDB

本产品使用 TMDB API，但**未获得 TMDB 的认可或认证**。分发与界面中必须保留以下署名：

> **This product uses the TMDB API but is not endorsed or certified by TMDB.**
>
> 数据与图片来自 [The Movie Database (TMDB)](https://www.themoviedb.org/)。

### 8.3 Bangumi

- 番组条目、剧集、角色、评论、吐槽等数据与部分内容来自
  [Bangumi 番组计划](https://bgm.tv/)。
- 分发时应保留「数据与部分内容来自 Bangumi 番组计划」的署名，并遵守其 API 使用条款。
- AniList 数据来自 [AniList](https://anilist.co/) 官方 GraphQL API，遵守其非商业与限流条款。

### 8.4 在线资源规则

- 在线资源能力依赖**公开社区规则仓库**的规则 JSON，规则版权归各规则作者 / 仓库所有。
- 本仓库提供规则解析与播放客户端能力；**不内置、不分发、不托管任何影视内容**。
- 第三方站点的内容、可用性与合法性由该站点负责；规则失效、站点反爬或验证码属于正常现象。
- 你应仅将在线资源功能用于访问你有权访问的内容，并遵守当地法律与目标站点条款。

### 8.5 弹幕数据（弹弹play）

- 弹幕数据来自 **弹弹play 开放平台（Dandanplay）**，本项目使用其开放 API 做番剧匹配与弹幕获取。
- 使用时应遵守弹弹play 开放平台的 API 条款与署名要求；弹幕内容版权归原平台及原作者所有。
- 内置 B 站弹幕链路与自建 `danmu_api` 仅作为可选来源，不改变上述归属。

### 8.6 mpv / uosc / 字体 / 图标

- **mpv** 是独立第三方播放器，遵循其自身许可（GPL/LGPL，详见 mpv 项目）。
- **uosc** 与弹幕脚本遵循其各自仓库许可；本仓库不分发其二进制。
- **PotPlayer** 为第三方专有软件，本项目**不随包分发**，仅调用用户本机安装的版本。
- 内置字体使用 **SIL Open Font License** 的开源字体（如 Baloo 2）或系统字体；
  **不内嵌任何商业字体**。
- 图标使用 [Lucide](https://lucide.dev)（**ISC 许可**）。

### 8.7 内容与版权边界

- AniBox 提供在线资源聚合与播放能力，但**不存储、不分发任何影视内容**；
  内容版权归原站 / 权利人所有。
- 媒体库内容由用户自己的 NAS 或合法来源提供；在线资源仅聚合公开规则。
- 请勿使用本软件侵犯任何第三方的知识产权。

### 8.8 密钥与隐私

- 仓库**不包含任何密钥、token、`.env` 或镜像准入密钥**；镜像密钥只从运行时环境或编译期读取。
- 匿名统计仅上报：匿名 ID、版本号、系统类型、CPU 架构、日期；**不含账号、IP、媒体库、
  文件路径或播放内容**。可在设置中随时关闭。
- 反馈内容与日志提交前脱敏；反馈工单只用于问题跟踪。
- 本机 `config.json` 可能包含密码 / token，请勿提交、上传或分享该文件。

---

## 9. 已知限制

1. **平台**：当前主要面向 Windows 10/11 x64；代码含大量 Win32 调用，其他平台未验证。
2. **PotPlayer 模式**：不支持内置弹幕、uosc 底栏与精确暂停 / 跳转 / 倍速控制；
   需要 Referer / UA 的 HLS 源依赖本地 relay，仍可能受目标站限制。
3. **热补丁边界**：只能覆盖前端静态资源；Rust / 原生依赖改动必须全量更新；
   dev（Vite）模式下覆盖层不生效。
4. **镜像依赖**：缺少镜像密钥时会降级（Bangumi 直连官方域、TMDB 使用用户自己的 key）；
   无 key 时 TMDB 相关功能可能不可用。
5. **在线资源**：依赖第三方站点结构，规则可能失效；验证码、反爬、限流会导致搜索或解析失败。
6. **AniList**：直连官方 API，受其限流约束，仅按天缓存，失败时前端静默隐藏放送时刻。
7. **手柄**：采用 Xbox 布局标准映射；窗口失焦时浏览器 / WebView 会暂停手柄输入，点回窗口恢复；
   `Y` 键「下一集」当前为预留事件，后端尚未提供命令。
8. **登录方式**：当前为 NAS 地址 + 用户名 / 密码；FN Connect / OAuth 远程登录尚未实现（TODO）。
9. **播放进度**：客户端记录本机观看历史；「已看」可写回飞牛，接口不可用时降级为本地标记。
   未实现向飞牛实时回写播放进度（TODO）。
10. **弹幕**：内置链路依赖 mpv 便携配置中的弹幕脚本与本地 Node shim；脚本缺失时弹幕不可用。

---

## 10. 反馈与贡献

### 10.1 应用内反馈

- 侧栏「反馈」入口：新建工单、查看我的工单、会话回复。
- 「日志与诊断」页可查看 / 导出诊断日志、mpv 日志、调试日志开关与系统信息。
- 提交前请尽量附上：问题现象、复现步骤、版本号、诊断日志最后一段。

### 10.2 Issue 约定

提交 Issue 时建议包含：

- AniBox 版本、Windows 版本、mpv / PotPlayer 版本；
- 问题分类（播放 / 元数据 / 在线资源 / 更新 / 界面 / 其他）；
- 复现步骤与预期 / 实际结果；
- 相关日志（**请先移除账号、token、路径等敏感信息**）。

### 10.3 交流群与项目地址

- 项目地址：**TODO**（代码中暂有两处占位：
  `https://github.com/AniBox/AniBox` 与 `https://github.com/YDMY007/Fntv-Plus`，
  待正式仓库定名后统一替换）。
- 交流群：**TODO**（尚未确定，请勿臆造群号）。

### 10.4 贡献

- 欢迎提交 Issue 与 Pull Request；提交前请确保 `cargo test` 与 `npm run build` 通过。
- 请勿在 PR 中提交密钥、部署脚本、验证脚本、本地状态或个人数据（详见仓库 `AGENTS.md`）。
- 一个改动一个 commit，保持提交信息清晰。

---

## 11. 免责声明与隐私

### 11.1 免责声明（摘要）

> 完整条款见应用内「首次启动弹窗」与「设置 → 关于 → 免责声明」，同意后方可使用。

- 本软件为开源软件（GPL-3.0），应免费获取；若从第三方付费获取，建议向对方索赔。
- 本软件仅供个人学习、交流等非商业用途；严禁用于商业目的。
- 本软件提供在线资源聚合与播放能力，但不存储、不分发任何影视资源；在线资源来自你自行添加的第三方规则。
- 使用本软件产生的一切后果由使用者自行承担；作者与贡献者不承担任何直接或间接损失。
- 使用者应遵守所在地法律法规与内容平台条款，不得利用本软件从事违法违规行为。
- 如本软件侵犯了你的合法权益，请联系作者，我们会及时处理。

### 11.2 隐私政策

- **匿名统计（默认开启，可关闭）**：仅上报匿名 ID、版本号、系统类型、CPU 架构、日期；
  不含账号、IP、媒体库、文件路径或播放内容；每天最多一次，失败静默补报。
- **反馈**：仅在你主动提交时发送反馈内容、可选联系方式、脱敏日志与匿名设备 ID。
- **本地数据**：配置、缓存、播放历史、收藏、评分均保存在本机应用数据目录，不上传。
- **无第三方遥测 SDK**：除上述自建端点外，不接入任何分析 / 广告 SDK。

---

## 12. 致谢

本项目实际使用或依赖以下第三方组件，各自遵循其自身许可：

- [mpv](https://github.com/mpv-player/mpv) —— 独立播放内核（GPL/LGPL，详见其项目）。
- [uosc](https://github.com/tomasklaen/uosc) —— mpv 现代化界面框架（遵循其仓库许可）。
- [uosc_danmaku](https://github.com/Tony15246/uosc_danmaku) —— 基于 uosc 的弹幕脚本（遵循其仓库许可）。
- [弹弹play 开放平台（Dandanplay）](https://www.dandanplay.com/) —— 弹幕数据与番剧匹配 API
  （遵循其开放 API 条款与署名要求）。
- [Lucide](https://lucide.dev) —— 图标（ISC）。
- [Baloo 2](https://fonts.google.com/specimen/Baloo+2) —— 内置开源字体（SIL Open Font License）。
- [Tauri](https://tauri.app/) / [React](https://react.dev/) / [Vite](https://vite.dev/) —— 应用框架与构建工具。
- [The Movie Database (TMDB)](https://www.themoviedb.org/) —— 影视元数据与图片。
- [Bangumi 番组计划](https://bgm.tv/) —— 番组条目、剧集、角色、评论与吐槽数据。
- [AniList](https://anilist.co/) —— 放送时刻数据。

> 在线资源规则来自公开社区规则仓库，版权归各规则作者所有；本项目仅提供解析与播放能力。

---

<p align="center">
  <sub>AniBox · 基于 GPL-3.0 开源 · 仅供个人学习与自用</sub>
</p>
