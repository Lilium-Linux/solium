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

    -- The monitors.
    --
    -- Empty means "work it out": every connected screen is driven, left to
    -- right in the order the kernel enumerated the connectors, top edges
    -- aligned. That is right about half the time, and wrong in a way you can
    -- see and fix in one line.
    --
    --     monitors = {
    --         { name = "DP-1", mode = "2560x1440@260", vrr = true, primary = true },
    --         { name = "DP-2", mode = "2560x1440@75", right_of = "DP-1", align = "end" },
    --         { name = "DP-3", above = "DP-1", transform = "90" },
    --         { name = "HDMI-A-1", enabled = false },
    --     },
    --
    -- `name` is the connector name; `solium --probe` prints the ones this
    -- machine has, and a name nothing answers to gets a line in the log rather
    -- than being ignored. Everything else is optional:
    --
    --   right_of, left_of, above, below   beside another monitor, by name.
    --                                     Prefer this to x and y: it does not
    --                                     go stale when a resolution changes,
    --                                     and a chain resolves whatever order
    --                                     you write the list in.
    --
    --   align = "start" | "centre"        which way the *other* axis lines up
    --         | "end"                     when placed beside something taller
    --                                     or wider. "centre" is the default.
    --                                     A 1080p beside a 1440p leaves 360
    --                                     rows belonging to no screen, and
    --                                     this decides which end they are at
    --                                     -- which is where the pointer will
    --                                     catch on the way past.
    --
    --   x, y                              the top-left corner outright, in the
    --                                     one global space every monitor is a
    --                                     window onto. `y` is how much lower
    --                                     one screen sits than another, which
    --                                     is what a monitor on a taller desk
    --                                     actually needs.
    --
    --   mode = "2560x1440@260"            resolution and refresh rate. The
    --                                     refresh is optional -- "2560x1440"
    --                                     alone means the fastest mode at that
    --                                     size. A table `{ w = , h = ,
    --                                     refresh = }` does the same thing, for
    --                                     generating a configuration rather
    --                                     than writing one.
    --
    --                                     Three words also work:
    --                                       "best"       the highest refresh at
    --                                                    the preferred
    --                                                    resolution. The
    --                                                    default.
    --                                       "preferred"  exactly what the
    --                                                    monitor's EDID says,
    --                                                    refresh included --
    --                                                    for one that is
    --                                                    unstable at its
    --                                                    fastest.
    --                                       "widest"     the largest
    --                                                    resolution, fastest at
    --                                                    that size.
    --
    --                                     "best" is not "preferred": the
    --                                     EDID's preferred *flag* names a
    --                                     resolution and usually pairs it with
    --                                     a pedestrian 60 Hz. A 260 Hz panel
    --                                     reports 2560x1440@60 as preferred,
    --                                     and taking that literally drives a
    --                                     fast display slowly and makes every
    --                                     animation look worse than it is.
    --
    --                                     A mode the monitor does not have
    --                                     warns and falls back; `--probe` says
    --                                     how many each one offers.
    --
    --   vrr = true                        variable refresh rate, where the
    --                                     monitor and the driver both offer it
    --                                     -- FreeSync, G-Sync compatible,
    --                                     Adaptive-Sync. The display's refresh
    --                                     follows what is actually being drawn
    --                                     instead of the other way round, which
    --                                     is what removes the tear and the
    --                                     stutter on anything that cannot hold
    --                                     a steady frame rate. Left alone by
    --                                     default, because it interacts badly
    --                                     with some panels at low frame rates
    --                                     (visible flicker) and that is not a
    --                                     thing to turn on for somebody.
    --
    --   transform = "90"                  rotation, anticlockwise, as degrees:
    --                                     "normal", "90", "180", "270", or the
    --                                     same with a "flipped-" prefix. A
    --                                     rotated monitor's work area is
    --                                     portrait, so every layout follows it
    --                                     without knowing about it.
    --
    --   enabled = false                   do not drive it. It also frees its
    --                                     CRTC for another screen, which
    --                                     matters on a card with more
    --                                     connectors than CRTCs.
    --
    --   primary = true                    the monitor things belonging to one
    --                                     screen go on: a dock, a bar, any
    --                                     layer surface that did not name an
    --                                     output. Without this it is the first
    --                                     monitor -- stable, but not a choice
    --                                     anybody made.
    --
    --   scale = 2                         how many device pixels to a logical
    --                                     one. Everything doubles in size: a
    --                                     window, a titlebar, the pointer, and
    --                                     the compositor's own QML is
    --                                     rasterised at that many pixels
    --                                     rather than stretched.
    --
    --                                     Left out, it is worked out from the
    --                                     panel's own size -- 2x above 192 dpi,
    --                                     which is the number GNOME and KDE
    --                                     both use, and 1x below. That puts a
    --                                     13" 4K laptop at 2x and a 27" 4K at
    --                                     1x, and the second of those is
    --                                     genuinely a matter of taste, which
    --                                     is why this is settable. `--probe`
    --                                     and the log both print the dpi it
    --                                     measured.
    --
    --                                     Fractional values work; between 0.5
    --                                     and 8. Anything else is refused as
    --                                     far likelier a typo than a request.
    --
    -- `super+shift+r` applies a change without ending the session.
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
    --   "none"       no frame at all: no bar, no border, and no QML scene
    --                built per window. For a desktop with no window furniture,
    --                or a tiling layout whose own bar makes a titlebar
    --                redundant.
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
        -- Whether each monitor has its own active workspace.
        --
        -- On, `super+2` switches the screen the pointer is on and leaves the
        -- other showing whatever it was: a reference on the second monitor
        -- stays put while you move around on the first. This is what sway,
        -- Hyprland and niri do, and what most people expect.
        --
        -- Off, one switch moves every screen at once, so a workspace is a
        -- whole desk rather than a screenful. That is GNOME's model, and it is
        -- the right one if you think of your two monitors as one surface you
        -- happen to have cut in half.
        --
        -- Only matters with more than one monitor.
        per_monitor = true,

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
