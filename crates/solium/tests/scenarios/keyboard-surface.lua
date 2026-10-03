-- The keyboard pill on its own surface: `keyboard.indicator.show = "surface"`.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`, with
-- `us,ru` and Russian live. A window at (300, 200) whose client says its
-- caret is at (100, 40), 2 by 16, in its surface: on screen that is (400, 240)
-- to (402, 256), so the surface -- 112 by 56, the capsule 28 high in the
-- middle of it -- is centred under the caret, the capsule's top 6 below it.

local function pill(world)
    local surface = world.surfaces["keyboard-indicator"]
    assert(surface, "the overlay surface is declared")
    return surface, surface.properties.cue
end

return {
    user = [[ return { keyboard = { indicator = { show = "surface" } } } ]],
    init = [[ require("keyboard_indicator") ]],
    steps = {
        { open = true },
        { move = { 300, 200 } },
        { field = { 100, 40, 2, 16 } },
        {
            expect = function(world)
                local _, cue = pill(world)
                assert(cue.what == "", "built at load, showing nothing")
                assert(world.field and world.field.x == 400 and world.field.y == 240,
                    "the caret is where the window puts it")
            end,
        },

        -- Caps Lock on: the Caps pill at the caret, for `duration` -- placed
        -- once, it cannot follow the caret as you type.
        { key = "caps_lock" },
        {
            expect = function(world)
                local surface, cue = pill(world)
                assert(surface.x == 345 and surface.y == 248 and surface.w == 112 and surface.h == 56,
                    string.format("placed at %s,%s %sx%s", surface.x, surface.y, surface.w, surface.h))
                assert(cue.what == "caps" and cue.hold == false and cue.duration == 1200,
                    "the Caps pill, timed")
                assert(world.panes.keyboard_indicator.show == false, "and no pane draws one")
            end,
        },

        -- A letter changes nothing.
        { key = "a" },
        {
            expect = function(world)
                local _, cue = pill(world)
                assert(cue.what == "caps", "still the Caps pill")
            end,
        },

        -- A layout switch with Caps Lock on: the layout's pill, handing back
        -- to nothing at a caret it cannot follow.
        { key = "shift+alt_l" },
        {
            expect = function(world)
                local _, cue = pill(world)
                assert(cue.what == "layout" and cue.after == nil,
                    "the layout's pill, handing back to nothing here")
            end,
        },

        -- On screen, with no caret to follow, the layout's pill hands back
        -- to Caps Lock's, held, when it goes.
        { field = true },
        { key = "shift+alt_l" },
        {
            expect = function(world)
                local surface, cue = pill(world)
                assert(cue.what == "layout" and cue.after == "caps",
                    "the layout's pill, handing back to Caps Lock's")
                assert(surface.x == 904 and surface.y == 938,
                    string.format("on screen at %s,%s", surface.x, surface.y))
            end,
        },
        { field = { 100, 40, 2, 16 } },

        -- Caps Lock off: hidden at once.
        { key = "caps_lock" },
        {
            expect = function(world)
                local _, cue = pill(world)
                assert(cue.what == "", "hidden as Caps goes off")
            end,
        },

        -- A layout switch by key: the layout's pill, for `duration`.
        { key = "shift+alt_l" },
        {
            expect = function(world)
                local surface, cue = pill(world)
                assert(cue.what == "layout" and not cue.hold and cue.duration == 1200,
                    "the layout's pill, timed")
                assert(surface.x == 345 and surface.y == 248, "at the caret")
            end,
        },

        -- A field with no caret: the pill falls back to the screen, centred
        -- across the monitor near its bottom.
        { field = true },
        { key = "shift+alt_l" },
        {
            expect = function(world)
                local surface, cue = pill(world)
                assert(world.field and world.field.x == nil, "a field with no caret")
                assert(cue.what == "layout", "the layout's pill")
                assert(surface.x == 904 and surface.y == 938,
                    string.format("on screen at %s,%s", surface.x, surface.y))
            end,
        },
    },
}
