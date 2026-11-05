-- Snapping is the floating desktop's (#455): with tiling in charge the snap
-- scene is gone, a window dragged to the left edge previews nothing, and let
-- go there it is not snapped -- tiling puts it back in its tile.
--
-- Played by `scenario::tests::each::desktop_snap_tiling`, on one
-- 1920x1080 monitor with nothing reserved.

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
        { expect = function(world) assert(world.surfaces.snap, "floating: the snap scene") end },
        { key = "super+t" },
        { answer = 1 },
        { answer = 2 },
        { expect = function(world) assert(world.surfaces.snap == nil, "tiling: no snap scene") end },
        { press = { 1500, 540, super = true } },
        { drag = { 3, 540 } },
        { expect = function(world) assert(world.surfaces.snap == nil, "still none while dragging") end },
        { release = true },
        { answer = 1 },
        { answer = 2 },
        { wait = 500 },
        {
            expect = function(world)
                for n = 1, 2 do
                    local values = world.windows[n].values or {}
                    assert(not values.snapped, "window " .. n .. " is not snapped")
                end
            end,
        },
        -- And back to floating, the scene is back.
        { key = "super+t" },
        { expect = function(world) assert(world.surfaces.snap, "floating again: the snap scene") end },
    },
}
