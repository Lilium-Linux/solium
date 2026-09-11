-- Prism's shell: a bar and a dock, drawn by the compositor.
--
-- `sol.surface` draws any QML scene at any layer on any monitor, and that is
-- the entire mechanism — a wallpaper, a bar, a dock and the deck's scrim are
-- the same call with a different layer and a different rectangle. Nothing in
-- the compositor knows what a dock is.
--
-- This is not a claim that a desktop should be built this way: a real bar is a
-- layer-shell client and one of those is drawn straight over these, which is
-- the point docs/shell-boundary.md makes. It is a claim that the surface layer
-- can carry a real one.

local prism = require("prism")
local monitors = require("monitors")

local shell = {}

-- How wide the dock needs to be for this many windows. The surface is sized to
-- its contents rather than given the width of the screen, because an
-- interactive surface swallows every press inside its rectangle — a full-width
-- strip would look identical and make the bottom of the screen unclickable.
local function dock_width(count)
    local d = prism.dock
    return count * d.tile + math.max(0, count - 1) * d.spacing + d.padding
end

-- Place the bar, and the dock if there is anything to put in it.
--
-- Against the *whole* monitor, not the inset one: the shell is what the inset
-- is reserved for, and measuring it against its own reservation would walk it
-- down the screen a little further on every reload.
function shell.place()
    local screen = monitors.whole()
    if not screen then
        return
    end

    sol.surface("bar", {
        scene = "bar.qml",
        layer = "top",
        on = { x = screen.x, y = screen.y, w = screen.w, h = prism.bar.height },
        interactive = true,
    })

    local count = #sol.windows()
    if count == 0 then
        -- No windows, no dock. An empty pill floating over the wallpaper says
        -- nothing and still eats the clicks underneath it.
        sol.surface("dock", false)
        return
    end

    local width = dock_width(count)
    sol.surface("dock", {
        scene = "dock.qml",
        layer = "top",
        on = {
            x = screen.x + math.floor((screen.w - width) / 2),
            y = screen.y + screen.h - prism.dock.height - prism.dock.margin,
            w = width,
            h = prism.dock.height,
        },
        interactive = true,
    })
end

-- What the scenes ask for. QML sets `action`, the compositor takes the string
-- and clears it, and this turns it back into a call — the only direction the
-- channel runs, and the reason a scene can never do anything the script has
-- not agreed to.
sol.on("surface", function(name, action)
    if action == "deck" then
        require("deck").enter()
        return
    end

    local id = string.match(action or "", "^focus:(%d+)$")
    if id then
        sol.focus(tonumber(id))
    end
end)

-- The dock is sized to the number of windows, so it is re-placed whenever that
-- changes. `focus` is in the list because the tile under the focused window
-- lights up, and the scene reads that from the compositor's window list rather
-- than being told — but the list is only published when something asks for it.
sol.on("open", shell.place)
sol.on("close", shell.place)

-- Placed on the `monitors` event and not at the top of the file. Scripts load
-- before the screens are known — on the hardware backend, before the GPU is
-- even opened — so a rectangle computed now is computed against zeros. It
-- fires once when the monitors are first known and again on every hotplug,
-- which is when a placement needs redoing anyway.
sol.on("monitors", shell.place)

return shell
