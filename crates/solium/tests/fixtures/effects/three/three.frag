vec4 sol_effect(vec2 uv) { return vec4(sol_tex(uv).r, sol_a(uv).g, sol_shape(uv), 1.0); }
