-- Desktop mode: a window nearly as tall, or nearly as wide, as the work area
-- still gets a spot of its own; with the default `wrap = "corner"`, nothing
-- piles up on the centre (#240's review).
--
-- On a 1920x110 monitor the 200x100 windows have 10 pixels of room below the
-- centre's (860, 5), less than one `cascade` step, so the corner diagonals
-- start as far down as that room allows: (32, 10), then (64, 10) and (96, 10).
-- On a 210x1080 monitor beside it they have 10 pixels of room across, so no
-- diagonal has room for even its first spot, and each window after the centre
-- (1925, 490) takes the next free spot of the `cascade` grid down the left
-- edge: (1920, 0), (1920, 32), (1920, 64). Opened with Russian live.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

local short = { x = 0, y = 0, w = 1920, h = 110 }
local narrow = { x = 1920, y = 0, w = 210, h = 1080 }

local function show(window)
    return string.format("%d,%d %dx%d", window.x, window.y, window.w, window.h)
end

local function inside(window, area)
    return window.x >= area.x and window.y >= area.y
        and window.x + window.w <= area.x + area.w
        and window.y + window.h <= area.y + area.h
end

-- Windows `from` to `from + #spots - 1` at each spot in `spots`, in order,
-- inside `area`, and no two on the same spot.
local function landed(from, spots, area)
    return function(world)
        assert(#world.windows == from + #spots - 1,
            "the premise: " .. (from + #spots - 1) .. " windows open")
        for k, spot in ipairs(spots) do
            local n = from + k - 1
            local window = world.windows[n]
            assert(window.x == spot[1] and window.y == spot[2],
                "window " .. n .. " at " .. spot[1] .. "," .. spot[2] .. ": " .. show(window))
            assert(inside(window, area), "window " .. n .. " is inside its monitor: " .. show(window))
            for m = from, n - 1 do
                local other = world.windows[m]
                assert(other.x ~= window.x or other.y ~= window.y,
                    "windows " .. m .. " and " .. n .. " share a spot: " .. show(window))
            end
        end
    end
end

local steps = { { pointer = { 960, 55 } } }
for _ = 1, 4 do
    steps[#steps + 1] = { open = true }
end
steps[#steps + 1] = {
    expect = landed(1, { { 860, 5 }, { 32, 10 }, { 64, 10 }, { 96, 10 } }, short),
}
steps[#steps + 1] = { pointer = { 2025, 540 } }
for _ = 1, 4 do
    steps[#steps + 1] = { open = true }
end
steps[#steps + 1] = {
    expect = landed(5, { { 1925, 490 }, { 1920, 0 }, { 1920, 32 }, { 1920, 64 } }, narrow),
}

return {
    -- Atrium off: this tests placement and snapping on the whole work area,
    -- which its strip would take the left of.
    user = [[ return { floating = { atrium = { enabled = false } } } ]],
    monitors = { { 1920, 110 }, { 210, 1080 } },
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
