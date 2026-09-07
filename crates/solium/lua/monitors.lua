-- Monitors, for the layouts.
--
-- The compositor hands scripts one global coordinate space and a list of
-- monitors as rectangles in it, and that is genuinely all the mechanism there
-- is: a window is on the second screen because its x lands there. What is left
-- is bookkeeping every layout needs and none of them should write twice —
-- which windows are on which screen, and which screen a key press meant.
--
-- The important consequence: a layout is *per monitor*. `tiling.lua` keeps a
-- tree for each, `scrolling.lua` a strip for each. Laying out every window
-- against `sol.monitor()` would pile both screens' worth of windows onto the
-- one the pointer happens to be on.

local monitors = {}

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
        out[#out + 1] = { monitor = monitor, windows = mine }
    end
    return out
end

-- The monitor the pointer is on: where a new window goes, and what a binding
-- pressed with no window in mind is about.
function monitors.active()
    local all = sol.monitors()
    for _, monitor in ipairs(all) do
        if monitor.focused then
            return monitor
        end
    end
    -- No monitor claims focus only while the pointer is somewhere impossible.
    -- The first one is a working answer and being wrong about which screen is
    -- better than a layout that does nothing.
    return all[1]
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
--
-- One string rather than a table of tables: `state[key(1, "DP-1")]` reads and
-- writes in one step and cannot half-exist.
function monitors.key(workspace, name)
    return tostring(workspace) .. "@" .. tostring(name)
end

-- The monitor named, as a rect, or the active one.
function monitors.named(name)
    for _, monitor in ipairs(sol.monitors()) do
        if monitor.name == name then
            return monitor
        end
    end
    return monitors.active()
end

return monitors
