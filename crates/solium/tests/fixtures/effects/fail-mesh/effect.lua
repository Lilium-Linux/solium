-- A geometry file whose mesh fails between its ends: it draws the window
-- where it is at progress 0 and 1, so it passes the checks at load, and
-- indexes a field nobody set anywhere between, so every frame of a flight
-- is refused.
-- `state::tests::real_client::effects_present_is_the_default_a_deform_overrides`.
return {
    api = 1,
    grid = { 1, 1 },
    mesh = function(t, cols, rows, out)
        if t.progress > 0 and t.progress < 1 then
            local nothing = t.nothing.here
        end
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
