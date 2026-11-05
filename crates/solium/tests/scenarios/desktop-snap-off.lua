-- `floating.snap.drag.enabled = false` (#455): a window dragged to an edge
-- previews nothing and stays where it was let go, unmarked; the keys still
-- snap and mark.
--
-- Played by `scenario::tests::each::desktop_snap_off`, on one
-- 1920x1080 monitor with nothing reserved.

return {
    -- Atrium off: this tests placement and snapping on the whole work area,
    -- which its strip would take the left of.
    user = [[ return { floating = { atrium = { enabled = false }, snap = { drag = { enabled = false } } } } ]],
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
