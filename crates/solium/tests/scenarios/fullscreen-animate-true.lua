-- `animate = true` is the shipped motion, not an instant change: `true` is
-- what you write to turn the animation back on after `false`, so going
-- fullscreen still grows and maximising still grows (#49). Keys pressed with
-- Russian live.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

local before = { x = 300, y = 200, w = 200, h = 100 }
local screen = { x = 0, y = 0, w = 1920, h = 1080 }

local function show(rect)
    return string.format("%.1f,%.1f %.1fx%.1f", rect.x, rect.y, rect.w, rect.h)
end

local function growing(what)
    return function(world)
        local drawn = world.windows[1].drawn
        assert(drawn.w > before.w + 1 and drawn.w < screen.w - 1,
            what .. " still grows with animate = true: " .. show(drawn))
    end
end

return {
    user = [[
        return {
            fullscreen = { animate = true },
            maximize = { animate = true },
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
        { wait = 100 },
        { expect = growing("going fullscreen") },
        { wait = 400 },
        { key = "super+f" },
        { answer = 1 },
        { wait = 400 },

        { key = "super+shift+m" },
        { answer = 1 },
        { wait = 100 },
        { expect = growing("maximising") },
        {
            expect = function(world)
                assert(#world.unknown == 0, "animate = true is a setting")
            end,
        },
    },
}
