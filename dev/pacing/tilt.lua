-- Phase 0's capture check (0.4): two windows held at 6 degrees from the
-- moment they open, one idle and one ticking ten times a second, with no
-- frame style. Before captures are kept, both are drawn again on every pass;
-- after, the idle one never is and the ticking one once a tick.
-- dev/pacing-tty.sh and dev/pacing-nested.sh load it for their `tilt` runs.
--
-- The tilt is made here, in the open handler, because the hardware backend
-- reads none of the scripted knobs. PACING_MONITOR names the monitor, as in
-- s1.lua.
sol.pane("none")

local WANTED = os.getenv("PACING_MONITOR")

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

local opened = 0
sol.on("open", function(id)
    opened = opened + 1
    local m = monitor()
    if m == nil then
        return
    end
    sol.animate({ duration = 0 })
    sol.place(id, { x = m.x + 40 + (opened - 1) * (m.w // 2), y = m.y + 80, w = m.w // 2 - 80, h = m.h // 2 })
    sol.present(id, { rotate_z = 6 })
    sol.log(string.format("pacing-tilt window %d on %s, held at 6 degrees", opened, m.name))
end)
