-- Scrolling: columns and a moving view, as niri does it.
--
-- Reimplemented from niri's src/layout/scrolling.rs (GPL-3.0-or-later, which
-- this project's GPL-3.0-only can use); the behaviour is the specification.
--
-- The unit is a column, not a window. A column holds a stack sharing its
-- width, and that width is a share of the *view* — so opening a tenth window
-- never makes the first nine thinner. The strip simply gets longer and the
-- view moves. That is the difference between a scroller and a row of windows
-- squeezed to fit, and it is why this works the same on a laptop and an
-- ultrawide.
--
-- The view is measured from the active column rather than from the left end
-- of the strip, so focus and view cannot drift apart.

local config = require("config")
local workspaces = require("workspaces")
local modes = require("modes")
local monitors = require("monitors")

local scrolling = { active = false, views = {} }

-- One strip per workspace per monitor.
--
-- Per workspace because scrolling on one must not move another. Per monitor
-- because a column's width is a share of *the view*, and the view is one
-- screen -- a shared strip on a 2560 and a 1920 beside it would have columns
-- that are the right width on neither.
local function view_for(index, monitor)
    local key = monitors.key(index, monitor or (monitors.active() or {}).name)
    if not scrolling.views[key] then
        scrolling.views[key] = sol.layout.scroller()
    end
    return scrolling.views[key]
end

local function options(monitor)
    local area = monitor and monitors.named(monitor) or sol.monitor()
    -- A copy: the monitor table belongs to the snapshot and the layout adds
    -- keys to what it is handed.
    local out = { x = area.x, y = area.y, w = area.w, h = area.h }
    out.gap = config.gap
    return out
end

-- The strip a window is in, and the monitor it belongs to.
local function view_of(id)
    local monitor = monitors.of(id)
    return view_for(workspaces.active, monitor), monitor
end

function scrolling.apply(animation)
    if not scrolling.active then
        return
    end
    sol.animate(animation or config.scrolling.motion)
    -- One `sol.animate` for every screen: two strips moving at once is one
    -- movement. See docs/animation.md.
    for _, each in ipairs(monitors.each(workspaces.visible())) do
        local view = view_for(workspaces.active, each.monitor.name)
        for _, slot in ipairs(view:layout(options(each.monitor.name))) do
            sol.place(slot.id, slot)
        end
    end
end

-- Follow the strip's own idea of focus, so the keyboard goes where the view
-- went.
--
-- The *active* monitor's strip: with two screens there are two focused
-- columns, and the keyboard belongs to the one you are looking at.
local function settle(animation)
    scrolling.apply(animation)
    local active = monitors.active()
    local focused = view_for(workspaces.active, active and active.name):focused()
    if focused then
        sol.focus(focused)
    end
end

function scrolling.adopt()
    -- Also how a window that moved between monitors settles: missing from its
    -- new screen's strip, still in its old one's, and both fixed here.
    local present = {}
    for _, each in ipairs(monitors.each(workspaces.visible())) do
        local view = view_for(workspaces.active, each.monitor.name)
        for _, window in ipairs(each.windows) do
            present[window.id] = each.monitor.name
            if not view:contains(window.id) then
                view:insert(window.id, options(each.monitor.name))
            end
        end
    end
    for key, view in pairs(scrolling.views) do
        for _, window in ipairs(sol.windows()) do
            local belongs = present[window.id]
            if view:contains(window.id)
                and (not belongs or monitors.key(workspaces.active, belongs) ~= key)
            then
                view:remove(window.id)
            end
        end
    end
end

function scrolling.started()
    scrolling.adopt()
    settle()
end

function scrolling.toggle()
    modes.use("scrolling")
end

modes.register("scrolling", scrolling)

-- A new window opens in its own column beside the active one, and the view
-- follows it.
-- The compositor changed how much room windows get -- a decoration that
-- reserves a different amount, most likely. The slots are unchanged; what
-- fits inside them is not, so the arithmetic is redone.
sol.on("layout", function()
    scrolling.apply()
end)

