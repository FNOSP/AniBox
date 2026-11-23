-- fntv_rtx.lua
-- AniBox · NVIDIA RTX 视频增强（RTX 超分辨率 VSR / RTX Video HDR）
-- ───────────────────────────────────────────────────────────────────────
-- 由显卡驱动在 mpv 的 d3d11vpp 滤镜里完成，零额外依赖：
--   · VSR  ：scaling-mode=nvidia + scale=N（把画面放大到屏幕分辨率，最多 4×）
--   · HDR  ：nvidia-true-hdr=yes（SDR 片源由显卡转成 HDR 输出）
--
-- 配置：script-opts/fntv_rtx.conf（应用「设置 → 播放与画质 → AI 增强」写入）
--   vsr = no|yes      默认 no
--   hdr = no|yes      默认 no
--
-- 三条硬规则（Tigerest Theater 实测结论，见 src/player/PlayerComponent.cpp）：
--   1. 滤镜必须**永远排在链尾**。插帧（RIFE/MVTools/SVP）、AI 超分、弹幕 lavfi
--      都会改 vf 链；RTX 若被甩到前面，插帧的输出会带着 HDR 全范围标签再进
--      d3d11vpp，驱动直接拒绝并让 mpv 禁用滤镜。这里观察 vf，发现不在链尾就
--      摘掉重挂。
--   2. 只在 D3D11 原生输出 + d3d11va 硬解下可用（d3d11vpp 需要 D3D11 视频处理器）。
--   3. HDR 只对 SDR 片源、且系统 HDR 已开（目标色彩空间为 PQ）时挂 —— 否则
--      显卡把 SDR 转成 HDR 再被 tone-map 回 SDR，等于白折腾。
--
-- 状态发布：user-data/fntv_rtx/hint（给播放器菜单显示）；状态变化走
-- fntv_status_stack 的左上角胶囊提示。
-- ───────────────────────────────────────────────────────────────────────

local mp = require 'mp'
local opt = require 'mp.options'

local conf = {
    vsr = 'no',
    hdr = 'no',
    -- [lc-2320] 系统 HDR 是否开着：由 Rust 起播时问 Windows（DisplayConfig）后
    -- 经 --script-opts-append 传进来。**不要**自己从 video-target-params/gamma 推断 ——
    -- mpv 在 SDR 片源上恒为 srgb，即便系统 HDR 开着也不会变 pq（实测 2026-11-23）。
    system_hdr = 'no',
}
opt.read_options(conf, 'fntv_rtx')

local LABEL = 'fntv-rtx'
local MAX_SCALE = 4.0
local MIN_SCALE = 1.05      -- 低于这个倍数不值得放大（1.05 对齐 Tigerest）
local DEBOUNCE = 0.15       -- 一次切档/切集会连续改多个属性，合并评估

local want_vsr = conf.vsr == 'yes'
local want_hdr = conf.hdr == 'yes'

-- 最近一次评估结果（给菜单/诊断用）
local last = { filter = '', reason = '', scale = 0, active = false }
-- 上一次**真正推入链里**的 filter 文本。参数变了（例如系统 HDR 刚打开）必须重挂：
-- 只看「在不在链尾」会漏掉这种变化，表现为「HDR 开关点了没反应」。
local pushed = ''
local applying = false      -- 正在写我们自己的命令，避免观察回环
local timer = nil

local function toast(text)
    pcall(function()
        mp.commandv('script-message-to', 'fntv_status_stack', 'flash', 'rtx',
            (tostring(text or ''):gsub('%s*\n%s*', ' ')), '4000')
    end)
end

local function native(name)
    return mp.get_property_native(name)
end

local function num(name)
    return mp.get_property_number(name, 0) or 0
end

-- 视频实际显示的像素尺寸：变形片源（dw > w）按显示宽度算。
local function shown_size()
    local p = native('video-params') or {}
    local w, h = p.w or 0, p.h or 0
    local dw, dh = p.dw or 0, p.dh or 0
    return math.max(w, dw), math.max(h, dh)
end

local function source_is_hdr()
    local g = native('video-params/gamma')
    return g == 'pq' or g == 'hlg'
end

-- 系统 HDR 是否开着：优先用启动时 Rust 传进来的值（准确），
-- 读不到才退回「目标色彩空间是 PQ/HLG」这个较弱信号。
local function system_hdr_on()
    local from_rust = tostring(conf.system_hdr or '') == 'yes'
    if from_rust then return true end
    -- 兜底：真在放 HDR 片源且目标也是 PQ，说明系统 HDR 必然是开的
    local g = native('video-target-params/gamma')
    return g == 'pq' or g == 'hlg'
end

