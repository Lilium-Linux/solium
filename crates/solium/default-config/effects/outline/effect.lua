-- A border just outside a part's rounded outline, `width` pixels wide in one
-- colour, antialiased on both edges, so it covers none of the window.
-- looks.lua puts it on windows that draw their own titlebar, which no pane
-- style frames, so they are bordered as the framed ones are (qml/panes/*):
-- violet with the keyboard, dim without.
--
-- What an effect draws is the part padded by its `reach` (its `bleed` is not
-- what widens it), and a result that reads `shape` is not cut to the part,
-- so the reach is what gives the band room outside. Reads nothing but the
-- part's shape: it runs once, and again only when the part's size or the
-- params change.
return {
    api = 1,
    inputs = { "shape" },
    params = {
        width = { 2, min = 0, max = 16 },            -- px
        color = { { 0.576, 0.427, 1, 1 } },          -- #936dff
        radius = { 12, min = 0, max = 512 },         -- the part's corners, px; the border's are this plus width
    },
    reach = function(p) return math.ceil(p.width) + 1 end,
    frag = "outline.frag",
}
