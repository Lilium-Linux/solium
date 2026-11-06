// The band, in the part's own pixels, as the shipped shade draws its edge:
// d is the signed distance to the rounded outline, so it follows the
// corners. A neon line on the outline and a glow reaching `width` in from
// it, coloured round the outline by angle (violet, cyan, magenta) and
// drifting with `phase`, and a white-hot head of light at `phase` that
// trails `tail` of the way round behind it. Premultiplied, and antialiased
// across the outline.
vec4 sol_effect(vec2 uv) {
    vec2 size = sol_content.zw * sol_box_px;
    vec2 p = sol_to_content(uv);
    float d = sol_sdf_rrect(p, size, sol_radii);
    float cover = clamp(0.5 - d, 0.0, 1.0);
    if (cover <= 0.0 || p_width <= 0.0) return vec4(0.0);
    float x = max(-d, 0.0);
    float line = 1.0 - smoothstep(2.0, 5.0, x);
    float band = pow(clamp(1.0 - x / p_width, 0.0, 1.0), 1.2);
    vec2 c = p - size * 0.5;
    float angle = atan(c.y, c.x) / 6.2831853 + 0.5;
    float hue = fract(angle - p_phase) * 3.0;
    vec3 violet = vec3(0.62, 0.35, 1.00);
    vec3 cyan = vec3(0.20, 0.95, 1.00);
    vec3 magenta = vec3(1.00, 0.25, 0.80);
    vec3 colour = hue < 1.0 ? mix(violet, cyan, smoothstep(0.0, 1.0, hue))
        : hue < 2.0 ? mix(cyan, magenta, smoothstep(0.0, 1.0, hue - 1.0))
        : mix(magenta, violet, smoothstep(0.0, 1.0, hue - 2.0));
    float trail = fract(p_phase - angle);
    float head = pow(clamp(1.0 - trail / p_tail, 0.0, 1.0), 3.0);
    colour = mix(colour, vec3(1.0), head * 0.6);
    float glow = 0.7 + 0.3 * head;
    float a = clamp(max(line, band * 0.9) * glow * p_strength, 0.0, 1.0) * cover;
    return vec4(colour * a, a);
}
