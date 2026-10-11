# Tauri 2 迁移具体方案

> 2026-10-11 · 承接 [WinUI3重写与安装包瘦身评估](WinUI3重写与安装包瘦身评估.md)、[FlyNarwhal调研与MPV内嵌可行性评估](FlyNarwhal调研与MPV内嵌可行性评估.md)
> 决策：不选 WinUI 3（丢 mac），选 **Tauri 2** —— 保留 TS 前端技能、安装包最小、双平台原生背景材质
> 基线：现安装包 206MB（mpv 瘦身后 ~150MB），Electron 内存 300-500MB

**一句话架构：Tauri 2 壳 + 自绘 Web UI（TS）+ Rust 核心（fn_api/mpv 控制/弹幕）+ Go 代理 sidecar + fntv-mpv 原样 spawn 内嵌（双层窗口，UI 完全自主）。**

```
┌─ Fntv-Plus (Tauri 2) ─────────────────────────────────┐
│ 顶层：透明 WebView —— Vite+TS+React 自绘全部 UI         │
│   登录/媒体库/详情/搜索/设置 + 自绘播放控制栏            │
│            ▲ invoke() / event 流                       │
│ Rust 核心 (src-tauri)：                                │
│   ├ fn_api 客户端（reqwest + Authx 签名移植）           │
│   ├ mpv 生命周期 + JSON IPC（named pipe）+ script-msg  │
│   ├ 双层子窗口管理（mpv 垫底层 + WebView 顶层）+ 亚克力  │
│   ├ 弹幕数据层（danmu_api / dandanplay / B站WBI）       │
│   └ conf 生成器（mpv-user.conf / uosc_danmaku.conf…）  │
└─────────┬──────────────────────┬──────────────────────┘
   sidecar：Go 代理        spawn：fntv-mpv/mpv.exe
   (127.0.0.1:22346)      (--wid=底层子HWND/NSView，垫底)
```

**核心原则：不动的东西绝不动。** Go 代理（独立 exe → sidecar）、fntv-mpv 全家（自编译核心 + uosc_danmaku + smart_skip + 着色器，spawn + `--wid`）、danmu_api 服务、fpk —— 四大资产零改动平移。重写只发生在两处：Node 主进程逻辑 → Rust；网页注入 UI → 自绘 Web UI。

---

## 〇、播放器：双层窗口架构（UI 完全自主，PotPlayer 退役）

需求变更：控制栏不再用 uosc，**全部 UI 由我们画**；播放器只有内嵌 mpv 一个，PotPlayer 选项退役。

```
z 轴叠层（同一 Tauri 窗口内）：
┌─────────────────────────────────┐
│ 透明 WebView（顶层）              │  ← 我们的全部 UI：控制栏/菜单/OSD 提示
│   非视频区透出亚克力背景材质        │
│   视频区 = 透明，看见下面……        │
├─────────────────────────────────┤
│ mpv 子窗口（--wid，底层）          │  ← mpv.exe 照常渲染：视频 + ASS 弹幕 + smart_skip
└─────────────────────────────────┘
```

- **为什么可行**：空域问题（HTML 画不进子窗口上方）的解法不是"往同一层里塞"，而是**把 WebView 整层放到子窗口上面**：WebView2 背景透明（Tauri `transparent` + WebView2 `DefaultBackgroundColor=0`），视频区域在 HTML 里保持透明，mpv 画面从底下透出来；控制栏是普通 HTML，想怎么画怎么画。mpv 仍是独立进程 —— 自编译核心、副字幕轨弹幕（按显示器刷新率渲染）、smart_skip、着色器**全部原样工作**，且不用做纹理共享/CPU 拷帧。
- **输入模型反转**：所有鼠标/键盘/手柄事件先进我们 UI，由前端经 IPC 转发 mpv 标准命令（cycle pause、seek、set volume…）。现成先例：Electron 版手柄控制就是这么干的。`input.conf` 键位保留作后备。需要自研的手势就十来个：双击全屏、滚轮音量、拖动进度、长按倍速、上下键切集。
- **mpv 内可留的东西**：弹幕（副字幕轨）、smart_skip、插帧、着色器、thumbfast —— thumbfast 本来就走 script-message 协议（`thumb x y w h`，uosc 同款），我们自己的进度条 hover 时把坐标发过去，缩略图定位就能跟随**我们的**进度条。
- **退役清单**：uosc 控制栏（脚本文件保留在 mpv 目录里作应急后备，平时关 OSC/隐藏）、`playChoice` 播放器三选一 UI、PotPlayer 启动链路（不迁移）。
- **不选 libmpv 进程内方案的原因**：tauri-plugin-libmpv 那条路即使做通，控制栏还是得叠在渲染层上方，得到的双层结构一模一样，却要放弃 mpv.exe 进程隔离和整套 Lua 资产——纯亏。

