-- Atrium: the desktop as a stage and a strip, part of floating (#449, #85).
--
-- The window you are using, and the windows grouped with it, stay where they
-- live: on the stage, live and whole. Every other group on the desk is a card
-- in a strip down the left of the work area, most recent at the top, drawn
-- from the real window in perspective -- still live, never a screenshot.
-- Choosing a card brings its group onto the stage, and the group that was
-- there springs into the strip. Apple calls this Stage Manager; an atrium is
-- the hall of a Roman house the rooms open off.
--
-- It is part of floating rather than a mode of its own: floating is "no layout
-- in charge" (`floating.lua` says why that absence matters), and atrium never
-- places anything. The stage windows are floating windows. The cards are
-- `sol.present`s -- where a window is *drawn*, not where it lives -- so turning
-- atrium off, or switching to tiling, is a `sol.present_clear` and every
-- window springs back to exactly where it was.
--
-- **Focus decides the stage.** A click on a card, Alt+Tab, a taskbar button,
-- the overview and an application asking to come forward all end in `focus`,
-- so one rule covers them: the group holding the focused window takes the
-- stage. Nothing here needs to know about any of them.
--
-- **Groups are per desk** (a workspace on a monitor), and start out as one
-- per application. A card shift-clicked, or dragged onto the stage, joins the
-- stage's group; a stage window dropped on the strip leaves it.
--
-- The strip's clicks are taken by its own scene, `qml/atrium/Strip.qml`,
-- declared over the strip alone: it draws each group's app icons under its
-- card and tells this file what was picked. Everywhere else the pointer goes
-- to the windows, so the stage stays live -- there is no `sol.grab_input`.

local config = require("config")
local modes = require("modes")
local monitors = require("monitors")

local atrium = {}

-- Across `super+shift+r`: the groups are not something the screen can tell a
-- fresh Lua state, and neither is which windows this file is drawing as cards.
--
--   on         `super+a`'s answer, or nil to follow `floating.atrium.enabled`
--   desks      groups of window ids by desk, most recent first; the first is
--              the stage, and in each group the most recent window is first
--   presented  windows this file has a present on, by `tostring(id)`, so it
--              only ever clears its own -- never a genie `minimize.lua` plays
--   surfaces   the strip scenes declared, by name
--   peek       monitors whose hidden strip the pointer has brought back
--   hover      the card under the pointer, by monitor
--   aside      another overlay (the overview, Alt+Tab, the desk stack) is up
local kept = sol.keep("atrium", {
    desks = {},
    presented = {},
    surfaces = {},
    peek = {},
    hover = {},
})
kept.desks = kept.desks or {}
kept.presented = kept.presented or {}
kept.surfaces = kept.surfaces or {}
kept.peek = kept.peek or {}
kept.hover = kept.hover or {}

atrium.desks = kept.desks

-- `config.floating.atrium`, each value checked and the shipped one in its
-- place when it is not something this file can draw with.
local function settings()
    local given = type(config.floating.atrium) == "table" and config.floating.atrium or {}
    local function number(value, fallback, low)
        if type(value) ~= "number" or value ~= value or value < (low or 0) then
            return fallback
        end
        return value
    end
    local motion = type(given.motion) == "table" and given.motion or {}
    local hover = type(given.hover) == "table" and given.hover or {}
    local card = type(given.card) == "table" and given.card or {}
    return {
        enabled = given.enabled ~= false,
        strip = number(given.strip, 220, 80),
        cards = math.floor(number(given.cards, 6, 1)),
        tilt = number(given.tilt, 14, -89),
        perspective = number(given.perspective, 1100, 100),
        fan = number(given.fan, 14),
        hide = given.hide == "never" and "never" or "overlap",
        aspect = number(card.aspect, 0.72, 0.1),
        spacing = number(card.spacing, 64),
        behind = math.min(1, number(card.behind, 0.92)),
        icon = math.floor(number(card.icon, 44, 8)),
        lift = number(hover.lift, 1.06, 0.1),
        hover_tilt = number(hover.tilt, 0.5),
        drag = number(given.drag, 8),
        motion = {
            stiffness = number(motion.stiffness, 300, 1),
            damping = number(motion.damping, 32, 1),
        },
    }
end

function atrium.enabled()
    if kept.on ~= nil then
        return kept.on
    end
    return settings().enabled
end

-- Whether atrium is arranging anything right now.
function atrium.running()
    return atrium.enabled() and modes.current() == "floating" and not kept.aside
end

-- ## Desks and groups

-- The desk a window is on: its workspace on its monitor, as `workspaces.lua`
-- names its selections. Asked of `package.loaded` for the reason
-- `overview.lua` gives: a configuration without workspaces has one desk per
-- monitor.
local function desk_of(window)
    local workspaces = package.loaded["workspaces"]
    local index = workspaces and workspaces.at(window.id, window.monitor) or 1
    return index .. "@" .. window.monitor
end

-- The desk a monitor is showing.
local function showing(monitor)
    local workspaces = package.loaded["workspaces"]
    local index = workspaces and workspaces.on(monitor) or 1
    return index .. "@" .. monitor
end

-- Windows that belong in a group. A dialog goes with the window it belongs
-- to, a window kept above the rest (a picture-in-picture) stays above them,
-- and a window closing or minimized is on its way out, and comes back as a
-- newcomer if it is refused or shown again.
local function grouped(window)
    return not window.leaving
        and not window.minimized
        and not window.modal
        and not window.parent
        and not window.above
end

local function find(id)
    for key, groups in pairs(kept.desks) do
        for index, group in ipairs(groups) do
            for at, each in ipairs(group) do
                if each == id then
                    return key, index, at
                end
            end
        end
    end
    return nil
end

-- The index of the first of `groups` holding a window of `app`.
local function group_of_app(groups, app, by_id)
    if not app or app == "" then
        return nil
    end
    for index, group in ipairs(groups) do
        for _, id in ipairs(group) do
            local window = by_id[id]
            if window and window.app_id == app then
                return index
            end
        end
    end
    return nil
end

-- Make the groups agree with the windows: a window gone, closing, minimized
-- or moved to another desk leaves its group, and a group left empty goes --
-- which is how the next group comes forward when the stage empties. A window
-- this file has not seen joins its application's group on its desk, or is a
-- group of its own: at the back when the desk is being set up for the first
-- time (so the strip comes out in stacking order), and on the stage otherwise.
local function reconcile(windows, by_id)
    local where = {}
    for _, window in ipairs(windows) do
        if grouped(window) then
            where[window.id] = desk_of(window)
        end
    end
    for key, groups in pairs(kept.desks) do
        for index = #groups, 1, -1 do
            local group = groups[index]
            for at = #group, 1, -1 do
                if where[group[at]] ~= key then
                    table.remove(group, at)
                end
            end
            if #group == 0 then
                table.remove(groups, index)
            end
        end
        if #groups == 0 then
            kept.desks[key] = nil
        end
    end
    local fresh = {}
    for _, window in ipairs(windows) do
        local key = where[window.id]
        if key and not find(window.id) then
            if fresh[key] == nil then
                fresh[key] = kept.desks[key] == nil
            end
            local groups = kept.desks[key] or {}
            kept.desks[key] = groups
            local app = group_of_app(groups, window.app_id, by_id)
            if app then
                local group = groups[app]
                group[#group + 1] = window.id
            elseif fresh[key] then
                groups[#groups + 1] = { window.id }
            else
                table.insert(groups, 1, { window.id })
            end
        end
    end
end

-- Group `index` of desk `key` onto the stage, and window `id` to the front of
-- it. True when anything moved.
local function to_stage(key, index, id)
    local groups = kept.desks[key]
    if not groups or not groups[index] then
        return false
    end
    local moved = false
    if index > 1 then
        table.insert(groups, 1, table.remove(groups, index))
        moved = true
    end
    local group = groups[1]
    for at, each in ipairs(group) do
        if each == id and at > 1 then
            table.insert(group, 1, table.remove(group, at))
            break
        end
    end
    return moved
end

-- ## Where things are drawn

-- The strip's column on a monitor's work area.
local function strip_of(area, set)
    return { x = area.x, y = area.y, w = set.strip, h = area.h }
end

-- What is left of the work area for the stage: where `floating.lua` opens a
-- new window while atrium is on, so it does not cover the strip at once.
function atrium.stage_area(area)
    if not area or not atrium.running() then
        return area
    end
    local width = settings().strip
    return { x = area.x + width, y = area.y, w = math.max(1, area.w - width), h = area.h }
end

local function overlaps(a, b)
    return a.x < b.x + b.w and b.x < a.x + a.w and a.y < b.y + b.h and b.y < a.y + a.h
end

-- The card boxes of a strip holding `count` cards: the same size each,
-- spaced evenly and centred down the work area, with room under each for its
-- icons.
local function slots(area, set, count)
    local pad = math.floor(set.strip * 0.1)
    local w = set.strip - 2 * pad
    local h = math.floor(w * set.aspect)
    local shown = math.min(count, set.cards)
    local pitch = math.min(h + set.spacing, math.floor((area.h - 48) / math.max(shown, 1)))
    local top = area.y + math.floor((area.h - shown * pitch) / 2)
    local out = {}
    for k = 1, shown do
        out[k] = { x = area.x + pad, y = top + (k - 1) * pitch + math.floor((pitch - h) / 2), w = w, h = h }
    end
    return out
end

-- Where each window of a group is drawn inside its card box. One scale for the
-- whole group, so a large window stays larger than a small one beside it, as
-- they are on the stage; the most recent in front, the rest fanned up and to
-- the left behind it, the set centred in the box.
local function fanned(group, by_id, box, fan)
    local wide, tall = 1, 1
    for _, id in ipairs(group) do
        local window = by_id[id]
        if window then
            wide = math.max(wide, window.w)
            tall = math.max(tall, window.h)
        end
    end
    local spread = fan * (#group - 1)
    local scale = math.min((box.w - spread) / wide, (box.h - spread) / tall, 1)
    local cx, cy = box.x + box.w / 2, box.y + box.h / 2
    local out = {}
    for at, id in ipairs(group) do
        local window = by_id[id]
        if window then
            local shift = spread / 2 - fan * (at - 1)
            local w, h = window.w * scale, window.h * scale
            out[at] = { id = id, x = cx + shift - w / 2, y = cy + shift - h / 2, w = w, h = h }
        end
    end
    return out
end

-- The windows one `arrange` has drawn or let go, so what it did not touch
-- can be let go of after it.
local touched = {}

local function present(id, options)
    touched[id] = true
    kept.presented[tostring(id)] = true
    sol.present(id, options)
end

local function release(id)
    touched[id] = true
    if kept.presented[tostring(id)] then
        kept.presented[tostring(id)] = nil
        sol.present_clear(id)
    end
end

-- Every card back where its window lives, and the strips taken down. The
-- groups are kept, for the next time atrium is on.
local function release_all()
    for key in pairs(kept.presented) do
        local id = tonumber(key)
        kept.presented[key] = nil
        if id then
            sol.present_clear(id)
        end
    end
    for name in pairs(kept.surfaces) do
        sol.surface(name, false)
        kept.surfaces[name] = nil
    end
    kept.peek = {}
    kept.hover = {}
end

-- The theme's name for an application's icon: its desktop entry's, when one
-- is installed under the window's app id, and the id itself otherwise, which
-- is a theme icon name for most applications.
local icons = nil
local function icon_of(app)
    if not app or app == "" then
        return ""
    end
    if icons == nil then
        icons = {}
        for _, entry in ipairs(sol.apps() or {}) do
            if entry.icon and entry.icon ~= "" then
                icons[entry.id:lower()] = entry.icon
                icons[(entry.id:match("([^.]+)$") or entry.id):lower()] = entry.icon
                if entry.name then
                    icons[entry.name:lower()] = icons[entry.name:lower()] or entry.icon
                end
            end
        end
    end
    local lower = app:lower()
    return icons[lower] or icons[(lower:match("([^.]+)$") or lower)] or app
end

-- One desk's strip: every group but the stage as a card, the groups past
-- `cards` waiting off the edge, all of them slid away while a stage window
-- covers the strip. Returns what the strip scene draws, for a desk in view.
local function arrange_desk(key, area, by_id, set, children)
    local groups = kept.desks[key] or {}
    local monitor = key:match("@(.*)$")
    local strip = strip_of(area, set)

    for _, id in ipairs(groups[1] or {}) do
        release(id)
        for _, child in ipairs(children[id] or {}) do
            release(child)
        end
    end

    local covered = false
    if set.hide == "overlap" then
        for _, id in ipairs(groups[1] or {}) do
            local window = by_id[id]
            if window and overlaps(window, strip) then
                covered = true
            end
        end
    end
    local in_view = showing(monitor) == key
    local peeking = covered and in_view and kept.peek[monitor] == true
    local hidden = covered and not peeking
    local hovered = in_view and kept.hover[monitor] or nil

    local boxes = slots(area, set, #groups - 1)
    local cards = {}
    for index = 2, #groups do
        local k = index - 1
        local box = boxes[k]
        local waiting = box == nil
        box = box or boxes[#boxes]
        local lift = (hovered == k and not hidden) and set.lift or 1
        local tilt = (hovered == k) and set.tilt * set.hover_tilt or set.tilt
        local drawn = fanned(groups[index], by_id, box, set.fan)
        for at, card in ipairs(drawn) do
            local w, h = card.w * lift, card.h * lift
            local x = card.x - (w - card.w) / 2
            local y = card.y - (h - card.h) / 2
            local opacity = at == 1 and 1 or set.behind
            if hidden or waiting then
                x = x - set.strip
                opacity = 0
            end
            -- Behind the stage, unless the strip has been brought back over it.
            local z = (peeking and 100 or 0) - k - at * 0.1
            local rect = { x = x, y = y, w = w, h = h }
            present(card.id, {
                x = x, y = y, w = w, h = h,
                rotate_y = tilt, pivot_x = 0, perspective = set.perspective,
                opacity = opacity, z = z,
            })
            -- A dialog goes into its window's card, out of sight.
            for _, child in ipairs(children[card.id] or {}) do
                present(child, { x = rect.x, y = rect.y, w = rect.w, h = rect.h, opacity = 0, z = z })
            end
        end
        if not waiting and in_view then
            local apps, seen = {}, {}
            for _, id in ipairs(groups[index]) do
                local window = by_id[id]
                local app = window and window.app_id or ""
                if not seen[app] then
                    seen[app] = true
                    apps[#apps + 1] = icon_of(app)
                end
            end
            -- The group's oldest window names it, whichever is in front.
            local name = math.huge
            for _, id in ipairs(groups[index]) do
                name = math.min(name, id)
            end
            cards[#cards + 1] = {
                card = k,
                key = name,
                x = box.x - area.x,
                y = box.y - area.y,
                w = box.w,
                h = box.h,
                icons = apps,
            }
        end
    end
    if not in_view then
        return nil
    end
    return { cards = cards, hidden = hidden, peeking = peeking, hovered = hovered or 0 }
end

-- The strip's scene on one monitor: up while it has cards to pick, and
-- gone otherwise, so an empty strip takes no clicks from the window under it.
local function declare_strip(monitor, area, set, drawn)
    local name = "atrium@" .. monitor
    if not drawn or #drawn.cards == 0 then
        if kept.surfaces[name] then
            sol.surface(name, false)
            kept.surfaces[name] = nil
        end
        return
    end
    kept.surfaces[name] = true
    sol.surface(name, {
        scene = "atrium/Strip.qml",
        layer = "top",
        on = { x = area.x, y = area.y, w = set.strip, h = area.h },
        interactive = true,
        properties = {
            cards = drawn.cards,
            hidden = drawn.hidden,
            peeking = drawn.peeking,
            hovered = drawn.hovered,
            iconSize = set.icon,
            dragThreshold = set.drag,
            lift = set.lift,
        },
    })
end

-- Lay out every desk: the stage released, the strip drawn, the scenes told.
function atrium.arrange()
    if not atrium.running() then
        return
    end
    local set = settings()
    local windows = sol.windows()
    local by_id = {}
    local children = {}
    for _, window in ipairs(windows) do
        by_id[window.id] = window
        if window.parent then
            local list = children[window.parent] or {}
            children[window.parent] = list
            list[#list + 1] = window.id
        end
    end
    reconcile(windows, by_id)
    sol.animate({ easing = { spring = set.motion } })
    touched = {}
    for _, monitor in ipairs(sol.monitors()) do
        local drawn = nil
        for key in pairs(kept.desks) do
            if key:match("@(.*)$") == monitor.name then
                local this = arrange_desk(key, monitor, by_id, set, children)
                drawn = drawn or this
            end
        end
        declare_strip(monitor.name, monitor, set, drawn)
    end
    -- A card whose window has left every group: closing or minimized, it is
    -- someone else's to draw now (the fade where it stood, the genie), so it
    -- is only forgotten; anything else -- kept above, gone fullscreen -- is
    -- let go of, back to where it lives.
    for key in pairs(kept.presented) do
        local id = tonumber(key)
        if id and not touched[id] then
            kept.presented[key] = nil
            local window = by_id[id]
            if window and not window.leaving and not window.minimized then
                sol.present_clear(id)
            end
        end
    end
end

-- Atrium on or off within floating: `super+a` (`floating.atrium.key`).
function atrium.toggle()
    kept.on = not atrium.enabled()
    if kept.on then
        atrium.arrange()
    else
        sol.animate({ easing = { spring = settings().motion } })
        release_all()
    end
end

-- Group `index` of the desk `monitor` is showing onto the stage, or with
-- `join`, into the stage's group.
local function pick(monitor, card, join)
    local key = showing(monitor)
    local groups = kept.desks[key]
    local index = (tonumber(card) or 0) + 1
    if not groups or index < 2 or not groups[index] then
        return
    end
    local chosen = groups[index]
    if join then
        table.remove(groups, index)
        for _, id in ipairs(chosen) do
            table.insert(groups[1], 1, id)
        end
    else
        to_stage(key, index, chosen[1])
    end
    kept.hover[monitor] = nil
    kept.peek[monitor] = nil
    atrium.arrange()
    sol.focus(chosen[1])
end

-- A card being dragged out of the strip: its group follows the pointer, flat
-- and in front of everything, at the size it has in its card. `x` and `y` are
-- the strip scene's, from the column's top-left corner.
local function drag(monitor, card, x, y)
    local key = showing(monitor)
    local groups = kept.desks[key]
    local index = (tonumber(card) or 0) + 1
    local area = monitors.named(monitor)
    if not groups or index < 2 or not groups[index] or not area then
        return
    end
    local set = settings()
    local by_id = {}
    for _, window in ipairs(sol.windows()) do
        by_id[window.id] = window
    end
    local boxes = slots(area, set, #groups - 1)
    local box = boxes[index - 1] or boxes[#boxes]
    local drawn = fanned(groups[index], by_id, box, set.fan)
    local dx = area.x + (tonumber(x) or 0) - (box.x + box.w / 2)
    local dy = area.y + (tonumber(y) or 0) - (box.y + box.h / 2)
    sol.animate({ easing = { spring = { stiffness = 900, damping = 60 } } })
    for at, each in ipairs(drawn) do
        present(each.id, {
            x = each.x + dx, y = each.y + dy, w = each.w, h = each.h,
            rotate_y = 0, pivot_x = 0, perspective = set.perspective,
            opacity = at == 1 and 1 or set.behind, z = 200 - at * 0.1,
        })
    end
end

-- ## What atrium is told

sol.on("open", function(id)
    if not atrium.running() then
        return
    end
    local by_id, window = {}, nil
    for _, each in ipairs(sol.windows()) do
        by_id[each.id] = each
        if each.id == id then
            window = each
        end
    end
    if window and grouped(window) and not find(id) then
        local key = desk_of(window)
        local groups = kept.desks[key] or {}
        kept.desks[key] = groups
        if groups[1] and group_of_app({ groups[1] }, window.app_id, by_id) then
            table.insert(groups[1], 1, id)
        else
            table.insert(groups, 1, { id })
        end
    end
    atrium.arrange()
end)

sol.on("focus", function(id)
    if not atrium.running() or not id then
        return
    end
    local key, index = find(id)
    if not key then
        -- Shown again, or refused a close: a newcomer, which the groups take
        -- in before it is brought forward.
        atrium.arrange()
        key, index = find(id)
    end
    if key and to_stage(key, index, id) then
        atrium.arrange()
    end
end)

for _, event in ipairs({ "closing", "refused", "close", "minimize", "unminimize", "layout", "monitors" }) do
    sol.on(event, function()
        atrium.arrange()
    end)
end

-- A stage window let go over the strip leaves its group, as a card of its own
-- just under the stage, and is moved out of the strip's way for when it is
-- brought back. Any other drop may have moved a window over the strip, or off
-- it, which the arrangement checks.
sol.on("drop", function(id, x, y)
    if not atrium.running() then
        return
    end
    local key, index = find(id)
    local set = settings()
    local area = monitors.named(key and key:match("@(.*)$") or "")
    local strip = area and strip_of(area, set)
    if key and index == 1 and strip and #kept.desks[key][1] > 1
        and x >= strip.x and x < strip.x + strip.w and y >= strip.y and y < strip.y + strip.h
    then
        local groups = kept.desks[key]
        for at, each in ipairs(groups[1]) do
            if each == id then
                table.remove(groups[1], at)
                break
            end
        end
        table.insert(groups, 2, { id })
        for _, window in ipairs(sol.windows()) do
            if window.id == id then
                sol.place(id, {
                    x = strip.x + strip.w + 24, y = window.y, w = window.w, h = window.h, tile = false,
                })
            end
        end
    end
    atrium.arrange()
end)

sol.on("surface", function(surface, action, data)
    local monitor = type(surface) == "string" and surface:match("^atrium@(.*)$")
    if not monitor or not atrium.running() then
        return
    end
    data = type(data) == "table" and data or {}
    if action == "pick" then
        pick(monitor, data.card, data.join == true)
    elseif action == "drag" then
        drag(monitor, data.card, data.x, data.y)
    elseif action == "drop" then
        atrium.arrange()
    elseif action == "hover" then
        local card = tonumber(data.card) or 0
        kept.hover[monitor] = card > 0 and card or nil
        atrium.arrange()
    elseif action == "edge" then
        kept.peek[monitor] = true
        atrium.arrange()
    elseif action == "leave" then
        kept.peek[monitor] = nil
        kept.hover[monitor] = nil
        atrium.arrange()
    end
end)

-- A window sent to another workspace changes desk without any event of the
-- window's own; `workspaces.moved` is the one place that says so.
local workspaces = package.loaded["workspaces"]
if workspaces and workspaces.moved then
    workspaces.moved(function()
        atrium.arrange()
    end)
end

-- Floating in charge again, or not: the cards are a floating desktop's.
modes.watch(function(name)
    if name == "floating" then
        atrium.arrange()
    else
        release_all()
    end
end)

-- Another overlay up -- the overview, Alt+Tab, the desk stack -- presents the
-- windows itself, and clears every one of them as it leaves. Each says so by
-- naming itself as the mode in charge with `sol.status`, and saying nothing
-- again as it leaves, so that is what atrium listens to: it stands aside, its
-- strip down, while one is up, and lays the cards out again once it has gone.
-- Wrapped here rather than asked of each overlay, so one written after this
-- file, or in a configuration, is stood aside for too.
local status = sol.status
sol.status = function(text)
    status(text)
    local up = type(text) == "string" and text ~= "" and modes.registered[text] == nil
    if up and not kept.aside then
        kept.aside = true
        for name in pairs(kept.surfaces) do
            sol.surface(name, false)
            kept.surfaces[name] = nil
        end
    elseif not up and kept.aside then
        kept.aside = nil
        -- The overlay cleared every window it presented, cards included.
        kept.presented = {}
        -- And the window it chose was focused while atrium stood aside, so
        -- its group is brought onto the stage now.
        atrium.arrange()
        for _, window in ipairs(sol.windows()) do
            if window.focused then
                local key, index = find(window.id)
                if key and to_stage(key, index, window.id) then
                    atrium.arrange()
                end
            end
        end
    end
end

-- Put back after a reload: the groups and the presents survived it, and the
-- strip scenes are declared again from them.
sol.on("restore", function()
    atrium.arrange()
end)

return atrium
