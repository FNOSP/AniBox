-- fntv_aq.lua
-- Fntv-Plus · 「超分与补帧」自适应画质统一管理
-- ─────────────────────────────────────────────────────────────────────────
-- 安全模型对齐 mpv-Yaozhi 的 adaptive-quality / ai-interpolation（安全边界自研实现）：
--   · 空间增强（超分）与时间平滑（补帧）两项独立、默认关闭；模式 off/auto/performance/quality。
--     默认值由应用「设置面板 > 超分与补帧」写 script-opts/fntv_aq.conf；播放器内菜单改动仅本会话生效。
--   · auto 档按 显卡档位(tier) + 片源分辨率 + 帧率 + 输出负载(守卫) + 当前画面格式(HDR/HLG) 决策。
--   · 通用空间增强只用 SSim / FSRCNNX(/KrigBilateral 色度)；Anime4K、NNEDI3、RAVU 等保持手动，
--     用户手动着色器链（fntv_shader 按钮 / Ctrl+1~9 / mpv-user.conf 预设）不会被自动替换。
--   · RIFE 只在保持片源分辨率与超分质量时建 AI 2× 链路：绝不先降采样再补帧；运行时不可用或
--     触发保护（倍速/VFR/原盘/4K保护/HLG）时回退 mpv 原生平滑（display-resample + interpolation）。
--     注：随包 mpv 内核未内置 VapourSynth，RIFE 需按 script-opts/fntv_aq.conf 的 rife_runtime
--     显式指向可用运行时（含 [fntv-aq-rife] profile）才会尝试建链，默认保持原生平滑。
--   · HDR10/HDR10+（PQ）可走保留色彩的安全路径（着色器在色调映射前的线性域运行）；
--     HLG 画面保持原始色彩链与原生平滑。
--   · 暂停/跳转/播放恢复期间不重建滤镜图：决策签名未变化时完全不写属性，复用既有滤镜图。
-- 配置：script-opts/fntv_aq.conf（应用面板写入）＋播放器「更多」菜单（实时切换）

local mp = require 'mp'
local msg = require 'mp.msg'
local opt = require 'mp.options'
local utils = require 'mp.utils'

local conf = {
    -- 空间增强（超分）：off / auto / performance / quality
    superres_mode = 'off',
    -- 时间平滑（补帧）：off / auto / performance / quality
    smooth_mode = 'off',
    -- RIFE 后端：vulkan / tensorrt（需完整扩展包，仅兼容 NVIDIA RTX）
    rife_backend = 'vulkan',
    -- 显卡档位（应用探测写入）：low / balanced / medium / high / upper_high / auto
    tier = 'auto',
    -- 4K 补帧性能保护：4K 片源不进入重负载方案，过载直接回退，不先降采样
    protect_4k_smooth = 'yes',
    -- 通用色度增强（KrigBilateral）：仅在像素吞吐预算内自动加载
    auto_chroma = 'yes',
    -- 智能去色带：仅 8bit、非 HDR、单位像素帧码率较低时启用（只开不关，不覆盖用户预设）
    smart_deband = 'yes',
    deband_max_bpppf = 0.060,
    -- 中档显卡：FSRCNNX/降采样增强仅用于 ≤30fps 片源；高档放宽到 60fps
    medium_fsrcnnx_max_fps = 30.5,
    high_fsrcnnx_max_fps = 60.5,
    shader_max_fps = 60.5,
    -- 色度增强像素吞吐上限：中档 ≈1080p60，高档 ≈1440p60/4K30
    medium_chroma_max_pixel_rate = 130000000,
    high_chroma_max_pixel_rate = 260000000,
    -- 轻量档（SSimSuperRes）允许的最低放大倍率（低于此值无放大收益，不加载）
    light_min_scale = 1.05,
    -- 完整档（FSRCNNX）允许的最低放大倍率（FSRCNNX 自身触发倍率 1.2 的保守取值）
    full_min_scale = 1.15,
    -- 性能守卫：平均丢帧超阈值连续 N 次才降级；默认只降不升（下一文件从满画质重新评估）
    guard_interval = 2.0,
    guard_start_delay = 8.0,
    guard_change_cooldown = 8.0,
    guard_bad_samples = 2,
    -- RIFE 运行时路径（含 vapoursynth 支持的扩展包目录）；留空=不尝试 AI 补帧（保持原生平滑）
    rife_runtime = '',
}
opt.read_options(conf, 'fntv_aq')

