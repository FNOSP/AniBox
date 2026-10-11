local mp = require "mp"
local msg = require "mp.msg"
local utils = require "mp.utils"

local fb = require "file-browser"

local http_client_ok, http_client = pcall(require, "alist-http")
if not http_client_ok or not http_client.available then
    local reason = not http_client_ok and http_client or http_client.reason
    msg.warn("AList addon is unavailable in this mpv runtime: " .. tostring(reason))
    return {}
end

local http_request = http_client.request

local config = {
    host = "",
    username = "",
    password = "",
    token = nil,
}

local config_path = mp.command_native({"expand-path", "~~/script-opts/file_browser_alist.conf"})
local last_dir_path = mp.command_native({"expand-path", "~~/files/file_browser_alist_last_dir"})
local archive_cache_dir = mp.command_native({"expand-path", "~~/cache"})
local raw_url_info = {}
local fallback_urls = {}
local archive_requests = {}
local archive_streams = {}
local archive_meta_cache = {}
local autoloading_playlist = false
local forced_title_active = false
local latest_alist_title = nil
local manual_refresh_requests = {}
local title_refresh_generation = 0
local title_refresh_timers = {}

-- Ask AList/OpenList to refresh a directory automatically when it is opened,
-- but avoid repeatedly hitting the storage provider while the user navigates.
local AUTO_REFRESH_INTERVAL = 30
local AUTO_REFRESH_FAILURE_BACKOFF = 300
local LIST_PAGE_SIZE = 200
local auto_refresh_times = {}
local auto_refresh_available = true
local auto_refresh_retry_after = 0

-- Notification overlay (ASS-based, matches uosc theme)
local notify_overlay = mp.create_osd_overlay("ass-events")
local notify_timer = mp.add_timeout(4, function() notify_overlay:remove() end)
notify_timer:kill()

local function show_notification(text, duration)
    duration = duration or 3
    local font = mp.get_property("options/osd-font", "sans-serif")
    local font_size = mp.get_property_number("options/osd-font-size", 24)
    local display_w, display_h = mp.get_osd_size()
    local scale = math.min(display_w / 1280, display_h / 720)
    local fs = math.floor(font_size * scale)
    local margin_x = math.floor(30 * scale)
    local margin_y = math.floor(42 * scale)
    local ass = string.format(
        [[{\an7\pos(%d,%d)\fs%d\fn%s\b1\1c&HF2E655&\3c&H071522&\1a&H00&\bord1.8\q2}%s]],
        margin_x, margin_y, fs, font, text
    )
    notify_overlay.res_x = display_w
    notify_overlay.res_y = display_h
    notify_overlay.data = ass
    notify_overlay:update()
    notify_timer.timeout = duration
    notify_timer:kill()
    notify_timer:resume()
end

local archive_extensions = {
    zip = true, rar = true, ["7z"] = true, tar = true, gz = true, bz2 = true,
    xz = true, arj = true, cab = true, lzh = true, ace = true,
    z = true, tgz = true, tbz = true, txz = true, taz = true, lzma = true,
    zstd = true, dmg = true, squasfs = true,
}

-- libarchive-backed formats exposed as directly playable AList items. Keep
-- this deliberately narrower than archive_extensions: other archives remain
-- hidden until their remote playback behavior has been validated.
local playable_archive_extensions = {
    zip = true, rar = true,
}

local video_extensions = {
    ["264"] = true, ["265"] = true, ["3g2"] = true, ["3gp"] = true, ["3gp2"] = true,
    ["3gpp"] = true, aiff = true, asf = true, avi = true, divx = true, dv = true,
    ["dvr-ms"] = true, evo = true, evob = true, f4v = true, flv = true, h264 = true,
    h265 = true, hdmov = true, hdv = true, hevc = true, m1v = true, m2t = true,
    m2ts = true, m2v = true, m4v = true, mk3d = true, mkv = true, mod = true,
    mov = true, mp2v = true, mp4 = true, mp4v = true, mpe = true, mpeg = true,
    iso = true, mpeg2 = true, mpeg4 = true, mpg = true, mpg4 = true, mpv = true,
    mpv2 = true,
    mts = true, mtv = true, mxf = true, nsv = true, nut = true, ogm = true,
    ogv = true, qt = true, rm = true, rmvb = true, tod = true, trp = true,
    ts = true, vfw = true, vob = true, vro = true, webm = true, wm = true,
    wmv = true, wtv = true, x264 = true, x265 = true, xvid = true, y4m = true,
    yuv = true,
}

local alist_parser = {
    api_version = "1.0.0",
    priority = 95,
    name = "alist",
}

local function trim(str)
    return tostring(str or ""):gsub("^%s+", ""):gsub("%s+$", "")
end

local function normalize_host(host)
    host = trim(host)
    if host ~= "" and not host:match("^https?://") then host = "http://" .. host end
    return host:gsub("/+$", "")
end

local function load_config()
    config.host, config.username, config.password = "", "", ""

    local file = io.open(config_path, "r")
    if not file then return false end

    for line in file:lines() do
        local key, value = line:match("^%s*([^#=]+)%s*=%s*(.-)%s*$")
        if key and value then
            key = trim(key)
            if key == "host" then
                config.host = normalize_host(value)
            elseif key == "username" then
                config.username = trim(value)
            elseif key == "password" then
                config.password = value
            end
        end
    end
    file:close()

    return config.host ~= "" and config.username ~= "" and config.password ~= ""
