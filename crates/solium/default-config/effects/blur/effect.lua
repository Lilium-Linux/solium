-- A dual Kawase blur, after Bjørge (SIGGRAPH 2015): `passes` halvings and as
-- many doublings back, each sampling at `offset` texels.
--
-- The shipped one, with the passes in between kept in 16-bit float
-- (`rgba16f`): each 8-bit pass rounded a smooth gradient to 256 steps, and
-- ten in a row left bands. The last up pass dithers (`dither`, in 8-bit
-- steps) as it writes 8 bits (last.frag), the one place the result is
-- rounded: the result every window reads each frame stays half the size a
-- 16-bit one would be, and no dither runs per window.
--
-- Its input is the backdrop, what is behind the part, from xray. A rule
-- reads the window's own pixels instead with `source = "self"`:
--   { match = { app_id = "mpv" }, part = "client", slot = "replace",
--     effect = { "blur", source = "self", passes = 3 } }
return {
    api = 1,
    inputs = { "backdrop" },
    params = {
        passes = { 3, min = 1, max = 6, int = true },
        offset = { 3, min = 0 },
        dither = { 2, min = 0, max = 8 },           -- in 1/255 steps
    },
    -- How far outside the part the blur reads: the last down pass's reach.
    reach = function(p) return math.ceil(p.offset * 2 ^ (p.passes + 1)) end,
    -- Cheaper rungs for when a frame is late (the ladder, X2.7).
    fallback = { { passes = 2 }, { passes = 1 } },
    -- Run when the params are bound, never per frame.
    stages = function(p)
        local s = {}
        for _ = 1, p.passes do s[#s + 1] = { "pass", "down.frag", scale = 0.5, format = "rgba16f" } end
        for _ = 1, p.passes - 1 do s[#s + 1] = { "pass", "up.frag", scale = 2, format = "rgba16f" } end
        s[#s + 1] = { "pass", "last.frag", scale = 2 }
        return s
    end,
}
