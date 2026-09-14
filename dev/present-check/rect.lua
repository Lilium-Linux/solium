-- Hit-testing follows the drawn rect, and it is the drawn rect that moves.
--
-- The other half of *Hit-testing does not move*, and the likelier break. "The
-- hit test started following `z`" is exotic; "the hit test went back to the
-- real geometry" is ordinary -- `state.rs` tests
-- `self.drawn_at(pane, outer, now).rect.contains(location)`, and a regression
-- to `outer.contains` would pass every other check in here, because none of
-- them presents a rect at all.
--
-- One window is placed at a rect and presented at a disjoint one. A window
-- parked elsewhere opens last and holds focus, so both wrong answers are
-- distinguishable from doing nothing:
--
--   click the REAL rect     focus must NOT move: nothing is drawn there
--   click the DRAWN rect    focus MUST move to it
--
-- With `outer.contains` the two are exactly inverted, and either one alone
-- catches it.
--
-- The single capture is taken after the present and before the clicks, so a
-- failure of the second click is a hit-test failure and not a present that
-- never happened -- the check reads the pixels at both rects to tell those
-- apart.

sol.pane("none")

-- Three rectangles, none touching: real x 200..499 / y 150..349, drawn
-- x 760..1059 / y 300..499, parked x 1150..1529 / y 620..849. The clicks land
-- at (350, 250) and (910, 400), each a hundred pixels clear of any edge.
local REAL = { x = 200, y = 150, w = 300, h = 200 }
local DRAWN = { x = 760, y = 300, w = 300, h = 200 }
local PARKED = { x = 1150, y = 620, w = 380, h = 230 }

local ids = {}
local opened = 0

sol.on("open", function(id)
    opened = opened + 1
    local slot = opened == 1 and REAL or PARKED
    ids[opened] = id
    sol.animate({ duration = 0 })
    sol.place(id, slot)
    sol.log(string.format("present-check open #%d id=%d at %d,%d %dx%d",
        opened, id, slot.x, slot.y, slot.w, slot.h))
    if opened == 2 then
        sol.log(string.format("present-check IDS moved=%d parked=%d", ids[1], ids[2]))
    end
end)

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

sol.bind("super+p", function()
    sol.animate({ duration = 0 })
    -- `sol.windows()` keeps reporting the *real* rect, which is the point:
    -- nothing about the layout changed and the hit test must follow the
    -- drawing anyway.
    sol.present(ids[1], DRAWN)
    sol.log(string.format("present-check id=%d presented at %d,%d %dx%d",
        ids[1], DRAWN.x, DRAWN.y, DRAWN.w, DRAWN.h))
    dump("presented")
end)

sol.bind("super+s", function()
    dump("after-real-click")
end)

sol.bind("super+d", function()
    dump("after-drawn-click")
end)
