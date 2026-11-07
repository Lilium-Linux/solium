-- Alt+Tab as Android's recents: every window as a card in one row across the
-- screen, most recently used first, the chosen one big in the middle.
--
--   alt+tab / alt+shift+tab   open on the previous window, then step
--   tab, arrows, the wheel    step, once Alt is let go
--   (the pointer is held by the shell's picker layer meanwhile: Shell.qml)
--   enter, space, a click     switch to the chosen window
--   escape, a click outside   close, leaving everything as it was
--
-- Picking is a key or a click, not letting go of Alt: a script is told about
-- key presses, not releases.
--
-- The cards are the live windows themselves, presented where the row puts
-- them (as overview.lua does with a grid), every workspace's included, and
-- minimised ones too (taskbar.lua puts those back in their button after).
--
-- One screen at a time: the windows of the monitor it opens on, and the
-- other monitors left as they are, so nothing moves on a screen you are not
-- looking at.
--
-- A window drawn bare -- one that draws its own titlebar, like Claude or a
-- browser -- has no frame to carry the icon badge, so the shell draws it over
-- its card instead (shell/RecentsFrames.qml); its border is looks.lua's
-- effect, which moves with the card as a frame would.

local recents = { active = false }

-- Window ids, the one focused last first. Across reloads.
local kept = sol.keep("recents", { order = {} })

local SLIDE = require("motion").travel
local LEAVE = require("motion").reflow
local CARD_HEIGHT = 0.62 -- of the screen
local CARD_WIDTH = 0.6
local GAP = 40
local SIDE_SCALE = 0.86
local SIDE_OPACITY = 0.6

local cards = {}
local index = 1

-- The monitor the row is on, by name.
local function active_monitor()
    local area = sol.monitor()
    for _, monitor in ipairs(sol.monitors()) do
        local m = monitor.whole or monitor
        if area.x >= m.x and area.y >= m.y and area.x < m.x + m.w and area.y < m.y + m.h then
            return monitor.name
        end
    end
    return nil
end

-- Hand the shell the cards it frames itself.
local function push_bare(list)
    local config = require("config")
    config.shell = type(config.shell) == "table" and config.shell or {}
    config.shell.properties = type(config.shell.properties) == "table" and config.shell.properties or {}
    config.shell.properties.recentsBare = { monitor = recents.monitor or "", cards = list }
    require("shell").apply()
end

local function forget(id)
    for i = #kept.order, 1, -1 do
        if kept.order[i] == id then
            table.remove(kept.order, i)
        end
    end
end

local function touch(id)
    forget(id)
    table.insert(kept.order, 1, id)
end

-- Every window, most recently used first; any never focused at the end.
local function windows()
    local by_id, all = {}, {}
    for _, w in ipairs(sol.windows()) do
        if not w.leaving and w.w > 0 and w.h > 0
            and (not recents.monitor or w.monitor == recents.monitor) then
            by_id[w.id] = w
            all[#all + 1] = w
        end
    end
    local out, seen = {}, {}
    for _, id in ipairs(kept.order) do
        if by_id[id] and not seen[id] then
            out[#out + 1] = by_id[id]
            seen[id] = true
        end
    end
    for _, w in ipairs(all) do
        if not seen[w.id] then
            out[#out + 1] = w
        end
    end
    return out
end

-- Each card wears its app's icon on its corner, drawn by the frames
-- themselves (qml/accent/RecentsBadge.qml, a layer of both pane styles), so it
-- moves with its window. A layer knows its window's title and not its app,
-- so every pane is handed the icons by title, and which title is chosen.
local function icon_for(app_id)
    if not app_id or app_id == "" then
        return ""
    end
    local want = app_id:lower()
    local tail = want:match("([^.]+)$")
    local apps = sol.apps() or {}
    for pass = 1, 2 do
        for _, app in ipairs(apps) do
            local id = (app.id or ""):lower():gsub("%.desktop$", "")
            local name = (app.name or ""):lower()
            local match = pass == 1 and id == want
                or pass == 2 and (id:match("([^.]+)$") == tail or name == want or name == tail)
            if match and app.icon and app.icon ~= "" then
                return app.icon
            end
        end
    end
    return app_id
end

local icons = {}

local function badges(active)
    local chosen = active and cards[index] and cards[index].title or ""
    sol.pane_values({ recents = { active = active, chosen = chosen, icons = icons } })
end

-- The row: the chosen card centred, its neighbours smaller and dimmer either
-- side of it, running off the screen's edges like a carousel.
local function layout()
    local area = sol.monitor()
    local function size(w)
        local height = area.h * CARD_HEIGHT
        local width = height * w.w / w.h
        if width > area.w * CARD_WIDTH then
            width = area.w * CARD_WIDTH
            height = width * w.h / w.w
        end
        return width, height
    end

    local centre_y = area.y + area.h / 2
    local x = {}
    local chosen_w = size(cards[index])
    x[index] = area.x + area.w / 2 - chosen_w / 2
    for i = index + 1, #cards do
        local prev_w = size(cards[i - 1])
        x[i] = x[i - 1] + prev_w + GAP
    end
    for i = index - 1, 1, -1 do
        local this_w = size(cards[i])
        x[i] = x[i + 1] - GAP - this_w
    end

    local bare = {}
    for i, w in ipairs(cards) do
        local width, height = size(w)
        local scale = i == index and 1 or SIDE_SCALE
        local sw, sh = width * scale, height * scale
        local rect = {
            x = x[i] + (width - sw) / 2,
            y = centre_y - sh / 2,
            w = sw,
            h = sh,
            opacity = i == index and 1 or SIDE_OPACITY,
        }
        sol.present(w.id, rect)
        if w.style == "none" then
            bare[#bare + 1] = {
                id = w.id, x = rect.x, y = rect.y, w = rect.w, h = rect.h,
                opacity = rect.opacity, chosen = i == index,
                icon = icons[w.title] or icon_for(w.app_id),
                -- The badge is drawn at the window's scale on a frame; the
                -- same size on the card here.
                scale = rect.w / w.w,
            }
        end
    end
    push_bare(bare)
    badges(true)
end

local KEYS = {
    tab = function() recents.step(1) end,
    right = function() recents.step(1) end,
    ["shift+tab"] = function() recents.step(-1) end,
    left = function() recents.step(-1) end,
    ["return"] = function() recents.pick() end,
    kp_enter = function() recents.pick() end,
    space = function() recents.pick() end,
    escape = function() recents.leave() end,
}

local function keys(on)
    for combo, run in pairs(KEYS) do
        if on then
            sol.bind(combo, run)
        else
            sol.unbind(combo)
        end
    end
end

function recents.enter(step)
    if recents.active then
        return
    end
    local overview = require("overview")
    if overview.active then
        return
    end
    recents.monitor = active_monitor()
    cards = windows()
    if #cards == 0 then
        recents.monitor = nil
        return
    end
    -- Straight to the window before this one, as Alt+Tab does.
    index = 1
    if #cards > 1 then
        index = (step or 1) > 0 and 2 or #cards
    end

    recents.active = true
    icons = {}
    for _, w in ipairs(cards) do
        if w.title and w.title ~= "" then
            icons[w.title] = icon_for(w.app_id)
        end
    end
    sol.animate(SLIDE)
    overview.gather(recents.monitor)
    layout()
    sol.grab_input(true)
    sol.status("recents")
    keys(true)
end

function recents.step(by)
    if not recents.active or #cards < 2 then
        return
    end
    index = (index - 1 + by) % #cards + 1
    sol.animate(SLIDE)
    layout()
end

function recents.leave()
    if not recents.active then
        return
    end
    recents.active = false
    keys(false)

    local taskbar = package.loaded["taskbar"]
    sol.animate(LEAVE)
    for _, w in ipairs(cards) do
        if taskbar and taskbar.is_minimized(w.id) then
            taskbar.park(w.id)
        else
            sol.present_clear(w.id)
        end
    end
    require("overview").scatter(LEAVE)
    badges(false)
    -- The shell's badges go home with their windows, fading on the way, as a
    -- frame's do.
    local home = {}
    for _, w in ipairs(cards) do
        if w.style == "none" then
            home[#home + 1] = {
                id = w.id, x = w.x, y = w.y, w = w.w, h = w.h, opacity = 1,
                chosen = false, icon = icons[w.title] or icon_for(w.app_id),
                scale = 1, leaving = true,
            }
        end
    end
    push_bare(home)
    recents.monitor = nil

    sol.grab_input(false)
    sol.status("")
end

-- Close the row and switch to the chosen window (or the one given).
function recents.pick(id)
    if not recents.active then
        return
    end
    local chosen = id or (cards[index] and cards[index].id)
    recents.leave()
    if not chosen then
        return
    end
    local taskbar = package.loaded["taskbar"]
    if taskbar and taskbar.is_minimized(chosen) then
        taskbar.restore(chosen)
        return
    end
    local workspaces = package.loaded["workspaces"]
    for _, w in ipairs(sol.windows()) do
        if w.id == chosen and workspaces then
            workspaces.go(workspaces.at(chosen, w.monitor), w.monitor)
        end
    end
    sol.focus(chosen)
end

sol.bind("alt+tab", function()
    if recents.active then
        recents.step(1)
    else
        recents.enter(1)
    end
end)

sol.bind("alt+shift+tab", function()
    if recents.active then
        recents.step(-1)
    else
        recents.enter(-1)
    end
end)


-- A click the shell's picker layer took, at scene coordinates on `monitor`,
-- as a point in the global space `sol.window_at` reads.
local function global(data)
    for _, monitor in ipairs(sol.monitors()) do
        if monitor.name == data.monitor then
            local whole = monitor.whole or monitor
            return whole.x + (tonumber(data.x) or 0), whole.y + (tonumber(data.y) or 0)
        end
    end
    return tonumber(data.x) or 0, tonumber(data.y) or 0
end

local function clicked(x, y)
    local id = sol.window_at(x, y)
    for _, w in ipairs(cards) do
        if w.id == id then
            recents.pick(id)
            return
        end
    end
    recents.leave()
end

-- Clicks come from the shell's picker layer (Shell.qml), which holds the
-- pointer while this is open; `click` is the grab's own, for a session with
-- no such shell.
sol.on("click", function(x, y)
    if recents.active then
        clicked(x, y)
    end
end)

sol.on("surface", function(_, action, data)
    if not recents.active then
        return
    end
    data = type(data) == "table" and data or {}
    if action == "picker.click" then
        clicked(global(data))
    elseif action == "picker.wheel" then
        recents.step(tonumber(data.by) or 1)
    end
end)

sol.on("focus", function(id)
    if not recents.active then
        touch(id)
    end
end)

sol.on("close", forget)

return recents
