-- Presentation state only; never changes mpv's measured playback properties.
local M = {}
function M.new() return {fps=nil, candidate=nil, since=0, text_values={}} end

function M.fps(state, value, now, nominal)
    value = tonumber(value)
    if not value or value <= 0 or value ~= value then return state.fps end
    nominal=tonumber(nominal)
    -- The first decoded frames can yield an incomplete estimate. Start from
    -- the declared source rate; sustained later changes still use the filter.
    if not state.fps and nominal and nominal>0 then
        value=nominal
    end
    -- Keep broadcast rates distinct (59.94 vs 60), while ignoring estimator
    -- noise. A sustained rate change is accepted after one second.
    local rates = {23.976,24,25,29.97,30,47.952,48,50,59.94,60,90,100,119.88,120,144,165,240}
    for _, rate in ipairs(rates) do
        if math.abs(value-rate) < .02 then value=rate; break end
    end
    if not state.fps then state.fps=value end
    local tolerance=math.max(.5,state.fps*.015)
    if math.abs(value-state.fps) < tolerance then
        state.candidate=nil
    elseif not state.candidate or math.abs(value-state.candidate) >= tolerance then
        state.candidate,state.since=value,now
    elseif now-state.since >= 1 then
        state.fps,state.candidate=value,nil
    end
    return state.fps
end

-- Use useful precision instead of reserving blank space for arbitrary digits.
function M.fps_label(value)
    if not value then return '' end
    local precision=math.abs(value*1001/1000-math.floor(value+.5))<.02 and 3 or 2
    return string.format('%.'..precision..'f',value):gsub('0+$',''):gsub('%.$','')..'FPS'
end

-- Sample display text at most once a second. Real text determines capsule
-- width; startup, source switching and scale changes need no width reserves.
function M.text(state, key, value, now)
    local old=state.text_values[key]
    if not old or now-old.time>=1 then
        old={value=value,time=now};state.text_values[key]=old
    end
    return old.value
end
return M
