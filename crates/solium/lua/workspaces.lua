-- Workspaces, as a presentation.
--
-- A workspace switch does not move windows: it moves the *view*. Every window
-- keeps living exactly where its layout put it, and the ones belonging to
-- other workspaces are simply drawn a screen away. `sol.present` is that, and
-- the compositor animates the difference — so the slide is the same machinery
-- that overview and tiling use, and cannot drift out of step with them.
--
-- Three consequences worth having, all of them free:
--
--   * Nothing else needs to know. Tiling arranges the workspace in view and has
--     no idea the others exist, because their windows never move.
--   * Hit-testing follows the transform, so a window drawn off-screen is not
--     under the cursor either. A workspace you cannot see is one you cannot
--     click into by accident.
--   * The arrangement -- a row, a column, a grid -- is only a question of
--     which direction the offset runs. That is why all three are the same
--     code.
--
-- ## Per monitor, or not
--
-- `config.workspaces.per_monitor` decides whether each screen has its own
-- workspace in view. On, `super+2` switches the monitor the pointer is on and
-- leaves the other showing what it was; off, one switch moves every screen.
--
-- Both are real desktops and the difference is what you think a workspace *is*
-- -- a screenful, or a whole desk. So it is a setting rather than a decision
-- made here, and the two share every line below: which workspace a monitor
-- shows is looked up by monitor either way, and with the setting off every
-- monitor looks up the same entry.

local config = require("config")
local monitors = require("monitors")

-- The key every monitor shares when workspaces are not per monitor. A name no
-- connector can have, so it cannot collide with a real one.
local TOGETHER = "*all*"

local workspaces = {
    settings = config.workspaces,
    -- Which workspace each monitor is showing, by monitor name.
    showing = {},
    -- Which workspace each window belongs to, by window id.
    of = {},
}

local function key(monitor)
    if not workspaces.settings.per_monitor then
        return TOGETHER
    end
    if monitor then
        return monitor
    end
    local active = monitors.active()
    return active and active.name or TOGETHER
end

-- Where a workspace sits in the arrangement, as a column and a row.
function workspaces.cell(index)
    local settings = workspaces.settings
    if settings.arrangement == "vertical" then
        return 1, index
    elseif settings.arrangement == "grid" then
        local columns = math.max(1, settings.columns)
        return ((index - 1) % columns) + 1, math.floor((index - 1) / columns) + 1
    end
    return index, 1
end

function workspaces.count()
    local settings = workspaces.settings
    if settings.arrangement == "grid" then
        return math.max(1, settings.columns) * math.max(1, settings.rows)
    elseif settings.arrangement == "vertical" then
        return math.max(1, settings.rows)
    end
    return math.max(1, settings.columns)
end

-- The workspace a monitor is showing. Never nil: a screen nobody has switched
-- yet is showing the first one.
function workspaces.on(monitor)
    return workspaces.showing[key(monitor)] or 1
end

-- The workspace the user is looking at, which is the active monitor's.
function workspaces.current()
    return workspaces.on(nil)
end

-- Which workspace a window belongs to.
--
-- Unknown windows are on whatever *their own monitor* is showing, so a window
-- that appears while the compositor is not looking is visible where it opened
-- rather than stranded on a workspace nobody is on.
function workspaces.at(id, monitor)
    return workspaces.of[id] or workspaces.on(monitor or monitors.of(id))
end

function workspaces.on_active(id, monitor)
    monitor = monitor or monitors.of(id)
    return workspaces.at(id, monitor) == workspaces.on(monitor)
end

