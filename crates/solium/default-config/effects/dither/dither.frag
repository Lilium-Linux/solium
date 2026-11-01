// Triangular noise (two hashes, difference) of `amount` 8-bit steps per
// channel, kept inside what is there: premultiplied, so the noise is scaled
// by alpha and a transparent pixel stays transparent.
vec4 sol_effect(vec2 uv) {
    vec4 c = sol_tex(uv);
    vec2 px = uv * sol_size;
    float n = sol_hash(px) - sol_hash(px + vec2(17.3, 41.9));
    vec3 rgb = c.rgb + vec3(n * p_amount / 255.0) * c.a;
    return vec4(clamp(rgb, 0.0, c.a), c.a);
}
