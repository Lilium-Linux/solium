-- Desktop mode, `placement.wrap = "center"`: when the next `cascade` step
-- would take a new window past the edge of the work area, the new diagonal
-- starts back at the centre moved right by one `cascade.x` for each time it
-- has wrapped, so it never lands on the first window (#240).
--
-- A cascade of (120, 100) on 1920x1080 with 200x100 windows: the first
-- diagonal from the centre (860, 490) holds five; the sixth starts again at
-- (980, 490), a step right of the first window; that diagonal holds five
-- more, and the eleventh starts at (1100, 490). Opened with Russian live.
--
-- Played by `scenario::tests::each::desktop_placement_wrap_center`.

local screen = { x = 0, y = 0, w = 1920, h = 1080 }

local function show(window)
    return string.format("%d,%d %dx%d", window.x, window.y, window.w, window.h)
end

local function inside(window, area)
    return window.x >= area.x and window.y >= area.y
        and window.x + window.w <= area.x + area.w
        and window.y + window.h <= area.y + area.h
end

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

local spots = {
    { 860, 490 }, { 980, 590 }, { 1100, 690 }, { 1220, 790 }, { 1340, 890 },
    { 980, 490 }, { 1100, 590 }, { 1220, 690 }, { 1340, 790 }, { 1460, 890 },
    { 1100, 490 },
}

local steps = {
    {
        expect = function(world)
            assert(#world.unknown == 0,
                "placement.wrap is a setting, not a typo: " .. tostring(world.unknown[1] and world.unknown[1].key))
        end,
    },
}
for _ = 1, 6 do
    steps[#steps + 1] = { open = true }
end
steps[#steps + 1] = { expect = landed(6, { table.unpack(spots, 1, 6) }) }
steps[#steps + 1] = { open = true }
steps[#steps + 1] = { expect = landed(7, { table.unpack(spots, 1, 7) }) }
for _ = 8, 11 do
    steps[#steps + 1] = { open = true }
end
steps[#steps + 1] = { expect = landed(11, spots) }

return {
    -- Atrium off: this tests placement and snapping on the whole work area,
    -- which its strip would take the left of.
    user = [[ return { floating = { atrium = { enabled = false }, placement = { cascade = { x = 120, y = 100 }, wrap = "center" } } } ]],
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
