-- The `pulse` style's breathing line, in the shipped theme: opaque and grey,
-- focused or not, so what is behind the window -- the wallpaper's colour --
-- never shows through the frame.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`, in
-- software. The pane is 400 by 300; the bar is 36 high and the line the two
-- rows below it, 36 and 37.

local function line(world, what)
    for _, y in ipairs({ 36, 37 }) do
        for _, x in ipairs({ 8, 200, 392 }) do
            local r, g, b, a = world.pixel("bar", x, y)
            local said = string.format("%s at (%d, %d): %d %d %d %d", what, x, y, r, g, b, a)
            assert(a == 255, said .. ", not opaque")
            assert(math.abs(r - g) <= 2 and math.abs(g - b) <= 2, said .. ", not a grey")
        end
    end
end

return {
    qt = true,
    steps = {
        { pane = "pulse", client = { 400, 262 } },
        { tell = { focused = false, title = "terminal" } },
        { wait = 400 },
        { expect = function(world) line(world, "an unfocused window's line") end },

        { tell = { focused = true } },
        { wait = 300 },
        { expect = function(world) line(world, "a focused window's line, early") end },
        { wait = 1000 },
        { expect = function(world) line(world, "a focused window's line, at its faintest") end },
    },
}
