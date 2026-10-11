# WinUI 3 重写自建客户端 + 内嵌 MPV 可行性评估

> 2026-10-11 · 承接 [FlyNarwhal调研与MPV内嵌可行性评估](FlyNarwhal调研与MPV内嵌可行性评估.md)
> 目标：甩掉"网页注入"，像飞鲸那样直调 API 自绘界面，内嵌 MPV，**核心指标：安装包大小**（用户明确不管安装后体积）
> 现状基线：NSIS 安装包 **206MB**，安装后 716MB（Electron 壳 ≈250MB + fntv-mpv 385MB + 自有代码）

**结论先行：**

1. **可行，但"换 WinUI 3"不是安装包的最大那刀。** 385MB 的 mpv 分发里有 **208MB 死重**（114MB 旧版 mpv.exe 备份、18MB 脚本备份、14MB 缓存、39MB 播放用不到的编码器 DLL），删掉它安装包立刻 **206 → ~150MB**，零架构改动、当天可做。
2. 换 WinUI 3 自包含部署，安装包再降到 **~105-125MB**；若用 framework-dependent（运行时装用户机器上）可到 **~65-75MB**。
3. WinUI 3 下内嵌 MPV **可行但比 Electron 费劲**：没有现成维护的 C# 嵌入库（Mpv.NET 是 WPF 的，media_kit 是 Flutter 的），要么 `--wid` 子 HWND（保留我们全部 mpv 资产，推荐），要么自研 D3D11 桥接进 SwapChainPanel（有 UWP 先例 mpv#5979，月级工作量）。
4. 必须接受的代价：**macOS 版直接没有**（当前 electron-builder 配了 dmg x64+arm64）；WinUI 3 社区 2023-2025 长期停滞（Build 2026 微软重新表态投入）；整个 Node 主进程逻辑要用 C# 重写。
5. 建议节奏：**先做零风险的 mpv 瘦身**（立刻受益）→ WinUI 3 PoC 验证嵌入手感 → 决定全量迁移。

---

## 一、安装包 206MB 的解剖（你的核心指标）

| 成分 | 未压缩 | 压缩后(约) | 说明 |
| --- | --- | --- | --- |
| Electron 壳（Chromium/Node/DLL） | ~250MB | **~85MB** | 换框架可整块拿掉 |
| fntv-mpv 合法资产 | ~177MB | ~55MB | mpv.exe + 解码/渲染/字幕 DLL + 47MB CJK 字体（弹幕/ASS 必需） |
| **fntv-mpv 死重** | **~208MB** | **~55MB** | 见下表，**可全删** |
| 自有代码/资源 | ~10MB | ~6MB | asar + login 页 + wiki 等 |

**死重清单**（`third_party/fntv-mpv/`，实测 du）：

| 路径 | 大小 | 为什么是死重 |
| --- | --- | --- |
| `_legacy-v157/` | **119MB** | 旧版 mpv.exe 整包备份（单 exe 114MB），发布包里完全不该有 |
| `libx265-217.dll` + `libaom.dll` + `libSvtAv1Enc-4.dll` | **39MB** | 三个视频**编码器**，纯播放器用不到 |
| `scripts-backup/` | 18MB | uosc 原版备份（含 darwin/linux 的 ziggy 二进制） |
| `portable_config/.cache/` | 14.5MB | 运行时缓存被提交进仓库了 |
| `libjxl.dll` | 6MB | JPEG XL 解码，影视场景极边缘 |
| `librsvg-2-2.dll` | 11.5MB | SVG 渲染，uosc 不依赖时可删（删前需验证） |

**矩阵：不同组合下的安装包预估**（压缩比按实测 0.28-0.34 推算，标 ☆ 的是估算）：

| 方案 | 安装包 | 改动量 |
| --- | --- | --- |
| 现状 | 206MB | — |
| 仅 mpv 瘦身（Electron 不动） | **~150MB** | 删文件+改打包过滤，1 天 |
| mpv 瘦身 + WinUI 3 自包含 | ☆ 105-125MB | 整个重写（本文主题） |
| mpv 瘦身 + WinUI 3 framework-dependent | ☆ 65-75MB | 重写 + 用户机装运行时（bootstrapper 自动拉） |