-- ─────────────────────────── 状态 ───────────────────────────

function norm_mode(v)
    if v == 'auto' or v == 'performance' or v == 'quality' then return v end
    return 'off'
end

-- 交叉引用函数前置声明（消息处理器/观察器先注册、函数体后定义，Lua 需显式前向）
local apply_decision, rife_runtime_ok, start_guard, stop_guard, guard_tick, downgrade
local status_text, update_status_osd, open_menu

local state = {
    sr_mode = norm_mode(conf.superres_mode), -- 当前会话生效的超分模式
    sm_mode = norm_mode(conf.smooth_mode),       -- 当前会话生效的补帧模式
    sr_cap = 'full',                         -- 本文件超分上限（守卫降级用；下一文件重置为满画质）
    file_sm_off = false,                     -- 本文件补帧被守卫关闭（下一文件重置）
    applied_key = nil,                       -- 已应用决策的签名（签名不变→不写任何属性）
    aq_chain = '',                           -- 本脚本最后写入的 glsl-shaders（用于区分手动链）
    sm_managed = false,                      -- 是否已托管过补帧属性（未托管前绝不覆写用户预设）
    manual_shaders = false,                  -- 检测到用户手动着色器链（本脚本让路）
    manual_interp = false,                   -- fntv_interp.lua 手动插帧占用（本脚本让路）
    guard = nil,                             -- 守卫定时器
    guard_prev = nil,
    guard_bad = 0,
    guard_cooldown_until = 0,
    last_status = '',
}

local function tier_rank()
    local t = tostring(conf.tier or 'auto')
    if t == 'upper_high' then return 5
    elseif t == 'high' then return 4
    elseif t == 'medium' then return 3
    elseif t == 'low' then return 1
    end
    return 2 -- balanced / auto 兜底：不冒进
end

local function toast(text)
    pcall(function() mp.commandv('show-text', text, 2200) end)
end

local function expand(p)
    local ok, r = pcall(function() return mp.command_native({ 'expand-path', p }) end)
    return ok and r or p
end

local function file_exists(p)
    if not p or p == '' then return false end
    local info = utils.file_info(expand(p))
    return info ~= nil and info.is_file == true
end

local function pnum(name, default)
    local v = mp.get_property_number(name)
    if v == nil then return default end
    return v
end

-- ─────────────────────── 着色器链管理 ───────────────────────

-- 手动着色器链检测：glsl-shaders 当前值既非空、也不是本脚本写入的链 → 视为手动占用。
mp.observe_property('glsl-shaders', 'string', function(_, val)
    local cur = tostring(val or '')
    if cur == '' then
        state.manual_shaders = false
    elseif cur ~= state.aq_chain then
        state.manual_shaders = true
    end
end)

-- fntv_interp.lua 手动占用广播（其 default_on/按钮/右键菜单生效时置位）
mp.register_script_message('fntv-interp-manual', function(v)
    state.manual_interp = (v == 'yes' or v == 'true')
    if state.manual_interp then apply_decision() end
end)

local function shader_path(name)
    return '~~/shaders/' .. name
end

