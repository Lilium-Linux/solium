-- Workspaces, as a presentation.
--
-- A workspace switch does not move windows: it moves the *view*. Every window
-- keeps living exactly where its layout put it, and the desks belonging to
-- other workspaces are simply drawn a screen away. `sol.present_group` is that,
-- and the compositor animates the difference — so the slide is the same
-- machinery that overview and tiling use, and cannot drift out of step with
-- them.
--
-- **A desk is a selection, not a loop over windows.** This file used to call
-- `sol.present` once per window, which is why the wallpaper stayed behind: a
-- surface had no transform to set, so there was nothing to put in the same
-- sentence as the windows. `sol.group` names the desk — its windows *and* its
-- background — and one `sol.present_group` carries all of it, on one clock.
--
-- Three consequences worth having, all of them free:
--
--   * Nothing else needs to know. Tiling arranges the workspace in view and has
--     no idea the others exist, because their windows never move.
--   * Hit-testing follows the transform, so a window drawn off-screen is not
--     under the cursor either. A workspace you cannot see is one you cannot
--     click into by accident.
--   * The arrangement -- a row, a column, a grid -- is only a question of
--     which direction the offset runs. That is why all three are the same
--     code.
--
-- ## Per monitor, or not
--
-- `config.workspaces.per_monitor` decides whether each screen has its own
-- workspace in view. On, `super+2` switches the monitor the pointer is on and
-- leaves the other showing what it was; off, one switch moves every screen.
--
-- Both are real desktops and the difference is what you think a workspace *is*
-- -- a screenful, or a whole desk. So it is a setting rather than a decision
-- made here, and the two share every line below: which workspace a monitor
-- shows is looked up by monitor either way, and with the setting off every
-- monitor looks up the same entry.

local config = require("config")
local monitors = require("monitors")
local wallpaper = require("wallpaper")

-- The key every monitor shares when workspaces are not per monitor. A name no
-- connector can have, so it cannot collide with a real one.
local TOGETHER = "*all*"

-- What the session is, as opposed to what this file is.
--
-- **This is issue #116.** Both of these used to be plain `{}` at this file's
-- top level, and a reload runs this file again -- so `super+shift+r` on
-- workspace 3 came back believing every monitor was showing workspace 1 and
-- that no window belonged to any workspace in particular. `regroup` then swept
-- every window onto desk 1, which the *previous* session had left carried two
-- screen-widths off-stage, and the desktop was simply gone. `super+1` could
-- not bring it back, because `go` returns early when you are already on the
-- workspace asked for and this file believed you were.
--
-- `sol.keep` hands these two tables back across the reload, and it belongs to
-- the script host rather than to this file. This file had already tried to
-- answer the question by itself once -- the desk sweep at the bottom, which
-- reasoned about the *compositor's* lifetime because that was the only
-- lifetime it could see -- and got the timing wrong for exactly that reason. A
-- script cannot see the seam it is being cut at, so the mechanism lives where
-- the cut is made.
--
-- `carried` is deliberately *not* kept. It is a cache of what the compositor
-- was last told, and the compositor still knows: re-stating an offset a
-- selection is already at starts an animation from a place to itself, which
-- costs a frame and changes nothing. Keeping it would mean keeping a claim
-- about compositor state that a shorter arrangement -- the sweep below -- is
-- allowed to invalidate.
local kept = sol.keep("workspaces", {
    -- Which workspace each monitor is showing, by monitor name.
    showing = {},
    -- Which workspace each window belongs to, by window id.
    of = {},
})

local workspaces = {
    settings = config.workspaces,
    showing = kept.showing,
    of = kept.of,
    -- Where each desk was last asked to sit, by selection name. See `apply`.
    carried = {},
}

local function key(monitor)
    if not workspaces.settings.per_monitor then
        return TOGETHER
    end
    if monitor then
        return monitor
    end
    local active = monitors.active()
    return active and active.name or TOGETHER
end

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

-- The workspace a monitor is showing. Never nil: a screen nobody has switched
-- yet is showing the first one.
function workspaces.on(monitor)
    return workspaces.showing[key(monitor)] or 1
end

-- The workspace the user is looking at, which is the active monitor's.
function workspaces.current()
    return workspaces.on(nil)
