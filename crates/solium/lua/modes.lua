-- Which layout is in charge.
--
-- Tiling and scrolling were independent toggles, so both could be on at once
-- and both would place every window — the second one to run winning, and the
-- arrangement looking like whichever that happened to be. Switching from
-- scrolling to tiling left windows in columns, because scrolling was still
-- running and still placing them.
--
-- A layout is a choice of one, so it is held in one place. Registering here
-- rather than having each layout know about the others keeps a new layout from
-- needing to be told about every layout that came before it.

local modes = { current = "floating", registered = {} }

function modes.register(name, layout)
    modes.registered[name] = layout
end

function modes.use(name)
    if modes.current == name then
        name = "floating"
    end
    for other, layout in pairs(modes.registered) do
        if other ~= name and layout.active then
            layout.active = false
            if layout.stopped then
                layout.stopped()
            end
        end
    end
    modes.current = name
    local layout = modes.registered[name]
    if layout then
        layout.active = true
        if layout.started then
            layout.started()
        end
        sol.status(name)
    else
        sol.status("")
    end
end

return modes
