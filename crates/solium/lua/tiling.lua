-- Tiling: dwindle, as Hyprland does it.
--
-- Not master-and-stack, and not a formula. A new window splits *a particular
-- existing window* — the one under the pointer — and the split runs across
-- whichever axis that window's own box is longer on. Which window you were
-- pointing at when you opened a terminal changes the result, and no function
-- of "how many windows are there" can recover that afterwards.
--
-- So the arrangement is a tree, and the tree lives here, in the script that
-- owns the layout. `sol.layout.tree()` builds one; the compositor holds no
-- opinion about tiling at all.
--
-- Reimplemented from Hyprland's `CDwindleAlgorithm::addTarget`
-- (src/layout/algorithm/tiled/dwindle/, BSD-3-Clause), not copied: that is
-- C++ against its own window types. The behaviour is the specification.

local config = require("config")
local workspaces = require("workspaces")
local modes = require("modes")
local monitors = require("monitors")
local dialogs = require("dialogs")
local sizes = require("sizes")
local direction = require("direction")

-- `exiled` is the ids this layout has taken out of its trees because they are
-- modal dialogs, and it is what makes `unset_modal` reversible: only a window
-- that was taken out is ever put back. See `dialogs.settle` for why it is not
-- simply "re-admit whatever is missing".
--
-- `leaving` is where each window that is being closed stood when it was taken
-- out of its tree, by id: the centre of its rectangle and the key of the tree
-- it was in. It is what a refused close puts the window back by. See the
-- `closing` and `refused` handlers.
--
-- `trees` is the arrangement, one tree per desk (see `tree_for`), and
-- `sol.keep` holds it so that it outlives `super+shift+r`: a reload is a change
-- of configuration, not of arrangement. A tree is userdata, and the host carries
-- it across whole, split ratios included. Before it did, the trees died with
-- the Lua state and `adopt` built new ones from `sol.windows()`, which lists the
-- windows topmost first -- so the window in use went in first and took the
-- screen, and windows not stacked in the order they were opened in traded
-- places (#118). See
-- `a_reload_leaves_a_tiled_desk_exactly_as_it_was` and
-- `firefox_and_kitty_on_workspace_3_keep_their_places_through_a_reload` in
-- `script.rs`.
--
-- `cramped` is the windows this layout has said are cramped -- laid out
-- smaller than their own minimum (#115) -- and have been named in the log for,
-- by id. See `say_cramped`. Kept beside the trees, so a reload does not name
-- them again: they are the same windows in the same tiles, and the log has
-- said so already. See `a_reload_does_not_name_a_cramped_window_again` in
-- `script.rs`.
--
-- `unsized` is the windows `open` placed before their application had said
-- anything -- every window launched with `sol.spawn`, whose `open` comes at
-- the launch, before its application exists -- by id, with the split target
-- and the pointer that `open` decided from. See `reconsider`.
local kept = sol.keep("tiling", { trees = {}, cramped = {} })
kept.cramped = kept.cramped or {}
local tiling = {
    active = false,
    trees = kept.trees,
    exiled = {},
    leaving = {},
    cramped = kept.cramped,
    unsized = {},
}

-- Whether the other windows close up the moment a close is asked for, rather
-- than once the application has gone. Read when each event arrives rather than
-- captured when this file loads, so a script that changes the setting while the
-- session runs changes the next close. See `reflow_on_close` in config.lua;
-- anything but "when_gone" is the default.
local function reflows_at_once()
    return config.tiling.reflow_on_close ~= "when_gone"
end

-- A line in the log the first time it is said, and never again this session.
--
-- For a setting that is wrong: it is read on every event, and a line per event
-- would bury the log under one mistake. Cleared by a reload, which is also how
-- a correction is applied -- so a value that is still wrong afterwards is said
-- once more.
local told = {}
local function once(message)
    if not told[message] then
        told[message] = true
        sol.log(message)
    end
end

-- The smallest a tile may be, frame included: `config.tiling.minimum`, as the
-- layout is handed it.
--
-- A side that is not a size in pixels -- a string, a negative, `0/0` -- is no
-- minimum on that side, and is named in the log once. Absent altogether is no
-- minimum at all, which is how tiling behaved before #134, and says nothing: a
-- configuration written then has no reason to know the setting exists. See
-- `a_minimum_that_is_not_a_size_is_named_once_and_ignored`.
local function minimum()
    local given = config.tiling.minimum
    local out = { w = 0, h = 0 }
    if given == nil then
        return out
    end
    if type(given) ~= "table" then
        once("tiling.minimum is not a table like { w = 160, h = 96 }; tiles have no minimum")
        return out
    end
    for _, side in ipairs({ "w", "h" }) do
        local value = given[side]
        -- NaN fails `>= 0`, and `math.huge` is no tile anybody can have.
        if type(value) == "number" and value >= 0 and value < math.huge then
            out[side] = value
        elseif value ~= nil then
            once(string.format(
                "tiling.minimum.%s is %s, which is not a size in pixels; no minimum on that side",
                side,
                tostring(value)
            ))
        end
    end
    return out
end

-- Where a window being opened goes when the tile under the pointer has no room
-- for it, either way: `config.tiling.overflow`, the steps in order.
--
-- An entry that is not a step is skipped and named once. Absent altogether is
-- the shipped list, for the same reason an absent `minimum` is none: an older
-- configuration says nothing about it because it could not. What happens when
-- the steps run out is `open_in`'s business, not this.
local STEPS = { largest = true, workspace = true, allow = true }
local function overflow()
    local given = config.tiling.overflow
    if given == nil then
        return { "largest", "workspace", "allow" }
    end
    if type(given) ~= "table" then
        once("tiling.overflow is not a list of steps; a window with no room splits the tile "
            .. "under the pointer anyway")
        return {}
    end
    local out = {}
    for _, step in ipairs(given) do
        if STEPS[step] then
            out[#out + 1] = step
        else
            once(string.format(
                "tiling.overflow names %s, which is not a step (largest, workspace or allow); "
                    .. "it is skipped",
                tostring(step)
            ))
        end
    end
    return out
end

-- Whether the view goes with a window that opened on another workspace. Read
-- at each open, like `reflow_on_close`; anything but `false` is the default.
local function follows_overflow()
    return config.tiling.follow_overflow ~= false
end

-- One tree per workspace *per monitor*.
--
-- Per workspace because a window closing on workspace 2 must not disturb the
-- arrangement on workspace 1. Per monitor for the same reason twice over: the
-- screens are different sizes, the split that reads well on one is wrong on
-- the other, and a window moved across has to leave one arrangement and join
-- another rather than being in both.
-- Takes only the monitor: which workspace that screen is showing is a thing
-- `workspaces` knows and nothing here should be passing around. Threading the
-- index through every call site is how one of them ends up asking for the
-- wrong screen's workspace.
local function tree_for(monitor)
    local key = monitors.key(workspaces.on(monitor), monitor)
    if not tiling.trees[key] then
        tiling.trees[key] = sol.layout.tree()
    end
    return tiling.trees[key]
end

-- The area a tree divides, which is one monitor's work area.
local function options(monitor)
    local area = monitor and monitors.named(monitor) or sol.monitor()
    -- A copy: the monitor table is the snapshot's, and the layout adds keys.
    local out = { x = area.x, y = area.y, w = area.w, h = area.h }
    out.gap = config.gap
    out.split = config.tiling.split
    -- In every options table, so the seams are held to it as well as the
    -- splits: `tree:drag_seam` and `tree:resize` read it from here.
    out.minimum = minimum()
    -- Each window's own minimum (#115), under `tiling.client_minimum` and
    -- `tiling.client_size_ignore`: what the tree lays each window out around,
    -- splits and seams included. See `sizes.lua`.
    out.floors = sizes.floors()
    return out
end

-- Say once that a window is cramped, with the numbers, and forget it once it
-- is not.
--
-- Once and not on every pass: `apply` runs once a frame for the length of a
-- seam drag, and a window that stays cramped through one has been named
-- already. A window that stops being cramped and then is again is named again,
-- because that is news. See
-- `a_window_that_cannot_have_its_minimum_is_cramped_and_said_once` in
-- `script.rs`.
local function say_cramped(slot, window)
    if not slot.cramped then
        tiling.cramped[slot.id] = nil
        return
    end
    if tiling.cramped[slot.id] then
        return
    end
    tiling.cramped[slot.id] = true
    local floor = (window and sizes.floor(window)) or { w = 0, h = 0 }
    local needs
    if floor.w > 0 and floor.h > 0 then
        needs = string.format("%.0fx%.0f", floor.w, floor.h)
    elseif floor.w > 0 then
        needs = string.format("%.0f wide", floor.w)
    else
        needs = string.format("%.0f high", floor.h)
    end
    sol.log(string.format(
        "tiling: window %d needs at least %s, frame included, and its tile is %.0fx%.0f; "
            .. "it is cramped, and its application's picture is cut to the tile",
        slot.id,
        needs,
        slot.w,
        slot.h
    ))
end

-- Place one window the tree laid out, and hand back the rectangle it is at.
--
-- The tile itself, unless the window's own maximum is smaller than it and
-- `tiling.client_maximum` is "center": then a pane of that size in the middle
-- of the tile, with the tile handed over as `tile` so the compositor holds the
-- client inside the tile and a dragged edge moves the tile's seam (#115). And
-- `cramped`, which the tree put on the slot, goes with it either way.
local function place(slot, window)
    say_cramped(slot, window)
    local pane = sizes.centred(window, slot)
    if not pane then
        sol.place(slot.id, slot)
        return slot
    end
    pane.tile = { x = slot.x, y = slot.y, w = slot.w, h = slot.h }
    pane.cramped = slot.cramped
    sol.place(slot.id, pane)
    return pane
end

-- The windows `sol.windows()` lists, by id.
local function by_id(windows)
    local out = {}
    for _, window in ipairs(windows or sol.windows()) do
        out[window.id] = window
    end
    return out
end

-- A window that is already open, going back into `tree`.
--
-- Tiling switched on, a reload, a monitor change (all three `adopt`), a dialog
-- that stops being modal: none of these knows where the window should be, so
-- it is found a tile -- the one `insert` would choose from `target`, `x` and
-- `y`, either way; then the largest with room; then the first one anyway,
-- below the minimum.
--
-- A close the application refused, and a drop, *do* know, and say so with
-- `stays`: where the window stood, where it was let go. The window goes back
-- to the tile there, either way, and below the minimum if that is what it
-- takes -- not to the largest tile, which is somewhere else. A refused window
-- was in that tile a second ago, and an arrangement that had not otherwise
-- changed has to come back as it was, including a tile `"allow"` made under
-- the minimum; see `a_refused_window_under_the_minimum_comes_back_where_it_was`
-- and `a_dropped_window_goes_to_the_tile_it_was_let_go_over`.
--
-- **Never another workspace, whatever `config.tiling.overflow` says.** Overflow
-- is for a window being opened, which has no place yet. A window coming back
-- has one: sending it off to the next empty workspace would scatter a desktop
-- across the workspaces on every reload, and a window whose close was refused
-- would come back somewhere else entirely. See
-- `nothing_already_open_is_sent_to_another_workspace` in `script.rs`.
local function rejoin(tree, id, target, x, y, area, stays)
    if tree:insert_fitting(id, target, x, y, area) then
        return
    end
    if not stays and tree:insert_largest(id, area) then
        return
    end
    tree:insert(id, target, x, y, area)
end

-- A modal dialog is in no tree, and one that stops being modal rejoins.
--
-- Run on every apply rather than only when a dialog appears, because that is
-- the cheapest way to make the invariant true rather than hoped for: the
-- compositor re-runs the layout on `set_modal`, `unset_modal` and
-- `set_parent`, so this is on the path for every one of them, and a tree that
-- somehow acquired a dialog loses it on the next pass instead of keeping it
-- until the mode is toggled.
local function settle_dialogs(windows)
    dialogs.settle(
        tiling.exiled,
        windows,
        function(id)
            -- Every tree, not just this window's monitor's: a window that was
            -- dragged to the other screen and then went modal is still in the
            -- tree it left, and one window in two trees gets two slots.
            for _, tree in pairs(tiling.trees) do
                tree:remove(id)
            end
        end,
        function(id)
            local monitor = monitors.of(id)
            local tree = tree_for(monitor)
            if not tree:contains(id) then
                -- No split target and no pointer position, unlike `open`: the
                -- pointer is wherever it is now, which has nothing to do with
                -- a dialog the user has just dismissed. `adopt` inserts the
                -- same way and for the same reason.
                rejoin(tree, id, nil, nil, nil, options(monitor))
            end
        end
    )
end

function tiling.apply(animation)
    if not tiling.active then
        return
    end

    local visible = workspaces.visible()
    settle_dialogs(visible)

    sol.animate(animation or config.tiling.motion)
    -- Every monitor, each against its own area. One `sol.animate` for the lot,
    -- because two screens rearranging at once is one movement -- see
    -- docs/animation.md on why the feel is set per batch.
    local screens = monitors.each(visible)
    -- What the trees decided, by window id. Collected because a dialog is
    -- centred on its parent and its parent is very likely one of these: the
    -- snapshot says where that window *was*, and this pass is in the middle of
    -- moving it.
    local placed = {}
    local windows = by_id()
    for _, each in ipairs(screens) do
        local tree = tree_for(each.monitor.name)
        for _, slot in ipairs(tree:layout(options(each.monitor.name))) do
            placed[slot.id] = place(slot, windows[slot.id])
        end
    end
    -- A second pass rather than the tail of the first: a dialog on DP-1 may
    -- belong to a window on DP-2, and centring it needs that window's new slot
    -- rather than its old one. Dialogs are in no tree, so this is the only
    -- thing that places them -- without it a Wayland toplevel stays in the
    -- top-left corner it was mapped at.
    --
    -- One pass for every screen's dialogs, not one per screen, and `options`
    -- itself rather than this screen's: a dialog goes onto its *parent's*
    -- monitor, which the loop above has no way to name. Handing it one screen's
    -- area is what pinned a cross-screen dialog to the wrong edge.
    dialogs.place(visible, options, placed)
end

-- Bring every desk's tree in line with the windows on that desk: a window
-- missing from its own desk's tree goes in, and a tree lets go of a window that
-- is not on its desk. Run when tiling is switched on, on `monitors`, and on
-- `restore` through `modes.lua`.
--
-- **Every desk, not just the ones in view.** `monitors` is announced on every
-- reload as well as on every hotplug, and this used to measure every tree
-- against the windows on the desks in view -- so each hidden desk's tree was
-- emptied while its windows stayed in their tiles, and back on that desk the
-- next window opened took the whole screen, over the top of them (#129). See
-- `a_hidden_desk_keeps_its_arrangement_through_a_reload_and_a_monitors_event`
-- in `script.rs`.
function tiling.adopt()
    -- Also how a window that moved between monitors settles: it is missing
    -- from its new screen's tree and still in its old one's, and both halves
    -- are fixed here. See `a_monitor_that_goes_away_still_hands_its_windows_on`.
    local present = {}
    for _, each in ipairs(monitors.each(sol.windows())) do
        local name = each.monitor.name
        for _, window in ipairs(each.windows) do
            -- A window being closed is, to a layout that reflows at once,
            -- already gone: `closing` took it out of its tree, and putting it
            -- back is `refused`'s decision and nobody else's. So it is neither
            -- inserted nor counted as present -- the sweep below takes it out
            -- of any tree that still holds it, which is what makes a missed
            -- `closing` recoverable here like any other missed event. Waiting
            -- for the client instead, it is an ordinary window until `close`.
            if window.leaving and reflows_at_once() then
                -- Nothing: see above.
            -- A dialog is deliberately absent from every tree, so `adopt` --
            -- whose whole job is to put back whatever is missing -- has to be
            -- told that this one is missing on purpose. It is left out of
            -- `present` too, so the sweep below does not go looking for it in
            -- a tree it was never in.
            elseif not dialogs.floats(window) then
                -- Its own workspace's tree on the screen it is on, which is
                -- the tree of the desk in view only when that is where it is.
                local key = monitors.key(workspaces.at(window.id, name), name)
                present[window.id] = key
                local tree = tiling.trees[key] or sol.layout.tree()
                tiling.trees[key] = tree
                if not tree:contains(window.id) then
                    -- `rejoin`, never `open_in`: these windows are open
                    -- already, and a reload must not scatter them across the
                    -- workspaces.
                    rejoin(tree, window.id, nil, nil, nil, options(name))
                end
            end
        end
    end
    for key, tree in pairs(tiling.trees) do
        for _, id in ipairs(tree:windows()) do
            -- Removed when the window is gone, and when it is on another
            -- monitor now: one window in two trees is one window given two
            -- slots, and it ends up in whichever was laid out last.
            if present[id] ~= key then
                tree:remove(id)
            end
        end
    end
end

function tiling.started()
    tiling.adopt()
    tiling.apply()
end

function tiling.toggle()
    modes.use("tiling")
end

modes.register("tiling", tiling)

-- A new window splits whatever the pointer is over. This is the whole of
-- "the window opens where the cursor is".
-- The compositor changed how much room windows get -- a decoration that
-- reserves a different amount, most likely. The slots are unchanged; what
-- fits inside them is not, so the arithmetic is redone.
-- A monitor arrived or went away.
--
-- `apply` walks the monitors that exist, so a window on one that has gone is
-- in a tree nothing iterates: it keeps a slot that is now off every
-- screen, and it comes back on that screen when the monitor does. `adopt` is
-- already the function that re-homes a window whose monitor changed -- it was
-- only ever called when the mode was switched on.
sol.on("monitors", function()
    tiling.adopt()
end)

-- Where a window being opened goes (#134): the tile under the pointer, either
-- way, and when that has no room, `config.tiling.overflow` one step at a time.
-- Returns the workspace it was sent to, or nil when it went into the tree of
-- the one in view.
--
-- Decided here, at `open`. For a window launched with `sol.spawn` that is the
-- moment it is asked for, before its application has connected
-- (`Solium::begin_loading`), so a window that goes to another workspace is
-- placed there in the same dispatch that opens it -- the one tile it is ever
-- given -- rather than in this workspace's first and moved later. A launched
-- window left in this workspace is decided here once more, if its
-- application's own minimum turns out not to fit the tile it was given: see
-- `reconsider`.
local function open_in(id, monitor, target, cursor)
    local tree = tree_for(monitor)
    local area = options(monitor)
    if tree:insert_fitting(id, target, cursor.x, cursor.y, area) then
        return nil
    end
    local from = workspaces.on(monitor)
    for _, step in ipairs(overflow()) do
        if step == "largest" then
            if tree:insert_largest(id, area) then
                return nil
            end
        elseif step == "workspace" then
            -- A window being closed keeps its workspace when it keeps its
            -- tile: with "when_gone" its leaf is still in that workspace's
            -- tree, which the fresh tree below would throw away while the
            -- window is still drawn there.
            local index = workspaces.vacant(monitor, id, not reflows_at_once())
            if index then
                workspaces.of[id] = index
                -- A new tree rather than the one that may be there. Nothing is
                -- on that workspace, but its tree can still hold a window that
                -- was on it: `workspaces.send` moves a window without telling
                -- any tree, and only the next `adopt` sweeps the leaf it left.
                -- Split, that leaf would keep half the screen for a window
                -- that is not there. See
                -- `a_workspace_left_empty_by_a_send_is_given_whole`.
                local fresh = sol.layout.tree()
                tiling.trees[monitors.key(index, monitor)] = fresh
                fresh:insert(id, nil, nil, nil, area)
                sol.log(string.format(
                    "tiling: no room for window %d on workspace %d; it opens on workspace %d",
                    id,
                    from,
                    index
                ))
                return index
            end
        else
            tree:insert(id, target, cursor.x, cursor.y, area)
            return nil
        end
    end
    -- A window has to go somewhere, so a list that ran out without an "allow"
    -- ends in one all the same -- said, so that a list which was meant to
    -- keep windows at the minimum can be seen not to have.
    sol.log(string.format(
        "tiling: no room for window %d on workspace %d and no step of tiling.overflow "
            .. "placed it; it splits the tile under the pointer, below tiling.minimum",
        id,
        from
    ))
    tree:insert(id, target, cursor.x, cursor.y, area)
    return nil
end

-- Open window `id` where `open_in` says, and see to what goes with it: the
-- view going with a window sent to another workspace, or, where it does not,
-- the window placed on that workspace's desk. `open`'s, and `reconsider`'s for
-- a window placed again.
local function settle_open(id, monitor, target, cursor)
    local elsewhere = open_in(id, monitor, target, cursor)
    local follows = follows_overflow()
    if elsewhere and follows then
        -- The view goes with it, as `super+<n>` would take it. Focused by name
        -- afterwards as well: `go` hands the keyboard to the first window it
        -- finds on the workspace, which can be one still fading out there
        -- (`a_window_being_closed_is_room_for_the_next_one`).
        --
        -- At the `open` of a window launched with `sol.spawn` the compositor
        -- drops that request, because that `open` comes before the
        -- application exists and there is nothing yet to give the keyboard
        -- to. The keyboard goes to this window with the application's first
        -- frame; until then it is not on it, and it can be on the window the
        -- view has just left, as `super+<n>` onto an empty workspace leaves
        -- it. Both halves are
        -- `a_launched_window_that_overflows_with_the_view_takes_the_keyboard_when_it_arrives`
        -- in `state.rs`.
        workspaces.go(elsewhere, monitor)
        sol.focus(id)
    elseif elsewhere then
        -- Regrouped now, so the window joins its own desk's selection -- the
        -- one carried a screen away -- in the same dispatch that places it,
        -- rather than whenever something next regroups.
        --
        -- The keyboard is not this file's to move, and stays where it was:
        -- the compositor gives a new window the keyboard only if it is headed
        -- somewhere the user can see (`Solium::offer_keyboard`). See
        -- `a_window_that_overflows_to_a_hidden_workspace_does_not_take_the_keyboard`
        -- and its launched twin in `state.rs`.
        workspaces.apply()
    end
    tiling.apply()
    if elsewhere and not follows then
        -- Its tree is not one `apply` walks, since that workspace is not in
        -- view. Placed here so it is already in its tile when the user goes
        -- there, and so that until then it is where its desk carries it.
        local key = monitors.key(elsewhere, monitor)
        local windows = by_id()
        for _, slot in ipairs(tiling.trees[key]:layout(options(monitor))) do
            place(slot, windows[slot.id])
        end
    end
end

sol.on("open", function(id)
    -- A dialog joins no tree. `apply` places it over its parent and records it
    -- as exiled, so there is nothing to do here but let that happen.
    if dialogs.floating(id) then
        tiling.apply()
        return
    end
    local monitor = monitors.of(id)
    local cursor = sol.cursor()
    -- Skip the window being opened: it is already mapped and under the
    -- pointer, so asking without skipping names it as its own split target.
    local target = sol.window_at(cursor.x, cursor.y, id)

    -- Every layout hears `open` whether or not it is in charge, and keeps the
    -- window in its own structure for when it is. One that is not in charge
    -- sends nobody anywhere: the workspace step moves the view and puts the
    -- window on another desk, and a floating desktop whose windows went off to
    -- other workspaces because a tree nobody is looking at was full would be
    -- a layout rearranging a session it is not running. So it keeps its tree
    -- by `rejoin`'s rule, from the tile under the pointer: that tile either
    -- way, the largest with room, that tile anyway. See
    -- `a_layout_not_in_charge_sends_nothing_to_another_workspace`.
    if not tiling.active then
        rejoin(tree_for(monitor), id, target, cursor.x, cursor.y, options(monitor))
        tiling.apply()
        return
    end

    local window = dialogs.by_id(id)
    if window and not window.shown then
        tiling.unsized[id] = { target = target, cursor = { x = cursor.x, y = cursor.y } }
    end
    settle_open(id, monitor, target, cursor)
end)

-- Whether window `id`'s slot in `tree`, laid out on `monitor`, is smaller
-- than the window's own floor. See `Tiling::cramped`.
local function cramped_in(tree, id, monitor)
    for _, slot in ipairs(tree:layout(options(monitor))) do
        if slot.id == id then
            return slot.cramped == true
        end
    end
    return false
end

-- A window `open` placed before its application had said how small it can
-- be, placed again now that it has -- once, and only if it has not been shown
-- yet and the tile it was given cannot hold it (#115). Returns whether it
-- placed anything, which it has then applied.
--
-- The tile was decided at the launch, with nothing to decide it by. The
-- application's minimum comes with its first commit and is told as a
-- `layout` (`Solium::notice_limits`), and the tree gives the window what the
-- windows beside it can spare; where that is not enough its slot comes back
-- cramped. Such a window is taken out of its tree and opened again by
-- `open`'s own rule -- the tile under the pointer, then
-- `config.tiling.overflow` -- from the target and the pointer `open` had, so a
-- window that is opening goes through the overflow chain whichever way it was
-- opened. See
-- `a_launched_window_whose_minimum_does_not_fit_goes_where_overflow_says` in
-- `state/tests.rs`.
--
-- A window that has been shown is never moved by this: its minimum growing
-- is a rebalance and nothing more. Nor is one that is not in the tree of the
-- desk in view -- left on its own desk by the user switching away, say --
-- since the chain decides for the desk in view, and the window would end up
-- in two trees. Each of the three is a case of
-- `a_launched_window_is_placed_again_by_its_minimum_only_before_it_is_shown`
-- in `script.rs`.
local function reconsider()
    if not tiling.active or next(tiling.unsized) == nil then
        return false
    end
    local windows = by_id()
    local ids = {}
    for id in pairs(tiling.unsized) do
        ids[#ids + 1] = id
    end
    table.sort(ids)
    local placed = false
    for _, id in ipairs(ids) do
        local opened = tiling.unsized[id]
        local window = windows[id]
        if not window or window.leaving or window.shown then
            tiling.unsized[id] = nil
        elseif sizes.floor(window) then
            tiling.unsized[id] = nil
            local monitor = monitors.of(id)
            local tree = tree_for(monitor)
            if tree:contains(id) and cramped_in(tree, id, monitor) then
                tree:remove(id)
                settle_open(id, monitor, opened.target, opened.cursor)
                placed = true
            end
        end
    end
    return placed
end

sol.on("layout", function()
    if not reconsider() then
        tiling.apply()
    end
end)

-- ...and a window leaving hands its space to its neighbour, rather than
-- re-tiling the screen around the hole.
--
-- At the moment the close is asked for, by default. The compositor answers
-- every interaction at once and lets the application catch up -- a window has a
-- place before its program has started, and a dragged edge is where the hand
-- is before the client has redrawn -- and a close is the same: the window
-- fades where it stood while its neighbours grow into the space (#128).
-- `reflow_on_close = "when_gone"` is the old behaviour, and these two handlers
-- then do nothing and `close` does it all.
sol.on("closing", function(id)
    -- Only while this layout is in charge. A tree nobody is using is not on
    -- screen, and a window taken out of it here would be put back at
    -- `refused` from a centre measured in another mode's geometry, at the
    -- configured split rather than its own -- a rearrangement of a layout the
    -- user is not even looking at. `adopt` leaves a window being closed out
    -- if the user switches to tiling during the close.
    if not tiling.active or not reflows_at_once() then
        return
    end
    local window = dialogs.by_id(id)
    local from
    for key, tree in pairs(tiling.trees) do
        if tree:contains(id) then
            from = key
        end
        -- Every tree, as `close` does: one window in two trees is one window
        -- given two slots.
        tree:remove(id)
    end
    -- The centre of where it stood, which is inside whichever window has just
    -- grown over its space -- its old sibling, when that was a single window.
    -- Kept only for a window this took out of a tree: a dialog is in none.
    if window and from then
        tiling.leaving[id] = {
            x = window.x + window.w / 2,
            y = window.y + window.h / 2,
            tree = from,
        }
    end
    tiling.apply()
end)

-- The tree a window belongs in now, and its key: its own workspace's, on the
-- monitor it is on. Not `tree_for`, which answers for the workspace that
-- monitor is showing -- the user may have switched away during the close.
local function home_of(id)
    local monitor = monitors.of(id)
    local key = monitors.key(workspaces.at(id, monitor), monitor)
    if not tiling.trees[key] then
        tiling.trees[key] = sol.layout.tree()
    end
    return tiling.trees[key], key, monitor
end

-- The application declined -- "save your changes?" -- and the window is back.
-- It goes back into the tree it left, split off whichever window now covers the
-- centre of where it stood. When its neighbour was a single window that is the
-- neighbour, grown into both their spaces, and the window comes back on the
-- same side of it -- so an arrangement that had not otherwise changed comes
-- back as it was. When the neighbour was a group, it splits one of the group.
--
-- Not the old split *ratio*. `remove` deleted the split, and the window returns
-- at the configured `split`; keeping the ratio would mean a tree that can hold
-- an absent leaf, which nothing has needed yet.
--
-- Not `open`, which the compositor deliberately does not send for this: the
-- window never went, and an arrival would run `open.lua`'s animation again.
-- Nor `open`'s placement: a refused window is never sent to another workspace
-- for want of room, and goes back into the tile where it stood even under
-- `tiling.minimum` (#134). See `rejoin`.
--
-- **Decided by what happened, not by the setting.** A window is put back if
-- `closing` took it out -- whatever `reflow_on_close` says now, since a script
-- may have changed it in between -- or if it is missing from a layout that is
-- in charge, which is where `adopt` leaves a window being closed. Where it
-- stood is used only if that is still the tree the window belongs in: a
-- monitor unplugged during the close leaves a tree `apply` never walks.
sol.on("refused", function(id)
    local stood = tiling.leaving[id]
    tiling.leaving[id] = nil
    if not stood and not tiling.active then
        return
    end
    local window = dialogs.by_id(id)
    if not window or dialogs.floats(window) then
        return
    end
    for _, tree in pairs(tiling.trees) do
        if tree:contains(id) then
            return
        end
    end
    local tree, key, monitor = home_of(id)
    if stood and stood.tree ~= key then
        stood = nil
    end
    rejoin(tree, id, nil, stood and stood.x, stood and stood.y, options(monitor), stood ~= nil)
    tiling.apply()
end)

-- The window is gone. Unchanged by `reflow_on_close`: a window that closed
-- itself was never `closing`, so this is the one event every layout hears for
-- every window that goes. For one `closing` already took out, the `remove`
-- below finds nothing to remove.
sol.on("close", function(id)
    for _, tree in pairs(tiling.trees) do
        tree:remove(id)
    end
    -- Ids are never reused, so a stale entry here would not put the wrong
    -- window back -- it would simply accumulate for the life of the session.
    tiling.exiled[id] = nil
    tiling.leaving[id] = nil
    tiling.cramped[id] = nil
    tiling.unsized[id] = nil
    dialogs.forget(id)
    tiling.apply()
end)

-- Dropped onto another window, the two trade places in the tree; dropped
-- anywhere else, the window slides back to its own slot.
sol.on("drop", function(id, x, y)
    if not tiling.active then
        return
    end
    -- A dialog stays where it was dropped, and joins no tree: it is in none,
    -- and the insert below is the one way it could get into one. `dropped`
    -- records the drag so the next pass does not undo it -- as an offset from
    -- the parent, so the dialog still follows the window it belongs to.
    if dialogs.dropped(id) then
        tiling.apply(config.tiling.snap)
        return
    end
    -- Skip the window being dragged: it follows the cursor, so it is always
    -- the topmost thing under it, and asking without skipping just names the
    -- window in your hand.
    local target = sol.window_at(x, y, id)
    -- Where it was *dropped*, which after a drag across the boundary is the
    -- other monitor's tree. Taken out of every tree first, so a window moved
    -- between screens does not stay in the one it left.
    local landed = monitors.of(id)
    for _, tree in pairs(tiling.trees) do
        tree:remove(id)
    end
    local tree = tree_for(landed)
    -- Where it was let go, and not the largest tile, which is somewhere else;
    -- `rejoin` with a place turns the split rather than go under the minimum,
    -- and goes under it rather than leave the tile it was dropped on.
    if target and target ~= id then
        -- Re-inserting where it was dropped is the swap: out of its old seam,
        -- into the one under the pointer.
        rejoin(tree, id, target, x, y, options(landed), true)
    else
        -- Dropped on nothing: it still belongs to whatever screen it landed
        -- on, so it rejoins that tree rather than falling out of the layout.
        rejoin(tree, id, nil, x, y, options(landed), true)
    end
    tiling.apply(config.tiling.snap)
end)

-- Dragging an edge moves the seam this window shares with its neighbour,
-- rather than giving the window a size of its own. In a tiled arrangement a
-- window does not have one: the space is divided, and dragging an edge moves
-- where the division falls. Returning a command tells the compositor we took
-- it, so it does not also resize the window directly.
--
-- `edge_x` and `edge_y` are where the dragged edge should come to rest, one
-- per axis, in the same coordinates `tree:layout` hands back and `sol.place`
-- takes. Not where the pointer is, which is what this used to be handed: a
-- seam set from the cursor lands under the cursor, so a drag begun anywhere
-- but exactly on the edge threw that edge across to the cursor on its first
-- frame. `super` plus the right button begins a resize from the *middle* of a
-- window, so there the throw was most of a window wide. That is #124, and the
-- fix is entirely on the compositor's side of this call -- the arithmetic here
-- and in `drag_seam` is unchanged, and is simply given a relative target now.
--
-- `horizontal_side` and `vertical_side` are the *sides* the pointer has hold
-- of -- "left" or "right", "top" or "bottom", and nil for an axis that is not
-- being dragged. Named for the side and not the axis because the value is a
-- side: `horizontal` holding "left" invites the reader to test it as a
-- boolean, which is the very mistake below.
-- They were plain booleans until #120, and a boolean is not enough to
-- choose a seam with: a window that is the right-hand child of a vertical
-- split has that split's seam on its *left*, so "the horizontal axis is in
-- play" moved that seam for a drag on the window's right edge. The far side
-- jumped 209 pixels and the side under the pointer did not move at all.
-- Passed straight through, because the side is exactly what `drag_seam` wants
-- and the compositor knew it all along.
sol.on("resize", function(id, edge_x, edge_y, horizontal_side, vertical_side)
    if not tiling.active then
        return
    end
    local monitor = monitors.of(id)
    local tree = tree_for(monitor)
    -- The seam goes where the dragged edge goes. Still a position and not a
    -- delta: a delta would be measured against a layout this very drag just
    -- changed, and the windows would shake for as long as the button was held.
    -- The position is relative to the grab all the same, because the
    -- compositor derives it from the rectangle the drag has produced.
    --
    -- Both arms can run: a corner drag is two drags, one seam per axis. Each
    -- gets its own edge -- `edge_x` for the vertical seam, `edge_y` for the
    -- horizontal one -- and `drag_seam` reads only the one its side names, so
    -- neither axis can borrow the other's.
    if horizontal_side then
        tree:drag_seam(id, horizontal_side, edge_x, edge_y, options(monitor))
    end
    if vertical_side then
        tree:drag_seam(id, vertical_side, edge_x, edge_y, options(monitor))
    end
    -- Placed immediately. An animation would be chasing the pointer, and the
    -- pointer wins.
    tiling.apply({ duration = 0 })
end)

sol.bind("super+t", tiling.toggle)

-- Move the seam this window sits on. Everything on the far side stays put,
-- which is the property a tree has and a recomputed arrangement does not.
--
-- The axis is named, where a drag names a side: a keypress says "wider", not
-- which neighbour gives up the room, so `tree:resize` prefers the seam on the
-- right or below and falls back to the other one. Before #120 it moved the
-- window's immediate parent, and in an ordinary four-window dwindle every
-- leaf's parent is a horizontal split -- so "wider" reached the centre
-- vertical seam in no arrangement at all and quietly changed the height.
local function focused_window()
    for _, window in ipairs(sol.windows()) do
        if window.focused then return window.id end
    end
    return nil
end

local function nudge(axis, by)
    return function()
        local focused = focused_window()
        if focused then
            -- With the monitor's options, which carry `tiling.minimum`: a
            -- press no more takes a tile under it than a drag does.
            local monitor = monitors.of(focused)
            tree_for(monitor):resize(focused, axis, by, options(monitor))
            tiling.apply(config.tiling.snap)
        end
    end
end

sol.bind("super+minus", nudge("width", -0.05))
sol.bind("super+equal", nudge("width", 0.05))
-- Height on the same two keys, because the other axis was reachable before
-- this change -- by accident, in the arrangements whose parent split happened
-- to be horizontal -- and losing it to fix the width would trade one
-- unreachable seam for another.
--
-- Ctrl and not shift, which is what these were first written as. A binding is
-- a table lookup on the string `input::combo_for` builds, and that string
-- names the key from `modified_sym` -- the keysym with the modifiers already
-- applied. Shift changes it: on a `us` layout shift+`-` arrives as
-- `underscore` and shift+`=` as `plus`, so `super+shift+minus` is a spelling
-- nothing will ever produce, and a binding nothing produces fails silently --
-- no warning at load, and at the press only a `no script has bound this` at
-- info. Ctrl selects no shift level, so ctrl+`-` still arrives as `minus`;
-- `super+ctrl+left` and its three siblings in `workspaces.lua` have been live
-- on exactly that basis. Verified against a real `us` keymap rather than
-- assumed -- see `every_height_bind_is_a_key_that_arrives` in `script.rs`.
--
-- That `modified_sym` is what bindings match on at all is #121, which is not
-- fixed here: it changes how every binding in the compositor is matched and
-- wants a live keyboard. Ctrl is the spelling that is correct either way --
-- when #121 lands and matching moves to the unmodified keysym, ctrl+`-` is
-- still `minus`, whereas `super+shift+underscore` would work today and die on
-- that commit.
sol.bind("super+ctrl+minus", nudge("height", -0.05))
sol.bind("super+ctrl+equal", nudge("height", 0.05))

-- ## By direction (#150)
--
-- `direction.lua` hands these the keys; which window is that way is its
-- `direction.find`, over the tiles the trees laid out, and not `loose`: the
-- tile that way is one wholly past this one's edge and level with it, never
-- one off on a diagonal, and with none on this monitor the next monitor is
-- asked. `in_tiling_focus_and_move_reach_the_neighbour_in_each_direction` and
-- `in_tiling_a_tile_off_on_a_diagonal_is_not_that_way`.

-- The focused window's tile, marked with its monitor, or nil when the keyboard
-- is on a window no tree in view holds: a dialog, or one the user floated.
-- `a_window_the_layout_does_not_arrange_gets_the_floating_answer`.
local function focused_tile()
    local id = focused_window()
    if not id then
        return nil
    end
    local monitor = monitors.of(id)
    for _, slot in ipairs(tree_for(monitor):layout(options(monitor))) do
        if slot.id == id then
            slot.monitor = monitor
            return slot
        end
    end
    return nil
end

-- Every tile on the desk monitor `name` is showing, but window `except`'s.
-- `at_a_monitors_edge_tiling_crosses_to_the_desk_the_next_monitor_shows`.
local function tiles_on(name, except)
    local out = {}
    for _, slot in ipairs(tree_for(name):layout(options(name))) do
        if slot.id ~= except then
            slot.monitor = name
            out[#out + 1] = slot
        end
    end
    return out
end

-- Whether a keyboard move goes into the neighbour's split rather than trading
-- places with it: `config.tiling.move`, read at each move. Anything but
-- "split" is a swap, and anything but "swap" as well is named once.
-- `with_tiling_move_split_a_move_goes_into_the_neighbours_split`.
local function moves_into_split()
    local given = config.tiling.move
    if given == "split" then
        return true
    end
    if given ~= nil and given ~= "swap" then
        once(string.format(
            "tiling.move is %s, which is neither \"swap\" nor \"split\"; a move swaps",
            tostring(given)
        ))
    end
    return false
end

-- Focus the tile that way. A window no tree holds is left to the floating
-- answer, which finds its neighbours among every window on screen.
-- `a_window_the_layout_does_not_arrange_gets_the_floating_answer`.
function tiling.focus_direction(dir)
    local from = focused_tile()
    if not from then
        return false
    end
    local to = direction.find(from, dir, function(name)
        return tiles_on(name, from.id)
    end)
    if to then
        sol.focus(to.id)
    end
    return true
end

-- Trade places with the tile that way: every split keeps its axis and its
-- ratio, and nothing else moves. At the monitor's edge the window leaves its
-- tree for the tree of the desk the next monitor is showing, going in beside
-- the tile it arrives at, or taking the screen when that desk is empty. With
-- `tiling.move = "split"` a move within a monitor splits the neighbour's tile
-- instead, across its longer side as a window opening there would, which is
-- Hyprland's `movewindow`. A window no tree holds does not move. A window that
-- belongs to no workspace in particular belongs to none after crossing, too.
-- `in_tiling_focus_and_move_reach_the_neighbour_in_each_direction`,
-- `in_tiling_a_tile_off_on_a_diagonal_is_not_that_way`,
-- `with_tiling_move_split_a_move_goes_into_the_neighbours_split`,
-- `at_a_monitors_edge_tiling_crosses_to_the_desk_the_next_monitor_shows`,
-- `a_window_on_no_workspace_is_on_none_after_crossing` and
-- `a_window_the_layout_does_not_arrange_gets_the_floating_answer`.
function tiling.move_direction(dir)
    local from = focused_tile()
    if not from then
        return true
    end
    local to, name, crossed = direction.find(from, dir, function(monitor)
        return tiles_on(monitor, from.id)
    end)
    if not name then
        return true
    end
    if to and not crossed and not moves_into_split() then
        tree_for(name):swap(from.id, to.id)
    else
        for _, tree in pairs(tiling.trees) do
            tree:remove(from.id)
        end
        local tree = tree_for(name)
        if to then
            local side = crossed and direction.opposite[dir] or dir
            local x, y = direction.inside(to, side, from)
            rejoin(tree, from.id, to.id, x, y, options(name), true)
        else
            rejoin(tree, from.id, nil, nil, nil, options(name), true)
        end
        if crossed and workspaces.of[from.id] ~= nil then
            workspaces.of[from.id] = workspaces.on(name)
        end
    end
    tiling.apply(config.tiling.snap)
    return true
end

return tiling
