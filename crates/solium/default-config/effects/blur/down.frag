// The blur, down pass: half the size, five taps at p_offset texels apart.
vec4 sol_effect(vec2 uv) { vec2 h = sol_texel * 0.5 * p_offset, d = vec2(h.x, -h.y);
  return (sol_tex(uv) * 4.0 + sol_tex(uv - h) + sol_tex(uv + h) + sol_tex(uv + d) + sol_tex(uv - d)) / 8.0; }
