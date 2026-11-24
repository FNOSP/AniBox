-- fntv_pipeline.lua
-- AniBox · 处理顺序流采集（给播放器「视频详情」弹窗一行用）
-- ───────────────────────────────────────────────────────────────────────
-- 思路借自 Tigerest-Theater 的 proMode「处理顺序」：把一帧从输入到输出
-- 经过的每一站压成一行文字，用户一眼看清当前画质链路，而不是分散在
-- 各个开关里靠猜：
--
--   1920×1080 H264 24fps → 硬解 d3d11va → 着色器 3 个 → 缩放 ewa_lanczossharp 2.0x
--   → 去色带 2 → RIFE 补帧 → 输出 3840×2160 HDR
--
-- 只出现在真正发生的环节（没开去色带就不写、没缩放就不写），
-- 避免一行全是「关」。
--
-- 为什么单独一个脚本：modernz.lua 的主 chunk 已顶到 LuaJIT 的 200 local 上限，
-- 往里塞逻辑会整脚本不加载（所有按钮全死，lc-2328 血训）。这里只负责
-- **采集 + 发布一个字符串**，弹窗侧读属性渲染一行，互不侵入。
--
-- 发布：user-data/anibox/pipeline（弹窗读；idle/无视频时删除该键）
-- 采样：1 秒一次，字符串没变就不写（避免无意义的重绘通知）
-- 配置：script-opts/fntv_pipeline.conf
--   enabled  = yes
--   interval = 1.0       采样间隔（秒）

local mp = require 'mp'
local opt = require 'mp.options'

local conf = {
    enabled = 'yes',
    interval = 1.0,
}
opt.read_options(conf, 'fntv_pipeline')

local KEY = 'user-data/anibox/pipeline'

-- file 里能认出来的插帧引擎（fntv_interp 写 vpy 文件名，按子串识别）
local INTERP_NAMES = {
    { 'rife', 'RIFE 补帧' },
    { 'svpflow', 'SVP 补帧' },
    { 'mvtools', 'MVTools 补帧' },
}

-- vf 里需要单独说明的滤镜（其余只报个数，不逐个列名）
local VF_NAMES = {
    d3d11vpp = 'RTX 增强',
    lavfi = '弹幕渲染',
    sub = '字幕叠加',
    crop = '裁切',
    scale = '缩放滤镜',
    format = '像素格式转换',
}

local function expand(p)
    local ok, r = pcall(function() return mp.command_native({ 'expand-path', p }) end)
    return ok and tostring(r or p) or p
end

-- 纯 UI 着色器（皮肤玻璃挂在 ~~cache/）不算用户的画质链路
local function shader_count()
    local chain = mp.get_property('glsl-shaders') or ''
    if chain == '' then return 0 end
    -- ⚠ 展开失败时 expand('~~cache/') 返回空串，而 string.find(s, '', 1, true) 恒等于 1
    -- —— 空 cache 前缀会把**每一个**着色器都判成 UI 着色器，计数永远是 0。
    -- 所以拿不到 cache 目录时直接不做过滤。
    local cache = expand('~~cache/')
    if cache == '' then cache = nil end
    local n = 0
    for p in chain:gmatch('[^;]+') do
        if not (cache and expand(p):find(cache, 1, true)) then n = n + 1 end
    end
    return n
end