-- d3d11vpp 的可用前提：D3D11 渲染上下文 + d3d11va 原生硬解。
local function d3d11_ready()
    local ctx = mp.get_property('current-gpu-context') or ''
    if ctx ~= 'd3d11' and ctx ~= 'd3d11va' then
        return false, '渲染后端不是 D3D11（' .. (ctx ~= '' and ctx or '未知') .. '）'
    end
    local hw = mp.get_property('hwdec-current') or ''
    if not hw:find('d3d11') then
        return false, '需要 d3d11va 硬解（当前：' .. (hw ~= '' and hw or '软解') .. '）'
    end
    return true
end

-- 算出这次该挂什么；返回 filter 文本（空串=什么都不挂）与原因。
local function desired_filter()
    local filter = { parts = {} }
    local function part(s) filter.parts[#filter.parts + 1] = s end
    local reasons = {}

    local ok, why = d3d11_ready()
    local video_ok = ok and (num('video-params/w') > 0) and (num('video-params/h') > 0)
    if not ok then reasons[#reasons + 1] = why end

    local scale = 0
    if want_vsr then
        if not video_ok then
            if ok then reasons[#reasons + 1] = '视频开始播放后生效' end
        else
            local sw, sh = shown_size()
            local dw = num('display-width')
            local dh = num('display-height')
            local fit = 0
            if sw > 0 and sh > 0 and dw > 0 and dh > 0 then
                fit = math.min(dw / sw, dh / sh)
            end
            if fit < MIN_SCALE then
                reasons[#reasons + 1] = string.format(
                    '视频（%d×%d）不比屏幕（%d×%d）小，不需要放大', sw, sh, dw, dh)
            else
                scale = math.min(fit, MAX_SCALE)
                part('scaling-mode=nvidia')
                part(string.format('scale=%.4f', scale))
            end
        end
    end

    if want_hdr then
        if not video_ok then
            if ok then reasons[#reasons + 1] = '视频开始播放后生效' end
        elseif source_is_hdr() then
            reasons[#reasons + 1] = '片源本身就是 HDR，不需要转换'
        elseif not system_hdr_on() then
            reasons[#reasons + 1] = '系统 HDR 未开启（Windows 设置 → 系统 → 显示 → HDR）'
        else
            part('nvidia-true-hdr=yes')
        end
    end

    return #filter.parts > 0 and ('d3d11vpp=' .. table.concat(filter.parts, ':')) or '',
        table.concat(reasons, '；'), scale
end

-- 当前 vf 链里我们的条目的位置（nil=不在链上）。
local function find_ours(vf)
    vf = vf or native('vf') or {}
    for i = 1, #vf do
        if vf[i] and vf[i].label == LABEL then
            return i, vf[i], #vf
        end
    end
end

local function remove_ours()
    if not find_ours() then return end
    applying = true
    pcall(function() mp.commandv('vf', 'remove', '@' .. LABEL) end)
    applying = false
    pushed = ''
end

-- 提示文案：真生效就写生效项；请求了但没生效就把**原因**写出来（否则用户只看到
-- 「VSR ×2」，看不出 HDR 为什么没开）。
local function publish()
    local hint
    if last.filter ~= '' then
        local bits = {}
        if want_vsr and last.scale > 0 then
            bits[#bits + 1] = string.format('VSR ×%.2f', last.scale)
        end
        if want_hdr and last.filter:find('nvidia%-true%-hdr', 1, false) then
            bits[#bits + 1] = 'HDR'
        end
        hint = #bits > 0 and table.concat(bits, ' + ') or 'RTX'
        if last.reason ~= '' then hint = hint .. '（' .. last.reason .. '）' end
    elseif want_vsr or want_hdr then
        hint = 'RTX 未生效：' .. (last.reason ~= '' and last.reason or '等待视频')
    else
        hint = 'RTX 关闭'
    end
    pcall(function() mp.set_property('user-data/fntv_rtx/hint', hint) end)
end

-- 评估并落链。返回 true 表示本条命令真的改了链。
local function apply(quiet)
    local filter, reason, scale = desired_filter()
    local was = last.filter
    last.filter = filter
    last.reason = reason
    last.scale = scale

    local idx, entry, count = find_ours()
    local at_end = idx ~= nil and idx == count
    local alive = entry == nil or entry.enabled ~= false
    -- 「已经是对的」= 在链尾 + 活着 + 参数文本没变
    if filter ~= '' and at_end and alive and filter == pushed then
        last.active = true
        publish()
        return false
    end

    if filter == '' then
        if idx then remove_ours() end
        last.active = false
        publish()
        return was ~= ''
    end

    -- 摘掉旧条目再追加：vf add 永远追加到链尾，这就是「RTX 永远最后」的实现。
    if idx then remove_ours() end
    applying = true
    -- pcall 的第二个返回值是错误信息。**不要静默吞掉**：mpv 对滤镜命令失败会抛错
    -- （例如驱动不支持 d3d11vpp、或当前输出不是 D3D11），吞掉就变成「开关点了没反应」。
    local ok, err = pcall(function()
        mp.commandv('vf', 'add', '@' .. LABEL .. ':' .. filter)
    end)
    applying = false
    last.active = ok
    pushed = ok and filter or ''
    if not ok then
        last.reason = 'mpv 拒绝了滤镜命令：' .. tostring(err)
        if not quiet then toast('RTX 滤镜挂载失败：' .. tostring(err)) end
    end
    publish()
    return true
end

local function schedule()
    if applying then return end
    if timer then timer:kill() end
    timer = mp.add_timeout(DEBOUNCE, function()
        timer = nil
        local changed = apply(true)
        -- 命令被接受 ≠ 滤镜存活：驱动拒绝时 mpv 会把条目从 vf 里剔掉（enabled=false）。
        -- 1.2s 后核对一次，没活下来就报原因。
        if changed and last.filter ~= '' then
            mp.add_timeout(1.2, function()
                local _, e = find_ours()
                if not e or e.enabled == false then
                    last.active = false
                    last.reason = '显卡/驱动没有接受 RTX 滤镜（NVIDIA App 里打开「RTX 视频增强」）'
                    publish()
                    toast(last.reason)
                end
            end)
        end
    end)
end

-- ── 观察：vf 链被别的脚本改过、片源/目标色彩/显示器变化都重新评估 ──────────
mp.observe_property('vf', 'native', function(_, vf)
    if applying then return end
    local idx, entry, count = find_ours(vf)
    if last.filter == '' then
        -- 本不该有：万一残留（例如上次会话），摘掉
        if idx then schedule() end
        return
    end
    if not idx or idx ~= count or (entry and entry.enabled == false) then
        schedule()
    end
end)

mp.observe_property('video-params', 'native', schedule)
mp.observe_property('video-target-params', 'native', function()
    -- 目标色彩空间变了（系统 HDR 开关）：重新评估，并刷新提示
    schedule()
end)
mp.observe_property('display-width', 'number', schedule)
mp.observe_property('display-height', 'number', schedule)
mp.observe_property('current-gpu-context', 'string', schedule)
mp.observe_property('hwdec-current', 'string', schedule)

mp.register_event('file-loaded', function()
    mp.add_timeout(0.2, function() apply(true) end)
end)

-- ── 外部端点（播放器菜单 / 应用设置面板）────────────────────────────────
local function set_flags(vsr, hdr, quiet)
    local before = want_vsr ~= vsr or want_hdr ~= hdr
    want_vsr, want_hdr = vsr, hdr
    if not before then return end
    local filter, reason = desired_filter()
    if filter ~= '' then
        if not quiet then toast('RTX：' .. (want_vsr and '超分' or '') ..
            (want_vsr and want_hdr and ' + ' or '') .. (want_hdr and 'HDR' or '') .. ' 已开启')
    end elseif (want_vsr or want_hdr) and not quiet then
        toast('RTX 未生效：' .. (reason ~= '' and reason or '等待视频'))
    end
    schedule()
end

mp.register_script_message('fntv-rtx-set', function(vsr, hdr)
    local on = function(v) return v == 'yes' or v == '1' or v == 'on' or v == 'true' end
    set_flags(on(vsr), on(hdr), false)
    -- 回写配置，让下次起播保持（应用设置面板与菜单同一份文件）
    pcall(function()
        local f = mp.find_config_file('script-opts/fntv_rtx.conf')
        local path = f or mp.command_native({ 'expand-path', '~~/script-opts/fntv_rtx.conf' })
        local out = io.open(path, 'w')
        if out then
            out:write('# AniBox RTX 视频增强（播放器菜单或设置面板写入）\n')
            out:write('# vsr = RTX 超分辨率（放大到屏幕分辨率）；hdr = RTX Video HDR（SDR→HDR）\n')
            out:write('vsr=' .. (want_vsr and 'yes' or 'no') .. '\n')
            out:write('hdr=' .. (want_hdr and 'yes' or 'no') .. '\n')
            out:close()
        end
    end)
end)

mp.register_script_message('fntv-rtx-toggle-vsr', function()
    set_flags(not want_vsr, want_hdr, false)
    mp.commandv('script-message-to', 'fntv_rtx', 'fntv-rtx-set',
        want_vsr and 'yes' or 'no', want_hdr and 'yes' or 'no')
end)
mp.register_script_message('fntv-rtx-toggle-hdr', function()
    set_flags(want_vsr, not want_hdr, false)
    mp.commandv('script-message-to', 'fntv_rtx', 'fntv-rtx-set',
        want_vsr and 'yes' or 'no', want_hdr and 'yes' or 'no')
end)

publish()
if want_vsr or want_hdr then schedule() end
