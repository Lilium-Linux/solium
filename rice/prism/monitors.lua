-- Monitors, inset by the shell.
--
-- This replaces the shipped `monitors.lua` wholesale, which is the sanctioned
-- move: the user's directory is searched first, so a module dropped here takes
-- over and everything that requires it gets this one instead.
--
-- The reason to replace this particular module rather than each layout is that
-- *every* mode reads its area through it — tiling, scrolling, overview and the
-- deck all call `each`, `named` or `active`. Insetting here means the bar and
-- the dock are respected by all of them, including modes written after this
-- file, and it is about ten lines. Insetting in the layouts instead would be
-- the same ten lines four times, and the fifth layout would get it wrong.
--
-- A scripted surface reserves nothing from the work area by itself: that is
-- wlr-layer-shell's job and a surface is not a layer-shell client. So this is
-- the reservation.

local prism = require("prism")

local monitors = {}

-- A monitor as the layouts should see it: the same table, with the shell's
-- bands taken off. `name` and everything else is carried through untouched,
-- because callers match windows against `monitor.name` and a copy that lost it
-- would put every window on no screen at all.
local function inset(monitor)
    if not monitor then
        return nil
    end
    local margins = prism.inset
    local copy = {}
    for key, value in pairs(monitor) do
        copy[key] = value
    end
    copy.x = monitor.x + margins.left
    copy.y = monitor.y + margins.top
    copy.w = math.max(1, monitor.w - margins.left - margins.right)
    copy.h = math.max(1, monitor.h - margins.top - margins.bottom)
    return copy
end

-- Every monitor, with the windows on it, in the order they were given.
--
-- The list is always complete: a monitor with nothing on it is still in it,
-- with an empty list. A layout has to hear about an empty screen — that is
-- what tells it the last window left.
function monitors.each(windows)
    windows = windows or sol.windows()
    local out = {}
    for _, monitor in ipairs(sol.monitors()) do
        local mine = {}
        for _, window in ipairs(windows) do
            if window.monitor == monitor.name then
                mine[#mine + 1] = window
            end
        end
        out[#out + 1] = { monitor = inset(monitor), windows = mine }
    end
    return out
end

-- The monitor the pointer is on: where a new window goes, and what a binding
-- pressed with no window in mind is about.
function monitors.active()
    local all = sol.monitors()
    for _, monitor in ipairs(all) do
        if monitor.focused then
            return inset(monitor)
        end
    end
    return inset(all[1])
end

-- Which monitor a window is on, by name. Never nil for a window that exists.
function monitors.of(id)
    for _, window in ipairs(sol.windows()) do
        if window.id == id then
            return window.monitor
        end
    end
    local active = monitors.active()
    return active and active.name
end

-- A key for per-monitor state, which is always also per workspace.
function monitors.key(workspace, name)
    return tostring(workspace) .. "@" .. tostring(name)
end

-- The monitor named, as a rect, or the active one.
function monitors.named(name)
    for _, monitor in ipairs(sol.monitors()) do
        if monitor.name == name then
            return inset(monitor)
        end
    end
    return monitors.active()
end

-- The whole screen, uninset. The shell places itself against this — a bar that
-- reserved space from itself would creep down the screen on every reload.
function monitors.whole(name)
    for _, monitor in ipairs(sol.monitors()) do
        if not name or monitor.name == name then
            return monitor
        end
    end
    return sol.monitors()[1]
end

return monitors
