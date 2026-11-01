// The blur, up pass: twice the size, eight taps at p_offset texels apart.
vec4 sol_effect(vec2 uv) { vec2 h = sol_texel * 0.5 * p_offset, d = vec2(h.x, -h.y), x = vec2(2.0 * h.x, 0.0), y = vec2(0.0, 2.0 * h.y);
  return (sol_tex(uv - x) + sol_tex(uv + x) + sol_tex(uv - y) + sol_tex(uv + y)
        + 2.0 * (sol_tex(uv + h) + sol_tex(uv - h) + sol_tex(uv + d) + sol_tex(uv - d))) / 12.0; }
