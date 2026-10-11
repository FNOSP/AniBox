-- render.lua ass-track 行为沙箱验证（luajit 直跑，零 GUI）
-- 双向验证原则：
--   旧核心（补丁属性缺失）→ 绝不 sub-add，回落 OSD 定时器路径
--   新核心（三属性齐全）→ sub-add 接管 secondary-sid + 设置 refresh-rate/full-viewport
--   原位换轨 / sub-add 失败回退 / 用户选第二字幕回退 / 隐藏与卸载复原

local calls = {}          -- 记录 commandv/set_property_native 调用序列
local scenario = {}       -- 每用例覆盖的属性返回值
local observers = {}      -- name -> fn
local timers = {}
local hooks = {}
local events = {}
local overlay_removed = 0
local overlay_updates = 0

local function reset_env(new_scenario)
    calls = {}
    scenario = new_scenario or {}
    overlay_removed = 0
    overlay_updates = 0
    timers = {}
    observers = {}
    hooks = {}
    events = {}
end

-- render.lua 在加载期探测核心能力（native_refresh_available 等），
-- 与真实 mpv 一致：必须先设好场景再加载
local function load_render()
    dofile("modules/render.lua")
    -- 模拟真实播放态（pause 观察器会把模块内 pause 置 false）
    if observers["pause"] then observers["pause"](nil, false) end
end

local function calls_of(name)
    local out = {}
    for _, c in ipairs(calls) do
        if c[1] == name then table.insert(out, c) end
    end
    return out
end

local function called(name, needle)
    for _, c in ipairs(calls) do
        if c[1] == name then
            if not needle then return true end
            for _, v in ipairs(c) do
                if tostring(v) == needle then return true end
            end
        end
    end
    return false
end

-- ── mp 桩件 ──
local msg = { info = function() end, warn = function() end, error = function() end, verbose = function() end, fatal = function() end }
local utils = { join_path = function(a, b) return a .. "/" .. b end }
package.preload["mp.msg"] = function() return msg end
package.preload["mp.utils"] = function() return utils end

local osd_size = { 1920, 1080 }
local mp = {
    msg = msg,
    utils = utils,
    get_property_native = function(key, def)
        if scenario[key] ~= nil then return scenario[key] end
        if key == "command-list" then return scenario._command_list or {} end
        if key == "track-list" then return scenario._track_list or {} end
        if key == "osd-dimensions" then return { h = 1080, mt = 0, mb = 0 } end
        if key == "video-params" then return nil end
        if key == "vf" then return {} end
        return def
    end,
    get_property = function(key, def)
        if scenario["p:" .. key] ~= nil then return scenario["p:" .. key] end
        return def
    end,
    get_property_number = function(key, def)
        if scenario["n:" .. key] ~= nil then return scenario["n:" .. key] end
        return def
    end,
    set_property_native = function(key, value) table.insert(calls, { "set_native", key, value }) end,
    set_property = function(key, value) table.insert(calls, { "set", key, value }) end,
    set_property_bool = function(key, value) table.insert(calls, { "set_bool", key, value }) end,
    commandv = function(name, ...)
        local c = { "cmdv", name, ... }
        table.insert(calls, c)
        -- 可脚本化的命令结果
        if name == "sub-add" then
            if scenario._sub_add_fails then return false end
            if scenario._sub_add_track then
                table.insert(scenario._track_list, scenario._sub_add_track)
            end
            return true
        end
        if name == "secondary-ass-replace" then
            if scenario._replace_fails then return false end
            return true
        end
        return true
    end,
    command = function(name, ...) return mp.commandv(name, ...) end,
    command_native = function() return "." end,
    create_osd_overlay = function()
        return {
            update = function() overlay_updates = overlay_updates + 1 end,
            remove = function() overlay_removed = overlay_removed + 1 end,
        }
    end,
    add_periodic_timer = function(interval, fn)
        local t = { timeout = interval, callback = fn, enabled = true,
            kill = function(self) self.enabled = false end,
            resume = function(self) self.enabled = true end,
            is_enabled = function(self) return self.enabled end }
        if interval < 0.1 then timers["render"] = t else timers["message"] = t end
        return t
    end,
    add_timeout = function(interval, fn)
        local t = { timeout = interval, callback = fn, enabled = true,
            kill = function(self) self.enabled = false end,
            resume = function(self) self.enabled = true end,
            is_enabled = function(self) return self.enabled end }
        timers["timeout"] = t
        return t
    end,
    add_hook = function(name, pri, fn) hooks[name] = fn end,
    register_event = function(name, fn) events[name] = fn end,
    observe_property = function(name, _type, fn) observers[name] = fn end,
    get_osd_size = function() return osd_size[1], osd_size[2] end,
    get_time = function() return 1000.0 end,
    _G_types = true,
}
_G.mp = mp

