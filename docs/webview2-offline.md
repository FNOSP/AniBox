# WebView2 离线运行时安装包方案

> 面向 AniBox（Tauri 2 + React）Windows 安装包。目标：**用户双击安装包即可完成安装，全程无需联网**。
> 本文同时解释 `WebView2 运行时初始化失败 HResult 0x80010108 RPC_E_DISCONNECTED` 的成因与排查。

## 1. 当前技术栈与版本

| 项 | 值 |
| --- | --- |
| 框架 | Tauri 2（`tauri` 2.12.x、`@tauri-apps/cli` 2.12.x） |
| 前端 | React 19 + Vite |
| 安装器 | NSIS（自定义模板 `src-tauri/installer/installer.nsi` + 自绘 SetupUi） |
| 版本号 | `4.0.0`（`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`package.json` 三处一致） |
| 最低 WebView2 | `109.0.0.0`（`bundle.windows.minimumWebview2Version`） |

改动前 `bundle.windows.webviewInstallMode` 为 `downloadBootstrapper`（用户端安装时联网下载约 1.8MB 的 Bootstrapper）；现已改为：

```json
"webviewInstallMode": { "type": "offlineInstaller", "silent": true }
```

## 2. 各平台运行时策略

### Windows（核心）

`offlineInstaller` 会在 **构建时** 把微软官方的 Evergreen Standalone 安装器（x64 约 214MB）嵌入 NSIS 安装包，安装时静默执行 `/silent /install`。用户端不需要网络。

- 安装包体积增加：Tauri 官方口径约 **+127MB**（压缩后）；实际离线安装器原始文件约 **214MB**（2026-10-20 实测 `Content-Length`）。
- 构建机需要联网一次，由 Tauri CLI 自动下载并缓存；也可用 `tools/fetch-webview2-offline.ps1` 预先获取、留档与校验。
- 仓库内**不提交**该二进制（见 `.gitignore` 的 `/src-tauri/webview2/`）。

**arm64 取舍**：Tauri CLI 的离线安装器只内置 x86 / x64 两个官方短链（`2099617` / `2124701`），**没有 ARM64 分支**；且 NSIS 安装器本身在 ARM 上是 x86 模拟运行。若将来发布原生 ARM64 版本，需要：
1. 用 `fixedRuntime` 内置 ARM64 固定运行时；或
2. 单独分发微软官方 ARM64 Standalone 安装器并自行处理安装步骤。
> 注意：`linkid=2124703` 是约 1.8MB 的 **Bootstrapper**，不是 ARM64 离线包。ARM64 离线包需从微软 WebView2 下载页取得实际链接。

**fixedRuntime 备选**（内置固定版本运行时目录）：
- 体积约 **+180MB**，且版本被钉死：WebView2 的安全更新不再随系统自动升级，每次升级都要重新构建、重新分发整个安装包。
- 仅建议用于完全隔离/内网、且能接受定期人工升级的场景；普通用户仍推荐 `offlineInstaller`。

### Linux

WebKitGTK 是系统共享库，**无法像 WebView2 一样内置进安装包**：
- `.deb` 必须声明依赖：`libwebkit2gtk-4.1-0`、`libgtk-3-0`（Tauri 2 使用 WebKitGTK 4.1）。由 apt 从发行版仓库安装。
- AppImage 会打包大部分依赖库，但仍依赖宿主机的 glibc、图形驱动与部分系统库，不能保证“拷贝即用”。
- 离线内网部署时，需自建 apt 源或预先下载对应 `.deb` 依赖链。

### macOS

使用系统自带的 **WKWebView**，无需也不允许内置。只需保证最低系统版本：**macOS 10.15 (Catalina) 及以上**（Tauri 2 的下限）。

## 3. 用户端安装流程（Windows）

1. 双击 `AniBox_4.0.0_x64-setup.exe`。
2. 自绘安装器选择安装模式（当前用户 / 所有用户）与路径。
3. 安装器检测注册表中的 WebView2 版本：
   - **未安装** → 解压并静默运行内置的 `MicrosoftEdgeWebView2RuntimeInstallerX64.exe`，**无需联网**。
   - 已安装且版本 ≥ 109 → 跳过。
4. 安装完成后直接启动 AniBox。

> 说明：当前自定义 NSIS 模板只在“注册表里完全没有 WebView2”时使用内置离线包；若检测到旧版本（<109），模板会尝试通过 Edge Update 联网升级。**纯离线环境**遇到旧版本时，请先手动运行离线安装器（见 §5 修复包）。

## 4. 构建与产出

```powershell
# 1) 只检查官方链接是否可用（不下载 200MB）
pwsh -NoProfile -File .\tools\fetch-webview2-offline.ps1 -HeadOnly

# 2) 下载 x64 离线包到 src-tauri/webview2/ 并打印 SHA-256
pwsh -NoProfile -File .\tools\fetch-webview2-offline.ps1

# 3) 可选：校验固定指纹（发布说明中给出）
pwsh -NoProfile -File .\tools\fetch-webview2-offline.ps1 -ExpectedSha256 <SHA256>

