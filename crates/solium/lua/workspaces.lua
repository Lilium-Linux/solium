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
--   * Nothing else needs to know. Tiling arranges the active workspace and has
--     no idea the others exist, because their windows never move.
--   * Hit-testing follows the transform, so a window drawn off-screen is not
--     under the cursor either. A workspace you cannot see is one you cannot
--     click into by accident.
--   * The arrangement -- a row, a column, a grid -- is only a question of
--     which direction the offset runs. That is why all three are the same
--     code.

local config = require("config")

local workspaces = {
    settings = config.workspaces,
    active = 1,
    of = {},
}

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

-- Which workspace a window belongs to. Unknown windows are on this one, so a
-- window that appears while the compositor is not looking is never stranded
-- somewhere nobody can reach.
function workspaces.at(id)
    return workspaces.of[id] or workspaces.active
end

function workspaces.on_active(id)
    return workspaces.at(id) == workspaces.active
end

-- Windows on the workspace in view, in the order the caller was given them.
function workspaces.visible(windows)
    local out = {}
    for _, window in ipairs(windows or sol.windows()) do
        if workspaces.on_active(window.id) then
            out[#out + 1] = window
        end
    end
    return out
end

-- Draw every window where its workspace is, relative to the one in view.
function workspaces.apply(animation)
    local area = sol.monitor()
    local spread = workspaces.settings.spread or 1.0
    local active_col, active_row = workspaces.cell(workspaces.active)

    sol.animate(animation or workspaces.settings.motion)
    for _, window in ipairs(sol.windows()) do
        local col, row = workspaces.cell(workspaces.at(window.id))
        local dx = (col - active_col) * area.w * spread
        local dy = (row - active_row) * area.h * spread
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

function workspaces.go(index)
    index = math.max(1, math.min(index, workspaces.count()))
    if index == workspaces.active then
        return
    end
    workspaces.active = index
    workspaces.apply()
    workspaces.announce()

    -- Focus follows the view. Without this the keyboard still belongs to a
    -- window nobody can see, and the next keystroke goes somewhere off-screen.
    for _, window in ipairs(sol.windows()) do
        if workspaces.on_active(window.id) then
            sol.focus(window.id)
            break
        end
    end
end

-- Step through the arrangement. Directions that the arrangement has no room
-- for do nothing, so the same bindings work for a row, a column and a grid.
function workspaces.step(dx, dy)
    local settings = workspaces.settings
    local col, row = workspaces.cell(workspaces.active)
    if settings.arrangement == "horizontal" then
        return workspaces.go(workspaces.active + dx)
    elseif settings.arrangement == "vertical" then
        return workspaces.go(workspaces.active + dy)
    end
    local columns = math.max(1, settings.columns)
    local rows = math.max(1, settings.rows)
    col = math.max(1, math.min(col + dx, columns))
    row = math.max(1, math.min(row + dy, rows))
    workspaces.go((row - 1) * columns + col)
end

-- Send the focused window to another workspace and follow it there or not.
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
    if settings.arrangement == "grid" then
        local col, row = workspaces.cell(workspaces.active)
        sol.status(string.format("workspace %d,%d", col, row))
    else
        sol.status(string.format("workspace %d", workspaces.active))
    end
end

-- A new window belongs to the workspace it appeared on.
sol.on("open", function(id)
    if workspaces.settings.follow_new_windows then
        workspaces.of[id] = workspaces.active
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
