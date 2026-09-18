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
local tiling = { active = false, trees = {}, exiled = {} }

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
            -- A dialog is deliberately absent from every tree, so `adopt` --
            -- whose whole job is to put back whatever is missing -- has to be
            -- told that this one is missing on purpose. It is left out of
            -- `present` too, so the sweep below does not go looking for it in
            -- a tree it was never in.
            if not dialogs.floats(window) then
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
sol.on("close", function(id)
    for _, tree in pairs(tiling.trees) do
        tree:remove(id)
    end
    -- Ids are never reused, so a stale entry here would not put the wrong
    -- window back -- it would simply accumulate for the life of the session.
    tiling.exiled[id] = nil
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
sol.on("resize", function(id, x, y, horizontal, vertical)
    if not tiling.active then
        return
    end
    local monitor = monitors.of(id)
    local tree = tree_for(monitor)
    -- The seam goes where the pointer is. Not where it moved to: a delta would
    -- be measured against a layout this very drag just changed, and the windows
    -- would shake for as long as the button was held.
    if horizontal then
        tree:drag_seam(id, "width", x, y, options(monitor))
    end
    if vertical then
        tree:drag_seam(id, "height", x, y, options(monitor))
    end
    -- Placed immediately. An animation would be chasing the pointer, and the
    -- pointer wins.
    tiling.apply({ duration = 0 })
end)

sol.bind("super+t", tiling.toggle)

-- Move the seam this window sits on. Everything on the far side stays put,
-- which is the property a tree has and a recomputed arrangement does not.
sol.bind("super+minus", function()
    local focused = nil
    for _, window in ipairs(sol.windows()) do
        if window.focused then focused = window.id end
    end
    if focused then
        tree_for(monitors.of(focused)):resize(focused, -0.05)
        tiling.apply(config.tiling.snap)
    end
end)

sol.bind("super+equal", function()
    local focused = nil
    for _, window in ipairs(sol.windows()) do
        if window.focused then focused = window.id end
    end
    if focused then
        tree_for(monitors.of(focused)):resize(focused, 0.05)
        tiling.apply(config.tiling.snap)
    end
end)

return tiling
