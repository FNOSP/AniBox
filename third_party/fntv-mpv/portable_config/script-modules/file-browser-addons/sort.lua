local mp = require 'mp'
local msg = require 'mp.msg'
local utils = require 'mp.utils'
local fb = require 'file-browser'
local fb_utils = require 'modules.utils'
local g = require 'modules.globals'

local parser = {
    -- Run before AList (95) and the local file parser (110) so the persisted
    -- mode is applied to the first list returned after mpv starts.
    priority = 94,
    api_version = '1.2.0'
}

local parsers

local sort_modes = {
    { id = 'name_asc', field = 'name', label = '名称↑', descending = false },
    { id = 'name_desc', field = 'name', label = '名称↓', descending = true },
    { id = 'date_asc', field = 'date', label = '日期↑', descending = false },
    { id = 'date_desc', field = 'date', label = '日期↓', descending = true },
}

local state_file = mp.command_native({'expand-path', '~~/files/file_browser_sort_state'})

local sort_mode = 1

-- 恢复上次的排序状态
do
    local f = io.open(state_file, 'r')
    if f then
        local saved = tonumber(f:read('*l'))
        f:close()
        if saved and saved >= 1 and saved <= #sort_modes then
            sort_mode = saved
        end
    end
end

local function save_sort_mode()
    local f = io.open(state_file, 'w')
    if f then
        f:write(tostring(sort_mode))
        f:close()
    end
end

local function publish_sort_mode()
    local mode = sort_modes[sort_mode] or sort_modes[1]
    mp.set_property('user-data/file-browser/sort-mode', mode.id)
    mp.set_property('user-data/file-browser/sort-label', mode.label)
end

publish_sort_mode()

fb_utils.code_fns['^'] = function()
    local mode = sort_modes[sort_mode] or sort_modes[1]
    return '排序:' .. mode.label
end

local function display_width(text)
    local width = 0
    local index = 1
    text = tostring(text or '')
    while index <= #text do
        local byte = text:byte(index)
        if byte < 0x80 then index = index + 1
        elseif byte < 0xE0 then index = index + 2
        elseif byte < 0xF0 then index = index + 3
        else index = index + 4 end
        width = width + (byte < 0x80 and 1 or 2)
    end
    return width
end

