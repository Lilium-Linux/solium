-- The floating titlebar's Lua half (qml/panes/rounded-violet: Frame.qml is
-- the bar, which grows to hold QuickSettings.qml, the quick settings).
--
-- The frame sends what the user asked for with `Solium.send`, heard here as
-- `sol.on("frame")`, and is told back what it shows with `sol.frame(id, ...)`,
-- as `windowValues`:
--
--   quick    whether the quick settings are open (a right click on the bar
--            opens them, Esc or a press elsewhere folds them away);
--   above    whether the window is kept above the others (#424);
--   follow   whether the window follows you from workspace to workspace.
--
-- The application's volume needs nothing here: the frame reads and sets it
-- through `Audio.forWindow` itself.

local screenshot = require("screenshot")

local titlebar = {}

local function window(id)
    for _, w in ipairs(sol.windows()) do
        if w.id == id then
            return w
        end
    end
    return nil
end

-- A window follows the workspaces while it belongs to none in particular:
-- workspaces.lua shows such a window on whichever workspace its monitor is
-- showing, so it comes along on every switch.
local function following(id)
    return require("workspaces").of[id] == nil
end

-- What the frame shows of the window's state.
local function tell(id)
    local w = window(id)
    if w then
        sol.frame(id, { above = w.above == true, follow = following(id) })
    end
end

local function close_quick(id)
    sol.frame(id, { quick = false })
end

local handlers = {}

function handlers.quick(id, data)
    if data.open == true then
        tell(id)
    end
    sol.frame(id, { quick = data.open == true })
end

-- Kept above every other window, for what is drawn and what takes clicks.
function handlers.above(id)
    if sol.keep_above then
        sol.keep_above(id, "toggle")
    end
    tell(id)
end

-- Following: off its workspace, so it is on whichever one is shown. Back:
-- onto the workspace in view, as any window that opened there.
function handlers.follow(id)
    local workspaces = require("workspaces")
    if following(id) then
        local w = window(id)
        workspaces.move(id, workspaces.on(w and w.monitor or nil), "follow")
    else
        workspaces.of[id] = nil
        workspaces.apply()
    end
    tell(id)
end

-- Floating and tiled: this window alone while a layout tiles the desktop,
-- the whole desktop into the tiling layout while it floats (a floating
-- desktop has no tile to put one window into).
function handlers.tile(id)
    close_quick(id)
    local modes = require("modes")
    local layout = modes.registered and modes.registered[modes.current()]
    if layout and layout.active then
        modes.toggle_floating(id)
    else
        modes.use("tiling")
    end
end

function handlers.fullscreen(id)
    close_quick(id)
    sol.toggle_fullscreen(id)
end

-- The bar folds back first, so the quick settings are not in the shot.
function handlers.screenshot(id)
    close_quick(id)
    screenshot.window(id)
end

-- For a frozen application: SIGKILL to its process, after the second click.
-- Never to a process that cannot be told, nor to init.
function handlers.kill(id)
    close_quick(id)
    local w = window(id)
    local pid = w and tonumber(w.pid) or -1
    if pid and pid > 1 then
        sol.run("kill", "-KILL", tostring(pid))
    end
end

sol.on("frame", function(id, name, data)
    local handler = handlers[name]
    if handler then
        handler(id, type(data) == "table" and data or {})
    end
end)

return titlebar
