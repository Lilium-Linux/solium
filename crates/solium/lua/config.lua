-- Everything tunable, in one place.
--
-- Values were scattered across the layout scripts as local constants, which is
-- hardcoding written in a scripting language. A setting nobody can find is not
-- a setting. Edit this file; nothing here needs the compositor rebuilt.
--
-- To change something without copying this file, write only what you want in
-- ~/.config/solium/user.lua and it is merged over these:
--
--     return {
--         gap = 4,
--         decoration = "reactive",
--         tiling = { split = 0.618 },
--     }
--
-- Nested tables merge key by key, so `tiling = { split = ... }` keeps the
-- animations below it. Lists are replaced whole, because a list of widths with
-- one entry changed is a different list, not a longer one.

local defaults = {
    -- Space between windows and around the work area, in logical pixels.
    gap = 12,

    -- Which QML file frames every window. A name is one of the decorations in
    -- `qml/decorations`, or one of your own in
    -- ~/.config/solium/qml/decorations, which shadows a shipped one of the
    -- same name. A path is anywhere.
    --
    --   "top"        a titlebar above the window (the default)
    --   "left"       a titlebar down the left side
    --   "bottom"     a titlebar underneath
    --   "border"     no bar, just a frame
    --   "reactive"   a border lit where the cursor is, with a bar
    --   "proximity"  a border that answers the pointer arriving and leaving
    --   "reveal"     a bar that slides out of the window's edge on approach
    --   "pulse"      a bar with an animation running in it
    decoration = "top",

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

    dock = {
        -- What sits on the dock. Programs, by the name used to run them.
        items = { "kitty", "firefox" },
        -- How a window grows out of its icon. Slower than an ordinary open,
        -- because the distance travelled is the thing being shown.
        morph = { duration = 340, easing = "outCubic" },
    },

    open = {
        -- The animation a window arrives with.
        motion = { duration = 200, easing = "outCubic" },
        scale = 0.92,
    },
}

-- A list is a table with a [1]; anything else with keys is a section to
-- descend into. Crude, and right for every shape in this file.
local function is_list(value)
    return type(value) == "table" and value[1] ~= nil
end

local function merge(base, over)
    for key, value in pairs(over) do
        if type(value) == "table" and type(base[key]) == "table"
            and not is_list(value) and not is_list(base[key]) then
            merge(base[key], value)
        else
            base[key] = value
        end
    end
    return base
end

-- `require` searches the user's directory first, so this finds
-- ~/.config/solium/user.lua when there is one and nothing when there is not.
-- A broken user file is reported and ignored rather than taking the session
-- down with it.
local found, user = pcall(require, "user")
if found and type(user) == "table" then
    merge(defaults, user)
elseif found then
    sol.log("user.lua did not return a table; ignoring it")
end

return defaults
