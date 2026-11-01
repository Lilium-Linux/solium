-- A fullscreen window stays as it is: a drag on it (super+drag, a finger, or
-- its own client asking to be moved) holds it still for the whole drag, and a
-- resize of it (an edge, super+right drag, a finger) is held for the whole
-- resize, so it never comes off its monitor or changes size. Leaving
-- fullscreen is super+f.
--
-- Mechanism only from Solium: the `drag` and `resizing` events with
-- `sol.drag_hold` (#452) and `sol.resize_hold` (#454); the rule is this file.

local held = {}

local function fullscreen(id)
    for _, window in ipairs(sol.windows()) do
        if window.id == id then
            return window.fullscreen == true
        end
    end
    return false
end

sol.on("drag", function(id, phase)
    if phase == "begin" then
        if fullscreen(id) then
            held[id] = true
            sol.drag_hold(id, true)
        end
    elseif phase == "end" and held[id] then
        held[id] = nil
        sol.drag_hold(id, false)
    end
end)

-- A hold lasts for that one resize, so nothing needs letting go at its end.
sol.on("resizing", function(id, phase)
    if phase == "begin" and fullscreen(id) then
        sol.resize_hold(id, true)
    end
end)

return {}
