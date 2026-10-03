-- `keyboard.indicator = false`: the same as `show = false`, no surface and no
-- pane draws a pill, whatever the keyboard does.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`, with
-- `us,ru` and Russian live.

return {
    user = [[ return { keyboard = { indicator = false } } ]],
    init = [[ require("keyboard_indicator") ]],
    steps = {
        { open = true },
        { field = { 100, 40, 2, 16 } },
        { key = "caps_lock" },
        { key = "shift+alt_l" },
        {
            expect = function(world)
                assert(world.surfaces["keyboard-indicator"] == nil, "no surface")
                assert(world.panes.keyboard_indicator.show == false, "and the panes are told so")
            end,
        },
    },
}
