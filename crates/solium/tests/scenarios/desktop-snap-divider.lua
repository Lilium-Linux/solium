-- Two windows snapped side by side by dragging (#455) share a seam: the scene
-- is handed one holder on it, the whole height of the screen; dragging the
-- left window's right edge along it moves both windows, as a tiled seam
-- moves, each frame, held so neither is made narrower than the shipped
-- `divider.minimum`; and the holder goes where the seam went. A third window
-- over the seam hides its holder.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`, on one
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

-- The holders the scene is handed: one, on a vertical seam at `x`, or none.
local function holder(x, dragging)
    return function(world)
        local seams = world.surfaces.snap.properties.seams or {}
        if not x then
            assert(#seams == 0, "no holder: " .. #seams)
            return
        end
        assert(#seams == 1, "one holder: " .. #seams)
        local seam = seams[1]
        assert(seam.vertical == true and seam.x == x and seam.y == 0 and seam.h == 1080
            and seam.monitor == "scenario-1",
            string.format("the holder down x = %d: %s at %d,%d %dx%d", x,
                tostring(seam.vertical), seam.x, seam.y, seam.w, seam.h))
        assert(seam.dragging == (dragging == true),
            "dragging " .. tostring(dragging) .. ": " .. tostring(seam.dragging))
    end
end

return {
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
        { expect = is(2, 892, 522, 200, 100, "the premise: placed one step from the first") },

        -- The second to the right edge, the first to the left.
        { press = { 990, 570, super = true } },
        { drag = { 1917, 570 } },
        { release = true },
        { answer = 2 },
        { wait = 500 },
        { expect = is(2, 960, 0, 960, 1080, "the right half") },
        { expect = holder(nil) },
        { press = { 900, 540, super = true } },
        { drag = { 3, 540 } },
        { release = true },
        { answer = 1 },
        { wait = 500 },
        { expect = is(1, 0, 0, 960, 1080, "the left half") },
        { expect = holder(960) },

        -- The left window's right edge, from inside it, without the
        -- modifier: its own resize border.
        { press = { 957, 540 } },
        { drag = { 1157, 540 } },
        { answer = 1 },
        { answer = 2 },
        { expect = is(1, 0, 0, 1160, 1080, "the seam moved: the left window wider") },
        { expect = is(2, 1160, 0, 760, 1080, "and the right one narrower beside it") },
        { expect = holder(1160, true) },
        { drag = { 1950, 540 } },
        { answer = 1 },
        { answer = 2 },
        { expect = is(1, 0, 0, 1800, 1080, "held at the right window's minimum") },
        { expect = is(2, 1800, 0, 120, 1080, "which is divider.minimum") },
        { drag = { 557, 540 } },
        { answer = 1 },
        { answer = 2 },
        { expect = is(1, 0, 0, 560, 1080, "and back") },
        { expect = is(2, 560, 0, 1360, 1080, "both") },
        { release = true },
        { focus = 2 },
        { expect = holder(560) },

        -- A third window, opened over the seam, hides the holder.
        { open = true },
        { move = { 460, 400 } },
        { focus = 3 },
        { expect = holder(nil) },
    },
}
