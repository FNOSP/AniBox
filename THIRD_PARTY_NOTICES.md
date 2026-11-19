# 第三方组件声明（Third-Party Notices）

本仓库随安装包分发以下第三方组件。源码随包提供，即满足相应许可证的源码提供义务。

AniBox 本体以 **AGPL-3.0** 授权（见仓库根目录 `LICENSE`）。下列组件各自遵循其自身许可，
不因随包分发而被 AniBox 的许可覆盖；本文件即为这些组件的归属与许可声明。

---

## 一、桌面端随包分发的二进制与运行时资源

> 这些内容**不在本仓库内**（`third_party/` 按入库规则不入库），但会由
> `tools/stage-bundle-resources.mjs` 复制进 `src-tauri/bundle-resources/` 并最终打进
> Windows 安装包（实测 staging 约 287 MB / 1014 个文件）。安装后的位置为
> `<安装目录>\third_party\`。

### mpv（播放内核，含 mpv.exe / mpv.com）

- 上游项目：https://github.com/mpv-player/mpv
- 本项目所用构建：[Yaozhil/mpv-Yaozhi](https://github.com/Yaozhil/mpv-Yaozhi)（第三方预编译版本，**非本项目构建 / 编译**）
- 许可证：**GPL-2.0-or-later / LGPL-2.1-or-later**（mpv 各文件混合授权，详见上游）
- 版权：归 mpv 项目及其构建者所有。本项目**不声称拥有 mpv 代码**。
- 源码获取：https://github.com/mpv-player/mpv 、https://github.com/Yaozhil/mpv-Yaozhi
- 分发说明：`mpv.exe` 与 `portable_config/` 下的脚本 / 着色器作为**独立程序**随包共存，
  通过 IPC 管道与本程序通信。若你再次分发本安装包，必须一并保留 mpv 的 GPL/LGPL 义务
  （含对应的源码获取途径）。

### portable_config（mpv 配置、脚本、着色器、字体、徽章资产）

位于 `third_party/fntv-mpv/portable_config/`，内容与许可如下。各文件均保留原始头部声明。

#### mpv Lua 脚本（`scripts/`、`script-modules/`）

脚本清单、作者来源与逐项许可见随包自带的
`portable_config/scripts/THIRD-PARTY-LICENSES.md`（**该文件随包分发**）。
其中需特别指明的 copyleft 项：

| 脚本 | 许可 |
| --- | --- |
| `modernz.lua`（ModernGlass 液态玻璃皮肤，基于 ModernZ / ModernX / mpv 官方 osc.lua） | **LGPL-2.1** |
| `uosc/`（uosc 界面框架） | **LGPL-2.1**（副本：`scripts/uosc/LICENSE.LGPL`） |
| `autoload.lua` | 随 mpv 官方脚本，GPL/LGPL |
| `stats.lua` | 随 mpv 官方脚本，GPL/LGPL |
| `thumbfast.lua` | **MPL-2.0** |
| `undoredo.lua` | BSD-2-Clause |
| `chapterskip.lua`、`sub-assrt.lua` | MIT |
| `autosubsync/` | 见 `scripts/autosubsync/LICENSE` |
| `uosc_danmaku/` 及其 `modules/aes.lua` | 上游各自声明（aes.lua 为 Apache-2.0） |

#### GLSL 着色器（`shaders/`，59 个）

来源与逐项许可见随包自带的 `portable_config/shaders/LICENSE-shaders.txt`（**该文件随包分发**）。
其中 `FSRCNNX_*.glsl` 为 **LGPL-3.0**（igv），Anime4K 系列为 MIT（bloc97），其余多为 MIT。

#### 字体（`fonts/`）

| 字体 | 许可 | 随包许可文件 |
| --- | --- | --- |
| Inter（Medium / SemiBold） | SIL Open Font License 1.1 | `fonts/Inter-LICENSE.txt` |
| Noto Sans CJK Bold | SIL Open Font License 1.1（Google / Adobe） | 见上游 |
| Material Design Iconic Font | SIL OFL 1.1 / MIT（zavoloklom） | 见上游 |
| Material Icons Round | Apache-2.0（Google） | 见上游 |
| uosc_icons / uosc_textures | 随 uosc，见其仓库 | 见上游 |

#### 起播格式徽章资产（`script-assets/startup-format-logos/`，697 个文件）

- 来源：Yaozhil/mpv-Yaozhi 1.0.6-2 整合包
- 许可：**MIT**（Copyright (c) 2026 Yaozhil）
- 声明文件：`script-assets/startup-format-logos/README.md`（随包分发）

#### Mozilla CA 证书包

- 文件：`resources/ca-bundle.crt`（随包分发，约 224KB）
- 来源：Mozilla CA 证书库（取自 Git for Windows 的 OpenSSL `ca-bundle.crt`）
- 许可证：MPL-2.0（证书数据本身为公开信息）
- 用途：给 mpv 子进程注入 `SSL_CERT_FILE`，修复 Windows 上 mpv/OpenSSL 找不到系统根证书导致
  HTTPS 直播源 TLS 校验失败、播放器黑屏的问题。**不关闭 TLS 校验。**

---

## 二、短剧车道（`third_party/duanju/`）

> Windows amd64 随包分发。安装后的位置为 `<安装目录>\third_party\duanju\`。

### duanju_core.dll（guoapp「红果鉴」Go c-shared 核心）

- 上游仓库：https://github.com/Xanerix/guoapp
- 许可证：**GNU General Public License v3.0（GPL-3.0）**
- 源码获取：上述上游仓库；本项目通过 `tools/fetch-duanju-core.py` 从其官方 Release 拉取
- 使用方式：以 FFI（`DuanjuRequest(JSON) -> JSON` 单入口）方式调用，站源逻辑全部在该核内
- 兼容性说明：guoapp 以 GPL-3.0 发布（含 "or any later version"），可并入本项目的 AGPL-3.0；
  本项目对其**未作修改**，按原样随包分发

### 站源模块（`third_party/duanju/modules/<id>/`）

- 来源：guoapp 官方在线模块仓库（`guoapp-modules/index`，按 index 的 sha256 逐文件校验）
- 许可：随 guoapp 上游，GPL-3.0

### 伴随的 MinGW 运行时 DLL

`libgcc_s_seh-1.dll`、`libstdc++-6.dll`、`libwinpthread-1.dll`、`libde265-0.dll`
均为 guoapp 发布包内自带的运行时依赖，随该包原样分发：

| DLL | 许可 |
| --- | --- |
| `libgcc_s_seh-1.dll` / `libstdc++-6.dll` | GPL-3.0 **with GCC Runtime Library Exception** |
| `libwinpthread-1.dll` | MinGW-w64 runtime（MIT / BSD 风格） |
| `libde265-0.dll`（HEVC 解码，用于 HEIC 封面转码） | **LGPL-3.0**（https://github.com/strukturag/libde265） |

---

## 三、内置弹幕服务

### danmu_api

- 名称：danmu_api（LogVar 弹幕 API）
- 上游仓库：https://github.com/huangxd-/danmu_api
- 许可证：**GNU Affero General Public License v3.0（AGPL-3.0）**
- 许可证全文：`src-tauri/resources/danmu-api/LICENSE`
- 源码位置：`src-tauri/resources/danmu-api/`
- 版本：上游 `Globals.VERSION = 1.21.10`（快照 commit `619612b`，2026-11-14）
- 使用方式：作为「内置自建弹幕源」随应用本地运行，仅监听 `127.0.0.1:22352`，不提供任何公共服务。

### 本仓库对上游源码所做的最小修改

1. `danmu_api/server.js`
   - 代理端口由硬编码 `5321` 改为可用 `DANMU_API_PROXY_PORT` 覆盖（默认仍为 5321）。
   - 主服务与代理服务支持 `DANMU_API_HOST` 指定监听地址（默认行为不变）。
2. `danmu_api/utils/server-listen-util.js`
   - `listenOnAllInterfaces` 支持显式 `host`（默认行为不变）。
3. `danmu_api/configs/globals.js`
   - 正向代理回退地址使用 `DANMU_API_PROXY_PORT`（默认仍为 5321）。
4. `package.json`
   - `esbuild` 从 `dependencies` 移至 `devDependencies`（仅构建工具，运行期不需要）；
     删除 `build-forward-widget*` 脚本及其脚本文件。
5. 删除与本地运行无关的部署目录/脚手架：`.github/`、`node-functions/`、`forward/`、`netlify/`、
   `danmu_api/tests/`、`Dockerfile`、`edgeone.json`、`netlify.toml`、`vercel.json`、`wrangler.toml`、
   `README.hf.md`、`.pr_agent.toml`。

> 上游源码以 AGPL-3.0 授权；本仓库对它的分发与修改遵循同一许可证。若你再次分发本安装包，
> 请保留本声明与 `src-tauri/resources/danmu-api/LICENSE`，并提供对应源码。

### 内置 danmu_api 的 npm 运行时依赖

`src-tauri/resources/danmu-api/node_modules/` 由 `npm install --omit=dev` 生成，各依赖许可证见其包内 `LICENSE`。主要依赖：

| 包 | 许可证 |
| --- | --- |
| `@dan-uni/dan-any` | LGPL-3.0-or-later |
| `dotenv` | BSD-2-Clause |
| `chokidar` | MIT |
| `node-fetch` | MIT |
| `https-proxy-agent` | MIT |
| `opencc-js` | MIT AND Apache-2.0 |
| `pako` | MIT AND Zlib |
| `brotli` | MIT |
| `redis` | MIT |

### Node.js

内置实例使用用户机器上的 Node.js 运行时（系统 PATH 或 `ANIBOX_NODE_PATH`），本仓库不随包分发 Node.js 二进制。

## 四、编译期依赖

### zhconv（Rust crate）

- 名称：zhconv-rs（`zhconv` crate）
- 上游仓库：https://github.com/Gowee/zhconv-rs
- 许可证：**GPL-2.0-or-later**（可并入本项目的 AGPL-3.0）
- 使用方式：编译期内嵌 MediaWiki 简繁转换表（仅启用 `mediawiki-hans`），用于把播放器展示标题
  （mpv `force-media-title` / 整季 `#EXTINF` / PotPlayer 标题）从繁体归一化为简体，运行时无网络请求。
