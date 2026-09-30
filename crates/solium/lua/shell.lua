-- Hosting a shell inside the compositor.
--
-- `SOLIUM_SHELL_SCENE=/path/to/shell.qml` runs a QML shell in-process, which
-- is a development affordance rather than the supported way to have a bar:
-- see docs/shell-boundary.md. A real shell is a layer-shell client, and one of
-- those is drawn over anything declared here.
--
-- It used to be a compositor feature -- an accessor on the state, its own
-- pointer routing, its own render hook and an environment variable read in
-- Rust. It is `sol.surface` now, and the only thing left in the compositor is
-- the Quickshell compatibility layer, which is a different job.
--
-- One scene, on the primary monitor. A shell that wants a bar on every screen
-- writes layer surfaces, one per output -- giving this one a screen each would
-- be building the multi-monitor shell the compositor has no business owning.
-- The primary monitor and not the active one, for the same reason a dock goes
-- there: a bar that moves screens when the pointer does is a bar nobody asked
-- to move.

local shell = {}

local scene = os.getenv("SOLIUM_SHELL_SCENE")

function shell.apply()
    if not scene then
        return
    end
    local area = sol.monitor()
    sol.surface("shell", {
        scene = scene,
        layer = "top",
        on = { x = area.x, y = area.y, w = area.w, h = area.h },
        interactive = true,
        -- What shell components ask about the screen they are on. The window
        -- list is not here: that reaches QML through the `Solium` singleton,
        -- which any scene can read.
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
-- computed now is computed against zeros. It fires once at startup and again
-- on every hotplug, which is when this needs redoing anyway.
if scene then
    sol.on("monitors", shell.apply)
end

return shell
