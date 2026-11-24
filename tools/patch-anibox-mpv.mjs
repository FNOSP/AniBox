#!/usr/bin/env node
// [lc-2341/2338] 构建期补丁：把两项「借自 Tigerest-Theater 思路、我们自己重写」的
// 改动打进上游配置包里的两个脚本。
//
// 为什么必须是补丁而不是直接改文件：`third_party/` 不入库，CI 每次从自托管 R2
// 配置包（fntv-mpv-config.zip）解压覆盖。只改本地文件 = 本地能用、发布版全丢。
// 已有先例：patch-fntv-mpv.mjs（lc-2075）、patch-modernz-contrast.mjs（lc-2328）。
//
// 两个补丁：
//   ① modernz.lua 视频详情弹窗加「处理顺序」一行 —— 数据由 scripts/fntv_pipeline.lua
//      采集写 user-data/anibox/pipeline（那个脚本走 tools/mpv-scripts 正常同步）。
//   ② fntv_interp.lua 追加 ResumePrime（空闲 >5s 恢复时先 seek 回原位预热推理图）。
//
// 两个都必须**幂等**、**靶串精确**（找不到就报错退出，不静默跳过 —— 上游结构
// 变了要人工跟，静默跳过的后果是发布版少功能且没人知道）。
import fs from "node:fs";
import path from "node:path";
import url from "node:url";

const root = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const SCRIPTS = path.join(root, "third_party", "fntv-mpv", "portable_config", "scripts");

if (!fs.existsSync(SCRIPTS)) {
  console.log(`[patch-anibox-mpv] ${SCRIPTS} 不存在（配置包未就位），跳过`);
  process.exit(0);
}

/** 一次替换；已含标记则跳过；找不到靶串则记录错误。 */
function patch(file, marker, edits) {
  const target = path.join(SCRIPTS, file);
  return patchFile(target, marker, edits);
}

function patchFile(target, marker, edits) {
  if (!fs.existsSync(target)) {
    console.error(`[patch-anibox-mpv] 目标不存在：${target}`);
    return false;
  }
  let text = fs.readFileSync(target, "utf8");
  if (text.includes(marker)) {
    console.log(`[patch-anibox-mpv] ${path.basename(target)} 已含 ${marker}，跳过`);
    return true;
  }
  // 上游文件可能是 CRLF：把靶串/替换串的行尾换算成同款，避免匹配失败
  const crlf = text.includes("\r\n");
  const fix = (s) => (crlf ? s.replace(/\r?\n/g, "\r\n") : s);
  for (const e of edits) {
    const from = fix(e.from);
    const idx = text.indexOf(from);
    if (idx < 0) {
      console.error(`[patch-anibox-mpv] ${path.basename(target)}：找不到靶串（${e.name}），上游结构可能已变化，需要人工跟`);
      return false;
    }
    if (text.indexOf(from) !== text.lastIndexOf(from)) {
      console.error(`[patch-anibox-mpv] ${path.basename(target)}：靶串不唯一（${e.name}），拒绝模糊替换`);
      return false;
    }
    text = text.slice(0, idx) + fix(e.to) + text.slice(idx + from.length);
  }
  fs.writeFileSync(target, text, "utf8");
  console.log(`[patch-anibox-mpv] 已修复：${path.basename(target)}（${edits.length} 处，${marker}）`);
  return true;
}

let ok = true;

// ── ① modernz.lua：详情弹窗「处理顺序」行 ─────────────────────────────────
ok = patch("modernz.lua", "lc-2341", [
  {
    name: "detail 弹窗 插帧行之后插入处理顺序行",
    from: `        row("插帧", interp_desc or "关")
`,
    to: `        row("插帧", interp_desc or "关")
        -- [lc-2341] 处理顺序流（借 Tigerest-Theater 的 proMode「处理顺序」思路）：
        -- 一帧从输入到输出经过的每一站压成一行，比上面这些分散的行更直观地
        -- 说明「现在这条画质链路到底在干什么」。数据由 scripts/fntv_pipeline.lua
        -- 采集后写 user-data/anibox/pipeline —— 采集放独立脚本是必须的：
        -- 本文件主 chunk 已顶到 LuaJIT 的 200 local 上限，不能再加逻辑。
        local pipeline = mp.get_property_native("user-data/anibox/pipeline")
        if type(pipeline) == "string" then pipeline = pipeline:gsub('"', '') end
        row("处理顺序", pipeline)
`,
  },
]) && ok;

