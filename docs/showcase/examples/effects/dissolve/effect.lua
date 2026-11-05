-- Dissolve: the window burns away into nothing on a violet edge, drifting a
-- little towards its target as it goes, and comes back the same way. A
-- showcase example (docs/showcase.md) of a minimise morph: copied to
-- ~/.config/solium/effects/genie/, it is what super+m plays (see fold).
--
-- The drift is the mesh below; the burn is dissolve.frag, the window's own
-- pixels through value noise against the present's progress.
return {
    api = 1,
    duration = 700,
    easing = "linear",
    grid = { along = 2, across = 2 },
    params = {
        spread = { 1.4, min = 0 },                 -- genie's; what minimise hands over
        drift = { 0.18, min = 0, max = 1 },        -- how far towards the target it moves
        shrink = { 0.08, min = 0, max = 0.5 },
        grain = { 9, min = 1, max = 64 },          -- the noise's scale, cells across the window
    },
    frag = "dissolve.frag",
    mesh = function(t, cols, rows, out)
        local f, to, p, n = t.from, t.to, t.part, 0
        local q = math.max(0, math.min(1, t.progress))
        local e = q * q * (3 - 2 * q)
        local cx = f.x + f.w / 2 + ((to.x + to.w / 2) - (f.x + f.w / 2)) * t.drift * e
        local cy = f.y + f.h / 2 + ((to.y + to.h / 2) - (f.y + f.h / 2)) * t.drift * e
        local s = 1 - t.shrink * e
        for r = 0, rows do
            local v = p.v0 + (p.v1 - p.v0) * (r / rows)
            for c = 0, cols do
                local u = p.u0 + (p.u1 - p.u0) * (c / cols)
                out[n + 1] = cx + (u - 0.5) * f.w * s
                out[n + 2] = cy + (v - 0.5) * f.h * s
                n = n + 2
            end
        end
    end,
}
