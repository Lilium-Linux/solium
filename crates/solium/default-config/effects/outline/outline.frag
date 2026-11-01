// The border: d is the signed distance to the part's outline at `radius`
// (negative inside), so the band 0 < d < width lies just outside it, its
// outer edge concentric at radius + width. It starts a pixel inside, so the
// window's own corner, cut and faded at d = 0, fades onto the border rather
// than leaving a hairline of wallpaper between them. Premultiplied.
vec4 sol_effect(vec2 uv) {
    vec2 size = sol_content.zw * sol_box_px;
    vec2 p = sol_to_content(uv);
    float d = sol_sdf_rrect(p, size, vec4(p_radius));
    float inner = clamp(d + 1.5, 0.0, 1.0);
    float outer = clamp(p_width + 0.5 - d, 0.0, 1.0);
    float a = clamp(p_color.a * inner * outer, 0.0, 1.0);
    return vec4(p_color.rgb * a, a);
}