# 4) 正式打包（内部会执行 npm run tauri build -- --bundles nsis）
.\release.cmd
```

- Tauri 在 `tauri build` 时会自行下载同一离线安装器到自己的缓存（Windows 通常为 `%LOCALAPPDATA%\tauri`），不需要手工指定路径。
- 产物：`src-tauri/target/release/bundle/nsis/AniBox_4.0.0_x64-setup.exe`，`tools/build-installer.ps1` 会复制到 `release/` 并打印 SHA-256。
- 离线构建机：可先在有网机器上跑步骤 2，把 `src-tauri/webview2/` 整个目录拷到离线构建机，再确认 Tauri 缓存可用（首次构建仍需能取到文件；若完全无网，需提前填充 CLI 缓存）。

### 下载链接（2026-10-20 HEAD 实测）

| 架构 | 短链 | 落地文件名 | 大小 |
| --- | --- | --- | --- |
| x64 | `https://go.microsoft.com/fwlink/?linkid=2124701` | `MicrosoftEdgeWebView2RuntimeInstallerX64.exe` | 214,156,024 B |
| x86 | `https://go.microsoft.com/fwlink/?linkid=2099617` | `MicrosoftEdgeWebView2RuntimeInstallerX86.exe` | 189,916,408 B |
| arm64 | 无稳定短链，需从官方下载页取得 | `MicrosoftEdgeWebView2RuntimeInstallerARM64.exe` | — |
| Bootstrapper（在线兜底） | `https://go.microsoft.com/fwlink/p/?LinkId=2124703` | `MicrosoftEdgeWebview2Setup.exe` | 1,873,104 B |

> 短链始终指向“最新版”，版本与 SHA-256 会随时间变化；**以当次下载结果为准**，并在发布说明中记录指纹。

### 校验方式

```powershell
# 文件指纹
Get-FileHash .\src-tauri\webview2\MicrosoftEdgeWebView2RuntimeInstallerX64.exe -Algorithm SHA256

# 确认是微软签名（应为 Microsoft Corporation）
Get-AuthenticodeSignature .\src-tauri\webview2\MicrosoftEdgeWebView2RuntimeInstallerX64.exe |
  Format-List Status, SignerCertificate

# 安装包本身
Get-FileHash .\release\AniBox_4.0.0_x64-setup.exe -Algorithm SHA256
```

## 5. 纯离线环境部署 / 修复包

建议随安装包额外分发一个“运行时修复包”（同一个离线安装器）：

```powershell
# 静默安装/修复（需要时以管理员身份运行可安装到所有用户）
.\MicrosoftEdgeWebView2RuntimeInstallerX64.exe /silent /install
```

内网分发时，把 `AniBox_4.0.0_x64-setup.exe` 与该离线安装器放在同一共享目录即可，用户端不需要访问 `go.microsoft.com`。

## 6. 0x80010108（RPC_E_DISCONNECTED）成因与排查

`0x80010108` 是 COM/RPC 连接被断开，不是“WebView2 未安装”。常见触发：

1. **WebView2 浏览器进程被杀**：安全软件 / 电脑管家 / 优化工具结束 `msedgewebview2.exe`，或注入 DLL 导致崩溃。
2. **运行时正在升级/卸载**：安装或更新 WebView2 时 AniBox 正在运行，进程间 RPC 断开。
3. **用户数据目录损坏**：`%LOCALAPPDATA%\com.anibox.app\EBWebView` 下的缓存/配置损坏。
4. **组件缺失或安装不完整**：只装了部分运行时，或注册表存在但文件被删。
5. **会话/权限异常**：以服务、Session 0、受限令牌或跨用户会话启动，导致 RPC 端点不可用。

排查顺序（从低风险到高风险）：

1. 完全退出 AniBox（含托盘），重新启动；仍失败则重启系统（清掉残留 RPC 端点）。
2. 确认安全软件没有拦截 `msedgewebview2.exe` / AniBox；把 AniBox 安装目录与 `%LOCALAPPDATA%\com.anibox.app` 加入白名单。
3. 关闭 AniBox 后重命名/删除用户数据目录 `%LOCALAPPDATA%\com.anibox.app\EBWebView`（会重置登录态与缓存）。
4. 运行修复包：`MicrosoftEdgeWebView2RuntimeInstallerX64.exe /silent /install`（同版本会修复，新版本会升级）。
5. 仍失败时检查注册表 `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` 的 `pv` 与 `%ProgramFiles(x86)%\Microsoft\EdgeWebView\Application\<ver>\msedgewebview2.exe` 是否一致；不一致说明安装损坏，先 `/uninstall` 再 `/install`。
6. 极端场景（无法修复、必须锁定版本）：改用 `fixedRuntime` 内置固定运行时。

> 应用内已有 `check_webview2` 命令与设置页状态展示；仓库根目录 `安装WebView2.cmd` 是**联网**兜底入口（Bootstrapper），离线场景请用本文的离线安装器。

## 7. 相关文件

- `src-tauri/tauri.conf.json` —— `webviewInstallMode: offlineInstaller`
- `tools/fetch-webview2-offline.ps1` —— 下载 / HEAD 校验 / SHA-256 校验
- `tools/ensure-webview2.ps1`、`安装WebView2.cmd` —— 在线兜底（Bootstrapper）
- `src-tauri/installer/installer.nsi` —— 自定义 NSIS 模板中的 `Section WebView2`
- `src-tauri/src/webview2.rs` —— 应用内运行时检测
