-- The `proximity` style's border, in the shipped theme: opaque and grey at
-- rest and on approach, focused or not, so what is behind the window -- the
-- wallpaper's colour -- never shows through it.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`, in
-- software. The pane is 400 by 300 outside, with 4 pixels reserved on every
-- side: at rest the border is the outermost of them, a hairline, and with the
-- pointer inside it fills all four.

local function opaque_grey(world, points, what)
    for _, at in ipairs(points) do
        local r, g, b, a = world.pixel("border", at[1], at[2])
        local said = string.format("%s at (%d, %d): %d %d %d %d", what, at[1], at[2], r, g, b, a)
        assert(a == 255, said .. ", not opaque")
        assert(math.abs(r - g) <= 2 and math.abs(g - b) <= 2, said .. ", not a grey")
    end
end

-- The outermost pixel of each side, away from the corners.
local hairline = { { 0, 150 }, { 399, 150 }, { 200, 0 }, { 200, 299 } }
-- The third pixel in from each side, which only the swollen border covers.
local swollen = { { 2, 150 }, { 397, 150 }, { 200, 2 }, { 200, 297 } }

return {
    qt = true,
    steps = {
        { pane = "proximity", client = { 392, 292 } },
        { tell = { focused = true, inside = false, title = "terminal" } },
        { wait = 400 },
        { expect = function(world) opaque_grey(world, hairline, "a focused window's hairline") end },

        { tell = { focused = false } },
        { wait = 400 },
        { expect = function(world) opaque_grey(world, hairline, "an unfocused window's hairline") end },

        { tell = { inside = true } },
        { wait = 400 },
        { expect = function(world) opaque_grey(world, swollen, "the border under the pointer") end },
    },
}
