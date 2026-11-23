#!/usr/bin/env node
/**
 * tools/install-mpv-scripts.mjs
 *
 * 把仓库里可入库的 mpv 附属脚本（`tools/mpv-scripts/*.lua`）安装进
 * `third_party/fntv-mpv/portable_config/scripts/`。
 *
 * 为什么需要这一步：`third_party/` 按仓库规则不入库，mpv 的 portable_config
 * 来自自托管 R2 配置包（CI 每次下载覆盖）。我们自己的脚本如果只放在 third_party
 * 里，新 clone / CI 上就会凭空消失，而 R2 包又是按需手工重打的。
 * 所以：**源文件入库**（tools/mpv-scripts），每次 dev / build 前由本脚本同步过去。
 *
 * 幂等：内容相同就跳过。缺 third_party 目录（纯前端开发）时安静跳过。
 */
import fs from "node:fs";
import path from "node:path";
import url from "node:url";

const root = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const SRC = path.join(root, "tools", "mpv-scripts");
const DEST = path.join(root, "third_party", "fntv-mpv", "portable_config", "scripts");

if (!fs.existsSync(SRC)) {
  console.log("[mpv-scripts] 没有 tools/mpv-scripts，跳过");
  process.exit(0);
}
if (!fs.existsSync(DEST)) {
  console.log(`[mpv-scripts] ${DEST} 不存在（third_party 未就位），跳过`);
  process.exit(0);
}

let copied = 0;
for (const name of fs.readdirSync(SRC)) {
  if (!name.endsWith(".lua")) continue;
  const from = path.join(SRC, name);
  const to = path.join(DEST, name);
  const next = fs.readFileSync(from);
  if (fs.existsSync(to) && fs.readFileSync(to).equals(next)) continue;
  fs.copyFileSync(from, to);
  copied++;
  console.log(`[mpv-scripts] 安装 ${name}`);
}
console.log(copied ? `[mpv-scripts] ${copied} 个脚本已更新` : "[mpv-scripts] 已是最新");
