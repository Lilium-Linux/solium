-- The overlay surface's scene, `qml/indicator/keyboard.qml`, as
-- `keyboard-surface.lua`'s policy declares it: 112 by 56, the capsule in the
-- middle of it, drawn from the cue it is handed.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`, in
-- software. The capsule is 28 high and at least 32 wide, centred: its middle
-- row is y = 28, and (44, 28) is on it, clear of the glyph in its middle.

-- The capsule, `Theme.accent` whatever it is set to: opaque, a grey (red,
-- green and blue within 2 of one another), and light -- so it stands out on
-- a dark window -- yet a shade off white, so it shows on a light one too.
-- Neither the dark glyph nor the shadow is that.
local function capsule(r, g, b, a)
    return a == 255
        and math.abs(r - g) <= 2 and math.abs(g - b) <= 2
        and r >= 160 and r <= 224
end

-- Whether any opaque pixel of the scene in the box from (x0, y0) to (x1, y1)
-- is dark: the glyph, drawn on the capsule in a shade that contrasts with it.
local function inked(world, x0, y0, x1, y1)
    for y = y0, y1 do
        for x = x0, x1 do
            local r, g, b, a = world.pixel("scene", x, y)
            if a == 255 and r <= 96 and math.abs(r - g) <= 2 and math.abs(g - b) <= 2 then
                return true
            end
        end
    end
    return false
end

return {
    qt = true,
    steps = {
        { scene = "indicator/keyboard.qml", size = { 112, 56 } },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("scene", 44, 28)
                assert(a == 0, "built showing nothing")
            end,
        },

        { set = { cue = { what = "caps", serial = 1, hold = true, duration = 1200 } } },
        { wait = 300 },
        {
            expect = function(world)
                local r, g, b, a = world.pixel("scene", 44, 28)
                assert(capsule(r, g, b, a), string.format("the capsule: %d %d %d %d", r, g, b, a))
                -- The capsule spans 40 to 71 across and 14 to 41 down.
                assert(inked(world, 40, 14, 71, 41), "with Caps Lock's glyph dark on it")
                r, g, b, a = world.pixel("scene", 56, 44)
                assert(a >= 8 and r == 0 and g == 0 and b == 0,
                    string.format("and its shadow below: %d %d %d %d", r, g, b, a))
                local _, _, _, corner = world.pixel("scene", 2, 2)
                assert(corner == 0, "and room around it")
            end,
        },

        -- Hidden at once with an empty cue.
        { set = { cue = { what = "", serial = 2 } } },
        { wait = 300 },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("scene", 44, 28)
                assert(a == 0, "hidden")
            end,
        },

        -- The same cue again is nothing new, and shows nothing.
        { set = { cue = { what = "caps", serial = 2, hold = true } } },
        { wait = 300 },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("scene", 44, 28)
                assert(a == 0, "a cue already seen is not shown again")
            end,
        },
    },
}
