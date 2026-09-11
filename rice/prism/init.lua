-- Prism. A rice for Solium: frosted glass, violet light, and a deck.
--
-- This replaces the shipped entry point, and it is deliberately almost the
-- same file. Everything that ships is still required — the modes, the
-- workspaces, the layouts — because a rice that reimplemented them would stop
-- getting their fixes. What is different is four lines near the bottom.
--
--   rice/prism/          this directory
--   ~/.config/solium/    where it goes
--
-- Nothing here needs the compositor rebuilt, and `super+shift+r` reads it all
-- again without ending the session.

local config = require("config")

-- Settings the compositor itself holds. Both take effect on reload too.
sol.decoration(config.decoration)
sol.loading(config.loading)
sol.keyboard(config.keyboard)
sol.monitors(config.monitors)

-- The wallpaper is a script like any other, and so is everything below it.
require("wallpaper")

-- `monitors` is replaced by this rice rather than added to: it is where every
-- mode reads its work area, so insetting it there is what keeps tiling,
-- scrolling, overview *and* the deck out from under the bar and the dock.
require("monitors")

require("tweaks")

require("modes")
require("open")
require("overview")
require("workspaces")
require("tiling")
require("scrolling")

-- Prism's own. `shell` places the bar and the dock and answers what they ask
-- for; `deck` is the switcher.
require("shell")
require("deck")

-- Programs. `sol.spawn` starts them as clients of this compositor, whatever
-- session the compositor itself happens to be nested in.
local function words(text)
    local parts = {}
    for word in string.gmatch(text, "%S+") do
        parts[#parts + 1] = word
    end
    return parts
end

local CANDIDATES = {
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

sol.bind("super+return", open_terminal)
sol.bind("super+kp_enter", open_terminal)

sol.bind("super+q", function()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            sol.close(window.id)
            return
        end
    end
end)

sol.bind("super+shift+q", function()
    sol.quit()
end)

sol.bind("super+shift+d", function()
    require("tweaks").toggle()
end)

sol.bind("super+shift+k", function()
    local keyboard = sol.keyboard()
    if #keyboard.layouts < 2 then
        return
    end
    local next_layout = keyboard.active % #keyboard.layouts + 1
    sol.keyboard({ active = next_layout })
    sol.log("keyboard layout: " .. keyboard.layouts[next_layout])
end)

sol.bind("super+shift+r", function()
    sol.reload()
end)

sol.log("prism loaded")
