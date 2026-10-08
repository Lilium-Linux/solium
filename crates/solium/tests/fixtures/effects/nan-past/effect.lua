-- A geometry file that draws the window where it is over its own unit
-- square, and writes NaN for any point past its bottom edge (v > 1): it
-- passes the checks at load, which draw only parts inside the window, and
-- its grid is refused for a menu that opens below the window.
-- `state::tests::real_client::a_present_refused_for_its_popups_alone_is_said_once`.
return {
    api = 1,
    grid = { 1, 1 },
    mesh = function(t, cols, rows, out)
        local n = 0
        for r = 0, rows do
            for c = 0, cols do
                local u, v = sol_grid(t, c, r)
                out[n + 1] = t.from.x + u * t.from.w
                out[n + 2] = t.from.y + v * t.from.h + 0 * math.sqrt(1 - v)
                n = n + 2
            end
        end
    end,
}
