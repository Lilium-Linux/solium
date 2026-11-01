-- Desktop mode, `placement.policy = "cascade"`: a new window is always one
-- `cascade` step from the last window opened, wherever that one is now --
-- not from the centre, which is where `"center"` would have put the third
-- window here, on the spot the second was moved off. And when that step
-- would leave the work area, a new diagonal starts at the work area's
-- top-left plus `cascade` (#240). Opened with Russian live.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

local function show(window)
    return string.format("%d,%d %dx%d", window.x, window.y, window.w, window.h)
end

local function at(n, x, y, why)
    return function(world)
        local window = world.windows[n]
        assert(window.x == x and window.y == y,
            why .. ": window " .. n .. " at " .. x .. "," .. y .. ": " .. show(window))
    end
end

return {
    -- Atrium off: its strip takes the left of the work area, and this is the
    -- cascade's own arithmetic on the whole of it.
    user = [[ return { floating = { placement = { policy = "cascade" }, atrium = { enabled = false } } } ]],
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
        { expect = at(1, 860, 490, "the first window is centred") },
        { open = true },
        { expect = at(2, 892, 522, "one step from the first") },
        { move = { 100, 100 } },
        { open = true },
        { expect = at(3, 132, 132, "one step from the second, where it was moved to") },
        { move = { 1720, 980 } },
        { open = true },
        { expect = at(4, 32, 32, "one step from the third would leave the work area") },
    },
}
