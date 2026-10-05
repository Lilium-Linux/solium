return { api = 1, inputs = { "self" }, fallback = { { passes = 2 }, { passes = 1 } },
  params = { passes = { 3, min = 1, max = 6, int = true }, offset = { 3 } }, reach = function(p) return math.ceil(p.offset * 2^(p.passes+1)) end,
  stages = function(p) local s = {}      -- at load, never per frame
    for i = 1, p.passes do s[#s+1] = { "pass", "down.frag", scale = .5 } end
    for i = 1, p.passes do s[#s+1] = { "pass", "up.frag", scale = 2 } end
    return s end }
