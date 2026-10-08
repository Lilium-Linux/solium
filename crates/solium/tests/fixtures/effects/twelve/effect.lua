-- A geometry file with twelve numeric params, `a` to `l`, that draws the
-- window where it is: more than `effects.limits.params`' default 8.
-- `state::tests::real_client::a_geometry_effect_with_twelve_params_presents_under_a_raised_limit`.
return {
    api = 1,
    grid = { 1, 1 },
    params = {
        a = { 0 }, b = { 0 }, c = { 0 }, d = { 0 }, e = { 0 }, f = { 0 },
        g = { 0 }, h = { 0 }, i = { 0 }, j = { 0 }, k = { 0 }, l = { 0 },
    },
    mesh = function(t, cols, rows, out)
        local n = 0
        for r = 0, rows do
            for c = 0, cols do
                local u, v = sol_grid(t, c, r)
                out[n + 1], out[n + 2] = t.from.x + u * t.from.w, t.from.y + v * t.from.h
                n = n + 2
            end
        end
    end,
}