-- Windows on the workspace their own monitor is showing, in the order given.
function workspaces.visible(windows)
    local out = {}
    for _, window in ipairs(windows or sol.windows()) do
        if workspaces.on_active(window.id, window.monitor) then
            out[#out + 1] = window
        end
    end
    return out
end

-- Draw every window where its workspace is, relative to the one its monitor
-- has in view.
function workspaces.apply(animation)
    local spread = workspaces.settings.spread or 1.0

    sol.animate(animation or workspaces.settings.motion)
    for _, window in ipairs(sol.windows()) do
        -- Its *own* monitor's size and its own monitor's workspace. A window on
        -- a 1920 screen slid by a 2560's width lands somewhere nothing can
        -- reach and comes back to where it started only by luck.
        local area = sol.monitor(window.id)
        local showing_col, showing_row = workspaces.cell(workspaces.on(window.monitor))
        local col, row = workspaces.cell(workspaces.at(window.id, window.monitor))
        local dx = (col - showing_col) * area.w * spread
        local dy = (row - showing_row) * area.h * spread
        if dx == 0 and dy == 0 then
            -- Back to where it really lives, which is where it has been all
            -- along. Clearing rather than presenting at its own rect matters:
            -- a window with no transform is one the layout can move freely.
            sol.present_clear(window.id)
        else
            sol.present(window.id, {
                x = window.x + dx,
                y = window.y + dy,
                w = window.w,
                h = window.h,
            })
        end
    end
end

-- Switch the monitor in front of you, or every monitor when workspaces are
-- not per monitor.
function workspaces.go(index)
    index = math.max(1, math.min(index, workspaces.count()))
    local monitor = key(nil)
    if index == workspaces.on(nil) then
        return
    end
    workspaces.showing[monitor] = index
    workspaces.apply()
    workspaces.announce()

    -- Focus follows the view. Without this the keyboard still belongs to a
    -- window nobody can see, and the next keystroke goes somewhere off-screen.
    --
    -- Only among the windows on the screen that just changed: switching the
    -- left monitor must not take focus off the right one's window if there is
    -- nothing on the left to take it.
    local switched = workspaces.settings.per_monitor and monitor or nil
    for _, window in ipairs(sol.windows()) do
        if (not switched or window.monitor == switched)
            and workspaces.on_active(window.id, window.monitor)
        then
            sol.focus(window.id)
            return
        end
    end
end

-- Step through the arrangement. Directions that the arrangement has no room
-- for do nothing, so the same bindings work for a row, a column and a grid.
function workspaces.step(dx, dy)
    local settings = workspaces.settings
    local col, row = workspaces.cell(workspaces.on(nil))
    if settings.arrangement == "horizontal" then
        return workspaces.go(workspaces.on(nil) + dx)
    elseif settings.arrangement == "vertical" then
        return workspaces.go(workspaces.on(nil) + dy)
    end
    local columns = math.max(1, settings.columns)
    local rows = math.max(1, settings.rows)
    col = math.max(1, math.min(col + dx, columns))
    row = math.max(1, math.min(row + dy, rows))
    workspaces.go((row - 1) * columns + col)
end

-- Send the focused window to another workspace. It stays on its own monitor:
-- a workspace is a screenful, and sending a window sideways through the
-- workspaces should not also throw it at the other screen.
function workspaces.send(index)
    index = math.max(1, math.min(index, workspaces.count()))
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            workspaces.of[window.id] = index
            workspaces.apply()
            workspaces.announce()
            return
        end
    end
end

function workspaces.announce()
    local settings = workspaces.settings
    local index = workspaces.on(nil)
    -- Which monitor, but only when saying so means anything: with one screen,
    -- or with workspaces switching together, naming it is noise.
    local where = ""
    if settings.per_monitor and #sol.monitors() > 1 then
        local active = monitors.active()
        where = active and (" on " .. active.name) or ""
    end
    if settings.arrangement == "grid" then
        local col, row = workspaces.cell(index)
        sol.status(string.format("workspace %d,%d%s", col, row, where))
    else
        sol.status(string.format("workspace %d%s", index, where))
    end
end

-- A new window belongs to the workspace its own monitor is showing.
sol.on("open", function(id)
    if workspaces.settings.follow_new_windows then
        workspaces.of[id] = workspaces.on(monitors.of(id))
    end
end)

for index = 1, 9 do
    sol.bind("super+" .. index, function()
        workspaces.go(index)
    end)
    sol.bind("super+shift+" .. index, function()
        workspaces.send(index)
    end)
end

sol.bind("super+ctrl+left", function() workspaces.step(-1, 0) end)
sol.bind("super+ctrl+right", function() workspaces.step(1, 0) end)
sol.bind("super+ctrl+up", function() workspaces.step(0, -1) end)
sol.bind("super+ctrl+down", function() workspaces.step(0, 1) end)

return workspaces
