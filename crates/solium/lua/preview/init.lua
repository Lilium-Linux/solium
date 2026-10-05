-- The preview shell: the compositor's own showcase, built as configuration
-- on the public API (docs/shell-boundary.md), nothing more. Today it is one
-- piece, the bottom bar, `qml/preview/Shell.qml`; see docs/ricing.md for what
-- it shows and what is still to come.
--
-- `preview = false` in `user.lua` turns the whole thing off. Short of that,
-- this only ever fills in a default: a `shell.scene` named in `user.lua`, or
-- `SOLIUM_SHELL_SCENE` for one run, always wins -- see `shell.lua`, which
-- reads the environment first and this file's default second.
--
-- Required as `require("preview.init")` (`package.path` has no `?/init.lua`
-- pattern, so the bare name would not resolve), and before `require("shell")`
-- in `init.lua`, or it would be setting a value `shell.lua` already read.

local config = require("config")

local preview = {}

function preview.apply()
    if config.preview == false then
        return
    end
    if not config.shell.scene then
        config.shell.scene = "preview/Shell.qml"
    end
    -- The top dock's own policy (pins, visibility): see its module doc for
    -- why it is a separate file and why it runs after the line above, not
    -- before it (it reads and writes `config.shell`, which this function
    -- just gave a default `scene`).
    require("preview.dock")
    -- Quick search's own policy (04-ui.md §4.7: the key, the matching, the
    -- commands) -- see its module doc for the same ordering reason.
    require("preview.search")
end

preview.apply()

return preview
