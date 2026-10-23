-- A pixels effect: the capture with its corners cut at `radius`.
-- `state::tests::real_client::a_present_through_a_pixels_effect_reads_the_params_it_blends`.
return {
    api = 1,
    inputs = { "self" },
    params = { radius = { 8.0, min = 0 } },
    frag = "rounded.frag",
}
