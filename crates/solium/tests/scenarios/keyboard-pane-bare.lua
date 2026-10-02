-- The keyboard pill with `show = "pane"`, the default, in a window drawn with
-- no frame -- fullscreen, or one drawing its own decorations. No pane style is
-- drawn around it to draw the pill, so the surface goes to its caret instead,
-- where `show = "surface"` would put it.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`, with
-- `us,ru` and Russian live. A window at (300, 200) whose client says its caret
-- is at (100, 40), 2 by 16: on screen that is (400, 240) once the window is
-- bare, so the surface -- 112 by 48 -- is centred under the caret at
-- (345, 250), as in `keyboard-surface.lua`.

local function handed(world)
    local values = world.panes.keyboard_indicator
    assert(values and values.show == true, "the panes are told to draw it")
    return values.cue
end

local function pill(world)
    local surface = world.surfaces["keyboard-indicator"]
    assert(surface, "the overlay surface is declared, even with show = \"pane\"")
    return surface, surface.properties.cue
end

return {
    init = [[ require("keyboard_indicator") ]],
    steps = {
        { open = true },
        { move = { 300, 200 } },
        { framed = 1 },
        { field = { 100, 40, 2, 16 } },

        -- Framed: the pane draws the Caps pill, and the surface shows nothing.
        { key = "caps_lock" },
        {
            expect = function(world)
                assert(world.field and world.field.framed == true, "a framed window")
                assert(handed(world).what == "caps", "the pane draws the Caps pill")
                local _, cue = pill(world)
                assert(cue.what == "", "not on the surface as well")
            end,
        },
        { key = "caps_lock" },

        -- Bare, as a window going fullscreen is left: on the surface, at the
        -- caret, and no pane is handed it.
        { bare = 1 },
        { key = "caps_lock" },
        {
            expect = function(world)
                assert(world.field and world.field.framed == false, "a bare window")
                assert(world.field.x == 400 and world.field.y == 240,
                    string.format("the caret at %s,%s", world.field.x, world.field.y))
                assert(handed(world).what == "", "no pane draws it")
                local surface, cue = pill(world)
                assert(cue.what == "caps" and cue.hold == true, "the Caps pill, held, on the surface")
                assert(surface.x == 345 and surface.y == 250,
                    string.format("at the caret: %s,%s", surface.x, surface.y))
            end,
        },
    },
}
