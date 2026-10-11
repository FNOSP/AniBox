-- fntv_menu.lua
-- Fntv-Plus · 功能开关集（对照 mpv-Yaozhi「杳知功能菜单」的可移植功能集）
-- ─────────────────────────────────────────────────────────────────────────
-- 入口：右键 /「更多」菜单 → Fntv-Plus 子菜单，功能列表静态平铺（input.conf #menu:，无二级入口）：
--   超分与补帧… / 起播格式标签 / 音乐模式 / 置顶模式 为已有功能的消息端点转发；
-- 本脚本实现四个开关：
--   · 视频章节显示：切章时 OSD 显示章节标题（默认关）
--   · 起播自动全屏：开始播放自动进入全屏（默认关）
--   · 最小化暂停播放：窗口最小化自动暂停、恢复时续播（默认开，对齐 Yaozhi）；
--     音乐模式激活时让路（music-mode 的 minimize-bypass），不抢音乐播放
--   · 音频源码直通：audio-spdif 家用功放常用集（ac3/dts/dts-hd/eac3/truehd，默认关；
--     输出设备不支持时 mpv 自动回落解码）
-- 不移植项（依赖 Yaozhi 私有核心或无收益）：启动页、HDR 输出峰值（gpu-next 自动）、
-- HDR 图形字幕亮度（需核心补丁）、蓝光圆盘菜单、赞赏。
-- 所有开关为会话级（重启播放器回到 script-opts/fntv_menu.conf 的默认值），点击后 OSD 提示状态。
-- ─────────────────────────────────────────────────────────────────────────

local mp = require 'mp'
local msg = require 'mp.msg'
local opt = require 'mp.options'

local conf = {
    -- 视频章节显示：切章时 OSD 章节标题（yes/no）
    chapters_osd = 'no',
    -- 起播自动全屏（yes/no）
    auto_fullscreen = 'no',
    -- 最小化暂停播放（yes/no）
    minimize_pause = 'yes',
    -- 音频源码直通（yes/no；设备不支持时 mpv 自动回落解码）
    audio_passthrough = 'no',
}
opt.read_options(conf, 'fntv_menu')

local SPDIF_CODECS = 'ac3,dts,dts-hd,eac3,truehd'

local state = {
    we_paused = false, -- 最小化暂停是否由本脚本触发（恢复时只续播自己按下的暂停）
}

local function norm(v) return v == 'yes' end

local function get_flag(name)
    return mp.get_property('user-data/fntv_menu/' .. name, norm(conf[name]) and 'yes' or 'no') == 'yes'
end

local function set_flag(name, on)
    mp.set_property('user-data/fntv_menu/' .. name, on and 'yes' or 'no')
end

for _, name in ipairs({ 'chapters_osd', 'auto_fullscreen', 'minimize_pause', 'audio_passthrough' }) do
    set_flag(name, norm(conf[name]))
end

local function toast(text)
    pcall(function() mp.commandv('show-text', text, 2200) end)
end

-- ─────────────────────── 视频章节显示 ───────────────────────

mp.observe_property('current-chapter', 'number', function(_, chapter)
    if not get_flag('chapters_osd') or chapter == nil or chapter < 0 then return end
    local list = mp.get_property_native('chapter-list') or {}
    local item = list[chapter + 1]
    if not item then return end
    local title = tostring(item.title or '')
    if title == '' then title = '(未命名章节)' end
    pcall(function()
        mp.commandv('show-text', ('章节 %d/%d · %s'):format(chapter + 1, #list, title), 3000)
    end)
end)

-- ─────────────────────── 起播自动全屏 ───────────────────────

mp.register_event('file-loaded', function()
    if not get_flag('auto_fullscreen') then return end
    if (mp.get_property('vid') or 'auto') == 'no' then return end -- 纯音频不起全屏
    if mp.get_property('fullscreen') == 'yes' then return end
    pcall(function() mp.set_property('fullscreen', 'yes') end)
end)

-- ─────────────────────── 最小化暂停播放 ───────────────────────

mp.observe_property('window-minimized', 'bool', function(_, minimized)
    if not get_flag('minimize_pause') then return end
    if minimized then
        -- 音乐模式激活时让路：它本来就设计为最小化后继续播
        if mp.get_property('user-data/music-mode/minimize-bypass', 'no') == 'yes' then return end
        if mp.get_property_bool('pause', false) then return end
        state.we_paused = true
        pcall(function() mp.set_property_bool('pause', true) end)
    elseif state.we_paused then
        state.we_paused = false
        pcall(function() mp.set_property_bool('pause', false) end)
    end
end)

-- 关闭开关时归还控制权：若正由我们暂停则续播
mp.register_script_message('fntv-menu-minimize-pause', function(v)
    local on = v == 'yes'
    set_flag('minimize_pause', on)
    if not on and state.we_paused then
        state.we_paused = false
        pcall(function() mp.set_property_bool('pause', false) end)
    end
    toast('最小化暂停播放: ' .. (on and '开启' or '关闭'))
end)

-- ─────────────────────── 音频源码直通 ───────────────────────

local function apply_spdif()
    pcall(function()
        mp.set_property('audio-spdif', get_flag('audio_passthrough') and SPDIF_CODECS or '')
    end)
end
apply_spdif()

mp.register_script_message('fntv-menu-audio-pt', function(v)
    local on = v == 'yes'
    set_flag('audio_passthrough', on)
    apply_spdif()
    toast('音频源码直通: ' .. (on and '开启（设备不支持时自动回落解码）' or '关闭'))
end)

-- ─────────────────────── 通用开关消息 ───────────────────────

local function simple_toggle(name, label)
    return function(v)
        local on
        if v == 'yes' or v == 'no' then
            on = v == 'yes'
        else
            on = not get_flag(name)
        end
        set_flag(name, on)
        toast(label .. ': ' .. (on and '开启' or '关闭'))
    end
end
mp.register_script_message('fntv-menu-chapters', simple_toggle('chapters_osd', '视频章节显示'))
mp.register_script_message('fntv-menu-fullscreen', simple_toggle('auto_fullscreen', '起播自动全屏'))

-- ─────────────────────── 功能列表端点（input.conf #menu: Fntv-Plus > 静态平铺，无二级菜单） ───────────────────────

-- 音乐模式：直接翻转 user-data/music-mode/active（music-mode.lua 的开关桥会换算成 set/off）
mp.register_script_message('fntv-menu-music', function()
    local active = mp.get_property('user-data/music-mode/active', 'no') == 'yes'
    pcall(function() mp.set_property('user-data/music-mode/active', active and 'no' or 'yes') end)
    toast('音乐模式: ' .. (active and '关闭' or '开启（循环播放）'))
end)

msg.info(('fntv_menu 已加载 (章节OSD=%s, 起播全屏=%s, 最小化暂停=%s, 音频直通=%s)')
    :format(tostring(conf.chapters_osd), tostring(conf.auto_fullscreen),
        tostring(conf.minimize_pause), tostring(conf.audio_passthrough)))
