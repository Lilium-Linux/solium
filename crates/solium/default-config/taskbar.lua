-- The taskbar's half in Lua (~/.config/solium/shell/Shell.qml is the other):
-- minimising into the taskbar, restoring, switching to a window on another
-- workspace, the battery reading, and the layout widget's click.
--
-- Minimising is Solium's own (#30, the shipped minimize.lua): the layouts
-- leave the window out, an application's own minimise button works, and the
-- overview shows it. All this file changes is where the genie goes: into the
-- window's taskbar button instead of the shipped slot in the middle of the
-- bottom edge.
--
-- Required from user.lua, while the configuration is still loading, so the
-- shipped modules are required inside the functions rather than up here.

local taskbar = {}

local kept = sol.keep("taskbar", {
    -- Each window's taskbar button, in global logical pixels, by id.
    rects = {},
})
-- Windows the old homemade minimise floated and parked, before it was
-- Solium's: none survive a restart, but a reload carries the table over.
kept.minimized = nil

-- Hand the bar a property, the way the shipped search does.
local function push(name, value)
    local config = require("config")
    config.shell = type(config.shell) == "table" and config.shell or {}
    config.shell.properties = type(config.shell.properties) == "table" and config.shell.properties or {}
    config.shell.properties[name] = value
    require("shell").apply()
end

local function window(id)
    for _, w in ipairs(sol.windows()) do
        if w.id == id then
            return w
        end
    end
    return nil
end

-- Where the genie goes: the window's button, or the shipped slot when the
-- bar has not said where that is yet.
local function target(id)
    return kept.rects[id] or require("minimize").slot(sol.monitor(id))
end

-- Draw window `id` squeezed into its button. What the shipped `minimize`
-- event plays, once `aim` has pointed it here; also used by the window
-- switcher (recents.lua) to put one back after showing it as a card.
function taskbar.park(id)
    local config = require("config")
    sol.animate(config.minimize.motion)
    sol.present(id, {
        deform = {
            effect = "genie",
            axis = "down",
            spread = config.minimize.spread,
            to = target(id),
        },
    })
end

-- Point the shipped minimize at the buttons: every way a window is
-- minimised (super+m, its own button, a click on its task) and the overview
-- putting one back go through `minimize.into`. Done once the scripts have
-- loaded, at startup and after every reload, as looks.lua does.
local function aim()
    local minimize = package.loaded["minimize"]
    if minimize then
        minimize.into = taskbar.park
    end
end
sol.on("monitors", aim)
sol.on("restore", aim)

-- Show the window's own workspace on its monitor, then give it the keyboard,
-- which shows it again if it was minimised.
local function bring(w)
    local workspaces = require("workspaces")
    workspaces.go(workspaces.at(w.id, w.monitor), w.monitor)
    sol.focus(w.id)
end

function taskbar.is_minimized(id)
    local w = window(id)
    return w ~= nil and w.minimized == true
end

function taskbar.minimize(id)
    aim()
    sol.minimize(id)
end

function taskbar.restore(id)
    local w = window(id)
    if w then
        bring(w)
    end
end

-- A click on a task: restore it, minimise it if it has the keyboard, or
-- bring it forward from wherever it is.
function taskbar.click(id)
    local w = window(id)
    if not w then
        return
    end
    if w.minimized then
        bring(w)
    elseif w.focused then
        taskbar.minimize(id)
    else
        bring(w)
    end
end

-- A click on the now-playing card: the player's window forward. Found by its
-- app id against what the player calls itself (`desktopEntry`, "spotify";
-- its last part, for "org.mozilla.firefox"; or its name, "Spotify"); a
-- player with no window on screen -- closed to its tray -- is asked to show
-- itself (MPRIS Raise).
function taskbar.raise_player(entry, identity, service)
    local names = {}
    for _, name in ipairs({ entry, identity }) do
        if type(name) == "string" and name ~= "" then
            names[name:lower()] = true
            names[(name:match("[^.]+$") or name):lower()] = true
        end
    end
    for _, w in ipairs(sol.windows()) do
        local app = type(w.app_id) == "string" and w.app_id:lower() or ""
        if not w.leaving and app ~= "" and (names[app] or names[app:match("[^.]+$") or app]) then
            bring(w)
            return
        end
    end
    if type(service) == "string" and service:match("^org%.mpris%.MediaPlayer2%.") then
        sol.run("busctl", "--user", "call", service, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2", "Raise")
    end
end

local function read(path)
    local file = io.open(path, "r")
    if not file then
        return nil
    end
    local text = file:read("l")
    file:close()
    return text
end

function taskbar.battery()
    for _, name in ipairs({ "BAT0", "BAT1", "BAT2" }) do
        local base = "/sys/class/power_supply/" .. name .. "/"
        local capacity = tonumber(read(base .. "capacity") or "")
        if capacity then
            require("bar").battery({ present = true, percent = capacity, status = read(base .. "status") or "" })
            return
        end
    end
    require("bar").battery({ present = false, percent = 0, status = "" })
end

sol.on("surface", function(_, action, data)
    data = type(data) == "table" and data or {}
    local id = tonumber(data.id)
    if action == "task.click" and id then
        taskbar.click(id)
    elseif action == "media.raise" then
        taskbar.raise_player(data.entry, data.identity, data.service)
    elseif action == "task.rect" and id then
        for _, monitor in ipairs(sol.monitors()) do
            if monitor.name == data.monitor then
                local whole = monitor.whole or monitor
                kept.rects[id] = {
                    x = whole.x + data.x, y = whole.y + data.y, w = data.w, h = data.h,
                }
            end
        end
    elseif action == "keyboard.next" then
        local keyboard = sol.keyboard()
        if #keyboard.layouts > 1 then
            sol.keyboard({ active = keyboard.active % #keyboard.layouts + 1 })
        end
    elseif action == "battery.poll" then
        taskbar.battery()
    elseif action == "workspace.step" then
        local step = tonumber(data.by)
        if step then
            local workspaces = require("workspaces")
            local count = workspaces.count()
            local index = workspaces.on(data.monitor) + step
            if index >= 1 and index <= count then
                workspaces.go(index, data.monitor)
            end
        end
    end
end)

sol.on("close", function(id)
    kept.rects[id] = nil
end)

return taskbar
