vec4 sol_effect(vec2 uv) {
    vec4 colour = sol_tex(uv);
    vec2 inside = (uv - sol_content.xy) / sol_content.zw;
    if (any(lessThan(inside, vec2(0.0))) || any(greaterThan(inside, vec2(1.0)))) return colour;
    float p = sol_clamped;
    vec2 size = sol_content.zw * sol_box_px * mix(vec2(1.0), vec2(p_icon_x, p_icon_y), p);
    float ratio = size.x / max(size.y, 0.0001);
    float iconRatio = (p_icon_x * sol_content.z * sol_box_px.x) / max(p_icon_y * sol_content.w * sol_box_px.y, 0.0001);
    float toward = mix(ratio, iconRatio, 1.0 - pow(1.0 - p, 3.0));
    if (toward < ratio) size.x = size.y * toward; else size.y = size.x / max(toward, 0.0001);
    float radius = mix(p_rest, p_corner, p) + p_peak * sin(3.14159265359 * p);
    radius = min(radius, min(size.x, size.y) * 0.5);
    float distance = sol_sdf_rrect(inside * size, size, vec4(radius));
    return colour * clamp(0.5 - distance, 0.0, 1.0);
}
