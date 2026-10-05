-- A genie on the second monitor lands on its target (#143).
--
-- Two nested monitors (SOLIUM_OUTPUTS=2). One window placed on the right
-- one, then a genie at progress 1 into a rectangle on the right one: every
-- pixel of the window must be inside that rectangle. The log says where the
-- rectangle is in the nested window's pixels, which is what the check reads.
sol.pane("none")

local placed = nil
sol.on("open", function(id)
    local rows = sol.monitors()
    local right = rows[2] or rows[1]
    sol.animate({ duration = 0 })
    sol.place(id, { x = right.x + 60, y = right.y + 80, w = 420, h = 300 })
    placed = id
end)

sol.bind("super+g", function()
    local rows = sol.monitors()
    local left, right = rows[1], rows[2] or rows[1]
    local to = { x = right.x + 200, y = right.y + 600, w = 160, h = 40 }
    sol.animate({ duration = 0 })
    sol.present(placed, { deform = { effect = "genie", axis = "down", spread = 0, to = to } })
    sol.log(string.format("AIM_TARGET %d %d %d %d", to.x - right.x + left.whole.w, to.y - right.y, to.w, to.h))
end)
