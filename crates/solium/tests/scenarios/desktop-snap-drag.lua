-- Snapping by dragging (#455), on the shipped configuration with a real
-- client and the drag modifier: a preview of the zone the pointer is in --
-- the left edge's half, a corner's quarter, the top edge's whole work area,
-- none in the middle -- moving as the pointer moves, gone when it leaves; let
-- go on the right edge, the window is the right half and marked snapped;
-- taken again, it stays put for a few pixels and then comes off at the size
-- it had, under the pointer, no longer marked.
--
-- Played by `scenario::tests::each::desktop_snap_drag`, on one
-- 1920x1080 monitor with nothing reserved.

local function show(window)
    return string.format("%d,%d %dx%d", window.x, window.y, window.w, window.h)
end

local function is(n, x, y, w, h, why)
    return function(world)
        local window = world.windows[n]
        assert(window.x == x and window.y == y and window.w == w and window.h == h,
            why .. ": window " .. n .. " at " .. x .. "," .. y .. " " .. w .. "x" .. h .. ": "
                .. show(window))
    end
end

-- The preview the scene is handed: `kind` at that rect, or none.
local function previews(kind, x, y, w, h)
    return function(world)
        local surface = world.surfaces.snap
        assert(surface, "the snap scene is declared")
        local zone = surface.properties.zone
        if not kind then
            assert(zone == false, "no preview: " .. tostring(zone and zone.kind))
            return
        end
        assert(type(zone) == "table", "a preview of " .. kind)
        assert(zone.monitor == "scenario-1" and zone.kind == kind
            and zone.x == x and zone.y == y and zone.w == w and zone.h == h,
            string.format("the preview of %s at %d,%d %dx%d: %s %s at %d,%d %dx%d", kind, x, y, w, h,
                tostring(zone.monitor), tostring(zone.kind), zone.x, zone.y, zone.w, zone.h))
    end
end

local function snapped(n, kind)
    return function(world)
        local values = world.windows[n].values or {}
        assert(values.snapped == kind,
            "window " .. n .. " snapped " .. tostring(kind) .. ": " .. tostring(values.snapped))
    end
end

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
        { expect = is(1, 860, 490, 200, 100, "the premise: placed in the middle") },
        { expect = previews(nil) },

        -- Taken a hundred in and fifty down, by the modifier.
        { press = { 960, 540, super = true } },
        { drag = { 500, 540 } },
        { expect = previews(nil) },
        { drag = { 3, 540 } },
        { expect = previews("left", 0, 0, 960, 1080) },
        { drag = { 3, 60 } },
        { expect = previews("top_left", 0, 0, 960, 540) },
        { drag = { 3, 1070 } },
        { expect = previews("bottom_left", 0, 540, 960, 540) },
        { drag = { 900, 3 } },
        { expect = previews("maximize", 0, 0, 1920, 1080) },
        { drag = { 900, 500 } },
        { expect = previews(nil) },
        { drag = { 1917, 540 } },
        { expect = previews("right", 960, 0, 960, 1080) },
        { release = true },
        { expect = previews(nil) },
        { answer = 1 },
        { wait = 500 },
        { expect = is(1, 960, 0, 960, 1080, "let go on the right edge: the right half") },
        { expect = snapped(1, "right") },

        -- Taken again: held for a few pixels, then off at its own size,
        -- under the pointer, as far along as it was taken.
        { press = { 1440, 540, super = true } },
        { drag = { 1442, 541 } },
        { expect = is(1, 960, 0, 960, 1080, "held within the threshold") },
        { expect = snapped(1, "right") },
        { drag = { 1300, 600 } },
        { answer = 1 },
        {
            expect = function(world)
                local window = world.windows[1]
                assert(window.w == 200 and window.h == 100,
                    "off at the size it had before: " .. show(window))
                assert(math.abs(window.x - (1300 - 100)) <= 1,
                    "half way along, as it was taken: " .. show(window))
                assert(window.y <= 600 and window.y + window.h >= 600,
                    "under the pointer: " .. show(window))
            end,
        },
        { expect = snapped(1, false) },
        { expect = previews(nil) },
        { release = true },
        { expect = snapped(1, false) },
    },
}
