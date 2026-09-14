-- A raised window is drawn in front, and clicks did not follow it.
--
-- Three windows, and the third is what makes the check mean anything.
--
-- The first two overlap. The first to open is the *lower* of the pair, because
-- a later window is mapped restacked to the top -- so raising the first with
-- z = 1 is a window drawn in front of the one the layout has on top, which is
-- the only arrangement in which "drawn on top" and "on top" can disagree.
--
-- The third overlaps neither and opens last, so it holds focus when the click
-- happens. **Without it this check is one refactor from a tautology**: with
-- only two windows the upper one is already focused before the click, so
-- "focus is the upper one" cannot be told from "the click did nothing", and it
-- separated them only because Solium happens to emit a focus event on a
-- no-change refocus. With the parked window focused first, all three outcomes
-- are distinct:
--
--   focus moves to the upper one   the hit test walked the stack: correct
--   focus moves to the lower one   the hit test followed `z`: wrong
--   focus stays on the parked one  the click found nothing: also wrong
--
--   f-000  before                (super+s dumps the stacking order)
--   f-001  after the raise       (super+z)
--   f-002  after the click       (super+d dumps it again)
--
-- The click is driven by `SOLIUM_DRAG_AT` into the overlap, which goes through
-- the real grab, the real hit test and the real focus path. `window_under`
-- walks panes in stacking order and tests the drawn `rect`; `z` never enters
-- it, and that is what is being pinned.

sol.pane("none")

-- The pair overlaps in x 520..819, y 320..579; the check samples (670, 450),
-- which is over a hundred pixels clear of every edge -- a press inside the
-- resize border would be a resize rather than a click. The parked window is
-- clear of both (x 1100..1479 against the pair's 300..1039).
local SLOTS = {
    { x = 300, y = 200, w = 520, h = 380 },
    { x = 520, y = 320, w = 520, h = 380 },
    { x = 1100, y = 80, w = 380, h = 240 },
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
    if opened == #SLOTS then
        sol.log(string.format("present-check IDS lower=%d upper=%d parked=%d",
            ids[1], ids[2], ids[3]))
    end
end)

-- `sol.windows()` is topmost first, which is the order a hit test walks, and
-- it carries `focused` -- so one dump answers both "what does the layout have
-- on top" and "what holds focus at this instant". The focused id is logged on
-- its own line rather than left as an asterisk for something to parse.
local function dump(tag)
    local order, focused = {}, 0
    for index, window in ipairs(sol.windows()) do
        order[index] = string.format("%d@%d,%d %dx%d%s", window.id, window.x, window.y,
            window.w, window.h, window.focused and "*" or "")
        if window.focused then
            focused = window.id
        end
    end
    sol.log(string.format("present-check STACK %s topmost-first: %s",
        tag, table.concat(order, " | ")))
    sol.log(string.format("present-check FOCUSED %s id=%d", tag, focused))
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
