-- Its input with the corners cut round at `radius`, antialiased: a window's
-- own pixels (`source = "self"`) for a window that draws its own titlebar and
-- so has no pane style to cut them, or, as the last link of a chain, the
-- chain's result -- a blur behind such a window, so it is no squarer than the
-- window. Reading `shape`, it owns its edges.
return {
    api = 1,
    inputs = { "backdrop", "shape" },
    params = {
        radius = { 12, min = 0, max = 512 },         -- px
    },
    frag = "corners.frag",
}
