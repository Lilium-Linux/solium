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

    -- Where the monitors are, relative to each other.
    --
    -- Empty means "arrange them yourself": left to right in the order the
    -- kernel enumerated the connectors, top edges aligned. That is right about
    -- half the time, and wrong in a way you can see and fix in one line.
    --
    --     monitors = {
    --         { name = "DP-1", x = 0, y = 0 },
    --         { name = "DP-2", x = 2560, y = 180 },
    --     },
    --
    -- The names are connector names; `solium --probe` prints the ones this
    -- machine has, and a name nothing answers to is warned about in the log
    -- rather than ignored. Positions are the top-left corner in the global
    -- space, so `y` is how much lower one monitor sits than the other -- which
    -- is what a screen standing on a different-height desk actually needs.
    --
    -- A monitor you do not name goes to the right of everything you did, so
    -- plugging in a third does not land it on top of one of the other two.
    monitors = {},

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

    -- What a window does between being asked for and its application
    -- arriving. A window's life starts when you ask for it, not when the
    -- program gets around to connecting -- these decide what that looks like.
    loading = {
        -- Which QML draws it. A name is one of the scenes in `qml/loading`,
        -- or one of your own in ~/.config/solium/qml/loading, which shadows a
        -- shipped one of the same name. A path is anywhere. SOLIUM_LOADING
        -- overrides this, because that is set per run.
        scene = "window",
        -- How long to keep a window open for an application that never
        -- arrives, in milliseconds. After that it closes, exactly as if you
        -- had closed it, and the layout is told.
        patience = 8000,
        -- Whether it takes its place in the layout straight away. With this
        -- off, the other windows only move aside once the application is
        -- really there -- less eager, and some people will prefer it.
        reserves_a_slot = true,
        -- Whether the frame is *drawn* while it waits. The room it takes is
        -- reserved either way, so the window does not change shape when the
        -- application arrives; this only decides whether the bar is on screen
        -- meanwhile. On, and you get a close button for an application that is
        -- not coming. Off, and the scene has the whole window.
        decorated = false,
        -- How long the scene takes to fade off the application that replaced
        -- it, in milliseconds. It is drawn *over* the window, so what is
        -- underneath is already the application. 0 cuts straight to it.
        fade = 180,
    },

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
--
-- Having no user file and having a broken one both make `require` fail, and
-- they must not be treated alike: a typo that silently changes nothing is the
-- worst way to lose an afternoon. Only "no such module" is quiet; anything
-- else is raised, so `solium --check` reports it and a reload keeps whatever
-- was already running.
local found, user = pcall(require, "user")
if found then
    if type(user) == "table" then
        merge(defaults, user)
    else
        error("user.lua must return a table, got " .. type(user), 0)
    end
elseif not tostring(user):match("module 'user' not found") then
    error(tostring(user), 0)
end

return defaults
