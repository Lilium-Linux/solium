-- How a window goes fullscreen or maximised, and comes back.
--
-- The compositor makes the change: the application is told its new size the
-- moment the key is pressed, and the window lives at its new rectangle from
-- then on. Then it moves the window's picture there from wherever it is drawn,
-- on the same clock and through the same transform a layout uses to move a
-- window. How long that takes, on which curve, and for which applications it
-- is instant, is this file's to say, from `fullscreen` and `maximize` in
-- config.lua -- each read on its own, so the two can move differently.
--
-- It answers with `sol.animate`. A listener that set nothing would leave the
-- change instant, which is what `{ duration = 0 }` says out loud below.
-- Played, with Russian live, by `tests/scenarios/fullscreen-glides.lua`,
-- `fullscreen-instant.lua`, `fullscreen-animate-false.lua`,
-- `maximize-animate-false.lua`, `fullscreen-animate-true.lua`,
-- `fullscreen-and-maximize-timed-apart.lua` and `fullscreen-tiled.lua`.

local config = require("config")

local INSTANT = { duration = 0 }

-- What config.lua ships as each `animate`, for `animate = true`: the merge
-- puts `true` where the shipped motion was, and `true` is what turns the
-- animation back on after `false`, not what turns it off.
-- `tests/scenarios/fullscreen-animate-true.lua`.
local SHIPPED = {
    fullscreen = { duration = 260, easing = "outCubic" },
    maximize = { duration = 220, easing = "outCubic" },
}

-- Whether `app_id` is one of `names`.
local function listed(names, app_id)
    if type(names) ~= "table" or app_id == nil or app_id == "" then
        return false
    end
    for _, name in ipairs(names) do
        if name == app_id then
            return true
        end
    end
    return false
end

-- The motion window `id` changes with under `settings`, which is
-- `config.fullscreen` or `config.maximize`, shipped as `shipped`: its
-- `animate`, unless that is `false` or the application is on its `instant`
-- list.
local function motion(settings, shipped, id)
    if type(settings) ~= "table" then
        return INSTANT
    end
    local animate = settings.animate
    if animate == true then
        animate = shipped
    end
    if type(animate) ~= "table" then
        return INSTANT
    end
    local instant = type(settings.instant) == "table" and settings.instant.app_id or nil
    for _, window in ipairs(sol.windows()) do
        if window.id == id and listed(instant, window.app_id) then
            return INSTANT
        end
    end
    return animate
end

sol.on("fullscreen", function(id)
    sol.animate(motion(config.fullscreen, SHIPPED.fullscreen, id))
end)

sol.on("maximize", function(id)
    sol.animate(motion(config.maximize, SHIPPED.maximize, id))
end)
