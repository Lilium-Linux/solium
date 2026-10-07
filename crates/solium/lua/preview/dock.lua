-- The preview dock's policy (04-ui.md §4.6): forwards `preview.dock`'s
-- settings onto the shell's root item as plain properties
-- (`shell.lua`'s generic `properties` passthrough -- see its own comment for
-- why that is enough to need no model of its own).
--
-- **Where "the first installed app from each role" is decided, and why
-- it is not here.** A `.lua` file's top level runs with `sol.apps()` (and
-- `sol.windows()`, `sol.monitors()`) reading an empty, default snapshot --
-- the real one arrives only once the compositor dispatches an event to the
-- scripts it just loaded (`script.rs::load_carrying` sets a default
-- `Snapshot` before anything here runs; `Solium::restored` hands over the
-- live one afterwards). Worse, on a first start `Solium.Apps` has not even
-- been scanned yet at that point (`models::mod::publish_models` scans it
-- lazily, on the first frame, which is not ordered against the first
-- dispatch). So this file names *candidates*, and `qml/preview/Dock.qml`
-- picks the first one `Solium.Apps` -- always live, always current -- says
-- is installed, once `Solium.Apps.ready`. `preview.dock.pinned` in
-- `user.lua` skips all of that and pins exactly that list.
--
-- Also cut for this first version: `sol.store` (a pin set here or picked by
-- the dock is not kept across a restart -- it is recomputed the same way
-- every time, which is a no-op for an explicit `pinned` and harmless for the
-- computed default), and the click policy's finer points (middle-click, the
-- wheel, dragging to reorder or unpin): `Dock.qml` sends the two native
-- actions a click needs (`apps.launch`, `windows.focus`) directly, which is
-- simple enough to need no Lua handler of its own.

local config = require("config")

local dock = {}

-- One role's candidates, most to least common -- the same idea as
-- `init.lua` picking a terminal with `sol.which`, just against desktop ids.
-- `qml/preview/Dock.qml` keeps its own copy (QML cannot read a Lua table at
-- scene load; see the module doc above for why this cannot simply be
-- computed here instead and handed over).
dock.candidates = {
    { "org.gnome.Console", "org.kde.konsole", "kitty", "foot", "alacritty", "gnome-terminal" },
    { "org.kde.dolphin", "org.gnome.Nautilus", "nautilus", "pcmanfm", "Thunar", "nemo" },
    { "org.mozilla.firefox", "firefox", "org.chromium.Chromium", "chromium-browser", "chromium" },
}

local function settings()
    local preview = config.preview
    if type(preview) == "table" and type(preview.dock) == "table" then
        return preview.dock
    end
    return {}
end

function dock.apply()
    local wanted = settings()
    config.shell = type(config.shell) == "table" and config.shell or {}
    config.shell.properties = type(config.shell.properties) == "table" and config.shell.properties or {}
    local properties = config.shell.properties
    -- `nil`, not `{}`, for "name none": `Dock.qml` tells "not configured"
    -- from "configured empty" so it only computes a default in the first
    -- case (`model.var === undefined`).
    properties.dockPinned = wanted.pinned
    properties.dockVisibility = wanted.visibility or "autohide"
    properties.dockIconSize = wanted.icon_size or 40
end

dock.apply()

return dock
