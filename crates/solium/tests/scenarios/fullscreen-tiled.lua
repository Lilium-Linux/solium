-- A tiled window leaving fullscreen goes back into its tile with the
-- fullscreen motion, not the layout's (#49): the layout places it, and the
-- change's own glide replaces the layout's. Two windows side by side, and a
-- second-long linear fullscreen, which is half way at 500 ms where tiling's
-- 240 ms would have landed. Keys pressed with Russian live.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

local screen = { x = 0, y = 0, w = 1920, h = 1080 }
-- The second window's tile, once tiling has placed it.
local tile = nil

local function show(rect)
    return string.format("%.1f,%.1f %.1fx%.1f", rect.x, rect.y, rect.w, rect.h)
end

local function on(rect, wanted)
    return math.abs(rect.x - wanted.x) < 0.5 and math.abs(rect.y - wanted.y) < 0.5
        and math.abs(rect.w - wanted.w) < 0.5 and math.abs(rect.h - wanted.h) < 0.5
end

-- Half way between the tile and the monitor, either way: a linear glide half
-- way through its second.
local function half_way(world)
    local window = world.windows[2]
    local wide = (tile.w + screen.w) / 2
    assert(math.abs(window.drawn.w - wide) < 10,
        "half way between " .. show(tile) .. " and the monitor, about " .. wide .. " wide: "
            .. show(window.drawn))
    assert(window.transformed, "moved by a transform")
end

local function landed_on(name)
    return function(world)
        local window = world.windows[2]
        local wanted = name == "tile" and tile or screen
        assert(on(window.drawn, wanted), "landed on " .. show(wanted) .. ": " .. show(window.drawn))
        assert(not window.transformed, "and at rest it holds no transform")
    end
end

return {
    user = [[ return { fullscreen = { animate = { duration = 1000, easing = "linear" } } } ]],
    init = [[
        require("modes")
        require("tiling")
        require("fullscreen")
    ]],
    steps = {
        { open = true },
        { open = true },
        { key = "super+t" },
        { answer = 1 },
        { answer = 2 },
        { wait = 1000 },
        {
            expect = function(world)
                local window = world.windows[2]
                assert(not window.transformed, "the premise: tiled and at rest")
                assert(window.drawn.w < screen.w / 2,
                    "the premise: in half of the monitor: " .. show(window.drawn))
                tile = window.drawn
            end,
        },

        { key = "super+f" },
        { answer = 2 },
        { wait = 500 },
        { expect = half_way },
        { wait = 600 },
        { expect = landed_on("screen") },

        { key = "super+f" },
        { answer = 2 },
        { wait = 500 },
        { expect = half_way },
        { wait = 600 },
        { expect = landed_on("tile") },
    },
}