// ── ② fntv_interp.lua：ResumePrime ────────────────────────────────────────
const PRIME = `
-- [lc-2342] ResumePrime —— 长时间暂停后恢复时，先 seek 回原位置把推理图「预热」再放。
--
-- 思路借自 Tigerest-Theater 的 InterpolationPolicy::ResumePrime（原实现是 C++，
-- 这里按同样语义重写为 Lua，不复制其代码）：
-- 推理图（ncnn/TRT 一类）闲置几秒后第一次喂帧会明显偏慢，实测表现是恢复播放
-- 瞬间视频落后、连丢若干帧（用户看到「暂停久了点播放，画面卡一下」）。
-- 做法：暂停 → 保持暂停的前提下 seek 回当前位置 → 等首帧真正渲染出来 → 再放。
-- 那几帧慢推理发生在暂停期间，用户只感觉到「点播放后画面已经在动了」。
--
-- 四条边界，缺一条就会变成新 bug：
--   ① 只在我们自己的引擎（rife/mvtools/svp）挂载时做：内置平滑没有推理图，纯属白等；
--   ② 空闲阈值 5s（低于这个时长图还在缓存里，prime 反而更慢）；
--   ③ 有超时（3s）：seek 失败/片源不支持时不能把用户永远卡在暂停里，到点无条件放行；
--   ④ prime 期间的暂停必须与用户手动暂停区分开：自己写的 pause 不能被当成
--      「用户想暂停」，否则下一次恢复会读到我们自己写的值（隐蔽的自锁）。
local PRIME_IDLE = 5.0
local PRIME_TIMEOUT = 3.0
prime_paused_at = nil
prime_in_flight = false
prime_token = 0

local function prime_eligible()
    return interp_on and (active_engine == 'rife' or active_engine == 'mvtools'
        or active_engine == 'svp')
end

local function vf_pass_count()
    local n = 0
    pcall(function()
        for _, p in ipairs((mp.get_property_native('vo-passes') or {}).fresh or {}) do
            n = n + (p.count or 0)
        end
    end)
    return n
end

mp.observe_property('pause', 'bool', function(_, paused)
    if prime_in_flight then return end
    if paused then
        prime_paused_at = mp.get_time()
        return
    end
    local since = prime_paused_at
    prime_paused_at = nil
    if not since then return end
    local idle = mp.get_time() - since
    if idle < PRIME_IDLE or not prime_eligible() then return end
    if mp.get_property_number('duration') == nil then return end
    local pos = mp.get_property_number('time-pos')
    if not pos then return end

    prime_in_flight = true
    prime_token = prime_token + 1
    local token = prime_token
    local released = false
    local function release(why)
        if released then return end
        released = true
        if token ~= prime_token then return end
        prime_in_flight = false
        pcall(function() mp.set_property_bool('pause', false) end)
        mp.log('info', ('[fntv_interp] 恢复预热结束（%s，空闲 %.1fs）'):format(why, idle))
    end
    -- 先重新暂停（用户刚点了播放，这里立刻接住），再 seek 到原位把推理图填满
    pcall(function() mp.set_property_bool('pause', true) end)
    pcall(function() mp.commandv('seek', pos, 'absolute', 'exact') end)
    local base = vf_pass_count()
    local waited = 0
    local poll
    poll = mp.add_periodic_timer(0.05, function()
        waited = waited + 0.05
        if vf_pass_count() > base then
            poll:kill()
            mp.add_timeout(0.12, function() release('首帧就绪') end)
        elseif waited >= PRIME_TIMEOUT then
            poll:kill()
            release('超时放行')
        end
    end)
    mp.log('info', ('[fntv_interp] 空闲 %.1fs 后恢复 → 预热推理图（%.2fs）'):format(idle, pos))
end)

-- seek / 换文件时取消在途的 prime：否则令牌失效后 pause 会永久停在 true
mp.register_event('seek', function()
    if prime_in_flight then
        prime_token = prime_token + 1
        prime_in_flight = false
        pcall(function() mp.set_property_bool('pause', false) end)
    end
end)
mp.register_event('end-file', function()
    prime_token = prime_token + 1
    prime_in_flight = false
    prime_paused_at = nil
end)
`;

ok = patch("fntv_interp.lua", "lc-2342", [
  {
    name: "文件尾追加 ResumePrime",
    from: `mp.log('info', '[fntv_interp] 已加载 (engine=' .. tostring(conf.engine) .. ', default_on=' .. tostring(conf.default_on) .. ')')`,
    to: `mp.log('info', '[fntv_interp] 已加载 (engine=' .. tostring(conf.engine) .. ', default_on=' .. tostring(conf.default_on) .. ')')
${PRIME}`,
  },
]) && ok;

// ③ fntv_aq 稳健帧率（原计划的 lc-2339）**未实装**：写完自证是 no-op ——
// 只要 container-fps > 0，无论 estimated-vf-fps 是多少，返回值恒等于 container，
// 等于什么都没改。真正的口径修正需要 mpv 逐帧 pts 差分，成本远超收益（本文件的
// bpppf 只用于开关去色带，不是精确预算）。留此说明，避免后来者按老清单再走一遍。

// ── ④ mpv-user.conf：去色带参数（动漫档 3/24/12/12）──────────────────────
// 取值来自 Tigerest-Theater 的 mpv.default.conf（数值本身不受版权保护）。
// 我们原来是 mpv 默认的 iterations=2，其余全走默认；实测证明调到这一档的代价
// 可以忽略，见下面注释里的 A/B 数字。
ok = patchFile(
  path.join(root, "third_party", "fntv-mpv", "portable_config", "mpv-user.conf"),
  "lc-2343",
  [
    {
      name: "渲染预设块改去色带参数",
      from: `deband=yes
deband-iterations=2
hdr-compute-peak=yes`,
      to: `deband=yes
# [lc-2343] 去色带参数取自 Tigerest-Theater 的动漫档（3/24/12/12，纯取值不受许可限制）。
# 本机 A/B 实测（RTX 5070，vo=gpu-next + ewa_lanczossharp 不变）：
#   1080p 输出：it2 0.410ms / it3(新档) 0.418ms / it4 0.425ms
#   4K   输出：it2 1.995ms / it4 2.194ms
# 即从最保守调到最激进，每帧只多 8μs~0.2ms —— 成本可以忽略；而默认 2 次迭代
# 在天空/暗部渐变上留下的色带是能看出来的。所以直接 3 次 + threshold 48 / range 18 / grain 6。
deband-iterations=3
deband-threshold=48
deband-range=18
deband-grain=6
hdr-compute-peak=yes`,
    },
  ],
) && ok;

process.exit(ok ? 0 : 1);
