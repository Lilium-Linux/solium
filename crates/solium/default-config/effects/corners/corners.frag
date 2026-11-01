// The input, cut at the part's rounded outline and antialiased across it.
// Premultiplied, so cutting is scaling every channel.
vec4 sol_effect(vec2 uv) {
    vec2 size = sol_content.zw * sol_box_px;
    vec2 p = sol_to_content(uv);
    float d = sol_sdf_rrect(p, size, vec4(p_radius));
    return sol_tex(uv) * clamp(0.5 - d, 0.0, 1.0);
}
