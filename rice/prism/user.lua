-- Prism's settings. Merged over the shipped defaults, key by key.
--
-- Everything else — the bar, the dock, the deck, the frame, the wallpaper —
-- is a file beside this one rather than a value in it. That split is the
-- point: a setting is for a number somebody might want different, and a scene
-- is for a thing somebody might want *else*.

return {
    -- Wider than the shipped 12. The frames are translucent, so the wallpaper
    -- between two windows is part of the composition rather than wasted space,
    -- and at 12 there is not enough of it to read as glass.
    gap = 16,

    -- qml/decorations/glass.qml, here in this directory. A file of that name
    -- in the user's own directory shadows a shipped one, so this could equally
    -- have been called "top" and meant this rice's idea of a top bar.
    decoration = "glass",

    -- Still "solium", and still ignored: qml/wallpaper.qml here shadows the
    -- shipped scene, and the shipped scene is the only thing that ever looked
    -- at this value. Left set rather than turned off because `false` would
    -- stop the wallpaper script running at all, and the script is what places
    -- the surface.
    wallpaper = "solium",

    -- A little slower than the default and with a touch of overshoot, so a
    -- window arriving at its slot settles rather than stops. These are the
    -- four numbers CSS calls cubic-bezier, so a feel found anywhere else
    -- transfers directly.
    tiling = {
        motion = { duration = 280, easing = { 0.22, 1.0, 0.36, 1.0 } },
    },
    scrolling = {
        motion = { duration = 300, easing = { 0.22, 1.0, 0.36, 1.0 } },
    },

    -- Windows arrive slightly small and grow in. Under 0.9 it reads as a zoom;
    -- above 0.96 you cannot tell it happened.
    open = {
        motion = { duration = 240, easing = "outCubic" },
        scale = 0.94,
    },

    keyboard = {
        repeat_rate = 40,
        repeat_delay = 300,
    },
}