-- ── main.lua 侧全局桩件 ──
_G.PID = "999"
_G.DANMAKU_PATH = "/tmp"
_G.HAS_DANMAKU = "user-data/test/has-danmaku"
_G.ENABLED = true
_G.COMMENTS = {
    { start_time = 1.0, end_time = 16.0, style = "R2L",
      text = [[{\move(2020,39,-40,39)}{\c&HFFFFFF&}hello]] },
    { start_time = 2.0, end_time = 7.0, style = "TOP",
      text = [[{\pos(960,39)}{\c&HFFFFFF&}topmsg]] },
}
_G.DELAYS = {}
_G.DANMAKU = { sources = {}, count = 1 }
_G.BILI_INFO = nil
_G.uosc_available = false
_G.options = {
    vf_fps = false, fps = "60/1.001", filter_render = false,
    render_mode = "auto", fullscreen_blackbar = true,
    fontname = "Microsoft YaHei", fontsize = 30, opacity = 0.7,
    bold = true, outline = 1.2, shadow = 0, displayarea = 0.85,
    scrolltime = 15, fixtime = 5, opacity = 0.7,
    merge_tolerance = -1, message_anlignment = 7, message_x = 30, message_y = 30,
    density_percent = 100, save_danmaku = false,
}
function _G.time_to_seconds(t) return tonumber(t) or 0 end
function _G.seconds_to_time(s)
    local h = math.floor(s / 3600); local m = math.floor(s / 60) % 60; local sec = s % 60
    return string.format("%d:%02d:%06.3f", h, m, sec)
end
function _G.get_delay_for_time(delays, pos) return 0 end
function _G.file_exists() return false end
function _G.save_danmaku() end
function _G.show_loaded() end
function _G.get_danmaku_visibility() return true end
function _G.sync_danmaku_toggle_btn() end

local passed, failed = 0, 0
local function check(name, cond)
    if cond then passed = passed + 1; print("PASS  " .. name)
    else failed = failed + 1; print("FAIL  " .. name) end
end

-- ── 用例 1：旧核心（补丁属性不存在）→ 绝不 sub-add，走 OSD 定时器 ──
reset_env({ ["n:time-pos"] = 5.0, _command_list = {} })
load_render(); show_danmaku_func()
check("旧核心: 不 sub-add", not called("cmdv", "sub-add"))
check("旧核心: 不写 refresh-rate", not called("set_native", "secondary-sub-ass-refresh-rate"))
check("旧核心: OSD 定时器恢复", timers["render"] and timers["render"]:is_enabled())
check("旧核心: overlay 有内容更新", overlay_updates >= 1)
check("旧核心: track 未知", not called("cmdv", "secondary-ass-replace"))

