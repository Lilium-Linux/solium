-- `floating.snap.drag.enabled = false` (#455): a window dragged to an edge
-- previews nothing and stays where it was let go, unmarked; the keys still
-- snap and mark.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`, on one
-- 1920x1080 monitor with nothing reserved.

return {
    user = [[ return { floating = { snap = { drag = { enabled = false } } } } ]],
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
        { press = { 960, 540, super = true } },
        { drag = { 3, 540 } },
        { expect = function(world) assert(world.surfaces.snap == nil, "no snap scene") end },
        { release = true },
        {
            expect = function(world)
                local window = world.windows[1]
                assert(window.x == -97 and window.w == 200, "left where it was let go: " .. window.x)
                assert(not (window.values or {}).snapped, "and not snapped")
            end,
        },
        { key = "super+left" },
        { answer = 1 },
        {
            expect = function(world)
                local window = world.windows[1]
                assert(window.x == 0 and window.w == 960, "the key snaps")
                assert(window.values.snapped == "left", "and marks")
            end,
        },
    },
}
