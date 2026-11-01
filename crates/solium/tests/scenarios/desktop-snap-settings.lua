-- `floating.snap.drag`'s numbers change what they say (#455): a wider
-- `edge` makes a zone of a point thirty pixels in; `corner = 0` leaves no
-- quarters; `top = false` takes the top edge's maximise away; a larger
-- `divider.minimum` stops a seam sooner; and `motion`, which the shipped file
-- leaves to `tiling.snap`, is a setting `--check` knows. `enabled = false` is
-- `desktop-snap-off.lua`.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`, on one
-- 1920x1080 monitor with nothing reserved.

local function zone(world)
    return world.surfaces.snap.properties.zone
end

local function kind(expected)
    return function(world)
        local z = zone(world)
        if not expected then
            assert(z == false, "no preview: " .. tostring(z and z.kind))
        else
            assert(type(z) == "table" and z.kind == expected,
                "a preview of " .. expected .. ": " .. tostring(z and z.kind))
        end
    end
end

return {
    user = [[
        return {
            floating = {
                snap = {
                    drag = {
                        edge = 40,
                        corner = 0,
                        top = false,
                        motion = { duration = 0 },
                        divider = { minimum = 300 },
                    },
                },
            },
        }
    ]],
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
        { expect = function(world) assert(#world.unknown == 0, "every setting is known") end },
        { open = true },
        { open = true },
        { press = { 990, 570, super = true } },
        { drag = { 30, 540 } },
        { expect = kind("left") },
        { drag = { 3, 3 } },
        { expect = kind("left") },
        { drag = { 900, 3 } },
        { expect = kind(nil) },
        { drag = { 1890, 540 } },
        { expect = kind("right") },
        { release = true },
        { answer = 2 },
        { wait = 500 },
        { press = { 900, 540, super = true } },
        { drag = { 3, 540 } },
        { release = true },
        { answer = 1 },
        { wait = 500 },
        { press = { 957, 540 } },
        { drag = { 1857, 540 } },
        { answer = 1 },
        { answer = 2 },
        {
            expect = function(world)
                local a, b = world.windows[1], world.windows[2]
                assert(a.w == 1620 and b.x == 1620 and b.w == 300,
                    string.format("held at divider.minimum = 300: %dx%d, %d %dx%d", a.w, a.h, b.x, b.w, b.h))
            end,
        },
        { release = true },
    },
}
