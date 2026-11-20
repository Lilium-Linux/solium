// FAILS_BETWEEN: between its ends it places no point (the test compilers
// read this word; a GPU reads the NaN).
vec2 sol_mesh(vec2 uv) {
    if (sol_progress > 0.0 && sol_progress < 1.0) {
        return vec2(sqrt(-1.0));
    }
    return sol_at(sol_from, uv);
}
