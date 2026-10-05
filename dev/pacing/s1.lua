-- FX-S1: the scene Phase 0's frame-pacing numbers are measured on. The
-- rounded pane style, four idle windows and one player, at fixed rectangles
-- on one monitor, over the shipped wallpaper. dev/pacing-tty.sh and
-- dev/pacing-nested.sh load it with SOLIUM_LUA_INIT.
--
-- The rectangles are fractions of the monitor's work area, so a capture's
-- cost is the same on every run on that monitor, and nothing else places a
-- window: there is no layout script here. PACING_MONITOR names the monitor;
-- the primary one, then the first, is the fallback, and the log names the
-- one used.
--
-- PACING_TILT=<degrees> is S1 tilted: the player, the fifth window, is held
-- at that angle from its first frame, so its warp is captured again whenever
-- it commits, before captures are kept and after. The tilt is made here, in
-- the open handler, because the hardware backend reads none of the scripted
-- knobs (SOLIUM_TRIGGER_AT included).
require("wallpaper")
sol.pane("rounded")

local WANTED = os.getenv("PACING_MONITOR")
local TILT = tonumber(os.getenv("PACING_TILT") or "")

local function monitor()
    local rows = sol.monitors()
    for _, row in ipairs(rows) do
        if row.name == WANTED then
            return row
        end
    end
    for _, row in ipairs(rows) do
        if row.primary then
            return row
        end
    end
    return rows[1]
end

-- Windows 1 to 4 on a two-by-two grid over the left 60%; the fifth, the
-- player, 16:9 in the right 40%.
local function rect(index, m)
    if index <= 4 then
        local w, h = math.floor(m.w * 0.29), math.floor(m.h * 0.44)
        local column, row = (index - 1) % 2, (index - 1) // 2
        return { x = m.x + 20 + column * (w + 20), y = m.y + 20 + row * (h + 20), w = w, h = h }
    end
    local w = math.floor(m.w * 0.36)
    return { x = m.x + math.floor(m.w * 0.62), y = m.y + math.floor(m.h * 0.2), w = w, h = math.floor(w * 9 / 16) }
end

local opened = 0
sol.on("open", function(id)
    opened = opened + 1
    local m = monitor()
    if m == nil then
        return
    end
    sol.animate({ duration = 0 })
    sol.place(id, rect(opened, m))
    local held = ""
    if TILT ~= nil and opened == 5 then
        sol.present(id, { rotate_z = TILT })
        held = string.format(", held at %g degrees", TILT)
    end
    sol.log(string.format("pacing-s1 window %d on %s%s", opened, m.name, held))
end)
