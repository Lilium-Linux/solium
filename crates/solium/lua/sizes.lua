-- What a window's own application says about its size (#115), for the layouts.
--
-- `sol.windows()` carries `min` and `max` for every window whose application
-- limited them, frame included -- the same terms as the window's `w` and `h`,
-- so they compare with a tile directly. Whether a layout listens is a setting,
-- and it is one setting for both layouts because it is one question: does this
-- application get a say. The user's answer outranks the application's, and
-- these are where it is written down:
--
--   tiling.client_minimum      "respect" (the default) or "ignore"
--   tiling.client_maximum      "center" (the default) or "ignore"
--   tiling.client_size_ignore  applications, by app_id, whose sizes are not
--                              believed at all
--   floating.client_limits     "respect" (the default) or "ignore", for a
--                              floating window's edge drag
--
-- See config.lua for what each does to an arrangement.

local config = require("config")

local sizes = {}

-- A floating window's edge drag is held to its application's sizes by the
-- compositor, since no layout is asked about one: so the two settings that
-- say whether it may be are handed over, once, as this loads.
-- `client_size_ignore` is the same list the layouts read,
-- so an application not believed there is not believed here either. A
-- configuration from before `floating` existed has no such table, and is the
-- default. See `ClientSizes` in `script.rs`, and
-- `the_floating_setting_and_the_ignored_applications_reach_the_compositor`.
sol.client_sizes({
    floating = type(config.floating) == "table" and config.floating.client_limits or nil,
    ignore = config.tiling.client_size_ignore,
})

-- Whether this window's application is one the user has said not to believe.
--
-- A list that is not a list believes everyone, which is how tiling behaved
-- before the setting existed.
local function ignored(window)
    local list = config.tiling.client_size_ignore
    if type(list) ~= "table" or not window.app_id or window.app_id == "" then
        return false
    end
    for _, app in ipairs(list) do
        if app == window.app_id then
            return true
        end
    end
    return false
end

-- The smallest this window may be laid out, as its application says, or nil
-- when it said nothing or is not listened to. Anything but "ignore" is
-- "respect".
function sizes.floor(window)
    if config.tiling.client_minimum == "ignore" or ignored(window) then
        return nil
    end
    return window.min
end

-- The largest, likewise. Anything but "ignore" is "center".
function sizes.maximum(window)
    if config.tiling.client_maximum == "ignore" or ignored(window) then
        return nil
    end
    return window.max
end

-- Every window's floor, by id, as a layout's `options.floors` takes them.
function sizes.floors(windows)
    local out = {}
    for _, window in ipairs(windows or sol.windows()) do
        local floor = sizes.floor(window)
        if floor then
            out[window.id] = { w = floor.w, h = floor.h }
        end
    end
    return out
end

-- Where a window goes in `tile`: the whole of it, or -- where the window's
-- maximum is smaller than the tile -- a pane of that size in its middle, on
-- each side the maximum limits. Nil when it is the whole tile.
function sizes.centred(window, tile)
    local max = window and sizes.maximum(window)
    if not max then
        return nil
    end
    local w = (max.w > 0 and tile.w > max.w) and max.w or tile.w
    local h = (max.h > 0 and tile.h > max.h) and max.h or tile.h
    if w == tile.w and h == tile.h then
        return nil
    end
    return {
        x = tile.x + (tile.w - w) / 2,
        y = tile.y + (tile.h - h) / 2,
        w = w,
        h = h,
    }
end

return sizes