-- ── 用例 2：新核心 → sub-add 接管，设置三个补丁属性 ──
reset_env({
    ["n:time-pos"] = 5.0,
    ["secondary-sub-ass-refresh-rate"] = 0,
    ["secondary-sub-ass-full-viewport"] = false,
    ["p:secondary-sub-visibility"] = "yes",
    ["p:secondary-sub-ass-override"] = "strip",
    ["secondary-sid"] = "no",
    ["n:display-fps"] = 120.0,
    _command_list = { { name = "secondary-ass-replace" } },
    _sub_add_track = { type = "sub", external = true, id = 7, title = "uosc_danmaku",
        ["external-filename"] = "/tmp/danmaku-render-999.ass" },
    _track_list = {},
})
load_render(); show_danmaku_func()
check("新核心: sub-add 挂轨", called("cmdv", "sub-add"))
check("新核心: sub-add 标题 uosc_danmaku", called("cmdv", "uosc_danmaku"))
check("新核心: secondary-sid=7", called("set_native", "7"))
check("新核心: refresh-rate=120", called("set_native", "secondary-sub-ass-refresh-rate")
    and (function() for _, c in ipairs(calls_of("set_native")) do
        if c[2] == "secondary-sub-ass-refresh-rate" and c[3] == 120 then return true end end end)())
check("新核心: ass-override=no", (function()
    for _, c in ipairs(calls_of("set")) do
        if c[2] == "secondary-sub-ass-override" and c[3] == "no" then return true end
    end end)())
check("新核心: OSD 定时器已停", timers["render"] and not timers["render"]:is_enabled())
check("新核心: overlay 已清", overlay_removed >= 1)
-- 轨道模式下 render() 必须旁路
overlay_updates = 0
render()
check("新核心: render() 旁路（零 overlay 更新）", overlay_updates == 0)

-- ── 用例 3：轨道模式下 rebuild_render → secondary-ass-replace 原位换轨 ──
calls = {}
rebuild_render()
check("重建: secondary-ass-replace 被调用", called("cmdv", "secondary-ass-replace"))
check("重建: 不再 sub-add", not called("cmdv", "sub-add"))

-- ── 用例 4：sub-add 失败 → 回退 OSD，且本场毒化（再次 show 仍不 sub-add）──
reset_env({
    ["n:time-pos"] = 5.0,
    ["secondary-sub-ass-refresh-rate"] = 0,
    ["p:secondary-sub-visibility"] = "yes",
    ["secondary-sid"] = "no",
    ["n:display-fps"] = 60.0,
    _command_list = { { name = "secondary-ass-replace" } },
    _sub_add_fails = true,
    _track_list = {},
})
load_render(); show_danmaku_func()
check("挂轨失败: 回退 OSD（定时器恢复）", timers["render"]:is_enabled())
check("挂轨失败: 确实尝试过 sub-add", called("cmdv", "sub-add"))
calls = {}
show_danmaku_func()
check("挂轨失败: 本场毒化不再尝试 sub-add", not called("cmdv", "sub-add"))
check("挂轨失败: 第二次仍走 OSD", overlay_updates >= 1)
hide_danmaku_func()

-- ── 用例 5：先成功挂轨，再模拟用户选择第二字幕 → 卸轨回退 + 复原可见性 ──
reset_env({
    ["n:time-pos"] = 5.0,
    ["secondary-sub-ass-refresh-rate"] = 0,
    ["p:secondary-sub-visibility"] = "yes",
    ["p:secondary-sub-ass-override"] = "strip",
    ["secondary-sid"] = "no",
    ["n:display-fps"] = 60.0,
    _command_list = { { name = "secondary-ass-replace" } },
    _sub_add_track = { type = "sub", external = true, id = 7, title = "uosc_danmaku",
        ["external-filename"] = "/tmp/danmaku-render-999.ass" },
    _track_list = {},
})
load_render(); show_danmaku_func()
check("用例5前置: 轨道已接管", called("cmdv", "sub-add"))
calls = {}
observers["secondary-sid"](nil, "3")   -- 用户选了 id=3 的真实第二字幕
check("第二字幕: sub-remove 卸轨", called("cmdv", "sub-remove"))
check("第二字幕: 可见性复原 yes", (function()
    for _, c in ipairs(calls_of("set")) do
        if c[2] == "secondary-sub-visibility" and c[3] == "yes" then return true end
    end end)())
