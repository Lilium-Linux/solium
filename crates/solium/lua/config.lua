-- Everything tunable, in one place.
--
-- Values were scattered across the layout scripts as local constants, which is
-- hardcoding written in a scripting language. A setting nobody can find is not
-- a setting. Edit this file; nothing here needs the compositor rebuilt.

return {
    -- Space between windows and around the work area, in logical pixels.
    gap = 12,

    tiling = {
        -- Where a split falls, as a share of the window being divided.
        -- Hyprland calls this dwindle:default_split_ratio.
        split = 0.5,
        motion = { duration = 240, easing = "outCubic" },
        -- The shorter feel for a window snapping back after a drag.
        snap = { duration = 180, easing = "outCubic" },
    },

    scrolling = {
        -- The widths a column cycles through with super+r, as shares of the
        -- view. A new column starts at `default_width`, an index into these.
        widths = { 1 / 3, 1 / 2, 2 / 3 },
        default_width = 1,
        motion = { duration = 260, easing = "outCubic" },
        -- The shorter feel for bringing a column into view.
        snap = { duration = 200, easing = "outCubic" },
    },

    workspaces = {
        -- "horizontal": workspaces sit in a row and slide sideways.
        -- "vertical":   a column, sliding up and down.
        -- "grid":       both, `columns` wide and `rows` tall.
        --
        -- The arrangement decides which way a switch travels, and that is the
        -- whole difference between the three: a workspace to the right of this
        -- one enters from the right, because that is where it is.
        arrangement = "horizontal",
        columns = 4,
        rows = 2,
        -- How far apart workspaces sit, as a fraction of the screen. Above 1.0
        -- there is a gap of empty space between them mid-slide, which reads as
        -- distance rather than as a cut.
        spread = 1.06,
        motion = { duration = 300, easing = "outCubic" },
        -- Whether a new window joins the workspace you are looking at.
        follow_new_windows = true,
    },

    open = {
        -- The animation a window arrives with.
        motion = { duration = 200, easing = "outCubic" },
        scale = 0.92,
    },
}
