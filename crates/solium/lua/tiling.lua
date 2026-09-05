-- Tiling, as a script.
--
-- A layout decides where windows *live*, which is different from what a mode
-- does: overview moves where windows are drawn and puts them back, while this
-- changes the geometry everything else reads. `sol.place` is that authority,
-- and the compositor glides each window from where it was to where it now is —
-- so switching layouts is animated for free and cannot disagree with a mode
-- about where a window is going.
--
-- Master and stack, the arrangement worth having first: one large window with
-- the rest in a column beside it. Scrolling and the phone and tablet layouts
-- are further scripts beside this one, not modes inside it.

local config = require("config")
local workspaces = require("workspaces")

local tiling = { active = false, ratio = config.tiling.ratio, order = {} }

local GAP = config.gap
local SETTLE = config.tiling.motion

-- The tiled order, kept by the script rather than derived from stacking.
--
-- Derived order cannot survive a swap: the moment two windows trade places the
-- arrangement has to remember that, and stacking order does not. Windows that
-- have gone are dropped and new ones are appended, so opening a window never
-- reshuffles the ones already placed.
function tiling.reconcile()
    -- Only the workspace in view is arranged. Windows elsewhere are drawn a
    -- screen away and must not take a slot in this one.
    local windows = workspaces.visible()
    local by_id = {}
    for _, window in ipairs(windows) do
        by_id[window.id] = window
    end

    local kept, seen = {}, {}
    for _, id in ipairs(tiling.order) do
        if by_id[id] then
            kept[#kept + 1] = id
            seen[id] = true
        end
    end
    -- Oldest first, so the master is the window that has been there longest
    -- rather than whichever was clicked last.
    for i = #windows, 1, -1 do
        local id = windows[i].id
        if not seen[id] then
            kept[#kept + 1] = id
            seen[id] = true
        end
    end

    tiling.order = kept
    local ordered = {}
    for _, id in ipairs(kept) do
        ordered[#ordered + 1] = by_id[id]
    end
    return ordered
end

function tiling.apply()
    if not tiling.active then
        return
    end
    local ordered = tiling.reconcile()
    if #ordered == 0 then
        return
    end

    -- The arrangement itself comes from `sol.layout`, which is the same code
    -- the preview page calls. A script that wanted a different one would
    -- compute its own rectangles here instead; that is the difference between
    -- offering an arrangement and imposing one.
    local area = sol.monitor()
    area.gap = GAP
    area.ratio = tiling.ratio
    local slots = sol.layout.master_stack(#ordered, area)

    sol.animate(SETTLE)
    for i, window in ipairs(ordered) do
        sol.place(window.id, slots[i])
    end
end

function tiling.toggle()
    tiling.active = not tiling.active
    if tiling.active then
        sol.status("tiling")
        tiling.apply()
    else
        -- Windows stay where the tiling left them. Restoring their floating
        -- positions would mean remembering geometry across a layout change,
        -- which is a second authority over where a window lives.
        sol.status("")
    end
end

-- A window let go in a tiled layout does not stay where it was dropped: that
-- is the whole point of tiling. Dropped onto another window the two trade
-- places; dropped anywhere else it slides back to its own slot.
sol.on("drop", function(id, x, y)
    if not tiling.active then
        return
    end
    -- `sol.window_at` answers with an id, not a window.
    local target = sol.window_at(x, y)
    if target and target ~= id then
        local from, to
        for index, known in ipairs(tiling.order) do
            if known == id then from = index end
            if known == target then to = index end
        end
        if from and to then
            tiling.order[from], tiling.order[to] = tiling.order[to], tiling.order[from]
        end
    end
    tiling.apply()
end)

sol.bind("super+t", tiling.toggle)

-- A window appearing or leaving changes the arrangement, so the layout runs
-- again. Appended rather than replacing the open animation — both listen.
sol.on("open", tiling.apply)

return tiling
