-- Breaks up the banding a blur leaves in a smooth gradient: its input with
-- a little noise added, `amount` steps of an 8-bit channel, fixed to the
-- pixel so it does not crawl. The last link of a blur's chain.
return {
    api = 1,
    inputs = { "backdrop" },
    params = {
        amount = { 1.5, min = 0, max = 8 },          -- in 1/255 steps
    },
    frag = "dither.frag",
}
