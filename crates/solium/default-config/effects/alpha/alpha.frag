// Premultiplied, so scaling all four channels fades it evenly.
vec4 sol_effect(vec2 uv) {
    return sol_tex(uv) * p_amount;
}
