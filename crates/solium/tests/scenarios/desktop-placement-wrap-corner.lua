-- Desktop mode: when the next `cascade` step would take a new window past the
-- edge of the work area, a new diagonal starts at the work area's top-left
-- plus `cascade` (`placement.wrap = "corner"`, the default), and once that
-- one fills too, one `cascade.x` further right. Nothing piles up on the last
-- spot that fit (#240).
--
-- A cascade of (120, 100) on 1920x1080 with 200x100 windows: the first
-- diagonal from the centre (860, 490) holds five; the sixth starts again at
-- (120, 100); that diagonal holds nine, down to (1080, 900); the fifteenth
-- starts the next one at (240, 100). The step is not the shipped (32, 32), so
-- this is also `cascade` being read. Opened with Russian live.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

local screen = { x = 0, y = 0, w = 1920, h = 1080 }

local function show(window)
    return string.format("%d,%d %dx%d", window.x, window.y, window.w, window.h)
end

local function inside(window, area)
    return window.x >= area.x and window.y >= area.y
        and window.x + window.w <= area.x + area.w
        and window.y + window.h <= area.y + area.h
end

-- Window `n` of `count` at each spot in `spots`, in order, and the lot inside
-- the work area with no two on the same spot.
local function landed(count, spots)
    return function(world)
        assert(#world.windows == count, "the premise: " .. count .. " windows open")
        for n, spot in ipairs(spots) do
            local window = world.windows[n]
            assert(window.x == spot[1] and window.y == spot[2],
                "window " .. n .. " at " .. spot[1] .. "," .. spot[2] .. ": " .. show(window))
        end
        for n, window in ipairs(world.windows) do
            assert(inside(window, screen), "window " .. n .. " is inside the work area: " .. show(window))
            for m = 1, n - 1 do
                local other = world.windows[m]
                assert(other.x ~= window.x or other.y ~= window.y,
                    "windows " .. m .. " and " .. n .. " share a spot: " .. show(window))
            end
        end
    end
end

local first = { { 860, 490 }, { 980, 590 }, { 1100, 690 }, { 1220, 790 }, { 1340, 890 } }

local function and_then(more)
    local all = {}
    for _, spot in ipairs(first) do
        all[#all + 1] = spot
    end
    for _, spot in ipairs(more) do
        all[#all + 1] = spot
    end
    return all
end

local steps = {}
for _ = 1, 6 do
    steps[#steps + 1] = { open = true }
end
steps[#steps + 1] = { expect = landed(6, and_then({ { 120, 100 } })) }
steps[#steps + 1] = { open = true }
steps[#steps + 1] = { expect = landed(7, and_then({ { 120, 100 }, { 240, 200 } })) }
for _ = 8, 15 do
    steps[#steps + 1] = { open = true }
end
steps[#steps + 1] = {
    expect = landed(15, and_then({
        { 120, 100 }, { 240, 200 }, { 360, 300 }, { 480, 400 }, { 600, 500 },
        { 720, 600 }, { 840, 700 }, { 960, 800 }, { 1080, 900 },
        { 240, 100 },
    })),
}

return {
    -- Atrium off: this tests placement and snapping on the whole work area,
    -- which its strip would take the left of.
    user = [[ return { floating = { atrium = { enabled = false }, placement = { cascade = { x = 120, y = 100 } } } } ]],
    init = [[
        require("modes")
        require("open")
        require("workspaces")
        require("tiling")
        require("scrolling")
        require("direction")
        require("floating")
    ]],
    steps = steps,
}
