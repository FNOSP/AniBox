// [lc-2075] 构建期补丁：给上游 fntv-mpv-config 的 portable_config/scripts/fntv_shader.lua
// 打「剔除运行时玻璃 shader」修复。
//
// 背景：modernz 玻璃皮肤的 modernz-glass.glsl 是运行时状态（modernz 每次启动按需
// 挂载/摘除，glsl-shader-opts 参数由它独占管理），但 fntv_shader 的兜底持久化
// （lc-1801 observer）会把它连同用户预设一起固化进 anibox-shaders.conf；之后每次
// 启动 fntv_shader 都会恢复这条 modernz 管不到的副本——陈旧路径 / 重复实例，
// 个别显卡驱动上整体偏色（外部用户反馈「画面发蓝」，原生 uosc 皮肤无此现象，
// 本机 SDR 环境无法复现）。修复 = 持久化/恢复一律剔除该文件，并在恢复时顺手
// 治愈已被污染的持久化文件。
//
// third_party/ 不入库，CI 每次从 QiaoKes/fntv-mpv-config release 拉包覆盖，
// 因此本修复以「下载后打补丁」的方式随构建生效；上游收录后本脚本会自动空转。
import fs from "node:fs";
import path from "node:path";
import url from "node:url";

const root = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const target = path.join(root, "third_party", "fntv-mpv", "portable_config", "scripts", "fntv_shader.lua");

if (!fs.existsSync(target)) {
  console.error(`[patch-fntv-mpv] 目标不存在：${target}`);
  process.exit(1);
}
let text = fs.readFileSync(target, "utf8");
if (text.includes("[lc-2075]")) {
  console.log("[patch-fntv-mpv] 已含 lc-2075 修复，跳过");
  process.exit(0);
}
// 上游文件为 CRLF 时把靶串换算成同款行尾，避免 CI 上匹配失败
const crlf = text.includes("\r\n");
const adapt = (s) => (crlf ? s.replace(/\n/g, "\r\n") : s);

// 替换条目：找不到靶串直接报错（上游结构变了要人工跟，不能静默跳过）
const edits = [
  {
    name: "strip_runtime 定义（parse_persisted 之后）",
    from: `-- 读取两份候选文件中「最新」的一份（写入失败时会落到兜底文件，不能永远读旧的首选文件）。
local function read_persisted()`,
    to: `-- [lc-2075] 剔除 modernz 的运行时玻璃 shader（modernz-glass.glsl）。
-- 它是皮肤玻璃效果的运行时状态：modernz 每次启动按需挂载/摘除，参数也由它独占
-- 管理（glsl-shader-opts 全局共享）。兜底持久化曾把它连同用户预设一起固化，
-- 之后每次启动 fntv_shader 都会恢复一条 modernz 管不到的副本（陈旧路径 / 重复
-- 实例，个别显卡驱动上整体偏色——外部用户「画面发蓝」）。持久化与恢复一律剔除。
local function strip_runtime(list)
    if not list or list == '' then return list end
    local out, changed = {}, false
    for entry in list:gmatch('[^;]+') do
        if entry:lower():match('modernz%-glass%.glsl$') then
            changed = true
        else
            out[#out + 1] = entry
        end
    end
    if not changed then return list end
    return table.concat(out, ';')
end

-- 读取两份候选文件中「最新」的一份（写入失败时会落到兜底文件，不能永远读旧的首选文件）。
local function read_persisted()`,
  },
  {
    name: "apply_list 落盘剔除",
    from: `    local saved = true
    if persist then
        saved = write_persisted(list or '')
    end`,
    to: `    local saved = true
    if persist then
        saved = write_persisted(strip_runtime(list or ''))
    end`,
  },
  {
    name: "兜底持久化 observer 剔除",
    from: `    debounce = mp.add_timeout(0.6, function()
        local cur = mp.get_property('glsl-shaders') or ''
        if cur ~= persisted_value then`,
    to: `    debounce = mp.add_timeout(0.6, function()
        -- [lc-2075] 落盘前剔除 modernz 运行时玻璃 shader：它由 modernz 自管，
        -- 固化进持久化文件后每次启动都会被本脚本复活（陈旧路径/重复实例）。
        local cur = strip_runtime(mp.get_property('glsl-shaders') or '')
        if cur ~= persisted_value then`,
  },
  {
    name: "启动恢复剔除 + 治愈持久化文件",
    from: `local restored, restored_from = read_persisted()
persisted_value = restored
if restored ~= nil then`,
    to: `local restored, restored_from = read_persisted()
-- [lc-2075] 恢复前剔除运行时玻璃 shader；旧版已固化的持久化文件在此顺手治愈（重写为干净链）。
local healed = strip_runtime(restored)
if restored ~= nil and healed ~= restored then
    restored = healed
    write_persisted(restored)
    msg.info('[fntv_shader] 已从持久化剔除运行时玻璃 shader（治愈 anibox-shaders.conf）')
end
persisted_value = restored
if restored ~= nil then`,
  },
];

for (const edit of edits) {
  const from = adapt(edit.from);
  const to = adapt(edit.to);
  if (!text.includes(from)) {
    console.error(`[patch-fntv-mpv] 未找到靶串（${edit.name}），上游文件结构可能已变化，需要人工跟`);
    process.exit(1);
  }
  // 用函数形式替换：替换串含 $' / $& 等序列时 String.replace 会做模式展开，把产物搅烂
  text = text.replace(from, () => to);
}

fs.writeFileSync(target, text);
console.log(`[patch-fntv-mpv] 已修复：${path.relative(root, target)}（4 处，lc-2075）`);
