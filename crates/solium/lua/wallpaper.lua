-- The wallpaper.
--
-- No Rust. That is the point of it: `sol.surface` draws any QML scene at any
-- layer on any monitor, and a wallpaper is the smallest thing you can build on
-- it. A bar, a dock, a heads-up display and a debug overlay are the same call
-- with a different layer.
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
--
-- ## One picture, or one per desk
--
-- A list of images gives each workspace its own background, and it *travels*
-- with the workspace: `workspaces.lua` puts it in the same selection as that
-- desk's windows, so one `sol.present_group` carries both.
--
--     wallpaper = { "~/Pictures/one.png", "~/Pictures/two.png" }
--
-- With a single image there is nothing to carry, and this declares one static
-- surface exactly as it always did. That is deliberate rather than timid: every
-- desk sharing one picture means a wallpaper that slides is pixel-identical to
-- one that does not, so the memory would buy nothing. It is the images being
-- *different* that makes the movement visible, and that is the moment to start
-- paying for it.
--
-- What it costs when you do ask: one screen-sized rasterisation per desk you
-- have actually visited, per monitor. A desk you have never been to has never
-- been drawn, and a surface is not built until it is drawn.

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

-- The images in force, as a list. One entry means one static wallpaper.
local images = {}

local function declare(name, source)
    sol.surface(name, {
        scene = "wallpaper.qml",
        layer = "background",
        on = "every-monitor",
        properties = { source = source },
    })
end

-- The surface carrying one desk's background, or nil when there is nothing for
-- a desk to carry.
--
-- `workspaces.lua` asks this and puts the answer in that desk's selection. Nil
-- is the ordinary case and means the background belongs to the monitor rather
-- than to a workspace -- so it stays put, which is what one picture behind
-- everything should do.
--
-- Fewer images than desks cycles rather than running out: eight workspaces and
-- two pictures alternate, which is a sensible reading of what was asked for and
-- better than four desks with no background at all.
function wallpaper.for_desk(index)
    if #images < 2 then
        return nil
    end
    return "wallpaper-" .. ((index - 1) % #images + 1)
end

function wallpaper.apply(setting)
    -- Whatever was declared last time goes first, so switching between one
    -- picture and several on a reload does not leave the other arrangement's
    -- surfaces behind it.
    sol.surface("wallpaper", false)
    for index = 1, #images do
        sol.surface("wallpaper-" .. index, false)
    end
    images = {}

    if setting == false or setting == nil then
        return
    end
    if type(setting) ~= "table" then
        images = { setting }
        declare("wallpaper", wallpaper.source(setting))
        return
    end
    for index, image in ipairs(setting) do
        images[index] = image
    end
    if #images == 0 then
        return
    end
    if #images == 1 then
        declare("wallpaper", wallpaper.source(images[1]))
        return
    end
    for index, image in ipairs(images) do
        declare("wallpaper-" .. index, wallpaper.source(image))
    end
end

wallpaper.apply(config.wallpaper)

return wallpaper
