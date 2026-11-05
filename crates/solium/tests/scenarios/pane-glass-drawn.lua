-- The `glass` style: a bar along the bottom of the window with no fill at all,
-- focused or not, so the glass behind the pane shows through it, its title
-- dimmer on a window without the keyboard. The thin dark shade round the edge
-- of such a window is not a layer of the style's: it is the `shade` effect its
-- `effects.lua` puts in front of the pane, fading with the focus
-- (`effect::fade::tests`,
-- `state::tests::real_client::a_rule_with_a_fade_eases_its_slot_in_and_out_with_the_focus`,
-- and on the GPU wirecheck's "the shipped shade").
--
-- Played by `scenario::tests::each::pane_glass_drawn`, in
-- software. The pane is 400 by 300, its bar the bottom 32 rows, 268 to 299,
-- and its corners 16 round. The buttons are at the left (`pane-buttons-drawn.lua`
-- reads them), the title, "terminal", right after them, inside the box from
-- (56, 276) to (130, 292), and with no app id told there is no icon at the
-- right. x = 200 of the bar is bar and nothing else.

-- The alpha of a layer at (x, y), and the lightest of its channels:
-- premultiplied, so a dark pixel is near-black at any alpha.
local function at(world, layer, x, y)
    local r, g, b, a = world.pixel(layer, x, y)
    return a, math.max(r, g, b)
end

-- The bar draws nothing at x = 200, nor where an icon would be.
local function bar_clear(world, what)
    for y = 268, 299 do
        local a = at(world, "bar", 200, y)
        assert(a == 0, string.format("%s: the bar at (200, %d) has alpha %d, not nothing", what, y, a))
    end
    for y = 276, 292 do
        for x = 372, 388 do
            local a = at(world, "bar", x, y)
            assert(a == 0, string.format("%s: an icon at (%d, %d) with no app id", what, x, y))
        end
    end
end

-- The lightest pixel of the title's box: a grey, so light text on nothing.
local function title(world)
    local lightest = 0
    for y = 276, 292 do
        for x = 56, 130 do
            local r, g, b = world.pixel("bar", x, y)
            assert(math.abs(r - g) <= 2 and math.abs(g - b) <= 2,
                string.format("(%d, %d) of the title is %d %d %d, not a grey", x, y, r, g, b))
            lightest = math.max(lightest, r)
        end
    end
    return lightest
end

local seen = {}

return {
    qt = true,
    steps = {
        { pane = "glass", client = { 400, 268 } },
        { tell = { focused = true, title = "terminal" } },
        { wait = 400 },
        {
            expect = function(world)
                bar_clear(world, "focused")
                seen.title = title(world)
                assert(seen.title >= 160, string.format("the focused title is not light: %d", seen.title))
            end,
        },

        { tell = { focused = false } },
        { wait = 400 },
        {
            expect = function(world)
                bar_clear(world, "unfocused")
                local dim = title(world)
                assert(dim < seen.title,
                    string.format("an unfocused title (%d) is not dimmer than a focused one (%d)", dim, seen.title))
            end,
        },
    },
}
