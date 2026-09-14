-- A raised window is drawn in front, and clicks did not follow it.
--
-- Two overlapping windows. The first to open is the *lower* one, because a
-- later window is mapped restacked to the top -- so raising the first with
-- z = 1 is a window drawn in front of the one the layout has on top, and that
-- is the only arrangement in which "drawn on top" and "on top" can disagree.
--
--   f-000  before                (super+s dumps the stacking order)
--   f-001  after the raise       (super+z)
--   f-002  after the click       (super+d dumps it again)
--
-- The click is driven by `SOLIUM_DRAG_AT` into the overlap, which goes through
-- the real grab, the real hit test and the real focus path. Focus must land on
-- the window at the head of the stack dump -- `Solium::window_under` walks
-- panes in stacking order and tests the drawn `rect`; `z` never enters it.

sol.pane("none")

-- Overlapping in x 520..819, y 320..579. The sample point the check uses is
-- (670, 450), which is over a hundred pixels clear of every edge -- a press
-- inside the resize border would be a resize rather than a click.
local SLOTS = {
    { x = 300, y = 200, w = 520, h = 380 },
    { x = 520, y = 320, w = 520, h = 380 },
}

local ids = {}
local opened = 0

sol.on("open", function(id)
    opened = opened + 1
    local slot = SLOTS[opened] or SLOTS[#SLOTS]
    ids[opened] = id
    sol.animate({ duration = 0 })
    sol.place(id, slot)
    sol.log(string.format("present-check open #%d id=%d at %d,%d %dx%d",
        opened, id, slot.x, slot.y, slot.w, slot.h))
end)

sol.on("focus", function(id)
    sol.log(string.format("present-check FOCUS id=%d (lower=%s upper=%s)",
        id, tostring(ids[1]), tostring(ids[2])))
end)

-- `sol.windows()` is topmost first, which is the order a hit test walks.
local function dump(tag)
    local order = {}
    for index, window in ipairs(sol.windows()) do
        order[index] = string.format("%d@%d,%d %dx%d%s", window.id, window.x, window.y,
            window.w, window.h, window.focused and "*" or "")
    end
    sol.log(string.format("present-check STACK %s topmost-first: %s",
        tag, table.concat(order, " | ")))
end

sol.bind("super+s", function()
    dump("before")
end)

sol.bind("super+z", function()
    sol.animate({ duration = 0 })
    sol.present(ids[1], { z = 1 })
    sol.log(string.format("present-check id=%d z=1, the lower window", ids[1]))
    dump("raised")
end)

sol.bind("super+d", function()
    dump("after-click")
end)
