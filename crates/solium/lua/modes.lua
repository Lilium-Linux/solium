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
--
-- ## Surviving a reload
--
-- This file used to open with `local modes = { current = "floating", ... }`,
-- which is true exactly once: at startup. `super+shift+r` throws the Lua state
-- away and reads every script again, so after a reload that line declared the
-- session to be floating while every window was still sitting in the tile a
-- tree had put it in. Nothing looked wrong on screen and nothing appeared in
-- the log; the next `super+t` simply toggled the wrong way, because the file
-- and the desktop disagreed about what was already on.
--
-- That is the same defect #116 showed in `workspaces.lua`, wearing different
-- clothes, so it has the same answer -- and the answer belongs to the script
-- host rather than to either file. Only the host knows when the Lua state
-- dies, so only the host can carry anything across it. `sol.keep` is that.
--
-- It holds plain data, which is why `registered` stays out of it: those are
-- tables full of functions, the reload rebuilds them correctly by itself, and
-- the *name* is the only thing here that cannot be recomputed from what is on
-- screen.

local config = require("config")
local dialogs = require("dialogs")

local modes = { registered = {} }

-- Which layout is in charge, held by the host so it outlives the reload.
local kept = sol.keep("modes", { current = "floating" })

-- A function and not a field, because a field would be a second copy of this
-- and the two would drift the first time anything assigned to it.
function modes.current()
    return kept.current
end

function modes.register(name, layout)
    modes.registered[name] = layout
end

-- Whether the pointer moving over a window should focus it, in mode `name`
-- (#219): `config.focus.modes[name]`, or `config.focus.follow` -- which may
-- itself be `nil`, meaning the input profile's own answer for this machine
-- -- when the mode says nothing.
local function follow_for(name)
    local named = config.focus.modes[name]
    if named == "click" then
        return false
    end
    if named == "follow" then
        return true
    end
    return config.focus.follow
end

-- Hand the resolved model to the compositor. Called here whenever the mode
-- changes, and once more below at this file's own top level -- which is what
-- makes a reload answer correctly: `modes.use` runs only when a key actually
-- switches modes, but this file runs fresh on every load, startup and reload
-- alike, and `kept.current` is what survived the reload (see "Surviving a
-- reload" above). Without the top-level call a fresh Lua state would only
-- ever hear about the mode in charge when the user next switched away from
-- it, so `super+shift+r` while tiling would silently answer floating's
-- click-only default until the next `super+t`.
-- `a_reload_keeps_the_focus_model_tiling_was_switched_to`.
local function apply_focus_mode(name)
    sol.focus_mode({
        click = config.focus.click,
        follow = follow_for(name),
        clear_on_empty_click = config.focus.clear_on_empty_click,
    })
end

function modes.use(name)
    if kept.current == name then
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
    -- Every window out of its tile, before the next layout puts it in one.
    --
    -- The compositor holds a tiled window inside the rect `sol.place` gave it
    -- (#133), and only this file knows that the layout which gave it has
    -- stopped. Left alone, a window switched to floating would go on being cut
    -- to its old tile whenever it grew. Every window rather than the ones the
    -- old layout placed, because nothing here knows which those were -- and a
    -- layout starting below places its own windows again at once.
    --
    -- A window being closed is in the list too, and is let go like the rest;
    -- the compositor is what makes that wait. Its tile is what cuts it while
    -- it fades, so it keeps the tile until it has gone, or until a refused
    -- close brings it back, which is when the let-go is taken. See
    -- `a_mode_switched_during_a_fade_does_not_squash_the_window_leaving`.
    for _, window in ipairs(sol.windows()) do
        sol.unplace(window.id)
    end
    kept.current = name
    apply_focus_mode(name)
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

-- The model for whatever mode this Lua state opens already in -- "floating"
-- at a fresh start, or the one a reload carried over.
apply_focus_mode(kept.current)

-- Put the session back into the layout it was already in.
--
-- Remembering the name is only half of it. `layout.active` is a field on a
-- table this reload has just built fresh, so it is false for every layout no
-- matter what the kept name says -- and a mode that is current but not active
-- is the worst of the two halves: nothing arranges the windows, and `super+t`
-- toggles it *off*.
--
-- On `restore` rather than at this file's top level because the layouts have
-- not registered themselves yet when this file is read: `init.lua` requires
-- `modes` before `tiling` and `scrolling`, which is the order that lets a
-- layout register at all. `restore` is the host saying every script has
-- loaded, and it does not fire at startup -- where `current` is `floating`,
-- no layout is registered under that name, and there is nothing to put back.
sol.on("restore", function()
    local layout = modes.registered[kept.current]
    if not layout or layout.active then
        return
    end
    layout.active = true
    if layout.started then
        layout.started()
    end
    sol.status(kept.current)
end)

-- ## Out of the arrangement, and back (#150)
--
-- Three ways for the focused window to leave its tile. Floating is the
-- layouts': the window leaves the arrangement and floats over it. Maximised
-- and fullscreen are the compositor's, asked for as the frame's button and the
-- client's own request ask, and it takes the window out of its tile for both.
-- `a_script_toggles_fullscreen_and_maximised_as_the_client_would`,
-- `a_maximised_window_is_not_held_in_its_old_tile` and
-- `a_fullscreen_window_is_not_held_in_its_old_tile`.

-- Float window `id`, or put it back into the arrangement. Only while a layout
-- is in charge: with none every window floats already, and marking one would
-- change a desktop the user is not looking at -- it would only show on the
-- next `super+t`. `super_shift_space_floats_a_tiled_window_and_tiles_it_again`.
function modes.toggle_floating(id)
    local layout = modes.registered[kept.current]
    if not layout or not layout.active then
        return
    end
    dialogs.toggle(id)
    layout.apply()
end

sol.bind("super+shift+space", function()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            modes.toggle_floating(window.id)
            return
        end
    end
end)
sol.bind("super+f", function()
    sol.toggle_fullscreen()
end)
sol.bind("super+shift+m", function()
    sol.toggle_maximize()
end)

return modes
