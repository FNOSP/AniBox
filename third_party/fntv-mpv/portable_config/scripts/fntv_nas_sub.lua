-- [lc-1096] MPV「加载字幕」改为 fnOS NAS 外挂字幕菜单（与原生网页字幕菜单同源）。
-- 入口：uosc menus.lua 的 create_track_loader_menu_opener 发现当前 path 是本工程代理地址
-- (127.0.0.1:22346/22347) 时发 script-message fntv-nas-sub-open 委托过来；
-- 播本地文件/外链时仍走 uosc 原本的本地磁盘浏览器，不受影响。
-- 数据链：本脚本 → shim(127.0.0.1:22347)/nas-subtitles → 主进程签名直连 fnOS stream/list；
-- 选中后 /nas-subtitle-file 由主进程下载到本地临时目录，mpv sub-add 本地路径（比 http 直挂稳）。
local mp = require('mp')
local utils = require('mp.utils')
local msg = require('mp.msg')

local MENU_TYPE = 'fntv-nas-sub'

local function qparam(s, key)
    return s:match('[?&]' .. key .. '=([^&]*)')
end

-- 从当前播放 URL（代理地址）解析 itemGuid / token / domain；非代理播放返回 nil
local function current_fn_ctx()
    local path = mp.get_property('path') or ''
    if not path:match('^https?://127%.0%.0%.1:2234[67]/') then return nil end
    local itemGuid = path:match('/playvideo/([^?/&]+)')
    local token = qparam(path, 'token')
    local domain = qparam(path, 'domain')
    if not itemGuid or not token or not domain then return nil end
    -- 这些值在播放 URL 里已是 percent-encoded，原样拼进 shim query（shim 侧解码一次即还原）
    return { itemGuid = itemGuid, token = token, domain = domain }
end

local function fetch_json(api)
    local platform = mp.get_property('platform') or ''
    local res
    if platform == 'windows' then
        res = mp.command_native({
            name = 'subprocess',
            args = { 'powershell', '-NoProfile', '-NonInteractive', '-Command',
                -- [lc-1092] 同款坑：PowerShell 按控制台代码页写 stdout，中文机器(GBK)会把
                -- shim 的 UTF-8 JSON 重编码成乱码；钉成 UTF8 后与本机代码页无关。
                "[Console]::OutputEncoding=[Text.Encoding]::UTF8; try { (Invoke-WebRequest -Uri '" .. api .. "' -UseBasicParsing -TimeoutSec 30).Content } catch { Write-Output ('ERR:' + $_.Exception.Message) }" },
            capture_stdout = true, capture_stderr = true,
        })
    else
        res = mp.command_native({ name = 'subprocess', args = { 'curl', '-sS', '--max-time', '30', api }, capture_stdout = true, capture_stderr = true })
    end
    if not res or res.status ~= 0 then return nil, '请求失败（shim 未启动？）' end
    local body = (res.stdout or ''):gsub('%s+$', '')
    if body:sub(1, 4) == 'ERR:' then return nil, body:sub(5) end
    local ok, parsed = pcall(utils.parse_json, body)
    if not ok or type(parsed) ~= 'table' then return nil, '响应解析失败' end
    if not parsed.ok then return nil, parsed.error or '未知错误' end
    return parsed, nil
end

