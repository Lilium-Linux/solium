-- A pivot is the point the matrix leaves alone.
--
-- One window at a known asymmetric rect, and three frames from one run:
--
--   f-000  untransformed
--   f-001  rotate_z = 20, the default pivot   (super+a)
--   f-002  rotate_z = 20, pivot 0,0           (super+b)
--
-- Nothing is required. No layout script may move the window and no wallpaper
-- may put pixels behind it -- both would be measured as the compositor's
-- answer, and neither is.
--
-- `sol.pane("none")` for the same reason: a frame is the compositor's own
-- surface and would put a titlebar between the outer rect and the corner this
-- check measures.

sol.pane("none")

-- Asymmetric in position and in size, so a transposed coordinate anywhere
-- between the script and the mesh comes out as a wrong number rather than as
-- the right one by luck.
local WINDOW = { x = 420, y = 190, w = 520, h = 360 }
local first = nil
local opened = 0

sol.on("open", function(id)
    opened = opened + 1
    -- Instant, because this check photographs poses. An animation would make
    -- every number depend on when the shutter opened.
    sol.animate({ duration = 0 })
    sol.place(id, WINDOW)
    if opened == 1 then
        first = id
    end
    sol.log(string.format("present-check open #%d id=%d at %d,%d %dx%d",
        opened, id, WINDOW.x, WINDOW.y, WINDOW.w, WINDOW.h))
end)

sol.bind("super+a", function()
    sol.animate({ duration = 0 })
    sol.present(first, { rotate_z = 20 })
    sol.log(string.format("present-check id=%d rotate_z=20, default pivot", first))
end)

sol.bind("super+b", function()
    sol.animate({ duration = 0 })
    sol.present(first, { rotate_z = 20, pivot_x = 0, pivot_y = 0 })
    sol.log(string.format("present-check id=%d rotate_z=20, pivot 0,0", first))
end)
