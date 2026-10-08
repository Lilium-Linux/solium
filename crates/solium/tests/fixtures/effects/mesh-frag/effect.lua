-- A geometry file with a frag beside its mesh: the mesh draws the window
-- where it is, and the frag makes its version wait for the next compile.
-- `state::tests::real_client::a_geometry_with_a_frag_presents_before_it_compiled`.
return {
    api = 1,
    inputs = { "self" },
    frag = "effect.frag",
    grid = { 1, 1 },
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
