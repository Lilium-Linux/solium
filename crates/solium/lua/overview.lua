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

-- A grid that stays close to square, so windows end up as large as they can be.
local function shape(count)
    local columns = math.max(1, math.ceil(math.sqrt(count)))
    return columns, math.ceil(count / columns)
end

-- The area one window gets, before its aspect ratio is taken into account.
local function cell(monitor, columns, rows, index)
    local width = monitor.w / columns
    local height = monitor.h / rows
    local column = index % columns
    local row = math.floor(index / columns)
    return {
        x = monitor.x + column * width + PADDING,
        y = monitor.y + row * height + PADDING,
        w = math.max(1, width - PADDING * 2),
        h = math.max(1, height - PADDING * 2),
    }
end

-- Fit a window into a cell, keeping its aspect ratio and never enlarging it: a
-- small window blown up to fill a cell reads as a different window.
local function fit(window, box)
    if window.w <= 0 or window.h <= 0 then
        return box
    end
    local scale = math.min(box.w / window.w, box.h / window.h, 1.0)
    local width, height = window.w * scale, window.h * scale
    return {
        x = box.x + (box.w - width) / 2,
        y = box.y + (box.h - height) / 2,
        w = width,
        h = height,
    }
end

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

    local monitor = sol.monitor()
    local columns, rows = shape(#windows)

    sol.animate(ENTER)
    for index, window in ipairs(windows) do
        sol.present(window.id, fit(window, cell(monitor, columns, rows, index - 1)))
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
