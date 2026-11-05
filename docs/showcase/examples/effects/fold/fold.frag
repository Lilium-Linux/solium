// The light on the pleats: each face darkens as it turns away from you,
// towards the fold behind it, as much as folding has gone. `sol_to_pane`
// gives the window's own (u, v), which the mesh folded by.
vec4 sol_effect(vec2 uv) {
    vec4 c = sol_tex(uv);
    float a = clamp(sol_clamped / p_split, 0.0, 1.0);
    a = a * a * (3.0 - 2.0 * a);
    float x = clamp(sol_to_pane(uv).y, 0.0, 1.0) * p_pleats;
    float k = floor(x);
    float w = x - k;
    float back = mod(k, 2.0) < 0.5 ? w : 1.0 - w;
    float light = 1.0 - a * (0.5 * back + (mod(k, 2.0) < 0.5 ? 0.0 : 0.12));
    return vec4(c.rgb * light, c.a);
}
