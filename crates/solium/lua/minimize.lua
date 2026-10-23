-- Minimize (#30): `super+m` puts the focused window away, with the genie into
-- a slot at the bottom of its monitor, and the genie plays back out when the
-- window is shown again.
--
-- The compositor keeps the state and nothing else: a minimized window is not
-- drawn once the present on it has played out, takes no pointer or touch,
-- and never holds the keyboard. How it goes and how it comes back is this
-- file's, from the `minimize` and `unminimize` events -- whoever minimized
-- it: this key, `sol.minimize`, or the application's own button. Focusing a
-- minimized window shows it again (`sol.focus`, a click in the overview, an
-- application asking to be brought forward). The layouts leave a minimized
-- window out of their arrangement and take it back where it was.
--
-- `config.minimize` sets how long the genie plays, how much of the window
-- trails behind it, and the size of the slot it goes into.

local config = require("config")

local minimize = {}

-- The slot a window on monitor `area` goes into: `config.minimize.slot`'s
-- size, centred on the bottom edge of the area.
function minimize.slot(area)
    local slot = config.minimize.slot
    return {
        x = area.x + area.w / 2 - slot.w / 2,
        y = area.y + area.h - slot.h,
        w = slot.w,
        h = slot.h,
    }
end

-- Pull window `id` into its slot with the genie, over `config.minimize.motion`.
-- What the `minimize` event plays, the overview puts a minimized window back
-- with, and the Developer Tweaks panel's genie shows.
--
-- `deform` names an effect folder: `genie` ships, and a copy in
-- ~/.config/solium/effects/genie/ is used instead. `axis` is which edge leads,
-- so a dock down the side of the screen is `"left"`, and `"auto"` picks the
-- side the slot lies on when the window starts to move.
function minimize.into(id)
    sol.animate(config.minimize.motion)
    sol.present(id, {
        deform = {
            effect = "genie",
            axis = "down",
            spread = config.minimize.spread,
            to = minimize.slot(sol.monitor(id)),
        },
    })
end

-- A window was minimized: it goes into its slot, and is drawn until it is
-- there.
sol.on("minimize", function(id)
    minimize.into(id)
end)

-- It is shown again: the genie back out of the slot, which is the present
-- going back to the window's own rectangle. `present_clear` lets the present
-- go once it lands.
--
-- Not while the overview is up: a mode's picture is the mode's, and the
-- overview draws the window as one of its thumbnails until it leaves, which
-- clears every window it drew. A window shown again by an application asking
-- to be brought forward stays in the grid.
-- `a_window_shown_again_during_the_overview_stays_in_its_grid`.
sol.on("unminimize", function(id)
    local overview = package.loaded["overview"]
    if overview and overview.active then
        return
    end
    sol.animate(config.minimize.motion)
    sol.present_clear(id)
end)

-- The focused window, put away. Nothing when nothing is focused.
sol.bind("super+m", function()
    sol.minimize()
end)

return minimize
