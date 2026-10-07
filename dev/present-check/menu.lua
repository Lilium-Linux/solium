-- A menu over a warped window (0.8): wl-probe's window and its popup, the
-- window tilted with super+t. The popup must be in front of the titlebar.
sol.pane("top")

local first = nil
sol.on("open", function(id)
    if first == nil then
        first = id
        sol.animate({ duration = 0 })
        sol.place(id, { x = 300, y = 200, w = 520, h = 390 })
    end
end)

sol.bind("super+t", function()
    sol.animate({ duration = 0 })
    sol.present(first, { rotate_z = 4 })
end)
