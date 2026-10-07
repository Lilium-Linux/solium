return { api = 1, inputs = { "self" }, params = { n = { 1 } }, stages = { { "state", "n", depends = "params", stages = { { "pass", "count.frag" } } }, { "pass", "read.frag", uses = { "n" } } } }