> 注意：WinUI 3 的 .NET 运行时**不能靠 Native AOT/裁剪救命**——XAML 栈是反射重灾区，裁剪基本不可用（微软官方未支持），自包含就按 ~100-250MB 未压缩的 WinAppSDK+.NET 预算。

---

## 二、WinUI 3 路线评估

### 2.1 框架本身

那个修改器 = C++/WinRT + WinUI 3（Windows App SDK 2.0）+ VMProtect。对我们来说语言选 **C#**（C++/WinRT 写应用层是自虐）。你看到它的"轻"是真的：原生窗口、无 Chromium、内存 ~100-150MB 级（Electron 我们实测主+渲染 300-500MB）、冷启动 <1s。

但要清醒**三点**：

1. **成熟度风险**：WinUI 3 社区 2023-2025 抱怨停滞（"WinUI3 is really dead" 讨论、开源计划跳票），直到 Build 2026 微软才重新表态。它不是 Flutter 那种有大规模生产验证的框架，ListView 虚拟化、焦点、内存泄漏类问题要有自己踩坑的预期。
2. **macOS 直接出局**：当前 electron-builder 有 dmg（x64+arm64）目标。WinUI 3 是 Windows 独占。保留 mac = 双客户端双倍维护，不建议；接受 mac 用户留在旧 Electron 版即可（旧版继续发）。
3. **飞鲸当年没选 WinUI 3**：FlyNarwhal 1.x 用 Kotlin/Compose，2.x 换 Flutter + fluent_ui + media_kit，8.5 万行 Dart 跑在三个桌面平台。原因不难猜：media_kit 把 libmpv 内嵌、硬解探测、字幕注入全部封装好了，`Video` widget 一挂就出画面；这正是我们最难的点（见 2.2）。如果 WinUI 3 PoC 阶段嵌入卡壳，Flutter 是唯一有现成答案的 Plan B（亚克力有 flutter_acrylic 包，走的同样是 DWM 系统背景材质）。

### 2.2 内嵌 MPV：WinUI 3 下的两条路

| | 路线 A：`--wid` 子 HWND（推荐起步） | 路线 B：libmpv render API + SwapChainPanel |
| --- | --- | --- |
| 形态 | 继续跑 mpv.exe 独立进程，画面嵌进我们窗口 | libmpv.dll 进程内，自研 D3D11 桥接把帧送上 SwapChainPanel |
| 现成度 | **高**：mpv.net（C#）就是这么嵌 WPF 的；WinUI 3 窗口有真 HWND（`WindowNative.GetWindowHandle`），其上可开 Win32 子 HWND | **低**：无维护中的 C#/WinUI3 绑定（Mpv.NET 停滞且 WPF 专属）；有 UWP 时代 `vo=gpu + gpu-context=d3d11` 进 SwapChainPanel 的成功先例（mpv-player/mpv#5979），桥接代码要自己写 |
| 我们的资产 | **全保留**：自编译 mpv（secondary-ass 弹幕补丁）、uosc_danmaku、smart_skip、着色器、插帧、JSON IPC 控制链路原样工作 | 弹幕 ASS 副字幕轨能渲染（sub 轨进纹理），但 uosc/OSD 路径需重新验证；Lua 脚本大多可用但生态全要重测 |
| 空域(airspace)限制 | mpv 子窗口永远盖住 XAML 内容 → 播放控制栏继续用 **uosc**（本来就主题化了，零成本），或额外开置顶子 HWND 用 Win2D 画自绘控制栏 | 无空域问题，控制栏就是普通 XAML，体验上限最高 |
| 进程隔离 | 好（mpv 崩了 UI 不死） | 差（同进程，崩了连坐） |
| 工作量 | 天级 PoC，周级打磨 | 月级起步，长期自维护 |

**判断**：先用路线 A（它同时是 Electron PoC 的同一套验证），路线 B 作为二期升级项。这与飞鲸选 media_kit 的逻辑一致——真·纹理级内嵌的桥接成本，有人封装好（Flutter）就白拿，没人封装就别在第一版自己造。

