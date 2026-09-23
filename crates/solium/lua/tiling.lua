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

-- `exiled` is the ids this layout has taken out of its trees because they are
-- modal dialogs, and it is what makes `unset_modal` reversible: only a window
-- that was taken out is ever put back. See `dialogs.settle` for why it is not
-- simply "re-admit whatever is missing".
--
-- `leaving` is where each window that is being closed stood when it was taken
-- out of its tree, by id: the centre of its rectangle and the key of the tree
-- it was in. It is what a refused close puts the window back by. See the
-- `closing` and `refused` handlers.
local tiling = { active = false, trees = {}, exiled = {}, leaving = {} }

-- Whether the other windows close up the moment a close is asked for, rather
-- than once the application has gone. Read when each event arrives rather than
-- captured when this file loads, so a script that changes the setting while the
-- session runs changes the next close. See `reflow_on_close` in config.lua;
-- anything but "when_gone" is the default.
local function reflows_at_once()
    return config.tiling.reflow_on_close ~= "when_gone"
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
    return out
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
                tree:insert(id, nil, nil, nil, options(monitor))
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
    for _, each in ipairs(screens) do
        local tree = tree_for(each.monitor.name)
        for _, slot in ipairs(tree:layout(options(each.monitor.name))) do
            sol.place(slot.id, slot)
            placed[slot.id] = slot
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

-- Bring the tree in line with what is actually on screen. Used when tiling is
-- switched on, and as a backstop: events are the normal path, this is what
-- makes a missed one recoverable rather than permanent.
function tiling.adopt()
    -- Also how a window that moved between monitors settles: it is missing
    -- from its new screen's tree and still in its old one's, and both halves
    -- are fixed here.
    local present = {}
    for _, each in ipairs(monitors.each(workspaces.visible())) do
        local tree = tree_for(each.monitor.name)
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
                present[window.id] = each.monitor.name
                if not tree:contains(window.id) then
                    tree:insert(window.id, nil, nil, nil, options(each.monitor.name))
                end
            end
        end
    end
    for key, tree in pairs(tiling.trees) do
        for _, id in ipairs(tree:windows()) do
            -- Removed when the window is gone, and when it is on another
            -- monitor now: one window in two trees is one window given two
            -- slots, and it ends up in whichever was laid out last.
            if not present[id]
                or monitors.key(workspaces.on(present[id]), present[id]) ~= key
            then
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

sol.on("layout", function()
    tiling.apply()
end)

sol.on("open", function(id)
    -- A dialog joins no tree. `apply` places it over its parent and records it
    -- as exiled, so there is nothing to do here but let that happen.
    if dialogs.floating(id) then
        tiling.apply()
        return
    end
    local tree = tree_for(monitors.of(id))
    local cursor = sol.cursor()
    -- Skip the window being opened: it is already mapped and under the
    -- pointer, so asking without skipping names it as its own split target.
    tree:insert(
        id,
        sol.window_at(cursor.x, cursor.y, id),
        cursor.x,
        cursor.y,
        options(monitors.of(id))
    )
    tiling.apply()
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
    tree:insert(id, nil, stood and stood.x, stood and stood.y, options(monitor))
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
    if target and target ~= id then
        -- Re-inserting where it was dropped is the swap: out of its old seam,
        -- into the one under the pointer.
        tree:insert(id, target, x, y, options(landed))
    else
        -- Dropped on nothing: it still belongs to whatever screen it landed
        -- on, so it rejoins that tree rather than falling out of the layout.
        tree:insert(id, nil, x, y, options(landed))
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
            tree_for(monitors.of(focused)):resize(focused, axis, by)
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

return tiling
