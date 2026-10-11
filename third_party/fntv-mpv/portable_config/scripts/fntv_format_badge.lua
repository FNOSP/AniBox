-- fntv_format_badge.lua
-- Fntv-Plus · 专业起播格式标签（移植自 mpv-Yaozhi「起播格式标签」）
-- ─────────────────────────────────────────────────────────────────────
-- 画面信息就绪时识别当前画面标准与选中音轨，在真实视频画面的安全区短暂显示：
--   画面标签：Dolby Vision / HDR Vivid / HDR10+ / HDR10 / HLG / SDR
--   音频标签：Dolby Atmos / DTS:X / TrueHD / DTS-HD MA/HRA / AC-4 / E-AC-3 / AC-3 /
--             MPEG-H / Audio Vivid / DRA / APE / FLAC / PCM / AAC / Opus / MP3 等
-- 渲染（双模式，自动选择）：
--   · bgra（默认）：读取 script-assets/startup-format-logos/runtime/manifest.json，
--     用 mpv 标准 overlay-add 贴自绘徽章位图（Yaozhi 同款资产，MIT）；
--     彩色/透明白两风格 × 1x/2x 两档源（1080p 1:1，4K 下采样保锐度）×
--     6 级预渲染透明度帧淡入淡出。
--   · text（回退）：资产缺失或 overlay-add 失败时，回退 ASS 圆角胶囊 + 文字。
-- 音轨切换后自动刷新；编码黑边识别(cropdetect)在后台完成，不阻塞首个可呈现画面，
-- 识别完成后徽章自动收进真实画面内容区（排除窗口黑边 + 编码黑边两步）。
-- 与顶部信息条(fntv_mediainfo)同角时自动下移错位。
-- 仅用标准 mpv 属性 / 滤镜元数据，不依赖 uosc 或自编译内核补丁。
--
-- 探测依据（fntv-mpv 0.41 内核实测）：
--   杜比视界  = 选中视频轨 track-list/N/dolby-vision-profile 存在（上游 0.39+），
--               兜底 video-params/transform == 'dolbyvision'
--   HDR Vivid = video-params/hdr-vivid（内核 0040 补丁，mpv-Yaozhi 同款，ST 2094-60 侧数据）
--   HDR10+    = video-params/scene-avg 属性可用（上游仅在 ST 2094-40 侧数据存在时提供）
--   HDR10/HLG = video-params/gamma == pq / hlg
--   音频      = 选中音轨 codec / codec-profile / decoder-desc / codec-desc /
--               format-name / title 关键字匹配（Atmos 优先于 TrueHD/E-AC-3）

local opt = require('mp.options')
local msg = require('mp.msg')
local utils = require('mp.utils')

local options = {
    -- 总开关：yes=起播显示格式徽章
    enabled = 'yes',
    -- 显示样式：colored=彩色徽章 / white=透明白图标
    style = 'colored',
    -- 起播显示时长（秒），0 = 常显（bgra 模式为完整显示的停留时长，淡入淡出另计）
    display_duration = 4.5,
    -- 音轨切换后自动刷新显示
    refresh_on_track_switch = 'yes',
    -- 画面标签 / 音频标签显示开关
    show_video = 'yes',
    show_audio = 'yes',
    -- 编码黑边识别（cropdetect 后台跑一小段，把徽章收进真实画面内容区）
    crop_detect = 'yes',
    -- 黑边识别取样窗口（秒）
    crop_detect_duration = 2.5,
    -- 安全区边距（1080p 画面基准，像素；bgra 模式按视口比例缩放）
    margin_x = 16,
    margin_y = 18,
    -- 徽章停靠角：top-left / top-right / bottom-left / bottom-right
    --（默认右上，与左上信息条(fntv_mediainfo)错开；左上时自动下移让位）
    position = 'top-right',
    -- 徽章字号（1080p 画面基准；仅 text 回退模式使用）
    font_size = 19,
    -- bgra 模式：整体缩放（1 = 跟随视口 1080p 基准）与竖屏补偿
    badge_scale = 1.0,
    portrait_scale = 1.18,
    -- bgra 模式：淡入/淡出时长（秒，走预渲染透明度帧序列）
    fade_in = 0.12,
    fade_out = 0.18,
    -- 显示/隐藏快捷键（留空禁用；切换总开关）
    toggle_key = 'Ctrl+u',
}
opt.read_options(options, mp.get_script_name())

if options.style ~= 'white' then options.style = 'colored' end

-- 停靠角归一化（非法值回退 top-right）；mediainfo 在左上角，徽章同角时才需要下移让位
local function norm_position(v)
    local p = tostring(v or ''):lower():gsub('_', '-')
    if (p:find('top', 1, true) or p:find('bottom', 1, true))
        and (p:find('left', 1, true) or p:find('right', 1, true)) then
        return p
    end
    return 'top-right'
end
local POS = norm_position(options.position)
local POS_LEFT = POS:find('left', 1, true) ~= nil
local POS_BOTTOM = POS:find('bottom', 1, true) ~= nil

-- ─────────────────────────────────────────────────────────────────────
-- 徽章定义（slug = 位图资产键；badge = text 回退模式的文字胶囊）
-- ─────────────────────────────────────────────────────────────────────

