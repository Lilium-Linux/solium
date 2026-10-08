-- effects-check's `tilt` section: blur.lua's scene (a still terminal on the
-- left, a video player titled "effects-check-player" on the right), the
-- player blurred with its own pixels when BLUR=player, as blur.lua blurs it,
-- and two bindings that warp the player, so its capture is what is drawn:
--
--   super+t   holds it at rotate_z = 6, as dev/pacing/tilt.lua holds a window
--   super+m   pulls it into a rect at the bottom of the monitor, as the
--             shipped super+m genie does, over GENIE_MS (520 by default)
--
-- A warped pane's capture walks its slots, so the player stays blurred while
-- it is tilted and while it genies (Ruling 17).
sol.pane("none")
local LEFT = { x = 40, y = 60, w = 560, h = 380 }
local RIGHT = { x = 640, y = 60, w = 560, h = 380 }
local PLAYER = "effects-check-player"
local opened = 0
sol.on("open", function(id)
    opened = opened + 1
    sol.animate({ duration = 0 })
    sol.place(id, opened == 1 and LEFT or RIGHT)
end)
if os.getenv("BLUR") == "player" then
    sol.effects({ rules = { { match = { title = PLAYER }, part = "client", slot = "replace",
                              effect = { "blur", source = "self", passes = 3 } } } })
end

local function player()
    for _, window in ipairs(sol.windows()) do
        if window.title == PLAYER then
            return window.id
        end
    end
end

sol.bind("super+t", function()
    local id = player()
    if id then
        sol.animate({ duration = 300, easing = "outCubic" })
        sol.present(id, { rotate_z = 6 })
    end
end)

sol.bind("super+m", function()
    local id = player()
    if id == nil then
        return
    end
    local area = sol.monitor()
    sol.animate({ duration = tonumber(os.getenv("GENIE_MS") or "520"), easing = "inOutCubic" })
    sol.present(id, {
        deform = {
            effect = "genie",
            axis = "down",
            spread = 1.4,
            to = { x = area.x + area.w / 2 - 60, y = area.y + area.h - 24, w = 120, h = 24 },
        },
    })
end)
