-- hdr_mode.lua
-- Fntv-Plus · HDR 输出与黑位后端（对接 uosc 内置「HDR 输出与峰值亮度」「黑位与输出范围」菜单）
-- ─────────────────────────────────────────────────────────────────────────
-- uosc fork 已内置这两张菜单（codex-open-peak-menu / codex-open-black-level-menu），
-- 菜单项全部发 script-message-to hdr_mode …；本脚本为其提供原生 mpv 实现的后端：
--   · 输出范围 auto/full/limited → video-output-levels（黑位不匹配的显示链手动纠偏）
--   · 输出模式 auto/sdr          → target-trc/target-prim（播放器内 HDR→SDR，不改 Windows）
--   · 强制 HDR 直出              → target-trc=pq（SDR 屏上画面会发灰，仅调试/特例使用）
--   · 手动目标峰值 auto/200..4000 → target-peak
-- 全部会话级，切换即 OSD 提示；状态写入 user-data 供菜单 hint 与诊断显示。
-- 不移植部分（依赖 Yaozhi 私有核心）：逐帧 HDR 元数据透传、Windows 系统级 HDR 开关。
-- ─────────────────────────────────────────────────────────────────────────

local mp = require 'mp'
local msg = require 'mp.msg'

local state = {
    output_mode = 'auto',   -- auto | sdr（播放器 HDR→SDR）
    force_hdr = false,      -- 强制 HDR 直出（target-trc=pq）
    output_range = 'auto',  -- auto | full | limited
}

local function toast(text)
    pcall(function() mp.commandv('show-text', text, 2600) end)
end

local function publish()
    pcall(function()
        mp.set_property('user-data/hdr-mode/output-mode', state.output_mode)
        mp.set_property('user-data/black-level/output-range-mode', state.output_range)
        mp.set_property('user-data/black-level/reason-code',
            state.force_hdr and '强制 HDR 直出'
            or state.output_mode == 'sdr' and '播放器 HDR→SDR'
            or '自动协商')
        mp.set_property('user-data/black-level/detail',
            '范围:' .. state.output_range .. ' · 模式:' .. state.output_mode
            .. (state.force_hdr and ' · 强制HDR' or ''))
    end)
end

local function apply()
    if state.force_hdr then
        mp.set_property('target-trc', 'pq')
        mp.set_property('target-prim', 'bt.2020')
    elseif state.output_mode == 'sdr' then
        mp.set_property('target-trc', 'bt.1886')
        mp.set_property('target-prim', 'bt.709')
    else
        mp.set_property('target-trc', 'auto')
        mp.set_property('target-prim', 'auto')
    end
    mp.set_property('video-output-levels', state.output_range)
    publish()
end

-- 按实际输出链推导菜单的「Windows HDR / 决策」提示
local function refresh_hdr_status()
    local tp = mp.get_property_native('video-target-params') or {}
    local gamma = tostring(tp.gamma or '')
    local label = gamma == 'pq' and 'HDR 输出' or 'SDR 输出'
    pcall(function() mp.set_property('user-data/display-info/hdr-status', label) end)
end

mp.observe_property('video-target-params', 'native', function() refresh_hdr_status() end)
mp.register_event('file-loaded', function() refresh_hdr_status() end)

-- ─────────────────────── 菜单命令 ───────────────────────

mp.register_script_message('set-output-range', function(mode)
    if mode ~= 'auto' and mode ~= 'full' and mode ~= 'limited' then return end
    state.output_range = mode
    apply()
    toast('输出范围: ' .. (mode == 'auto' and '自动协商' or mode == 'full' and 'RGB Full (0-255)' or 'Limited (16-235)'))
end)

mp.register_script_message('set-output-mode', function(mode)
    if mode ~= 'auto' and mode ~= 'sdr' then return end
    state.output_mode = mode
    if mode == 'sdr' then state.force_hdr = false end
    apply()
    refresh_hdr_status()
    toast('输出模式: ' .. (mode == 'auto' and '自动选择 HDR/SDR' or '播放器 HDR→SDR（不改系统）'))
end)

mp.register_script_message('manual-force-toggle', function()
    state.force_hdr = not state.force_hdr
    apply()
    refresh_hdr_status()
    if state.force_hdr then
        toast('强制 HDR 直出: 开启（SDR 屏幕上画面会偏灰，仅特例使用）')
    else
        toast('强制 HDR 直出: 关闭')
    end
end)

mp.register_script_message('manual-force-peak', function(value)
    if value == 'auto' then
        mp.set_property('target-peak', 'auto')
        toast('目标峰值: 自动')
    else
        local nits = tonumber(value)
        if not nits or nits < 50 or nits > 10000 then return end
        mp.set_property('target-peak', tostring(math.floor(nits)))
        toast(('目标峰值: %d nits'):format(math.floor(nits)))
    end
end)

mp.register_script_message('show-black-level-diagnostics', function()
    local vp = mp.get_property_native('video-params') or {}
    local tp = mp.get_property_native('video-target-params') or {}
    local lines = {
        ('片源: %dx%d · %s · %s'):format(tonumber(vp.width) or 0, tonumber(vp.height) or 0,
            tostring(vp.colorlevels or '?'), tostring(vp.gamma or '?')),
        ('输出目标: %s · %s'):format(tostring(tp.colorlevels or '?'), tostring(tp.gamma or '?')),
        ('输出范围(手动): %s · 目标峰值: %s'):format(tostring(mp.get_property('video-output-levels', 'auto')),
            tostring(mp.get_property('target-peak', 'auto'))),
        ('模式: %s%s'):format(state.output_mode, state.force_hdr and ' + 强制HDR直出' or ''),
    }
    pcall(function() mp.commandv('show-text', table.concat(lines, '\n'), 6000) end)
end)

apply()
msg.info(('hdr_mode 已加载 (output-mode=%s, output-range=%s)'):format(state.output_mode, state.output_range))