local VIDEO_BADGES = {
    dv     = { slug = 'dolby-vision', text = 'Dolby Vision', bg = '000000', fg = 'FFFFFF' },
    vivid  = { slug = 'hdr-vivid',    text = 'HDR Vivid',    bg = 'E2231A', fg = 'FFFFFF' },
    hdr10p = { slug = 'hdr10-plus',   text = 'HDR10+',       bg = '2563EB', fg = 'FFFFFF' },
    hdr10  = { slug = 'hdr10',        text = 'HDR10',        bg = 'F59E0B', fg = '141414' },
    hlg    = { slug = 'hlg',          text = 'HLG',          bg = '7C3AED', fg = 'FFFFFF' },
    sdr    = { slug = 'sdr',          text = 'SDR',          bg = '4B5563', fg = 'FFFFFF' },
}

-- 顺序即优先级：Atmos > TrueHD > DTS:X > DTS-HD MA/HRA > DTS > AC-4 > E-AC-3 > AC-3 > ...
-- pats 在拼接串（codec+profile+解码器描述+轨名等，全小写）里做字面匹配；
-- codecs 只对 codec 字段做精确匹配（dra/ape 等短词避免误命中其它描述）。
local AUDIO_MATCHERS = {
    { key = 'atmos',    pats = { 'atmos' } },
    { key = 'truehd',   pats = { 'truehd', 'mlp' } },
    { key = 'dtsx',     pats = { 'dts:x', 'dts-x', 'dtsx' } },
    { key = 'dtshdma',  pats = { 'dts-hd ma', 'hd ma', 'dtshd ma' } },
    { key = 'dtshdhra', pats = { 'dts-hd hra', 'hd hra' } },
    { key = 'dts',      pats = { 'dts' } },
    { key = 'ac4',      pats = { 'ac-4', 'ac4' } },
    { key = 'eac3',     pats = { 'eac3', 'e-ac-3' } },
    { key = 'ac3',      pats = { 'ac3', 'ac-3', 'dolby digital' } },
    { key = 'mpegh',    pats = { 'mpegh', 'mpegh3da', 'mhm1', 'lhm1' } },
    { key = 'avivid',   pats = { 'av3a', 'audio vivid' } },
    { key = 'dra',      codecs = { 'dra' } },
    { key = 'ape',      codecs = { 'ape' }, pats = { "monkey'" } },
    { key = 'flac',     pats = { 'flac' } },
    { key = 'pcm',      pats = { 'pcm_', 'pcm ' } },
    { key = 'aac',      pats = { 'aac', 'mp4a' } },
    { key = 'opus',     pats = { 'opus' } },
    { key = 'vorbis',   pats = { 'vorbis' } },
    { key = 'mp3',      pats = { 'mp3' } },
    { key = 'alac',     pats = { 'alac' } },
    { key = 'wma',      pats = { 'wmav', 'wmapro', 'wma lossless' } },
}

-- key → 位图资产 slug（与 manifest.json 的 logos 键一致）
local AUDIO_SLUGS = {
    atmos    = 'dolby-atmos',
    truehd   = 'dolby-truehd',
    dtsx     = 'dts-x',
    dtshdma  = 'dts-hd-ma',
    dtshdhra = 'dts-hd-hra',
    dts      = 'dts',
    ac4      = 'ac4',
    eac3     = 'dolby-digital-plus',
    ac3      = 'dolby-digital',
    mpegh    = 'mpeg-h',
    avivid   = 'audio-vivid',
    dra      = 'dra',
    ape      = 'ape',
    flac     = 'flac',
    pcm      = 'pcm',
    aac      = 'aac',
    opus     = 'opus',
    vorbis   = 'vorbis',
    mp3      = 'mp3',
    alac     = 'alac',
    wma      = 'wma',
}

local AUDIO_BADGES = {
    atmos    = { text = 'Dolby Atmos', bg = '000000', fg = 'FFFFFF' },
    truehd   = { text = 'TrueHD',      bg = '101010', fg = 'F0F0F0' },
    dtsx     = { text = 'DTS:X',       bg = '000000', fg = 'FFFFFF' },
    dtshdma  = { text = 'DTS-HD MA',   bg = 'C8102E', fg = 'FFFFFF' },
    dtshdhra = { text = 'DTS-HD HRA',  bg = 'C8102E', fg = 'FFFFFF' },
    dts      = { text = 'DTS',         bg = 'C8102E', fg = 'FFFFFF' },
    ac4      = { text = 'AC-4',        bg = '101010', fg = 'F0F0F0' },
    eac3     = { text = 'E-AC-3',      bg = '1F2937', fg = 'FFFFFF' },
    ac3      = { text = 'AC-3',        bg = '1F2937', fg = 'FFFFFF' },
    mpegh    = { text = 'MPEG-H',      bg = '00897B', fg = 'FFFFFF' },
    avivid   = { text = 'Audio Vivid', bg = 'E2231A', fg = 'FFFFFF' },
    dra      = { text = 'DRA',         bg = 'EA580C', fg = 'FFFFFF' },
    ape      = { text = 'APE',         bg = '475569', fg = 'FFFFFF' },
    flac     = { text = 'FLAC',        bg = '1E40AF', fg = 'FFFFFF' },
    pcm      = { text = 'PCM',         bg = '4B5563', fg = 'FFFFFF' },
    aac      = { text = 'AAC',         bg = '6B7280', fg = 'FFFFFF' },
    opus     = { text = 'Opus',        bg = 'B45309', fg = 'FFFFFF' },
    vorbis   = { text = 'Vorbis',      bg = '0D9488', fg = 'FFFFFF' },
    mp3      = { text = 'MP3',         bg = '6B7280', fg = 'FFFFFF' },
    alac     = { text = 'ALAC',        bg = '52525B', fg = 'FFFFFF' },
    wma      = { text = 'WMA',         bg = '475569', fg = 'FFFFFF' },
    fallback = { text = 'AUDIO',       bg = '4B5563', fg = 'FFFFFF' },
}

