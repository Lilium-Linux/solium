-- The action vocabulary, routed (03 §3.1.5).
--
-- A scene asks with `Solium.send("windows.focus", { id: model.id })`, and Lua
-- decides. An action a file of the configuration answers itself, with
-- `actions.override`, goes there: the compositor does not know what a
-- workspace is, so `workspaces.go` is for whichever file keeps the
-- workspaces to answer. Any other action of the vocabulary goes to the
-- compositor through `sol.act`, which answers one it does not know with
-- "unknown-action". Anything else is left to whoever listens for the surface
-- by name, as `tweaks.lua` does. Copy this file to change how any of it is
-- answered.
-- See `windows_focus_from_a_scene_focuses_the_window` and
-- `actions_lua_routes_the_vocabulary_and_leaves_the_rest_alone`.

local actions = {}

-- The services whose actions make up the vocabulary in this release.
local services = { windows = true, workspaces = true, apps = true }

-- Each name's override now, so one replaced by a later override of the same
-- name answers nothing. See `a_later_override_of_the_same_name_replaces_the_earlier_one`.
local current = {}

-- An action of the vocabulary, to the compositor.
local function to_the_compositor(action, data)
    local service = action:match("^(%a+)%.")
    if service and services[service] then
        sol.act(action, data)
    end
end

-- Answer `name` in Lua instead: `handler(data, surface)`.
--
-- Each override is a `surface` listener of its own, under the deadline every
-- listener has, so one that runs too long is struck, and taken out at its
-- third stop, alone: every other action is still routed, and `name` goes to
-- the compositor again. See `an_override_stopped_three_times_is_taken_out_alone`.
function actions.override(name, handler)
    -- An override of nil is none: `name` goes to the compositor again. See
    -- `an_override_of_nil_gives_its_action_back_to_the_compositor`.
    if handler == nil then
        current[name] = nil
        return
    end
    local this = {}
    current[name] = this
    local heard = false
    sol.on("surface", function(surface, action, data)
        if action == name and current[name] == this then
            heard = true
            handler(data, surface)
        end
    end)
    -- Added after the override, so it runs after it every time: an action
    -- the override did not hear, because it was taken out, goes to the
    -- compositor. See `an_override_stopped_three_times_is_taken_out_alone`.
    sol.on("surface", function(_, action, data)
        if action == name and current[name] == this then
            if not heard then
                to_the_compositor(action, data)
            end
            heard = false
        end
    end)
end

-- Route an action no override answers. See
-- `actions_lua_routes_the_vocabulary_and_leaves_the_rest_alone`.
function actions.run(_, action, data)
    if not current[action] then
        to_the_compositor(action, data)
    end
end

sol.on("surface", actions.run)

return actions
