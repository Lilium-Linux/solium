// NAN_PAST: NaN past the window's bottom edge (the test compilers read this
// word).
vec2 sol_mesh(vec2 uv) {
    vec2 at = sol_at(sol_from, uv);
    return vec2(at.x, at.y + 0.0 * sqrt(1.0 - uv.y));
}