-- 着色器文件存在才进链（缺文件时 mpv 会整条链报错，自动档必须先自检）
local function build_chain(names)
    local out = {}
    for _, n in ipairs(names) do
        if file_exists(shader_path(n)) then out[#out + 1] = shader_path(n) end
    end
    return table.concat(out, ';')
end

local function apply_aq_chain(chain)
    if chain == state.aq_chain and (chain ~= '' or not state.manual_shaders) then return end
    -- 手动链优先：非本脚本设置的链一律不动
    if state.manual_shaders then return end
    state.aq_chain = chain
    pcall(function() mp.set_property('glsl-shaders', chain) end)
end

-- ─────────────────────── 片源与格式探测 ───────────────────────

local function source_info()
    local vp = mp.get_property_native('video-params') or {}
    -- ⚠️ 原生表键是 w/h（width/height 只是子属性别名，native 读取下不存在）
    local w = tonumber(vp.w or vp.width) or 0
    local h = tonumber(vp.h or vp.height) or 0
    local fps = pnum('container-fps', 0)
    -- 像素格式带位深（yuv420p10 → 10）；取不到按 8
    local pf = tostring(vp.pixelformat or '')
    local depth = tonumber(pf:match('p(%d%d)') or pf:match('(%d+)b') or '') or 8
    -- 传输特性：pq=HDR10/HDR10+（含 DV 映射 HDR10 的画面）；hlg=HLG（保持原色链）
    local gamma = tostring(vp.gamma or '')
    local is_hdr = gamma == 'pq'
    local is_hlg = gamma == 'hlg' or gamma == 'arib-std-b67'
    -- 输出放大倍率：min(输出宽/源宽, 输出高/源高)，osd 尺寸不可得时按 1（不放大）
    local ow = pnum('osd-width', 0)
    local oh = pnum('osd-height', 0)
    local scale = 1.0
    if ow > 0 and oh > 0 and w > 0 and h > 0 then
        scale = math.min(ow / w, oh / h)
    end
    return {
        w = w, h = h, fps = fps, depth = depth,
        is_hdr = is_hdr, is_hlg = is_hlg, scale = scale,
        bitrate = pnum('video-bitrate', 0),
        is_disc = (mp.get_property_number('disc-menu-active', 0) or 0) > 0
            or tostring(mp.get_property('disc-menu') or '') ~= '',
        speed = pnum('speed', 1),
    }
end

-- ─────────────────────── 决策与应用 ───────────────────────

-- 超分目标档：none / light(轻量=SSimSuperRes±Krig) / full(FSRCNNX+SSim±Krig) / manual
-- 结果最终受 state.sr_cap（本文件守卫降级上限）钳制
local function decide_sr(src)
    if state.manual_shaders then return 'manual' end
    if state.sr_cap == 'none' then return 'none' end
    local mode = state.sr_mode
    if mode == 'off' then return 'none' end

    -- 全局硬边界：HLG 保持原始色彩链（不做着色器增强）
    if src.is_hlg then return 'none' end
    -- 无片源信息时保守跳过
    if src.w <= 0 or src.h <= 0 then return 'none' end

    local rank = tier_rank()
    local pixels = src.w * src.h

    if mode == 'performance' then
        -- 性能优先：仅轻量校正，且帧率受全局 shader 上限约束
        if src.fps > 0 and src.fps > conf.shader_max_fps then return 'none' end
        if src.scale < conf.light_min_scale then return 'none' end
        return 'light'
    end

    if mode == 'quality' or mode == 'auto' then
        -- 质量档与自动档共用预算边界；auto 再按档位收紧触发条件
        local fsrcnnx_max_fps = rank >= 4 and conf.high_fsrcnnx_max_fps or conf.medium_fsrcnnx_max_fps
        -- 放大倍率不足（≈点对点）时没有超分收益
        if src.scale < conf.light_min_scale then return 'none' end
        -- 4K 保护：≥4K 不进完整档（完整档含 FSRCNNX，4K 放大场景本就少见）
        if pixels >= 3840 * 2160 * 0.9 and conf.protect_4k_smooth == 'yes' then
            if src.fps > 0 and src.fps <= conf.shader_max_fps then return 'light' end
            return 'none'
        end
        -- 完整档（FSRCNNX + SSimDownscaler）：放大倍率、帧率达标才上
        if src.scale >= conf.full_min_scale and src.fps > 0 and src.fps <= fsrcnnx_max_fps then
            return 'full'
        end
        -- 完整档不达标 → 轻量档兜底（低档位自动档仍受帧率上限约束）
        if src.fps <= conf.shader_max_fps then
            if mode == 'quality' or rank >= 3 then return 'light' end
        end
        return 'none'
    end

    return 'none'
end

-- 守卫降级上限钳制：full →(cap=light)→ light →(cap=none)→ none
local function clamp_sr(target)
    if target == 'manual' then return target end
    if state.sr_cap == 'none' then return 'none' end
    if state.sr_cap == 'light' and target == 'full' then return 'light' end
    return target
end

-- 补帧目标档：none / native(原生平滑) / rife(AI 2×，需运行时与安全边界全过) / manual
local function decide_sm(src)
    if state.manual_interp then return 'manual' end
    if state.file_sm_off then return 'none' end
    local mode = state.sm_mode
    if mode == 'off' then return 'none' end
    if mode == 'performance' then return 'native' end

    -- quality / auto：优先 RIFE，安全边界不过或运行时缺失 → 原生平滑
    local rife_ok = rife_runtime_ok()
        and src.speed == 1
        and not src.is_disc
        and not src.is_hlg
        and src.fps > 0
        and not (src.w * src.h >= 3840 * 2160 * 0.9 and conf.protect_4k_smooth == 'yes')
    if rife_ok then return 'rife' end
    return 'native'
end

rife_runtime_ok = function()
    -- 未显式配置运行时路径 → 不尝试（随包内核无 VapourSynth，盲试会污染滤镜图）
    if tostring(conf.rife_runtime or '') == '' then return false end
    return file_exists(conf.rife_runtime)
end

-- 智能去色带：仅 8bit、非 HDR/HLG、单位像素帧码率低、非 8K 的片源启用（只开不关）
local function decide_deband(src)
    if conf.smart_deband ~= 'yes' then return false end
    if src.depth > 8 or src.is_hdr or src.is_hlg then return false end
    if src.w * src.h >= 7680 * 4320 then return false end
    if src.bitrate <= 0 or src.fps <= 0 then return false end
    local bpppf = src.bitrate / (src.w * src.h * src.fps)
    return bpppf <= conf.deband_max_bpppf
end

-- KrigBilateral（4:2:0 色度升频增强）：仅在像素吞吐预算内
local function decide_chroma(src)
    if conf.auto_chroma ~= 'yes' then return false end
    if src.is_hdr or src.is_hlg then return false end
    if src.w <= 0 or src.fps <= 0 then return false end
    local rank = tier_rank()
    local budget = rank >= 4 and conf.high_chroma_max_pixel_rate or conf.medium_chroma_max_pixel_rate
    return src.w * src.h * src.fps <= budget
end

apply_decision = function()
    local src = source_info()
    local sr = clamp_sr(decide_sr(src))
    local sm = decide_sm(src)
    local deband = decide_deband(src)
    local key = table.concat({ sr, sm, deband and '1' or '0', tostring(src.is_hlg) }, '|')

    -- 决策未变 → 一个属性都不碰（暂停/跳转/恢复期间复用现有滤镜图的关键）
    if key == state.applied_key then return end

    -- ── 空间增强 ──
    if sr == 'manual' then
        -- 用户手动链优先：只同步状态，不动 glsl-shaders
        state.aq_chain = tostring(mp.get_property('glsl-shaders') or '')
    else
        local krig = decide_chroma(src) and file_exists(shader_path('KrigBilateral.glsl'))
        local chain = ''
        if sr == 'full' then
            -- 完整档：FSRCNNX（upper_high 且 ≤1080p 源用 16 变体）+ SSimDownscaler
            local fsrcnnx = (tier_rank() >= 5 and src.w <= 1920 and src.h <= 1080
                and file_exists(shader_path('FSRCNNX_x2_16-0-4-1.glsl')))
                and 'FSRCNNX_x2_16-0-4-1.glsl' or 'FSRCNNX_x2_8-0-4-1.glsl'
            local names = { fsrcnnx, 'SSimDownscaler.glsl' }
            if krig then table.insert(names, 1, 'KrigBilateral.glsl') end
            chain = build_chain(names)
        elseif sr == 'light' then
            -- 轻量档：SSimSuperRes 校正内置 scale ± KrigBilateral
            local names = { 'SSimSuperRes.glsl' }
            if krig then table.insert(names, 1, 'KrigBilateral.glsl') end
            chain = build_chain(names)
        end
        apply_aq_chain(chain)
    end

    -- ── 时间平滑 ──
    if sm ~= 'manual' then
        if sm == 'rife' then
            -- AI 2×：整链经 [fntv-aq-rife(-trt)] profile；建链失败自动回退原生平滑
            local profile = conf.rife_backend == 'tensorrt' and 'fntv-aq-rife-trt' or 'fntv-aq-rife'
            local ok = pcall(function() mp.commandv('apply-profile', profile) end)
            local vf = tostring(mp.get_property('vf') or '')
            if not ok or not vf:find('vapoursynth', 1, true) then
                msg.warn('RIFE 建链失败，回退原生平滑')
                sm = 'native'
            end
        end
        if sm ~= 'rife' then
            -- 未托管过补帧且本次目标也是「关闭」→ 不写属性，保留用户 mpv.conf 预设
            if sm == 'native' or state.sm_managed then
                pcall(function()
                    mp.set_property_native('interpolation', sm == 'native')
                    mp.set_property('video-sync', sm == 'native' and 'display-resample' or 'audio')
                end)
                state.sm_managed = true
            end
        end
    end

    -- ── 智能去色带（只开不关，不覆盖用户预设） ──
    if deband then pcall(function() mp.set_property_native('deband', true) end) end

    state.applied_key = key
    start_guard()
    update_status_osd(sr, sm, src, true)
end

-- ─────────────────────── 性能守卫 ───────────────────────

start_guard = function()
    stop_guard()
    state.guard_bad = 0
    state.guard_prev = nil
    state.guard_cooldown_until = mp.get_time() + (tonumber(conf.guard_start_delay) or 8)
    state.guard = mp.add_periodic_timer(tonumber(conf.guard_interval) or 2, guard_tick)
end

stop_guard = function()
    if state.guard then
        pcall(function() state.guard:kill() end)
        state.guard = nil
    end
end

guard_tick = function()
    if mp.get_time() < state.guard_cooldown_until then
        state.guard_prev = nil
        return
    end
    -- 丢帧信号：frame-drop-count（解码/输出综合丢帧，mpv 0.41 起为统一口径）；
    -- vo-drop-frame-count 在本核已并入该口径，读不到按 0 兜底。连续增长 → 实时过载。
    local d = pnum('frame-drop-count', 0) + pnum('vo-drop-frame-count', 0)
    if state.guard_prev ~= nil and d > state.guard_prev then
        state.guard_bad = state.guard_bad + 1
    else
        state.guard_bad = 0
    end
    state.guard_prev = d
    if state.guard_bad >= (tonumber(conf.guard_bad_samples) or 2) then
        state.guard_bad = 0
        state.guard_cooldown_until = mp.get_time() + (tonumber(conf.guard_change_cooldown) or 8)
        state.guard_prev = nil
        downgrade()
    end
end

-- 实时过载降级（仅本文件生效；下一文件从满画质重新评估，对齐 Yaozhi「只降不升」策略）：
-- 完整超分 → 轻量超分 → 关闭；仍过载 → 补帧关闭。
downgrade = function()
    local key = state.applied_key or ''
    if key:find('^full|') then
        state.sr_cap = 'light'
        state.applied_key = nil
        apply_decision()
        toast('超分与补帧：实时过载，超分已降级为轻量档（本文件）')
        return
    end
    if key:find('^light|') then
        state.sr_cap = 'none'
        state.applied_key = nil
        apply_decision()
        toast('超分与补帧：实时过载，超分已关闭（本文件）')
        return
    end
    if key:find('|native|') or key:find('|rife|') then
        state.file_sm_off = true
        state.applied_key = nil
        apply_decision()
        toast('超分与补帧：实时过载，补帧已关闭（本文件）')
    end
end

-- ─────────────────────── OSD / uosc 菜单 ───────────────────────

local MODE_LABEL = { off = '关闭', auto = '自动', performance = '性能', quality = '质量' }
local SR_LABEL = { none = '未启用', light = '轻量(SSim)', full = '完整(FSRCNNX+SSim)', manual = '手动(让路)' }
local SM_LABEL = { none = '未启用', native = '原生平滑', rife = 'RIFE AI 2×', manual = '手动(让路)' }
local TIER_LABEL = { low = '入门', balanced = '均衡', medium = '中档', high = '高档', upper_high = '旗舰', auto = '未知' }

status_text = function()
    local src = source_info()
    local sr = state.applied_key and state.applied_key:match('^([^|]+)|') or 'none'
    local sm = state.applied_key and state.applied_key:match('|([^|]+)|') or 'none'
    local lines = {
        '超分: ' .. MODE_LABEL[state.sr_mode] .. ' → ' .. (SR_LABEL[sr] or sr),
        '补帧: ' .. MODE_LABEL[state.sm_mode] .. ' → ' .. (SM_LABEL[sm] or sm),
        ('片源: %dx%d@%s | 显卡: %s'):format(src.w, src.h, math.floor(src.fps * 10) / 10, TIER_LABEL[tostring(conf.tier)] or '未知'),
        '（模式默认值在应用设置面板；本会话改动仅本次有效）',
    }
    return table.concat(lines, '\n')
end

update_status_osd = function(sr, sm, src, toast_on_change)
    local sr_l = SR_LABEL[sr] or sr
    local sm_l = SM_LABEL[sm] or sm
    local text = ('超分与补帧 ─ 超分:%s · 补帧:%s'):format(sr_l, sm_l)
    if text ~= state.last_status then
        state.last_status = text
        if toast_on_change and (state.sr_mode ~= 'off' or state.sm_mode ~= 'off') then
            toast(text)
        end
    end
end

-- 菜单项状态用 MaterialIconsRound 真图标：模式选择用 radio 圆点，开关行用 toggle 滑块
local RADIO_ON, RADIO_OFF = 'radio_button_checked', 'radio_button_unchecked'
local ON_ICON, OFF_ICON = 'toggle_on', 'toggle_off'

local function menu_item(title, value_cmd, active)
    return {
        title = title,
        icon = active and RADIO_ON or RADIO_OFF,
        active = active == true,
        value = value_cmd,
    }
end

open_menu = function()
    local items = {
        {
            title = '空间增强（超分）',
            items = {
                menu_item('关闭', { 'script-message', 'fntv-aq-sr', 'off' }, state.sr_mode == 'off'),
                menu_item('自动（按显卡与片源）', { 'script-message', 'fntv-aq-sr', 'auto' }, state.sr_mode == 'auto'),
                menu_item('性能优先（SSim 轻量）', { 'script-message', 'fntv-aq-sr', 'performance' }, state.sr_mode == 'performance'),
                menu_item('质量优先（FSRCNNX+SSim）', { 'script-message', 'fntv-aq-sr', 'quality' }, state.sr_mode == 'quality'),
            },
        },
        {
            title = '时间平滑（补帧）',
            items = {
                menu_item('关闭', { 'script-message', 'fntv-aq-sm', 'off' }, state.sm_mode == 'off'),
                menu_item('自动（AI 可用则用）', { 'script-message', 'fntv-aq-sm', 'auto' }, state.sm_mode == 'auto'),
                menu_item('原生平滑（性能优先）', { 'script-message', 'fntv-aq-sm', 'performance' }, state.sm_mode == 'performance'),
                menu_item('质量优先（RIFE 就绪时 AI 2×）', { 'script-message', 'fntv-aq-sm', 'quality' }, state.sm_mode == 'quality'),
            },
        },
        {
            title = 'RIFE 后端',
            items = {
                menu_item('Vulkan（随包/基础）', { 'script-message', 'fntv-aq-backend', 'vulkan' }, conf.rife_backend ~= 'tensorrt'),
                menu_item('TensorRT FP16（需 RTX 扩展包）', { 'script-message', 'fntv-aq-backend', 'tensorrt' }, conf.rife_backend == 'tensorrt'),
            },
        },
        (function()
            local on = conf.protect_4k_smooth == 'yes'
            return {
                title = '4K 补帧保护',
                hint = on and '开启' or '关闭',
                icon = on and ON_ICON or OFF_ICON,
                active = on,
                value = { 'script-message', 'fntv-aq-4k', 'toggle' },
            }
        end)(),
        { title = '当前状态', icon = 'insights', value = { 'script-message', 'fntv-aq-status' } },
    }
    local menu = {
        type = 'fntv_aq_menu',
        title = '超分与补帧',
        items = items,
    }
    local ok = pcall(function()
        mp.commandv('script-message-to', 'uosc', 'open-menu', utils.format_json(menu))
    end)
    if not ok then toast(status_text()) end -- 无 uosc 时退化为 OSD 状态
end

-- ─────────────────────── 模式切换（播放器内，会话级） ───────────────────────

local function publish_modes()
    pcall(function()
        mp.set_property('user-data/fntv_aq/superres-mode', state.sr_mode)
        mp.set_property('user-data/fntv_aq/smooth-mode', state.sm_mode)
    end)
end

local function set_sr_mode(mode)
    state.sr_mode = norm_mode(mode)
    state.sr_cap = 'full' -- 用户显式改档 = 撤销守卫降级
    state.applied_key = nil
    publish_modes()
    apply_decision()
    toast('超分模式: ' .. MODE_LABEL[state.sr_mode] .. '（会话级；默认值在应用设置面板）')
end

local function set_sm_mode(mode)
    state.sm_mode = norm_mode(mode)
    state.file_sm_off = false -- 用户显式改档 = 撤销守卫降级
    state.applied_key = nil
    publish_modes()
    apply_decision()
    toast('补帧模式: ' .. MODE_LABEL[state.sm_mode] .. '（会话级；默认值在应用设置面板）')
end

mp.register_script_message('fntv-aq-sr', function(v) set_sr_mode(v) end)
mp.register_script_message('fntv-aq-sm', function(v) set_sm_mode(v) end)
mp.register_script_message('fntv-aq-backend', function(v)
    conf.rife_backend = (v == 'tensorrt') and 'tensorrt' or 'vulkan'
    toast('RIFE 后端: ' .. (conf.rife_backend == 'tensorrt' and 'TensorRT FP16' or 'Vulkan'))
end)
mp.register_script_message('fntv-aq-4k', function()
    conf.protect_4k_smooth = (conf.protect_4k_smooth == 'yes') and 'no' or 'yes'
    state.applied_key = nil
    apply_decision()
    toast('4K 补帧保护: ' .. (conf.protect_4k_smooth == 'yes' and '开' or '关'))
end)
mp.register_script_message('fntv-aq-status', function() toast(status_text()) end)
mp.register_script_message('fntv-aq-menu', open_menu)
mp.add_key_binding(nil, 'menu', open_menu)

-- ─────────────────────── 生命周期 ───────────────────────

-- 重播管线：暂停/seek 不监听（复用现有滤镜图）；video-params/osd 尺寸变化才重决策。
mp.observe_property('video-params', 'native', function() state.applied_key = nil; apply_decision() end)
mp.observe_property('osd-width', 'native', function() state.applied_key = nil; apply_decision() end)
mp.observe_property('osd-height', 'native', function() state.applied_key = nil; apply_decision() end)
mp.observe_property('speed', 'number', function() state.applied_key = nil; apply_decision() end)

mp.register_event('file-loaded', function()
    -- 下一文件从满画质/满模式重新评估（守卫降级只在本文件生效）
    state.sr_cap = 'full'
    state.file_sm_off = false
    state.applied_key = nil
    -- video-params 可能尚未就绪；observe_property 会兜底再决策
    apply_decision()
end)

mp.register_event('end-file', function()
    stop_guard()
    state.applied_key = nil
    state.aq_chain = ''
    state.sm_managed = false
    state.sr_cap = 'full'
    state.file_sm_off = false
end)

publish_modes()

msg.info(('fntv_aq 已加载 (超分=%s, 补帧=%s, RIFE=%s, tier=%s, 4k保护=%s)')
    :format(state.sr_mode, state.sm_mode, tostring(conf.rife_backend), tostring(conf.tier), tostring(conf.protect_4k_smooth)))
