-- fntv_mediainfo.lua
-- Fntv-Plus · 视频信息徽章（对照 mpv-Yaozhi 底栏截图的顶部信息条）
-- ─────────────────────────────────────────────────────────────────────
-- 播放起播后在视频区左上角显示：硬解 · 动态范围 · 编码 · 分辨率 · 帧率 · 实时码率
--   例：硬解  SDR  HEVC  1080P  24FPS  8.4Mbps
-- 视频参数就绪后出现，持续 display_duration 秒后淡出；Ctrl+i（可配）随时
-- 重新显示/隐藏，script-message fntv_mediainfo-show 供外部调用。
-- 仅用标准 mpv 属性，不依赖 uosc / 自编译核心补丁。

local opt = require('mp.options')
local msg = require('mp.msg')

local options = {
    -- 起播显示时长（秒），0 = 常显（跟随播放一直刷新）
    display_duration = 6,
    -- 显示/隐藏快捷键（空字符串禁用）
    toggle_key = 'Ctrl+i',
    -- 顶边距附加偏移（像素，1080p 基准），全屏顶栏闪现时可加大避免重叠
    margin_x = 16,
    margin_y = 18,
    -- 字号（1080p 基准）
    font_size = 21,
}
opt.read_options(options, mp.get_script_name())

local mp_utils = require('mp.utils')
local ov = mp.create_osd_overlay('ass-events')
local enabled = true          -- 用户总开关（Ctrl+i 切换）
local visible = false         -- 当前是否在显示
local hide_timer
local refresh_timer
local bitrate_ready = false   -- 码率就绪前延迟首帧刷新，避免显示 0.0Mbps

local function is_video()
    local tracks = mp.get_property_native('track-list') or {}
    for _, track in ipairs(tracks) do
        if track.type == 'video' and not track.image and not track.albumart then
            return true
        end
    end
    return false
end

local function format_dr()
    local gamma = tostring(mp.get_property('video-params/gamma', '') or ''):lower()
    if gamma == 'pq' then return 'HDR10' end
    if gamma == 'hlg' then return 'HLG' end
    return 'SDR'
end

local function format_codec()
    local codec = tostring(mp.get_property('video-codec-name', '') or ''):lower()
    local map = {
        hevc = 'HEVC', h264 = 'AVC', av1 = 'AV1', vp9 = 'VP9', vp8 = 'VP8',
        mpeg2video = 'MPEG2', mpeg4 = 'MPEG4', mpeg1video = 'MPEG1',
        flv1 = 'FLV', wmv3 = 'WMV3', avs2 = 'AVS2', avs3 = 'AVS3',
    }
    if codec == '' then return nil end
    return map[codec] or codec:upper()
end

local function format_resolution()
    local h = mp.get_property_number('video-params/h')
    if not h or h <= 0 then return nil end
    return string.format('%dP', math.floor(h / 10 + 0.5) * 10)
end

local function format_fps()
    local fps = mp.get_property_number('container-fps')
        or mp.get_property_number('estimated-vf-fps')
    if not fps or fps <= 0 then return nil end
    return string.format('%dFPS', math.floor(fps + 0.5))
end

local function format_bitrate()
    local bps = mp.get_property_number('video-bitrate', 0) or 0
    if bps <= 0 then return nil end
    if bps >= 1e6 then
        return string.format('%.1fMbps', bps / 1e6)
    elseif bps >= 1e3 then
        return string.format('%dKbps', math.floor(bps / 1e3 + 0.5))
    end
    return string.format('%dbps', bps)
end

local function format_hwdec()
    local hwdec = mp.get_property('hwdec-current', 'no')
    if hwdec and hwdec ~= 'no' and hwdec ~= '' then return '硬解' end
    return nil
end

