-- The genie: the window pulled into its target like a sheet through a
-- letterbox, the edge nearest the target first. Held to Solium's Rust genie
-- (crates/effects, Deform::Genie) within 1e-9 on all four axes, at every
-- spread: effect::geometry::tests::the_genie_folder_matches_deform_genie_on_all_four_axes.
--
-- `t.part` is the part of the window's unit square this grid covers: the
-- window itself, or more of it (a shadow, a menu past its edge), so every
-- point's (u, v) is the window's own, the expression sol_grid gives:
-- effect::geometry::tests::the_genie_folder_matches_past_the_window.

-- How far each step across the sweep has gone, one per row (or per column
-- when the window is pulled sideways); made once and reused, so a frame
-- makes no garbage.
local e = {}

return {
    api = 1,
    duration = 520,
    easing = "inOutCubic",
    grid = { along = 48, across = 8 },
    params = { spread = { 1.4, min = 0 } },
    mesh = function(t, cols, rows, out)
        local f, to, sp, p, n = t.from, t.to, t.spread, t.part, 0
        local across = t.axis == "left" or t.axis == "right"
        local steps, lo, hi = rows, p.v0, p.v1
        if across then steps, lo, hi = cols, p.u0, p.u1 end
        -- Each step runs its own copy of the pull, the ones furthest from
        -- the target starting `spread` later, so the tail is drawn out
        -- behind the lead; smoothstepped, so the sheet arrives without a
        -- crease.
        for k = 0, steps do
            local w = lo + (hi - lo) * (k / steps)
            local s = math.min(math.max(t.progress * (1 + sp) - sol_phase(w, w, t.axis) * sp, 0), 1)
            e[k] = s * s * (3 - 2 * s)
        end
        for r = 0, rows do
            local v = p.v0 + (p.v1 - p.v0) * (r / rows)
            for c = 0, cols do
                local u, k = p.u0 + (p.u1 - p.u0) * (c / cols), across and c or r
                local x, y = f.x + u * f.w, f.y + v * f.h
                out[n + 1], out[n + 2], n = x + (to.x + u * to.w - x) * e[k], y + (to.y + v * to.h - y) * e[k], n + 2
            end
        end
    end,
}
