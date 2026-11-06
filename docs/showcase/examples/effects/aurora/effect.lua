-- Aurora: a band of colour round a window's rounded outline, violet, cyan
-- and pink, with a bright head that runs round the edge as `phase` goes from
-- 0 to 1. A showcase example (docs/showcase.md): `aurora.lua` beside this
-- folder puts it round the focused window and turns `phase` every frame.
--
-- Like the shipped `shade` it reads nothing of the frame, only the part's
-- shape, so it costs a pass only when `phase` or the part's size changes.
-- Copy this folder to ~/.config/solium/effects/aurora/ to use it.
return {
    api = 1,
    inputs = { "shape" },
    params = {
        phase = { 0, min = 0, max = 1 },           -- where the head is, round the edge
        width = { 28, min = 0, max = 64 },         -- how far in the glow reaches, px
        strength = { 1, min = 0, max = 1 },        -- how opaque it is at the edge
        tail = { 0.35, min = 0.05, max = 1 },      -- how much of the way round the head's glow trails
    },
    frag = "aurora.frag",
}
