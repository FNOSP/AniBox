# FlyNarwhal 调研与 MPV 内嵌可行性评估

> 2026-10-11 · 调研对象 [FNOSP/FlyNarwhal](https://github.com/FNOSP/FlyNarwhal)（AGPL-3.0，511★，v2.3.7，已克隆源码通读）
> 回答两个问题：① 它是不是"逆向接口自建桌面端"？② 我们下一步把 MPV 直接内嵌进 App 是否可行、怎么做。

**结论先行：**

1. **是的**，FlyNarwhal 是完全绕开飞牛网页、直调 `/v/api/v1/*` REST 接口的自建原生客户端（Flutter + 内嵌 libmpv），方案已被 500+ 用户验证可行。
2. **MPV 内嵌可行，且有两条完全不同的路线**：在我们现有 Electron 架构下，推荐 **`--wid` 子窗口嵌入**（改动小、现有弹幕/脚本资产零损失）；真·纹理级内嵌（libmpv render API）在 Electron 下**不推荐**，那是 Flutter media_kit 才有的便利。
3. 但要明确：**内嵌播放器解决不了"网页注入"的痛点本身**。真正的出路分两步走（见第五节）：先内嵌播放窗口，再用已验证的 API 面逐步自建浏览 UI，网页壳最后退役。
4. 许可注意：他们 **AGPL-3.0**，我们 **GPL-3.0**。端点、签名算法、参数这些"事实"可以自由参考（我们本来就独立逆向出了同一套），**不能直接拷他们的 Dart 代码**进我们仓库。

---

## 一、FlyNarwhal 是怎么做的

**一句话：逆向飞牛影视 API → Flutter 自建全部 UI → media_kit（libmpv）进程内嵌播放 → 无任何网页依赖。**

| 维度 | 实现 |
| --- | --- |
| 技术栈 | Flutter/Dart 桌面端（Win/macOS/Linux），Riverpod + go_router + Dio + fluent_ui，约 8.5 万行 Dart |
| 播放内核 | `media_kit: ^1.1.10` + `media_kit_video` + **全量 libmpv**，GPU 硬解、PGS/SUP 位图字幕、HDR/DV（DV profile 5 强制走转码） |
| UI 来源 | 全部自绘：登录、媒体库、首页、详情、演员、搜索、设置、播放器，无一处网页 |
| 配套服务端 | 另有自建 `fly-narwhal-server`（"飞鲸"），提供弹幕聚合、片头片尾智能分析、更新分发 —— **这些能力飞牛官方没有** |
| 许可 | AGPL-3.0，代码公开 |

### 1.1 API 面（与我们的重合度）

端点常量在 `lib/core/constants/app_constants.dart:43-132`，base 为 `http(s)://NAS/v/api/v1/*`。核心端点：

| 端点 | 用途 | 我们是否已逆向 |
| --- | --- | --- |
| `POST /login`、`/auth`、`GET /user/info` | 登录/OAuth 换 token/用户信息 | ✅ `fn_api/api.ts:143` 等 |
| `POST /play/info`、`GET /play/quality`、`POST /stream` | 播放信息/清晰度/轨道 | ✅ `api.ts:191,200,552` |
| `GET /stream/list/{guid}`、`GET /episode/list/{guid}` | 历史 / 剧集 | ✅ `api.ts:212,229` |
| `POST /item/list`、`POST /item/watched`、`POST /play/record` | 列表/已看/进度上报 | ✅ `api.ts:238,521,532` |
| `GET /subtitle/dl/{guid}` | 字幕内容 | ✅ `api.ts:450` |
| `POST /play/play` | **建转码会话拿 playLink（HLS）** | ❌（我们靠 Go 代理铸直链，没做转码） |
| `GET /media/range/{mediaGuid}` | NAS 代理直链（标准 Range） | ❌（我们的 Go 代理是等价物） |
| `GET /mediadb/list`、`GET /play/list`、`GET /search/list` | 媒体库/继续观看/搜索 | ❌（自建 UI 时要补） |
| `GET /season/list`、`/person/*`、`/tag/*`、`/favorite/list` | 剧/演员/标签/收藏 | 部分（season 走 test-season-* 脚本验证过） |
| `/subtitle/search`、`/upload`、`/mark` 等在线字幕全套 | 在线字幕站 | ❌（我们有 NAS 外挂字幕挂载，无在线搜索） |
| `POST /user/getData`、`/setData` | 用户偏好云同步 | ❌ |

**粗估：核心播放链路我们已覆盖 80%+，缺的是"浏览类"端点（库/搜索/收藏/人物/标签）和转码会话，均为常规 CRUD 型接口，难度低，纯工作量。**

### 1.2 认证（与我们互相印证）

- 本地登录 `POST /v/api/v1/login` 明文 {username, password} → token → **双通道**：`Authorization` 头 + `Cookie: Trim-MC-token`（`auth_interceptor.dart:37-46`）——与 `src/modules/fn_api/request.ts` 的 lc-294 方案完全一致。
- **Authx 签名**：apiKey/apiSecret 与我们 `request.ts:46-47` 内置的**完全相同**（都是飞牛网页版内置密钥），算法同款：`md5(apiKey_url_nonce_时间戳_dataMd5_apiSecret)`。两家独立逆向得到同一结论，说明这套接口逆向没有暗坑。
- FN ID：弹 WebView 加载官方登录页 → 注入 JS 自动填表/点授权 → hook fetch 拦 OAuth code → `POST /auth` 换 token。我们的 `fnid_login.ts` 同思路。
- 中转模式（5ddd.com/fnos.net）Cookie 追加 `mode=relay`；自签 HTTPS 按 host+指纹信任，mpv 侧对已信任 host 直接 `tls-verify=no`。

### 1.3 播放链路（三条路 + 回退）

```
play/info → stream（拿 qualities + direct_link_qualities）
  ├─ 本地文件 + 选了原画 → 直链：mpv 直连（本地路径/NAS /media/range/网盘 CDN url）
  ├─ 网盘(阿里/123/STRM) → 原始 CDN 直链（mpv 带 Referer/header）
  └─ 其他 → POST /play/play 建转码会话 → HLS m3u8（/v/media/{id}/preset.m3u8）
直链起播验证失败 → 本地文件自动回退 HLS；云盘不回退（弹错误面板，防止 24G 4K 转码炸 NAS）
```

进度上报：`POST /play/record` 每 15 秒 + seek 时立即 + 切集/退出时；已看 `POST /item/watched`。凭证通过 mpv `httpHeaders` 显式传入（同 host 才带 NAS Cookie/Authorization）。

**与我们的差异**：他们无本地代理（mpv 直连，凭证喂 header）；我们的 Go 代理（127.0.0.1:22346）方案在证书跳过、Cookie 搭车、多播放器（PotPlayer）复用上更稳，**不必换**。

### 1.4 他们踩过的坑（最有价值的部分，做自建 UI 时全是现成答案）

以下全部来自其源码注释，做第二阶段自建 UI 时直接照抄避免重蹈：

1. **DELETE 请求的 Authx 签名必须按 body 算**，放 query 会被判 invalid sign（`media_remote_data_source.dart:494`）
2. **转码目标编码不能等于源编码**（请求 av1→av1 报 ParameterError 8192）
3. **错误码 8192 = 服务端自动回退原画直链**，要当正常流处理
4. **mpv 不认 `?range=bytes=` 查询串直链**，但后端支持标准 Range 头；续播用 mpv `start` 属性最可靠
5. **请求头缺 `x-trim-client-version` 会隐藏 IPTV 媒体库**
6. mpv `time-pos` 在解码前会回显 seek 目标值，裸读会误判（我们的 mpv.ts 有同款竞态处理）
7. mpv `hwdec-current` 运行中改 hwdec 不刷新（mpv issue #4289），探测硬解要独立 player 实例
8. 云盘大文件转码协商超 10s，receiveTimeout 要临时放宽到 60s
9. 直链禁 cache-pause 防误暂停、慢源 underrun 又会永久冻结 → 需自适应切换

### 1.5 弹幕（我们领先的点）

他们的弹幕 = **自建飞鲸服务端聚合**（SSE 下发）+ Flutter canvas 自绘 overlay，需授权码才能用。我们的链路（danmu_api 聚合优选 → B站直连（WBI/cookie）→ 弹弹play → uosc_danmaku ASS 副字幕轨按显示器刷新率渲染 + 自编译 mpv 补丁）**不依赖任何第三方服务端、渲染质量和刷新率上限更高**。这是我们的核心差异化资产，任何架构演进都不该丢。

---

## 二、现状对照

| | Fntv-Plus（现状） | FlyNarwhal |
| --- | --- | --- |
| UI 来源 | 飞牛真实网页 `/v` + 28 个 preload 插件注入 | 全自建 |
| 播放器 | 外部 mpv.exe 独立窗口 + JSON IPC 命名管道 | libmpv 进程内嵌（media_kit Texture） |
| 直链/鉴权 | Go 本地代理（22346） | mpv 直连 + httpHeaders |
| 弹幕 | 三源聚合 + ASS 副字幕轨（自编译核心） | 飞鲸服务端 + canvas 自绘 |
| 片头片尾 | smart_skip + 手动标记 + 飞牛原生数据 | 飞鲸智能分析 |
| 对飞牛更新的鲁棒性 | **弱**（类名/DOM 结构脆弱依赖，靠热补丁续命） | **强**（只依赖 API，FlyNarwhal 的 18 条坑早已固化） |

**我们"基于网页改动"的代价，代码注释里已经自述得很充分**：580 行 ACRYLIC_CSS 精确匹配飞牛 Tailwind 类、JS 核弹级白底清除器 + MutationObserver 常驻对抗 React 重渲染、listLayout 九轮迭代、watchedSync 隐藏 iframe 方案七轮失败后改为直调 API（这条其实已经是"自建 UI 思路"的局部胜利）、DComp 钳 30Hz、onscroll 138ms 抢主线程……这些都是结构性成本，会一直付下去。

---

## 三、MPV 内嵌的三条技术路线（Electron 语境）

### 路线 A：`--wid` 子窗口嵌入（推荐 ★★★）

**原理**：mpv.exe 照常外部启动，但加 `--wid=<HWND>`，让它把画面渲染进我们指定的窗口句柄（作为 Electron 窗口的 WS_CHILD 子窗口），而不是自己开窗。控制通道（node-mpv-2 / 命名管道）、Lua 脚本、自编译核心**全部原样保留**。

- **改动量**：极小。`src/modules/players/impl/mpv.ts` spawn 参数加一项 `--wid`；`BrowserWindow.getNativeWindowHandle()` 取句柄（或专门建一个"播放舞台" BrowserWindow）。现有 IPC、script-message、进度回传、弹幕链路零改动 —— **弹幕是 ASS 副字幕轨，跟着 mpv 画面走，内嵌后原样工作**。
- **先例**：[electron-libmpv](https://github.com/REVENGE977/electron-libmpv)（名字叫 libmpv，实际就是 native 窗口嵌入方案，mpv.js 死后的替代品）、[empv](https://github.com/linmi/empv)、[hearth](https://github.com/Citizen-Forge/hearth)（10-foot TV 应用，--wid 子窗口 + 续播）。
- **优点**：
  - 进程隔离：mpv 崩溃不伤主程序（libmpv 内嵌则同进程，崩了连带 UI）
  - 现有 uosc/uosc_danmaku/smart_skip/thumbfast/着色器/插帧**全部资产原样继承**
  - 不碰 Electron 合成器，无 lc-1347 那类 DComp/透明合成性能坑
- **必须接受的限制（Windows）**：
  1. **子窗口永远压在 web 内容之上**：视频区域上不能直接叠 HTML 元素。播放控制要么继续用 uosc（本来就主题化了，推荐），要么做独立置顶透明 BrowserWindow 当 overlay（交互需 `setIgnoreMouseEvents` + 鼠标转发，可行但要调）
  2. **键盘焦点**：点击视频区焦点进 mpv 子窗口，Electron 层快捷键失效 → 维持 mpv 侧 input.conf 键位（我们已很全），或 IPC 转发
  3. **不要嵌进现在的主窗口**：主窗口是 `transparent:true` 亚克力壳（圆角/clip-path 一堆 workaround），子窗口无法继承圆角、还会踩透明合成性能坑。**正确姿势：新建一个不透明、无边框的专用"播放窗口"**，关详情页动画风暴的那套经验（pageAnim）也天然用不上
  4. resize 时子窗口重排有轻微闪烁（mpv 跟随父窗口客户区），全屏切换 = 父窗口全屏，偶发 DWM 模式切换 artifact
- **风险**：低。本质是把"独立窗口"换成"摆进我们窗口里的子窗口"，播放管线一行不动。

### 路线 B：libmpv render API 纹理共享（不推荐 ★）

mpv.js 靠 PPAPI，Chromium 已移除，**已死**。electron-libmpv 实际走的是窗口嵌入而非纹理共享。真·纹理级内嵌 = 自写 N-API 原生插件 + libmpv render API + D3D11/OpenGL 共享纹理接入 Electron 合成器 —— **无现成维护中的开源方案，等于自己造 media_kit 的 Electron 版**，工程量月级、长期维护黑洞。Electron 不支持外部纹理进合成器，这条路在 Electron 下没有性价比。

### 路线 C：换 Flutter + media_kit（= FlyNarwhal 同款，等于全面重写 ★★）

media_kit 把 libmpv 内嵌封装得很好（`Video` widget + Texture 上屏，硬解探测、缓存策略、字幕注入都有现成 API）。但代价是：**8.5 万行 Dart 是人家整个应用**；我们要放弃 28 个 preload 插件、全部 Electron 侧集成（Trakt/豆瓣/手柄/热补丁机制/fpk 复用链路），TS 资产归零重写。唯一真正的收益是"真·内嵌渲染"和更快的启动 —— 而这个收益用路线 A 能拿到八成。

### 对比

| | A：--wid 嵌入 | B：纹理共享 | C：Flutter 重写 |
| --- | --- | --- | --- |
| 改动量 | 天级 PoC，周级完善 | 月级（自研原生插件） | 季级（全应用重写） |
| 现有资产保留 | 全部 | 全部（控制层） | 仅 Lua 脚本部分可迁 |
| UI 一体化 | 中（uosc 或 overlay） | 高（任意 HTML 叠加） | 高（原生 widget） |
| 崩溃隔离 | 好 | 中 | 差（libmpv 进程内） |
| 弹幕 | 原样工作 | 原样 | 要重验 OSD/ASS 在 render API 下的表现 |
| 风险 | 低 | 高 | 高 |

---

## 四、AGPL 合规

- FlyNarwhal 是 **AGPL-3.0**，我们仓库是 **GPL-3.0**。
- **API 端点、参数、签名算法、坑位清单属于"事实"，不受版权保护**，可自由参考实现（何况核心链路我们早已独立逆向）。
- **不能直接复制他们的 Dart/TS 代码**进我们仓库 —— 一旦引入，衍生作品整体必须升级为 AGPL-3.0 并公开完整源码（含对应的 objCode 分发）。如果未来真走 Flutter 重写，也建议参考行为而非抄码，或届时由你决策是否接受 AGPL 化。
- 我们的反代注入 fpk、preload 补丁体系是独立作品，无此约束。

---

## 五、建议的分阶段路径

内嵌 MPV 只是手段，目标是摆脱"网页注入"的结构性成本。建议：

**阶段一（PoC，天级）：`--wid` 内嵌播放窗口**
- 新建专用播放 BrowserWindow（不透明、无边框、全屏友好），`getNativeWindowHandle()` → `--wid` 喂给现有 mpv 启动链
- 验证清单：起播/seek/倍速/弹幕 ASS 渲染/进度回传/全屏/多显示器 DPI/resize 闪烁体感/uosc 键位焦点
- 与现有"独立 mpv 窗口"模式做成**双模式可切换**（保留逃生门），不动任何现有默认行为
- 顺带验证：透明 overlay 窗口盖在视频上做 Electron 侧控制栏的可行性（决定阶段二播放器 UI 走 uosc 还是自绘）

**阶段二（周-月级）：自建浏览 UI，网页壳退役**
- 播放器 UI 定型后，逐步自建：媒体库/继续观看/搜索/详情/剧集/设置，数据层直接复用 `fn_api`（补齐 `mediadb/list`、`play/list`、`search/list`、`item/{guid}`、`person/*`、subtitle 全套 —— FlyNarwhal 清单即 checklist，九条坑照抄）
- 每迁走一个页面，就删掉对应的一坨 CSS 对抗/MutationObserver/热补丁
- 网页壳（`/v` 加载）降级为兜底模式，最终可选退役；fpk（NAS 侧注入）不受影响，继续服务电视/浏览器用户
- Trakt/豆瓣/手柄/B站弹幕等主进程能力与 UI 无关，原样复用

**不推荐**：直接立项 Flutter 重写（除非已决定放弃 Electron 生态）；在 Electron 下做纹理级内嵌。

## 风险清单

1. **飞牛 API 无官方承诺**，大版本更新可能变动 —— 但 FlyNarwhal 证明这套接口多年稳定，且变动时修 API（一处）远比修 DOM 注入（处处）便宜
2. `--wid` 嵌入在特定显卡/驱动组合下的渲染异常需真机回归（现有用户本来就在跑同一 mpv.exe，风险敞口不变）
3. 转码会话（`/play/play`）我们尚未实现，自建 UI 若要完整对齐网页端"软解设备播 4K"场景需补齐（参数坑见第四节第 2、3 条）
4. AGPL 红线：只看事实，不抄代码
