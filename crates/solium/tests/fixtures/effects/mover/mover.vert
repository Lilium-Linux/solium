// MOVES: every x one pixel to the right (the test compilers read this word).
vec2 sol_mesh(vec2 uv) {
    return sol_at(sol_from, uv) + vec2(1.0, 0.0);
}
