-- Overview: every window scaled onto a grid.
--
-- This file is the architecture's proof. Overview is not a compositor feature;
-- it is a script that sets a target rect per window and lets the compositor's
-- one animation clock get them there. The app switcher is this with a row
-- instead of a grid, peek is this with one window at the cursor, and the
-- icon-to-window genie is this with a dock icon named as the thing the window
-- comes out of.
--
-- If any of those ever needs new Rust, the transform layer is missing
-- something and *that* is the bug to fix -- not this file.

local monitors = require("monitors")

-- Kept across `super+shift+r`, because the grab and the thumbnails outlive a
-- reload: a fresh `false` here left an overview Escape could not leave.
-- `escape_leaves_the_overview_after_a_reload`.
--
-- `shown` is the minimized windows the overview is showing anyway, by id, with
-- the thumbnail each is drawn as (#30): what a click there is measured
-- against, since no pointer reaches a minimized window.
local kept = sol.keep("overview", { active = false, shown = {} })
kept.shown = kept.shown or {}

local overview = { active = kept.active }

-- Escape is the overview's only while it is up. Bound for good, it was taken
-- from every window, because a bound key never reaches a client (#174):
-- `escape_reaches_the_focused_window_while_the_overview_is_closed` and
-- `escape_leaves_the_overview_and_only_then_reaches_the_window`.
local function showing(active)
    overview.active = active
    kept.active = active
    if active then
        sol.bind("escape", overview.leave)
    else
        sol.unbind("escape")
    end
end

-- The windows on the desk in front of you, if there are desks, minimized ones
-- included: the overview shows those too, and choosing one shows it again.
--
-- Asked of `package.loaded` rather than `require`d, deliberately: requiring
-- `workspaces` here would register nine workspace bindings in a configuration
-- that had decided not to have workspaces, purely because it wanted overview.
-- Resolved at the press rather than at load, so the order `init.lua` requires
-- the two in does not matter.
local function on_this_desk(windows)
    local workspaces = package.loaded["workspaces"]
    if not workspaces then
        return windows
    end
    return workspaces.visible(windows, true)
end

local PADDING = 24
local ENTER = { duration = 260, easing = "outCubic" }
local LEAVE = { duration = 200, easing = "outCubic" }

function overview.enter()
    -- Idempotent: entering twice must not stack a second grab.
    if overview.active then
        return
    end

    -- The desk in front of you, and not every desk at once.
    --
    -- A workspace is a *selection* now: its windows are carried a screen away
    -- by the group they are in, and a window's own transform composes with its
    -- desk's rather than replacing it. So a grid slot handed to a window on
    -- workspace 3 is a slot on workspace 3's desk, which is two screens to the
    -- right -- it would be laid out perfectly and drawn where nobody can see
    -- it. Overview is about what is in front of you, which is also what
    -- `tiling` and `scrolling` have always taken it to mean.
    local windows = on_this_desk(sol.windows())
    if #windows == 0 then
        -- Nothing to show, so nothing is entered: a mode with no way out is
        -- worse than a key that did nothing.
        sol.log("overview: no windows")
        return
    end

    -- One grid per monitor, each on its own screen.
    --
    -- Not one grid across both: overview exists so you can see everything at
    -- once and point at the one you want, and a window that jumped to the
    -- other screen to be shown is a window you then have to find. Windows stay
    -- on the monitor they are on; they only get smaller.
    --
    -- The grid comes from `sol.layout`, the same arrangement the preview page
    -- draws, so what is tuned there is what happens here.
    sol.animate(ENTER)
    for _, each in ipairs(monitors.each(windows)) do
        if #each.windows > 0 then
            local area = each.monitor
            local slots = sol.layout.grid(each.windows, {
                x = area.x,
                y = area.y,
                w = area.w,
                h = area.h,
                padding = PADDING,
            })
            for index, window in ipairs(each.windows) do
                -- A minimized window is drawn only while a present on it
                -- plays, unless it is shown anyway, which is what a thumbnail
                -- of one is (#30).
                if window.minimized then
                    sol.show_minimized(window.id, true)
                    kept.shown[window.id] = slots[index]
                end
                sol.present(window.id, slots[index])
            end
        end
    end

    sol.grab_input(true)
    sol.status("overview")
    showing(true)
end

-- `chosen` is the window a click chose, which is being focused and so shown
-- again if it was minimized: it goes back to its own rectangle like the rest.
function overview.leave(chosen)
    if not overview.active then
        return
    end

    -- Every window, not only the ones entered with: a window opened while
    -- overview was up has a transform too, and leaving must clear all of them.
    --
    -- But a minimized window the overview was showing goes back where
    -- minimizing put it, and is drawn until it is there (#30): into its slot
    -- with `minimize.lua`'s genie, or at once without that file. One still
    -- minimized that the overview was not showing -- minimized while it was
    -- up -- is already there.
    sol.animate(LEAVE)
    local away = {}
    for _, window in ipairs(sol.windows()) do
        if window.minimized and window.id ~= chosen then
            if kept.shown[window.id] then
                away[#away + 1] = window.id
            end
        else
            sol.present_clear(window.id)
        end
    end
    local minimize = package.loaded["minimize"]
    for _, id in ipairs(away) do
        sol.show_minimized(id, false)
        if minimize then
            minimize.into(id)
        end
    end
    kept.shown = {}

    sol.grab_input(false)
    sol.status("")
    showing(false)
end

function overview.toggle()
    if overview.active then
        overview.leave()
    else
        overview.enter()
    end
end

-- Into the overview and out again; while it is up, Escape leaves it too.
-- Escape is bound only then (`showing`, above), so `solium --check`, which
-- lists what the configuration binds as it loads, does not show it.
sol.bind("super+space", overview.toggle)

-- Still up after a reload, so Escape is still the way out:
-- `escape_leaves_the_overview_after_a_reload`.
if overview.active then
    showing(true)
end

-- The minimized window whose thumbnail is at `x`, `y`, if one is: no pointer
-- reaches a minimized window, so `window_at` never names one, and the
-- overview asks its own thumbnails.
local function shown_at(x, y)
    for id, slot in pairs(kept.shown) do
        if x >= slot.x and x < slot.x + slot.w and y >= slot.y and y < slot.y + slot.h then
            return id
        end
    end
    return nil
end

-- Clicking a thumbnail focuses that window and leaves. `window_at` asks the
-- compositor, which hit-tests against where windows are *drawn* -- so this
-- works without the script knowing anything about the transform it set. A
-- minimized window chosen is shown again by the focus (#30).
-- `the_overview_shows_a_minimized_window_and_choosing_it_shows_it_again`.
sol.on("click", function(x, y)
    if not overview.active then
        return
    end
    local id = sol.window_at(x, y) or shown_at(x, y)
    if id then
        sol.focus(id)
    end
    overview.leave(id)
end)

return overview
