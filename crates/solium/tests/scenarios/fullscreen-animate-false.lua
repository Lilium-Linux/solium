-- `fullscreen.animate = false` makes going fullscreen and back instant, and
-- leaves maximising as it was: the two are read on their own (#49). Keys
-- pressed with Russian live.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

local before = { x = 300, y = 200, w = 200, h = 100 }
local screen = { x = 0, y = 0, w = 1920, h = 1080 }

local function show(rect)
    return string.format("%.1f,%.1f %.1fx%.1f", rect.x, rect.y, rect.w, rect.h)
end

local function on(rect, wanted)
    return math.abs(rect.x - wanted.x) < 0.5 and math.abs(rect.y - wanted.y) < 0.5
        and math.abs(rect.w - wanted.w) < 0.5 and math.abs(rect.h - wanted.h) < 0.5
end

local function at_once(wanted, why)
    return function(world)
        local drawn = world.windows[1].drawn
        assert(on(drawn, wanted), why .. ": " .. show(drawn))
    end
end

local function growing(world)
    local drawn = world.windows[1].drawn
    assert(drawn.w > before.w + 1 and drawn.w < screen.w - 1,
        "maximising still grows: " .. show(drawn))
end

return {
    user = [[ return { fullscreen = { animate = false } } ]],
    init = [[
        require("modes")
        require("fullscreen")
    ]],
    steps = {
        { open = true },
        { move = { 300, 200 } },
        { key = "super+f" },
        { answer = 1 },
        { expect = at_once(screen, "fullscreen on the key") },
        { wait = 400 },
        { key = "super+f" },
        { answer = 1 },
        { expect = at_once(before, "and back on the key") },
        { wait = 400 },

        { key = "super+shift+m" },
        { answer = 1 },
        { wait = 120 },
        { expect = growing },
        {
            expect = function(world)
                assert(#world.unknown == 0, "fullscreen.animate is a setting")
            end,
        },
    },
}
