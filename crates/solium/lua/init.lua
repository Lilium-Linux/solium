-- Solium's default configuration.
--
-- Copy to ~/.config/solium/init.lua to change it; the compositor prefers that
-- file when it exists. Modes live in their own scripts and register their own
-- bindings, so adding one is a `require` and removing one is deleting a line.

local config = require("config")

-- Settings the compositor itself holds, applied from the same file as
-- everything else. Both take effect immediately when reloaded.
sol.decoration(config.decoration)
sol.loading(config.loading)
-- Where the monitors go. Applied on reload too, so moving a screen in the
-- configuration is `super+shift+r` rather than logging out.
sol.monitors(config.monitors)

-- Only reachable when the compositor was started with --debug-mode, but the
-- entries are declared either way: what costs nothing to declare should not
-- need a conditional.
require("tweaks")

require("modes")
require("open")
require("overview")
require("workspaces")
require("tiling")
require("scrolling")

-- Programs. `sol.spawn` starts them as clients of this compositor, whatever
-- session the compositor itself happens to be nested in.

-- Split on spaces, because a terminal often needs arguments to open a *new*
-- window: KDE's konsole hands off to an already-running instance and exits
-- unless told `--separate --nofork`, which looks exactly like the spawn having
-- failed.
local function words(text)
    local parts = {}
    for word in string.gmatch(text, "%S+") do
        parts[#parts + 1] = word
    end
    return parts
end

-- `SOLIUM_TERMINAL` wins; otherwise the first of these that is actually
-- installed. A compositor cannot assume any particular terminal exists, and
-- picking one that does not is indistinguishable, from the keyboard, from the
-- binding being broken -- which is how the first hardware session went.
local CANDIDATES = {
    -- Plain Wayland terminals first: they open immediately. A KDE terminal
    -- waits on portal and session services that are not running under Solium
    -- and takes about twenty seconds to show a window, which reads as the
    -- binding being broken rather than as the app being slow -- so konsole is
    -- a fallback, not a preference, even on a KDE machine.
    "kitty",
    "alacritty",
    "wezterm",
    "foot",
    "konsole --separate --nofork",
    "xterm",
}

local function first_installed(candidates)
    for _, candidate in ipairs(candidates) do
        local parts = words(candidate)
        if sol.which(parts[1]) then
            return parts
        end
    end
    return nil
end

local TERMINAL = words(os.getenv("SOLIUM_TERMINAL") or "")
if #TERMINAL == 0 then
    TERMINAL = first_installed(CANDIDATES)
end
if TERMINAL then
    sol.log("terminal: " .. table.concat(TERMINAL, " "))
else
    sol.log("no terminal found -- set SOLIUM_TERMINAL to one you have")
end

local function open_terminal()
    if not TERMINAL then
        sol.log("no terminal is installed -- set SOLIUM_TERMINAL")
        return
    end
    sol.spawn(table.unpack(TERMINAL))
end

-- Both Enters. The keypad one is a different keysym (`kp_enter`), so binding
-- only `return` leaves whoever reaches for the near one pressing a key that
-- does nothing -- and nothing on screen says why. Bound separately rather than
-- folded together in the compositor, so a script can still tell them apart.
sol.bind("super+return", open_terminal)
sol.bind("super+kp_enter", open_terminal)

-- Close the focused window. A request, not a kill -- the client decides whether
-- it can go.
sol.bind("super+q", function()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            sol.close(window.id)
            return
        end
    end
end)

-- Ending the session. Ctrl+Alt+Backspace does this too and cannot be rebound,
-- because the way out has to work even when this file does not.
sol.bind("super+shift+q", function()
    sol.quit()
end)

sol.log("solium configuration loaded")

-- A tilted window, to see the compositor draw one as geometry rather than as
-- a rectangle. `rotate_y` turns it about its own vertical axis and
-- `perspective` is the viewer distance in pixels, which is what makes the far
-- edge recede instead of merely narrowing.
-- Dev: the genie. `genie` names the slot the window is pulled into -- a dock
-- icon's rectangle, once there is a dock -- and the compositor bends the
-- window into it: the rows nearest the slot go first, so it folds like a sheet
-- through a letterbox instead of shrinking. `spread` is how much of it is in
-- motion at once. Composes with a transform: add `rotate_y` here and the
-- window tilts while it is sucked in.
-- Show or hide the Developer Tweaks panel. Nothing without --debug-mode.
sol.bind("super+shift+d", function()
    sol.tweaks_toggle()
end)

-- Read this file again, without ending the session. Edit anything -- a
-- binding, a gap, a decoration, a whole layout mode -- and press it.
sol.bind("super+shift+r", function()
    sol.reload()
end)

sol.bind("super+m", function()
    local area = sol.monitor()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            sol.animate({ duration = 520, easing = "inOutCubic" })
            sol.present(window.id, {
                genie = {
                    x = area.x + area.w / 2 - 60,
                    y = area.y + area.h - 24,
                    width = 120,
                    height = 24,
                    spread = 1.4,
                },
            })
            return
        end
    end
end)

sol.bind("super+g", function()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            sol.animate({ duration = 260, easing = "outCubic" })
            sol.present(window.id, { rotate_y = 35, perspective = 900 })
            return
        end
    end
end)