-- 处理顺序：返回 nil=当前没有视频（弹窗不显示这一行）
local function build()
    local vw = mp.get_property_number('video-params/w')
    local vh = mp.get_property_number('video-params/h')
    if not vw or not vh then return nil end

    local parts = {}

    -- 1) 输入：分辨率 + 编码 + 帧率
    --    video-format 在 lavfi/裸帧源上会报 WRAPPED_AVFRAME 这类容器名，
    --    解码器真名要从 track-list 的 codec 拿（没有就退回 video-codec）。
    local head = vw .. '×' .. vh
    local codec = nil
    for _, t in ipairs(mp.get_property_native('track-list') or {}) do
        if t.type == 'video' and t.selected and t.codec then codec = t.codec break end
    end
    codec = codec or mp.get_property('video-codec')
    -- 合成/裸帧源（lavfi 测试图、--vo=image）报的是 wrapped_avframe / rawvideo
    -- 这类「没编码」的名字，写进「处理顺序」只是噪声，直接不显示。
    if codec then
        local c = codec:lower()
        if c == 'wrapped_avframe' or c == 'rawvideo' or c == '' then codec = nil end
    end
    if codec then
        head = head .. ' ' .. (codec:upper():gsub('^H264$', 'H.264'))
    end
    local fps = mp.get_property_number('container-fps')
        or mp.get_property_number('video-params/fps')
    if fps then head = head .. ' ' .. string.format('%.0f', fps) .. 'fps' end
    parts[#parts + 1] = head

    -- 2) 解码
    local hw = mp.get_property('hwdec-current')
    if hw and hw ~= '' and hw ~= 'no' then
        parts[#parts + 1] = '硬解 ' .. hw
    else
        parts[#parts + 1] = '软解'
    end

    -- 3) 滤镜链（含插帧、RTX、弹幕渲染）：插帧单独成站，其余合并计数
    local filters, interp = {}, nil
    for _, vf in ipairs(mp.get_property_native('vf') or {}) do
        if vf.enabled ~= false then
            if vf.name == 'vapoursynth' then
                local vpy = tostring((vf.params or {}).file or ''):lower()
                for _, pair in ipairs(INTERP_NAMES) do
                    if vpy:find(pair[1], 1, true) then interp = pair[2] break end
                end
                interp = interp or 'VapourSynth 处理'
            else
                filters[#filters + 1] = VF_NAMES[vf.name] or vf.name
            end
        end
    end
    if #filters > 0 then
        parts[#parts + 1] = table.concat(filters, ' + ')
    end

    -- 4) 着色器
    local n = shader_count()
    if n > 0 then parts[#parts + 1] = '着色器 ' .. n .. ' 个' end

    -- 5) 缩放：只有输入输出尺寸不同才真实发生
    --    osd-dimensions 在 --no-config 的裸窗口下报的是「视频按宽高比能占满的框」，
    --    不等于真实渲染像素（实测窗口 1280x720 时它报 1920x1080）。真实输出尺寸
    --    优先取 vo-passes 里最后一个 pass 的 w/h —— 那是实际渲染的分辨率。
    local ow, oh
    do
        local passes = mp.get_property_native('vo-passes') or {}
        for _, p in ipairs(passes.fresh or {}) do
            if (p.count or 0) > 0 and p.w and p.h then ow, oh = p.w, p.h end
        end
    end
    if not ow then
        local d = mp.get_property_native('osd-dimensions') or {}
        ow, oh = d.w, d.h
    end
    if ow and oh and (ow ~= vw or oh ~= vh) then
        local s = mp.get_property('scale') or ''
        s = s:gsub('_', '')
        local factor = math.max(ow / vw, oh / vh)
        parts[#parts + 1] = string.format('缩放 %s %.2fx', s, factor)
    end

    -- 6) 去色带
    if mp.get_property_bool('deband') then
        local it = mp.get_property_number('deband-iterations')
        parts[#parts + 1] = '去色带' .. (it and (' ' .. string.format('%.0f', it)) or '')
    end

    -- 7) 插帧（vapoursynth 之外还有内置 interpolation）
    if interp then
        parts[#parts + 1] = interp
    elseif mp.get_property_bool('interpolation') then
        local ts = (mp.get_property('tscale') or ''):gsub('_', '')
        parts[#parts + 1] = '内置平滑' .. (ts ~= '' and (' ' .. ts) or '')
    end

    -- 8) 输出：真实分辨率 + 动态范围
    local out = (ow and oh) and (ow .. '×' .. oh) or '输出'
    local prim = mp.get_property('video-params/primaries') or ''
    local tm = mp.get_property('tone-mapping') or ''
    if prim == 'bt.2020' or prim == 'bt.2100-pq' or prim == 'bt.2100-hlg' then
        out = out .. ' HDR'
    elseif tm ~= '' and tm ~= 'auto' and tm ~= 'clip' and tm ~= 'no' then
        out = out .. ' HDR 映射'
    end
    parts[#parts + 1] = out

    return table.concat(parts, ' → ')
end

local last = nil

local function publish(force)
    if conf.enabled ~= 'yes' then
        if last ~= nil then
            mp.del_property(KEY)
            last = nil
        end
        return
    end
    local text = build()
    if text == last and not force then return end
    last = text
    if text == nil then
        pcall(function() mp.del_property(KEY) end)
    else
        -- native 写：字符串属性用 set_property 回读会带引号（lc-2038 血训）
        pcall(function() mp.set_property_native(KEY, text) end)
    end
end

local timer = mp.add_periodic_timer(tonumber(conf.interval) or 1.0, function()
    publish(false)
end)
timer:stop()

mp.register_event('file-loaded', function()
    publish(true)
    timer:resume()
end)
mp.register_event('end-file', function()
    timer:stop()
    last = nil
    pcall(function() mp.del_property(KEY) end)
end)

-- 这些属性一变就立刻重算（体验上「点开关 → 弹窗那行马上变」）
for _, prop in ipairs({ 'vf', 'glsl-shaders', 'deband', 'deband-iterations',
                        'interpolation', 'tscale', 'video-sync', 'scale',
                        'osd-dimensions', 'video-params', 'hwdec-current' }) do
    mp.observe_property(prop, 'native', function()
        if timer then publish(false) end
    end)
end

mp.log('info', '[fntv_pipeline] 已加载 (enabled=' .. tostring(conf.enabled) .. ')')
