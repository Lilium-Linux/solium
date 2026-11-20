// The genie: the window pulled into its target like a sheet through a
// letterbox, the edge nearest the target first. Held to Solium's Rust genie
// (crates/effects, Deform::Genie) within 0.01 px on all four axes, at every
// spread, on the GPU (dev/wirecheck).
//
// Each step across the sweep runs its own copy of the pull, the ones
// furthest from the target starting `spread` later, so the tail is drawn
// out behind the lead; smoothstepped, so the sheet arrives without a crease.
// `uv` is the window's own, past its edges too (a shadow, a menu), so what
// lies beyond the window bends with it.
vec2 sol_mesh(vec2 uv) {
    float step = clamp(sol_progress * (1.0 + p_spread) - sol_phase(uv) * p_spread, 0.0, 1.0);
    float eased = step * step * (3.0 - 2.0 * step);
    return mix(sol_at(sol_from, uv), sol_at(sol_to, uv), eased);
}
