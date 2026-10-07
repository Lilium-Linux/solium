-- super+f grows the focused window to cover its monitor and shrinks it back,
-- and super+shift+m maximises and restores it the same way (#49): pressed
-- with Russian live, through the real input path, under the shipped
-- `fullscreen` and `maximize` settings, 260 and 220 ms on `outCubic`. The
-- application answers each size it is told, as one does. `drawn` is the
-- rectangle the window is drawn at, on the compositor's clock.
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

-- Strictly part of the way from `from` to `to`, on each of the four numbers.
local function part_way(rect, from, to)
    local function between(value, a, b)
        return (value - a) * (b - a) > 0 and (value - b) * (a - b) > 0
    end
    return between(rect.x, from.x, to.x) and between(rect.y, from.y, to.y)
        and between(rect.w, from.w, to.w) and between(rect.h, from.h, to.h)
end

local function moving(from, to)
    return function(world)
        local window = world.windows[1]
        assert(part_way(window.drawn, from, to),
            "part of the way from " .. show(from) .. " to " .. show(to) .. ": " .. show(window.drawn))
        assert(window.transformed, "moved by a transform")
    end
end

local function landed(wanted)
    return function(world)
        local window = world.windows[1]
        assert(on(window.drawn, wanted), "landed on " .. show(wanted) .. ": " .. show(window.drawn))
        assert(not window.transformed, "and at rest it holds no transform")
    end
end

return {
    init = [[
        require("modes")
        require("fullscreen")
    ]],
    steps = {
        { open = true },
        { move = { 300, 200 } },
        { expect = landed(before) },

        { key = "super+f" },
        { answer = 1 },
        { wait = 120 },
        { expect = moving(before, screen) },
        { wait = 300 },
        { expect = landed(screen) },

        { key = "super+f" },
        { answer = 1 },
        { wait = 120 },
        { expect = moving(screen, before) },
        { wait = 300 },
        { expect = landed(before) },

        { key = "super+shift+m" },
        { answer = 1 },
        { wait = 120 },
        { expect = moving(before, screen) },
        { wait = 300 },
        { expect = landed(screen) },

        { key = "super+shift+m" },
        { answer = 1 },
        { wait = 120 },
        { expect = moving(screen, before) },
        { wait = 300 },
        { expect = landed(before) },
    },
}
