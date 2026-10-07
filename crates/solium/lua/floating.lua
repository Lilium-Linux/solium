-- Desktop mode: snapping a window like Windows, and where a new window
-- lands (#222).
--
-- There is no `floating` layout, on purpose. "Floating" is `modes.lua`'s own
-- name for *no* layout being in charge -- `modes.registered["floating"]` is
-- nil, and that absence is read in three places: `direction.lua` falls back
-- to focusing the nearest window, `modes.toggle_floating` treats the whole
-- desktop as already floating and does nothing, and `modes.lua` itself sends
-- a self-toggle here. Registering a layout under that name would turn all
-- three of those into something else -- `modes.toggle_floating` in
-- particular would start calling an `apply` this file has no reason to have,
-- on a window the user never asked to lift out of anything. So this file
-- never calls `modes.register`, and asks `modes.watch` instead for the one
-- thing it actually needs: knowing when floating starts and stops being the
-- mode in charge, to bind and unbind its own keys.
--
-- What it does, while floating is in charge:
--
--   * `config.floating.snap`'s four keys snap the focused window to a half,
--     a quarter or the whole of its monitor, and back.
--   * A new window is placed and sized by `config.floating.placement`,
--     instead of arriving whatever size its application asked for, wherever
--     the compositor happened to map it (#222's "every window opens in the
--     same place, very large").

local config = require("config")
local modes = require("modes")
local monitors = require("monitors")
local direction = require("direction")
local dialogs = require("dialogs")
local sizes = require("sizes")
local workspaces = require("workspaces")

local floating = {}

-- `before_snap` is the rect a window had before its first snap, by id, so
-- `restore` can put it back whatever has happened since -- a quarter after a
-- half, or the next monitor after that. `maximized` is whether `sol.windows()`'s
-- `id` is maximised right now, which nothing else here can ask for directly
-- (there is no such field; see the `maximize` listener below) and is wanted
-- for the same reason: `restore` un-maximises rather than placing a rect when
-- that is what undoes the last snap. Both kept across `super+shift+r`, for the
-- reason every other mode's own state is: a reload must not forget a window
-- mid-snap. Ids are never reused, so a stale entry left by a closed window
-- only takes up room, never the wrong window's place; `close` below clears it
-- anyway.
local kept = sol.keep("floating", { before_snap = {}, maximized = {} })
local before_snap = kept.before_snap
local maximized = kept.maximized

-- Numbers out of Lua are `f64` through a divide or two: within a pixel counts
-- as the same rect. The same tolerance `direction.lua`'s `SLACK` is, kept
-- separate because the two files have no reason to agree about the name.
local NEAR = 1

local function near(a, b)
    return math.abs(a - b) <= NEAR
end

local function same_rect(a, b)
    return near(a.x, b.x) and near(a.y, b.y) and near(a.w, b.w) and near(a.h, b.h)
end

-- The motion a snap moves with: `config.floating.snap.motion`, or the shipped
-- one when a configuration from before #222 has none.
local function motion()
    local snap = config.floating.snap
    return (type(snap) == "table" and snap.motion) or { duration = 220, easing = "outCubic" }
end

-- Half of `area`, on `side` ("left" or "right").
--
-- The halves do not meet in the middle at a fraction: `area.w / 2` is not
-- always a whole pixel, and splitting the same way on both sides would leave
-- a one-pixel gap or overlap between them. The right half takes what the
-- left half did not, so the two always tile the monitor exactly.
local function half(area, side)
    local w = math.floor(area.w / 2)
    if side == "left" then
        return { x = area.x, y = area.y, w = w, h = area.h }
    end
    return { x = area.x + w, y = area.y, w = area.w - w, h = area.h }
end

-- The top quarter of that half: "from a half, the top quarter if a quarter
-- scheme is simple" is the issue's own hedge, and this is the simple one --
-- the same width the half already has, cut to its top. There is no bottom
-- quarter key; a second press of `maximize` from here does nothing further,
-- as Windows does.
local function quarter(area, side)
    local h = half(area, side)
    return { x = h.x, y = h.y, w = h.w, h = math.floor(h.h / 2) }
end

-- The window with the keyboard, or nil.
local function focused_window()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            return window
        end
    end
    return nil
end

local function place(id, rect)
    sol.animate(motion())
    sol.place(id, { x = rect.x, y = rect.y, w = rect.w, h = rect.h, tile = false })
end

-- Snap the focused window to `side`. A second press from that same half goes
-- to the next monitor that way instead, as Windows does -- `direction.beside`
-- is the same search `super+shift+left` already makes, read from the
-- window's own rect rather than loosely, since the window is already exactly
-- at that monitor's edge.
local function snap(side)
    local window = focused_window()
    if not window then
        return
    end
    if maximized[window.id] then
        -- Un-maximise first: the compositor holds a maximised window at its
        -- own rect regardless of what a script places it at, the same reason
        -- `modes.use` lets every window out of its tile before a new layout
        -- starts (`modes.lua`).
        sol.toggle_maximize(window.id)
        maximized[window.id] = nil
    end
    local area = monitors.named(window.monitor)
    local here = half(area, side)
    if same_rect(window, here) then
        local next = direction.beside(window.monitor, side, window)
        if not next then
            return
        end
        place(window.id, half(monitors.named(next.name), side))
        return
    end
    if not before_snap[window.id] then
        before_snap[window.id] = { x = window.x, y = window.y, w = window.w, h = window.h }
    end
    place(window.id, here)
end

-- Maximise the focused window, or, from a half, its top quarter.
local function maximize_or_quarter()
    local window = focused_window()
    if not window then
        return
    end
    local area = monitors.named(window.monitor)
    for _, side in ipairs({ "left", "right" }) do
        if same_rect(window, half(area, side)) then
            place(window.id, quarter(area, side))
            return
        end
        if same_rect(window, quarter(area, side)) then
            -- As far up as a quarter goes.
            return
        end
    end
    if maximized[window.id] then
        return
    end
    if not before_snap[window.id] then
        before_snap[window.id] = { x = window.x, y = window.y, w = window.w, h = window.h }
    end
    sol.toggle_maximize(window.id)
end

-- Put the focused window back the way it was before its first snap, or
-- un-maximise it, whichever is what the last snap did.
local function restore()
    local window = focused_window()
    if not window then
        return
    end
    if maximized[window.id] then
        sol.toggle_maximize(window.id)
        maximized[window.id] = nil
        before_snap[window.id] = nil
        return
    end
    local rect = before_snap[window.id]
    if not rect then
        return
    end
    before_snap[window.id] = nil
    place(window.id, rect)
end

-- `sol.windows()` has no field for "is this maximised": `sol.toggle_maximize`
-- is the only thing that asks for one, and asking twice undoes it. The
-- `maximize` event is the compositor's own word on it, true for a maximise
-- from any of its three doors -- this file's own key, `super+shift+m`, or the
-- frame's button -- so `restore` undoes whichever one actually happened.
sol.on("maximize", function(id, entering)
    maximized[id] = entering or nil
end)

sol.on("close", function(id)
    before_snap[id] = nil
    maximized[id] = nil
end)

-- Bind the four keys this file owns, from `config.floating.snap`; `false`
-- leaves one unbound, as a key in `config.bindings` does.
local function bind_keys()
    local keys = config.floating.snap or {}
    if keys.left then
        sol.bind(keys.left, function() snap("left") end)
    end
    if keys.right then
        sol.bind(keys.right, function() snap("right") end)
    end
    if keys.maximize then
        sol.bind(keys.maximize, maximize_or_quarter)
    end
    if keys.restore then
        sol.bind(keys.restore, restore)
    end
end

-- Give the four keys back to `direction.lua`'s own bindings. Not `sol.unbind`,
-- which would leave them doing nothing at all in tiling and scrolling, where
-- they have always meant focus by direction (#150) -- the point of asking
-- `direction.lua` to rebind its defaults is that tiling and scrolling never
-- notice this file exists.
local function unbind_keys()
    direction.bind_arrows()
end

modes.watch(function(name)
    if name == "floating" then
        bind_keys()
    else
        unbind_keys()
    end
end)

-- Still bound after a reload, or at a cold start that has never left
-- floating -- `modes.use` is what calls `modes.watch`'s listeners, and
-- neither a reload nor a cold start is a call to it (`modes.lua`'s own
-- `current` starts at `"floating"` and stays there until something switches
-- away). The same reload-safety `overview.lua` gives its own binding.
if modes.current() == "floating" then
    bind_keys()
end

-- Collision avoidance for `placement.policy = "center"`: whether `rect` sits
-- over any of `others`.
local function overlaps(rect, others)
    for _, other in ipairs(others) do
        if rect.x < other.x + other.w and other.x < rect.x + rect.w
            and rect.y < other.y + other.h and other.y < rect.y + rect.h
        then
            return true
        end
    end
    return false
end

-- A rect `w` by `h`, centred on `area`, moved by `offset` a growing number of
-- times and clamped back inside `area` each time, until it clears every rect
-- in `others` or this has been tried too many times to be worth another. The
-- cap is not a tuning knob: it is what keeps a desktop that is already full
-- from looping instead of simply opening the window somewhere, which is what
-- `dialogs.within`'s clamp eventually forces anyway.
--
-- **Starts past as many steps as `others` already holds**, rather than from
-- the centre every time. The search is a fixed sequence of points from the
-- centre outward, so two windows that each give up without ever finding a
-- clear one -- a desktop with no more room, well inside `TRIES` for windows
-- as large as the share `default_size` hands back -- would otherwise give up
-- at the *same* point and land exactly on top of each other, the second
-- window invisible behind the one just placed.
-- `desktop_mode_places_three_new_windows_without_stacking_them` is the two
-- most recent windows, not only the first two.
local TRIES = 16
local function cascaded_from_centre(area, w, h, others, offset)
    local cx, cy = area.x + (area.w - w) / 2, area.y + (area.h - h) / 2
    local base = #others
    local rect
    for step = base, base + TRIES do
        rect = dialogs.within({ x = cx + offset.x * step, y = cy + offset.y * step, w = w, h = h }, area)
        if not overlaps(rect, others) then
            return rect
        end
    end
    return rect
end

-- The size a new window opens at: its own, if it has shown a frame and so
-- has one worth believing; otherwise a share of the work area
-- (`placement.size`). Either way capped by the work area and, once the
-- application has said, by its own minimum and maximum (#115) -- the same
-- two settings and the same `sizes.lua` every layout reads them through, so
-- floating does not grow a second opinion about whether an application's
-- size claim is believed.
local function default_size(window, area)
    local share = type(config.floating.placement) == "table" and config.floating.placement.size or 0.6
    if type(share) ~= "number" or share <= 0 or share > 1 then
        share = 0.6
    end
    local w, h
    if window.shown and window.w > 0 and window.h > 0 then
        w, h = window.w, window.h
    else
        w, h = area.w * share, area.h * share
    end
    local min = sizes.floor(window)
    if min then
        if min.w > 0 then w = math.max(w, min.w) end
        if min.h > 0 then h = math.max(h, min.h) end
    end
    local max = sizes.maximum(window)
    if max then
        if max.w > 0 then w = math.min(w, max.w) end
        if max.h > 0 then h = math.min(h, max.h) end
    end
    return math.min(w, area.w), math.min(h, area.h)
end

-- Where window `id` goes: `config.floating.placement.policy`, from
-- `area` (its monitor's work area) and `others` (the floating windows
-- already on that monitor's desk, for "center" to cascade away from).
function floating.placement_for(window, area, others)
    local w, h = default_size(window, area)
    local settings = config.floating.placement
    local policy = type(settings) == "table" and settings.policy or "center"
    local offset = (type(settings) == "table" and settings.cascade) or { x = 32, y = 32 }
    if policy == "pointer" then
        local cursor = sol.cursor()
        return dialogs.within({ x = cursor.x - w / 2, y = cursor.y - h / 2, w = w, h = h }, area)
    end
    if policy == "cascade" then
        local last = others[#others]
        if not last then
            return { x = area.x + (area.w - w) / 2, y = area.y + (area.h - h) / 2, w = w, h = h }
        end
        return dialogs.within({ x = last.x + offset.x, y = last.y + offset.y, w = w, h = h }, area)
    end
    return cascaded_from_centre(area, w, h, others, offset)
end

-- A new window, while floating is in charge: placed and sized by
-- `floating.placement_for`, rather than left at whatever the compositor
-- mapped it with (#222). A dialog is never this file's: `dialogs.lua` lifts
-- it out of any arrangement already, and placing it here too would fight
-- whichever window later centres it over its parent.
sol.on("open", function(id)
    if modes.current() ~= "floating" or dialogs.floating(id) then
        return
    end
    local window = dialogs.by_id(id)
    if not window then
        return
    end
    local area = monitors.named(window.monitor)
    local others = {}
    for _, each in ipairs(workspaces.visible()) do
        if each.id ~= id and each.monitor == window.monitor and not each.leaving
            and not dialogs.floats(each)
        then
            others[#others + 1] = each
        end
    end
    -- The motion a window already arrives with (`open.lua`'s own
    -- `config.open.motion`, set earlier in this same dispatch) rather than a
    -- setting of this file's: this is still the window opening, and
    -- `placement.size` only decides how big, not how it gets there.
    local rect = floating.placement_for(window, area, others)
    sol.place(id, { x = rect.x, y = rect.y, w = rect.w, h = rect.h, tile = false })
end)

return floating