-- ─────────────────────────────────────────────────────────────────────
-- 状态 / 基础工具
-- ─────────────────────────────────────────────────────────────────────

local state = {
    on = options.enabled == 'yes',
    mode = 'text',        -- bgra | text（启动时按资产可用性决定，bgra 失败即永久回退）
    visible = false,      -- 当前是否在显示
    shown_once = false,   -- 本文件是否已首次显示过（音轨刷新以此为门槛）
    video_key = nil,
    audio_badge = nil,    -- { slug = string|nil, badge = {text,bg,fg} }
    audio_sig = '',
    crop = nil,           -- 编码黑边（源画面内容区原点，0~1 比例）
    crop_done = false,
    crop_scheduled = false,
    bgra_level = 0,       -- 当前透明度档索引（1..#levels）
    bgra_broken = false,  -- overlay-add 出过错（本次会话不再尝试 bgra）
    anim_timer = nil,     -- 淡入淡出步进
    hide_timer = nil,
    refresh_timer = nil,
    crop_timer = nil,
}

-- [lc-1353] 开关状态发布到 user-data，供功能菜单/其他脚本读取与高亮
local function publish_on()
    local ok, err = pcall(function() mp.set_property('user-data/fntv_format_badge/on', state.on and 'yes' or 'no') end)
    if not ok then msg.warn('发布开关状态失败: ' .. tostring(err)) end
end
publish_on()

-- text 回退模式的 ASS 覆盖层
local pills = {}
local texts = {}
for i = 1, 2 do
    pills[i] = mp.create_osd_overlay('ass-events')
    texts[i] = mp.create_osd_overlay('ass-events')
end

-- bgra 模式 overlay id 基址（+1=画面徽章 +2=音频徽章；0 号背景本资产主题不可见，不使用）
-- ⚠️ 内核限制 overlay-add id ∈ [0,63]，基址不可上调
local OVERLAY_BASE = 50

local function clamp(value, low, high)
    return math.max(low, math.min(high, value))
end

local function round(value)
    return math.floor(value + 0.5)
end

local function clear_text_overlays()
    for i = 1, 2 do
        pills[i]:remove()
        texts[i]:remove()
    end
end

local function overlay_remove(id)
    pcall(function() mp.command_native({ 'overlay-remove', id }) end)
end

local function clear_bgra_overlays()
    overlay_remove(OVERLAY_BASE)
    overlay_remove(OVERLAY_BASE + 1)
    overlay_remove(OVERLAY_BASE + 2)
end

local function clear_overlays()
    clear_text_overlays()
    clear_bgra_overlays()
end

local function stop_anim_timer()
    if state.anim_timer then state.anim_timer:kill() state.anim_timer = nil end
end

-- ─────────────────────────────────────────────────────────────────────
-- 位图资产（bgra 模式）
-- ─────────────────────────────────────────────────────────────────────

local manifest = nil
local asset_root = nil

local function load_assets()
    asset_root = mp.command_native({ 'expand-path', '~~/script-assets/startup-format-logos/runtime' })
    local info = utils.file_info(asset_root .. '/manifest.json')
    if not info or not info.is_file then
        msg.warn('未找到徽章位图资产(manifest.json)，回退文字胶囊渲染')
        return false
    end
    local f, err = io.open(asset_root .. '/manifest.json', 'r')
    if not f then
        msg.warn('manifest.json 读取失败: ' .. tostring(err))
        return false
    end
    local raw = f:read('*all')
    f:close()
    local parsed, perr = utils.parse_json(raw)
    if type(parsed) ~= 'table' or type(parsed.logos) ~= 'table'
        or type(parsed.base_layout) ~= 'table'
        or type(parsed.opacity_levels) ~= 'table' or #parsed.opacity_levels == 0 then
        msg.warn('manifest.json 解析失败或不完整: ' .. tostring(perr))
        return false
    end
    manifest = parsed
    return true
end

-- ─────────────────────────────────────────────────────────────────────
-- 公共几何（两种模式共用）
-- ─────────────────────────────────────────────────────────────────────

-- 视频画面在 OSD 坐标系里的显示矩形（排除窗口黑边；sw/sh 为显示尺寸）
local function video_rect()
    local d = mp.get_property_native('osd-dimensions') or {}
    local sw = tonumber(d.sw) or 0
    local sh = tonumber(d.sh) or 0
    if sw > 0 and sh > 0 then
        return tonumber(d.sx) or 0, tonumber(d.sy) or 0, sw, sh
    end
    local w, h = mp.get_osd_size()
    if not w or w <= 0 then return 0, 0, 0, 0 end
    local ml = tonumber(d.ml) or 0
    local mt = tonumber(d.mt) or 0
    local mr = tonumber(d.mr) or 0
    local mb = tonumber(d.mb) or 0
    return ml, mt, w - ml - mr, h - mt - mb
