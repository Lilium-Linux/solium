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

-- Who wants to know when the layout in charge changes, including to or from
-- floating -- which is never itself in `registered`. That absence is the
-- sentinel the rest of this file, `direction.lua` and `modes.toggle_floating`
-- read as "no layout is arranging anything", so floating cannot be
-- registered without changing what all three of those do. `lua/floating.lua`
-- wants to know the same transitions anyway -- to bind its snap keys only
-- while it is in charge, the way `overview.lua` binds Escape only while it is
-- up -- so this is the one small hook that lets it, without pretending to be
-- a layout. See `tiling_keeps_the_arrows_for_focus_and_desktop_mode_gets_them_back`
-- in `script.rs`, which switches out of tiling and back into floating and
-- checks the arrows mean something else each time.
local watchers = {}
function modes.watch(fn)
    watchers[#watchers + 1] = fn
end

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

-- What one `focus.modes` entry says: `false` for "click", `true` for
-- "follow", and `nil` for anything else, which counts as no entry.
local function word(entry)
    if entry == "click" then
        return false
    end
    if entry == "follow" then
        return true
    end
    return nil
end

-- Whether the pointer moving over a window should focus it, in mode `name`
-- (#219, #128). Highest first: your own `config.focus.modes[name]`; your own
-- `config.focus.follow`, for every mode at once; the shipped entry; and the
-- machine's own answer for its form factor (#159), which `nil` asks the
-- compositor for. `config.lua` keeps the shipped entries behind yours, in
-- the table's `__index`, so `rawget` reads yours alone. A shipped "follow"
-- is the machine's own answer -- follow where there is a pointer to hover,
-- not on a tablet or a phone -- so of the shipped words only "click" is
-- said here.
-- `your_mode_entry_then_your_focus_follow_then_the_shipped_entry_then_the_machine`
-- in `script.rs`, and
-- `your_focus_follow_false_keeps_every_mode_from_following_unless_you_name_one`
-- and `on_a_tablet_the_shipped_layouts_leave_hover_to_the_machine` in
-- `state/tests.rs`.
local function follow_for(name)
    local focus = config.focus
    local modes = type(focus.modes) == "table" and focus.modes or {}
    local yours = word(rawget(modes, name))
    if yours ~= nil then
        return yours
    end
    if focus.follow ~= nil then
        return focus.follow
    end
    local behind = getmetatable(modes)
    local shipped = type(behind) == "table" and type(behind.__index) == "table"
        and behind.__index[name]
    if word(shipped) == false then
        return false
    end
    return nil
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
--
-- `after_close` is the mode's own entry in `config.focus.after_close` (#128),
-- or nil -- the compositor's own rule, the window under the pointer and then
-- the topmost -- for a mode that has none, as for a mode registered outside
-- `config.lua`. `the_shipped_modes_each_hand_over_their_own_focus_model`.
local function apply_focus_mode(name)
    local after_close = config.focus.after_close
    sol.focus_mode({
        click = config.focus.click,
        follow = follow_for(name),
        clear_on_empty_click = config.focus.clear_on_empty_click,
        after_close = type(after_close) == "table" and after_close[name] or nil,
    })
end

-- How a window's picture is fitted while it changes size in mode `name`
-- (#233): your `config.resize.modes[name]`, or `config.resize.fill`, the one
-- for every mode. `init.lua` hands `config.resize` over before any mode is
-- known, which is all a configuration with no `resize.modes` entry needs, so
-- nothing is handed over again for one; with an entry, the mode's own fill
-- goes over with the background, the patience, `tell_hz`, `follow`, `sync` and `overlap` -- each call sets every key
-- -- whenever the mode changes, and once below at this file's top level, for
-- the reason `apply_focus_mode` gives.
-- `each_mode_fits_a_resize_by_its_own_fill_or_the_one_for_every_mode`.
local function apply_resize_mode(name)
    local resize = config.resize
    if type(resize) ~= "table" or type(resize.modes) ~= "table"
        or next(resize.modes) == nil then
        return
    end
    local own = resize.modes[name]
    sol.resize({
        fill = own ~= nil and own or resize.fill,
        background = resize.background,
        patience = resize.patience,
        tell_hz = resize.tell_hz,
        follow = resize.follow,
        sync = resize.sync,
        overlap = resize.overlap,
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
    --
    -- And out of a maximise, when a layout is taking over: its tile is where
    -- the window goes. A maximise is the floating desktop's way of filling
    -- the screen, and kept, the window went on covering the work area over
    -- the tiles while the layout held its own tile empty beside it, and no
    -- seam could size it, since the compositor holds a maximised window at
    -- its own rect (#443). Only at the switch: `super+shift+m` under a
    -- layout leaves that layout's tile on purpose and keeps it to come back
    -- to (#150). Not a fullscreen window, which leaves fullscreen on its own
    -- terms, nor one minimized or being closed, which no layout is placing.
    -- `a_window_maximised_while_floating_joins_its_tile_when_a_layout_takes_over`.
    local layout_next = modes.registered[name]
    for _, window in ipairs(sol.windows()) do
        sol.unplace(window.id)
        if layout_next and window.maximized and not window.fullscreen
            and not window.minimized and not window.leaving then
            sol.toggle_maximize(window.id)
        end
    end
    kept.current = name
    apply_focus_mode(name)
    apply_resize_mode(name)
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
    -- After the switch has fully landed, so a watcher that asks `modes.current()`
    -- or looks at `sol.windows()` sees the mode it was just told about.
    for _, watch in ipairs(watchers) do
        watch(name)
    end
end

-- The model for whatever mode this Lua state opens already in -- "floating"
-- at a fresh start, or the one a reload carried over -- and its fill.
apply_focus_mode(kept.current)
apply_resize_mode(kept.current)

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
