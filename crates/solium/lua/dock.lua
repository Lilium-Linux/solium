-- The dock, and the animation that makes it worth having.
--
-- A window opened from an icon grows out of that icon. This is the whole
-- reason the dock is drawn by the compositor instead of being a client: the
-- icon's rectangle and the window's are known to the same process at the same
-- moment, so one can be interpolated into the other. A dock in its own process
-- can hand over a rectangle, but by then the window is already somewhere else
-- and the two are only near each other by arrangement.
--
-- Nothing here is compositor code. `sol.present_from` says where a window
-- comes from and the compositor glides it to where it lives; the compositor
-- has no idea that this one happens to be a dock icon.

local config = require("config")

local dock = { launched = nil }

sol.dock(config.dock.items)

-- Pressing an icon spawns its program and remembers the square it came from.
sol.on("dock", function(label, x, y, w, h)
    dock.launched = { x = x, y = y, w = w, h = h }
    sol.spawn(label)
end)

-- ...and the next window to appear grows out of that square.
--
-- Cleared whether or not it was used: a window that opens for some other
-- reason must not inherit the last icon anyone pressed, or a terminal opened
-- from the keyboard would come flying out of the dock.
sol.on("open", function(id)
    local from = dock.launched
    dock.launched = nil
    if not from then
        return
    end
    sol.animate(config.dock.morph)
    sol.present_from(id, from)
end)

return dock
