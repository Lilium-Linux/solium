return { api = 1, inputs = { "self" }, stages = { { "repeat", over = { 64, 32, 16, 8, 4, 2, 1, 1 }, as = "jump", { "pass", "jump.frag" } } } }
