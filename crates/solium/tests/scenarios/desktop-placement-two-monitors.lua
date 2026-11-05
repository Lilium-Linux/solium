-- Desktop mode on two monitors: a new window lands on the work area of the
-- monitor the pointer is on, centred there and stepping by `cascade` from
-- the windows already on that monitor -- the other monitor's windows take
-- no spot on it (#240). Opened with Russian live.
--
-- Played by `scenario::tests::each::desktop_placement_two_monitors`.

local left = { x = 0, y = 0, w = 1920, h = 1080 }
local right = { x = 1920, y = 0, w = 1280, h = 1024 }

local function show(window)
    return string.format("%d,%d %dx%d", window.x, window.y, window.w, window.h)
end

local function inside(window, area)
    return window.x >= area.x and window.y >= area.y
        and window.x + window.w <= area.x + area.w
        and window.y + window.h <= area.y + area.h
end

local function at(n, x, y, area, why)
    return function(world)
        local window = world.windows[n]
        assert(window.x == x and window.y == y,
            why .. ": window " .. n .. " at " .. x .. "," .. y .. ": " .. show(window))
        assert(inside(window, area), why .. ": window " .. n .. " is inside its monitor: " .. show(window))
    end
end

return {
    -- Atrium off: this tests placement and snapping on the whole work area,
    -- which its strip would take the left of.
    user = [[ return { floating = { atrium = { enabled = false } } } ]],
    monitors = { { 1920, 1080 }, { 1280, 1024 } },
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
        { pointer = { 960, 540 } },
        { open = true },
        { open = true },
        { expect = at(1, 860, 490, left, "centred on the left monitor") },
        { expect = at(2, 892, 522, left, "one step on") },
        { pointer = { 2560, 512 } },
        { open = true },
        { expect = at(3, 2460, 462, right, "centred on the right monitor, where the pointer is") },
        { open = true },
        { expect = at(4, 2492, 494, right, "one step on, from the right monitor's own first window") },
        { pointer = { 960, 540 } },
        { open = true },
        { expect = at(5, 924, 554, left, "back on the left monitor, the next step there") },
    },
}
