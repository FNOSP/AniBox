-- modified from https://github.com/rkscv/danmaku/blob/main/danmaku.lua
local msg = require('mp.msg')
local utils = require("mp.utils")

-- [lc-1272] 渲染步进：vf_fps=yes 时 0.01s(100Hz)。120Hz 显示器与 100Hz 更新无法整除对齐，
--   有的显示帧重复旧位置、有的帧跳新位置 → 全屏下肉眼可见「左右抖动」。
--   display-fps 观察器会把步进改为「显示器刷新率 ÷2」(60Hz@120屏) 实现整帧对齐。
-- [lc-1303] 步进改为 1/fps + 未知刷新率时兜底 1/60：÷2 那版让弹幕位置每两显示帧才更新一次，
--   60Hz 屏上视频 60fps、弹幕 30fps，用户反馈「一抽一抽的，跟掉帧那样」；
--   原先 display-fps 未知时兜底是 0.001s（1000Hz 空转，白烧一个核）也一并改掉。
-- [lc-1273] 上述步进只对 OSD 回退路径生效。lc-1273 曾把默认路径改为 lavfi=[ass=...] 视频滤镜：
--   mpv 的 OSD overlay 恒以 ass_render_frame(t=0) 渲染（osd_libass.c append_ass 硬编码 0），
--   \move 在 OSD 内完全静止，只能靠 Lua 定时器逐 tick 重算 \pos；而 add_periodic_timer
--   相位与垂直同步无锁，更新落帧间隔抖动 → 全屏（高分辨率下事件循环更忙、定时器更不稳）
--   弹幕左右微抖，窗口模式负载低被掩盖。滤镜路径把弹幕交给视频滤镜链里的 libass，
--   由其按每帧 pts 精确插值 \move（实测 150 帧拟合斜率 -1.3331px/帧 vs 理论 -1.3333，
--   零定时器参与），定时器相位/坐标量化问题整体消失。
-- [lc-1301] 滤镜路径默认停用（options.filter_render=no，回退本 OSD 路径）：滤镜把弹幕烙进
--   视频帧、文字按视频分辨率光栅化，显示分辨率高于视频时整帧上采样 → 弹幕必糊（用户实测
--   「糊得不行」「调字号后又糊」）；每帧 hwdownload/hwupload + fps 补帧也重（弹幕一多就卡）；
--   滤镜图部分时机静默挂载失败 → 「弹幕加载成功却一条不显示」。OSD 路径按屏幕分辨率渲染
--   矢量文字（原版行为，清晰），全屏抖动由 lc-1272 步进对齐抑制。滤镜路径保留可选（=yes）。
-- [lc-1303] 兜底 0.001 → 1/60：display-fps 在窗口建立前是 nil，旧值让定时器以 1000Hz
--   空转（每 tick 都要重建整段 OSD ASS 字符串），是白烧一个核的隐患。
local INTERVAL = options.vf_fps and 0.01 or (1 / 60)
local osd_width, osd_height, pause = 0, 0, true

-- 提取 \move 参数 (x1, y1, x2, y2) 并返回
local function parse_move_tag(text)
    -- 匹配包括小数和负数在内的坐标值
    local x1, y1, x2, y2 = text:match("\\move%((%-?[%d%.]+),%s*(%-?[%d%.]+),%s*(%-?[%d%.]+),%s*(%-?[%d%.]+).*%)")
    if x1 and y1 and x2 and y2 then
        return tonumber(x1), tonumber(y1), tonumber(x2), tonumber(y2)
    end
    return nil
end

local function parse_comment(event, pos, height, delay)
    local x1, y1, x2, y2 = parse_move_tag(event.text)
    local displayarea = tonumber(height * options.displayarea)
    if not x1 then
        local current_x, current_y = event.text:match("\\pos%((%-?[%d%.]+),%s*(%-?[%d%.]+).*%)")
        if not current_y or tonumber(current_y) > displayarea then return end
        if event.style ~= "SP" and event.style ~= "MSG" then
            return string.format("{\\an8}%s", event.text)
        else
            return string.format("{\\an7}%s", event.text)
        end
    end

    -- 计算移动的时间范围
    local duration = event.end_time - event.start_time  --mean: options.scrolltime
    local progress = (pos - event.start_time - delay) / duration  -- 移动进度 [0, 1]

    -- 计算当前坐标
    local current_x = tonumber(x1 + (x2 - x1) * progress)
    local current_y = tonumber(y1 + (y2 - y1) * progress)

    -- 移除 \move 标签并应用当前坐标
    -- [lc-1272] 坐标精度 0.1px→0.01px：全屏后物理分辨率翻倍（overlay 纹理实测 4096 宽），
    --   0.1 逻辑像素的量化误差在物理像素间来回取整 → 弹幕左右抖动；0.01 足够平滑。
    local clean_text = event.text:gsub("\\move%(.-%)", "")
    if current_y > displayarea then return end
    if event.style ~= "SP" and event.style ~= "MSG" then
        return string.format("{\\pos(%.2f,%.2f)\\an8}%s", current_x, current_y, clean_text)
    else
        return string.format("{\\pos(%.2f,%.2f)\\an7}%s", current_x, current_y, clean_text)
    end
end