end

-- 顶部信息条(fntv_mediainfo)可见时徽章下移一行，避免同角重叠
local function mediainfo_extra_y(scale)
    if mp.get_property('user-data/fntv_mediainfo/visible', 'no') ~= 'yes' then return 0 end
    local mfs = mp.get_property_number('user-data/fntv_mediainfo/font_size', 21) or 21
    return math.floor((mfs * 1.75 + 8) * scale + 0.5)
end

-- ─────────────────────────────────────────────────────────────────────
-- bgra 渲染
-- ─────────────────────────────────────────────────────────────────────

local function overlay_add(id, x, y, file, sw, sh, dw, dh)
    local ok, err = pcall(mp.command_native, {
        'overlay-add', id, x, y, file, 0, 'bgra',
        sw, sh, sw * 4, dw, dh,
    })
    if not ok then
        msg.warn('overlay-add 失败，回退文字胶囊: ' .. tostring(err))
        state.bgra_broken = true
        return false
    end
    return true
end

local function bgra_slug_candidates()
    local video_slug = nil
    if options.show_video == 'yes' and state.video_key and VIDEO_BADGES[state.video_key] then
        video_slug = VIDEO_BADGES[state.video_key].slug
        if video_slug and not manifest.logos[video_slug] then video_slug = nil end
    end
    local audio_slug = nil
    if options.show_audio == 'yes' and state.audio_badge and state.audio_badge.slug then
        audio_slug = state.audio_badge.slug
        if audio_slug and not manifest.logos[audio_slug] then audio_slug = nil end
    end
    return video_slug, audio_slug
end

-- 徽章布局：锚定真实画面内容区（窗口黑边 + 编码黑边），按 position 停靠四角
local function bgra_layout(kind)
    local w, h = mp.get_osd_size()
    if not w or w <= 0 or not h or h <= 0 then return nil end
    local sx, sy, sw, sh = video_rect()
    if sw <= 0 or sh <= 0 then return nil end
    local base = manifest.base_layout[kind]
    if not base then return nil end
    local portrait = h > w
    local viewport_scale = portrait
        and math.min(w / 1080, h / 1920)
        or math.min(w / 1920, h / 1080)
    local orientation_scale = portrait and (tonumber(options.portrait_scale) or 1.18) or 1.0
    local display_scale = clamp(
        viewport_scale * (tonumber(options.badge_scale) or 1.0) * orientation_scale,
        0.25, 2.0)
    local fit = math.min((sw - 24) / (tonumber(base.w) or 156),
                         (sh - 24) / (tonumber(base.h) or 64))
    display_scale = math.min(display_scale, fit)
    if display_scale <= 0 then return nil end
    local mx = round(options.margin_x * viewport_scale)
    local my = round(options.margin_y * viewport_scale)
    local bw = round(base.w * display_scale)
    local bh = round(base.h * display_scale)
    -- 内容区边界：窗口黑边(osd-dimensions) + 编码黑边(cropdetect) 的左/上/右/下
    local crop = state.crop or {}
    local left = sx + (crop.x or 0) * sw
    local top = sy + (crop.y or 0) * sh
    local right = sx + (crop.r or 1) * sw
    local bottom = sy + (crop.b or 1) * sh
    local x = POS_LEFT and (left + mx) or (right - bw - mx)
    local y = POS_BOTTOM and (bottom - bh - my) or (top + my)
    -- 仅左上角与信息条(fntv_mediainfo)同位，需要下移让位
    if POS_LEFT and not POS_BOTTOM then
        y = y + mediainfo_extra_y(viewport_scale)
    end
    return {
        x = round(clamp(x, sx, sx + sw - bw)),
        y = round(clamp(y, sy, sy + sh - bh)),
        w = bw, h = bh,
        scale = display_scale,
        base = base,
    }
end

local function render_bgra_logo(id, slug, level_key, layout, center_y)
    local asset = manifest.logos[slug]
    if not asset then return false end
    local max_w = tonumber(manifest.base_layout.logo_max_w) or 156
    local max_h = tonumber(manifest.base_layout.logo_max_h) or 64
    local dw = math.max(1, round(max_w * layout.scale))
    local dh = math.max(1, round(max_h * layout.scale))
    local white = options.style == 'white'
    local src = { w = asset.w, h = asset.h, files = white and asset.white_files or asset.files }
    local variants = white and asset.white_variants or asset.variants
    if type(variants) == 'table' then
        -- 用能覆盖目标尺寸的最小源档：1080p 1x 1:1，1440p/4K 用 2x 缩到目标（避免上采样发糊）
        local one_x = variants['1']
        local two_x = variants['2']
        if type(one_x) == 'table'
            and dw <= (tonumber(one_x.w) or 0)
            and dh <= (tonumber(one_x.h) or 0) then
            src = { w = one_x.w, h = one_x.h, files = one_x.files }
        elseif type(two_x) == 'table' then
            src = { w = two_x.w, h = two_x.h, files = two_x.files }
        end
    end
    local filename = type(src.files) == 'table' and src.files[level_key]
    if not filename then return false end
    local x = round(layout.x + layout.w / 2 - dw / 2)
    local y = round(layout.y + (tonumber(center_y) or 0) * layout.scale - dh / 2)
    return overlay_add(id, x, y, asset_root .. '/' .. filename,
        tonumber(src.w) or 0, tonumber(src.h) or 0, dw, dh)
