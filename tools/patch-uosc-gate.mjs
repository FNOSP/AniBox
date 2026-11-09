// [lc-2103] 构建期补丁：给 uosc/main.lua 加**液态玻璃皮肤门控**。
//
// 问题：玻璃皮肤下 uosc 底栏与 modernz 玻璃栏叠画成「双 UI」。
// `--script-opts-append=uosc-disable_elements=...` 这条路走不通 —— 追加值会被
// script-opts/uosc.conf 里的同名键（disable_elements=audio_indicator）覆盖，且
// 会话复用时不会重新读取。
//
// 修法：在 uosc 的 Initial commit 之前插入门控——读 anibox_skin.conf，skin=glass 时
// Manager:disable('glass', <全部可视元素>)。**不能整体 early-return**：uosc_danmaku
// 依赖 uosc 的菜单实现，整脚本退出会让播放器内弹幕菜单全部失效。
//
// third_party/ 不入库（CI 每次从上游 release 拉包覆盖），所以走「下载后打补丁」。
import fs from "node:fs";
import path from "node:path";
import url from "node:url";

const root = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const target = path.join(root, "third_party", "fntv-mpv", "portable_config", "scripts", "uosc", "main.lua");

if (!fs.existsSync(target)) {
  console.error(`[patch-uosc-gate] 目标不存在：${target}`);
  process.exit(1);
}
let text = fs.readFileSync(target, "utf8");
if (text.includes("[lc-2103]")) {
  console.log("[patch-uosc-gate] 已含 lc-2103 门控，跳过");
  process.exit(0);
}
const crlf = text.includes("\r\n");
const adapt = (s) => (crlf ? s.replace(/\n/g, "\r\n") : s);

const from = `-- Initial commit
Manager:disable('user', options.disable_elements)`;
const to = `-- [lc-2103] 液态玻璃皮肤门控：glass 时 uosc 只当**菜单服务**（uosc_danmaku 的
-- 搜索/源管理菜单走它的 menu API），全部可视元素强制禁用 —— 否则现代玻璃底栏
-- （modernz）与本底栏叠画成双 UI。
-- 为什么不能像 modernz 那样整体 return：uosc_danmaku 依赖本脚本的菜单实现，
-- 整脚本退出会让播放器内弹幕菜单全部失效。
-- 为什么不用 --script-opts-append=uosc-disable_elements：追加值会被
-- script-opts/uosc.conf 里同名键（disable_elements=audio_indicator）覆盖，
-- 且会话复用时不重新读取——实测双 UI 长期存在正是这个原因。
do
    local want = "uosc"
    local f = io.open(mp.find_config_file("script-opts/anibox_skin.conf") or "", "r")
    if f then
        for line in f:lines() do
            local v = line:match("^%s*skin%s*=%s*(%S+)")
            if v then want = v end
        end
        f:close()
    end
    if want == "glass" then
        Manager:disable('glass', 'window_border,buffering_indicator,pause_indicator,top_bar,timeline,controls,volume')
    end
end

-- Initial commit
Manager:disable('user', options.disable_elements)`;

if (!text.includes(adapt(from))) {
  console.error("[patch-uosc-gate] 未找到靶串（Initial commit 处），上游 uosc 结构可能已变化");
  process.exit(1);
}
text = text.replace(adapt(from), () => adapt(to));
fs.writeFileSync(target, text);
console.log(`[patch-uosc-gate] 已加门控：${path.relative(root, target)}（lc-2103）`);