-- 从 ASS 文件中解析样式和事件
local function parse_ass_events(ass_path, callback)
    local ass_file = io.open(ass_path, "r")
    if not ass_file then
        callback("无法打开 ASS 文件")
        return
    end

    local events = {}
    local time_tolerance = options.merge_tolerance

    for line in ass_file:lines() do
        if line:match("^Dialogue:") then
            local start_time, end_time, style, text = line:match("Dialogue:%s*[^,]*,%s*([^,]*),%s*([^,]*),%s*([^,]*),[^,]*,[^,]*,[^,]*,[^,]*,[^,]*,(.*)")

            if start_time and end_time and text then
                local event = {
                    start_time = time_to_seconds(start_time),
                    end_time = time_to_seconds(end_time),
                    style = style,
                    text = text:gsub("%s+$", ""),
                    clean_text = text:gsub("\\h+", " "):gsub("{[\\=].-}", ""):gsub("^%s*(.-)%s*$", "%1"),
                    pos = text:match("\\pos"),
                    move = text:match("\\move"),
                }

                table.insert(events, event)
            end
        end
    end

    table.sort(events, function(a, b)
        return a.start_time < b.start_time
    end)

    ass_file:close()
    callback(nil, events)
end

local overlay = mp.create_osd_overlay('ass-events')

-- ============================================================
-- [lc-1301] 渲染路径：默认 OSD 定时器路径（原版行为）；滤镜路径由
--           options.filter_render=yes 显式开启（lc-1273 引入，见文件头说明）
-- ============================================================
local FILTER_LABEL = "fntv-danmaku"
-- nil=未探测, true=可用, false=探测失败（本场回退 OSD 路径）
local filter_available
local filter_active = false   -- 弹幕滤镜当前是否挂在 vf 链上
local render_ass_path_cache

local function get_render_ass_path()
    -- DANMAKU_PATH/PID 在 main.lua 尾部才赋值（本模块先于其加载），必须惰性取
    if not render_ass_path_cache then
        render_ass_path_cache = utils.join_path(DANMAKU_PATH, "danmaku-render-" .. PID .. ".ass")
    end
    return render_ass_path_cache
end

-- lavfi 滤镜图内的路径转义：Windows 盘符冒号在滤镜图里是选项分隔符，须 \: 转义，
-- 整体单引号包裹防空格（实测：不转义冒号 → AVFilterGraph "No option name" 解析失败）
local function escape_filter_path(p)
    p = p:gsub("\\", "/")
    p = p:gsub(":", "\\:")
    return "'" .. p .. "'"
end

-- [lc-1297] 图内前置 hwdownload：D3D11VA 硬解帧是 d3d11 硬件表面，ass 滤镜只收软件帧，
-- 硬帧直进 → "Impossible to convert … mpv_src_in0 → auto_scale_0" → 整图配置失败 →
-- mpv 禁用滤镜（vf add 命令本身返回 true，脚本侧无从得知）→ 弹幕加载成功却一条不显示
-- （2026-10-03 用户实测 HEVC 10bit：16391 条弹幕全零显示）。
-- 修法 = 图首插 hwdownload,format=<当前视频像素格式> 把硬帧拉回软件侧再交 ass 渲染；
-- format 必须与 hwdec 输出位深一致（10bit p010→p010le、8bit nv12→nv12；写死 yuv420p 会因
-- 输出回传 GPU 的格式协商失败——实测 hwdownload 后不接 format 也一样失败）。
-- mpv 的 lavfi wrapper 在 ass 渲染后自动把软件帧 hwupload 回 GPU，VO 侧无需干预。
-- 实测（本仓库 mpv v0.41.0-157 + vo=gpu-next + hwdec=auto/d3d11va）：
--   10bit 硬解 hwdownload,format=p010le,ass → VO p010 ✓；8bit 硬解 hwdownload,format=nv12,ass → VO nv12 ✓
--   软解（hwdec-current=no）时帧本就是软件格式，hwdownload 无 hw 帧可下会失败
--   → 仅在 hwdec-current 激活时才插入本段。
local function hw_pixelformat_le(fmt)
    -- video-params/pixelformat 给的是 mpv 名（p010/nv12/yuv420p10…），format= 要 little-endian 全名
    if fmt == "p010" then return "p010le" end
    if fmt == "yuv420p10" then return "yuv420p10le" end
    if fmt == "yuv422p10" then return "yuv422p10le" end
    if fmt == "yuv444p10" then return "yuv444p10le" end
    if fmt == "yuv420p16" then return "yuv420p16le" end
    if fmt == "yuv422p16" then return "yuv422p16le" end
    if fmt == "yuv444p16" then return "yuv444p16le" end
    return fmt
end

local function danmaku_filter_graph()
    local graph = ""
    local hwdec = mp.get_property("hwdec-current")
    if hwdec and hwdec ~= "no" and hwdec ~= "" then
        -- ⚠️ video-params.pixelformat 在硬解时是表面格式（d3d11），不能传给 format=；
        -- 底层像素格式在 video-params.hw-pixelformat（实测 gpu-next+d3d11va: p010/nv12）。
        local params = mp.get_property_native("video-params")
        local pixfmt = params and hw_pixelformat_le(params["hw-pixelformat"] or "") or ""
        if pixfmt and pixfmt ~= "" then
            graph = "hwdownload,format=" .. pixfmt .. ","
        end
    end
    return graph .. "ass=filename=" .. escape_filter_path(get_render_ass_path())
end

local function danmaku_filter_arg()
    return "@" .. FILTER_LABEL .. ":lavfi=[" .. danmaku_filter_graph() .. "]"
end

-- [lc-1289] watch_later 毒化自愈（「黑屏只有声音」根因修复）：
-- mpv 默认 watch-later-options 含 vf，弹幕滤镜挂链期间退出会把整条 vf（含指向按 PID 命名的
-- danmaku-render-<pid>.ass 的 lavfi 滤镜）存进 watch_later；下场播放恢复该链时，本进程的
-- ASS 还没写、旧 PID 的文件已不存在 → lavfi ass "fopen failed" → 整条 vf 链初始化失败 →
-- mpv 直接弃掉视频轨（"Video: no video"，黑屏只有声音；2026-10-01 实测 96 个 watch_later 全中毒）。
-- on_load 在选项恢复之后、滤镜链初始化之前执行，摘掉这两枚脚本自管滤镜即可保住视频轨；
-- 弹幕显示时 show_danmaku_func 会走 write_render_file + vf add 的正常路径重新挂上。
-- mpv.conf 侧已加 watch-later-options=-vf 阻止 vf 再被保存，本钩子兜底存量毒文件。
local function strip_stale_danmaku_filters()
    local filters = mp.get_property_native("vf")
    if type(filters) ~= "table" then return end
    for _, f in ipairs(filters) do
        if f.label == FILTER_LABEL or f.label == "danmaku" then
            mp.commandv("vf", "remove", "@" .. tostring(f.label))
        end
    end
