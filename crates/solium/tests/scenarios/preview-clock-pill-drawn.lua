-- The clock pill, `qml/preview/ClockPill.qml`, drawn stand-alone.
--
-- It is the one piece of the bar this harness can draw on its own: it reads
-- only `Theme` and a plain QML `Date`, with no `Solium.monitor` and none of
-- the live `Workspaces`/`Windows`/`Keyboard` models a `qt = true` scenario's
-- bare scene does not have (crates/solium/src/scenario.rs builds it with
-- `qml::Scene::for_host`, not the hosted, per-monitor kind). The rest of the
-- bar -- the pager, the chips, the layout chip -- reads those models and so
-- cannot be exercised this way; see `docs/ricing.md` for how it is checked
-- instead.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`.

-- The capsule, `Theme.surface`: opaque, a grey (red, green and blue within 2
-- of one another).
local function surface(r, g, b, a)
    return a == 255 and math.abs(r - g) <= 2 and math.abs(g - b) <= 2
end

-- Whether any opaque pixel in the box is the clock's text, `Theme.text`: a
-- light grey, well clear of the surface behind it.
local function inked(world, x0, y0, x1, y1)
    for y = y0, y1 do
        for x = x0, x1 do
            local r, g, b, a = world.pixel("scene", x, y)
            if a == 255 and r >= 200 and math.abs(r - g) <= 4 and math.abs(g - b) <= 4 then
                return true
            end
        end
    end
    return false
end

return {
    qt = true,
    steps = {
        -- A bare scene's root is sized by the host, not by its own bindings
        -- (`qml/host.cpp`'s `setWidth`/`setHeight`): unlike a pointer scene,
        -- which keeps its own size, an ordinary one -- this capsule among
        -- them -- is stretched to fill whatever size it is built at. So the
        -- size asked for here is the capsule's real one, 36 tall, wide
        -- enough for its text, and the whole canvas is the capsule: no
        -- padding around it to check is transparent.
        { scene = "preview/ClockPill.qml", size = { 140, 36 } },
        { wait = 300 },
        {
            expect = function(world)
                -- Its left edge, at half its height: with the radius equal
                -- to half the height, that is where a rounded pill is at its
                -- widest.
                local r, g, b, a = world.pixel("scene", 4, 18)
                assert(surface(r, g, b, a), string.format("the capsule: %d %d %d %d", r, g, b, a))
                assert(inked(world, 0, 10, 130, 26), "the clock's own time is drawn on it")
                -- The true corner, which the radius cuts: proof it is a
                -- rounded capsule and not a plain rectangle.
                local _, _, _, corner = world.pixel("scene", 1, 1)
                assert(corner == 0, "the corner is not rounded off")
            end,
        },
    },
}
