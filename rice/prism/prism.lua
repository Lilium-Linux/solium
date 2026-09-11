-- Prism's metrics, in one place.
--
-- Two files need to agree about how tall the bar is: the one that places it,
-- and the one that keeps windows out from under it. When they disagreed the
-- symptom was a titlebar half-hidden behind the clock, which reads as a
-- compositor bug rather than as two numbers that drifted.

return {
    -- The surface the bar is drawn into, not the panel inside it. The panel
    -- insets itself by Theme.panelInset on every side, so 34 + 12 + 12.
    bar = { height = 58 },

    -- The dock sizes itself to the number of windows, because an interactive
    -- surface swallows every press inside its rectangle and a full-width strip
    -- would eat every click along the bottom of the screen.
    dock = {
        height = 70,
        tile = 46,
        spacing = 8,
        padding = 28,
        -- How far the strip sits off the bottom edge.
        margin = 8,
    },

    -- What the layouts must keep clear. A scripted surface is not a layer-shell
    -- client, so it reserves nothing from the work area on its own — this is
    -- the reservation, applied in monitors.lua where every mode reads it.
    inset = { top = 58, bottom = 86, left = 0, right = 0 },
}
