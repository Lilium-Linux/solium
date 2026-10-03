-- Hosting a shell inside the compositor.
--
-- A shell -- the bar, the dock, the launcher -- is its own project, and it
-- runs here as your configuration: its QML is loaded into the same engine that
-- draws the window frames, beside them. `shell.scene` in `config.lua` names
-- its root file, and `SOLIUM_SHELL_SCENE` overrides that for one run. See
-- docs/shell-boundary.md.
--
-- It used to be a compositor feature -- an accessor on the state, its own
-- pointer routing, its own render hook and an environment variable read in
-- Rust. It is `sol.surface` now.
--
-- One scene on every monitor by default, and each instance reads its own as
-- `Solium.monitor`: `shell.on` in config.lua, which may also say "primary" or
-- name a connector. Declared when the configuration runs, which a reload does
-- again, so taking the shell out of the configuration takes it away. See
-- `the_shell_is_on_every_monitor_unless_the_configuration_names_one`.

local config = require("config")

local shell = {}

-- The scene to host, or nil for none. The environment first, because it is set
-- per run; see `the_environment_overrides_the_configured_shell_scene`.
local function scene()
    local override = os.getenv("SOLIUM_SHELL_SCENE")
    if override and override ~= "" then
        return override
    end
    local setting = type(config.shell) == "table" and config.shell.scene or nil
    if type(setting) == "string" and setting ~= "" then
        return setting
    end
    return nil
end

function shell.apply()
    local chosen = scene()
    if not chosen then
        -- A surface outlives a reload until something removes it by name, so
        -- a shell taken out of the configuration would otherwise stay on
        -- screen. See `the_shipped_configuration_hosts_no_shell`.
        sol.surface("shell", false)
        return
    end
    local settings = type(config.shell) == "table" and config.shell or {}
    sol.surface("shell", {
        scene = chosen,
        layer = "top",
        on = settings.on or "every-monitor",
        interactive = true,
        -- What a press outside an open popup does is the user's: see
        -- `the_shell_takes_its_outside_click_from_the_configuration`.
        outside_click = settings.outside_click,
        -- And which bindings still work while it holds the keyboard: see
        -- `the_shell_takes_its_keyboard_bindings_from_the_configuration`.
        keyboard = settings.keyboard,
    })
end

shell.apply()

return shell
