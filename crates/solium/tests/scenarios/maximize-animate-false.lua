-- `maximize.animate = false` makes maximising and restoring instant, and
-- leaves going fullscreen as it was: the two are read on their own (#49).
-- Keys pressed with Russian live.
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
        "going fullscreen still grows: " .. show(drawn))
end

return {
    user = [[ return { maximize = { animate = false } } ]],
    init = [[
        require("modes")
        require("fullscreen")
    ]],
    steps = {
        {
            expect = function(world)
                assert(#world.unknown == 0,
                    "maximize is a section, not a typo: " .. tostring(world.unknown[1] and world.unknown[1].key))
            end,
        },
        { open = true },
        { move = { 300, 200 } },
        { key = "super+shift+m" },
        { answer = 1 },
        { expect = at_once(screen, "maximised on the key") },
        { wait = 400 },
        { key = "super+shift+m" },
        { answer = 1 },
        { expect = at_once(before, "and restored on the key") },
        { wait = 400 },

        { key = "super+f" },
        { answer = 1 },
        { wait = 120 },
        { expect = growing },
    },
}