end
mp.add_hook("on_load", 50, strip_stale_danmaku_filters)

-- 虚拟画布(PlayRes)与字号：与 OSD 路径 render() 完全同一套超宽屏修正，
-- 区别仅在画布按视频原生尺寸取比例（滤镜渲染发生在视频帧上，而非 OSD 矩形）
-- [lc-1300] 字号回退原版行为：直接用 options.fontsize（PlayRes 画布由 libass
-- 自动缩放到渲染面，见 utils.lua lc-1300 注释；lc-1299 的显示高补偿是双重缩放）
local function canvas_geometry()
    local width, height = 1920, 1080
    local vw = mp.get_property_number('width') or 0
    local vh = mp.get_property_number('height') or 0
    local ratio = (vw > 0 and vh > 0) and vw / vh
        or (osd_height > 0 and osd_width / osd_height or 16 / 9)
    local fontsize = tonumber(options.fontsize) or 30
    if width / height < ratio then
        height = width / ratio
        fontsize = fontsize - ratio * 2
    end
    return width, height, math.floor(fontsize)
end

-- 从 COMMENTS 重建渲染用 ASS 文件（滤镜 init 时才读文件，任何内容变更都必须重挂滤镜）：
--   ①每条滚动弹幕的 \move 补上事件内时长 (0→dur_ms)：转换器写入的 \move 无时间参数，
--     libass 默认只在前 10s 内插值——这正是旧实现必须逐 tick 重算 \pos 的根因
--   ②显示延迟按段偏移事件时间；③displayarea 屏蔽（滚动弹幕 y 恒定，取 y1 即可）
--   ④&#NNN; 实体清理，与 OSD 路径 parse_comment 后处理语义一致
--      [lc-1303] 原第 ④ 条还含「合并弹幕 {\fs} ×1.5」——已删除：排版端（parse.lua 的
--      get_str_width / 行高 / 追及判据）全部按 ev_fs（含聚合放大 ≤1.3×）算宽度，
--      渲染端再乘 1.5 等于双重缩放：聚合弹幕实际画出来比排出来的位置宽 50%、高 50%，
--      同一行内压到前一条、行高 39px 的行里画 55px 的字直接越轨盖住上下行
--      （用户截图里的重叠、以及「合并弹幕速率跟别的不一样」都是这个 1.5 造成的）。
--   ⑤\an 锚点沿用 OSD 路径约定（SP/MSG 用 7，其余 8；转换器的 move/pos 坐标按中心锚计算）
local function write_render_file()
    local _, height, fontsize = canvas_geometry()
    local displayarea = height * tonumber(options.displayarea)
    local alpha = string.format("%02X", (1 - tonumber(options.opacity)) * 255)
    local bold = options.bold and "1" or "0"
    -- [lc-1300] 描边直接用配置值（原版行为）；lc-1299 的「随字号同比缩放」随双重缩放一并移除
    local outline = tonumber(options.outline) or 1.0
    local shadow = tonumber(options.shadow) or 0.0
    local fontname = options.fontname

    local style_align = { R2L = 7, TOP = 8, BTM = 2, SP = 7, MSG = 7, Default = 7 }
    local style_lines = {}
    for _, name in ipairs({ "R2L", "TOP", "BTM", "SP", "MSG", "Default" }) do
        style_lines[#style_lines + 1] = string.format(
            "Style: %s,%s,%d,&H%sFFFFFF,&H00FFFFFF,&H00000000,&H%s000000,%s,0,0,0,100,100,0,0,1,%.2f,%.2f,%d,0,0,0,1",
            name, fontname, fontsize, alpha, alpha, bold, outline, shadow, style_align[name])
    end

    local header = table.concat({
        "[Script Info]",
        "Title: Fntv-Plus danmaku render",
        "ScriptType: v4.00+",
        "Collisions: Normal",
        "PlayResX: 1920",
        string.format("PlayResY: %d", height),
        "Timer: 100.0000",
        "WrapStyle: 2",
        "ScaledBorderAndShadow: yes",
        "",
        "[V4+ Styles]",
        "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding",
        table.concat(style_lines, "\n"),
        "",
        "[Events]",
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text",
        "",
    }, "\n")

    local lines, count = {}, 0
    for _, event in ipairs(COMMENTS or {}) do
        local delay = get_delay_for_time(DELAYS, event.start_time)
        local start_t = event.start_time + delay
        local end_t = event.end_time + delay
        if end_t > 0 then
            if start_t < 0 then start_t = 0 end
            local text = (event.text or ""):gsub("&#%d+;", "")
            local an = (event.style ~= "SP" and event.style ~= "MSG") and 8 or 7
            local line
            local x1, y1, x2, y2 = parse_move_tag(text)
            if x1 then
                if y1 <= displayarea then
                    local clean = text:gsub("\\move%(.-%)", "")
                    local dur_ms = math.max(1, math.floor((end_t - start_t) * 1000 + 0.5))
                    line = string.format("Dialogue: 0,%s,%s,%s,,0,0,0,,{\\an%d\\move(%s,%s,%s,%s,0,%d)}%s",
                        seconds_to_time(start_t), seconds_to_time(end_t), event.style or "Default",
                        an, x1, y1, x2, y2, dur_ms, clean)
                end
            else
                local _, cy = text:match("\\pos%((%-?[%d%.]+),%s*(%-?[%d%.]+).*%)")
                if cy and tonumber(cy) <= displayarea then
                    line = string.format("Dialogue: 0,%s,%s,%s,,0,0,0,,{\\an%d}%s",
                        seconds_to_time(start_t), seconds_to_time(end_t), event.style or "Default",
                        an, text)
                end
            end
            if line then
                count = count + 1
                lines[#lines + 1] = line
            end
        end
    end

    local f = io.open(get_render_ass_path(), "w")
    if not f then
        msg.warn("[lc-1273] 渲染 ASS 写入失败: " .. get_render_ass_path())
        return nil
    end
    f:write(header .. table.concat(lines, "\n"))
    f:close()
    msg.info(string.format("[lc-1273] 渲染 ASS 重建: %d/%d 条弹幕 → %s",
        count, COMMENTS and #COMMENTS or 0, get_render_ass_path()))
    return true
end

-- [lc-1297] timer 声明必须前置于 fallback_to_osd（其回调引用 timer:resume()），
-- 否则函数捕获的是同名全局(nil)而非本 local → 回退路径一跑就崩。定时器的实际创建
-- 保持在 render() 定义之后（构造参数要传 render），此处先声明占位。
local timer

local function remove_danmaku_filter()
    if filter_active then
        mp.commandv("vf", "remove", "@" .. FILTER_LABEL)
        filter_active = false
    end
end

-- 尝试滤镜路径：成功返回 true；首次尝试即真实挂载（单独探针挂/撤会多两次滤镜链
-- 重配，首显时可感知卡顿）。任何一步失败都置 filter_available=false 永久回退。
-- [lc-1297] ⚠️ vf add 的返回值只代表「命令被接受」，滤镜图的配置是异步的：图配置失败时
-- mpv 会 "Disabling filter" 并把该条目从 vf 属性里剔除（enabled=false / 条目消失），
-- 但 Lua 侧 commandv 仍返回 true。旧实现据此认定成功，硬解弹幕全灭且永不回退。
-- 因此挂链后必须核对 vf 属性确认滤镜真实存活（下一事件循环 tick 内完成），
-- 失效即回退 OSD 定时器路径，保证任何环境（无 interop 的 VO、异常驱动等）都有弹幕。
local function filter_label_alive()
    local filters = mp.get_property_native("vf")
    if type(filters) ~= "table" then return false end
    for _, f in ipairs(filters) do
        if f.label == FILTER_LABEL and f.enabled ~= false then
            return true
        end
    end
    return false
end

local function fallback_to_osd(reason)
    remove_danmaku_filter()
    if filter_available ~= false then
        msg.warn("[lc-1297] lavfi ass 滤镜不可用(" .. reason .. ")，弹幕回退 OSD 定时器渲染")
    end
    filter_available = false
    render()
    if not pause then
        timer:resume()
    end
end

local function try_filter_path()
    if filter_available == false then return false end
    if write_render_file() then
        remove_danmaku_filter()
        if mp.commandv("vf", "add", danmaku_filter_arg()) then
            filter_active = true
            if filter_available ~= true then
                msg.info("[lc-1273] 弹幕走 lavfi ass 滤镜渲染（libass 按帧 pts 插值，全屏抖动根治）")
            end
            filter_available = true
            -- [lc-1297] 挂链后验证：滤镜图配置失败时 mpv 会在下一 tick 内把它从 vf 剔除。
            -- 先同步查一次（快速失败路径），再挂一个一次性定时器兜底查（异步失败路径）。
            if not filter_label_alive() then
                fallback_to_osd("挂链即失效")
                return true  -- 已切到 OSD 路径，调用方无需再走回退分支
            end
            mp.add_timeout(0.25, function()
                if filter_active and filter_available and not filter_label_alive() then
                    fallback_to_osd("挂链后被 mpv 禁用")
                end
            end)
            return true
        end
    end
    if filter_available ~= false then
        msg.warn("[lc-1273] lavfi ass 滤镜不可用，弹幕回退 OSD 定时器渲染")
    end
    filter_available = false
    return false
end

-- ============================================================
-- [lc-1351] 原生 ASS 副字幕轨渲染路径（render_mode=auto 时默认优先）
-- 依赖自编译核心的三个补丁属性（mpv-Yaozhi 0010/0016/0017 补丁系列，已随
-- v0.41.0-1011 核心内置；third_party/mpv-patches/）：
--   secondary-sub-ass-full-viewport : 全屏宽银幕时弹幕映射到完整输出画面
--   secondary-sub-ass-refresh-rate  : 副字幕按显示器刷新率独立重绘 + C 侧墙钟外推
--                                     （弹幕摆脱视频帧率限制，24fps 片源不再 24Hz 抖动）
--   secondary-ass-replace <id> <file>: 弹幕更新原位原子换轨（保留字体缓存、不闪烁）
-- 弹幕以真实 ASS 轨（sub-add → secondary-sid）交给 libass 按帧 pts 插值 \move，
-- 脚本侧零定时器。任一环节不可用即回退 OSD 定时器路径（旧核心行为不变）。
-- 与 Yaozhi 原版的差异：事件仍来自本仓库的 ASS 转换管线（延迟/聚合/占比在
-- write_render_file 中落地）；全屏黑边不切 overlay（直接 full-viewport，
-- 与旧 OSD 路径观感一致且无切换闪烁）；消息系统沿用本仓库的堆叠实现。
-- ============================================================
local REFRESH_OPTION = 'secondary-sub-ass-refresh-rate'
local FULL_VIEWPORT_OPTION = 'secondary-sub-ass-full-viewport'
local native_refresh_available = mp.get_property_native(REFRESH_OPTION) ~= nil
local native_full_viewport_available = mp.get_property_native(FULL_VIEWPORT_OPTION) ~= nil
local native_replace_available = false
for _, command in ipairs(mp.get_property_native('command-list') or {}) do
    if command.name == 'secondary-ass-replace' then native_replace_available = true end
end
local ass_track_id, ass_track_path = nil, nil
local ass_track_failed = false   -- 构建/挂载失败才毒化本场；二字幕占用是临时回退
local ass_track_active = false   -- 当前渲染是否走轨道路径
local changing_secondary = false
local native_full_viewport_active = false
local previous_secondary_sid, previous_secondary_visibility
local previous_secondary_ass_override, previous_secondary_refresh
local previous_secondary_full_viewport

local function is_ass_track_mode()
    return ass_track_active
end

local function is_disabled_sid(value)
    if value == nil or value == false then return true end
    local sid = tostring(value):lower()
    return sid == "" or sid == "no" or sid == "auto" or sid == "0" or sid == "false"
end

local function has_external_secondary_subtitle()
    local sid = mp.get_property_native("secondary-sid")
    if is_disabled_sid(sid) then return false end
    return not ass_track_id or tostring(sid) ~= tostring(ass_track_id)
end

local function get_osd_vertical_geometry()
    local width, height = mp.get_osd_size()
    if not width or width <= 0 or not height or height <= 0 then return nil end
    local dimensions = mp.get_property_native('osd-dimensions', {})
    if type(dimensions) ~= 'table' then return width, height, 0, height end
    local dimensions_height = tonumber(dimensions.h)
    local scale_y = dimensions_height and dimensions_height > 0 and height / dimensions_height or 1
    local top = math.max(0, math.min(height, (tonumber(dimensions.mt) or 0) * scale_y))
    local bottom = math.max(0, math.min(height, height - (tonumber(dimensions.mb) or 0) * scale_y))
    if bottom <= top then return width, height, 0, height end
    return width, height, top, bottom
end

local function should_use_fullscreen_blackbar_overlay()
    if options.fullscreen_blackbar == false then return false end
    if mp.get_property_native('fullscreen') ~= true then return false end
    local width, _, picture_top = get_osd_vertical_geometry()
    if not width then return false end
    return picture_top >= 1
end

-- 副字幕重绘目标频率：显示器原生刷新；>180Hz 面板按 144 封顶整数分频
-- （Yaozhi get_automatic_overlay_divisor 语义，防止超高分频外 Lua/ASS 重绘无上限）
local function get_target_refresh_rate()
    local display_fps = mp.get_property_number("display-fps")
    if not display_fps or display_fps <= 0 then return 60 end
    if display_fps <= 180.5 then return display_fps end
    return display_fps / math.max(2, math.ceil(display_fps / 144 - 0.001))
end

local function set_secondary_property(name, value, native)
    if value == nil then return end
    changing_secondary = true
    if native then
        mp.set_property_native(name, value)
    else
        mp.set_property(name, value)
    end
    changing_secondary = false
end

local function clear_saved_secondary_state()
    previous_secondary_sid = nil
    previous_secondary_visibility = nil
    previous_secondary_ass_override = nil
    previous_secondary_full_viewport = nil
    previous_secondary_refresh = nil
end

local function sync_native_refresh()
    if not native_refresh_available or not ass_track_id then return end
    if has_external_secondary_subtitle() then return end
    local rate = ass_track_active and get_target_refresh_rate() or 0
    if math.abs((mp.get_property_number(REFRESH_OPTION, 0) or 0) - rate) > 0.01 then
        set_secondary_property(REFRESH_OPTION, rate, true)
    end
end

local function sync_native_viewport()
    sync_native_refresh()
    if not native_full_viewport_available or not ass_track_id then return end
    if has_external_secondary_subtitle() then return end
    local enabled = ass_track_active and should_use_fullscreen_blackbar_overlay()
    if mp.get_property_native(FULL_VIEWPORT_OPTION) ~= enabled then
        set_secondary_property(FULL_VIEWPORT_OPTION, enabled, true)
    end
    native_full_viewport_active = enabled
end

local function save_secondary_state()
    if previous_secondary_sid == nil then
        previous_secondary_sid = mp.get_property_native("secondary-sid") or "no"
        previous_secondary_visibility = mp.get_property("secondary-sub-visibility", "yes")
        previous_secondary_ass_override = mp.get_property("secondary-sub-ass-override", "strip")
        if native_full_viewport_available then
            previous_secondary_full_viewport = mp.get_property_native(FULL_VIEWPORT_OPTION)
        end
        if native_refresh_available then
            previous_secondary_refresh = mp.get_property_native(REFRESH_OPTION)
        end
    end
end

local function unload_ass_track(restore_secondary)
    if native_refresh_available and previous_secondary_refresh ~= nil then
        set_secondary_property(REFRESH_OPTION, previous_secondary_refresh, true)
    end
    if native_full_viewport_available and previous_secondary_full_viewport ~= nil then
        set_secondary_property(FULL_VIEWPORT_OPTION, previous_secondary_full_viewport, true)
    end
    native_full_viewport_active = false
    local track_id = ass_track_id
    ass_track_id = nil
    ass_track_path = nil
    if track_id then
        pcall(mp.commandv, "sub-remove", track_id)
    end
    if restore_secondary then
        if previous_secondary_sid ~= nil then
            set_secondary_property("secondary-sid", previous_secondary_sid, true)
            set_secondary_property("secondary-sub-visibility", previous_secondary_visibility)
            set_secondary_property("secondary-sub-ass-override", previous_secondary_ass_override)
        end
        clear_saved_secondary_state()
    end
    if ass_track_failed then
        -- 毒化场合保留渲染文件供 OSD/滤镜路径复用；正常卸载与滤镜路径同生命周期
    end
end

local function find_ass_track_id(existing_ids, path)
    local tracks = mp.get_property_native("track-list") or {}
    for _, track in ipairs(tracks) do
        -- 只认领新挂载的外部轨；内嵌轨没有 external-filename，旧场同名轨也不认
        if track.type == "sub" and track.external and not existing_ids[track.id]
        and (track.title == "uosc_danmaku"
            or (path and track["external-filename"] == path)) then
            return track.id
        end
    end
    return nil
end

local function fallback_track_to_osd(reason)
    unload_ass_track(true)
    ass_track_active = false
    if reason then
        msg.warn("[lc-1351] 原生 ASS 轨不可用(" .. tostring(reason) .. ")，回退 OSD 定时器渲染")
    end
    render()
    if not pause then
        timer:resume()
    end
end

-- 尝试轨道路径：成功返回 true。内容变更（首次/延迟调整/重载）都会走这里：
-- 已有轨且核心支持 secondary-ass-replace 时原位换轨，否则 sub-add 新轨并接管
-- secondary-sid（保存并复原用户副字幕状态，弹幕隐藏/卸载时还原）。
local function try_ass_track_path()
    if not native_refresh_available then return false end
    local requested = tostring(options.render_mode or "auto"):lower()
    if requested == "overlay" then return false end
    if ass_track_failed then return false end
    if has_external_secondary_subtitle() then return false end
    if not COMMENTS or #COMMENTS == 0 then return false end
    if not write_render_file() then
        ass_track_failed = true
        return false
    end

    local track_path = get_render_ass_path()
    if ass_track_id and native_replace_available then
        local ok, result, err = pcall(mp.commandv, 'secondary-ass-replace', ass_track_id, track_path)
        if ok and result ~= false and not (result == nil and err) then
            ass_track_active = true
            sync_native_viewport()
            return true
        end
        msg.warn("[lc-1351] 原位换轨失败，改走 sub-add: " .. tostring(err or result))
    end

    save_secondary_state()
    local existing_ids = {}
    for _, track in ipairs(mp.get_property_native("track-list") or {}) do
        if track.type == "sub" then existing_ids[track.id] = true end
    end
    local ok, result, command_err = pcall(mp.commandv, "sub-add", track_path, "auto", "uosc_danmaku")
    if not ok or result == false or (result == nil and command_err) then
        -- 与 Yaozhi 原版一致：挂载失败毒化本场（每次显示重试只会反复失败）
        ass_track_failed = true
        return false
    end
    local new_id = find_ass_track_id(existing_ids, track_path)
    if not new_id then
        ass_track_failed = true
        return false
    end
    if ass_track_id and ass_track_id ~= new_id then
        pcall(mp.commandv, "sub-remove", ass_track_id)
    end
    ass_track_id, ass_track_path = new_id, track_path
    ass_track_active = true
    set_secondary_property("secondary-sub-visibility", "yes")
    set_secondary_property("secondary-sub-ass-override", "no")
    set_secondary_property("secondary-sid", ass_track_id, true)
    sync_native_viewport()
    -- 轨道接管渲染：停掉 OSD 定时器并清掉残留 overlay
    timer:kill()
    overlay:remove()
    if filter_active then
        remove_danmaku_filter()
    end
    msg.info(string.format("[lc-1351] 弹幕走原生 ASS 副字幕轨 (id=%s, refresh=%.3fHz, full-viewport=%s)",
        tostring(ass_track_id), get_target_refresh_rate(),
        tostring(native_full_viewport_active)))
    return true
end

function render(pos_arg)
    -- [lc-1351] 轨道模式下 libass 按帧 pts 自渲染（seek/暂停/续播均自动对齐），
    -- OSD 定时器路径整体旁路。此函数保留给 OSD 回退路径调用。
    if ass_track_active then return end
    -- [lc-1273] 滤镜路径下无需逐 tick 重画：seek/暂停/续播的弹幕位置由 libass
    -- 按帧 pts 自行对齐。此函数保留给 OSD 回退路径（定时器/换文件/seek 调用）
    if filter_available then return end
    if COMMENTS == nil then return end

    local pos, err = mp.get_property_number('time-pos')
    if err ~= nil then
        return msg.error(err)
    end

    local delay = get_delay_for_time(DELAYS, pos)

    local fontname = options.fontname
    -- [lc-1300] OSD 回退路径同样回退原版字号行为
    local fontsize = tonumber(options.fontsize) or 30
    local alpha = string.format("%02X", (1 - tonumber(options.opacity)) * 255)

    local width, height = 1920, 1080
    local ratio = osd_width / osd_height
    if width / height < ratio then
        height = width / ratio
        fontsize = fontsize - ratio * 2
    end

    local ass_events = {}

    for _, event in ipairs(COMMENTS) do
        if pos >= event.start_time + delay and pos <= event.end_time + delay then
            local text = parse_comment(event, pos, height, delay)
            if text then
                text = text:gsub("&#%d+;","")
            end
            -- [lc-1303] 这里原先把文本里所有 \fs 乘 1.5，已删除。聚合弹幕的内联
            --   {\fsN} 是排版端算好的字号（≤ 基础字号 ×1.3），再乘 1.5 会让它比排版
            --   假设的宽 50%、高 50% —— 同一行内压到相邻弹幕、也盖住上下轨道的字。
            --   行高按 1.3× 建（DanmakuArray:new），字号必须与排版口径一致。

            -- 构建 ASS 字符串
            local ass_text = text and string.format("{\\rDefault\\fn%s\\fs%d\\c&HFFFFFF&\\alpha&H%s\\bord%s\\shad%s\\b%s\\q2}%s",
                fontname, fontsize, alpha, options.outline, options.shadow, options.bold and "1" or "0", text)

            table.insert(ass_events, ass_text)
        end
    end

    overlay.res_x = width
    overlay.res_y = height
    overlay.data = table.concat(ass_events, '\n')
    overlay:update()
end

timer = mp.add_periodic_timer(INTERVAL, render, true)

function parse_danmaku(ass_file_path, from_menu, no_osd)
    parse_ass_events(ass_file_path, function(err, events)
        COMMENTS = events
        if err then
            msg.error("ASS 解析错误: " .. err)
            return
        end

        if ENABLED and (from_menu or get_danmaku_visibility()) then
            if not no_osd then
                show_loaded(true)
            end
            if uosc_available and sync_danmaku_toggle_btn then sync_danmaku_toggle_btn() end
            show_danmaku_func()
        else
            show_message("")
            hide_danmaku_func()
        end
    end)
end

local function filter_state(label, name)
    local filters = mp.get_property_native("vf")
    for _, filter in pairs(filters) do
        if filter.label == label or filter.name == name
        or filter.params[name] ~= nil then
            return true
        end
    end
    return false
end

function show_danmaku_func()
    mp.set_property_bool(HAS_DANMAKU, true)
    -- [lc-1351] 原生 ASS 轨优先：弹幕交给 libass 按显示刷新率重绘，脚本零定时器。
    -- 轨道路径绝不能动视频管线（fps 补帧滤镜只在 OSD 路径下才有意义）。
    if try_ass_track_path() then
        ass_track_active = true
        return
    end
    ass_track_active = false
    -- [lc-1273] fps 滤镜须先于 ass 滤镜入链：重复帧携带新 pts，libass 才能对每个重复帧
    -- 插值一次，弹幕才能随 fps 滤镜获得 60fps 平滑（顺序颠倒会退化为视频原始帧率）
    if options.vf_fps then
        local display_fps = mp.get_property_number('display-fps')
        local video_fps = mp.get_property_number('estimated-vf-fps')
        if not ((display_fps and display_fps < 58) or (video_fps and video_fps > 58)) then
            if not filter_state("danmaku", "fps") then
                mp.commandv("vf", "append", string.format("@danmaku:fps=fps=%s", options.fps))
            end
        end
    end
    if options.filter_render and try_filter_path() then
        return
    end
    -- ===== OSD 渲染路径（lc-1301 起为默认，原版行为）=====
    render()
    if not pause then
        timer:resume()
    end
end

-- [lc-1273] 弹幕延迟等「事件时间轴」变更后的刷新入口：滤镜路径重建文件并重挂
-- （滤镜 init 时才会重新读文件）；OSD 路径直接重画。main.lua 延迟处理调用此函数。
function rebuild_render()
    -- [lc-1351] 轨道模式：重建渲染 ASS 并原位换轨（secondary-ass-replace）
    if ass_track_active then
        if not try_ass_track_path() then
            ass_track_failed = true
            fallback_track_to_osd("重建失败")
        end
        return
    end
    if filter_available then
        if filter_active then
            try_filter_path()
        end
        return
    end
    render()
end

function hide_danmaku_func()
    -- [lc-1351] 先卸载弹幕轨并复原用户副字幕状态（在清 overlay/停表之前）
    if ass_track_id then
        unload_ass_track(true)
    end
    ass_track_active = false
    remove_danmaku_filter()
    timer:kill()
    mp.set_property_bool(HAS_DANMAKU, false)
    overlay:remove()
    if filter_state("danmaku") then
        mp.commandv("vf", "remove", "@danmaku")
    end
end

-- [lc-1267/1268] 消息堆叠 + 占位：每条消息各带到期时间，按行堆在同一处渲染（\N 换行）。
--   sticky（占位）消息不过期、一直显示（如「弹幕加载中...」），任意下一条消息出现时被顶替；
--   普通消息按各自时长堆叠展示，相同文本重复弹出只保留一条。
--   堆叠上限 4 条，超出丢最旧的；全部到期后移除 overlay 并停表。
local message_overlay = mp.create_osd_overlay('ass-events')
local active_messages = {}   -- { { text = ..., expire = ..., sticky = ... } }

local function render_message_block()
    local lines = {}
    for _, m in ipairs(active_messages) do lines[#lines + 1] = m.text end
    local message = string.format("{\\an%d\\pos(%d,%d)}%s", options.message_anlignment,
       options.message_x, options.message_y, table.concat(lines, "\\N"))
    local width, height = 1920, 1080
    local ratio = osd_width / osd_height
    if width / height < ratio then
        height = width / ratio
    end
    message_overlay.res_x = width
    message_overlay.res_y = height
    message_overlay.data = message
    message_overlay:update()
end

-- ⚠️ message_timer 必须前置声明：回调里要用到它自己，而 `local x = f(function() x end)`
-- 的 x 在初始化表达式中尚未进入作用域（会被解析成全局 nil → 回调一执行就崩掉整个脚本）。
local message_timer
message_timer = mp.add_periodic_timer(0.25, function()
    local now = mp.get_time()
    local keep, pruned = {}, false
    for _, m in ipairs(active_messages) do
        if m.expire > now then keep[#keep + 1] = m else pruned = true end
    end
    if #keep ~= #active_messages then active_messages = keep end
    if #active_messages == 0 then
        message_timer:kill()
        message_overlay:remove()
        return
    end
    if pruned then render_message_block() end
end)
message_timer:kill()   -- 空闲时不空转，首条消息弹出时再启动

function show_message(text, time, sticky)
    local expire = sticky and math.huge or (mp.get_time() + (time or 3) + 0.05)
    -- 新消息出现时：顶掉所有占位条 + 同文本旧条
    for i = #active_messages, 1, -1 do
        local m = active_messages[i]
        if m.sticky or m.text == text then table.remove(active_messages, i) end
    end
    if not sticky and #active_messages >= 4 then table.remove(active_messages, 1) end
    active_messages[#active_messages + 1] = { text = text, expire = expire, sticky = sticky }
    render_message_block()
    if not message_timer:is_enabled() then message_timer:resume() end
end

mp.observe_property('osd-width', 'number', function(_, value) osd_width = value or osd_width end)
mp.observe_property('osd-height', 'number', function(_, value) osd_height = value or osd_height end)

-- [lc-1300] 显示环境变化无需重建：字号/描边不再随显示分辨率变化（原版行为，
-- PlayRes 画布由 libass 自动缩放），渲染 ASS 内容与显示几何无关，滤镜无需重挂。
-- [lc-1303] 步进=1/fps（每个显示帧一次，与刷新严格同拍）：
--   原 2/fps 时弹幕位置每两显示帧才更新一次 —— 60Hz 屏上视频走 60fps、弹幕只走 30fps，
--   观感就是用户反馈的「一抽一抽的，跟掉帧那样」；且原实现的 else 分支在 >2000Hz 才会走，
--   判据本身也没意义，直接按刷新率重建即可（下限 1ms 防呆）。
--   [lc-1273] 滤镜路径下无定时器可调，跳过。
mp.observe_property('display-fps', 'number', function(_, value)
    -- [lc-1351] 轨道模式下不重建 OSD 定时器，只需把副字幕重绘率同步到新刷新率
    if ass_track_active then
        sync_native_refresh()
        return
    end
    if filter_available then return end
    if value ~= nil and value > 0 then
        local interval = 1 / value
        if interval < 0.001 then interval = 0.001 end
        timer:kill()
        timer = mp.add_periodic_timer(interval, render, true)
        if ENABLED then
            timer:resume()
        end
    end
end)
mp.observe_property('pause', 'bool', function(_, value)
    if value ~= nil then
        pause = value
    end
    -- [lc-1273] 滤镜路径下弹幕随 pts 冻结（暂停即静止，行为正确），无需停/启定时器
    -- [lc-1351] 轨道模式同理：0016 补丁的播放循环在暂停时自动冻结副字幕时钟
    if ENABLED and not filter_available and not ass_track_active then
        if pause then
            timer:kill()
        elseif COMMENTS ~= nil then
            timer:resume()
        end
    end
end)

-- [lc-1351] 用户在弹幕轨占用副字幕槽时手动选择了真实第二字幕：临时回退 OSD
-- 渲染（不毒化轨道路径，第二字幕关闭后下次显示自动回到轨道）；只复原可见性/
-- 样式覆盖，不抢回 sid 选择权。
mp.observe_property('secondary-sid', 'native', function(_, value)
    if changing_secondary then return end
    if ass_track_active and ass_track_id
        and value ~= nil and not is_disabled_sid(value)
        and tostring(value) ~= tostring(ass_track_id) then
        local visibility = previous_secondary_visibility
        local ass_override = previous_secondary_ass_override
        unload_ass_track(false)
        ass_track_active = false
        set_secondary_property("secondary-sub-visibility", visibility)
        set_secondary_property("secondary-sub-ass-override", ass_override)
        clear_saved_secondary_state()
        if ENABLED and COMMENTS ~= nil then
            msg.verbose("[lc-1351] 检测到第二字幕占用，弹幕临时切换 OSD 兼容渲染")
            render()
            if not pause then
                timer:resume()
            end
            show_message("检测到第二字幕，弹幕已切换兼容渲染", 3)
        end
    end
end)

-- [lc-1351] 全屏/画面几何变化：宽银幕黑边场景下开关 full-viewport（运行时属性，
-- 无需重挂轨道），并同步副字幕重绘率
mp.observe_property('fullscreen', 'bool', function()
    if ass_track_active then sync_native_viewport() end
end)
mp.observe_property('osd-dimensions', 'native', function()
    if ass_track_active then sync_native_viewport() end
end)

mp.register_event('playback-restart', function(event)
    if event.error then
        return msg.error(event.error)
    end
    if ENABLED and COMMENTS ~= nil then
        render()
    end
end)

mp.add_hook("on_unload", 50, function()
    COMMENTS, DELAY = nil, 0
    -- [lc-1351] 先卸载弹幕轨（sub-remove + 复原 secondary-sid/可见性/重绘率），
    -- 再走既有的滤镜/文件清理。unload_ass_track 不删渲染文件，由下方统一删除。
    if ass_track_id then
        unload_ass_track(true)
    end
    ass_track_active = false
    timer:kill()
    overlay:remove()
    -- [lc-1273] 撤滤镜 + 清理渲染 ASS（与 danmaku-PID.ass 同生命周期）
    remove_danmaku_filter()
    if file_exists(get_render_ass_path()) then
        os.remove(get_render_ass_path())
    end
    mp.set_property_native(DELAY_PROPERTY, 0)
    if filter_state("danmaku") then
        mp.commandv("vf", "remove", "@danmaku")
    end

    local files_to_remove = {
        file1 = utils.join_path(DANMAKU_PATH, "danmaku-" .. PID .. ".json"),
        file2 = utils.join_path(DANMAKU_PATH, "danmaku-" .. PID .. ".ass"),
        file3 = utils.join_path(DANMAKU_PATH, "temp-" .. PID .. ".mp4"),
        file4 = utils.join_path(DANMAKU_PATH, "bahamut-" .. PID .. ".json")
    }

    if options.save_danmaku and file_exists(files_to_remove.file2) then
        save_danmaku(true)
    end

    for _, file in pairs(files_to_remove) do
        if file_exists(file) then
            os.remove(file)
        end
    end

    for _, source in pairs(DANMAKU.sources) do
        if source.fname and source.from and source.from ~= "user_local" and file_exists(source.fname) then
            os.remove(source.fname)
        end
    end
    DANMAKU = {sources = {}, count = 1}
    -- 换文件时重置 B站关联元数据，避免配置面板显示上一集的残留状态
    BILI_INFO = nil
end)
