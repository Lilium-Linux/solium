-- Solium's effect prelude, api 1: what every effect's Lua may call. It runs
-- before the effect's own file, in the effect's own Lua, so an effect that
-- redefines one of these changes only itself.
-- `effect::geometry::tests::an_effects_lua_has_math_and_the_prelude_and_no_sol`.

-- How far through the sweep the point at (u, v) is: Rust's Axis::phase.
function sol_phase(u, v, axis)
    if axis == "down" then return 1 - v end
    if axis == "up" then return v end
    if axis == "left" then return u end
    return 1 - u
end

-- The pane's (u, v) at grid point (c, r) of a grid over t.part: the same
-- expression, in the same order, as Rust's warp::mesh_part. `t.cols` and
-- `t.rows` are set on `t` by every call.
function sol_grid(t, c, r)
    local p = t.part
    return p.u0 + (p.u1 - p.u0) * (c / t.cols), p.v0 + (p.v1 - p.v0) * (r / t.rows)
end
