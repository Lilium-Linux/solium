-- The keyboard pill drawn inside the pane, in the shipped `top` style, from
-- what the panes are handed: `KeyboardPillLayer` at the caret. The QML's half;
-- `keyboard-pane.lua` is the policy's.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`, in
-- software. The pane is 400 by 300, its bar 32 high. A caret at (100, 60),
-- 2 by 16, in the pane: the capsule is 28 high, its top 6 below the caret,
-- so its middle row is y = 96, and it is centred on x = 101, at least 32
-- wide. (89, 96) is on the capsule, clear of the glyph or label in its
-- middle.
--
-- And whether the layer is `dormant`, which spares a frame with no pill on
-- show the layer's image and its blending over the client: dormant whenever
-- nothing shows, awake from the cue that shows one.

-- The capsule, `Theme.accent` whatever it is set to: opaque, a grey (red,
-- green and blue within 2 of one another), and light -- so it stands out on
-- a dark window -- yet a shade off white, so it shows on a light one too.
-- Neither the dark glyph nor the shadow is that.
local function capsule(r, g, b, a)
    return a == 255
        and math.abs(r - g) <= 2 and math.abs(g - b) <= 2
        and r >= 160 and r <= 224
end

-- Whether any opaque pixel of `layer` in the box from (x0, y0) to (x1, y1)
-- is dark: the glyph, drawn on the capsule in a shade that contrasts with it.
local function inked(world, layer, x0, y0, x1, y1)
    for y = y0, y1 do
        for x = x0, x1 do
            local r, g, b, a = world.pixel(layer, x, y)
            if a == 255 and r <= 96 and math.abs(r - g) <= 2 and math.abs(g - b) <= 2 then
                return true
            end
        end
    end
    return false
end

-- The shadow below the capsule: something drawn there, and black.
local function shadowed(r, g, b, a)
    return a >= 8 and r == 0 and g == 0 and b == 0
end

local function cue(what, serial, hold, duration, after)
    return {
        keyboard_indicator = {
            show = true,
            cue = { what = what, serial = serial, hold = hold, duration = duration, after = after },
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
                local _, _, _, a = world.pixel("keyboard", 89, 96)
                assert(a == 0, "a cue from before the caret came is not shown late")
                assert(world.dormant.keyboard == true, "and the layer is dormant")
            end,
        },

        -- Caps Lock's pill, at the caret.
        { tell = { values = cue("caps", 2, true, 1200) } },
        { wait = 300 },
        {
            expect = function(world)
                local r, g, b, a = world.pixel("keyboard", 89, 96)
                assert(capsule(r, g, b, a), string.format("the capsule below the caret: %d %d %d %d", r, g, b, a))
                -- The capsule spans 85 to 116 across and 82 to 109 down.
                assert(inked(world, "keyboard", 85, 82, 116, 109), "with Caps Lock's glyph dark on it")
                r, g, b, a = world.pixel("keyboard", 101, 112)
                assert(shadowed(r, g, b, a), string.format("and its shadow below: %d %d %d %d", r, g, b, a))
                local _, _, _, above = world.pixel("keyboard", 86, 50)
                assert(above == 0, "and nothing above it")
                assert(world.dormant.keyboard == nil, "awake while it shows")
            end,
        },

        -- Held: still there long after a layout's pill would have gone.
        { wait = 1500 },
        {
            expect = function(world)
                assert(capsule(world.pixel("keyboard", 89, 96)), "a held pill stays")
            end,
        },

        -- The field goes: gone at once.
        { tell = { caret = false } },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("keyboard", 89, 96)
                assert(a == 0, "gone with the caret")
                assert(world.dormant.keyboard == true, "and dormant at once")
            end,
        },

        -- A layout's pill shows for its duration, then goes by itself.
        { tell = { caret = { 100, 60, 2, 16 }, values = cue("layout", 3, false, 500) } },
        { wait = 300 },
        {
            expect = function(world)
                assert(capsule(world.pixel("keyboard", 89, 96)), "the layout's pill")
            end,
        },
        { wait = 600 },
        {
            expect = function(world)
                local _, _, _, a = world.pixel("keyboard", 89, 96)
                assert(a == 0, "gone after its duration")
                assert(world.dormant.keyboard == true, "and dormant once it has faded")
            end,
        },

        -- Near the bottom of the pane: above the caret instead.
        { tell = { caret = { 100, 280, 2, 16 }, values = cue("caps", 4, true, 1200) } },
        { wait = 300 },
        {
            expect = function(world)
                -- Its bottom 6 above the caret: rows 246 to 274, middle 260.
                assert(capsule(world.pixel("keyboard", 89, 260)), "above the caret")
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
                local _, _, _, a = world.pixel("keyboard", 89, 96)
                assert(a == 0, "not drawn when the panes are told not to")
                assert(world.dormant.keyboard == true, "and dormant")
            end,
        },

        -- A layout's pill shown over Caps Lock's hands back to it when it
        -- goes, held, rather than leaving Caps Lock on with nothing shown.
        -- `show` comes back on its own first, as it is in a session, where
        -- it only changes with a reload: one `values` write turning `show`
        -- on and handing a new cue would leave `take()` reading `accepts`
        -- before or after its binding has caught up, as Qt's notify order
        -- falls, and this step is about the hand-back, not that order.
        { tell = { caret = { 100, 60, 2, 16 }, values = cue("", 6, false, 500) } },
        { tell = { values = cue("layout", 7, false, 500, "caps") } },
        { wait = 300 },
        {
            expect = function(world)
                assert(capsule(world.pixel("keyboard", 89, 96)), "the layout's pill")
            end,
        },
        { wait = 1000 },
        {
            expect = function(world)
                assert(capsule(world.pixel("keyboard", 89, 96)), "Caps Lock's, past the layout's duration")
                assert(world.dormant.keyboard == nil, "and still awake")
            end,
        },
    },
}
