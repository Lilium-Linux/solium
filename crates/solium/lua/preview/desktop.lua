-- The preview desktop icons (04-ui.md §4.9): a QML scene declared with
-- `sol.surface`, no Rust, exactly the way `wallpaper.lua` and `tweaks.lua`
-- already declare theirs -- see `docs/ricing.md`'s "the more interesting
-- part" for why that is the pattern rather than a special case. `layer =
-- "bottom"` is below ordinary windows, same as a real desktop's icons, and
-- below `shell.lua`'s own `layer = "top"` bar and dock: a separate surface
-- from `config.shell.scene`, not a piece added to it.
--
-- `qml/preview/Desktop.qml` reads `Folder` (the compositor's own live scan
-- of the desktop folder, `crates/solium/src/folder.rs`) directly, the same
-- way `qml/preview/Dock.qml` reads `Apps` directly: this file only ever
-- sets the *policy* knobs (which corner, how big a cell, how many label
-- lines, open-on-one-click or two), never the data.
--
-- Cut for this first version, so nothing here answers them: `sort` beyond
-- "folders first, then by name" (`crate::folder::scan` already does that,
-- wholesale, so there is no second sort to configure yet), positions kept
-- across a restart, `reveal_on_click` (nothing shows the desktop yet, so
-- there is nothing for a wallpaper click to reveal), and the empty-desktop
-- menu's own commands -- all the same `sol.store`-shaped gap
-- `preview.dock`'s own module doc names.

local config = require("config")

local desktop = {}

local function settings()
    local preview = config.preview
    if type(preview) == "table" and type(preview.desktop) == "table" then
        return preview.desktop
    end
    return {}
end

function desktop.apply()
    -- Whatever was declared last time goes first, so turning icons off on a
    -- reload does not leave the surface behind with nobody that knows its
    -- name (`wallpaper.lua`'s own module doc explains the same step).
    sol.surface("desktop", false)
    local wanted = settings()
    if wanted.icons == false then
        return
    end
    sol.surface("desktop", {
        scene = "preview/Desktop.qml",
        layer = "bottom",
        on = "primary",
        interactive = true,
        properties = {
            from = wanted.from or "top-right",
            cellWidth = (type(wanted.cell) == "table" and wanted.cell.width) or 96,
            cellHeight = (type(wanted.cell) == "table" and wanted.cell.height) or 100,
            labelLines = wanted.labels or 2,
            openOn = wanted.open or "double",
            showHidden = wanted.show_hidden or false,
        },
    })
end

desktop.apply()

return desktop