local function open_menu()
    local ctx = current_fn_ctx()
    if not ctx then
        msg.warn('当前 path 不是本工程代理地址，NAS 字幕菜单不适用')
        return
    end
    local api = ('http://127.0.0.1:22347/nas-subtitles?itemGuid=%s&token=%s&domain=%s'):format(ctx.itemGuid, ctx.token, ctx.domain)
    local data, err = fetch_json(api)
    local list = {}
    if data then
        list = data.items or {}
        -- 与自动挂载同口径：默认字幕优先，其余按标题序
        table.sort(list, function(a, b)
            if (b.is_default or 0) ~= (a.is_default or 0) then return (b.is_default or 0) > (a.is_default or 0) end
            return (a.title or '') < (b.title or '')
        end)
    end

    local items = {}
    if not data then
        items[#items + 1] = { title = '获取 NAS 字幕失败: ' .. (err or '未知错误'), selectable = false, muted = true }
    elseif #list == 0 then
        items[#items + 1] = { title = 'NAS 同目录没有外挂字幕文件（内封轨由播放器自动读取）', selectable = false, muted = true }
    end
    for _, s in ipairs(list) do
        local title = (s.title and s.title ~= '') and s.title or ('(未命名).' .. (s.format or 'srt'))
        items[#items + 1] = {
            title = title,
            hint = ((s.language and s.language ~= '') and (s.language .. ' · ') or '') .. (s.format or ''),
            value = { 'script-message-to', mp.get_script_name(), 'load', s.guid or '' },
        }
    end
    -- [lc-1354] 在线搜索入口：fnOS 服务端按媒体元数据联网搜（客户端不带关键词）
    items[#items + 1] = { title = '⌕ 在线搜索字幕（简体中文）', value = { 'script-message-to', mp.get_script_name(), 'search', 'zh-CN' } }
    items[#items + 1] = { title = '⌕ 在线搜索字幕（English）', value = { 'script-message-to', mp.get_script_name(), 'search', 'en' } }
    items[#items + 1] = { title = '↻ 重新加载', italic = true, value = { 'script-message-to', mp.get_script_name(), 'open' } }
    mp.commandv('script-message-to', 'uosc', 'open-menu', utils.format_json({
        type = MENU_TYPE,
        title = ('加载字幕（NAS · %d 条外挂）'):format(#list),
        items = items,
    }))
end

local function load_subtitle(guid)
    local ctx = current_fn_ctx()
    if not ctx or not guid or guid == '' then return end
    -- [lc-1267] 消息统一放左下角（{\an1}）：左上角是弹幕脚本的状态区，
    --  内置 OSD 默认对齐也是左上，不加定位会叠成两行字
    mp.osd_message('{\\an1}字幕下载中…', 3)
    local api = ('http://127.0.0.1:22347/nas-subtitle-file?itemGuid=%s&token=%s&domain=%s&guid=%s'):format(ctx.itemGuid, ctx.token, ctx.domain, guid)
    local data, err = fetch_json(api)
    if not data then
        mp.osd_message('{\\an1}字幕下载失败: ' .. (err or '未知错误'), 4)
        return
    end
    -- sub-add 签名是 <url> [flags] [title] [lang]：flags 位传 select 让新轨立即生效，标题放第三位
    mp.commandv('sub-add', data.path, 'select', data.title or '')
    mp.osd_message('{\\an1}已加载字幕: ' .. (data.title or ''), 3)
end

-- [lc-1354] trim_id 等服务端生成 ID 防御性 percent-encode（正常是字母数字，只为拼 URL 兜底）
local function enc(s)
    return tostring(s):gsub('[^%w%-%._]', function(c)
        return ('%%%02X'):format(c:byte())
    end)
end

-- [lc-1354] fnOS 在线字幕搜索：shim 转发 POST /v/api/v1/subtitle/search，
-- fnOS 服务端按媒体元数据联网搜，客户端不带关键词（与 FlyNarwhal 同款）。
local function online_search(lan)
    local ctx = current_fn_ctx()
    if not ctx then return end
    mp.osd_message('{\\an1}正在让 fnOS 联网搜索字幕…', 3)
    local api = ('http://127.0.0.1:22347/nas-subtitle-search?itemGuid=%s&token=%s&domain=%s&lan=%s')
        :format(ctx.itemGuid, ctx.token, ctx.domain, enc(lan or 'zh-CN'))
    local data, err = fetch_json(api)
    local list = {}
    if data then list = data.items or {} end

    local items = {}
    if not data then
        items[#items + 1] = { title = '在线搜索失败: ' .. (err or '未知错误'), selectable = false, muted = true }
    elseif #list == 0 then
        items[#items + 1] = { title = 'fnOS 未找到匹配的在线字幕，可换个语言再试', selectable = false, muted = true }
    end
    for _, s in ipairs(list) do
        local bits = {}
        if s.source and s.source ~= '' then bits[#bits + 1] = s.source end
        if s.download and tonumber(s.download) > 0 then bits[#bits + 1] = ('下载 %s 次'):format(s.download) end
        if s.format and s.format ~= '' then bits[#bits + 1] = s.format end
        items[#items + 1] = {
            title = (s.filename and s.filename ~= '') and s.filename or '(未命名)',
            hint = #bits > 0 and table.concat(bits, ' · ') or nil,
            value = { 'script-message-to', mp.get_script_name(), 'online-download', s.trim_id or '' },
        }
    end
    items[#items + 1] = { title = '..', hint = '返回 NAS 字幕菜单', value = { 'script-message-to', mp.get_script_name(), 'open' } }
    local lang_label = lan == 'en' and 'English' or '简体中文'
    mp.commandv('script-message-to', 'uosc', 'open-menu', utils.format_json({
        type = MENU_TYPE .. '-online',
        title = ('在线字幕搜索（%s · %d 条）'):format(lang_label, #list),
        items = items,
    }))
end

-- [lc-1354] fnOS 在线字幕下载：shim 转发 POST /v/api/v1/subtitle/download（服务端抓取并登记），
-- 拿回登记后的新字幕流 guid，主进程直接按 guid 取回本地，sub-add 挂载——
-- 全程不依赖 stream/list 缓存（5 分钟 TTL 看不到刚登记的流）。
local function online_download(trim_id)
    local ctx = current_fn_ctx()
    if not ctx or not trim_id or trim_id == '' then return end
    mp.osd_message('{\\an1}正在下载在线字幕（fnOS 服务端抓取，可能要几秒）…', 5)
    local api = ('http://127.0.0.1:22347/nas-subtitle-download?itemGuid=%s&token=%s&domain=%s&trimId=%s')
        :format(ctx.itemGuid, ctx.token, ctx.domain, enc(trim_id))
    local data, err = fetch_json(api)
    if not data then
        mp.osd_message('{\\an1}在线字幕下载失败: ' .. (err or '未知错误'), 4)
        return
    end
    mp.commandv('sub-add', data.path, 'select', data.title or '')
    mp.osd_message('{\\an1}已加载在线字幕: ' .. (data.title or ''), 3)
end

mp.register_script_message('fntv-nas-sub-open', open_menu)
mp.register_script_message('open', open_menu)
mp.register_script_message('load', load_subtitle)
mp.register_script_message('search', online_search)
mp.register_script_message('online-download', online_download)
