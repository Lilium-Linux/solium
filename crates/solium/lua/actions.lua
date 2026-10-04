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

local actions = { overrides = {} }

-- The services whose actions make up the vocabulary in this release.
local services = { windows = true, workspaces = true }

-- Answer `name` in Lua instead: `handler(data, surface)`.
function actions.override(name, handler)
    actions.overrides[name] = handler
end

function actions.run(surface, action, data)
    local handler = actions.overrides[action]
    if handler then
        return handler(data, surface)
    end
    local service = action:match("^(%a+)%.")
    if service and services[service] then
        sol.act(action, data)
    end
end

sol.on("surface", actions.run)

return actions
