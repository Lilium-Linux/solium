-- Desktop mode, `placement.policy = "center"` as shipped: three, four and
-- ten windows opened in a row each land one `cascade` step (32, 32) down and
-- right of the one before, the first centred, all inside the work area and no
-- two on the same spot (#240). Before the fix the second window was pushed
-- to the work area's bottom-right corner and every later one was clamped onto
-- that same corner. Opened with Russian live.
--
-- Played by `scenario::tests::each::desktop_placement_center`.

local screen = { x = 0, y = 0, w = 1920, h = 1080 }
local step = { x = 32, y = 32 }

local function show(window)
    return string.format("%d,%d %dx%d", window.x, window.y, window.w, window.h)
end

local function inside(window, area)
    return window.x >= area.x and window.y >= area.y
        and window.x + window.w <= area.x + area.w
        and window.y + window.h <= area.y + area.h
end

local function in_a_row(count)
    return function(world)
        assert(#world.windows == count, "the premise: " .. count .. " windows open")
        local first = world.windows[1]
        assert(first.x == math.floor((screen.w - first.w) / 2)
            and first.y == math.floor((screen.h - first.h) / 2),
            "window 1 is centred on the work area: " .. show(first))
        for n = 2, count do
            local before, now = world.windows[n - 1], world.windows[n]
            assert(now.x - before.x == step.x and now.y - before.y == step.y,
                "window " .. n .. " is one cascade step from window " .. (n - 1) .. ": "
                    .. show(before) .. " then " .. show(now))
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

return {
    -- Atrium off: this tests placement and snapping on the whole work area,
    -- which its strip would take the left of.
    user = [[ return { floating = { atrium = { enabled = false } } } ]],
    init = [[
        require("modes")
        require("open")
        require("workspaces")
        require("tiling")
        require("scrolling")
        require("direction")
        require("floating")
    ]],
    steps = {
        { open = true },
        { open = true },
        { open = true },
        { expect = in_a_row(3) },
        { open = true },
        { expect = in_a_row(4) },
        { open = true },
        { open = true },
        { open = true },
        { open = true },
        { open = true },
        { open = true },
        { expect = in_a_row(10) },
    },
}
