-- The bar's Lua half (shell/Bar.qml is the scene). The bar is a surface of
-- its own on every monitor, the 40 pixel strip along the bottom, rather than
-- part of the shell's full-screen scene: an effect rule's slot is cut to its
-- part, so a blur behind the bar needs a part the bar's size (looks.lua
-- puts one there). The bar reserves its strip itself.
--
-- The pop-ups its buttons open (volume, notifications, a tray menu) stay in
-- the shell's scene, over the whole monitor, so a click outside one closes
-- it. A button says which one to open here, this tells the shell, and the
-- shell says when it closed, so the button stops looking pressed.
--
-- Required from user.lua while the configuration is still loading, so the
-- shipped modules are required inside the functions.

local bar = {}

local HEIGHT = 40
local SCENE = (os.getenv("XDG_CONFIG_HOME") or (os.getenv("HOME") .. "/.config"))
    .. "/solium/shell/Bar.qml"

local kept = sol.keep("bar", {
    -- Which pop-up is open, and where: { what, monitor, x, id }, or empty.
    popup = {},
    battery = { present = false, percent = 0, status = "" },
    shown = {},
    -- Whether the clock is put away while the lock's is out of it.
    clock_away = false,
})

-- The clock, put away while the lock's is out of it. The lock this desktop
-- shows, qml/lock/Violet.qml, takes the bar's clock with it: its own starts
-- on the bar's, at its size and place, flies to the middle as the session
-- locks and back down into the bar's as it unlocks. So for that while there
-- is one clock, the lock's: the bar's goes the instant the lock starts, while
-- the two still lie one on the other, and fades back over the last LANDING
-- milliseconds of the exit, as the lock's settles onto it, so neither a
-- second clock nor no clock is ever seen. 60: the exit eases in, and that
-- long before its end the lock's clock is within a few pixels of the bar's;
-- 120 brought the bar's back with the lock's still 20 pixels off, two clocks
-- for as long. Only under Violet; another lock leaves the bar's clock alone.
local LANDING = 60

local function name(monitor)
    return "bar-" .. monitor.name
end

-- Every monitor's bar, by surface name: what looks.lua's blur rules name.
function bar.names()
    local names = {}
    for _, monitor in ipairs(sol.monitors()) do
        names[#names + 1] = name(monitor)
    end
    return names
end

-- The shell's half: which pop-up to show.
local function tell_shell()
    local config = require("config")
    config.shell = type(config.shell) == "table" and config.shell or {}
    config.shell.properties = type(config.shell.properties) == "table" and config.shell.properties or {}
    config.shell.properties.popup = kept.popup
    require("shell").apply()
end

-- One bar per monitor, on its bottom strip; a monitor that went takes its bar.
function bar.apply()
    local now = {}
    for _, monitor in ipairs(sol.monitors()) do
        local whole = monitor.whole or monitor
        local surface = name(monitor)
        now[surface] = true
        sol.surface(surface, {
            scene = SCENE,
            layer = "top",
            on = { x = whole.x, y = whole.y + whole.h - HEIGHT, w = whole.w, h = HEIGHT },
            interactive = true,
            reserve = { bottom = HEIGHT },
            properties = {
                battery = kept.battery,
                popup = kept.popup,
                clockAway = kept.clock_away,
                clockLanding = LANDING,
            },
        })
    end
    for surface in pairs(kept.shown) do
        if not now[surface] then
            sol.surface(surface, false)
        end
    end
    kept.shown = now
    local looks = package.loaded["looks"]
    if looks and looks.refresh then
        looks.refresh()
    end
end

function bar.battery(value)
    kept.battery = value
    bar.apply()
end

local function set_popup(popup)
    kept.popup = popup
    tell_shell()
    bar.apply()
end

sol.on("surface", function(_, action, data)
    data = type(data) == "table" and data or {}
    if action == "bar.popup" then
        local p = kept.popup
        -- The same button again closes it.
        if p.what == data.what and p.monitor == data.monitor and p.id == data.id then
            set_popup({})
        else
            set_popup({ what = data.what, monitor = data.monitor, x = tonumber(data.x) or 0, id = data.id })
        end
    elseif action == "bar.popup_closed" then
        if kept.popup.what ~= nil then
            set_popup({})
        end
    end
end)

-- The monitors as they are, so a relayout that changed none of them costs
-- nothing here.
local function sizes()
    local parts = {}
    for _, monitor in ipairs(sol.monitors()) do
        local w = monitor.whole or monitor
        parts[#parts + 1] = string.format("%s@%d,%d,%d,%d", monitor.name, w.x, w.y, w.w, w.h)
    end
    return table.concat(parts, ";")
end
local placed_for

local function place()
    placed_for = sizes()
    bar.apply()
end

-- The clock, put away while the lock's is out of it (LANDING, above).
local landing

local function clock_away(away)
    if kept.clock_away ~= away then
        kept.clock_away = away
        bar.apply()
    end
end

sol.on("lock", function(state)
    local lock = require("config").lock
    if type(lock) ~= "table" or lock.scene ~= "Violet" then
        return
    end
    if landing then
        landing:cancel()
        landing = nil
    end
    if state == "locking" or state == "locked" then
        clock_away(true)
    elseif state == "unlocking" then
        local exit = tonumber(lock.exit_ms) or 450
        landing = sol.after(math.max(0, exit - LANDING), function()
            landing = nil
            clock_away(false)
        end)
    else
        clock_away(false)
    end
end)

sol.on("monitors", place)
sol.on("restore", place)
-- A monitor can change size without being announced (a nested window
-- resized by its host): the layouts are told, so the bars ask then.
sol.on("layout", function()
    if sizes() ~= placed_for then
        place()
    end
end)

return bar
