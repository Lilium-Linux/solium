-- The snap scene, `qml/snap/snap.qml`, as `lua/snap.lua` declares it (#455):
-- built showing nothing; handed a zone, a preview of it fades in, inset by
-- `preview.inset`; handed the next zone, it moves there; handed none, it
-- fades out; and a seam's holder is a pill across the seam's middle.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`, in
-- software. A scene here is on no monitor, so `Solium.monitor.name` is empty
-- and what is handed for it names "".

local function alpha(world, x, y)
    local _, _, _, a = world.pixel("scene", x, y)
    return a
end

return {
    qt = true,
    steps = {
        { scene = "snap/snap.qml", size = { 400, 300 } },
        { wait = 50 },
        { expect = function(world) assert(alpha(world, 100, 150) == 0, "built showing nothing") end },

        { set = { zone = { monitor = "", kind = "left", x = 0, y = 0, w = 200, h = 300 } } },
        { wait = 400 },
        {
            expect = function(world)
                assert(alpha(world, 100, 150) > 0, "the preview fills the left half")
                assert(alpha(world, 3, 150) == 0, "inset from the zone's edge")
                assert(alpha(world, 300, 150) == 0, "and nothing on the right")
            end,
        },

        { set = { zone = { monitor = "", kind = "top_right", x = 200, y = 0, w = 200, h = 150 } } },
        { wait = 400 },
        {
            expect = function(world)
                assert(alpha(world, 300, 75) > 0, "moved to the top right quarter")
                assert(alpha(world, 100, 150) == 0, "and gone from the left")
            end,
        },

        { set = { zone = { monitor = "elsewhere", kind = "left", x = 0, y = 0, w = 200, h = 300 } } },
        { wait = 400 },
        { expect = function(world) assert(alpha(world, 100, 150) == 0, "another monitor's zone is not drawn here") end },

        { set = { zone = false } },
        { wait = 400 },
        { expect = function(world) assert(alpha(world, 300, 75) == 0, "faded out") end },

        { set = { seams = { { monitor = "", vertical = true, x = 200, y = 0, w = 0, h = 300, dragging = false } } } },
        { wait = 50 },
        {
            expect = function(world)
                local r, g, b, a = world.pixel("scene", 200, 150)
                assert(a > 0 and math.abs(r - g) <= 2 and math.abs(g - b) <= 2,
                    string.format("a grey pill across the seam: %d %d %d %d", r, g, b, a))
                assert(alpha(world, 200, 20) == 0, "across its middle, not its whole length")
                assert(alpha(world, 210, 150) == 0, "and thin")
            end,
        },
    },
}
