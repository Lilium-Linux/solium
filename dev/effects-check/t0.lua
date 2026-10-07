-- effects-check's `t0` section: one window at a known rect, nothing else, and
-- (when T0_RING is set) a generated ring behind its client, reaching 12 px
-- past it. Nothing but the ring can change the band around the window.
sol.pane("none")
local WINDOW = { x = 420, y = 190, w = 520, h = 360 }
sol.on("open", function(id)
    sol.animate({ duration = 0 })
    sol.place(id, WINDOW)
end)
if os.getenv("T0_RING") then
    sol.effects({ rules = { { match = "*", part = "client", slot = "behind", effect = { "ring", width = 4 } } } })
end
