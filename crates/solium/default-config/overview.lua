-- Overview of every window, on every workspace -- in place of the shipped
-- ~/.local/share/solium/lua/overview.lua (this directory is searched first),
-- which shows only the workspace in view. Same bindings and the same API
-- (`enter`, `leave`, `toggle`, `active`), so search's "Overview" still works.
--
-- The other workspaces' windows are drawn a screen away by their desk's group
-- (workspaces.lua). Entering pulls every desk back to zero so all windows can
-- be laid out on their monitor; leaving lets workspaces.lua put the desks back
-- where they were. Picking a window on another workspace switches to it.
-- Windows minimised into the taskbar (taskbar.lua) stay where they are.

local monitors = require("monitors")

local kept = sol.keep("overview", { active = false })

local overview = { active = kept.active }

local PADDING = 24
local ENTER = { duration = 260, easing = "outCubic" }
local LEAVE = { duration = 200, easing = "outCubic" }

local function showing(active)
    overview.active = active
    kept.active = active
    if active then
        sol.bind("escape", overview.leave)
    else
        sol.unbind("escape")
    end
end

local function minimized(id)
    local taskbar = package.loaded["taskbar"]
    return taskbar and taskbar.is_minimized and taskbar.is_minimized(id)
end

local function shown()
    local out = {}
    for _, w in ipairs(sol.windows()) do
        if not w.leaving and not minimized(w.id) then
            out[#out + 1] = w
        end
    end
    return out
end

-- Every desk at zero, so a window on any workspace is where its layout put it.
-- Public for the window switcher (recents.lua), which shows them all too.
function overview.gather(only)
    local workspaces = package.loaded["workspaces"]
    if not workspaces then
        return
    end
    for _, monitor in ipairs(sol.monitors()) do
        for index = 1, (not only or monitor.name == only) and workspaces.count() or 0 do
            local key = "desk-" .. index .. "@" .. monitor.name
            local was = workspaces.carried[key]
            if was and (was[1] ~= 0 or was[2] ~= 0) then
                sol.present_group_clear(key, ENTER)
                workspaces.carried[key] = { 0, 0 }
            end
        end
    end
end

-- And every desk back where workspaces.lua keeps it.
function overview.scatter(animation)
    local workspaces = package.loaded["workspaces"]
    if workspaces then
        workspaces.apply(animation)
    end
end

function overview.enter()
    if overview.active then
        return
    end
    local windows = shown()
    if #windows == 0 then
        sol.log("overview: no windows")
        return
    end

    sol.animate(ENTER)
    overview.gather()
    for _, each in ipairs(monitors.each(windows)) do
        if #each.windows > 0 then
            local area = each.monitor
            local slots = sol.layout.grid(each.windows, {
                x = area.x,
                y = area.y,
                w = area.w,
                h = area.h,
                padding = PADDING,
            })
            for index, window in ipairs(each.windows) do
                sol.present(window.id, slots[index])
            end
        end
    end

    sol.grab_input(true)
    sol.status("overview")
    showing(true)
end

function overview.leave()
    if not overview.active then
        return
    end

    sol.animate(LEAVE)
    for _, window in ipairs(sol.windows()) do
        if not minimized(window.id) then
            sol.present_clear(window.id)
        end
    end
    overview.scatter(LEAVE)

    sol.grab_input(false)
    sol.status("")
    showing(false)
end

function overview.toggle()
    if overview.active then
        overview.leave()
    else
        overview.enter()
    end
end

sol.bind("super+space", overview.toggle)

if overview.active then
    showing(true)
end


-- A click the shell's picker layer took, at scene coordinates on `monitor`,
-- as a point in the global space `sol.window_at` reads.
local function global(data)
    for _, monitor in ipairs(sol.monitors()) do
        if monitor.name == data.monitor then
            local whole = monitor.whole or monitor
            return whole.x + (tonumber(data.x) or 0), whole.y + (tonumber(data.y) or 0)
        end
    end
    return tonumber(data.x) or 0, tonumber(data.y) or 0
end

local function clicked(x, y)
    local id = sol.window_at(x, y)
    overview.leave()
    if not id then
        return
    end
    local workspaces = package.loaded["workspaces"]
    for _, w in ipairs(sol.windows()) do
        if w.id == id and workspaces then
            workspaces.go(workspaces.at(id, w.monitor), w.monitor)
        end
    end
    sol.focus(id)
end

sol.on("click", function(x, y)
    if overview.active then
        clicked(x, y)
    end
end)

-- The shell's picker layer (Shell.qml) holds the pointer while this is up,
-- so hovering does not reach the windows; its clicks come here.
sol.on("surface", function(_, action, data)
    if overview.active and action == "picker.click" then
        clicked(global(type(data) == "table" and data or {}))
    end
end)

return overview
