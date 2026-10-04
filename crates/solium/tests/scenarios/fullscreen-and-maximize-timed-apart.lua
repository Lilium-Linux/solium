-- `fullscreen.animate` and `maximize.animate` each time their own change
-- (#49): a second-long linear fullscreen is half way at 500 ms while a 200 ms
-- maximise has long landed. Keys pressed with Russian live.
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

return {
    user = [[
        return {
            fullscreen = { animate = { duration = 1000, easing = "linear" } },
            maximize = { animate = { duration = 200, easing = "linear" } },
        }
    ]],
    init = [[
        require("modes")
        require("fullscreen")
    ]],
    steps = {
        { open = true },
        { move = { 300, 200 } },
        { key = "super+f" },
        { answer = 1 },
        { wait = 500 },
        {
            expect = function(world)
                local drawn = world.windows[1].drawn
                local half = (before.w + screen.w) / 2
                assert(math.abs(drawn.w - half) < 40,
                    "half way through a second at 500 ms, about " .. half .. " wide: " .. show(drawn))
            end,
        },
        { wait = 600 },
        { key = "super+f" },
        { answer = 1 },
        { wait = 1100 },

        { key = "super+shift+m" },
        { answer = 1 },
        { wait = 300 },
        {
            expect = function(world)
                local window = world.windows[1]
                assert(on(window.drawn, screen) and not window.transformed,
                    "a 200 ms maximise has landed by 300 ms: " .. show(window.drawn))
            end,
        },
    },
}
