-- Developer Tweaks: what the panel offers, and what each entry does.
--
-- The panel is a list of whatever is declared here. Adding one is adding a
-- line to `entries` and a branch to the handler — there is nothing to rebuild
-- and nothing in the compositor that knows what any of these mean.
--
-- Only shown when the compositor is started with `--debug-mode`.

local config = require("config")

local function focused()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            return window
        end
    end
    return nil
end

-- Every decoration that ships, so switching between them is one press each.
local decorations = { "top", "left", "bottom", "border", "reactive", "proximity",
                      "reveal", "pulse" }

local entries = {}
for _, name in ipairs(decorations) do
    table.insert(entries, { id = "decoration:" .. name, label = name, group = "Decoration" })
end

-- Presentation: what a window can be drawn as, without moving it.
local effects = {
    { id = "tilt",     label = "Tilt away (3D)" },
    { id = "flat",     label = "Back to flat" },
    { id = "genie",    label = "Genie into the dock" },
    { id = "shrink",   label = "Shrink in place" },
    { id = "fade",     label = "Half fade" },
    { id = "spin",     label = "Spin about Z" },
}
for _, effect in ipairs(effects) do
    table.insert(entries, { id = "effect:" .. effect.id, label = effect.label, group = "Focused window" })
end

table.insert(entries, { id = "reload", label = "Reload configuration", group = "Session" })

-- The panel itself. It used to be a compositor feature -- an accessor on the
-- state, an area function, its own pointer routing, its own `--debug-mode`
-- gate and a `TweaksToggle` command. It is now `sol.surface` like anything
-- else, and the compositor has no idea what a tweak is.
--
-- Down the right-hand side of the monitor you are looking at, over the
-- windows, taking clicks.
local tweaks = { shown = true }

function tweaks.area()
    local screen = sol.monitor()
    local width = math.max(200, math.min(320, screen.w / 3))
    return { x = screen.x + screen.w - width, y = screen.y, w = width, h = screen.h }
end

function tweaks.apply()
    if not tweaks.shown then
        sol.surface("tweaks", false)
        return
    end
    sol.surface("tweaks", {
        scene = "tweaks.qml",
        layer = "overlay",
        on = tweaks.area(),
        interactive = true,
        properties = { entries = entries },
    })
end

function tweaks.toggle()
    tweaks.shown = not tweaks.shown
    tweaks.apply()
end

-- Only with `--debug-mode`, which is the same gate as before -- it just lives
-- here now rather than in the compositor.
--
-- Placed on the `monitors` event rather than here, because *here* is too
-- early: scripts load before the screens are known, so a rect computed now is
-- computed against zeros. That event fires once at startup and again whenever
-- a monitor arrives or leaves, which is exactly when this needs redoing.
if sol.debug_mode() then
    sol.on("monitors", tweaks.apply)
end

sol.on("surface", function(name, id)
    if name ~= "tweaks" then
        return
    end
    tweaks.handle(id)
end)

function tweaks.handle(id)
    local kind, name = id:match("^(%a+):(.+)$")

    if kind == "decoration" then
        sol.decoration(name)
        return
    end

    if id == "reload" then
        sol.reload()
        return
    end

    local window = focused()
    if not window then
        return
    end

    if kind ~= "effect" then
        return
    end

    if name == "tilt" then
        sol.animate({ duration = 260, easing = "outCubic" })
        sol.present(window.id, { rotate_y = 35, perspective = 900 })
    elseif name == "spin" then
        sol.animate({ duration = 420, easing = { 0.34, 1.56, 0.64, 1 } })
        sol.present(window.id, { rotate_z = 12, perspective = 1200 })
    elseif name == "flat" then
        sol.animate({ duration = 220, easing = "outCubic" })
        sol.present(window.id, {})
    elseif name == "shrink" then
        sol.animate({ duration = 220, easing = "outCubic" })
        sol.present(window.id, {
            x = window.x + window.w * 0.1,
            y = window.y + window.h * 0.1,
            w = window.w * 0.8,
            h = window.h * 0.8,
        })
    elseif name == "fade" then
        sol.animate({ duration = 220, easing = "outCubic" })
        sol.present(window.id, { opacity = 0.5 })
    elseif name == "genie" then
        local area = sol.monitor()
        sol.animate({ duration = 520, easing = "inOutCubic" })
        sol.present(window.id, {
            genie = {
                x = area.x + area.w / 2 - 60,
                y = area.y + area.h - 24,
                width = 120,
                height = 24,
                spread = config.genie_spread or 1.4,
            },
        })
    end
end

return tweaks
