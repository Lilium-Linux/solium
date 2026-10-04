-- `fullscreen.instant.app_id` lists the applications that go fullscreen and
-- back at once (#49): one on the list covers its monitor on the key and is
-- back on the key, and one that is not still grows. Keys pressed with Russian
-- live.
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

local function at_once(n, wanted, why)
    return function(world)
        local drawn = world.windows[n].drawn
        assert(on(drawn, wanted), why .. ": " .. show(drawn))
    end
end

return {
    user = [[ return { fullscreen = { instant = { app_id = { "mpv" } } } } ]],
    init = [[
        require("modes")
        require("fullscreen")
    ]],
    steps = {
        {
            expect = function(world)
                assert(#world.unknown == 0,
                    "fullscreen.instant is a setting, not a typo: " .. tostring(world.unknown[1] and world.unknown[1].key))
            end,
        },
        { open = true, app_id = "mpv" },
        { move = { 300, 200 } },
        { key = "super+f" },
        { answer = 1 },
        { expect = at_once(1, screen, "mpv covers its monitor on the key") },
        { wait = 400 },
        { key = "super+f" },
        { answer = 1 },
        { expect = at_once(1, before, "and is back on the key") },

        { open = true, app_id = "foot" },
        { move = { 300, 200 } },
        { key = "super+f" },
        { answer = 2 },
        { wait = 120 },
        {
            expect = function(world)
                local drawn = world.windows[2].drawn
                assert(drawn.w > before.w + 1 and drawn.w < screen.w - 1,
                    "foot, not on the list, is still growing: " .. show(drawn))
            end,
        },
    },
}
