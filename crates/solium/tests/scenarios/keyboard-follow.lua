-- The keyboard pill after the caret, with `show = "pane"`, the default, and a
-- client that says where its caret is only after a key, as kitty does: it
-- enables its field with no caret, and sends one a few milliseconds after
-- each key, `sol.on("text_input")` telling it as `"caret"`.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`, with
-- `us,ru` and Russian live. A framed window at (300, 200); the surface is
-- 112 by 56, centred under the caret with the capsule's top 6 below it, as
-- in `keyboard-surface.lua`, or on screen at (904, 938) with no caret.

local function handed(world)
    local values = world.panes.keyboard_indicator
    assert(values and values.show == true, "the panes are told to draw it")
    return values.cue
end

local function pill(world)
    local surface = world.surfaces["keyboard-indicator"]
    assert(surface, "the overlay surface is declared")
    return surface, surface.properties.cue
end

-- Where the surface goes for the caret `world.field` says.
local function under(world)
    local field = world.field
    assert(field and field.x, "a caret")
    return math.floor(field.x + field.w / 2 - 56 + 0.5), math.floor(field.y + field.h + 6 - 14 + 0.5)
end

-- The layout's cue, as first shown on the screen.
local first = nil
-- The cue the panes held for Caps Lock.
local caps = nil

return {
    init = [[ require("keyboard_indicator") ]],
    steps = {
        { open = true },
        { move = { 300, 200 } },
        { framed = 1 },
        { field = true },

        -- The layout switch is the first key the field sees: no caret yet,
        -- so the layout's pill goes on the screen.
        { key = "shift+alt_l" },
        {
            expect = function(world)
                local surface, cue = pill(world)
                assert(cue.what == "layout", "the layout's pill")
                assert(surface.x == 904 and surface.y == 938,
                    string.format("on screen at %s,%s", surface.x, surface.y))
                assert(handed(world).what == "", "with no caret, not in a pane")
                first = cue
            end,
        },

        -- The caret it sends after that key: the pill goes to it, on the
        -- surface and with the same cue, so it keeps its time rather than
        -- showing afresh.
        { caret = { 100, 40, 2, 16 } },
        {
            expect = function(world)
                local surface, cue = pill(world)
                local x, y = under(world)
                assert(surface.x == x and surface.y == y,
                    string.format("at the caret: %s,%s, not %s,%s", surface.x, surface.y, x, y))
                assert(cue.what == "layout" and cue.serial == first.serial, "the same cue")
                assert(handed(world).what == "", "and still no pane draws it")
            end,
        },

        -- Typing moves the caret, and the pill with it.
        { caret = { 110, 40, 2, 16 } },
        {
            expect = function(world)
                local surface, cue = pill(world)
                local x = under(world)
                assert(surface.x == x, string.format("after the caret: %s, not %s", surface.x, x))
                assert(cue.serial == first.serial, "the same cue")
            end,
        },

        -- Caps Lock on, with a caret now: in the pane, held, and the surface
        -- hidden.
        { key = "caps_lock" },
        {
            expect = function(world)
                caps = handed(world)
                assert(caps.what == "caps" and caps.hold == true, "the Caps pill in the pane, held")
                local _, cue = pill(world)
                assert(cue.what == "", "not on the surface as well")
            end,
        },

        -- The caret moves: the pane reads it itself, and nothing is sent.
        { caret = { 120, 40, 2, 16 } },
        {
            expect = function(world)
                assert(handed(world).serial == caps.serial, "no new cue for a caret the pane follows")
            end,
        },

        -- Caps Lock off: hidden, and a caret moving shows nothing again.
        { key = "caps_lock" },
        { caret = { 130, 40, 2, 16 } },
        {
            expect = function(world)
                assert(handed(world).what == "", "hidden")
                local _, cue = pill(world)
                assert(cue.what == "", "and nothing on the surface")
            end,
        },
    },
}