- 转换表来源：MediaWiki `ZhConversion.php`（GPL-2.0-or-later）。

## 五、源码引用的开源实现

以下项目在**编写时参考了其实现思路**（架构 / 协议 / 规则格式），代码为本项目自行实现，不构成对该项目的源码分发：

| 项目 | 许可 | 引用范围 |
| --- | --- | --- |
| [Predidit/Kazumi](https://github.com/Predidit/Kazumi) | GPL-3.0 | 在线资源（规则源）的规则格式与直链嗅探思路，见 `src-tauri/src/webrs.rs` |
| [Xanerix/guoapp](https://github.com/Xanerix/guoapp) | GPL-3.0 | 短剧站源协议；其编译产物 `duanju_core.dll` 作为独立组件随包（见第二节） |
| [akiralereal/iptv](https://github.com/akiralereal/iptv) | GPL-3.0 | IPTV 直播源聚合数据（仅消费其部署实例，发现页已注明来源） |

> 上述项目均为 **GPL-3.0**，与本项目的 AGPL-3.0 兼容（GPLv3 第 13 条允许并入 AGPLv3）。
- 用途：给 mpv 子进程注入 `SSL_CERT_FILE`，修复 Windows 上 mpv/OpenSSL 找不到系统根证书导致
  HTTPS 直播源 TLS 校验失败、播放器黑屏的问题。**不关闭 TLS 校验。**