local function compact_path(text, max_width)
    text = tostring(text or '')
    if display_width(text) <= max_width then return text end

    local prefix = text:match('^%a:[/\\]') or text:match('^[^/\\]+[/\\]') or ''
    local marker = '…/'
    local budget = math.max(8, max_width - display_width(prefix) - display_width(marker))
    local chars, index = {}, 1
    while index <= #text do
        local byte = text:byte(index)
        local length = byte < 0x80 and 1 or (byte < 0xE0 and 2 or (byte < 0xF0 and 3 or 4))
        local value = text:sub(index, index + length - 1)
        chars[#chars + 1] = {value = value, width = byte < 0x80 and 1 or 2}
        index = index + length
    end

    local suffix, used = {}, 0
    for i = #chars, 1, -1 do
        local char = chars[i]
        if used + char.width > budget then break end
        table.insert(suffix, 1, char.value)
        used = used + char.width
    end
    local tail = table.concat(suffix):gsub('^[/\\]+', '')
    return prefix .. marker .. tail
end

local function header_capacity()
    local osd_width = mp.get_property_number and mp.get_property_number('osd-width', 1920) or 1920
    local osd_height = mp.get_property_number and mp.get_property_number('osd-height', 1080) or 1080
    local aspect = osd_height > 0 and osd_width / osd_height or (16 / 9)
    return math.max(34, math.min(72, math.floor(aspect * 38)))
end

local function header_path(text, max_width)
    text = tostring(text or ''):gsub('\\', '/')
    if text == '' then return 'ROOT' end
    return compact_path(text, max_width)
end

local function tool_text(state)
    local mode = sort_modes[sort_mode] or sort_modes[1]
    return string.format('[ 刷新 ]   [ 排序：%s ]   共 %d 项', mode.label, #(state.list or {}))
end

local function is_inline_header(state)
    local path = state.directory == '' and 'ROOT' or state.directory_label or state.directory or ''
    return display_width(path) + 4 + display_width(tool_text(state)) <= header_capacity()
end

-- Keep the original blue path as the visual anchor. Directory actions occupy
-- the remaining first-row space when possible and fall back to their own row
-- only when the path is too long.
fb_utils.code_fns['~'] = function(_, state)
    local path = state.directory == '' and 'ROOT' or state.directory_label or state.directory or ''
    -- Keep the complete breadcrumb on its own title row. Long paths move the
    -- tools below instead of abbreviating information the user may need.
    return header_path(path, math.huge)
end

fb_utils.code_fns['&'] = function(_, state)
    return is_inline_header(state) and ('    ' .. tool_text(state)) or ''
end

fb_utils.code_fns['!'] = function(_, state)
    return string.rep('-', 48)
end

fb_utils.code_fns['#'] = function(_, state)
    return is_inline_header(state) and '' or tool_text(state)
end

-- Give long breadcrumbs a deliberate two-tier rhythm: title, compact breathing
-- space, secondary tools, then the shared blue divider.
fb_utils.code_fns['|'] = function(_, state)
    return is_inline_header(state) and ''
        or '\\N{\\fs4\\bord0}\\h\\N{\\fs21\\b0\\bord1\\c&HC6B6A3&}'
end

function parser:setup()
    parsers = fb.get_parsers()
end

---Sort items in-place. Shared between parse() and cycle_sort_mode().
---@param items Item[]
---@param directory string
---@param mode table
local function sort_items(items, directory, mode)
    local cache = {}

    local function item_name(item)
        return item.label or item.name or ''
    end

    local function is_parent_item(item)
        local name = item.name or ''
        local label = item.label or ''
        return name == '..' or name == '../' or label == '..' or label == '父文件夹'
    end

    local function reverse(t)
        local i, j = 1, #t
        while i < j do
            t[i], t[j] = t[j], t[i]
            i, j = i + 1, j - 1
        end
    end

    local function parse_alist_modified(value)
        if type(value) ~= 'string' or value == '' then return nil end
        local year, month, day, hour, minute, second =
            value:match('^(%d%d%d%d)%-(%d%d)%-(%d%d)[T ](%d%d):(%d%d):(%d%d)')
        if not year then return nil end

        local timestamp = os.time({
            year = tonumber(year), month = tonumber(month), day = tonumber(day),
            hour = tonumber(hour), min = tonumber(minute), sec = tonumber(second),
            isdst = false,
        })
        if not timestamp then return nil end
        local sign, offset_hour, offset_minute = value:match('([%+%-])(%d%d):?(%d%d)$')
        if sign then
            local offset = (tonumber(offset_hour) or 0) * 3600 + (tonumber(offset_minute) or 0) * 60
            timestamp = timestamp + (sign == '-' and offset or -offset)
        end
        return timestamp
    end

    local function get_file_info(item)
        if cache[item] then return cache[item] end
        local path = fb.get_full_path(item, directory)
        local info = utils.file_info(path) or {}
        if item.alist_modified then info.mtime = parse_alist_modified(item.alist_modified) or info.mtime end
        if item.alist_size ~= nil then info.size = tonumber(item.alist_size) or info.size end
        cache[item] = info
        return info
    end

    local parents, dirs, files, others = {}, {}, {}, {}
    for _, item in ipairs(items) do
        if is_parent_item(item) then
            parents[#parents + 1] = item
        elseif item.type == 'dir' then
            dirs[#dirs + 1] = item
        elseif item.type == 'file' then
            files[#files + 1] = item
        else
            others[#others + 1] = item
        end
    end

    fb_utils.sort(parents)

    if mode.field == 'name' then
        fb_utils.sort(dirs); fb_utils.sort(files); fb_utils.sort(others)
        if mode.descending then reverse(dirs); reverse(files); reverse(others) end
    elseif mode.field == 'date' then
        local function date_cmp(a, b)
            local at = get_file_info(a).mtime or 0
            local bt = get_file_info(b).mtime or 0
            if at == bt then return item_name(a):lower() < item_name(b):lower() end
            if mode.descending then
                return at > bt
            else
                return at < bt
            end
        end
        table.sort(dirs, date_cmp)
        table.sort(files, date_cmp)
        table.sort(others, date_cmp)
    end

    local pos = 1
    for _, group in ipairs({parents, dirs, files, others}) do
        for _, item in ipairs(group) do
            items[pos] = item
            pos = pos + 1
        end
    end
    while items[pos] do
        items[pos] = nil
        pos = pos + 1
    end
end

function parser:parse(directory)
    if fb.get_protocol(directory) then return end
    local list, opts = self:defer(directory)
    if not list then return list, opts end
    opts = opts or {}

    if parsers[opts.id] then
        local parser_name = parsers[opts.id].keybind_name or parsers[opts.id].name
        if parser_name ~= 'file' and parser_name ~= 'alist' then
            return list, opts
        end
    end

    local dir = opts.directory or directory
    local mode = sort_modes[sort_mode] or sort_modes[1]
    sort_items(list, dir, mode)

    opts.sorted = true
    return list, opts
end

local function is_local_fs()
    local parser = g.state.parser
    if not parser then return false end
    local n = parser.keybind_name or parser.name
    return n == 'file'
end

local function is_alist()
    local parser = g.state.parser
    if not parser then return false end
    local n = parser.keybind_name or parser.name
    return n == 'alist'
end

local function apply_current_mode()
    local state = g.state
    local mode = sort_modes[sort_mode] or sort_modes[1]
    local selected_item = state.list[state.selected]
    if #state.list > 0 then sort_items(state.list, state.directory, mode) end
    if selected_item then
        for index, item in ipairs(state.list) do
            if item == selected_item then
                state.selected = index
                break
            end
        end
    end
    publish_sort_mode()
    fb.redraw()
end

local function cycle_sort_mode()
    local all_modes = is_local_fs() or is_alist()

    -- Unknown remote parsers do not expose date metadata, so keep their
    -- cycle limited to deterministic name sorting.
    if not all_modes then
        sort_mode = sort_mode + 1
        if sort_mode > 2 then sort_mode = 1 end
    else
        sort_mode = sort_mode + 1
        if sort_mode > #sort_modes then sort_mode = 1 end
    end

    save_sort_mode()
    apply_current_mode()
end

local function set_sort_mode(mode_id)
    for index, mode in ipairs(sort_modes) do
        if mode.id == mode_id then
            sort_mode = index
            save_sort_mode()
            apply_current_mode()
            return
        end
    end
    msg.warn('Unknown file-browser sort mode: ' .. tostring(mode_id))
end

mp.register_script_message('file_browser/header-click', cycle_sort_mode)
mp.register_script_message('file_browser/sort-cycle', cycle_sort_mode)
mp.register_script_message('file_browser/sort-set', set_sort_mode)

return parser
