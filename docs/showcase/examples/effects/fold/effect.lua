-- Fold: the window folds up like a map, into pleats that close towards its
-- bottom edge, and the folded strip then flies into its target. A showcase
-- example (docs/showcase.md) of a minimise morph: copied to
-- ~/.config/solium/effects/genie/, it is what super+m plays, since the
-- default configuration's taskbar.lua minimises with the effect named
-- `genie` and a folder of yours shadows the shipped one of the same name.
--
-- The geometry is the mesh below; the light on the pleats, darker as each
-- face turns away, is fold.frag, drawn in the same pass.
return {
    api = 1,
    duration = 560,
    easing = "inOutCubic",
    grid = { along = 48, across = 8 },
    params = {
        spread = { 1.4, min = 0 },                 -- genie's; what minimise hands over
        pleats = { 6, min = 2, max = 12 },
        depth = { 0.2, min = 0, max = 0.6 },       -- how much narrower a fold that goes back is
        squash = { 0.86, min = 0, max = 0.98 },    -- how much of its height folding takes away
        split = { 0.55, min = 0.1, max = 0.9 },    -- how much of the time folding takes
    },
    frag = "fold.frag",
    mesh = function(t, cols, rows, out)
        local f, to, p, n = t.from, t.to, t.part, 0
        local q = math.max(0, math.min(1, t.progress))
        local a = math.min(1, q / t.split)
        a = a * a * (3 - 2 * a)
        local b = math.max(0, (q - t.split) / (1 - t.split))
        b = b * b * (3 - 2 * b)
        local height = f.h * (1 - t.squash * a)
        local bottom = f.y + f.h
        for r = 0, rows do
            local v = p.v0 + (p.v1 - p.v0) * (r / rows)
            -- A fold line every 1/pleats: the even ones in front, at the
            -- window's width, the odd ones behind, narrower.
            local x = math.max(0, math.min(1, v)) * t.pleats
            local k = math.floor(x)
            local w = x - k
            local back = (k % 2 == 0) and w or 1 - w
            local width = 1 - t.depth * a * back
            local y = bottom - (1 - v) * height
            for c = 0, cols do
                local u = p.u0 + (p.u1 - p.u0) * (c / cols)
                local fx = f.x + f.w / 2 + (u - 0.5) * f.w * width
                local tx, ty = to.x + u * to.w, to.y + v * to.h
                out[n + 1] = fx + (tx - fx) * b
                out[n + 2] = y + (ty - y) * b
                n = n + 2
            end
        end
    end,
}
