-- Minimise into the taskbar (super+m, Solium's own), the battery, and the
-- rest of the bar's Lua.
require("taskbar")
-- The bar, a surface of its own on every monitor so a blur can sit behind it.
require("bar")
-- Inactive windows see-through, and a border-only frame while tiling.
require("looks")
-- The floating titlebar's quick settings and opacity (qml/panes/rounded-violet).
require("titlebar")
-- A fullscreen window is not moved by a drag (fullscreen_still.lua).
require("fullscreen_still")
-- Alt+Tab: every window as a card in a row, Android-style.
require("recents")
-- super+tab: the workspaces as a 3D stack.
require("deskstack")
-- The selected circular ripple on an application notification.
require("notification-ripple")
-- Quick search on super+d (search.lua, shell/Search.qml).
require("search")

-- Screenshots: the whole desktop into the corner, or the tool
-- (screenshot.lua, shell/ScreenshotTool.qml).
local screenshot = require("screenshot")
-- Screens off on a key, or from outside (screens.lua).
local screens = require("screens")

-- This directory, found as Solium finds it: $XDG_CONFIG_HOME/solium when
-- that is set, ~/.config/solium otherwise. The scenes below are named from
-- it, so they are this copy's own and not whatever ~/.config holds.
local here = (os.getenv("XDG_CONFIG_HOME") or (os.getenv("HOME") .. "/.config")) .. "/solium"

return {
    -- Monitors: Solium places them itself. To arrange them, name each by its
    -- connector (`solium --probe` lists them), for example:
    --
    --     monitors = {
    --         { name = "DP-2", x = 0, y = 0 },
    --         { name = "DP-1", x = 0, y = 1440, primary = true },
    --     },

    -- Rounded windows, the focused one outlined in violet
    -- (~/.config/solium/qml/panes/rounded-violet), whose titlebar folds
    -- into the border while a window is in a tile (looks.lua).
    pane = "rounded-violet",

    -- Keyboard layouts: the system's own until set here. More than one,
    -- switched with super+shift+k or a click on the layout widget in the bar:
    --
    --     keyboard = { layout = "us,de", options = "grp:alt_shift_toggle" },

    -- Workspaces stacked in a column, sliding up and down; four of them
    -- (a column's count is `rows`).
    workspaces = {
        arrangement = "vertical",
        rows = 4,
    },

    -- The pointer: cursor/Cursor.qml beside this file, violet like the rest.
    cursor = { scene = here .. "/cursor/Cursor.qml", size = 24 },

    -- Solium's own picture, by its path where Solium is installed: the bare
    -- name "solium" would be looked for beside qml/wallpaper.qml here, not
    -- beside the shipped one. Any image path works (`~` is expanded).
    wallpaper = package.searchpath("wallpaper", package.path):gsub("/lua/wallpaper%.lua$", "")
        .. "/qml/wallpaper/solium.png",

    -- Effects (the rules are looks.lua's): an effect re-runs on every frame
    -- while a window moves, so a blurred backdrop keeps up with each
    -- monitor's own refresh rate rather than 60 a second.
    effects = { motion_hz = 0 },  -- every pass: effects follow each monitor's own rate
    -- Atrium, Stage Manager on the floating desktop: the window in use
    -- and its app's other windows on the stage, every other app a live,
    -- tilted card in a strip down the left, most recent at the top. A card
    -- click brings it forward, shift+click or a drag onto the stage joins it,
    -- a window dropped on the strip leaves its group; super+a turns it off
    -- and on. The strip slides away while a window covers it.
    floating = {
        atrium = { enabled = true, key = "super+a" },
    },

    -- Resizing as KDE does: every app always at its window's border. A
    -- floating window is drawn at the size its app has drawn (the shipped
    -- `follow = "app"`), and the two sides of a tiled seam move together once
    -- both apps have drawn, a slow one holding the other up for 200 ms at most.
    resize = { sync = 200 },

    -- The lock screen: Solium's own, in this desktop's wallpaper and violet
    -- (~/.config/solium/qml/lock/Violet.qml). super+ctrl+l locks, and so does
    -- five minutes idle. super+escape is the shipped one: it frees the pointer
    -- and the shortcuts from a game or a VM.
    -- The clock flies from the bar to the middle as it locks, and back as it
    -- unlocks: the longest entry Solium allows, so the flight is seen.
    lock = { scene = "Violet", enter_ms = 600, exit_ms = 700 },

    -- The shell: pop-ups, banners, quick search and the screenshot tool
    -- (shell/Shell.qml; the taskbar itself is bar.lua and shell/Bar.qml).
    shell = {
        scene = here .. "/shell/Shell.qml",
    },

    -- Screenshots and the lock key.
    bindings = {
        -- Volume, mute and media keys ship now: the volume ones go through
        -- Solium's own audio service, so they are not bound here.
        -- Screenshots: saved to ~/Pictures/Screenshots and copied.
        ["print"]                 = screenshot.screen,
        ["shift+print"]           = screenshot.region,
        ["super+shift+s"]         = screenshot.region,
        ["shift+super+escape"]    = screens.off,
        -- Tiling and floating only: super+t toggles between them.
        ["super+s"]               = false,
    },
}