check("第二字幕: 回退后定时器恢复", timers["render"]:is_enabled())
overlay_updates = 0; render()
check("第二字幕: OSD 路径恢复渲染", overlay_updates >= 1)

-- ── 用例 6：全屏黑边 → full-viewport 开启 ──
reset_env({
    ["n:time-pos"] = 5.0,
    ["secondary-sub-ass-refresh-rate"] = 0,
    ["secondary-sub-ass-full-viewport"] = false,
    ["p:secondary-sub-visibility"] = "yes",
    ["p:secondary-sub-ass-override"] = "strip",
    ["secondary-sid"] = "no",
    ["n:display-fps"] = 60.0,
    ["fullscreen"] = true,
    _command_list = { { name = "secondary-ass-replace" } },
    _sub_add_track = { type = "sub", external = true, id = 7, title = "uosc_danmaku",
        ["external-filename"] = "/tmp/danmaku-render-999.ass" },
    _track_list = {},
})
scenario["osd-dimensions"] = { h = 1080, mt = 138, mb = 138 }
load_render(); show_danmaku_func()
check("全屏黑边: full-viewport=true", (function()
    for _, c in ipairs(calls_of("set_native")) do
        if c[2] == "secondary-sub-ass-full-viewport" and c[3] == true then return true end
    end end)())

-- ── 用例 7：隐藏弹幕 → 卸轨 + 复原 secondary-sid + 清 refresh-rate ──
reset_env({
    ["n:time-pos"] = 5.0,
    ["secondary-sub-ass-refresh-rate"] = 0,
    ["p:secondary-sub-visibility"] = "yes",
    ["p:secondary-sub-ass-override"] = "strip",
    ["secondary-sid"] = "no",
    ["n:display-fps"] = 60.0,
    _command_list = { { name = "secondary-ass-replace" } },
    _sub_add_track = { type = "sub", external = true, id = 7, title = "uosc_danmaku",
        ["external-filename"] = "/tmp/danmaku-render-999.ass" },
    _track_list = {},
})
load_render(); show_danmaku_func()
calls = {}
hide_danmaku_func()
check("隐藏: sub-remove 卸轨", called("cmdv", "sub-remove"))
check("隐藏: secondary-sid 复原 no", (function()
    for _, c in ipairs(calls_of("set_native")) do
        if c[2] == "secondary-sid" and tostring(c[3]) == "no" then return true end
    end end)())
check("隐藏: refresh-rate 归零", (function()
    for _, c in ipairs(calls_of("set_native")) do
        if c[2] == "secondary-sub-ass-refresh-rate" and c[3] == 0 then return true end
    end end)())

-- ── 用例 8：on_unload 钩子同样卸轨 ──
reset_env({
    ["n:time-pos"] = 5.0,
    ["secondary-sub-ass-refresh-rate"] = 0,
    ["p:secondary-sub-visibility"] = "yes",
    ["p:secondary-sub-ass-override"] = "strip",
    ["secondary-sid"] = "no",
    ["n:display-fps"] = 60.0,
    _command_list = { { name = "secondary-ass-replace" } },
    _sub_add_track = { type = "sub", external = true, id = 7, title = "uosc_danmaku",
        ["external-filename"] = "/tmp/danmaku-render-999.ass" },
    _track_list = {},
})
load_render(); show_danmaku_func()
calls = {}
hooks["on_unload"]()
check("卸载: sub-remove 卸轨", called("cmdv", "sub-remove"))

print(("\n结果: %d 通过, %d 失败"):format(passed, failed))
os.exit(failed > 0 and 1 or 0)