end

local function url_decode(str)
    return tostring(str or ""):gsub("+", " "):gsub("%%(%x%x)", function(hex)
        return string.char(tonumber(hex, 16))
    end)
end

local function url_encode(s)
    return tostring(s or ""):gsub("([^%w%-%.%_%~])", function(c)
        return string.format("%%%02X", string.byte(c))
    end)
end

local function url_encode_path(path)
    return tostring(path or ""):gsub("([^/]+)", function(part)
        return url_encode(part)
    end)
end

local BASE64_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"

local function base64_encode(value)
    value = tostring(value or "")
    local result = {}
    for index = 1, #value, 3 do
        local a = value:byte(index) or 0
        local b = value:byte(index + 1) or 0
        local c = value:byte(index + 2) or 0
        local packed = a * 65536 + b * 256 + c
        result[#result + 1] = BASE64_ALPHABET:sub(math.floor(packed / 262144) % 64 + 1, math.floor(packed / 262144) % 64 + 1)
        result[#result + 1] = BASE64_ALPHABET:sub(math.floor(packed / 4096) % 64 + 1, math.floor(packed / 4096) % 64 + 1)
        result[#result + 1] = index + 1 <= #value
            and BASE64_ALPHABET:sub(math.floor(packed / 64) % 64 + 1, math.floor(packed / 64) % 64 + 1)
            or "="
        result[#result + 1] = index + 2 <= #value
            and BASE64_ALPHABET:sub(packed % 64 + 1, packed % 64 + 1)
            or "="
    end
    return table.concat(result)
end

local function authorization_header()
    return "Authorization: Basic " .. base64_encode(config.username .. ":" .. config.password)
end

-- Ordinary AList/OpenList media is seekable HTTP, but real cloud providers can
-- fluctuate below the media bitrate after a short startup burst. Keep this
-- policy file-local and memory-only: absorb the burst into a bounded 90-second
-- window, wait for three seconds at startup/underrun, and never turn on a
-- global or ordinary-stream disk cache.
local alist_network_cache_options = {
    ["cache"] = "yes",
    ["cache-pause"] = "yes",
    ["cache-pause-initial"] = "yes",
    ["cache-pause-wait"] = "3",
    ["demuxer-readahead-secs"] = "90",
    ["demuxer-max-bytes"] = "512MiB",
    ["demuxer-max-back-bytes"] = "64MiB",
    ["demuxer-seekable-cache"] = "yes",
}

local function merge_alist_network_cache_options(options)
    for key, value in pairs(alist_network_cache_options) do options[key] = value end
    return options
end

local function apply_alist_network_cache_options()
    for key, value in pairs(alist_network_cache_options) do
        mp.set_property("file-local-options/" .. key, value)
    end
end

local function write_last_dir(directory)
    directory = directory or ""
    if not directory:match("^AList/") then return end
    directory = url_decode(directory)

    local file = io.open(last_dir_path, "w")
    if not file then
        msg.warn("AList: could not write last directory")
        return
    end
    file:write(directory)
    file:close()
end

local function login()
    if not load_config() then return false, "Missing configuration" end

    local body = utils.format_json({
        username = config.username,
        password = config.password,
    })
    local response, err = http_request(config.host .. "/api/auth/login", "POST", {
        ["Content-Type"] = "application/json",
    }, body)

    if not response then return false, err or "Login failed" end

    local json = utils.parse_json(response)
    if not json or json.code ~= 200 or not json.data or not json.data.token then
        return false, json and json.message or "Invalid login response"
    end

    config.token = json.data.token
    return true, nil
end

local function ensure_logged_in()
    if config.token then return true end
    local ok, err = login()
    if not ok then msg.error("AList login failed: " .. tostring(err)) end
    return ok
end

local function is_archive_file(filename)
    local ext = filename and filename:match("%.([%w%-]+)$")
    return ext and archive_extensions[ext:lower()] == true
end

local function is_playable_archive_file(filename)
    local ext = filename and filename:match("%.([%w%-]+)$")
    return ext and playable_archive_extensions[ext:lower()] == true
end

local function is_video_file(filename)
    local ext = filename and filename:match("%.([%w%-]+)$")
    return ext and video_extensions[ext:lower()] == true
end

local archive_audio_extensions = {
    aac = true, ac3 = true, aiff = true, alac = true, ape = true, dts = true,
    eac3 = true, flac = true, m4a = true, mka = true, mp3 = true, ogg = true,
    opus = true, wav = true, wma = true, wv = true,
}

local function is_archive_media_file(filename)
    if is_video_file(filename) then return true end
    local ext = filename and filename:match("%.([%w%-]+)$")
    return ext and archive_audio_extensions[ext:lower()] == true
end

local subtitle_extensions = {
    aqt = true, ass = true, gsub = true, idx = true, jss = true, lrc = true,
    mks = true, pgs = true, pjs = true, psb = true, rt = true, slt = true,
    smi = true, sub = true, sup = true, sbv = true, srt = true, ssa = true,
    ssf = true, ttxt = true, txt = true, usf = true, vt = true, vtt = true,
}

local function is_subtitle_file(filename)
    local ext = filename and filename:match("%.([%w%-]+)$")
    return ext and subtitle_extensions[ext:lower()] == true
end

local function set_playlist_titles(new_titles)
    if not next(new_titles or {}) then return end
    local titles = mp.get_property_native("user-data/playlistmanager/titles") or {}
    for filename, title in pairs(new_titles) do
        if type(filename) == "string" and filename ~= "" and type(title) == "string" and title ~= "" then
            titles[filename] = title
        end
    end
    mp.set_property_native("user-data/playlistmanager/titles", titles)
end

local function set_playlist_title(filename, title)
    if type(filename) ~= "string" or filename == "" then return end
    if type(title) ~= "string" or title == "" then return end
    set_playlist_titles({[filename] = title})
end

local function request_directory(path, force_refresh, page)
    local body = utils.format_json({
        path = path,
        password = "",
        page = page or 1,
        per_page = LIST_PAGE_SIZE,
        refresh = force_refresh == true,
    })
    local function send_request()
        return http_request(config.host .. "/api/fs/list", "POST", {
            ["Content-Type"] = "application/json",
            ["Authorization"] = config.token,
        }, body)
    end

    local response, err = send_request()
    if response then return response, nil end

    config.token = nil
    if ensure_logged_in() then response, err = send_request() end
    return response, err
end

local function is_refresh_capability_error(reason)
    local text = tostring(reason or ""):lower()
    return text:find("permission", 1, true) ~= nil
        or text:find("forbidden", 1, true) ~= nil
        or text:find("unauthorized", 1, true) ~= nil
        or text:find("access denied", 1, true) ~= nil
        or text:find("http status 401", 1, true) ~= nil
        or text:find("http status 403", 1, true) ~= nil
        or text:find("not support", 1, true) ~= nil
        or text:find("unsupported", 1, true) ~= nil
        or text:find("权限", 1, true) ~= nil
        or text:find("无权", 1, true) ~= nil
        or text:find("不支持", 1, true) ~= nil
end

local function should_auto_refresh(path)
    if not auto_refresh_available then return false end

    local now = mp.get_time()
    if now < auto_refresh_retry_after then return false end

    local last_refresh = auto_refresh_times[path]
    if last_refresh and now - last_refresh < AUTO_REFRESH_INTERVAL then return false end

    auto_refresh_times[path] = now
    return true
end

local function list_directory(path, force_refresh)
    if not ensure_logged_in() then return nil, "Cannot login to AList/OpenList", false end

    local response, err = request_directory(path, force_refresh, 1)
    local json = response and utils.parse_json(response) or nil
    local refresh_applied = force_refresh == true and json and json.code == 200 and json.data ~= nil

    if refresh_applied then auto_refresh_retry_after = 0 end

    -- OpenList only permits forced refreshes for accounts with sufficient
    -- write permissions. Fall back to a normal read instead of breaking
    -- browsing for read-only customer accounts or older AList servers.
    if force_refresh and not refresh_applied then
        local reason = (json and json.message) or err or "unsupported"
        if is_refresh_capability_error(reason) then
            auto_refresh_available = false
            msg.warn("AList automatic refresh is not permitted or supported; using normal listings for this session: "
                .. tostring(reason))
        else
            auto_refresh_retry_after = mp.get_time() + AUTO_REFRESH_FAILURE_BACKOFF
            msg.warn("AList automatic refresh failed, falling back to normal listing: " .. tostring(reason))
        end
        response, err = request_directory(path, false, 1)
        json = response and utils.parse_json(response) or nil
    end

    if not response then return nil, err or "Directory read failed", false end
    if not json or json.code ~= 200 or not json.data then
        return nil, json and json.message or "Invalid directory response", false
    end

    -- Some AList/OpenList deployments cap one response at 200 entries even
    -- when per_page=0 is documented as unlimited. Follow data.total so large
    -- directories are never silently truncated.
    local merged, seen = {}, {}
    local function append_content(content)
        local added = 0
        for _, item in ipairs(content or {}) do
            local key = tostring(item.is_dir == true) .. "\0" .. tostring(item.name or "")
            if not seen[key] then
                seen[key] = true
                merged[#merged + 1] = item
                added = added + 1
            end
        end
        return added
    end

    local page = 1
    local page_content = json.data.content or {}
    append_content(page_content)
    local total = tonumber(json.data.total)
    while (total and #merged < total) or (not total and #page_content == LIST_PAGE_SIZE) do
        page = page + 1
        response, err = request_directory(path, false, page)
        local next_json = response and utils.parse_json(response) or nil
        if not response or not next_json or next_json.code ~= 200 or not next_json.data then
            local reason = err or (next_json and next_json.message) or "invalid response"
            return nil, string.format("Directory page %d failed: %s", page, tostring(reason)), false
        end
        page_content = next_json.data.content or {}
        local added = append_content(page_content)
        total = tonumber(next_json.data.total) or total
        if #page_content == 0 then break end
        if added == 0 then
            return nil, string.format(
                "Directory pagination stopped at page %d because the server ignored page parameters",
                page
            ), false
        end
    end
    if total and #merged < total then
        return nil, string.format("AList returned %d of %d directory entries", #merged, total), false
    end
    json.data.content = merged

    return json, nil, refresh_applied
end

local function build_directory_items(path, force_refresh)
    local json, err, refresh_applied = list_directory(path, force_refresh)
    if not json then return nil, err, false end

    -- Separate items into directories, playable files, and subtitles.
    -- ZIP/RAR are represented by a private virtual URL. The on_load hook
    -- resolves that URL through OpenList's archive API before mpv opens media,
    -- avoiding libarchive's expensive remote whole-archive reopen path.
    local dirs, videos, subs = {}, {}, {}
    for _, item in ipairs(json.data.content or {}) do
        if item.is_dir then
            dirs[#dirs + 1] = item
        elseif is_playable_archive_file(item.name) then
            videos[#videos + 1] = item
        elseif not is_archive_file(item.name) then
            if is_video_file(item.name) then
                videos[#videos + 1] = item
            elseif is_subtitle_file(item.name) then
                subs[#subs + 1] = item
            end
        end
    end

    local function dav_url(item_path)
        return config.host .. "/dav" .. url_encode_path(item_path)
    end

    -- Fallback: use Alist /d/ endpoint with sign when WebDAV fails.
    -- /d/ properly triggers sign/Link for proxy backends (115, Aliyun).
    local function d_url(item_path, sign)
        return config.host .. "/d" .. url_encode_path(item_path) .. "?sign=" .. url_encode(sign)
    end

    local dav_auth_header = authorization_header()

    local function archive_url(item_path)
        return "alist-archive://open/" .. url_encode(item_path)
    end

    -- Fuzzy match: find subtitles whose basename contains or is contained by video basename
    local function find_matching_subs(video_name)
        local video_base = video_name:gsub("%.[^%.]+$", ""):lower()
        local matched = {}
        local needs_dav_auth = false
        for _, sub_item in ipairs(subs) do
            local sub_base = sub_item.name:gsub("%.[^%.]+$", ""):lower()
            if video_base:find(sub_base, 1, true) or sub_base:find(video_base, 1, true) then
                local sub_path = (path .. "/" .. sub_item.name):gsub("//+", "/")
                if sub_item.sign and sub_item.hashinfo and sub_item.hashinfo ~= "null" then
                    matched[#matched + 1] = d_url(sub_path, sub_item.sign)
                else
                    matched[#matched + 1] = dav_url(sub_path)
                    needs_dav_auth = true
                end
            end
        end
        return matched, needs_dav_auth
    end

    local items = {}
    local playlist_titles = {}

    -- Add directories
    for _, item in ipairs(dirs) do
        local item_path = (path .. "/" .. item.name):gsub("//+", "/")
        items[#items + 1] = {
            name = item.name .. "/",
            label = item.name,
            type = "dir",
            path = "AList" .. item_path .. "/",
            alist_modified = item.modified or item.updated_at,
            alist_size = tonumber(item.size),
        }
    end

    -- Add video files with matched subtitles
    for _, item in ipairs(videos) do
        local item_path = (path .. "/" .. item.name):gsub("//+", "/")
        local playable_archive = is_playable_archive_file(item.name)
        local matched_subs, subtitle_needs_dav_auth = playable_archive and {} or find_matching_subs(item.name)
        local mpv_opts = {
            ["force-media-title"] = item.name,
        }
        if #matched_subs > 0 then
            local sep = package.config:sub(1, 1) == "\\" and ";" or ":"
            mpv_opts["sub-files"] = table.concat(matched_subs, sep)
        end
        -- hashinfo is a JSON string like '{"sha1":"..."}' for proxy
        -- backends (Ali, 115); null/nil for direct ones (Quark).
        -- Proxy backends need /d/ signed endpoint; direct ones use WebDAV.
        local video_url
        if playable_archive then
            video_url = archive_url(item_path)
            archive_requests[video_url] = {
                archive_path = item_path,
                archive_name = item.name,
                dav_url = dav_url(item_path),
                auth_header = dav_auth_header,
            }
        elseif item.sign and item.hashinfo and item.hashinfo ~= "null" then
            video_url = d_url(item_path, item.sign)
            local fallback_options = {
                ["force-media-title"] = item.name,
                ["http-header-fields"] = dav_auth_header,
            }
            merge_alist_network_cache_options(fallback_options)
            fallback_urls[video_url] = {
                url = dav_url(item_path),
                title = item.name,
                mpv_options = fallback_options,
            }
        else
            video_url = dav_url(item_path)
            mpv_opts["http-header-fields"] = dav_auth_header
        end
        if not playable_archive then merge_alist_network_cache_options(mpv_opts) end
        if subtitle_needs_dav_auth then mpv_opts["http-header-fields"] = dav_auth_header end
        playlist_titles[video_url] = item.name
        local fallback = fallback_urls[video_url]
        if fallback then playlist_titles[fallback.url] = item.name end
        items[#items + 1] = {
            name = item.name,
            label = item.name,
            type = "file",
            path = video_url,
            alist_path = item_path,
            alist_directory = path,
            alist_modified = item.modified or item.updated_at,
            alist_size = tonumber(item.size),
            is_playable_archive = playable_archive,
            mpv_options = mpv_opts,
        }
        raw_url_info[video_url] = {
            name = item.name,
            path = item_path,
            directory = path,
            sub_count = #matched_subs,
            is_playable_archive = playable_archive,
        }
    end

    -- Populate titles before any item is selected. This keeps uosc and
    -- playlistmanager from briefly exposing WebDAV credentials or host URLs.
    set_playlist_titles(playlist_titles)
    return items, nil, refresh_applied
end

local function clear_title_refresh_timers()
    for _, timer in ipairs(title_refresh_timers) do
        timer:kill()
    end
    title_refresh_timers = {}
end

local function is_current_alist_item(expected_path, expected_info)
    local current_path = mp.get_property("path", "")
    if current_path == expected_path then return true end

    local current_info = raw_url_info[current_path]
    return current_info ~= nil and expected_info ~= nil
        and current_info.path == expected_info.path
        and current_info.directory == expected_info.directory
end

local function update_title()
    title_refresh_generation = title_refresh_generation + 1
    local generation = title_refresh_generation
    clear_title_refresh_timers()

    local path = mp.get_property("path", "")
    local info = raw_url_info[path]

    -- Fallback: associate with AList server when URL belongs to it
    if not info and load_config() and config.host ~= "" and path:match("^https?://") then
        local host = config.host:gsub("^https?://", "")
        local host_pattern = host:gsub("([%.%-])", "%%%1")
        local alist_path = path:match("^https?://[^@]+@" .. host_pattern .. "/dav(/.+)$")
                      or path:match("^https?://" .. host_pattern .. "/dav(/.+)$")
                      or path:match("^" .. host_pattern .. "/d(/.+)$")
                      or path:match("^" .. host_pattern .. "/p(/.+)$")
                      or path:match("^https?://" .. host_pattern .. "/d(/.+)$")
        if alist_path then
            alist_path = url_decode(alist_path:gsub("[?#].*$", ""))
            local name = alist_path:match("/([^/]+)$") or alist_path
            local dir = alist_path:match("^(.*)/[^/]+$") or "/"
            info = {name = name, path = alist_path, directory = dir}
        end
    end

    if not info then
        mp.set_property_bool("user-data/alist/playing", false)
        mp.set_property("user-data/alist/name", "")
        mp.set_property("user-data/alist/path", "")
        mp.set_property("user-data/alist/directory", "")
        latest_alist_title = nil
        if forced_title_active then
            mp.set_property("force-media-title", "")
            forced_title_active = false
        end
        return nil, generation
    end

    mp.set_property_bool("user-data/alist/playing", true)
    mp.set_property("user-data/alist/name", info.name)
    mp.set_property("user-data/alist/path", info.path)
    mp.set_property("user-data/alist/directory", info.directory)
    mp.set_property("force-media-title", info.name)
    forced_title_active = true
    local clean_name = info.name
    latest_alist_title = clean_name
    set_playlist_title(path, clean_name)
    local pos = mp.get_property_number("playlist-pos", -1)
    if pos and pos >= 0 then
        set_playlist_title(mp.get_property("playlist/" .. pos .. "/filename", ""), clean_name)
    end
    pcall(mp.set_property, "media-title", clean_name)
    for _, delay in ipairs({2, 8, 20}) do
        local timer = mp.add_timeout(delay, function()
            -- A previous file can leave delayed title repairs behind. Only the
            -- newest generation may touch the file that originally scheduled it.
            if forced_title_active and generation == title_refresh_generation
                and latest_alist_title == clean_name and is_current_alist_item(path, info) then
                local current_path = mp.get_property("path", "")
                mp.set_property("force-media-title", clean_name)
                set_playlist_title(current_path, clean_name)
                pcall(mp.set_property, "media-title", clean_name)
            end
        end)
        title_refresh_timers[#title_refresh_timers + 1] = timer
    end
    return info, generation
end

local function get_loadfile_options_arg_index()
    local command_list = mp.get_property_native("command-list", {})
    for _, command in ipairs(command_list) do
        if command.name == "loadfile" then
            for i, arg in ipairs(command.args or {}) do
                if arg.name == "options" then return i end
            end
        end
    end
    return 3
end

local function loadfile_with_options(path, flag, options)
    if get_loadfile_options_arg_index() == 3 then
        return mp.command_native({"loadfile", path, flag, options}) ~= nil
    end
    return mp.command_native({"loadfile", path, flag, -1, options}) ~= nil
end

local function append_to_playlist(item)
    local ok = loadfile_with_options(item.path, "append", item.mpv_options)
    if ok and item.label and item.path then set_playlist_title(item.path, item.label) end
    return ok
end

local function request_archive_meta(path)
    local cached = archive_meta_cache[path]
    if cached then return cached, nil end
    if not ensure_logged_in() then return nil, "Cannot login to AList/OpenList" end

    local body = utils.format_json({
        path = path,
        password = "",
        refresh = false,
        archive_pass = "",
    })
    local function send_request()
        return http_request(config.host .. "/api/fs/archive/meta", "POST", {
            ["Content-Type"] = "application/json",
            ["Authorization"] = config.token,
        }, body)
    end

    local response, err = send_request()
    if not response then
        config.token = nil
        if ensure_logged_in() then response, err = send_request() end
    end
    if not response then return nil, err or "Archive metadata request failed" end

    local json = utils.parse_json(response)
    if not json or json.code ~= 200 or not json.data then
        return nil, json and json.message or "Invalid archive metadata response"
    end
    archive_meta_cache[path] = json.data
    return json.data, nil
end

local function flatten_archive_media(content)
    local result = {}
    local function walk(nodes, parent)
        for _, node in ipairs(nodes or {}) do
            local name = tostring(node.name or "")
            local inner = parent == "" and name or (parent .. "/" .. name)
            if node.is_dir then
                walk(node.children, inner)
            elseif name ~= "" and is_archive_media_file(name) then
                result[#result + 1] = {name = name, inner = inner}
            end
        end
    end
    walk(content, "")
    table.sort(result, function(a, b)
        local an = tonumber(a.name:match("(%d+)"))
        local bn = tonumber(b.name:match("(%d+)"))
        if an and bn and an ~= bn then return an < bn end
        return a.inner:lower() < b.inner:lower()
    end)
    return result
end

local function make_archive_stream_url(meta, inner_path)
    local raw_url = tostring(meta.raw_url or "")
    if raw_url == "" then return nil end
    if raw_url:sub(1, 1) == "/" then raw_url = config.host .. raw_url end
    local separator = raw_url:find("?", 1, true) and "&" or "?"
    return raw_url .. separator
        .. "inner=" .. url_encode_path(inner_path)
        .. "&pass=&sign=" .. url_encode(meta.sign or "")
end

local function archive_virtual_url(archive_path, inner_path, index)
    return "alist-archive://play/" .. url_encode(archive_path)
        .. "/" .. tostring(index) .. "/" .. url_encode(inner_path)
end

local function apply_archive_stream(entry)
    mp.set_property_bool("user-data/alist/archive-inner", true)
    -- OpenList's /ae extraction stream is sequential and commonly ignores
    -- HTTP Range. Preserve all already played packets on disk so seeking back
    -- does not depend on the small default 50 MiB memory back-buffer. Forward
    -- prefetch remains capped at 90 seconds to improve short forward seeks
    -- without downloading the complete archive entry
    -- entry eagerly. file-local-options keeps ordinary AList/network playback
    -- on the user's normal cache policy.
    mp.set_property("file-local-options/cache", "yes")
    mp.set_property("file-local-options/cache-on-disk", "yes")
    mp.set_property("file-local-options/cache-secs", "90")
    mp.set_property("file-local-options/demuxer-max-bytes", "512MiB")
    mp.set_property("file-local-options/demuxer-max-back-bytes", "128MiB")
    mp.set_property("file-local-options/demuxer-seekable-cache", "yes")
    mp.set_property("file-local-options/demuxer-cache-unlink-files", "immediate")
    if archive_cache_dir and archive_cache_dir ~= "" then
        mp.set_property("file-local-options/demuxer-cache-dir", archive_cache_dir)
    end
    mp.set_property("stream-open-filename", entry.stream_url)
    mp.set_property("force-media-title", entry.name)
    set_playlist_title(mp.get_property("path", ""), entry.name)
end

local function apply_archive_fallback(request)
    mp.set_property_bool("user-data/alist/archive-inner", false)
    mp.set_property('file-local-options/http-header-fields', request.auth_header)
    mp.set_property("stream-open-filename", request.dav_url)
    mp.set_property("force-media-title", request.archive_name)
end

local function resolve_archive_request(virtual_path, request)
    show_notification("正在读取压缩包目录…", 2.5)
    local meta, err = request_archive_meta(request.archive_path)
    local media = meta and flatten_archive_media(meta.content) or {}
    if not meta or meta.encrypted or #media == 0 then
        local reason = meta and (meta.encrypted and "archive is encrypted" or "no playable media")
            or tostring(err or "archive preview unsupported")
        msg.warn("AList archive API unavailable; using safe WebDAV compatibility mode: " .. reason)
        show_notification("压缩包预览不可用，已切换兼容模式", 3)
        apply_archive_fallback(request)
        return
    end

    local prepared = {}
    for index, item in ipairs(media) do
        local stream_url = make_archive_stream_url(meta, item.inner)
        if stream_url then
            local marker = index == 1 and virtual_path
                or archive_virtual_url(request.archive_path, item.inner, index)
            local entry = {name = item.name, inner = item.inner, stream_url = stream_url}
            archive_streams[marker] = entry
            raw_url_info[marker] = {
                name = item.name,
                path = request.archive_path .. "|/" .. item.inner,
                directory = request.archive_path:match("^(.*)/[^/]+$") or "/",
                sub_count = 0,
                is_archive_inner = true,
            }
            set_playlist_title(marker, item.name)
            prepared[#prepared + 1] = {path = marker, entry = entry}
        end
    end
    if #prepared == 0 then
        apply_archive_fallback(request)
        return
    end

    apply_archive_stream(prepared[1].entry)
    for index = 2, #prepared do
        loadfile_with_options(prepared[index].path, "append", {
            ["force-media-title"] = prepared[index].entry.name,
        })
    end
    show_notification(string.format("压缩包已就绪：%d 个媒体文件", #prepared), 3)
end

local function apply_direct_dav_auth(path)
    if type(path) ~= "string" or not path:match("^https?://") then return false end
    if not load_config() then return false end

    local host_prefix = config.host .. "/"
    if path:sub(1, #host_prefix):lower() ~= host_prefix:lower() then return false end
    apply_alist_network_cache_options()

    -- History/recent-playback entries intentionally store the clean URL, not
    -- reversible credentials. Restore Basic auth only for this exact configured
    -- AList origin and only as a file-local option for the current load.
    local dav_prefix = config.host .. "/dav/"
    if path:sub(1, #dav_prefix):lower() == dav_prefix:lower() then
        mp.set_property("file-local-options/http-header-fields", authorization_header())
    end
    return true
end

local function on_archive_load()
    local path = mp.get_property("path", "")
    apply_direct_dav_auth(path)

    local stream = archive_streams[path]
    if stream then
        apply_archive_stream(stream)
        return
    end
    local request = archive_requests[path]
    if request then resolve_archive_request(path, request) end
end

local function sort_alist_items(items)
    local function extract_num(s)
        local n = s:match("(%d+)")
        return n and tonumber(n)
    end
    table.sort(items, function(a, b)
        local na, nb = extract_num(a.name), extract_num(b.name)
        if na and nb and na ~= nb then return na < nb end
        return a.name:lower() < b.name:lower()
    end)
end

local function find_raw_url_info(path)
    if not path or path == "" then return nil end
    if raw_url_info[path] then return raw_url_info[path] end
    for stored_url, info in pairs(raw_url_info) do
        if path:find(stored_url, 1, true) or stored_url:find(path, 1, true) then
            return info
        end
    end
end

local function sync_current_playlist(items, directory)
    if mp.get_property_bool("idle-active", false) then return 0 end
    if mp.get_property_native("user-data/alist/directory", "") ~= directory then return 0 end

    local count = mp.get_property_number("playlist-count", 0)
    if count < 1 then return 0 end

    local existing_paths, existing_names = {}, {}
    for i = 0, count - 1 do
        local filename = mp.get_property("playlist/" .. i .. "/filename", "")
        local info = find_raw_url_info(filename)
        if not info or info.directory ~= directory then
            -- Do not modify mixed or manually assembled playlists.
            return 0
        end
        existing_paths[info.path] = true
        existing_names[info.name] = true
    end

    sort_alist_items(items)
    local appended = 0
    for _, item in ipairs(items) do
        if item.type == "file" and not item.is_playable_archive and not existing_paths[item.alist_path]
            and not existing_names[item.name] and append_to_playlist(item) then
            existing_paths[item.alist_path] = true
            existing_names[item.name] = true
            appended = appended + 1
        end
    end
    return appended
end

local function autoload_directory(info, current_url, generation)
    if autoloading_playlist then return end
    if generation ~= title_refresh_generation or not is_current_alist_item(current_url, info) then return end
    if mp.get_property_number("playlist-count", 1) > 1 then return end
    if not info.directory or info.directory == "/" then return end

    local items = build_directory_items(info.directory, false)
    if not items or #items < 2 then return end

    -- A directory request can finish after another client has switched files.
    -- Re-check the live identity before the first playlist mutation.
    if generation ~= title_refresh_generation or not is_current_alist_item(current_url, info) then return end
    if mp.get_property_number("playlist-count", 1) > 1 then return end

    sort_alist_items(items)

    local pos = 0
    local file_count = 0
    local appended = 0

    autoloading_playlist = true
    for _, item in ipairs(items) do
        if generation ~= title_refresh_generation or not is_current_alist_item(current_url, info) then
            break
        end
        if item.type == "file" and not item.is_playable_archive then
            if item.alist_path == info.path or item.path == current_url or (info.name and item.name == info.name) then
                pos = file_count
            else
                if append_to_playlist(item) then appended = appended + 1 end
            end
            file_count = file_count + 1
        end
    end

    if appended > 0 and generation == title_refresh_generation and is_current_alist_item(current_url, info) then
        mp.commandv("playlist-move", 0, pos + 1)
    end
    autoloading_playlist = false
end

local function on_file_loaded()
    local info, generation = update_title()
    mp.set_property_bool("user-data/alist/archive-inner", info and info.is_archive_inner == true or false)
    if info then
        local path = mp.get_property("path", "")
        raw_url_info[path] = info
        if info.is_playable_archive or info.is_archive_inner then return end
        mp.add_timeout(0.1, function()
            if generation ~= title_refresh_generation or not is_current_alist_item(path, info) then return end
            autoload_directory(info, path, generation)
            if generation ~= title_refresh_generation or not is_current_alist_item(path, info) then return end
            local sub_count = raw_url_info[path] and raw_url_info[path].sub_count or info.sub_count or 0
            if sub_count > 0 then
                show_notification("已加载 " .. sub_count .. " 个外挂字幕", 4)
            end
        end)
    end
end

local function request_manual_refresh(directory, selected_name)
    if type(directory) ~= "string" or not directory:match("^AList/") then
        show_notification("请先打开 AList/OpenList 目录", 3)
        return false
    end

    manual_refresh_requests[directory] = {selected_name = selected_name}
    archive_meta_cache = {}
    fb.rescan()
    return true
end

alist_parser.keybinds = {{
    key = "Ctrl+r",
    name = "refresh",
    parser = "alist",
    command = function(_, state)
        local selected = state.list and state.list[state.selected]
        return request_manual_refresh(state.directory, selected and selected.name)
    end,
}}

function alist_parser:setup()
    load_config()
    mp.set_property_bool("user-data/alist/playing", false)
    mp.set_property("user-data/alist/name", "")
    mp.set_property("user-data/alist/path", "")
    mp.set_property("user-data/alist/directory", "")
    mp.set_property_bool("user-data/alist/archive-inner", false)
    self:register_root_item({name = "AList/", label = "AList/OpenList"}, 50)
    mp.register_event("file-loaded", on_file_loaded)
end

function alist_parser:can_parse(directory)
    return type(directory) == 'string' and directory:find('^AList/') == 1
end

function alist_parser:parse(directory)
    if not load_config() then
        return {}, {empty_text = 'Not configured. Click the network drive button to setup AList/OpenList.'}
    end

    local path = directory:match("^AList(/.*)$") or "/"
    path = path:gsub("/$", "")
    if path == "" then path = "/" end

    local manual_refresh = manual_refresh_requests[directory]
    manual_refresh_requests[directory] = nil
    local force_refresh = manual_refresh ~= nil or should_auto_refresh(path)

    local items, err, refresh_applied = build_directory_items(path, force_refresh)
    if not items then
        if directory ~= "AList/" and directory ~= "AList" then
            msg.warn("AList: saved directory is unavailable, falling back to root")
            items, err, refresh_applied = build_directory_items("/", force_refresh)
            if items then
                write_last_dir("AList/")
                return items, {sorted = true, filtered = true, directory = "AList/"}
            end
        end
        return {}, {empty_text = "Failed to read directory: " .. tostring(err)}
    end

    local appended = sync_current_playlist(items, path)
    if appended > 0 then
        show_notification("AList 目录已更新，播放列表新增 " .. appended .. " 个文件", 4)
    elseif manual_refresh then
        local text = refresh_applied and "AList/OpenList 目录已强制刷新" or "AList/OpenList 目录已重新读取"
        show_notification(text, 3)
    end

    write_last_dir(directory)
    local opts = {sorted = true, filtered = true}
    if manual_refresh and manual_refresh.selected_name then
        for index, item in ipairs(items) do
            if item.name == manual_refresh.selected_name then
                opts.selected_index = index
                break
            end
        end
    end
    return items, opts
end

mp.register_script_message("alist-refresh-current", function()
    local ok_open, is_open = pcall(fb.get_open_status)
    local ok_dir, directory = pcall(fb.get_directory)
    if not ok_open or not is_open or not ok_dir then
        show_notification("请先打开 AList/OpenList 目录", 3)
        return
    end
    local selected
    local ok_item, item = pcall(fb.get_selected_item)
    if ok_item then selected = item end
    request_manual_refresh(directory, selected and selected.name)
end)

mp.register_event("shutdown", function()
    title_refresh_generation = title_refresh_generation + 1
    clear_title_refresh_timers()
    local ok, directory = pcall(fb.get_directory)
    if ok and type(directory) == "string" then write_last_dir(directory) end
    if latest_alist_title and forced_title_active then
        mp.set_property("force-media-title", latest_alist_title)
        pcall(mp.set_property, "media-title", latest_alist_title)
    end
end)

mp.observe_property('media-title', 'string', function(_, title)
    if not title then return end
    if forced_title_active and latest_alist_title and title ~= latest_alist_title then
        if title:find('^https?://') or title:find('link3%.cc') or title:find('latest') then
            mp.set_property('force-media-title', latest_alist_title)
            pcall(mp.set_property, 'media-title', latest_alist_title)
        end
    end
end)

-- Signed /d links fall back only after an actual playback error. A timeout is
-- not a failure signal: large remote files can legitimately spend several
-- seconds probing before time-pos becomes available.

-- Look up fallback /d/ URL for a given WebDAV URL, using fuzzy match
-- since mpv may normalize the path property.
local function find_fallback(path)
    if not path then return nil end
    if fallback_urls[path] then return fallback_urls[path] end
    for stored_url, f in pairs(fallback_urls) do
        if path:find(stored_url, 1, true) or stored_url:find(path, 1, true) then
            return f
        end
    end
    return nil
end

local function remove_fallback(path)
    for k, _ in pairs(fallback_urls) do
        if path:find(k, 1, true) or k:find(path, 1, true) then
            fallback_urls[k] = nil
            return
        end
    end
end

local function do_retry(fb)
    msg.info("AList: retrying with safe WebDAV fallback...")
    mp.add_timeout(0.05, function()
        local opts = fb.mpv_options or {}
        if fb.title and not opts["force-media-title"] then opts["force-media-title"] = fb.title end
        loadfile_with_options(fb.url, "replace", opts)
    end)
end

mp.register_event("end-file", function(event)
    if event.reason ~= "error" then return end
    local failed = mp.get_property("path", "")
    local fb = find_fallback(failed)
    if not fb then return end
    remove_fallback(failed)
    do_retry(fb)
end)

if mp.add_hook then mp.add_hook("on_load", 20, on_archive_load) end

return alist_parser