end

-- 渲染某一透明度档（index = 1..#opacity_levels）
local function render_bgra_level(index)
    if not manifest or state.bgra_broken then return false end
    local level = manifest.opacity_levels[index]
    if not level then return false end
    local level_key = tostring(level)
    local video_slug, audio_slug = bgra_slug_candidates()
    if not video_slug and not audio_slug then
        clear_bgra_overlays()
        return false
    end
    local kind = (video_slug and audio_slug) and 'double' or 'single'
    local layout = bgra_layout(kind)
    if not layout then
        clear_bgra_overlays()
        return false
    end
    if kind == 'double' then
        render_bgra_logo(OVERLAY_BASE + 1, video_slug, level_key, layout, layout.base.top_center_y)
        render_bgra_logo(OVERLAY_BASE + 2, audio_slug, level_key, layout, layout.base.bottom_center_y)
    else
        render_bgra_logo(OVERLAY_BASE + 1, video_slug or audio_slug, level_key, layout, layout.base.center_y)
        overlay_remove(OVERLAY_BASE + 2)
    end
    overlay_remove(OVERLAY_BASE)
    state.bgra_level = index
    return true
end

-- ─────────────────────────────────────────────────────────────────────
-- text 回退渲染
-- ─────────────────────────────────────────────────────────────────────

-- ASS 颜色：RRGGBB(RGB) -> BBGGRR(ASS 用 BGR)
local function to_bgr(hex)
    hex = tostring(hex or 'FFFFFF')
    return hex:sub(5, 6) .. hex:sub(3, 4) .. hex:sub(1, 2)
end

-- 徽章胶囊宽度估算：OSD 字体无测量 API，按字形宽度表近似（偏宽不影响观感）
local CH_W = {
    [' ']=0.33, ['-']=0.40, ['.']=0.32, [',']=0.32, [':']=0.34,
    ['!']=0.34, ['/']=0.42, ['+']=0.66,
    W=0.98, M=0.94, I=0.36, J=0.44,
    m=0.92, w=0.82, i=0.32, l=0.30, j=0.32, t=0.44, f=0.42, r=0.48,
}

local function char_w(c)
    if CH_W[c] then return CH_W[c] end
    if c:match('^%d$') then return 0.62 end
    if c:match('^[A-Z]$') then return 0.72 end
    if c:match('^[a-z]$') then return 0.60 end
    return 0.70
end

local function est_width(text, fs)
    local units = 0
    for c in tostring(text):gmatch('.') do
        units = units + char_w(c)
    end
    return math.ceil(units * fs)
end

-- 圆角胶囊 ASS 绘制路径（四角用三次贝塞尔，坐标绝对于 OSD）
local function rrect_path(x, y, w, h, r)
    if r * 2 > h then r = math.floor(h / 2) end
    local x0, y0 = x, y
    local x1, y1 = x + w, y + h
    if r < 1 then
        return string.format('m %d %d l %d %d l %d %d l %d %d',
            x0, y0, x1, y0, x1, y1, x0, y1)
    end
    return string.format(
        'm %d %d l %d %d b %d %d %d %d %d %d l %d %d b %d %d %d %d %d %d'
        .. ' l %d %d b %d %d %d %d %d %d l %d %d b %d %d %d %d %d %d',
        x0, y0 + r,                                    -- 左边下行
        x0, y1 - r,                                    -- 左下角
        x0, y1, x0 + r, y1, x0 + r, y1,
        x1 - r, y1,                                    -- 底边右行
        x1, y1, x1, y1 - r, x1, y1 - r,                -- 右下角
        x1, y0 + r,                                    -- 右边上行
        x1, y0, x1 - r, y0, x1 - r, y0,                -- 右上角
        x0 + r, y0,                                    -- 顶边左行
        x0 + r, y0, x0, y0, x0, y0 + r)                -- 左上角
end

local function draw_pill(ov, res_w, res_h, x, y, pw, ph, badge)
    ov.res_x = res_w
    ov.res_y = res_h
    local tags
    if options.style == 'white' then
        -- 透明白图标：无填充、白色半透明描边
        tags = '\\1c&HFFFFFF&\\1a&HFF&\\3c&HFFFFFF&\\3a&H50&\\bord1.3\\shad0'
    else
        tags = '\\1c&H' .. to_bgr(badge.bg) .. '\\3c&H' .. to_bgr(badge.bg) .. '\\bord0\\shad0'
    end
    ov.data = '{\\an7\\pos(0,0)\\p1' .. tags .. '}'
        .. rrect_path(x, y, pw, ph, math.floor(ph / 2)) .. '{\\p0}'
    ov:update()
end

local function draw_text(ov, res_w, res_h, x, y, fs, badge)
    ov.res_x = res_w
    ov.res_y = res_h
    local tags
    if options.style == 'white' then
        tags = '\\b1\\fs' .. fs
            .. '\\1c&HFFFFFF&\\1a&H38&\\3c&H000000&\\3a&H70&\\bord0.9\\shad0'
    else
        tags = '\\b1\\fs' .. fs
            .. '\\1c&H' .. to_bgr(badge.fg) .. '\\bord0\\shad0'
    end
    ov.data = '{\\an4\\pos(' .. x .. ',' .. math.floor(y + 0.5) .. ')' .. tags .. '}' .. badge.text
    ov:update()
