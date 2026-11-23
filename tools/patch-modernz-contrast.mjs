#!/usr/bin/env node
/**
 * [lc-2328] 构建期补丁：给上游 modernz.lua 修「透明悬浮皮肤下弹窗文字看不清」。
 *
 * 问题：默认皮肤是 clear（透明悬浮），它的弹窗底板是 `#C9CDD3` 浅灰 60%
 * （modernz.lua 里 update_glass_look 的 MODERNZ_SKIN_CLEAR 分支）。而菜单文字
 * 样式 `menu_styles()` 只写了 glass 皮肤那一套**白字**，clear 皮肤直接复用 ——
 * 浅灰底 + 白字。用户实测在亮绿色画面上整页「视频详情」几乎看不见。
 *
 * 实测对比度（纱底叠在画面上的复合色，不是理论值）：
 *   亮绿画面 rgb(181,203,187)：白字 1.72:1 → 浅黑 10.73:1
 *   白天空   rgb(217,219,223)：白字 1.39:1 → 浅黑 13.30:1
 *   暗场景   rgb(129,131,136)：白字 3.79:1 → 浅黑  4.86:1（过 AA 正文 4.5:1）
 *
 * 修法：clear 皮肤下菜单文字一律浅黑 `#10141C`；glass 皮肤保持白色。
 * **不动机板底**（用户明确要求）、**不加描边**（试过，被否：「不要描边太丑了」）。
 *
 * 三个必须避开的坑（都真踩过）：
 *   ① 不能新增文件级 `local` —— modernz 的 main chunk 已顶到 LuaJIT 的 200 个
 *      local 上限，多一个就整脚本不加载、播放器所有按钮全死。
 *   ② ASS 颜色值**不要带 `&H…&` 包裹** —— 模板里已写 `\1c&H%s&`，双层会拼成
 *      `\1c&H&H1C1410&&`，libass 解析失败并**静默回落成默认白色**（看起来像没改）。
 *   ③ 用**行级**定位而不是整块字符串匹配 —— 反斜杠转义在 JS 字符串里极易写错。
 *
 * third_party/ 不入库（CI 从自托管 R2 拉配置包覆盖），所以走「下载后打补丁」。
 */
import fs from "node:fs";
import path from "node:path";
import url from "node:url";

const root = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const target = path.join(
  root,
  "third_party",
  "fntv-mpv",
  "portable_config",
  "scripts",
  "modernz.lua"
);

if (!fs.existsSync(target)) {
  // 开发态可能还没拉配置包；构建态由 CI 先解压。缺文件不致命，跳过即可。
  console.log(`[patch-modernz-contrast] 目标不存在，跳过：${target}`);
  process.exit(0);
}

const text = fs.readFileSync(target, "utf8");
if (text.includes("[lc-2328]")) {
  console.log("[patch-modernz-contrast] 已含 lc-2328 修复，跳过");
  process.exit(0);
}

const crlf = text.includes("\r\n");
const EOL = crlf ? "\r\n" : "\n";
const lines = text.split(EOL);

// 定位 menu_styles 函数（行级，对转义不敏感）
const startIdx = lines.findIndex((l) => l.includes("local function menu_styles()"));
if (startIdx < 0) {
  console.error("[patch-modernz-contrast] 找不到 menu_styles 定义，上游结构可能已变化");
  process.exit(1);
}

// 校验它确实是我们要改的那个（含 muted_text_color 引用）
let endIdx = -1;
let sawMuted = false;
for (let i = startIdx; i < lines.length; i++) {
  if (lines[i].includes("osc_color_convert(user_opts.muted_text_color)")) sawMuted = true;
  if (lines[i].trim() === "end") {
    endIdx = i;
    break;
  }
}
if (endIdx < 0) {
  console.error("[patch-modernz-contrast] menu_styles 没有闭合 end");
  process.exit(1);
}
if (!sawMuted) {
  console.error(
    "[patch-modernz-contrast] menu_styles 里没有预期的 muted_text_color 引用，结构可能已变"
  );
  process.exit(1);
}

// 新实现。注意：全部用函数内 local（不新增文件级 local，见文件头 ①）
const replacement = [
  "local function menu_styles()",
  "    -- [lc-2328] clear（透明悬浮）皮肤下菜单文字一律**浅黑**，glass 保持白色。",
  "    -- 原因：clear 的弹窗底板是 #C9CDD3 浅灰 60%，白字在亮画面上只有 1.4~1.7:1。",
  "    -- 实测浅黑 #10141C 在同样底上 10.7~13.3:1（暗场景 4.86:1，仍过 AA）。",
  "    -- 不动机板底（用户要求）、不加描边（试过被否）。",
  "    -- 颜色值不带 &H…& 包裹（模板里已有），否则 libass 解析失败并静默回落成白色。",
  '    local primary = MODERNZ_SKIN_CLEAR and "1C1410" or "FFFFFF"',
  "    return {",
  '        title = string.format("{\\bord0\\shad0%s\\fn%s\\1c&H%s&}", ui_font.semibold_b, ui_font.semibold, primary),',
  '        heading = string.format("{\\bord0\\shad0%s\\fn%s\\1c&H%s&}", ui_font.semibold_b, ui_font.semibold, primary),',
  '        label = string.format("{\\bord0\\shad0\\b0\\fn%s\\1c&H%s&}", ui_font.regular, primary),',
  '        meta = string.format("{\\bord0\\shad0\\b0\\fn%s\\1c&H%s&}", ui_font.regular, primary),',
  "    }",
  "end",
];

lines.splice(startIdx, endIdx - startIdx + 1, ...replacement);
fs.writeFileSync(target, lines.join(EOL), "utf8");
console.log(`[patch-modernz-contrast] 已修复：${path.relative(root, target)}（lc-2328）`);
