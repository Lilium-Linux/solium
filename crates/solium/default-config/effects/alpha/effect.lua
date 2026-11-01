-- Its input at `amount` of its opacity: a window's own pixels
-- (`source = "self"`) made see-through, so the blur behind the window
-- (looks.lua) shows through it frosted. A group's opacity would not do: it
-- fades everything the window draws, the blur behind it too, and what showed
-- through then was the sharp wallpaper.
return {
    api = 1,
    inputs = { "backdrop" },
    params = {
        amount = { 1, min = 0, max = 1 },
    },
    frag = "alpha.frag",
}
