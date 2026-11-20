-- The pointer follows the keyboard (#220).
--
-- When focus moves by a key, the pointer goes to the window that took the
-- keyboard; when it moves by a click, it stays where the hand put it. Which is
-- which is `config.focus.cursor.follows`, by the `source` the `focus` event is
-- told, and all the compositor provides is the warp: `sol.warp` puts the
-- pointer there at once and flies its picture along `motion`, so what the
-- pointer is over is the new window's from the first instant.
--
-- Only when the keyboard actually moved: a layout focusing again the window
-- that has it -- scrolling does after every key it binds, `super+r` that only
-- widens a column included -- is told `changed = false`, and takes the pointer
-- nowhere unless `focus.cursor.refocus` says so.
--
-- Two kinds of change tell no `focus` that moved the keyboard, and are asked
-- for here by name: a move, which keeps the keyboard on the window that moved
-- (`direction.lua` calls `follow.window` once the layout has moved it, and
-- `scrolling.lua` does for its keys that move a column, or a window out of
-- one), and a workspace switch that leaves no window to take the keyboard
-- (`workspaces.lua` calls `follow.monitor`) -- or that comes back to the
-- window the keyboard stayed on while it was hidden, which `workspaces.lua`
-- asks for with `follow.window`.
-- `the_pointer_follows_a_key_and_not_a_click`,
-- `a_move_takes_the_pointer_to_where_the_window_went`,
-- `in_scrolling_a_key_takes_the_pointer_along_only_if_it_moves_the_keyboard_or_its_window`,
-- `a_switch_to_an_empty_workspace_takes_the_pointer_to_the_middle_or_leaves_it`
-- and their neighbours in `script.rs`'s `follow` tests.

local config = require("config")
local monitors = require("monitors")

local follow = {}

local function settings()
    return config.focus.cursor or {}
end

-- Say a setting that means nothing once, rather than on every key.
local said = {}
local function once(message)
    if not said[message] then
        said[message] = true
        sol.log(message)
    end
end

-- Whether a focus change from `source` takes the pointer along.
function follow.wanted(source)
    local follows = settings().follows
    return type(follows) == "table" and follows[source] == true
end

-- How the picture travels: the configured motion, or nil, which puts it
-- there at once.
local function motion()
    local given = settings().motion
    if type(given) == "table" then
        return given
    end
    if given ~= nil and given ~= false then
        once(string.format(
            "focus.cursor.motion is %s, which is neither a motion nor false; the pointer is put there at once",
            tostring(given)
        ))
    end
    return nil
end

-- Where the mouse carries on from if it moves before the picture lands:
-- "pointer", where the warp put it, or "picture", where the picture is.
-- `which_focus_changes_take_the_pointer_along_where_to_and_how_is_configured`.
local function interrupt()
    local given = settings().interrupt
    if given == nil or given == "pointer" or given == "picture" then
        return given
    end
    once(string.format(
        "focus.cursor.interrupt is %s, which is neither \"pointer\" nor \"picture\"; the mouse carries on from the pointer",
        tostring(given)
    ))
    return nil
end

-- The options every warp here is asked with.
local function options(to)
    return { to = to, motion = motion(), interrupt = interrupt() }
end

-- Take the pointer to window `id`, if a focus change from `source` does.
function follow.window(id, source)
    if id ~= nil and follow.wanted(source) then
        sol.warp(id, options(settings().to or "center"))
    end
end

-- Take the pointer to the middle of monitor `name`, for a workspace switch
-- from `source` that left no window to take the keyboard, if
-- `focus.cursor.empty_workspace` says so and that source takes it along.
function follow.monitor(name, source)
    local empty = settings().empty_workspace
    if empty ~= "center" then
        if empty ~= "stay" then
            once(string.format(
                "focus.cursor.empty_workspace is %s, which is neither \"center\" nor \"stay\"; the pointer stays",
                tostring(empty)
            ))
        end
        return
    end
    if not follow.wanted(source) then
        return
    end
    local monitor = monitors.named(name)
    if monitor then
        sol.warp({ x = monitor.x + monitor.w / 2, y = monitor.y + monitor.h / 2 }, options(nil))
    end
end

-- The keyboard moved, or a window that had it was focused again, which takes
-- the pointer along only if `focus.cursor.refocus` says so.
-- `in_scrolling_a_key_takes_the_pointer_along_only_if_it_moves_the_keyboard_or_its_window`.
sol.on("focus", function(id, source, changed)
    if changed or settings().refocus == true then
        follow.window(id, source)
    end
end)

return follow
