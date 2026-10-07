-- Focus and move by direction (#150).
--
-- `sol.focus_direction(dir)` and `sol.move_direction(dir)` name a verb and one
-- of left, right, up or down, and that is all the compositor knows about them:
-- it hands both to the `direction` event, and this file hands them on to the
-- layout in charge as `layout.focus_direction(dir)` or
-- `layout.move_direction(dir)`. A layout that has no such function, or answers
-- false, gets the floating answer below -- which is also what a desktop with no
-- layout in charge gets. `tiling.lua` and `scrolling.lua` implement both; a
-- mode of your own can too.
--
-- The geometry is here rather than in each layout, so the layouts cannot
-- disagree about which window is to the left. `direction.find` looks on the
-- window's own monitor first and, at its edge, on the next monitor that way,
-- among the windows of the workspace that monitor is showing.
-- `sol_direction_reaches_the_listeners_and_a_bad_one_is_an_error`,
-- `a_window_the_layout_does_not_arrange_gets_the_floating_answer` and
-- `at_a_monitors_edge_tiling_crosses_to_the_desk_the_next_monitor_shows`.

local modes = require("modes")
local monitors = require("monitors")
local workspaces = require("workspaces")
local dialogs = require("dialogs")

local direction = {}

direction.opposite = { left = "right", right = "left", up = "down", down = "up" }

-- Rectangles out of Lua are `f64` through a divide or two: less than a pixel
-- apart is the same place.
local SLACK = 1

-- How far `to` lies beyond `from` towards `dir`, from the edge of one to the
-- edge of the other that faces it. Negative where the two overlap that way.
local function gap(from, to, dir)
    if dir == "left" then
        return from.x - (to.x + to.w)
    elseif dir == "right" then
        return to.x - (from.x + from.w)
    elseif dir == "up" then
        return from.y - (to.y + to.h)
    end
    return to.y - (from.y + from.h)
end

-- How far `to`'s centre is beyond `from`'s towards `dir`, and how far to one
-- side of that line it is.
local function offsets(from, to, dir)
    local dx = (to.x + to.w / 2) - (from.x + from.w / 2)
    local dy = (to.y + to.h / 2) - (from.y + from.h / 2)
    if dir == "left" then
        return -dx, math.abs(dy)
    elseif dir == "right" then
        return dx, math.abs(dy)
    elseif dir == "up" then
        return -dy, math.abs(dx)
    end
    return dy, math.abs(dx)
end

-- How much of `to` is level with `from`, across `dir`.
local function level(from, to, dir)
    if dir == "left" or dir == "right" then
        return math.min(from.y + from.h, to.y + to.h) - math.max(from.y, to.y)
    end
    return math.min(from.x + from.w, to.x + to.w) - math.max(from.x, to.x)
end

-- The rectangle in `items` nearest `from` towards `dir`, or nil.
--
-- A rectangle wholly past `from`'s edge and level with some of it is *beside*
-- it -- every neighbouring tile is -- and of those the one whose facing edge is
-- nearest wins, then the one whose centre is least to one side. That is all a
-- layout's tiles get: a tile below and to one side of a tile is not the one to
-- its side, which is what sway and Hyprland say as well.
-- `in_tiling_a_tile_off_on_a_diagonal_is_not_that_way`.
--
-- With `loose`, failing any beside it, the nearest centre past `from`'s own
-- centre that way, which is how a floating window finds one that overlaps it
-- or sits off on a diagonal.
-- `in_tiling_focus_and_move_reach_the_neighbour_in_each_direction`,
-- `in_floating_focus_and_move_reach_the_nearest_window_each_way` and, for two
-- beside it at once,
-- `at_a_monitors_edge_tiling_crosses_to_the_desk_the_next_monitor_shows`.
function direction.nearest(from, items, dir, loose)
    local best, best_kind, best_first, best_second
    for _, item in ipairs(items) do
        local along, across = offsets(from, item, dir)
        local kind, first, second
        if gap(from, item, dir) >= -SLACK and level(from, item, dir) > SLACK then
            kind, first, second = 1, gap(from, item, dir), across
        elseif loose and along > SLACK then
            kind, first, second = 2, along * along + across * across, 0
        end
        if kind and (
            not best
            or kind < best_kind
            or (kind == best_kind and first < best_first - SLACK)
            or (kind == best_kind and math.abs(first - best_first) <= SLACK
                and second < best_second)
        ) then
            best, best_kind, best_first, best_second = item, kind, first, second
        end
    end
    return best
end

-- The monitor next to the one named, towards `dir`, or nil.
--
-- Only a monitor wholly past this one's edge that way, which is wlroots' rule
-- for the output beside another: a portrait screen standing to the right of a
-- landscape one is to its right and never below it, however far down it
-- reaches, and a smaller screen top-aligned beside a bigger one is above
-- nothing. Of those, the nearest to `from`, the rectangle of the window the
-- key was pressed on, by `direction.nearest`'s loose rule: one level with the
-- window, by the nearest facing edge, and failing any the nearest centre,
-- which is a monitor off on a diagonal. From the window and not from this
-- monitor, as wlroots ranks from a point in the focused window: of two
-- screens stacked beside a big one, the one level with the window is next.
-- `at_a_monitors_edge_the_next_monitor_is_one_wholly_that_way`,
-- `at_a_monitors_edge_floating_crosses_to_the_next_monitor` and
-- `of_two_monitors_that_way_the_one_level_with_the_window_is_next`.
--
-- With no `from`, or one with nothing past its centre that way -- a window
-- whose centre is on no screen -- ranked from this monitor instead.
-- `direction_beside_with_no_window_or_one_on_no_screen_ranks_from_the_monitor`.
function direction.beside(name, dir, from)
    local here
    local all = {}
    for _, monitor in ipairs(sol.monitors()) do
        local whole = monitor.whole or monitor
        local entry = { x = whole.x, y = whole.y, w = whole.w, h = whole.h, monitor = monitor }
        if monitor.name == name then
            here = entry
        else
            all[#all + 1] = entry
        end
    end
    if not here then
        return nil
    end
    local past = {}
    for _, entry in ipairs(all) do
        if gap(here, entry, dir) >= -SLACK then
            past[#past + 1] = entry
        end
    end
    local found = direction.nearest(from or here, past, dir, true)
        or direction.nearest(here, past, dir, true)
    return found and found.monitor
end

-- The rectangle nearest `from` towards `dir`, among the ones `on(name)` lists
-- for `from.monitor`, and that monitor's name: by `direction.nearest`, `loose`
-- or not. At the monitor's edge, the nearest on the next monitor that way
-- instead, which may be none, then that monitor's name and `true`. Nil when
-- there is neither a window nor a monitor that way.
--
-- On the next monitor the search is always loose. Everything on it is past
-- this monitor's edge, so the nearest centre there is still that way, and a
-- window on a screen of another height need not be level with the one it is
-- reached from. `at_a_monitors_edge_tiling_crosses_to_the_desk_the_next_monitor_shows`
-- and `in_tiling_a_tile_off_on_a_diagonal_is_not_that_way`.
function direction.find(from, dir, on, loose)
    local here = direction.nearest(from, on(from.monitor), dir, loose)
    if here then
        return here, from.monitor, false
    end
    local next = direction.beside(from.monitor, dir, from)
    if not next then
        return nil, nil, false
    end
    return direction.nearest(from, on(next.name), dir, true), next.name, true
end

-- A point just inside `rect`, by its edge on `side`, level with the centre of
-- `from` where that is inside `rect` at all: where a window going into a
-- layout beside `rect` is let go.
-- `with_tiling_move_split_a_move_goes_into_the_neighbours_split`.
function direction.inside(rect, side, from)
    local x = math.max(rect.x + SLACK, math.min(from.x + from.w / 2, rect.x + rect.w - SLACK))
    local y = math.max(rect.y + SLACK, math.min(from.y + from.h / 2, rect.y + rect.h - SLACK))
    if side == "left" then
        return rect.x + SLACK, y
    elseif side == "right" then
        return rect.x + rect.w - SLACK, y
    elseif side == "up" then
        return x, rect.y + SLACK
    end
    return x, rect.y + rect.h - SLACK
end

-- ## With no layout in charge
--
-- Windows are where they are. Focus goes to the nearest one that way, by
-- `direction.find`'s loose search, since floating windows overlap and sit off
-- on diagonals; a move trades places with it, each window keeping its own
-- size -- the same rule tiling's swap is, so a move and the opposite move put
-- both back. At the monitor's edge a move takes the window onto the next
-- monitor, as far across it as it was across this one, and a window that
-- belongs to no workspace in particular still belongs to none.
-- `in_floating_focus_and_move_reach_the_nearest_window_each_way`,
-- `at_a_monitors_edge_floating_crosses_to_the_next_monitor` and
-- `a_window_on_no_workspace_is_on_none_after_crossing`.

local floating = {}

-- The window with the keyboard, among `windows`.
local function focused(windows)
    for _, window in ipairs(windows) do
        if window.focused then
            return window
        end
    end
    return nil
end

-- The windows in `windows` on monitor `name`, but `except` and any leaving.
local function windows_on(windows, name, except)
    local out = {}
    for _, window in ipairs(windows) do
        if window.monitor == name and window.id ~= except and not window.leaving then
            out[#out + 1] = window
        end
    end
    return out
end

-- Put `window` at `x`, `y` at its own size, kept on monitor `name`.
local function put(window, x, y, name)
    local rect = dialogs.within({ x = x, y = y, w = window.w, h = window.h }, monitors.named(name))
    sol.place(window.id, { x = rect.x, y = rect.y, w = rect.w, h = rect.h, tile = false })
end

function floating.focus(dir)
    local windows = workspaces.visible()
    local from = focused(windows)
    if not from then
        return
    end
    local to = direction.find(from, dir, function(name)
        return windows_on(windows, name, from.id)
    end, true)
    if to then
        sol.focus(to.id)
    end
end

function floating.move(dir)
    local windows = workspaces.visible()
    local from = focused(windows)
    if not from then
        return
    end
    local to, name, crossed = direction.find(from, dir, function(monitor)
        return windows_on(windows, monitor, from.id)
    end, true)
    if not name then
        return
    end
    if to and not crossed then
        put(from, to.x, to.y, to.monitor)
        put(to, from.x, from.y, from.monitor)
        return
    end
    local here = monitors.named(from.monitor)
    local there = monitors.named(name)
    local across = function(offset, was, now)
        return offset / math.max(was, 1) * now
    end
    put(
        from,
        there.x + across(from.x - here.x, here.w, there.w),
        there.y + across(from.y - here.y, here.h, there.h),
        name
    )
    workspaces.carry(from.id, name)
end

sol.on("direction", function(verb, dir)
    local layout = modes.registered[modes.current()]
    local answer = layout and layout.active and layout[verb .. "_direction"]
    if answer and answer(dir) then
        return
    end
    floating[verb](dir)
end)

-- The plain arrows: super to focus, as `direction.beside` below. A separate
-- function and not four bare `sol.bind` calls, because `lua/floating.lua`
-- takes these four over while it is the mode in charge, for snapping a
-- window like Windows (#222) -- the way `overview.lua` binds Escape only
-- while it is up -- and has to be able to give them back unchanged when it
-- lets go, rather than leaving them bound to whatever it last did with them.
-- `floating.lua` is the only caller; this file always calls it once itself,
-- below, so a configuration that never loads `floating` sees no difference.
function direction.bind_arrows()
    sol.bind("super+left", function() sol.focus_direction("left") end)
    sol.bind("super+right", function() sol.focus_direction("right") end)
    sol.bind("super+up", function() sol.focus_direction("up") end)
    sol.bind("super+down", function() sol.focus_direction("down") end)
end
direction.bind_arrows()

-- Vim's h, j, k and l, as sway and Hyprland ship them: super to focus,
-- super+shift to move. Up on k moves with super+alt, because super+shift+k
-- cycles the keyboard layout (`init.lua`). Never taken over by floating's
-- snap -- Windows has no such keys, and these are left exactly as they are.
-- `the_direction_and_window_keys_fire_while_russian_is_active`.
sol.bind("super+h", function() sol.focus_direction("left") end)
sol.bind("super+l", function() sol.focus_direction("right") end)
sol.bind("super+k", function() sol.focus_direction("up") end)
sol.bind("super+j", function() sol.focus_direction("down") end)

sol.bind("super+shift+left", function() sol.move_direction("left") end)
sol.bind("super+shift+right", function() sol.move_direction("right") end)
sol.bind("super+shift+up", function() sol.move_direction("up") end)
sol.bind("super+shift+down", function() sol.move_direction("down") end)
sol.bind("super+shift+h", function() sol.move_direction("left") end)
sol.bind("super+shift+l", function() sol.move_direction("right") end)
sol.bind("super+alt+k", function() sol.move_direction("up") end)
sol.bind("super+shift+j", function() sol.move_direction("down") end)

return direction
