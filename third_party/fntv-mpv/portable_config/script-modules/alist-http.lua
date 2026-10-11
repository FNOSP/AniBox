local mp = require "mp"
local msg = require "mp.msg"
local utils = require "mp.utils"

local M = {
    available = false,
    backend = nil,
    reason = nil,
}

local TIMEOUT = 8

local function add_lua_paths()
    local lua_path = mp.command_native({"expand-path", "~~/../lua/?.lua"}) or ""
    local lua_init_path = mp.command_native({"expand-path", "~~/../lua/?/init.lua"}) or ""
    local c_path = mp.command_native({"expand-path", "~~/../?.dll"}) or ""
    local c_core_path = mp.command_native({"expand-path", "~~/../?/core.dll"}) or ""
    package.path = lua_path .. ";" .. lua_init_path .. ";" .. package.path
    package.cpath = c_path .. ";" .. c_core_path .. ";" .. package.cpath
end

local function create_socket_backend()
    add_lua_paths()

    local http_ok, http = pcall(require, "socket.http")
    local ltn12_ok, ltn12 = pcall(require, "ltn12")
    if not http_ok or not ltn12_ok then
        return nil, tostring(not http_ok and http or ltn12)
    end

    http.TIMEOUT = TIMEOUT

    return function(url, method, headers, body)
        local response_body = {}
        local request = {
            url = url,
            method = method or "GET",
            headers = headers or {},
            sink = ltn12.sink.table(response_body),
        }
        if body then
            request.source = ltn12.source.string(body)
            request.headers["Content-Length"] = #body
        end

        local ok, status = http.request(request)
        if not ok then return nil, status or "HTTP request failed" end
        if status ~= 200 then return nil, "HTTP status " .. tostring(status) end
        return table.concat(response_body), nil
    end
end

local function create_curl_backend()
    local system_root = os.getenv("SystemRoot") or "C:\\Windows"
    local curl_path = system_root .. "\\System32\\curl.exe"
    if not utils.file_info(curl_path) then
        return nil, curl_path .. " not found"
    end

    return function(url, method, headers, body)
        local marker = "__MPV_ALIST_HTTP_STATUS__:"
        local body_path = nil
        local args = {
            curl_path,
            "--silent",
            "--show-error",
            "--location",
            "--connect-timeout", tostring(TIMEOUT),
            "--max-time", tostring(TIMEOUT + 2),
            "--request", method or "GET",
            "--output", "-",
            "--write-out", "\\n" .. marker .. "%{http_code}",
        }

        for name, value in pairs(headers or {}) do
            args[#args + 1] = "--header"
            args[#args + 1] = tostring(name) .. ": " .. tostring(value)
        end

        if body then
            body_path = os.tmpname()
            local body_file, file_error = io.open(body_path, "wb")
            if not body_file then
                return nil, "could not create temporary request body: " .. tostring(file_error)
            end
            body_file:write(body)
            body_file:close()
            args[#args + 1] = "--data-binary"
            args[#args + 1] = "@" .. body_path
        end
        args[#args + 1] = url

        local command = {
            name = "subprocess",
            playback_only = false,
            capture_stdout = true,
            capture_stderr = true,
            args = args,
        }

        local command_ok, result = pcall(mp.command_native, command)
        if body_path then os.remove(body_path) end
        if not command_ok then return nil, tostring(result) end
        if not result or result.status ~= 0 then
            local reason = result and (result.stderr or result.error_string) or "curl subprocess failed"
            reason = tostring(reason or ""):gsub("%s+$", "")
            return nil, reason ~= "" and reason or "curl subprocess failed"
        end

        local output = result.stdout or ""
        local status = output:match("\n" .. marker .. "(%d%d%d)%s*$")
        if not status then return nil, "curl response did not include an HTTP status" end
        local response = output:gsub("\n" .. marker .. "%d%d%d%s*$", "")
        if tonumber(status) ~= 200 then return nil, "HTTP status " .. status end
        return response, nil
    end
end

local request, socket_error = create_socket_backend()
if request then
    M.available = true
    M.backend = "luasocket"
    M.request = request
else
    local curl_error
    request, curl_error = create_curl_backend()
    if request then
        M.available = true
        M.backend = "curl"
        M.request = request
        msg.verbose("AList HTTP backend: curl subprocess (LuaSocket unavailable)")
    else
        M.reason = "LuaSocket: " .. tostring(socket_error) .. "; curl: " .. tostring(curl_error)
    end
end

return M