end

-- Which workspace a window belongs to.
--
-- Unknown windows are on whatever *their own monitor* is showing, so a window
-- that appears while the compositor is not looking is visible where it opened
-- rather than stranded on a workspace nobody is on.
function workspaces.at(id, monitor)
    return workspaces.of[id] or workspaces.on(monitor or monitors.of(id))
end

function workspaces.on_active(id, monitor)
    monitor = monitor or monitors.of(id)
    return workspaces.at(id, monitor) == workspaces.on(monitor)
end

-- Windows on the workspace their own monitor is showing, in the order given.
function workspaces.visible(windows)
    local out = {}
    for _, window in ipairs(windows or sol.windows()) do
        if workspaces.on_active(window.id, window.monitor) then
            out[#out + 1] = window
        end
    end
    return out
end

-- One selection per desk: the windows on it, and its own background.
--
-- Named per monitor because a desk is a screenful: two screens showing
-- workspace 2 are two different desks, sitting at two different offsets and
-- made of different windows.
local function desk(monitor, index)
    return "desk-" .. index .. "@" .. monitor
end

-- How many desk names to clear when the arrangement shrinks. See the sweep at
-- the bottom of this file.
local MAX_DESKS = 16

-- Who is on which desk, and where each desk sits.
--
-- **One group per desk rather than one `sol.present` per window.** The
-- difference is not tidiness: a transform names a selection, so the desk's
-- background travels with its windows because it is *in* the selection, and one
-- animation carries the lot. `sol.present` per window could only ever move the
-- windows -- a surface had no transform at all to set.
--
-- It also stops the transform going stale. A window moved by the layout while
-- its desk is off screen used to keep an absolute rectangle worked out before
-- the move; a displacement stays right wherever the layout puts it.
function workspaces.regroup()
    local count = workspaces.count()

    -- Windows by monitor, then by desk. Built in one pass, because
    -- `workspaces.at` asks which workspace a window is on and that is a table
    -- lookup per window either way.
    local on = {}
    for _, window in ipairs(sol.windows()) do
        local mine = on[window.monitor] or {}
        on[window.monitor] = mine
        local index = workspaces.at(window.id, window.monitor)
        local list = mine[index] or {}
        mine[index] = list
        list[#list + 1] = window.id
    end

    for _, monitor in ipairs(sol.monitors()) do
        local mine = on[monitor.name] or {}
        for index = 1, count do
            local members = {
                windows = mine[index] or {},
                -- Which monitor's instance of the surfaces below. A wallpaper
                -- declared `on = "every-monitor"` is one name for several
                -- things, and a desk wants the one on its own screen.
                monitor = monitor.name,
            }
            local background = wallpaper.for_desk(index)
            if background then
                members.surfaces = { background }
            end
            sol.group(desk(monitor.name, index), members)
        end
    end
end

-- Carry every desk to where it sits relative to the one its monitor is showing.
function workspaces.apply(animation)
    local spread = workspaces.settings.spread or 1.0
    local count = workspaces.count()

    workspaces.regroup()
    sol.animate(animation or workspaces.settings.motion)

    for _, monitor in ipairs(sol.monitors()) do
        -- Its *own* monitor's size and its own monitor's workspace. A desk on a
        -- 1920 screen slid by a 2560's width lands somewhere nothing can reach
        -- and comes back to where it started only by luck.
        --
        -- A monitor *is* its work area -- `sol.monitors()` puts x, y, w and h
        -- straight on the entry and nests the whole screen under `whole` -- so
        -- this is the same measurement the per-window loop took from
        -- `sol.monitor(window.id)`, and the slide is the same length it was.
        local showing_col, showing_row = workspaces.cell(workspaces.on(monitor.name))
        for index = 1, count do
            local col, row = workspaces.cell(index)
            local dx = (col - showing_col) * monitor.w * spread
            local dy = (row - showing_row) * monitor.h * spread
            local key = desk(monitor.name, index)
            local was = workspaces.carried[key]
            -- Only when it has actually changed. `sol.present_group` starts a
            -- new animation from wherever the selection is now, so asking for
            -- the offset it is already heading to -- which is what a layout
            -- event mid-slide would do -- restarts the slide and stretches it.
            if not was or was[1] ~= dx or was[2] ~= dy then
                workspaces.carried[key] = { dx, dy }
                if dx == 0 and dy == 0 then
                    -- Back to nothing, and then nothing at all: the desk in
                    -- view is released when it lands, so the windows you are
                    -- looking at cost exactly what an ungrouped desktop costs.
                    sol.present_group_clear(key)
                else
                    sol.present_group(key, { x = dx, y = dy })
                end
            end
        end
    end

    workspaces.declare()
end

-- Switch the monitor in front of you, or every monitor when workspaces are
-- not per monitor.
--
-- `monitor` says which screen, and nil is the one in front of you, which is
-- what the bindings mean. `tiling.lua` names the screen a window opened on
-- when that window had no room and the view goes with it to another of that
-- screen's workspaces.
function workspaces.go(index, monitor)
    index = math.max(1, math.min(index, workspaces.count()))
    local held = key(monitor)
    if index == workspaces.on(monitor) then
        return
    end
    workspaces.showing[held] = index
    workspaces.apply()
    workspaces.announce()

    -- Focus follows the view. Without this the keyboard still belongs to a
    -- window nobody can see, and the next keystroke goes somewhere off-screen.
    --
    -- Only among the windows on the screen that just changed: switching the
    -- left monitor must not take focus off the right one's window if there is
    -- nothing on the left to take it.
    local switched = workspaces.settings.per_monitor and held or nil
    for _, window in ipairs(sol.windows()) do
        if (not switched or window.monitor == switched)
            and workspaces.on_active(window.id, window.monitor)
        then
            sol.focus(window.id)
            return
        end
    end
end

-- The next workspace after the one `monitor` is showing that has no window on
-- it, or nil when every one has. `except` is a window not to count: the one
-- being found a place, which is already listed and already belongs to the
-- workspace in view.
--
-- "Next" wraps round, so from the last workspace an empty one before it is
-- still found. The set is fixed -- `count()` of them, from the arrangement --
-- so there is never one to make; when all are taken the answer is nil and the
-- caller does something else.
--
-- A window being closed is not counted unless `closing_counts` says so. It is
-- fading out, and a layout that closes up at once has already given its space
-- away (#128); one that keeps the tile until the application has gone -- tiling
-- with `reflow_on_close = "when_gone"` -- has not, and passes true. See
-- `with_reflow_when_gone_a_closing_window_still_takes_its_workspace`. With
-- workspaces not per monitor a workspace is every screen at once, so a window on
-- any screen takes it; per monitor, only this screen's windows count.
--
-- A window that belongs to no workspace in particular takes all of them. `at`
-- answers for such a window with whatever its monitor is showing, so the view
-- carries it along and it is on screen whichever workspace that is -- the
-- workspace this would name included. Every window is one with
-- `follow_new_windows` off, so then nothing is empty (#134 review; see
-- `with_follow_new_windows_off_no_workspace_is_empty`).
function workspaces.vacant(monitor, except, closing_counts)
    local count = workspaces.count()
    local showing = workspaces.on(monitor)
    local taken = {}
    for _, window in ipairs(sol.windows()) do
        if window.id ~= except
            and (closing_counts or not window.leaving)
            and (not workspaces.settings.per_monitor or window.monitor == monitor)
        then
            local index = workspaces.of[window.id]
            if index == nil then
                return nil
            end
            taken[index] = true
        end
    end
    for step = 1, count - 1 do
        local index = (showing - 1 + step) % count + 1
        if not taken[index] then
            return index
        end
    end
    return nil
end

-- Step through the arrangement. Directions that the arrangement has no room
-- for do nothing, so the same bindings work for a row, a column and a grid.
function workspaces.step(dx, dy)
    local settings = workspaces.settings
    local col, row = workspaces.cell(workspaces.on(nil))
    if settings.arrangement == "horizontal" then
        return workspaces.go(workspaces.on(nil) + dx)
    elseif settings.arrangement == "vertical" then
        return workspaces.go(workspaces.on(nil) + dy)
    end
    local columns = math.max(1, settings.columns)
    local rows = math.max(1, settings.rows)
    col = math.max(1, math.min(col + dx, columns))
    row = math.max(1, math.min(row + dy, rows))
    workspaces.go((row - 1) * columns + col)
end

-- Send the focused window to another workspace. It stays on its own monitor:
-- a workspace is a screenful, and sending a window sideways through the
-- workspaces should not also throw it at the other screen.
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
    local index = workspaces.on(nil)
    -- Which monitor, but only when saying so means anything: with one screen,
    -- or with workspaces switching together, naming it is noise.
    local where = ""
    if settings.per_monitor and #sol.monitors() > 1 then
        local active = monitors.active()
        where = active and (" on " .. active.name) or ""
    end
    if settings.arrangement == "grid" then
        local col, row = workspaces.cell(index)
        sol.status(string.format("workspace %d,%d%s", col, row, where))
    else
        sol.status(string.format("workspace %d%s", index, where))
    end
end

-- What the workspaces are, said to the compositor, which has no idea
-- (03 §3.2.17): one group per monitor when they are per monitor, one for
-- every monitor when they are not, what each shows, and which workspace each
-- window is on. This is how a hosted shell's `Workspaces` knows them. Rust
-- diffs what it publishes, so saying it again unchanged costs no scene
-- anything. See `the_shipped_workspaces_declare_what_each_monitor_shows`,
-- `with_workspaces_together_one_group_has_every_monitor` and
-- `a_batch_qt_cannot_take_is_sent_again_once_it_can`.
--
-- The arrangement is the shape the workspaces really make, which is where
-- the last one sits: a row of four is four by one, whatever `rows` says.
-- See `the_declared_arrangement_is_the_shape_of_its_workspaces`.
--
-- `moved` is the monitor each window a handler has just moved is going to,
-- by window id, for the windows `sol.windows()` cannot show there yet: it is
-- the snapshot from before the handler ran. See `carry` and
-- `a_window_on_no_workspace_moved_across_is_declared_on_what_that_monitor_shows`.
function workspaces.declare(moved)
    local count = workspaces.count()
    local columns, rows = workspaces.cell(count)
    local function group(id, names)
        local list = {}
        for index = 1, count do
            local col, row = workspaces.cell(index)
            list[#list + 1] = { id = tostring(index), name = tostring(index), col = col, row = row }
        end
        return { id = id, monitors = names, showing = { tostring(workspaces.on(names[1])) }, workspaces = list }
    end
    local groups = {}
    if workspaces.settings.per_monitor then
        for _, monitor in ipairs(sol.monitors()) do
            groups[#groups + 1] = group(monitor.name, { monitor.name })
        end
    else
        local names = {}
        for _, monitor in ipairs(sol.monitors()) do
            names[#names + 1] = monitor.name
        end
        if #names > 0 then
            groups[1] = group(TOGETHER, names)
        end
    end
    local windows = {}
    for _, window in ipairs(sol.windows()) do
        local monitor = moved and moved[window.id] or window.monitor
        windows[window.id] = { tostring(workspaces.at(window.id, monitor)) }
    end
    sol.workspaces({
        arrangement = {
            kind = workspaces.settings.arrangement,
            columns = columns,
            rows = rows,
        },
        groups = groups,
        windows = windows,
    })
end

-- A window a layout moved onto another monitor goes to the workspace that
-- monitor shows, if it belonged to one, and the compositor is told at once:
-- no `layout` follows a move by key. One that belongs to no workspace in
-- particular is on whatever its new monitor shows, and is declared there
-- too. See
-- `a_window_moved_onto_the_other_monitor_is_declared_on_the_workspace_it_shows`
-- and `a_window_on_no_workspace_moved_across_is_declared_on_what_that_monitor_shows`.
function workspaces.carry(id, monitor)
    if workspaces.of[id] ~= nil then
        workspaces.of[id] = workspaces.on(monitor)
    end
    workspaces.declare({ [id] = monitor })
end

-- Send one window to a workspace on its own monitor: `windows.send` from a
-- scene, which names the window rather than meaning the focused one. See
-- `a_windows_send_from_a_scene_moves_the_window_it_names`.
function workspaces.send_window(id, index)
    index = math.max(1, math.min(index, workspaces.count()))
    workspaces.of[id] = index
    workspaces.apply()
    workspaces.announce()
end

-- Whether the compositor has window `id` now: `windows.send` for one it has
-- not is logged, not kept. See
-- `a_windows_send_names_its_window_by_number_or_digits_and_logs_any_other_form`.
local function is_open(id)
    for _, window in ipairs(sol.windows()) do
        if window.id == id then
            return true
        end
    end
    return false
end

-- A scene asks for these by name (`Solium.send("workspaces.go", { id: "2",
-- monitor: Solium.monitor.name })`), and only this file knows what they
-- mean. See `a_workspaces_go_from_a_scene_switches_the_monitor_it_names` and
-- `a_windows_send_from_a_scene_moves_the_window_it_names`.
--
-- Answered only where `actions.lua` routes a scene's actions, which
-- `init.lua` requires before this file: a configuration that took it out
-- routes them its own way, and requiring it here would route each one a
-- second time. See `workspaces_lua_does_not_route_a_scenes_actions_by_itself`.
--
-- A workspace is a whole number or its digits: `2.5` would slide the desks
-- half a screen, or put a window on no desk at all, so it is logged like any
-- other form and does nothing. See
-- `a_workspaces_go_from_a_scene_switches_the_monitor_it_names` and
-- `a_windows_send_names_its_window_by_number_or_digits_and_logs_any_other_form`.
local actions = package.loaded["actions"]
if actions then
    actions.override("workspaces.go", function(data)
        local index = type(data) == "table" and math.tointeger(tonumber(data.id))
        if index then
            workspaces.go(index, data.monitor)
        else
            sol.log("workspaces.go: answered only as { id, monitor }, the id a workspace's number or its digits")
        end
    end)
    -- `{ id, workspace }`, the id a number or its digits, for a window that
    -- is open. Anything else, `{ id, monitor }` among it, is logged and does
    -- nothing. See
    -- `a_windows_send_names_its_window_by_number_or_digits_and_logs_any_other_form`.
    actions.override("windows.send", function(data)
        local id = type(data) == "table" and tonumber(data.id)
        local index = id and math.tointeger(tonumber(data.workspace))
        if index and is_open(id) then
            workspaces.send_window(id, index)
        else
            sol.log("windows.send: answered only as { id, workspace } for a window that is open")
        end
    end)
end

-- A new window belongs to the workspace its own monitor is showing, and is
-- declared there at once.
--
-- **`"open"` is the only event some windows ever see.** `"layout"`, which the
-- handler below also declares from, fires only while a layout is arranging
-- something -- so a *floating* window (`modes.lua`'s own default, with no
-- layout registered at all) never gets one. Without the `declare()` here, a
-- floating window's row never carries a `workspace`, forever rather than for
-- one frame, because nothing else ever asks: `workspaces.of` (and
-- `workspaces.at`'s fallback) already had the right answer, live, which is
-- why `search.lua`'s own query of it was never wrong -- only a hosted shell
-- reading the *published* `Workspaces`/`WindowList` was, since publishing is
-- this call's job and nobody else's.
-- `a_window_that_opens_while_floating_is_declared_with_no_relayout_to_ask_for_it`.
sol.on("open", function(id)
    if workspaces.settings.follow_new_windows then
        workspaces.of[id] = workspaces.on(monitors.of(id))
    end
    workspaces.declare()
end)

-- An application you launched was already running, and answered by bringing
-- forward the window it had: Firefox, Telegram, anything that keeps one
-- instance (#177). The window stays on its own workspace, so the view goes to
-- it there, on its own monitor, and it takes the keyboard -- what a dock does
-- for an application that is running. The compositor will not focus a window
-- on a workspace nobody is looking at, so without this the launch would show
-- nothing at all. See `relaunching_an_application_whose_window_is_on_another_workspace_keeps_it_there_and_brings_it_into_view`
-- and `relaunching_an_application_whose_window_is_on_the_other_monitors_hidden_workspace_switches_that_monitor`.
--
-- Not for any other request: any client can make itself a token, so a view
-- that followed every one would be one any application could pull away.
-- `a_genuine_activation_is_told_as_a_request_and_the_view_stays_where_it_is`.
--
-- Nor for a window being closed: the compositor refused it the keyboard, and
-- the view going to it would show a window fading out and hand the keyboard to
-- it. `relaunching_an_application_whose_window_is_being_closed_leaves_the_view_and_the_keyboard_where_they_were`.
sol.on("activate", function(id, why)
    if why ~= "launch" then
        return
    end
    for _, window in ipairs(sol.windows()) do
        if window.id == id then
            if window.leaving then
                return
            end
            workspaces.go(workspaces.at(id, window.monitor), window.monitor)
            sol.focus(id)
            return
        end
    end
end)

-- The desks are per monitor, so a screen arriving or leaving is a different set
-- of them. This is also the first moment there are any monitors to build them
-- from: a script's top level runs before the compositor has placed a single
-- output, so declaring them there would declare nothing.
--
-- And, since the reload contract puts this event after `restore`, it is also
-- what carries every desk back to where the restored `showing` says it sits.
sol.on("monitors", function()
    workspaces.apply()
end)

-- A shorter arrangement than the last configuration had leaves desks behind.
--
-- A selection is the compositor's until something takes the name away: it
-- outlives `super+shift+r` exactly as a surface and a window's transform do.
-- So a reload that goes from eight workspaces to four would otherwise leave
-- four selections still carrying windows a screen away, with nothing left that
-- names them and so no way to carry them back.
--
-- This file used to do the sweep on the first `regroup` of a session, behind a
-- `swept` flag, with a comment claiming that was "the only moment the count can
-- have changed". It was not: the count changes when the configuration is
-- edited, which is the reload, which is *this* moment. The flag existed because
-- there was no event meaning "the scripts were replaced" to hang it on -- which
-- is the same gap `sol.keep` fills at the top of this file, and this is the
-- other half of it. A cold start sweeps nothing because there is nothing there
-- to sweep, and that is why `restore` does not fire at startup.
--
-- ## And nothing may still be pointing at one of them
--
-- Forgetting the abandoned desks is only half the sweep, and shipping the
-- first half alone made a *new* way to lose a window -- one the bug above did
-- not have, because before `sol.keep` both of these tables were wiped by every
-- reload. `columns = 4` edited to `columns = 2` left:
--
--   * a window whose `of` said 4 in no selection at all. `regroup` loops
--     `1..count`, so nothing names it; it is drawn over whatever is in view, on
--     top of the windows that belong there; `visible()` filters it out, so no
--     layout arranges it; and `go` clamps to `count()`, so no key switches to
--     where it thinks it is.
--   * every surviving desk carried off-stage, when `showing` was the one that
--     pointed past the end. The offsets below are worked out relative to the
--     cell the monitor is showing, so a monitor showing workspace 4 puts desks
--     1 and 2 three and two screens to its left and nothing on screen at all --
--     which is the reported fault again, reached by the other door.
--
-- Clamped rather than cleared: a window kept on the last workspace is a window
-- you can walk to, where one reset to workspace 1 has quietly moved.
--
-- A window whose *monitor* vanished needs nothing here. `of` holds a workspace
-- and not a screen, so it stays a valid index; `regroup` builds desks only for
-- the monitors that exist, and the window is grouped on whichever screen it now
-- reports. That is the difference between the two tables: `showing` is keyed by
-- monitor and may hold entries for screens that are gone, which cost nothing
-- because every loop here is over `sol.monitors()`.
--
-- Assigning to a key `pairs` has already produced is defined; only adding one
-- is not, and this only ever lowers a value that is there.
local function clamp(held, count)
    for key, index in pairs(held) do
        if index > count then
            held[key] = count
        end
    end
end

sol.on("restore", function()
    local count = workspaces.count()
    for _, monitor in ipairs(sol.monitors()) do
        for index = count + 1, MAX_DESKS do
            sol.group(desk(monitor.name, index), false)
        end
    end
    clamp(workspaces.of, count)
    clamp(workspaces.showing, count)
end)

-- And membership follows the windows. Only the membership -- no `sol.animate`
-- and no transform, so this cannot disturb whichever layout is also listening
-- for this event. A selection whose members have not changed is not a change at
-- all and costs nothing on the other side. So does what the compositor is told
-- of each window's workspace: `a_window_that_opens_is_declared_on_the_workspace_its_monitor_shows`.
sol.on("layout", function()
    workspaces.regroup()
    workspaces.declare()
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
