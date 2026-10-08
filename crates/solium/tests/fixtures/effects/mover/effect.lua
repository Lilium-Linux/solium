-- A genie-shaped geometry file that moves the window at rest: every x one
-- pixel to the right, at any progress, so it is refused at load.
-- `effect::geometry::tests::a_geometry_file_that_moves_the_window_at_progress_zero_is_refused_at_load`.
return {
    api = 1,
    grid = { along = 4, across = 2 },
    mesh = function(t, cols, rows, out)
        local n = 0
        for r = 0, rows do
            for c = 0, cols do
                local u, v = sol_grid(t, c, r)
                local x, y = t.from.x + u * t.from.w, t.from.y + v * t.from.h
                out[n + 1], out[n + 2] = x + 1, y
                n = n + 2
            end
        end
    end,
}