### 2.3 要用 C# 重写的东西（真正的成本所在）

换框架的死成本不是 UI，是**主进程逻辑**：

| 模块 | 迁移成本 | 备注 |
| --- | --- | --- |
| fn_api 客户端（Authx 签名/登录/播放链路） | 低，周内 | 端点/签名全已知（本就独立逆向过，飞鲸的坑清单可照抄），HttpClient 重写即可 |
| mpv JSON IPC 控制（node-mpv-2 → 自写） | 低 | 命名管道 + JSON，C# 原生擅长 |
| Go 本地代理（22346） | **零** | 独立 exe，原样带过去 |
| fntv-mpv + 全部 Lua 资产 | **零** | spawn + `--wid`，原样带过去 |
| danmu_api 聚合源 | **零** | 自建 HTTP 服务，C# 直连 |
| B站弹幕直连（bili_danmaku.js / WBI 签名） | 中，周级 | WBI 混淆算法要用 C# 重实现（算法公开，机械活）；或由 Go 代理代跑 |
| Trakt / 豆瓣 / Bangumi / TMDB 集成 | 中 | 各自 HTTP 客户端重写 |
| 设置体系（mpvConfig 双写、着色器预设、smart_skip 等 conf 生成） | 中 | 字符串模板搬运 |
| 28 个 preload 插件 | **零（该死）** | 全是为"改官方网页"而生的，重绘 UI 后整体退役，一个都不用迁 |
| fpk（NAS 侧反代注入） | **零** | 与桌面端框架无关 |

UI 本身（登录/媒体库/首页/详情/搜索/设置/播放器）：参照飞鲸规模 8.5 万行 Dart，我们做到"够用版"（列表/详情/搜索/播放/设置）是**月级**；想全功能对齐现在 Electron 版的所有花活（3D 轮播、Netflix 详情页、年度报告、手柄……）要有长期补齐的心理预期。

---

## 三、建议的执行顺序

**阶段 0 —— mpv 分发瘦身（零风险，立刻受益，与换框架无关）**
- 删 `_legacy-v157`、`scripts-backup`、`.cache`、三个编码器 DLL、libjxl；librsvg 验证后定去留
- 顺带修打包规则：`build.files`/extraResources 加过滤，防缓存再进包
- 产出：安装包 206 → ~150MB，**当天完成**，无论后面选哪条路都不亏

**阶段 1 —— WinUI 3 PoC（1-2 周）**
- C# + WinAppSDK 2.0（自包含），登录 → `item/list` 拉媒体库 → 详情页三个页面
- `--wid` 嵌入现有 mpv.exe + uosc，验证：嵌入手感、resize/全屏、弹幕 ASS、DPI、DWM 背景材质（acrylic/mica 顺手开出来）
- 验收标准：这仨页面流畅、mpv 嵌入无硬伤 → 才谈全量；嵌入有硬伤 → 评估 Flutter Plan B

**阶段 2 —— 数据层 C# 化 + 播放管线（月级）**
- fn_api 全量端点、Authx、播放/进度/已看上报、弹幕数据层（danmu_api 直连 + WBI 重写）、mpv IPC 控制
- 新客户端与旧 Electron 版**并存过渡**：共用同一套 fntv-mpv 配置与弹幕缓存，旧版继续给 mac 用户和兜底

**阶段 3 —— 全功能补齐 + Electron 退役决策**
- 设置面板、Trakt/豆瓣、播放记忆等逐个迁移；Windows 全量迁移后再决定旧版去留

## 风险清单

1. WinUI 3 框架本身的坑（列表性能/焦点/泄漏）——PoC 阶段用真实数据量（几千条目的媒体库）压测虚拟化列表
2. mac 用户流失（留在旧版）
3. 路线 B（render API）若二期要做，是自研桥接的长期维护承诺
4. 安装包的最终下限被 fntv-mpv 合法资产（~177MB/压缩 ~55MB）钉死：**~60-70MB 就是框架路线的地板**，除非砍字体/砍功能
5. AGPL（同前报告）：FlyNarwhal 只能看事实不能抄码
