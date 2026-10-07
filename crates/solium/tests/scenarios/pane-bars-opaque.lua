-- The bars of the `reactive` and `reveal` styles, in the shipped theme:
-- opaque and grey, focused or not, so what is behind the window -- the
-- wallpaper's colour -- never shows through them and tints their grey.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`, in
-- software. Every pane is 400 by 300 outside. `reactive`'s bar is the top 30
-- rows and `reveal`'s the top 34, there only while the pointer is inside the
-- window; the points read are bar and nothing else, clear of the centred
-- title and of the buttons at the right.

local styles = {
    { pane = "reactive", client = { 392, 266 }, at = { { 8, 8 }, { 100, 22 }, { 300, 8 } } },
    { pane = "reveal", client = { 400, 300 }, at = { { 8, 8 }, { 100, 26 }, { 300, 8 } }, inside = true },
}

local function opaque_grey(world, style, what)
    for _, at in ipairs(style.at) do
        local r, g, b, a = world.pixel("bar", at[1], at[2])
        local said = string.format("%s's %s bar at (%d, %d): %d %d %d %d", style.pane, what, at[1], at[2], r, g, b, a)
        assert(a == 255, said .. ", not opaque")
        assert(math.abs(r - g) <= 2 and math.abs(g - b) <= 2, said .. ", not a grey")
    end
end

local steps = {}
local function step(it)
    steps[#steps + 1] = it
end

for _, style in ipairs(styles) do
    step({ pane = style.pane, client = style.client })
    step({ tell = { focused = true, inside = style.inside == true, title = "terminal" } })
    step({ wait = 400 })
    step({ expect = function(world) opaque_grey(world, style, "focused") end })

    step({ tell = { focused = false } })
    step({ wait = 400 })
    step({ expect = function(world) opaque_grey(world, style, "unfocused") end })
end

return { qt = true, steps = steps }
