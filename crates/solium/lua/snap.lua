-- Snapping a floating window by dragging it to a screen's edge (#455), as
-- Windows, macOS and KDE do, and the holder between windows snapped side by
-- side.
--
-- While floating is in charge (`modes.lua`), a window dragged by its titlebar,
-- with the drag modifier, or by a finger:
--
--   * shows a preview of where it would go while the pointer is in a zone of
--     the screen it is on: the left or right edge is that half of the work
--     area, a corner that quarter, the top edge the whole work area
--     (`floating.snap.drag.top`), bottom corners their quarter;
--   * let go in a zone, glides there with `tiling.snap`'s motion, the one a
--     tiled window snaps into its tile with, and is marked as snapped:
--     `sol.frame(id, { snapped = <zone> })`, so a pane style can square its
--     corners; the top edge maximises it instead;
--   * taken by its titlebar again, it stays put for `maximize.restore_after`
--     pixels, as a maximised window does (#452), and past them comes off at
--     the size it had before its snap, under the pointer, held as far along
--     its titlebar as it was taken.
--
-- Windows snapped beside one another share a seam. Dragging either window's
-- edge along it moves the seam, and every window on either side of it with
-- it, as a tiled seam moves -- through the same `resize` event and the same
-- configure path, every frame -- held by each application's own minimum. A
-- holder is drawn over the seam (`qml/snap/snap.qml`) while no other window
-- covers it; it draws only, so the press reaches the windows' own resize
-- borders beneath, and the pointer shows their resize cursor.
--
-- Zones are per screen, and only on a screen's outer edges: where two screens
-- touch, the pointer goes on to the next screen rather than stopping at an
-- edge, so an edge zone there would snap a window every time it was dragged
-- across. Decided point by point, so a taller screen beside a shorter one
-- still snaps along the part of its edge nothing touches. A corner needs both
-- of its edges outer; beside a screen it is part of the edge that is.
--
-- The compositor knows none of this: it tells `drag`, `drop` and `resize`,
-- and holds, grips and places what it is told to. `lua/floating.lua` snaps by
-- key through the same marks (`snap.mark`), and tells this file each drop.

local config = require("config")
local modes = require("modes")
local monitors = require("monitors")
local workspaces = require("workspaces")

local snap = {}

-- `before` is the rect a window had before its first snap, by id, so taking
-- it off its zone -- by a drag, or `super+down` -- gives it back its size.
-- `side` is the zone a window is snapped to, by id: "left", "right",
-- "top_left", "top_right", "bottom_left" or "bottom_right". Both kept across
-- `super+shift+r`, as every mode's own state is.
local kept = sol.keep("snap", { before = {}, side = {} })
snap.before = kept.before
local side = kept.side

-- The scene that draws the preview and the holders, on every monitor, over
-- the windows.
local SURFACE = "snap"

-- Within a pixel counts as the same edge: numbers out of Lua go through a
-- divide or two.
local NEAR = 1

local function number(value, default, least)
    if type(value) == "number" and value == value and value >= least then
        return value
    end
    return default
end

-- `config.floating.snap.drag`, each key checked, and the shipped value for
-- one a configuration has none of or one this file cannot use.
local function settings()
    local floating = type(config.floating) == "table" and config.floating or {}
    local keys = type(floating.snap) == "table" and floating.snap or {}
    local drag = type(keys.drag) == "table" and keys.drag or {}
    local divider = type(drag.divider) == "table" and drag.divider or {}
    local tiling = type(config.tiling) == "table" and config.tiling or {}
    return {
        enabled = drag.enabled ~= false,
        edge = number(drag.edge, 8, 1),
        corner = number(drag.corner, 120, 0),
        top = drag.top ~= false,
        motion = drag.motion or tiling.snap or { duration = 180, easing = "outCubic" },
        scene = type(drag.scene) == "string" and drag.scene or "snap/snap.qml",
        preview = type(drag.preview) == "table" and drag.preview or {},
        divider = {
            enabled = divider.enabled ~= false,
            minimum = number(divider.minimum, 120, 1),
            look = divider,
        },
    }
end

-- How far a snapped window's drag goes before it comes off its zone, and how
-- far any drag goes before a zone is offered at all: the same few pixels a
-- maximised window holds for (#452), so a click on a titlebar at the top of
-- the screen does not flash a preview.
local function restore_after()
    local pixels = type(config.maximize) == "table" and config.maximize.restore_after
    if type(pixels) == "number" and pixels >= 0 then
        return pixels
    end
    return 6
end

-- Whether snapping is this file's to do now: no layout in charge, and not
-- switched off.
local function active()
    return modes.current() == "floating" and settings().enabled
end

local function by_id(id)
    for _, window in ipairs(sol.windows()) do
        if window.id == id then
            return window
        end
    end
end

local function rect_of(window)
    return { x = window.x, y = window.y, w = window.w, h = window.h }
end

-- The monitor whose whole screen holds the point, or nil.
local function screen_at(x, y)
    for _, monitor in ipairs(sol.monitors()) do
        local whole = monitor.whole or monitor
        if x >= whole.x and x < whole.x + whole.w and y >= whole.y and y < whole.y + whole.h then
            return monitor
        end
    end
end

-- Where a window snapped to `kind` goes in `area`, a work area. The halves
-- and quarters meet on whole pixels: the right and bottom ones take what the
-- left and top did not, so they always tile the area exactly.
function snap.rect(area, kind)
    if kind == "maximize" then
        return { x = area.x, y = area.y, w = area.w, h = area.h }
    end
    local w = math.floor(area.w / 2)
    local h = math.floor(area.h / 2)
    local rect = { x = area.x, y = area.y, w = area.w, h = area.h }
    if kind:find("left") then
        rect.w = w
    elseif kind:find("right") then
        rect.x, rect.w = area.x + w, area.w - w
    end
    if kind:find("^top") then
        rect.h = h
    elseif kind:find("^bottom") then
        rect.y, rect.h = area.y + h, area.h - h
    end
    return rect
end

-- The zone the pointer at `x, y` is in, or nil: `{ monitor, kind, rect }`,
-- `rect` where a window let go there goes, in the global space.
function snap.zone(x, y)
    local s = settings()
    local monitor = screen_at(x, y)
    if not monitor then
        return nil
    end
    local whole = monitor.whole or monitor
    local right_x, bottom_y = whole.x + whole.w, whole.y + whole.h
    -- An edge is outer at this point when no screen lies just past it.
    local outer_left = not screen_at(whole.x - 1, y)
    local outer_right = not screen_at(right_x, y)
    local outer_top = not screen_at(x, whole.y - 1)
    local outer_bottom = not screen_at(x, bottom_y)
    local left = outer_left and x - whole.x < s.edge
    local right = outer_right and right_x - x <= s.edge
    local top = outer_top and y - whole.y < s.edge
    local bottom = outer_bottom and bottom_y - y <= s.edge
    local kind
    if left or right then
        local h = left and "left" or "right"
        if outer_top and y - whole.y < s.corner then
            kind = "top_" .. h
        elseif outer_bottom and bottom_y - y <= s.corner then
            kind = "bottom_" .. h
        else
            kind = h
        end
    elseif top or bottom then
        local v = top and "top" or "bottom"
        if outer_left and x - whole.x < s.corner then
            kind = v .. "_left"
        elseif outer_right and right_x - x <= s.corner then
            kind = v .. "_right"
        elseif top and s.top then
            kind = "maximize"
        end
    end
    if not kind then
        return nil
    end
    return { monitor = monitor.name, kind = kind, rect = snap.rect(monitors.named(monitor.name), kind) }
end

-- What is being dragged, by id: `began`, the rect to remember as the one
-- before a snap; `from`, where the drag began; `held`, how a snapped window
-- was taken, while it is held on its zone; `zone`, the zone the pointer is in.
local drags = {}

-- The zone being previewed, and the seam an edge drag is moving now.
local previewing = nil
local moving_seam = nil

-- Where this file has placed windows, by id. `sol.windows()` is the snapshot
-- taken for the handler running now, so a window placed in it is still at
-- its old rect there, and the holders drawn at the end of the same handler
-- would be drawn where the windows were. Each placement is kept with the rect
-- it was placed from, and believed only while the snapshot still says that:
-- the next handler's snapshot has the window where it was placed.
local placed = {}

local function place(window, rect)
    sol.place(window.id, { x = rect.x, y = rect.y, w = rect.w, h = rect.h, tile = false })
    placed[window.id] = { from = rect_of(window), to = { x = rect.x, y = rect.y, w = rect.w, h = rect.h } }
end

-- `window` where it is, placements of this handler's included.
local function current(window)
    local moved = placed[window.id]
    if not moved then
        return window
    end
    local from = moved.from
    if window.x ~= from.x or window.y ~= from.y or window.w ~= from.w or window.h ~= from.h then
        placed[window.id] = nil
        return window
    end
    return setmetatable({ x = moved.to.x, y = moved.to.y, w = moved.to.w, h = moved.to.h },
        { __index = window })
end

-- Every snapped window on screen, topmost first, and the full list beside it
-- for what may cover them.
local function on_screen()
    local shown = {}
    for _, window in ipairs(workspaces.visible()) do
        shown[#shown + 1] = current(window)
    end
    local snapped = {}
    for _, window in ipairs(shown) do
        if side[window.id] and not window.leaving and not window.maximized
            and not window.fullscreen and window.w > 0 and window.h > 0
        then
            snapped[#snapped + 1] = window
        end
    end
    return snapped, shown
end

-- On axis "x" a window's near edge is its left and its far edge its right;
-- on "y", its top and its bottom. `along` is the other axis.
local AXES = {
    x = { at = "x", size = "w", along = "y", length = "h" },
    y = { at = "y", size = "h", along = "x", length = "w" },
}

local function overlap(a, b, axis)
    local o = AXES[axis]
    local from = math.max(a[o.along], b[o.along])
    local to = math.min(a[o.along] + a[o.length], b[o.along] + b[o.length])
    return from, to
end

-- The seam on `axis` at `at` that `start` touches, among `windows`: `before`,
-- the windows whose far edge is on it, and `after`, those whose near edge
-- is, each joined only through one on the other side it lies beside, so two
-- pairs that happen to line up on different halves of a screen are two
-- seams. Nil unless both sides have a window.
local function seam_at(windows, start, axis, at)
    local o = AXES[axis]
    local function far(window) return window[o.at] + window[o.size] end
    local member = { [start.id] = start }
    local grew = true
    while grew do
        grew = false
        for _, window in ipairs(windows) do
            if not member[window.id] and window.monitor == start.monitor then
                local is_before = math.abs(far(window) - at) <= NEAR
                local is_after = math.abs(window[o.at] - at) <= NEAR
                for _, joined in pairs(member) do
                    local joined_before = math.abs(far(joined) - at) <= NEAR
                    local from, to = overlap(window, joined, axis)
                    if to - from > NEAR
                        and ((is_before and not joined_before) or (is_after and joined_before))
                    then
                        member[window.id] = window
                        grew = true
                        break
                    end
                end
            end
        end
    end
    local seam = { axis = axis, at = at, before = {}, after = {}, monitor = start.monitor }
    for _, window in pairs(member) do
        if math.abs(far(window) - at) <= NEAR then
            seam.before[#seam.before + 1] = window
        else
            seam.after[#seam.after + 1] = window
        end
    end
    if #seam.before == 0 or #seam.after == 0 then
        return nil
    end
    -- Along the seam, from where the two sides first meet to where they last do.
    for _, a in ipairs(seam.before) do
        for _, b in ipairs(seam.after) do
            local from, to = overlap(a, b, axis)
            if to > from then
                seam.from = math.min(seam.from or from, from)
                seam.to = math.max(seam.to or to, to)
            end
        end
    end
    -- Named by its windows rather than by where it is, so the seam an edge
    -- drag is moving keeps its name as it moves.
    local ids = {}
    for each in pairs(member) do
        ids[#ids + 1] = each
    end
    table.sort(ids)
    seam.key = seam.monitor .. ":" .. axis .. ":" .. table.concat(ids, ",")
    return seam
end

-- Every seam between snapped windows on screen, once each.
local function seams(windows)
    local found, out = {}, {}
    for _, window in ipairs(windows) do
        for axis, o in pairs(AXES) do
            local seam = seam_at(windows, window, axis, window[o.at] + window[o.size])
            if seam and not found[seam.key] then
                found[seam.key] = true
                out[#out + 1] = seam
            end
        end
    end
    return out
end

-- Whether a window that is none of the seam's is stacked over the seam's
-- topmost window and covers any of the seam: then its holder is not drawn,
-- as a window's own edge is not seen through the window over it.
local function covered(seam, shown, settings_)
    local o = AXES[seam.axis]
    local half = number(settings_.divider.look.thickness, 6, 1)
    local band = {
        [o.at] = seam.at - half, [o.size] = half * 2,
        [o.along] = seam.from, [o.length] = seam.to - seam.from,
    }
    local mine = {}
    for _, window in ipairs(seam.before) do mine[window.id] = true end
    for _, window in ipairs(seam.after) do mine[window.id] = true end
    for _, window in ipairs(shown) do
        if mine[window.id] then
            return false
        end
        if window.x < band.x + band.w and band.x < window.x + window.w
            and window.y < band.y + band.h and band.y < window.y + window.h
        then
            return true
        end
    end
    return false
end

-- A rect in the global space, in the coordinates of the monitor `name`'s
-- own scene.
local function local_to(name, rect)
    for _, monitor in ipairs(sol.monitors()) do
        if monitor.name == name then
            local whole = monitor.whole or monitor
            return { x = rect.x - whole.x, y = rect.y - whole.y, w = rect.w, h = rect.h }
        end
    end
    return rect
end

-- Tell the scene what to draw: the zone, and the seams.
local function draw()
    if not active() then
        return
    end
    local s = settings()
    local zone = false
    if previewing then
        zone = local_to(previewing.monitor, previewing.rect)
        zone.monitor = previewing.monitor
        zone.kind = previewing.kind
    end
    local holders = {}
    if s.divider.enabled then
        local snapped, shown = on_screen()
        for _, seam in ipairs(seams(snapped)) do
            if not covered(seam, shown, s) then
                local line = seam.axis == "x"
                    and { x = seam.at, y = seam.from, w = 0, h = seam.to - seam.from }
                    or { x = seam.from, y = seam.at, w = seam.to - seam.from, h = 0 }
                local holder = local_to(seam.monitor, line)
                holder.monitor = seam.monitor
                holder.vertical = seam.axis == "x"
                holder.dragging = seam.key == moving_seam
                holders[#holders + 1] = holder
            end
        end
    end
    sol.surface(SURFACE, {
        scene = s.scene,
        layer = "top",
        on = "every-monitor",
        interactive = false,
        properties = {
            zone = zone,
            seams = holders,
            preview = s.preview,
            divider = s.divider.look,
        },
    })
end
snap.draw = draw

-- Say that window `id` is snapped to `kind`, or no longer is (nil): kept,
-- handed to its frame as `snapped`, false when it is not, and the holders
-- drawn again for it.
function snap.mark(id, kind)
    side[id] = kind
    sol.frame(id, { snapped = kind or false })
    draw()
end

local function motion()
    return settings().motion
end

-- Take a held, snapped window off its zone: back to the size it had before
-- its snap, gripped where it was taken.
local function unsnap(id, drag)
    local window = by_id(id)
    local before = snap.before[id]
    snap.mark(id, nil)
    snap.before[id] = nil
    if window and before then
        sol.animate(motion())
        place(window, { x = window.x, y = window.y, w = before.w, h = before.h })
        local down = drag.held.down
        if drag.held.titlebar > 0 then
            down = math.min(down, drag.held.titlebar - 1)
        end
        sol.drag_grip(id, { x = drag.held.along, dy = down })
    end
    sol.drag_hold(id, false)
    -- Snapped again in this same drag, it still comes back to this.
    drag.began = before
    draw()
end

local function same(a, b)
    return (a and a.monitor) == (b and b.monitor) and (a and a.kind) == (b and b.kind)
end

local function preview(zone)
    if same(zone, previewing) then
        return
    end
    previewing = zone
    draw()
end

sol.on("drag", function(id, phase, x, y)
    if phase == "begin" then
        drags[id] = nil
        if not active() then
            return
        end
        local window = by_id(id)
        if not window or window.fullscreen then
            return
        end
        local drag = { from = { x = x, y = y } }
        if not window.maximized and not side[id] then
            drag.began = rect_of(window)
        end
        if side[id] and window.w > 0 then
            drag.held = {
                along = math.min(math.max((x - window.x) / window.w, 0), 1),
                down = math.max(y - window.y, 0),
                titlebar = window.insets.top,
            }
            sol.drag_hold(id, true)
        end
        drags[id] = drag
        return
    end
    local drag = drags[id]
    if not drag then
        return
    end
    local gone = math.sqrt((x - drag.from.x) ^ 2 + (y - drag.from.y) ^ 2) >= restore_after()
    if drag.held then
        if not gone then
            if phase == "end" then
                drags[id] = nil
            end
            return
        end
        unsnap(id, drag)
        drag.held = nil
    end
    drag.zone = gone and active() and snap.zone(x, y) or nil
    if phase == "end" then
        -- Kept for the `drop` that follows; nothing is previewed past here.
        preview(nil)
    else
        preview(drag.zone)
    end
end)

-- Window `id` was let go at `x, y`: into the zone it was let go in, if any.
-- Told by `lua/floating.lua`, after the workspace it was let go on has taken
-- it, so a window snapped onto another monitor ends on the workspace that
-- monitor shows.
function snap.dropped(id)
    local drag = drags[id]
    drags[id] = nil
    if not drag or not drag.zone or not active() then
        return
    end
    local window = by_id(id)
    if not window then
        return
    end
    local zone = drag.zone
    if zone.kind == "maximize" then
        if side[id] then
            snap.mark(id, nil)
        end
        if not window.maximized then
            sol.toggle_maximize(id)
        end
        draw()
        return
    end
    if not snap.before[id] then
        snap.before[id] = drag.began or rect_of(window)
    end
    sol.animate(motion())
    place(window, zone.rect)
    snap.mark(id, zone.kind)
    if window.monitor ~= zone.monitor then
        workspaces.carry(id, zone.monitor)
    end
    draw()
end

-- The smallest a seam leaves a window on `axis`: its application's own
-- minimum, unless `floating.client_limits` says not to believe it, and never
-- less than `divider.minimum`.
local function least(window, axis, s)
    local floor = s.divider.minimum
    local limits = type(config.floating) == "table" and config.floating.client_limits
    if limits ~= "ignore" and window.min then
        floor = math.max(floor, window.min[AXES[axis].size] or 0)
    end
    return floor
end

-- Move the seam window `id`'s `edge` is on to `to`, with every window on it.
-- False when that edge is on no seam.
local function drag_seam(id, axis, edge, to)
    local snapped = on_screen()
    local start
    for _, window in ipairs(snapped) do
        if window.id == id then
            start = window
        end
    end
    if not start then
        return false
    end
    local o = AXES[axis]
    local at = start[o.at]
    if edge == "right" or edge == "bottom" then
        at = at + start[o.size]
    end
    local seam = seam_at(snapped, start, axis, at)
    if not seam then
        return false
    end
    local s = settings()
    local low, high = -math.huge, math.huge
    for _, window in ipairs(seam.before) do
        low = math.max(low, window[o.at] + least(window, axis, s))
    end
    for _, window in ipairs(seam.after) do
        high = math.min(high, window[o.at] + window[o.size] - least(window, axis, s))
    end
    to = math.floor(to + 0.5)
    if low <= high then
        to = math.min(math.max(to, low), high)
    else
        to = at
    end
    sol.animate({ duration = 0 })
    for _, window in ipairs(seam.before) do
        local rect = rect_of(window)
        rect[o.size] = to - window[o.at]
        place(window, rect)
    end
    for _, window in ipairs(seam.after) do
        local rect = rect_of(window)
        rect[o.at] = to
        rect[o.size] = window[o.at] + window[o.size] - to
        place(window, rect)
    end
    moving_seam = seam.key
    return true
end

-- An edge of a snapped window dragged: on a seam, the seam moves, with every
-- window on it, as a tiled seam does. A corner moves the seam it is on and
-- leaves the outer edge alone. An edge on no seam is the window's own: it
-- resizes as any floating window does, and is no longer in its zone, so it
-- is no longer snapped -- square corners belong to a window filling its zone.
sol.on("resize", function(id, edge_x, edge_y, horizontal_side, vertical_side)
    if not active() or not side[id] then
        return
    end
    local moved = false
    if horizontal_side then
        moved = drag_seam(id, "x", horizontal_side, edge_x) or moved
    end
    if vertical_side then
        moved = drag_seam(id, "y", vertical_side, edge_y) or moved
    end
    if not moved then
        snap.mark(id, nil)
    end
    draw()
end)

-- A snapped window maximised or made fullscreen is no longer in its zone.
local function left_zone(id, entering)
    if entering and side[id] then
        snap.mark(id, nil)
    end
    draw()
end
sol.on("maximize", left_zone)
sol.on("fullscreen", left_zone)

sol.on("close", function(id)
    side[id] = nil
    snap.before[id] = nil
    drags[id] = nil
    draw()
end)

-- What can cover or uncover a seam: a window raised, shown, hidden or moved
-- to another workspace, a workspace switched, the monitors changing. The
-- seam an edge drag moved is drawn as moved until the next of these, which
-- the press that starts any other drag is: its window is focused.
local function redraw()
    moving_seam = nil
    draw()
end
for _, event in ipairs({ "focus", "minimize", "unminimize", "monitors", "layout" }) do
    sol.on(event, redraw)
end
workspaces.applied(redraw)

-- Floating starting and stopping being in charge: a layout takes every window
-- into a tile, where none is snapped, and the scene goes with it.
modes.watch(function(name)
    if name == "floating" then
        draw()
        return
    end
    for id in pairs(side) do
        snap.mark(id, nil)
    end
    drags = {}
    previewing = nil
    sol.surface(SURFACE, false)
end)

-- A reload hands every frame its values afresh, so the marks are handed
-- again; and the scene is built now, with nothing to show, so the first
-- preview appears at once rather than after its scene has loaded.
local open = {}
for _, window in ipairs(sol.windows()) do
    open[window.id] = true
end
for id, kind in pairs(side) do
    if open[id] and modes.current() == "floating" then
        sol.frame(id, { snapped = kind })
    else
        side[id] = nil
        snap.before[id] = nil
    end
end
draw()

return snap
