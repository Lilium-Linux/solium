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

function tiling.toggle()
    tiling.active = not tiling.active
    if tiling.active then
        tiling.adopt()
        sol.status("tiling")
        tiling.apply()
    else
        sol.status("")
    end
end

-- A new window splits whatever the pointer is over. This is the whole of
-- "the window opens where the cursor is".
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
sol.on("resize", function(id, dx, dy)
    if not tiling.active then
        return
    end
    local area = sol.monitor()
    local tree = tree_for(workspaces.active)
    -- Whichever axis moved more is the one the seam runs along; the tree knows
    -- which way its own branch was cut.
    local by
    if math.abs(dx) >= math.abs(dy) then
        by = dx / math.max(area.w, 1)
    else
        by = dy / math.max(area.h, 1)
    end
    tree:resize(id, by)
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
