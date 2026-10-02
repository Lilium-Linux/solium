-- The overlay surface's scene, `qml/indicator/keyboard.qml`, as
-- `keyboard-surface.lua`'s policy declares it: 112 by 48, the capsule in the
-- middle of it, drawn from the cue it is handed.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`, in
-- software. The capsule is 24 high and at least 42 wide, centred: its middle
-- row is y = 24, and (40, 24) is on it, clear of the glyph in its middle.

local function accent(r, g, b, a)
    return r < 8 and math.abs(g - 0x60) < 8 and math.abs(b - 0xc0) < 8 and a == 255
end

return {
    qt = true,
    steps = {
        { scene = "indicator/keyboard.qml", size = { 112, 48 } },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("scene", 40, 24)
                assert(a == 0, "built showing nothing")
            end,
        },

        { set = { cue = { what = "caps", serial = 1, hold = true, duration = 1200 } } },
        { wait = 300 },
        {
            expect = function(world)
                local r, g, b, a = world.pixel("scene", 40, 24)
                assert(accent(r, g, b, a), string.format("the capsule: %d %d %d %d", r, g, b, a))
                local _, _, _, corner = world.pixel("scene", 2, 2)
                assert(corner == 0, "and room around it")
            end,
        },

        -- Hidden at once with an empty cue.
        { set = { cue = { what = "", serial = 2 } } },
        { wait = 300 },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("scene", 40, 24)
                assert(a == 0, "hidden")
            end,
        },

        -- The same cue again is nothing new, and shows nothing.
        { set = { cue = { what = "caps", serial = 2, hold = true } } },
        { wait = 300 },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("scene", 40, 24)
                assert(a == 0, "a cue already seen is not shown again")
            end,
        },
    },
}
