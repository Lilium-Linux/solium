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
    -- Whether this window was already maximised when the key was pressed,
    -- kept past the un-maximise below: `window` is the snapshot read at the
    -- top of this call, before `sol.toggle_maximize`'s effect has landed, so
    -- while it was maximised, `window.x/y/w/h` is still the maximised
    -- rectangle -- the whole work area -- not whatever it was before that.
    local was_maximized = maximized[window.id]
    if was_maximized then
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
        -- The window a layout moved onto another monitor goes to the
        -- workspace that monitor shows (`workspaces.carry`'s own comment) --
        -- the same call `direction.lua`'s own cross-monitor `floating.move`
        -- makes right after its `put`, for the same reason: nothing else
        -- follows a move by key with a `layout` event.
        -- `desktop_mode_a_cross_monitor_snap_carries_the_window_to_the_workspace_its_new_monitor_shows`.
        workspaces.carry(window.id, next.name)
        return
    end
    -- `was_maximized` is skipped rather than captured here: maximised
    -- through one of this file's *other* two doors -- the frame's button, or
    -- `super+shift+m` -- `window` is still the stale, maximised snapshot
    -- from the top of this call, since the un-maximise above has not landed
    -- within this same handler. The compositor is the only thing that still
    -- remembers this window's true pre-maximise rectangle, and placing it at
    -- `here` is about to throw that away regardless; recording the stale
    -- rectangle would only make a later `restore` place the window at
    -- something close to the whole work area it was just maximised to, not
    -- back where it truly was. Left unset, `restore` does nothing instead,
    -- which is honest about there being nothing left here worth restoring.
    -- `desktop_mode_snap_after_an_external_maximize_does_not_capture_a_bogus_before_snap_rect`.
    if not was_maximized and not before_snap[window.id] then
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

-- A window let go over a desk of another workspace -- one switched to
-- mid-drag, or shown by the monitor it was dragged onto -- belongs to that
-- workspace now, and is shown where it was let go (#401). The layouts do this
-- in their own `drop`; with none in charge, it is done here.
-- `a_floating_window_dropped_on_the_other_monitors_workspace_belongs_to_it`.
sol.on("drop", function(id, x, y)
    if modes.current() == "floating" then
        workspaces.drop(id, x, y)
    end
end)

-- Exactly the combinations the last `bind_keys` call bound, so `unbind_keys`
-- can let go of exactly those rather than asking `config.floating.snap`
-- again -- which `config.floating.snap` may have moved off the plain arrows
-- entirely by then (`unbind_keys`'s own comment).
local bound = {}

-- Bind the four keys this file owns, from `config.floating.snap`; `false`
-- leaves one unbound, as a key in `config.bindings` does.
--
-- A combination `config.bindings` has an entry for -- even `false` -- is
-- skipped rather than taken over: that entry already outranks every shipped
-- binding once (#117, `require("bindings")` is the last line of `init.lua`),
-- and a user who wrote `config.bindings["super+left"] = ...` wins at the
-- *first* load while this file's own `bind_keys` runs before `bindings.lua`
-- does -- "at load it wins as intended" is the premise
-- `a_config_bindings_override_on_an_arrow_survives_leaving_and_returning_to_floating`
-- starts from. Without this, the override would win at load, lose the first
-- time floating let go and `unbind_keys` handed the arrows back to it, and
-- then lose *again* the next time floating took over, since this function
-- would reassert the shipped snap over it a second time with nothing left
-- to stop it.
local function take(combo, action)
    if not combo then
        return
    end
    if (config.bindings or {})[combo] ~= nil then
        return
    end
    sol.bind(combo, action)
    bound[#bound + 1] = combo
end

local function bind_keys()
    bound = {}
    local keys = config.floating.snap or {}
    take(keys.left, function() snap("left") end)
    take(keys.right, function() snap("right") end)
    take(keys.maximize, maximize_or_quarter)
    take(keys.restore, restore)
end

-- Give back exactly what `bind_keys` bound, then put the plain arrows back
-- to whatever `config.bindings` says for them -- not `direction.bind_arrows()`
-- unconditionally, which only knows the shipped answer (`sol.focus_direction`)
-- and used to reassert it over a configured override every time floating let
-- go, not only once: a `config.bindings["super+left"]` function won at load,
-- since `require("bindings")` is the last line of `init.lua`
-- (`bindings.lua`'s own comment), and then lost to this on the very first
-- `super+t`. `bindings.rebind` is `require("bindings")`'s own decision, asked
-- again for one combination, so the arrows go back to what a reload would
-- have given them -- a configured override, or the shipped default, never
-- unconditionally the latter.
--
-- The first loop is what the arrows alone cannot be: a snap key
-- `config.floating.snap` moved off a plain arrow, onto a combination of its
-- own (`snap.left = "super+z"`, say), is nothing the second loop below ever
-- looks at, and so would be left bound to `snap` forever once tiling or
-- scrolling took over, with nothing left to ever call `sol.unbind` on it.
-- `require("bindings")` happens here, lazily, rather than at this file's own
-- top level: this file loads before `bindings.lua` does (`init.lua`'s own
-- order), and asking at load would run `config.bindings` before tiling,
-- scrolling and this file's own `bind_keys` have bound anything at all,
-- inverting "the later call wins" for every configured key, not only these
-- four. By the time a mode switch can call this, `init.lua` has already
-- required `bindings` once, so this only ever reaches the cache.
-- `a_config_bindings_override_on_an_arrow_survives_leaving_and_returning_to_floating`
-- and `a_snap_key_moved_off_the_plain_arrows_is_not_left_bound_once_floating_lets_go`.
local function unbind_keys()
    for _, combo in ipairs(bound) do
        sol.unbind(combo)
    end
    bound = {}
    local bindings = require("bindings")
    for _, dir in ipairs({ "left", "right", "up", "down" }) do
        bindings.rebind("super+" .. dir, function() sol.focus_direction(dir) end)
    end
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

-- `config.floating.placement`, each key checked, and the shipped value for
-- one a configuration from before it has none of, or one this file cannot
-- use: a `size` outside (0, 1], or a `cascade` axis that is not a step of a
-- pixel or more -- a step of nothing would put every window on the first,
-- and the walk below would never leave the spot it started on.
-- `desktop_mode_a_cascade_of_no_step_is_read_as_the_shipped_one`.
local function placement_settings()
    local given = type(config.floating.placement) == "table" and config.floating.placement or {}
    local size = given.size
    if type(size) ~= "number" or size <= 0 or size > 1 then
        size = 0.6
    end
    local cascade = type(given.cascade) == "table" and given.cascade or {}
    local function step(value)
        if type(value) == "number" and value >= 1 then
            return value
        end
        return 32
    end
    return {
        policy = given.policy,
        size = size,
        step = { x = step(cascade.x), y = step(cascade.y) },
        wrap = given.wrap == "center" and "center" or "corner",
    }
end

-- The top-left corner a `w` by `h` window has when centred on `area`, on a
-- whole pixel, so a row of steps from it stays on whole pixels too.
-- `desktop-placement-center.lua`.
local function centred(area, w, h)
    return {
        x = math.floor(area.x + (area.w - w) / 2),
        y = math.floor(area.y + (area.h - h) / 2),
    }
end

-- Whether a `w` by `h` window with its top-left corner at `spot` is inside
-- `area`, within the pixel `NEAR` allows for a divide.
-- `desktop-placement-wrap-corner.lua`.
local function fits(spot, w, h, area)
    return spot.x >= area.x - NEAR and spot.y >= area.y - NEAR
        and spot.x + w <= area.x + area.w + NEAR
        and spot.y + h <= area.y + area.h + NEAR
end

-- Whether `spot` is taken: another window's top-left corner is nearer to it
-- than half a `step` on both axes. Half a step, so a spot the cascade itself
-- made, a whole step from the one before, is free whatever a pixel of
-- rounding did to either, while a window dragged a few pixels off a spot
-- still holds it -- a new window there would sit all but on top of it.
-- `desktop_mode_a_spot_a_window_was_nudged_off_is_still_taken`.
local function taken(spot, others, step)
    for _, other in ipairs(others) do
        if math.abs(other.x - spot.x) < step.x / 2 and math.abs(other.y - spot.y) < step.y / 2 then
            return true
        end
    end
    return false
end

-- Where the `n`th new diagonal starts, once the one before would leave the
-- work area (`placement.wrap`). "corner": the work area's top-left plus one
-- step, and one `step.x` further right for each diagonal after that; down by
-- no more than the room the window leaves below it, so a window nearly as
-- tall as the work area still gets these diagonals rather than none
-- (`desktop_mode_windows_nearly_the_work_area_tall_get_spots_of_their_own`).
-- "center": the centre, one `step.x` further right for each diagonal, so the
-- first never starts on the first window. Either way each starts on a column
-- of its own, so no two restarts retrace each other. One may still cross the
-- diagonal the walk began on -- on a square work area the corner's first
-- lies along the centre's -- and there the spots already held are `taken`
-- and stepped past like any other.
-- `desktop-placement-wrap-corner.lua`, `desktop-placement-wrap-center.lua`.
local function restart(n, area, w, h, step, wrap)
    if wrap == "center" then
        local centre = centred(area, w, h)
        return { x = centre.x + step.x * n, y = centre.y }
    end
    return { x = area.x + step.x * n, y = area.y + math.min(step.y, math.floor(area.h - h)) }
end

-- The first spot from `start` that no window in `others` has taken: one
-- `step` down and right at a time, and a new diagonal (`restart`) whenever
-- the next step would take the window out of `area`, rather than clamping it
-- back onto the last spot that fit (#240). Nil when even the next diagonal's
-- first spot does not fit: every spot this walk can reach is taken, and
-- `grid_spot` looks further.
--
-- Finite without a cap: a step is a pixel or more on both axes
-- (`placement_settings`), so every diagonal leaves the area, and each
-- restart starts further right than the one before, so they run out too.
-- `desktop-placement-center.lua`, `desktop-placement-pointer.lua`.
local function free_spot(start, w, h, area, others, step, wrap)
    local spot = start
    local restarts = 0
    while true do
        if not fits(spot, w, h, area) then
            restarts = restarts + 1
            spot = restart(restarts, area, w, h, step, wrap)
            if not fits(spot, w, h, area) then
                return nil
            end
        end
        if not taken(spot, others, step) then
            return spot
        end
        spot = { x = spot.x + step.x, y = spot.y + step.y }
    end
end

-- The offsets along one axis of `grid_spot`'s grid, with `room` pixels to
-- spare on it: 0, then one `stride` at a time while there is room, and
-- `room` itself, the far edge, where the strides fall short of it.
-- `desktop_mode_windows_nearly_the_work_area_wide_get_spots_of_their_own`.
local function offsets(room, stride)
    local out = {}
    local at = 0
    while at < room do
        out[#out + 1] = at
        at = at + stride
    end
    out[#out + 1] = room
    return out
end

-- The first spot no window in `others` has taken on a grid of `step` laid
-- over every spot where a `w` by `h` window fits in `area`, a row at a time
-- from the top-left. The last resort once every diagonal `free_spot` starts
-- is spent: a window nearly as wide as the work area leaves no room for a
-- diagonal at all, and one nearly as tall leaves room for a single spot on
-- each, so without it every window after the first few would land on
-- another. Nil only when every spot of the grid is taken.
--
-- Finite and short: the grid is bounded by the work area, and a window takes
-- at most two of its offsets on each axis (`taken` reaches half a step
-- either way; the far edge may sit nearer than a step to the one before), so
-- a free spot turns up within four checks a window.
-- `desktop_mode_once_every_diagonal_is_spent_a_window_takes_a_free_spot_of_the_grid`,
-- `desktop-placement-tall-and-wide.lua`.
local function grid_spot(w, h, area, others, step)
    local columns = offsets(math.floor(area.w - w), step.x)
    for _, dy in ipairs(offsets(math.floor(area.h - h), step.y)) do
        for _, dx in ipairs(columns) do
            local spot = { x = area.x + dx, y = area.y + dy }
            if not taken(spot, others, step) then
                return spot
            end
        end
    end
    return nil
end

-- The window of `others` opened last: ids are handed out in order and never
-- reused, so the highest. `"cascade"`'s "the last window opened", which is
-- not the last one `workspaces.visible` happens to list.
-- `desktop-placement-cascade.lua`.
local function newest(others)
    local last = nil
    for _, other in ipairs(others) do
        if not last or other.id > last.id then
            last = other
        end
    end
    return last
end

-- The size a new window opens at: its own, if it has shown a frame and so
-- has one worth believing; otherwise `share` of the work area
-- (`placement.size`). Either way capped by the work area and, once the
-- application has said, by its own minimum and maximum (#115) -- the same
-- two settings and the same `sizes.lua` every layout reads them through, so
-- floating does not grow a second opinion about whether an application's
-- size claim is believed.
-- `desktop_mode_a_window_with_no_size_of_its_own_gets_a_share_of_the_work_area`.
local function default_size(window, area, share)
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

-- Where window `window` goes: `config.floating.placement`, from `area` (the
-- work area of the monitor it opened on) and `others` (the floating windows
-- already on that monitor's desk).
--
-- The policy only says where the walk starts: the centre ("center", and
-- anything unknown), one step from the last window opened ("cascade"), or
-- centred on the pointer ("pointer"). Every policy then steps past the spots
-- already taken (`free_spot`, then `grid_spot`), so no two new windows share
-- one while the work area has a spot free (#240). Until #240 the
-- "center" search stepped away from any window the new one *overlapped* --
-- and at 60% of the work area a step of 32 pixels overlaps every time, so
-- the search ran out at the bottom-right corner it was clamped into, and
-- every window after the first landed there.
-- `desktop_mode_ten_spawned_windows_step_by_cascade_and_wrap_at_the_edge` and the
-- `desktop-placement-*.lua` scenarios.
function floating.placement_for(window, area, others)
    local settings = placement_settings()
    local w, h = default_size(window, area, settings.size)
    local step = settings.step
    local start
    local last = settings.policy == "cascade" and newest(others)
    if settings.policy == "pointer" then
        local cursor = sol.cursor()
        local around = dialogs.within({
            x = math.floor(cursor.x - w / 2),
            y = math.floor(cursor.y - h / 2),
            w = w,
            h = h,
        }, area)
        start = { x = around.x, y = around.y }
    elseif last then
        start = { x = last.x + step.x, y = last.y + step.y }
    else
        start = centred(area, w, h)
    end
    local spot = free_spot(start, w, h, area, others, step, settings.wrap)
        or grid_spot(w, h, area, others, step)
    if not spot then
        -- Nowhere free, which for a window as large as the work area is
        -- every spot after the first: the start, kept inside the work area.
        -- `desktop_mode_with_no_free_spot_a_window_lands_where_its_policy_starts`.
        return dialogs.within({ x = start.x, y = start.y, w = w, h = h }, area)
    end
    return { x = spot.x, y = spot.y, w = w, h = h }
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
    -- The focused monitor's work area: the compositor opens every new window
    -- on the monitor under the pointer (`Solium::active_output`, which both
    -- a launch's slot and a client's first placement read), so at this
    -- moment that is the one `window.monitor` names -- and naming it rather
    -- than asking again keeps the window on the monitor `workspaces.lua`'s
    -- own `open` has just filed it under. `desktop-placement-two-monitors.lua`.
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
