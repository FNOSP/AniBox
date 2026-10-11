-- On-demand Windows telemetry without launching a shell for each sample.
-- Optional: callers retain their existing providers when LuaJIT/APIs are absent.
local ffi = require 'ffi'
assert(ffi.os == 'Windows', 'Windows telemetry is unavailable')
ffi.cdef[[
typedef struct { uint32_t low, high; } stats_filetime;
int __stdcall GetSystemTimes(stats_filetime *, stats_filetime *, stats_filetime *);
uint32_t __stdcall GetActiveProcessorCount(uint16_t);
long __stdcall PdhOpenQueryA(const char *, uintptr_t, void **);
long __stdcall PdhAddEnglishCounterA(void *, const char *, uintptr_t, void **);
long __stdcall PdhCollectQueryData(void *);
long __stdcall PdhCloseQuery(void *);
typedef struct {
    uint32_t status;
    union { long long large; double value; const char *text; };
} stats_pdh_value;
typedef struct { const char *name; stats_pdh_value value; } stats_pdh_item;
long __stdcall PdhGetFormattedCounterArrayA(void *, uint32_t, uint32_t *, uint32_t *, stats_pdh_item *);
int nvmlInit_v2(void);
int nvmlShutdown(void);
int nvmlDeviceGetHandleByIndex_v2(unsigned int, void **);
typedef struct { unsigned int gpu, memory; } stats_nvml_util;
int nvmlDeviceGetUtilizationRates(void *, stats_nvml_util *);
]]
local kernel = ffi.load('kernel32')
local M = {}
local previous, query, counter, gpu_warm, pdh, gpu_failed
local nvml, nv_device, nv_checked
local times = ffi.new('stats_filetime[3]')
local function ticks(t) return tonumber(t.high) * 4294967296 + tonumber(t.low) end

function M.cpu()
    -- GetSystemTimes covers only the calling processor group above 64 CPUs.
    if kernel.GetActiveProcessorCount(0xffff) > 64 then return nil, 'unsupported' end
    if kernel.GetSystemTimes(times, times+1, times+2) == 0 then return nil, 'unavailable' end
    local idle, total = ticks(times[0]), ticks(times[1]) + ticks(times[2])
    local old = previous
    previous = {idle=idle, total=total}
    if not old or total <= old.total then return nil, 'warming' end
    return math.max(0, math.min(100, 100 * (1 - (idle-old.idle)/(total-old.total))))
end

local function close_query()
    if query then pdh.PdhCloseQuery(query); query, counter = nil, nil end
    gpu_warm = false
end

function M.gpu_engines()
    if gpu_failed then return nil, 'unavailable' end
    if not query then
        local ok, lib = pcall(ffi.load, 'pdh')
        if not ok then gpu_failed=true; return nil, 'unavailable' end
        pdh = lib
        local q, c = ffi.new('void *[1]'), ffi.new('void *[1]')
        if pdh.PdhOpenQueryA(nil, 0, q) ~= 0 then gpu_failed=true; return nil, 'unavailable' end
        query = q[0]
        if pdh.PdhAddEnglishCounterA(query, '\\GPU Engine(*)\\Utilization Percentage', 0, c) ~= 0 then
            close_query(); gpu_failed=true; return nil, 'unavailable'
        end
        counter = c[0]
    end
    if pdh.PdhCollectQueryData(query) ~= 0 then
        close_query(); return nil, 'unavailable'
    end
    if not gpu_warm then gpu_warm=true; return nil, 'warming' end
    local bytes, count = ffi.new('uint32_t[1]'), ffi.new('uint32_t[1]')
    local status = pdh.PdhGetFormattedCounterArrayA(counter, 0x200, bytes, count, nil)
    -- PDH_MORE_DATA, then the exact buffer required by this collected snapshot.
    if status ~= -2147481646 or bytes[0] == 0 or bytes[0] > 16*1024*1024 then
        return nil, 'unavailable'
    end
    local buffer = ffi.new('uint8_t[?]', bytes[0])
    local items = ffi.cast('stats_pdh_item *', buffer)
    if pdh.PdhGetFormattedCounterArrayA(counter, 0x200, bytes, count, items) ~= 0 then
        return nil, 'unavailable'
    end
    local engines = {}
    for i=0,tonumber(count[0])-1 do
        local item = items[i]
        local value = tonumber(item.value.value)
        if item.name ~= nil and item.value.status <= 1 and value == value and value >= 0 and value <= 100 then
            engines[ffi.string(item.name)] = value
        end
    end
    if not next(engines) then return nil, 'unavailable' end
    return engines
end

function M.nvidia()
    if not nv_checked then
        nv_checked = true
        local ok, lib = pcall(ffi.load, 'nvml')
        if ok then
            local initialized, status = pcall(function() return lib.nvmlInit_v2() end)
            if initialized and status == 0 then
                nvml = lib
                local device = ffi.new('void *[1]')
                if nvml.nvmlDeviceGetHandleByIndex_v2(0, device) == 0 then nv_device=device[0] end
            end
        end
    end
    if not nv_device then return nil end
    local util = ffi.new('stats_nvml_util[1]')
    if nvml.nvmlDeviceGetUtilizationRates(nv_device, util) == 0 and util[0].gpu <= 100 then
        return tonumber(util[0].gpu)
    end
end

function M.reset()
    -- Resume from two fresh samples, never average over time the panel was shut.
    previous, gpu_warm = nil, false
end

function M.close()
    close_query()
    if nvml then nvml.nvmlShutdown(); nvml, nv_device = nil, nil end
    previous, nv_checked, gpu_failed = nil, nil, nil
end
return M
