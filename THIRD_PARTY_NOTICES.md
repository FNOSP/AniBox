# 第三方组件声明（Third-Party Notices）

本仓库随安装包分发以下第三方组件。源码随包提供，即满足相应许可证的源码提供义务。

## danmu_api

- 名称：danmu_api（LogVar 弹幕 API）
- 上游仓库：https://github.com/huangxd-/danmu_api
- 许可证：**GNU Affero General Public License v3.0（AGPL-3.0）**
- 许可证全文：`src-tauri/resources/danmu-api/LICENSE`
- 源码位置：`src-tauri/resources/danmu-api/`
- 版本：上游 `Globals.VERSION = 1.21.6`（vendor 快照）
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

## 内置 danmu_api 的 npm 运行时依赖

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

## Node.js

内置实例使用用户机器上的 Node.js 运行时（系统 PATH 或 `ANIBOX_NODE_PATH`），本仓库不随包分发 Node.js 二进制。
