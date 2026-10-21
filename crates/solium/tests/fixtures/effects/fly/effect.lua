-- A geometry file whose pixels are another effect's frag: the mesh draws
-- the window where it is, and `rounded`'s frag is drawn in the same warp.
-- Its own param `lift` is the mesh's; `radius` is `rounded`'s, and a
-- `sol.present` deform gives either.
-- `state::tests::real_client::a_present_through_a_pixels_effect_reads_the_params_it_blends`,
-- `effect::host::tests::a_geometrys_pixels_compile_in_the_warp_and_follow_their_frag`.
return {
    api = 1,
    pixels = "rounded",
    grid = { 1, 1 },
    params = { lift = { 0.0 } },
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
