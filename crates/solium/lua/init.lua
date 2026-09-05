-- Solium's default configuration.
--
-- Copy to ~/.config/solium/init.lua to change it; the compositor prefers that
-- file when it exists. Modes live in their own scripts and register their own
-- bindings, so adding one is a `require` and removing one is deleting a line.

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

sol.log("solium configuration loaded")
