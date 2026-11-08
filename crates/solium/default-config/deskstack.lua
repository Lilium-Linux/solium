-- A 3D overview of the workspaces: they stand in a vertical stack, the chosen
-- one flat and large in the middle on its plate, the ones above and below it
-- tilted away from you, smaller and dimmer, the way the workspaces slide.
--
--   super+tab                 open, then the next desk (wrapping)
--   super+shift+tab           open, then the one before
--   up/down, k/j, tab, wheel  choose
--   1..9                      go straight to that workspace
--   enter, space, a click     go there (a click on a window picks its desk)
--   escape                    close, staying where you were
--
-- Each workspace is a plane made of its windows: every window of a desk is
-- presented scaled about the desk's centre and turned about that same point
-- (a pivot outside the window), so a desk tilts as one sheet. Nothing else is
-- drawn: the wallpaper eases back behind the stack and pans to the chosen
-- desk (looks.lua, qml/wallpaper.qml). Minimised windows stay in their
-- taskbar buttons.

local deskstack = { active = false }

local OPEN = require("motion").travel
local STEP = require("motion").travel
local LEAVE = require("motion").reflow

local SCALE = 0.5        -- the chosen desk, of the monitor
local SHRINK = 0.6       -- each step away from it
local SPACING = 0.42     -- of the work area's height, between neighbours
local TILT = 35          -- degrees, per step away, up to two steps
local PERSPECTIVE = 1500 -- viewer distance, in pixels
local FADE = 0.5         -- opacity one step away; less further out

local chosen = 1
local here -- the monitor row it opened on

local function monitor()
    for _, m in ipairs(sol.monitors()) do
        if m.focused then
            return m
        end
    end
    return sol.monitors()[1]
end

local function minimized(id)
    local taskbar = package.loaded["taskbar"]
    return taskbar and taskbar.is_minimized(id)
end

-- The wallpaper follows the stack: back and blurred while it is open, panned
-- to the chosen desk.
local function backdrop(shown)
    local looks = package.loaded["looks"]
    if looks then
        looks.wallpaper(shown, here and here.name, chosen)
    end
end

-- Where desk `d` steps from the chosen one sits: its scale, and how far its
-- centre is from the work area's, summed step by step as the desks shrink.
local function placement(d, area)
    local steps = math.abs(d)
    local offset = 0
    for k = 1, steps do
        offset = offset + area.h * SPACING * SHRINK ^ (k - 1)
    end
    return SCALE * SHRINK ^ steps, (d < 0 and -offset or offset)
end

local function layout()
    local workspaces = require("workspaces")
    local whole = here.whole or here
    local cx = here.x + here.w / 2
    local cy = here.y + here.h / 2
    local mx = whole.x + whole.w / 2
    local my = whole.y + whole.h / 2

    for _, w in ipairs(sol.windows()) do
        if w.monitor == here.name and not w.leaving and not minimized(w.id) and w.w > 0 and w.h > 0 then
            local d = workspaces.at(w.id, w.monitor) - chosen
            local s, dy = placement(d, here)
            local dcx, dcy = cx, cy + dy
            local rect = {
                x = dcx + (w.x - mx) * s,
                y = dcy + (w.y - my) * s,
                w = w.w * s,
                h = w.h * s,
            }
            local steps = math.min(math.abs(d), 2)
            sol.present(w.id, {
                x = rect.x,
                y = rect.y,
                w = rect.w,
                h = rect.h,
                -- Above the chosen desk the top leans away, below it the bottom.
                rotate_x = (d < 0 and -1 or 1) * steps * TILT,
                perspective = PERSPECTIVE,
                pivot_x = (dcx - rect.x) / rect.w,
                pivot_y = (dcy - rect.y) / rect.h,
                opacity = d == 0 and 1 or math.max(0.2, FADE - 0.2 * (math.abs(d) - 1)),
                z = 100 - math.abs(d),
            })
        end
    end
end

