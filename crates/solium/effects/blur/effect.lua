-- A dual Kawase blur, after Bjørge (SIGGRAPH 2015): `passes` halvings and as
-- many doublings back, each sampling at `offset` texels. Copy this folder to
-- ~/.config/solium/effects/blur/ to change it; your copy is used instead.
--
-- Its input is the backdrop. Until xray arrives (X2.1) a rule reads the
-- window's own pixels with `source = "self"`:
--   { match = { app_id = "mpv" }, part = "client", slot = "replace",
--     effect = { "blur", source = "self", passes = 3 } }
return {
    api = 1,
    inputs = { "backdrop" },
    params = {
        passes = { 3, min = 1, max = 6, int = true },
        offset = { 3, min = 0 },
    },
    -- How far outside the part the blur reads: the last down pass's reach.
    reach = function(p) return math.ceil(p.offset * 2 ^ (p.passes + 1)) end,
    -- Cheaper rungs for when a frame is late (the ladder, X2.7).
    fallback = { { passes = 2 }, { passes = 1 } },
    -- Run when the params are bound, never per frame.
    stages = function(p)
        local s = {}
        for _ = 1, p.passes do s[#s + 1] = { "pass", "down.frag", scale = 0.5 } end
        for _ = 1, p.passes do s[#s + 1] = { "pass", "up.frag", scale = 2 } end
        return s
    end,
}
