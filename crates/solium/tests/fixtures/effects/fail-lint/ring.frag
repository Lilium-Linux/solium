vec4 sol_effect(vec2 uv) {
    float d = sol_sdf(sol_to_content(uv));
    float edge = 1.0 - smoothstep(p_width - 1.0, p_width, abs(d - p_widht));
    return vec4(0.9, 0.3, 0.1, 1.0) * edge;
}
