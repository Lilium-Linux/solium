-- Desktop mode's snap keys, pressed through the real input path with
-- Russian live, on a window placed by `placement` (#222, kept by #240):
-- `super+left` and `super+right` snap the focused window to a half,
-- `super+up` from a half goes to its top quarter, and `super+down` puts it
-- back where it was placed. Each snap marks the window for its frame
-- (`snapped`, #455), and `super+down` takes the mark off.
--
-- Played by `scenario::tests::each::desktop_snap_keys`.

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

local function snapped(kind)
    return function(world)
        local values = world.windows[2].values or {}
        assert(values.snapped == kind, "snapped " .. tostring(kind) .. ": " .. tostring(values.snapped))
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
        { open = true },
        { expect = is(2, 892, 522, 200, 100, "the premise: placed one step from the first") },
        { key = "super+left" },
        { answer = 2 },
        { expect = is(2, 0, 0, 960, 1080, "super+left: the left half") },
        { expect = snapped("left") },
        { key = "super+right" },
        { answer = 2 },
        { expect = is(2, 960, 0, 960, 1080, "super+right: the right half") },
        { expect = snapped("right") },
        { key = "super+up" },
        { answer = 2 },
        { expect = is(2, 960, 0, 960, 540, "super+up from a half: its top quarter") },
        { expect = snapped("top_right") },
        { key = "super+down" },
        { answer = 2 },
        { expect = is(2, 892, 522, 200, 100, "super+down: back where it was placed") },
        { expect = snapped(false) },
    },
}