end

local function render_text()
    if not state.visible then return end
    local vw = mp.get_property_number('video-params/w', 0) or 0
    local vh = mp.get_property_number('video-params/h', 0) or 0
    if vw <= 0 or vh <= 0 then return end
    local w, h = mp.get_osd_size()
    if not w or w <= 0 or not h or h <= 0 then return end
    local sx, sy, sw, sh = video_rect()
    if sw <= 0 or sh <= 0 then clear_text_overlays() return end

    local scale = math.max(0.5, sh / 1080)
    local fs = math.max(12, math.floor(options.font_size * scale + 0.5))
    local pad = math.floor(fs * 0.58 + 0.5)
    local gap = math.floor(fs * 0.45 + 0.5)
    local ph = math.floor(fs * 1.66 + 0.5)

    local badges = {}
    if options.show_video == 'yes' and state.video_key and VIDEO_BADGES[state.video_key] then
        badges[#badges + 1] = VIDEO_BADGES[state.video_key]
    end
    if options.show_audio == 'yes' and state.audio_badge and state.audio_badge.badge then
        badges[#badges + 1] = state.audio_badge.badge
    end
    if #badges == 0 then clear_text_overlays() return end

    -- 内容区边界：窗口黑边(osd-dimensions) + 编码黑边(cropdetect)，按 position 停靠
    local crop = state.crop or {}
    local left = sx + (crop.x or 0) * sw
    local top = sy + (crop.y or 0) * sh
    local right = sx + (crop.r or 1) * sw
    local bottom = sy + (crop.b or 1) * sh
    -- 先算总宽（右上锚定需要）
    local total_w = 0
    for i, b in ipairs(badges) do
        total_w = total_w + est_width(b.text, fs) + pad * 2 + (i > 1 and gap or 0)
    end
    local x = POS_LEFT and (left + math.floor(options.margin_x * scale + 0.5))
        or (math.floor(right + 0.5) - total_w - math.floor(options.margin_x * scale + 0.5))
    local y = POS_BOTTOM and (math.floor(bottom + 0.5) - ph - math.floor(options.margin_y * scale + 0.5))
        or (top + math.floor(options.margin_y * scale + 0.5))
    -- 仅左上角与信息条(fntv_mediainfo)同位，需要下移让位
    if POS_LEFT and not POS_BOTTOM then
        y = y + mediainfo_extra_y(scale)
    end

    for i = 1, 2 do
        local b = badges[i]
        if not b then
            pills[i]:remove()
            texts[i]:remove()
        else
            local tw = est_width(b.text, fs)
            local pw = tw + pad * 2
            draw_pill(pills[i], w, h, x, y, pw, ph, b)
            draw_text(texts[i], w, h, x + pad, y + ph / 2, fs, b)
            x = x + pw + gap
        end
    end
end

-- 统一重绘入口：按当前模式分发
local function render()
    if not state.visible then return end
    if state.mode == 'bgra' then
        render_bgra_level(state.bgra_level > 0 and state.bgra_level or #manifest.opacity_levels)
    else
        render_text()
    end
end

-- ─────────────────────────────────────────────────────────────────────
-- 格式识别
-- ─────────────────────────────────────────────────────────────────────

local function selected_track(tp)
    for _, tr in ipairs(mp.get_property_native('track-list') or {}) do
        if tr.type == tp and tr.selected and not tr.image and not tr.albumart then
            return tr
        end
    end
    return nil
end

local function detect_video_key()
    local vtr = selected_track('video')
    if vtr and vtr['dolby-vision-profile'] then return 'dv' end
    if mp.get_property('video-params/transform') == 'dolbyvision' then return 'dv' end
    -- HDR Vivid：fntv 内核 0040 补丁（mpv-Yaozhi 同款）把 ST 2094-60/CUVA 动态元数据
    -- 侧数据透出为 video-params/hdr-vivid 布尔属性；旧内核无此属性时 get_property_bool 落默认 false
    if mp.get_property_bool('video-params/hdr-vivid', false) then return 'vivid' end
    local gamma = tostring(mp.get_property('video-params/gamma', '') or ''):lower()
    if gamma == 'pq' then
        -- HDR10+：上游 mpv 仅在 ST 2094-40 动态元数据存在时才提供 scene-max/scene-avg
        --（静态 HDR10 只有 max-cll/max-fall），属性可用即 HDR10+，无需内核补丁
        if mp.get_property('video-params/scene-avg') then return 'hdr10p' end
        return 'hdr10'
    end
    if gamma == 'hlg' then return 'hlg' end
    if gamma ~= '' then return 'sdr' end
    return nil
end

local function detect_audio_badge()
    local tr = selected_track('audio')
    if not tr then return nil end
    local codec = tostring(tr.codec or ''):lower()
    local raw = table.concat({
        codec,
        tostring(tr['codec-profile'] or ''),
        tostring(tr['decoder-desc'] or ''),
        tostring(tr['codec-desc'] or ''),
        tostring(tr['format-name'] or ''),
        tostring(tr.title or ''),
    }, ' '):lower()
    for _, m in ipairs(AUDIO_MATCHERS) do
        local matched = false
        if m.codecs then
            for _, c in ipairs(m.codecs) do
                if codec == c then matched = true break end
            end
        end
        if not matched and m.pats then
            for _, p in ipairs(m.pats) do
                if raw:find(p, 1, true) then matched = true break end
            end
        end
        if matched then
            return { slug = AUDIO_SLUGS[m.key], badge = AUDIO_BADGES[m.key] }
        end
    end
    if codec ~= '' then
        return { slug = nil, badge = { text = codec:upper(), bg = '4B5563', fg = 'FFFFFF' } }
    end
    return { slug = nil, badge = AUDIO_BADGES.fallback }
end

local function audio_signature()
    local tr = selected_track('audio')
    if not tr then return '' end
    return tostring(tr.id or '') .. '|' .. tostring(tr.codec or '')
        .. '|' .. tostring(tr.title or '') .. '|' .. tostring(tr['codec-profile'] or '')
end

local function compute_badges()
    state.video_key = detect_video_key()
    state.audio_badge = detect_audio_badge()
    state.audio_sig = audio_signature()
    msg.info(string.format('格式识别: video=%s audio=%s',
        state.video_key or 'none',
        state.audio_badge and state.audio_badge.badge.text or 'none'))
end

-- ─────────────────────────────────────────────────────────────────────
-- 编码黑边识别（cropdetect 后台跑，读滤镜元数据，不解析日志）
-- ─────────────────────────────────────────────────────────────────────

local CROP_LABEL = 'fntv-badge'

local function has_crop_vf()
    for _, f in ipairs(mp.get_property_native('vf') or {}) do
        if f.label == CROP_LABEL then return true end
    end
    return false
end

local function remove_crop_vf()
    if state.crop_timer then state.crop_timer:kill() state.crop_timer = nil end
    if has_crop_vf() then
        mp.commandv('vf', 'remove', '@' .. CROP_LABEL)
    end
end

local function finish_cropdetect()
    if state.crop_timer then state.crop_timer:kill() state.crop_timer = nil end
    local md = nil
    if has_crop_vf() then
        -- ⚠️ 必须先读滤镜元数据再摘滤镜：vf 摘除后 vf-metadata 立即失效
        md = mp.get_property_native('vf-metadata/' .. CROP_LABEL) or {}
        mp.commandv('vf', 'remove', '@' .. CROP_LABEL)
    end
    if state.crop_done then return end
    state.crop_done = true
    local vw = mp.get_property_number('video-params/w', 0) or 0
    local vh = mp.get_property_number('video-params/h', 0) or 0
    if vw <= 0 or vh <= 0 or not md then return end
    local cx = tonumber(md['lavfi.cropdetect.x'])
    local cy = tonumber(md['lavfi.cropdetect.y'])
    local cw = tonumber(md['lavfi.cropdetect.w'])
    local ch = tonumber(md['lavfi.cropdetect.h'])
    if not (cx and cy and cw and ch) or cw <= 0 or ch <= 0 then return end
    if cw >= vw and ch >= vh then return end -- 全幅画面，无编码黑边
    -- 旋转画面的比例换算不成立，保守跳过（徽章留在窗口黑边安全区）
    local rot = (mp.get_property_number('video-params/rotate', 0) or 0) % 180
    if rot ~= 0 then return end
    state.crop = { x = cx / vw, y = cy / vh, r = (cx + cw) / vw, b = (cy + ch) / vh }
    msg.info(string.format('编码黑边识别完成: crop=%dx%d+%d+%d (内容区占比 %.1f%%)',
        cw, ch, cx, cy, cw * ch / (vw * vh) * 100))
    if state.visible then render() end
end

local function schedule_cropdetect()
    if options.crop_detect ~= 'yes' or state.crop_done or state.crop_scheduled then return end
    state.crop_scheduled = true
    -- 后台进行：徽章先按窗口黑边安全区显示，识别完成后自动微调进内容区
    local ok = pcall(mp.commandv, 'vf', 'pre',
        '@' .. CROP_LABEL .. ':cropdetect=limit=24/255:round=2:reset=0')
    if not ok or not has_crop_vf() then
        msg.warn('cropdetect 滤镜挂载失败，徽章按窗口黑边安全区显示')
        state.crop_done = true
        return
    end
    state.crop_timer = mp.add_timeout(math.max(0.5, options.crop_detect_duration), finish_cropdetect)
end

-- ─────────────────────────────────────────────────────────────────────
-- 显示控制（bgra 模式带透明度帧淡入淡出）
-- ─────────────────────────────────────────────────────────────────────

local function stop_timers()
    stop_anim_timer()
    if state.hide_timer then state.hide_timer:kill() state.hide_timer = nil end
    if state.refresh_timer then state.refresh_timer:kill() state.refresh_timer = nil end
end

local function hide()
    stop_timers()
    state.visible = false
    state.bgra_level = 0
    clear_overlays()
end

-- bgra 模式：淡入 → 停留 → 淡出（预渲染透明度帧步进）
local function start_bgra_fade_out()
    stop_anim_timer()
    local levels = manifest.opacity_levels
    local count = #levels
    local duration = math.max(0, tonumber(options.fade_out) or 0)
    if duration <= 0 or count <= 1 then
        hide()
        return
    end
    local interval = duration / count
    local index = count - 1
    local function step()
        if not state.visible then return end
        if index < 1 then
            hide()
            return
        end
        render_bgra_level(index)
        index = index - 1
        state.anim_timer = mp.add_timeout(interval, step)
    end
    step()
end

local function start_bgra_fade_in()
    stop_anim_timer()
    local levels = manifest.opacity_levels
    local count = #levels
    local duration = math.max(0, tonumber(options.fade_in) or 0)
    local function hold()
        if not state.visible then return end
        if options.display_duration > 0 then
            state.anim_timer = mp.add_timeout(options.display_duration, function()
                if state.visible then start_bgra_fade_out() end
            end)
        end
    end
    if duration <= 0 or count <= 1 then
        render_bgra_level(count)
        hold()
        return
    end
    local interval = duration / (count - 1)
    local index = 1
    local function step()
        if not state.visible then return end
        render_bgra_level(index)
        if index >= count then
            hold()
            return
        end
        index = index + 1
        state.anim_timer = mp.add_timeout(interval, step)
    end
    step()
end

local function show()
    if not state.on then return end
    if not selected_track('video') then return end -- 纯音频不显示（徽章挂在真实画面上）
    stop_anim_timer()
    if state.hide_timer then state.hide_timer:kill() state.hide_timer = nil end
    state.visible = true
    if state.mode == 'bgra' then
        start_bgra_fade_in()
    else
        render_text()
        if options.display_duration > 0 then
            state.hide_timer = mp.add_timeout(options.display_duration, hide)
        end
    end
    -- 显示期间窗口尺寸可能变化（起播阶段常见），低频重排（保持当前透明度档）
    if state.refresh_timer then state.refresh_timer:kill() end
    state.refresh_timer = mp.add_periodic_timer(0.5, function()
        if state.visible then render() end
    end)
    schedule_cropdetect()
end

local function toggle()
    state.on = not state.on
    publish_on()
    if state.on then show() else hide() end
end

local function reset_for_file()
    remove_crop_vf()
    state.shown_once = false
    state.video_key = nil
    state.audio_badge = nil
    state.audio_sig = ''
    state.crop = nil
    state.crop_done = false
    state.crop_scheduled = false
end

-- ─────────────────────────────────────────────────────────────────────
-- 事件挂接
-- ─────────────────────────────────────────────────────────────────────

mp.register_event('file-loaded', function()
    reset_for_file()
    hide()
    -- 等 video-params 就绪（流媒体可能要几秒），轮询到再显示；识别本身毫秒级，
    -- 绝不阻塞首个可呈现画面（显示时机 = 画面就绪后一个轮询周期内）
    local probe
    probe = mp.add_periodic_timer(0.4, function()
        if (mp.get_property_number('video-params/w', 0) or 0) <= 0 then return end
        if not selected_track('video') then return end
        probe:kill()
        compute_badges()
        state.shown_once = true
        show()
    end)
    -- 兜底：30s 内没有视频参数（纯音频/加载失败）就放弃本轮
    mp.add_timeout(30, function() if probe then probe:kill() end end)
end)

mp.register_event('end-file', function()
    reset_for_file()
    hide()
end)

-- 音轨切换（或音轨迟于画面就绪出现）→ 自动刷新（重新走淡入，双徽章布局自动重算）
mp.observe_property('track-list', nil, function()
    if not state.shown_once then return end
    local sig = audio_signature()
    if sig == state.audio_sig then return end
    state.audio_sig = sig
    compute_badges()
    if options.refresh_on_track_switch ~= 'yes' then return end
    show()
end)

-- 画面标准中途变化（罕见：动态切换流）→ 立即刷新画面标签
mp.observe_property('video-params', nil, function()
    if not state.visible then return end
    local key = detect_video_key()
    if key ~= state.video_key then state.video_key = key end
    render()
end)

mp.register_script_message('fntv_format_badge-show', function()
    state.on = true
    publish_on()
    show()
end)
mp.register_script_message('fntv_format_badge-hide', hide)
mp.register_script_message('fntv_format_badge-toggle', toggle)
mp.register_script_message('fntv_format_badge-dump', function()
    local audio_text = state.audio_badge and state.audio_badge.badge.text or 'none'
    local audio_slug = state.audio_badge and tostring(state.audio_badge.slug or 'none') or 'none'
    msg.info(string.format('dump: on=%s visible=%s mode=%s style=%s pos=%s video=%s audio=%s(%s) crop=%s sig=[%s] once=%s',
        tostring(state.on), tostring(state.visible), state.mode, options.style, POS,
        tostring(state.video_key), audio_text, audio_slug,
        state.crop and string.format('%.3f,%.3f', state.crop.x, state.crop.y) or 'none',
        tostring(state.audio_sig), tostring(state.shown_once)))
end)
if options.toggle_key and options.toggle_key ~= '' then
    mp.add_key_binding(options.toggle_key, 'fntv_format_badge-toggle', toggle)
end

-- 资产加载决定渲染模式（缺失/损坏 → text 回退，且 overlay-add 出错后本次会话不再尝试 bgra）
if load_assets() then
    state.mode = 'bgra'
end

msg.info(string.format('fntv_format_badge loaded (mode=%s, style=%s)',
    state.mode, options.style))
