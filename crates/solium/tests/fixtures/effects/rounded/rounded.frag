vec4 sol_effect(vec2 uv) {
    float away = sol_sdf_rrect(uv * sol_size, sol_size, vec4(p_radius));
    return sol_tex(uv) * clamp(0.5 - away, 0.0, 1.0);
}
