-- Overview: every window scaled onto a grid.
--
-- This file is the architecture's proof. Overview is not a compositor feature;
-- it is a script that sets a target rect per window and lets the compositor's
-- one animation clock get them there. The app switcher is this with a row
-- instead of a grid, peek is this with one window at the cursor, and the
-- icon-to-window genie is this with an icon rect as the starting point.
--
-- If any of those ever needs new Rust, the transform layer is missing
-- something and *that* is the bug to fix -- not this file.

local overview = { active = false }

local PADDING = 24
local ENTER = { duration = 260, easing = "outCubic" }
local LEAVE = { duration = 200, easing = "outCubic" }

function overview.enter()
    -- Idempotent: entering twice must not stack a second grab.
    if overview.active then
        return
    end

    local windows = sol.windows()
    if #windows == 0 then
        -- Nothing to show, so nothing is entered: a mode with no way out is
        -- worse than a key that did nothing.
        sol.log("overview: no windows")
        return
    end

    -- The grid comes from `sol.layout`, the same arrangement the preview page
    -- draws, so what is tuned there is what happens here.
    local area = sol.monitor()
    area.padding = PADDING
    local slots = sol.layout.grid(windows, area)

    sol.animate(ENTER)
    for index, window in ipairs(windows) do
        sol.present(window.id, slots[index])
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
