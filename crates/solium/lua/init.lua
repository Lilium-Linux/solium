-- Solium's default configuration.
--
-- Copy to ~/.config/solium/init.lua to change it; the compositor prefers that
-- file when it exists. Modes live in their own scripts and register their own
-- bindings, so adding one is a `require` and removing one is deleting a line.

require("overview")

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

local TERMINAL = words(os.getenv("SOLIUM_TERMINAL") or "foot")

sol.bind("super+return", function()
    sol.spawn(table.unpack(TERMINAL))
end)

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
