-- Snap zones on two monitors side by side (#455): only the outer edges snap.
-- Dragged across the edge the two share -- from either side of it -- a
-- window previews nothing; the top edge beside it is still the top edge,
-- maximising that monitor's work area, without a corner where the shared
-- edge is; the right monitor's outer right edge is its right half, and let
-- go there the window goes there.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`, on two
-- 1920x1080 monitors side by side, `scenario-1` on the left.

local function previews(monitor, kind, x, y, w, h)
    return function(world)
        local zone = world.surfaces.snap.properties.zone
        if not monitor then
            assert(zone == false, "no preview: " .. tostring(zone and zone.kind))
            return
        end
        assert(type(zone) == "table", "a preview of " .. kind)
        assert(zone.monitor == monitor and zone.kind == kind
            and zone.x == x and zone.y == y and zone.w == w and zone.h == h,
            string.format("the preview of %s on %s at %d,%d %dx%d: %s %s at %d,%d %dx%d", kind, monitor,
                x, y, w, h, tostring(zone.monitor), tostring(zone.kind), zone.x, zone.y, zone.w, zone.h))
    end
end

return {
    monitors = { { 1920, 1080 }, { 1920, 1080 } },
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
        { press = { 960, 540, super = true } },
        { drag = { 1700, 540 } },
        { drag = { 1918, 540 } },
        { expect = previews(nil) },
        { drag = { 1921, 540 } },
        { expect = previews(nil) },
        { drag = { 1917, 2 } },
        { expect = previews("scenario-1", "maximize", 0, 0, 1920, 1080) },
        { drag = { 1925, 2 } },
        { expect = previews("scenario-2", "maximize", 0, 0, 1920, 1080) },
        { drag = { 3837, 540 } },
        -- In the right monitor's own pixels: its right half.
        { expect = previews("scenario-2", "right", 960, 0, 960, 1080) },
        { release = true },
        { answer = 1 },
        { wait = 500 },
        {
            expect = function(world)
                local window = world.windows[1]
                assert(window.x == 2880 and window.y == 0 and window.w == 960 and window.h == 1080,
                    string.format("the right monitor's right half: %d,%d %dx%d",
                        window.x, window.y, window.w, window.h))
                assert(window.values.snapped == "right", "snapped right")
            end,
        },
    },
}
