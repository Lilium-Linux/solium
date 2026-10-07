-- effects-check's `none` section: the `blur` section's two windows, a still
-- terminal on the left and a video player on the right, and no rule at all:
-- with no effects configured nothing is captured and nothing runs (spec
-- §8.4).
sol.pane("none")
local LEFT = { x = 40, y = 60, w = 560, h = 380 }
local RIGHT = { x = 640, y = 60, w = 560, h = 380 }
local opened = 0
sol.on("open", function(id)
    opened = opened + 1
    sol.animate({ duration = 0 })
    sol.place(id, opened == 1 and LEFT or RIGHT)
end)
