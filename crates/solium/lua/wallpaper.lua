-- The wallpaper.
--
-- Nine lines, no Rust. That is the point of it: `sol.surface` draws any QML
-- scene at any layer on any monitor, and a wallpaper is the smallest thing you
-- can build on it. A bar, a dock, a heads-up display and a debug overlay are
-- the same call with a different layer.
--
-- It used to be a compositor feature -- a `Command::Wallpaper`, an accessor on
-- the state, a special case in the renderer, a hundred lines of Rust for a
-- picture behind the windows. See issue #86 for why that was the wrong shape:
-- a compositor that needs new Rust for a wallpaper is one that will need new
-- Rust for the next thing too.
--
-- `config.wallpaper = false` turns it off, which is what you want if you run
-- `swaybg`, `hyprpaper` or a shell that draws its own -- those are layer-shell
-- clients on the background layer, and they are drawn *over* this.

local config = require("config")

local wallpaper = {}

-- Where the image actually is. A bare "solium" is the one that ships, beside
-- the QML that draws it; anything else is a path, and `~` is expanded here
-- because nothing between this file and Qt would do it.
function wallpaper.source(setting)
    if setting == "solium" then
        return "wallpaper/solium.png"
    end
    -- `~` is what every other dotfile uses and nothing between here and Qt
    -- expands it, so a configuration written the obvious way would load
    -- nothing at all and say nothing about why.
    local home = os.getenv("HOME")
    if home then
        local rest = string.match(setting, "^~/(.*)$")
        if rest then
            return home .. "/" .. rest
        end
    end
    return setting
end

function wallpaper.apply(setting)
    if setting == false or setting == nil then
        sol.surface("wallpaper", false)
        return
    end
    sol.surface("wallpaper", {
        scene = "wallpaper.qml",
        layer = "background",
        on = "every-monitor",
        properties = { source = wallpaper.source(setting) },
    })
end

wallpaper.apply(config.wallpaper)

return wallpaper
