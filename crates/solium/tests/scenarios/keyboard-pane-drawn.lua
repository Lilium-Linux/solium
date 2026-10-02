-- The keyboard pill drawn inside the pane, in the shipped `top` style, from
-- what the panes are handed: `KeyboardPillLayer` at the caret. The QML's half;
-- `keyboard-pane.lua` is the policy's.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`, in
-- software. The pane is 400 by 300, its bar 32 high. A caret at (100, 60),
-- 2 by 16, in the pane: the capsule is 24 high, its top 6 below the caret,
-- so its middle row is y = 94, and it is centred on x = 101, at least 42
-- wide. (86, 94) is on the capsule, clear of the glyph in its middle.
--
-- And whether the layer is `dormant`, which spares a frame with no pill on
-- show the layer's image and its blending over the client: dormant whenever
-- nothing shows, awake from the cue that shows one.

-- `Theme.accent`, opaque, whatever it is set to -- #0060c0 today, #936DFF
-- once the theme turns violet: blue the strongest channel by a clear
-- margin, which neither the white glyph nor the shadow is.
local function accent(r, g, b, a)
    return a == 255 and b > r + 24 and b > g + 24
end

local function cue(what, serial, hold, duration)
    return {
        keyboard_indicator = {
            show = true,
            cue = { what = what, serial = serial, hold = hold, duration = duration },
        },
    }
end

return {
    qt = true,
    steps = {
        { pane = "top", client = { 400, 268 } },

        -- Handed while the pane had no caret: not shown, then or later.
        { tell = { caret = false, values = cue("caps", 1, true, 1200) } },
        { tell = { caret = { 100, 60, 2, 16 } } },
        { wait = 300 },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("keyboard", 86, 94)
                assert(a == 0, "a cue from before the caret came is not shown late")
                assert(world.dormant.keyboard == true, "and the layer is dormant")
            end,
        },

        -- Caps Lock's pill, at the caret.
        { tell = { values = cue("caps", 2, true, 1200) } },
        { wait = 300 },
        {
            expect = function(world)
                local r, g, b, a = world.pixel("keyboard", 86, 94)
                assert(accent(r, g, b, a), string.format("the capsule below the caret: %d %d %d %d", r, g, b, a))
                local _, _, _, above = world.pixel("keyboard", 86, 50)
                assert(above == 0, "and nothing above it")
                assert(world.dormant.keyboard == nil, "awake while it shows")
            end,
        },

        -- Held: still there long after a layout's pill would have gone.
        { wait = 1500 },
        {
            expect = function(world)
                assert(accent(world.pixel("keyboard", 86, 94)), "a held pill stays")
            end,
        },

        -- The field goes: gone at once.
        { tell = { caret = false } },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("keyboard", 86, 94)
                assert(a == 0, "gone with the caret")
                assert(world.dormant.keyboard == true, "and dormant at once")
            end,
        },

        -- A layout's pill shows for its duration, then goes by itself.
        { tell = { caret = { 100, 60, 2, 16 }, values = cue("layout", 3, false, 500) } },
        { wait = 300 },
        {
            expect = function(world)
                assert(accent(world.pixel("keyboard", 86, 94)), "the layout's pill")
            end,
        },
        { wait = 600 },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("keyboard", 86, 94)
                assert(a == 0, "gone after its duration")
                assert(world.dormant.keyboard == true, "and dormant once it has faded")
            end,
        },

        -- Near the bottom of the pane: above the caret instead.
        { tell = { caret = { 100, 280, 2, 16 }, values = cue("caps", 4, true, 1200) } },
        { wait = 300 },
        {
            expect = function(world)
                -- Its bottom 6 above the caret: rows 250 to 274, middle 262.
                assert(accent(world.pixel("keyboard", 86, 262)), "above the caret")
            end,
        },

        -- `show` anything but true: nothing, whatever the cue.
        {
            tell = {
                caret = { 100, 60, 2, 16 },
                values = { keyboard_indicator = { show = false, cue = { what = "caps", serial = 5, hold = true } } },
            },
        },
        { wait = 300 },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("keyboard", 86, 94)
                assert(a == 0, "not drawn when the panes are told not to")
                assert(world.dormant.keyboard == true, "and dormant")
            end,
        },
    },
}
