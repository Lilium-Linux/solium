-- Screens off: super+shift+escape turns every monitor's display off (any
-- key, click or pointer motion turns them back on), and so does a reload
-- that finds $XDG_RUNTIME_DIR/solium-screens-off, which it then removes --
-- the way to ask from outside Solium (a terminal, a script, over ssh):
--
--     touch "$XDG_RUNTIME_DIR/solium-screens-off" && touch ~/.config/solium/user.lua

local screens = {}

local FLAG = (os.getenv("XDG_RUNTIME_DIR") or "/tmp") .. "/solium-screens-off"

function screens.off()
    sol.monitor_power("all", "off")
end

local function asked()
    local file = io.open(FLAG, "r")
    if file then
        file:close()
        os.remove(FLAG)
        screens.off()
    end
end

sol.on("monitors", asked)
sol.on("restore", asked)

return screens