local function build_tokens()
    local tokens = {}
    local hwdec = format_hwdec()
    if hwdec then tokens[#tokens + 1] = hwdec end
    tokens[#tokens + 1] = format_dr()
    local codec = format_codec()
    if codec then tokens[#tokens + 1] = codec end
    local res = format_resolution()
    if res then tokens[#tokens + 1] = res end
    local fps = format_fps()
    if fps then tokens[#tokens + 1] = fps end
    local bitrate = format_bitrate()
    if bitrate then tokens[#tokens + 1] = bitrate end
    if #tokens == 0 then return nil end
    return table.concat(tokens, '  ')
end

local function render()
    if not visible or not enabled then return end
    if not is_video() then return end
    local tokens = build_tokens()
    if not tokens then return end

    local width, height = mp.get_osd_size()
    if not width or width <= 0 then return end
    local scale = height / 1080
    local dims = mp.get_property_native('osd-dimensions') or {}
    -- 徽章锚定在【视频画面】左上角（黑边内收），而非窗口角落
    local x = (tonumber(dims.ml) or 0) + math.floor(options.margin_x * scale + 0.5)
    local y = (tonumber(dims.mt) or 0) + math.floor(options.margin_y * scale + 0.5)
    local fs = math.max(13, math.floor(options.font_size * scale + 0.5))
    -- 供 fntv_format_badge 徽章参考：信息条可见时徽章自动下移错位（user-data 键）
    mp.set_property_bool('user-data/fntv_mediainfo/visible', true)
    mp.set_property_number('user-data/fntv_mediainfo/font_size', fs)

    ov.res_x = width
    ov.res_y = height
    ov.data = string.format(
        '{\\an7\\pos(%d,%d)\\blur0\\bord%.2f\\shad0\\1c&HFFFFFF&\\3c&H0D1217&\\fs%d}%s',
        x, y, math.max(1, 1.3 * scale), fs, tokens)
    ov:update()
end

local function stop_timers()
    if hide_timer then hide_timer:kill() end
    if refresh_timer then refresh_timer:kill() end
end

local function hide()
    stop_timers()
    visible = false
    mp.set_property_bool('user-data/fntv_mediainfo/visible', false)
    ov:remove()
end

local function show()
    if not enabled or not is_video() then return end
    visible = true
    bitrate_ready = false
    render()
    -- 码率属性每秒更新一次，就绪前每 0.5s 刷新，尽快补上 Mbps 徽章
    refresh_timer = mp.add_periodic_timer(0.5, function()
        render()
        if (mp.get_property_number('video-bitrate', 0) or 0) > 0 then
            bitrate_ready = true
        end
        if bitrate_ready then
            refresh_timer:kill()
            -- 码率就绪后降为 1s 节奏（码率会随播放波动）
            if options.display_duration <= 0 or true then
                refresh_timer = mp.add_periodic_timer(1, function()
                    if visible then render() end
                end)
            end
        end
    end)
    if options.display_duration > 0 then
        hide_timer = mp.add_timeout(options.display_duration, hide)
    end
end

local function toggle()
    if visible then
        hide()
        enabled = false
    else
        enabled = true
        show()
    end
end

mp.register_event('file-loaded', function()
    hide()
    -- 等 video-params 就绪（流媒体可能要几秒），轮询到再显示
    local probe
    probe = mp.add_periodic_timer(0.5, function()
        if mp.get_property_number('video-params/h', 0) and is_video() then
            probe:kill()
            show()
        end
    end)
    -- 兜底：20s 内一直没视频参数就放弃本轮
    mp.add_timeout(20, function() if probe then probe:kill() end end)
end)

mp.register_event('end-file', hide)
mp.register_script_message('fntv_mediainfo-show', function()
    enabled = true
    show()
end)
mp.register_script_message('fntv_mediainfo-toggle', toggle)
if options.toggle_key and options.toggle_key ~= '' then
    mp.add_key_binding(options.toggle_key, 'fntv_mediainfo-toggle', toggle)
end

msg.info('fntv_mediainfo loaded')
