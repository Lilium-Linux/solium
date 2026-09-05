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

local tiling = { active = false, ratio = 0.6 }

local GAP = 12
local SETTLE = { duration = 240, easing = "outCubic" }

function tiling.apply()
    if not tiling.active then
        return
    end
    local windows = sol.windows()
    if #windows == 0 then
        return
    end

    -- `sol.windows` is topmost first; tiling wants a stable order, so the
    -- oldest window is the master rather than whichever was clicked last.
    local ordered = {}
    for i = #windows, 1, -1 do
        ordered[#ordered + 1] = windows[i]
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

sol.bind("super+t", tiling.toggle)

-- A window appearing or leaving changes the arrangement, so the layout runs
-- again. Appended rather than replacing the open animation — both listen.
sol.on("open", tiling.apply)

return tiling