sol.on("open", function(id)
    local view, monitor = view_of(id)
    view:insert(id, options(monitor))
    settle(config.scrolling.snap)
end)

sol.on("close", function(id)
    for _, view in pairs(scrolling.views) do
        view:remove(id)
    end
    scrolling.apply(config.scrolling.snap)
end)

-- Dropping a window on another column moves it there; dropped anywhere else
-- it slides back. Without this a drag in a scrolling layout could not move a
-- window at all, only pick it up and put it down again.
sol.on("drop", function(id, x, y)
    if not scrolling.active then
        return
    end
    -- Where it *landed*: a window dragged across the boundary belongs to the
    -- other screen's strip, so it leaves every strip and joins that one.
    local landed = monitors.of(id)
    local view = view_for(workspaces.active, landed)
    local target = sol.window_at(x, y, id)
    if not view:contains(id) then
        for _, each in pairs(scrolling.views) do
            each:remove(id)
        end
        view:insert(id, options(landed))
    end
    if target and view:contains(target) and view:contains(id) then
        view:move_to_column_of(id, target, options(landed))
    end
    settle(config.scrolling.snap)
end)

-- Clicking or hovering a column that is only half on screen brings it fully
-- into view. Without this a column can hold focus while hanging off the edge,
-- which is the state that makes a scroller feel like it is fighting you.
sol.on("focus", function(id)
    if not scrolling.active then
        return
    end
    local view, monitor = view_of(id)
    if view:contains(id) then
        view:focus_window(id, options(monitor))
        scrolling.apply(config.scrolling.snap)
    end
end)

-- An edge drag changes the column's width rather than one window's size:
-- every window in a column shares its width, so there is nothing else it
-- could mean.
sol.on("resize", function(id, dx, _)
    if not scrolling.active or dx == 0 then
        return
    end
    local view, monitor = view_of(id)
    if view:contains(id) then
        -- A share of *this window's* monitor: the same drag means a different
        -- fraction on a 2560 than on a 1920 beside it.
        view:widen(id, dx / math.max(options(monitor).w, 1), options(monitor))
        scrolling.apply({ duration = 0 })
    end
end)

-- Super plus the wheel moves the view. Unmodified, the wheel still belongs to
-- whatever is under the cursor.
sol.on("scroll", function(_, dy)
    if not scrolling.active or dy == 0 then
        return
    end
    -- The strip under the pointer: the wheel scrolls the screen you are
    -- pointing at, which is the only thing it could reasonably mean.
    local active = monitors.active()
    local name = active and active.name
    view_for(workspaces.active, name):focus_sideways(dy > 0 and 1 or -1, options(name))
    settle(config.scrolling.snap)
end)

local function bind(combo, action)
    sol.bind(combo, function()
        if not scrolling.active then
            return
        end
        local active = monitors.active()
        action(view_for(workspaces.active, active and active.name), active and active.name)
        settle(config.scrolling.snap)
    end)
end

sol.bind("super+s", scrolling.toggle)

-- Move between columns, and within one.
bind("super+bracketleft", function(view, on) view:focus_sideways(-1, options(on)) end)
bind("super+bracketright", function(view, on) view:focus_sideways(1, options(on)) end)
bind("super+ctrl+bracketleft", function(view, on) view:move_column(-1, options(on)) end)
bind("super+ctrl+bracketright", function(view, on) view:move_column(1, options(on)) end)
bind("super+shift+bracketleft", function(view) view:focus_vertically(-1) end)
bind("super+shift+bracketright", function(view) view:focus_vertically(1) end)

-- Stack a window into this column, or push it back out into its own.
bind("super+comma", function(view) view:consume() end)
bind("super+period", function(view, on) view:expel(options(on)) end)

-- Cycle the column through the preset widths.
bind("super+r", function(view, on) view:cycle_width(options(on)) end)

return scrolling
