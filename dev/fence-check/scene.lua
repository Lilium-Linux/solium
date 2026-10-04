-- dev/fence-check.sh's configuration: two windows at fixed rectangles, both
-- rounded (so each client is captured for its corners), the first tilted
-- with a held transform (so it is captured for a warp too).
--
-- FENCE_CHECK_ANGLE is the tilt, 8 degrees unless the control asks for 9.
sol.pane("rounded")

local ANGLE = tonumber(os.getenv("FENCE_CHECK_ANGLE") or "8")
local RECTS = {
    { x = 120, y = 140, w = 520, h = 360 },
    { x = 700, y = 160, w = 480, h = 340 },
}
local first = nil
local opened = 0

sol.on("open", function(id)
    opened = opened + 1
    sol.animate({ duration = 0 })
    sol.place(id, RECTS[opened] or RECTS[2])
    if opened == 1 then
        first = id
    end
end)

sol.bind("super+t", function()
    sol.animate({ duration = 0 })
    sol.present(first, { rotate_z = ANGLE })
end)