---

## 一、定案的技术选型

| 项 | 选择 | 理由 / 备注 |
| --- | --- | --- |
| 前端框架 | **Vite + TS + React 19** | 团队对 fnOS 的 React 代码库已有两年贴身经验，熟悉度优先；体积敏感时降级 SolidJS（UI 代码几乎照搬） |
| 样式/组件 | Tailwind v4 + 自绘组件 | 现有 ACRYLIC_CSS 的设计语言（玻璃拟态/云母/圆角）就是资产，抽成 design token 继续用 |
| 列表虚拟化 | TanStack Virtual | 媒体库几千条目，PoC 就要用真实数据量压 |
| 亚克力 | Tauri 2 内置 `windowEffects`：Win11 `mica`/`acrylic`，Win10 回退 `blur` 或不透明，**mac `vibrancy`** | 不需要第三方 crate；按 build 号门控（Win10 的 acrylic 走 SetWindowCompositionAttribute，拖动会卡，已知坑） |
| mpv 内嵌 | **双层窗口**：mpv.exe `--wid` 垫底层子 HWND/NSView + 透明 WebView 顶层画控制栏；自写 ~300 行 Rust 模块 | [tauri-plugin-libmpv](https://lib.rs)（v0.3.2）太年轻且走 libmpv 进程内路线——会丢掉 mpv.exe 的整套 Lua 生态，**不采用，仅作窗口机制参考** |
| mpv IPC | `mpvipc` crate（GPL-3.0，1.3.1 活跃）或自写 | 自写也就 ~300 行（协议我们吃透了，node-mpv-2 同款）；mpvipc 需先验证 Windows named pipe 支持 |
| fn_api | reqwest(rustls) + 自写 Authx 中间件 | 签名算法 = `md5(apiKey_url_nonce_时间戳_dataMd5_apiSecret)`，纯机械移植 |
| 密码存储 | `keyring`（OS 凭据库） | 对齐飞鲸的加密存储思路，不做自造加密格式 |
| 更新 | `tauri-plugin-updater` | 需生成签名密钥对；mac 公证流程另列 |
| WebView2 | `webviewInstallMode: downloadBootstrapper` | 老 Win10 无 evergreen 运行时时自动装 |

## 二、模块迁移映射（旧 → 新）

| 现有模块 | 去向 | 量级 |
| --- | --- | --- |
| 28 个 preload 插件 | **全部退役**（它们的存在意义就是改官方网页） | 0（不迁移） |
| `mainwin.ts`/`winctrl.ts`（窗口/守卫/亚克力CSS 580行） | `tauri.conf.json` + `lib.rs`；ACRYLIC_CSS 由系统背景材质取代 | 小 |
| `tray.ts`/单实例/退出流程 | tauri tray + `single-instance` 插件 | 小 |
| `fn_api/request.ts`+`api.ts`（Authx、lc-294 Cookie） | Rust `api` 模块；不再需要 Cookie 双通道（没有网页了），纯 Header + 端点照 [FlyNarwhal 调研](FlyNarwhal调研与MPV内嵌可行性评估.md) 的清单补全（mediadb/play list/search/season/person…） | 中 |
| `auth.ts`/`fnid_login.ts`（三种登录） | 本地登录 = Rust 直调；FN ID OAuth = 子 `WebviewWindow` + `initialization_script` hook fetch 拿 code（同现思路） | 中 |
| `players/impl/mpv.ts`（node-mpv-2）+ `media.ts` | Rust `mpv` 模块：spawn + `--wid` + named pipe JSON IPC + `script-message`（skip-metadata 等）；播放 URL 继续走 Go 代理 | 中 |
| `mpvConfig.ts`（conf 双写/着色器预设/弹幕设置） | Rust conf 生成器，字符串模板搬运 | 中（机械） |
| `danmuApi.ts`/`biliRunner.ts` | Rust `danmaku` 模块：danmu_api HTTP 直连（零成本）→ dandanplay 开放API → B站 WBI（C/Rust 重写，算法公开，放最后） | 中 |
| Go 代理（22346） | **sidecar 原样**（tauri `externalBin`） | 0 |
| fntv-mpv + Lua 全套 | **资产原样**（先做阶段0瘦身） | 0 |
| `resource/login` | 改造为前端登录页底子 | 小 |
| Trakt/豆瓣/Bangumi/TMDB handlers | Rust 各自 HTTP 客户端 | 中，Phase 3 |
| fpk | 不动 | 0 |
| 桌面花活（3D轮播/Netflix详情/年度报告/手柄…） | 前端自绘逐个补齐 | 长期 |
| **PotPlayer 支持（playChoice 三选一等）** | **退役不迁移**：唯一播放器 = 内嵌 mpv，播放器选择 UI 消失 | 0 |

## 三、执行计划（含验收标准）

**阶段 0：mpv 分发瘦身（1 天，独立收益，先行）—— 已做引用扫描 + PE 导入表 + 运行时改名回放三重验证**
- ✅ 可删（~152MB 本地，其中 ~135MB 进安装包）：`_legacy-v157/`(117M 旧版整包备份)、`scripts-backup/`(18M uosc 原版备份+ziggy 三平台二进制)、`installer/`(0.3M) —— 三者零引用、不在任何 DLL 加载链上；`portable_config/.cache`(15M)+`cache`(2.3M) 为 libplacebo 着色器/ICC 运行时缓存，自动重建（打包 filter 本就排除，仅清本地 + .gitignore）
- ❌ 不可删（本次验证推翻了最初清单）：**全部编码器 DLL（x265/aom/SvtAv1Enc/rav1e/x264/vpx/lame…）以及 libjxl/librsvg 都是 `avcodec-63.dll` 的启动硬导入**（PE 导入表实证 + 改名后 mpv 进程无法启动 exit=127 实测）——删任何一个 mpv 直接起不来。想去掉这 ~45MB 只能走自编译管线裁掉 ffmpeg 编码器（列 mpv-patches 构建待办，不阻塞瘦身）
- 明确不删：fonts(47M，弹幕/ASS 中文字形)、libshaderc(Anime4K 着色器与插帧 Vulkan 路径依赖)、libplacebo(gpu-next 核心)、libdav1d(AV1 解码)、libvpl(Intel QSV 硬解)、libmpv-2(未来纹理级内嵌的种子)、whisper 三件(仅 1.5M 且为懒加载，不值得动)
- 打包动作：fntv-mpv 的 `**/*` 全拷加 filter 排除 `_legacy-v157`/`scripts-backup`/`installer`；**PotPlayer 暂不动**（用户决策，`third_party/potplayer` 206M 留待将来）
- 回归验证：H.264/HEVC/AV1 各播一段 + 弹幕字形 + Anime4K + 插帧 + thumbfast + smart_skip（本次已用 lavfi 自产三编码测试片预演回放通过）
- 产出：安装包 206 → **~166MB**（PotPlayer 将来若退役再省 ~60-70MB）
- ✅ **2026-10-11 已执行**：本地 386→236MB（-150MB）；win.extraFiles filter 已加三个排除；回归通过（H.264/HEVC/AV1 自产测试片回放 exit=0 + 真实 portable_config 冒烟 exit=0，`mpv-interp-nvidia.conf` 缺失为既有优雅降级、应用启动自动重建）；`installer/` 内 4 个 git 跟踪小文件随下次提交入库，其余删除项均未跟踪无痕

**阶段 1：PoC（1-2 周）—— 用最短路径验证三件事：嵌入、材质、列表性能**
- 骨架：Tauri 2 + Vite/React + 侧边栏导航；`windowEffects` 开 mica/acrylic
- 登录页 → `POST /login` → `item/list` 媒体库网格（真实数据量）→ 条目详情页
- mpv 内嵌：Rust 双层子窗口模块（mpv `--wid` 垫底 + 透明 WebView 顶层）+ 自绘控制栏雏形（播放/暂停/进度条/音量/倍速/双击全屏）；播放走 Go 代理 URL
- **验收标准**：① 三页面 60fps 滚动不卡；② 视频上方自绘控制栏事件命中正常，透明 WebView 合成经 perfDiag 同款方法 A/B 无掉帧；③ mpv 嵌入 resize/全屏/DPI 无硬伤，弹幕 ASS 正常；④ Win11 mica/acrylic 生效、Win10 优雅回退；⑤ macOS `vibrancy` + NSView 嵌入跑通
- 任何一条不过 → 停下来评估（Flutter Plan B 或回 Electron 自绘路线），不带病推进

**阶段 2：核心功能（3-4 周）**
- fn_api 全量端点 + 播放管线完整化（play/info → 清晰度 → 代理直链 → open → 进度每15s+seek上报 + 已看标记）
- 剧集/季导航、搜索、继续观看、播放记忆、自动连播
- 弹幕基础链路（danmu_api + dandanplay 手动搜索候选）

**阶段 3：补齐与打包（2-3 周）**
- 设置面板（写回 mpv-user.conf/uosc_danmaku.conf 的生成器）、FN ID 登录、Trakt/豆瓣、B站 WBI 直连、smart_skip/thumbfast 配置面
- updater 接入、mac 公证、NSIS 安装包验收（目标 **~65-80MB**：Tauri 壳 3-10 + Go 代理 ~4 + 前端资产 ~3 + mpv 瘦身后 ~55，均压缩后估算）

**阶段 4：双轨过渡 → 退役**
- Tauri 版走独立"测试版"发布通道，与 Electron 版并存（共享 fntv-mpv 配置与弹幕缓存目录）
- Windows 用户迁移完毕后 Electron-Windows 停更；mac 等 Tauri 版功能对齐后同法处理；Electron 最终退役或仅保养老用户

## 四、风险与对策

| 风险 | 对策 |
| --- | --- |
| 透明 WebView 叠视频：WebView2 透明合成性能需实测（Electron 时代有 DComp 钳 30Hz 的教训） | PoC 阶段用 perfDiag 同款方法 A/B；不过关的降级方案：视频区外自绘 UI + 视频区内临时 uosc 兜底 |
| 手势/键位全自研（双击全屏、滚轮音量、拖动 seek、长按倍速…） | 总量十来个，IPC 转发 mpv 标准命令；input.conf 键位保留作键盘后备；thumbfast 缩略图经 script-message 跟随自绘进度条 |
| Win10 acrylic 拖动卡顿 | build 号门控：<22621 用 `blur` 或纯色 |
| WebView2/WKWebView 双引擎渲染差异 | UI 全自绘 + 视频全走 mpv，webview 只画界面，风险面小；两平台真机各过一遍 |
| Rust 学习曲线 | 后端面刻意做小：HTTP + 文件 IO + 管道，重活都在 Go 代理和 mpv 进程里 |
| `mpvipc` 的 Windows named pipe 支持 | PoC 第一天验证；不行就自写（代码我们等于已有） |
| 旧用户迁移 | 新 appId 全新配置目录，需重新登录；弹幕缓存目录沿用 `%PUBLIC%\fnos-danmaku`；写一次性导入器读旧 `AppData/Roaming/fntv` 的域名配置 |
| 许可 | 项目 GPL-3.0：mpvipc(GPL-3.0)✓、tauri-plugin-libmpv(MPL-2.0) 仅参考不抄码、FlyNarwhal(AGPL) 只看事实 |

## 五、需要拍板的三个点（都给了默认值，不反对就按默认走）

1. **前端 React（默认）还是 SolidJS**——差在包体 ~40KB gzip 和运行时性能，React 熟。
2. **仓库形态**——默认同仓新顶层目录 `tauri/`（共享 third_party 与 resource），稳定后按需拆库。
3. **mac 过渡**——默认 Tauri-mac 从测试通道起步，Electron-mac 保留到功能对齐再退役。
