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

local tiling = { active = false, trees = {} }

-- One tree per workspace. A window closing on workspace 2 must not disturb
-- the arrangement on workspace 1, and a shared tree cannot promise that.
local function tree_for(index)
    if not tiling.trees[index] then
        tiling.trees[index] = sol.layout.tree()
    end
    return tiling.trees[index]
end

local function options()
    local area = sol.monitor()
    area.gap = config.gap
    area.split = config.tiling.split
    return area
end

function tiling.apply(animation)
    if not tiling.active then
        return
    end
    local tree = tree_for(workspaces.active)
    local slots = tree:layout(options())
    if #slots == 0 then
        return
    end

    sol.animate(animation or config.tiling.motion)
    for _, slot in ipairs(slots) do
        sol.place(slot.id, slot)
    end
end

-- Bring the tree in line with what is actually on screen. Used when tiling is
-- switched on, and as a backstop: events are the normal path, this is what
-- makes a missed one recoverable rather than permanent.
function tiling.adopt()
    local tree = tree_for(workspaces.active)
    local present = {}
    for _, window in ipairs(workspaces.visible()) do
        present[window.id] = true
        if not tree:contains(window.id) then
            tree:insert(window.id, nil, nil, nil, options())
        end
    end
    for _, id in ipairs(tree:windows()) do
        if not present[id] then
            tree:remove(id)
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
sol.on("layout", function()
    tiling.apply()
end)

sol.on("open", function(id)
    local tree = tree_for(workspaces.active)
    local cursor = sol.cursor()
    -- Skip the window being opened: it is already mapped and under the
    -- pointer, so asking without skipping names it as its own split target.
    tree:insert(id, sol.window_at(cursor.x, cursor.y, id), cursor.x, cursor.y, options())
    tiling.apply()
end)

-- ...and a window leaving hands its space to its neighbour, rather than
-- re-tiling the screen around the hole.
sol.on("close", function(id)
    for _, tree in pairs(tiling.trees) do
        tree:remove(id)
    end
    tiling.apply()
end)

-- Dropped onto another window, the two trade places in the tree; dropped
-- anywhere else, the window slides back to its own slot.
sol.on("drop", function(id, x, y)
    if not tiling.active then
        return
    end
    -- Skip the window being dragged: it follows the cursor, so it is always
    -- the topmost thing under it, and asking without skipping just names the
    -- window in your hand.
    local target = sol.window_at(x, y, id)
    local tree = tree_for(workspaces.active)
    if target and target ~= id then
        -- Re-inserting where it was dropped is the swap: out of its old seam,
        -- into the one under the pointer.
        tree:remove(id)
        tree:insert(id, target, x, y, options())
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
    local tree = tree_for(workspaces.active)
    -- The seam goes where the pointer is. Not where it moved to: a delta would
    -- be measured against a layout this very drag just changed, and the windows
    -- would shake for as long as the button was held.
    if horizontal then
        tree:drag_seam(id, "width", x, y, options())
    end
    if vertical then
        tree:drag_seam(id, "height", x, y, options())
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
        tree_for(workspaces.active):resize(focused, -0.05)
        tiling.apply(config.tiling.snap)
    end
end)

sol.bind("super+equal", function()
    local focused = nil
    for _, window in ipairs(sol.windows()) do
        if window.focused then focused = window.id end
    end
    if focused then
        tree_for(workspaces.active):resize(focused, 0.05)
        tiling.apply(config.tiling.snap)
    end
end)

return tiling
