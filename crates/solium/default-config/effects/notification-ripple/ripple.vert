// A circular wave travelling from the window's centre toward every edge:
// each point pushed out from the centre by a wave `width` pixels across and
// `strength` high, faded in and out at the ends, still at rest.
vec2 sol_mesh(vec2 uv) {
    float p = 1.0 - sol_clamped;
    float farthest = length(sol_from.zw) / 2.0;
    float front = -p_width + p * (farthest + 2.5 * p_width);
    float fade = min(1.0, min(p * 10.0, (1.0 - p) * 10.0));
    if (sol_clamped <= 0.0 || sol_clamped >= 1.0) {
        fade = 0.0;
    }
    vec2 at = sol_at(sol_from, uv);
    vec2 d = (uv - 0.5) * sol_from.zw;
    float radius = length(d);
    if (radius <= 0.000001) {
        return at;
    }
    float q = (radius - front) / p_width;
    float centre = radius / sqrt(radius * radius + 3600.0);
    float wave = p_strength * fade * centre * sin(3.14159265359 * q) * exp(-q * q * 0.5);
    return at + d / radius * wave;
}
