-- The keyboard pill inside the pane: `keyboard.indicator.show = "pane"`, the
-- default. The policy's half: what every pane is handed, and the fallback to
-- the screen; `keyboard-pane-drawn.lua` is the QML's half.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`, with
-- `us,ru` and Russian live.

local function handed(world)
    local values = world.panes.keyboard_indicator
    assert(values and values.show == true, "the panes are told to draw it")
    return values.cue
end

local function surface(world)
    local declared = world.surfaces["keyboard-indicator"]
    assert(declared, "the fallback surface is declared")
    return declared, declared.properties.cue
end

return {
    init = [[ require("keyboard_indicator") ]],
    steps = {
        { open = true },
        { open = true },
        { focus = 1 },
        { field = { 100, 40, 2, 16 } },

        -- Caps Lock on, with a caret: the panes draw the Caps pill, and the
        -- surface shows nothing.
        { key = "caps_lock" },
        {
            expect = function(world)
                local cue = handed(world)
                assert(cue.what == "caps" and cue.hold == true, "the Caps pill, held")
                local _, shown = surface(world)
                assert(shown.what == "", "not on the surface as well")
            end,
        },

        -- The keyboard to the other window: gone, as it was about the last one.
        { focus = 2 },
        {
            expect = function(world)
                assert(handed(world).what == "", "gone with the focus")
            end,
        },

        -- A field there, focused while Caps is on: the Caps pill again.
        { field = { 30, 10, 2, 16 } },
        {
            expect = function(world)
                assert(handed(world).what == "caps", "Caps' pill again on focus")
            end,
        },

        -- Caps off: hidden at once.
        { key = "caps_lock" },
        {
            expect = function(world)
                assert(handed(world).what == "", "hidden as Caps goes off")
            end,
        },

        -- A layout switch where there is no caret: on the screen instead.
        { field = true },
        { key = "shift+alt_l" },
        {
            expect = function(world)
                assert(handed(world).what == "", "no pane has the caret")
                local declared, cue = surface(world)
                assert(cue.what == "layout", "the layout's pill, on screen")
                assert(declared.x == 904 and declared.y == 946,
                    string.format("at %s,%s", declared.x, declared.y))
            end,
        },
    },
}
