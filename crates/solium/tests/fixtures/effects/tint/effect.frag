vec4 sol_effect(vec2 uv) { vec4 c = sol_tex(uv); return vec4(mix(c.rgb, vec3(c.a), p_amount), c.a); }
