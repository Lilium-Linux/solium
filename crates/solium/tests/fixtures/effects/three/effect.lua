return { api = 1, inputs = { "self", "shape", "state:a" }, stages = { { "state", "a", depends = "params", stages = { { "pass", "a.frag" } } }, { "pass", "three.frag", uses = { "a", "shape" } } } }
