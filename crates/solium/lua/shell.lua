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
-- One scene, on the primary monitor. The primary monitor and not the active
-- one, for the same reason a dock goes there: a bar that moves screens when
-- the pointer does is a bar nobody asked to move.

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

-- The primary monitor's usable area: the one `primary = true` picks out, or
-- the first when none is marked. See
-- `the_shell_is_on_the_primary_monitor_not_the_focused_one`.
local function primary()
    local first = nil
    for _, monitor in ipairs(sol.monitors()) do
        if monitor.primary then
            return monitor
        end
        first = first or monitor
    end
    return first or sol.monitor()
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
    local area = primary()
    sol.surface("shell", {
        scene = chosen,
        layer = "top",
        on = { x = area.x, y = area.y, w = area.w, h = area.h },
        interactive = true,
        -- What shell components ask about the screen they are on.
        properties = {
            screenInfo = {
                name = "primary",
                x = area.x,
                y = area.y,
                width = area.w,
                height = area.h,
                scale = 1,
            },
        },
    })
end

-- On the `monitors` event and not here: scripts load before the screens are
-- known -- on the hardware backend, before the GPU is open -- so a rect
-- computed now is computed against zeros. It fires once at startup, again on
-- every hotplug, and after every reload, which is when this needs redoing
-- anyway. See `the_shell_scene_is_read_from_the_configuration`.
sol.on("monitors", shell.apply)

return shell