local KEYS = {
    up = function() deskstack.step(-1) end,
    k = function() deskstack.step(-1) end,
    ["shift+tab"] = function() deskstack.step(-1) end,
    down = function() deskstack.step(1) end,
    j = function() deskstack.step(1) end,
    tab = function() deskstack.step(1) end,
    ["return"] = function() deskstack.pick() end,
    kp_enter = function() deskstack.pick() end,
    space = function() deskstack.pick() end,
    escape = function() deskstack.leave() end,
}
for n = 1, 9 do
    KEYS[tostring(n)] = function() deskstack.pick(n) end
end

local function keys(on)
    for combo, run in pairs(KEYS) do
        if on then
            sol.bind(combo, run)
        else
            sol.unbind(combo)
        end
    end
end

function deskstack.enter()
    if deskstack.active then
        return
    end
    local overview = require("overview")
    local recents = package.loaded["recents"]
    if overview.active or (recents and recents.active) then
        return
    end
    here = monitor()
    if not here then
        return
    end
    chosen = require("workspaces").on(here.name)

    deskstack.active = true
    sol.animate(OPEN)
    overview.gather()
    layout()
    backdrop(true)
    sol.grab_input(true)
    sol.status("desks")
    keys(true)
end

function deskstack.step(by, wrap)
    if not deskstack.active then
        return
    end
    local count = require("workspaces").count()
    local next_one = math.max(1, math.min(count, chosen + by))
    if wrap then
        next_one = (chosen - 1 + by) % count + 1
    end
    if next_one == chosen then
        return
    end
    chosen = next_one
    sol.animate(STEP)
    layout()
    backdrop(true)
end

-- Close, every window back where its layout keeps it; `go` first switches to
-- the chosen workspace, so its windows land straight in place.
local function close(go)
    deskstack.active = false
    keys(false)
    if go then
        require("workspaces").go(chosen, here.name)
    end
    sol.animate(LEAVE)
    for _, w in ipairs(sol.windows()) do
        if not minimized(w.id) then
            sol.present_clear(w.id)
        end
    end
    require("overview").scatter(LEAVE)
    backdrop(false)
    sol.grab_input(false)
    sol.status("")
end

function deskstack.pick(index)
    if not deskstack.active then
        return
    end
    if index then
        chosen = math.max(1, math.min(require("workspaces").count(), index))
    end
    close(true)
end

function deskstack.leave()
    if deskstack.active then
        close(false)
    end
end

function deskstack.toggle()
    if deskstack.active then
        deskstack.leave()
    else
        deskstack.enter()
    end
end

-- Super held, Tab again: the next desk, as Alt+Tab steps its cards. Picking
-- is Enter or a click (a script hears presses, not Super being let go).
sol.bind("super+tab", function()
    if deskstack.active then
        deskstack.step(1, true)
    else
        deskstack.enter()
    end
end)

sol.bind("super+shift+tab", function()
    if deskstack.active then
        deskstack.step(-1, true)
    else
        deskstack.enter()
    end
end)

-- The backdrop surface an earlier version of this file drew; gone now.
sol.surface("desk-stack", false)

-- Clicks and the wheel come from the shell's picker layer (Shell.qml).
sol.on("surface", function(_, action, data)
    if not deskstack.active then
        return
    end
    data = type(data) == "table" and data or {}
    if action == "picker.wheel" then
        deskstack.step(tonumber(data.by) or 1)
    elseif action == "picker.click" then
        local whole = here.whole or here
        local x = whole.x + (tonumber(data.x) or 0)
        local y = whole.y + (tonumber(data.y) or 0)
        local id = sol.window_at(x, y)
        if id then
            deskstack.pick(require("workspaces").at(id, here.name))
            return
        end
        -- Empty space where the chosen desk stands goes there too.
        local cx, cy = here.x + here.w / 2, here.y + here.h / 2
        if math.abs(x - cx) <= whole.w * SCALE / 2 and math.abs(y - cy) <= whole.h * SCALE / 2 then
            deskstack.pick()
        else
            deskstack.leave()
        end
    end
end)

return deskstack
