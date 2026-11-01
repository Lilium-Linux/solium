// The blur's last up pass, with the dither (effects/dither) in it: the up
// pass's eight taps, read from the 16-bit passes before, then triangular
// noise of p_dither 8-bit steps before this pass writes 8 bits. One pass and
// one texture fewer than a dither link after the blur, and with nothing left
// per window, the whole chain is run once per monitor and shared.
vec4 sol_effect(vec2 uv) {
    vec2 h = sol_texel * 0.5 * p_offset, d = vec2(h.x, -h.y), x = vec2(2.0 * h.x, 0.0), y = vec2(0.0, 2.0 * h.y);
    vec4 c = (sol_tex(uv - x) + sol_tex(uv + x) + sol_tex(uv - y) + sol_tex(uv + y)
            + 2.0 * (sol_tex(uv + h) + sol_tex(uv - h) + sol_tex(uv + d) + sol_tex(uv - d))) / 12.0;
    vec2 px = uv * sol_size;
    float n = sol_hash(px) - sol_hash(px + vec2(17.3, 41.9));
    vec3 rgb = c.rgb + vec3(n * p_dither / 255.0) * c.a;
    return vec4(clamp(rgb, 0.0, c.a), c.a);
}
