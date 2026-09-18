-- How a window appears.
--
-- In the compositor this is a fallback; here it is the actual behaviour, and
-- that is the point: the animation a window plays when it opens is a script's
-- decision, so changing it does not mean changing the compositor.
--
-- The interesting version of this is the one that has not been built yet. When
-- the dock publishes its icon rectangles (see docs/shell-boundary.md), this
-- handler asks for the icon belonging to the window's application and passes
-- *that* to `sol.present_from` — and a window grows out of the icon that
-- launched it, macOS-style. Same function, same primitive, different rectangle.

-- The numbers are `config.open`'s, and were not until #117.
--
-- This file held `{ duration = 220, easing = "outBack" }` and `0.88` as local
-- constants while `config.lua` advertised `open.motion` and `open.scale` at
-- 200 / outCubic / 0.92 -- two answers to one question, with the written-down
-- one being the answer nobody was using. Editing the setting did nothing, and
-- there was no way to tell that from having misunderstood what it meant. The
-- defaults over there are now these numbers rather than those, because these
-- are what every session so far has actually been watching; the full reasoning
-- is at the setting.
local config = require("config")

-- Shrunk about its own centre: "smaller, in place".
local function shrunk(window, factor)
    local w, h = window.w * factor, window.h * factor
    return {
        x = window.x + (window.w - w) / 2,
        y = window.y + (window.h - h) / 2,
        w = w,
        h = h,
        opacity = 0,
    }
end

sol.on("open", function(id)
    for _, window in ipairs(sol.windows()) do
        if window.id == id then
            sol.animate(config.open.motion)
            -- Where it comes *from*; the compositor animates it to where it
            -- actually lives.
            sol.present_from(id, shrunk(window, config.open.scale))
            return
        end
    end
end)
