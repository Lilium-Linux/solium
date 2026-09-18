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

-- The monitor a rectangle is on, judged by its centre, or nil.
--
-- The same question the compositor answers, answered the same way: a window
-- belongs to the screen its centre lands on (`Solium::output_of`). The two have
-- to agree, because this decides where a layout *puts* something and the
-- compositor is what reports which monitor it ended up on — disagree and the
-- next pass reads back a monitor the layout did not intend and moves it again.
-- By the centre rather than by any overlap, so a window straddling the boundary
-- is on one screen rather than on two.
--
-- `whole` rather than the work area: the question is which screen, and a point
-- under a bar is still on the screen the bar is on.
--
-- Nil rather than a fallback, unlike `named`. The caller has a better second
-- answer than "the active monitor" — the window's own `monitor`, which the
-- compositor already decided — and a helper that never says no is a helper that
-- hides the L-shaped arrangement with a hole in the middle of it.
function monitors.covering(rect)
    if not rect then
        return nil
    end
    local x = rect.x + rect.w / 2
    local y = rect.y + rect.h / 2
    for _, monitor in ipairs(sol.monitors()) do
        -- The monitor table *is* its work area, with `whole` alongside; see
        -- `sol.monitors`.
        local whole = monitor.whole or monitor
        if x >= whole.x and x < whole.x + whole.w
            and y >= whole.y and y < whole.y + whole.h
        then
            return monitor
        end
    end
    return nil
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
