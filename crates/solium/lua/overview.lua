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
local workspaces = require("workspaces")

local overview = { active = false }

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
    local windows = workspaces.visible()
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
                sol.present(window.id, slots[index])
            end
        end
    end

    sol.grab_input(true)
    sol.status("overview")
    overview.active = true
end

function overview.leave()
    if not overview.active then
        return
    end

    -- Every window, not only the ones entered with: a window opened while
    -- overview was up has a transform too, and leaving must clear all of them.
    sol.animate(LEAVE)
    for _, window in ipairs(sol.windows()) do
        sol.present_clear(window.id)
    end

    sol.grab_input(false)
    sol.status("")
    overview.active = false
end

function overview.toggle()
    if overview.active then
        overview.leave()
    else
        overview.enter()
    end
end

sol.bind("super+space", overview.toggle)
sol.bind("escape", overview.leave)

-- Clicking a thumbnail focuses that window and leaves. `window_at` asks the
-- compositor, which hit-tests against where windows are *drawn* -- so this
-- works without the script knowing anything about the transform it set.
sol.on("click", function(x, y)
    if not overview.active then
        return
    end
    local id = sol.window_at(x, y)
    if id then
        sol.focus(id)
    end
    overview.leave()
end)

return overview
