-- The default `top` style's bar, in the shipped theme: dark and grey,
-- the focused window's bar told apart from the others' by its shade, and its
-- title light on it -- dimmer on a window without the keyboard.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`, in
-- software. The pane is 400 by 300, its bar 32 high. (8, 8) is bar and
-- nothing else; the title, "terminal", is centred on (200, 16), and the box
-- from (160, 6) to (240, 26) holds all of it.

-- The bar at (8, 8): opaque, grey, and dark. Answers its grey.
local function bar(world, what)
    local r, g, b, a = world.pixel("bar", 8, 8)
    local said = string.format("%s: %d %d %d %d", what, r, g, b, a)
    assert(a == 255, said .. ", not opaque")
    assert(math.abs(r - g) <= 2 and math.abs(g - b) <= 2, said .. ", not a grey")
    assert(r <= 64, said .. ", not dark")
    return r
end

-- The lightest pixel of the title's box, as a grey, every pixel of it grey.
local function title(world, what)
    local lightest = 0
    for y = 6, 26 do
        for x = 160, 240 do
            local r, g, b = world.pixel("bar", x, y)
            assert(math.abs(r - g) <= 2 and math.abs(g - b) <= 2,
                string.format("%s: (%d, %d) is %d %d %d, not a grey", what, x, y, r, g, b))
            lightest = math.max(lightest, r)
        end
    end
    return lightest
end

local seen = {}

return {
    qt = true,
    steps = {
        { pane = "top", client = { 400, 268 } },
        { tell = { focused = true, title = "terminal" } },
        { wait = 400 },
        {
            expect = function(world)
                seen.bar = bar(world, "the focused bar")
                seen.title = title(world, "the focused title")
                assert(seen.title >= seen.bar + 96,
                    string.format("the focused title (%d) does not stand out on its bar (%d)", seen.title, seen.bar))
            end,
        },

        { tell = { focused = false } },
        { wait = 400 },
        {
            expect = function(world)
                local shade = bar(world, "an unfocused bar")
                assert(math.abs(seen.bar - shade) >= 8,
                    string.format("the focused bar (%d) and an unfocused one (%d) are not told apart", seen.bar, shade))
                local dim = title(world, "an unfocused title")
                assert(dim >= shade + 48,
                    string.format("an unfocused title (%d) does not stand out on its bar (%d)", dim, shade))
                assert(dim < seen.title,
                    string.format("an unfocused title (%d) is not dimmer than a focused one (%d)", dim, seen.title))
            end,
        },
    },
}
