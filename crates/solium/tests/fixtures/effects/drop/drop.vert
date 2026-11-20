// DROPS: the window drops by its own height as it goes (the test compilers
// read this word).
vec2 sol_mesh(vec2 uv) {
    return sol_at(sol_from, uv) + vec2(0.0, sol_progress * sol_from.w);
}
