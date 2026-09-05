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

local APPEAR = { duration = 220, easing = "outBack" }

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
            sol.animate(APPEAR)
            -- Where it comes *from*; the compositor animates it to where it
            -- actually lives.
            sol.present_from(id, shrunk(window, 0.88))
            return
        end
    end
end)
