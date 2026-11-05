-- Desktop mode, `placement.policy = "pointer"`: a new window is centred on
-- the pointer, kept inside the work area; a second one with the pointer
-- still there steps one `cascade` on rather than landing on the first; and
-- when that step would leave the work area, a new diagonal starts at the work
-- area's top-left plus `cascade` (#240). Opened with Russian live.
--
-- Played by `scenario::tests::each::desktop_placement_pointer`.

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
    -- Atrium off: this tests placement and snapping on the whole work area,
    -- which its strip would take the left of.
    user = [[ return { floating = { atrium = { enabled = false }, placement = { policy = "pointer" } } } ]],
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
        { pointer = { 400, 300 } },
        { open = true },
        { expect = at(1, 300, 250, "centred on the pointer") },
        { open = true },
        { expect = at(2, 332, 282, "the pointer has not moved, so one step on") },
        { pointer = { 1900, 1070 } },
        { open = true },
        { expect = at(3, 1720, 980, "centred on the pointer, kept inside the work area") },
        { open = true },
        { expect = at(4, 32, 32, "one step on would leave the work area") },
    },
}
