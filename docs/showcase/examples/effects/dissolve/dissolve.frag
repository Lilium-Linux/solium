// The burn: a pixel goes once the present's progress passes the noise at its
// place in the window, the edge nearest the bottom first, and the band just
// before it goes glows violet. Premultiplied, as every pass is.
vec4 sol_effect(vec2 uv) {
    vec4 c = sol_tex(uv);
    vec2 w = sol_to_pane(uv);
    float n = sol_noise(w * vec2(p_grain * 1.6, p_grain)) * 0.7
            + sol_noise(w * p_grain * 4.0) * 0.3;
    float edge = n + (1.0 - clamp(w.y, 0.0, 1.0)) * 0.25 - sol_clamped * 1.3;
    if (edge < 0.0) return vec4(0.0);
    float burning = step(0.0001, sol_clamped) * step(sol_clamped, 0.9999);
    float rim = (1.0 - smoothstep(0.0, 0.07, edge)) * burning;
    vec3 glow = vec3(0.70, 0.55, 1.00);
    float a = max(c.a, rim * 0.9);
    return vec4(mix(c.rgb, glow * a, rim), a);
}
