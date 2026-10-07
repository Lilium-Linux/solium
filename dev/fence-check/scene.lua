-- dev/fence-check.sh's configuration: two windows at fixed rectangles, both
-- rounded, both tilted with a held transform, so both are captured for a
-- warp on every frame. A rounded window is no longer captured for its
-- corners: its surfaces are drawn through the clipped programs where they
-- are, so only a warp still captures.
--
-- FENCE_CHECK_ANGLE is the tilt, 8 degrees unless the control asks for 9.
sol.pane("rounded")

local ANGLE = tonumber(os.getenv("FENCE_CHECK_ANGLE") or "8")
local RECTS = {
    { x = 120, y = 140, w = 520, h = 360 },
    { x = 700, y = 160, w = 480, h = 340 },
}
local first = nil
local second = nil
local opened = 0

sol.on("open", function(id)
    opened = opened + 1
    sol.animate({ duration = 0 })
    sol.place(id, RECTS[opened] or RECTS[2])
    if opened == 1 then
        first = id
    else
        second = second or id
    end
end)

sol.bind("super+t", function()
    sol.animate({ duration = 0 })
    sol.present(first, { rotate_z = ANGLE })
    sol.present(second, { rotate_z = -ANGLE })
end)
