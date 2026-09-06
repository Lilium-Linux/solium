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

local scrolling = { active = false, views = {} }

-- One strip per workspace: scrolling on one must not move another.
local function view_for(index)
    if not scrolling.views[index] then
        scrolling.views[index] = sol.layout.scroller()
    end
    return scrolling.views[index]
end

local function options()
    local area = sol.monitor()
    area.gap = config.gap
    return area
end

function scrolling.apply(animation)
    if not scrolling.active then
        return
    end
    local slots = view_for(workspaces.active):layout(options())
    if #slots == 0 then
        return
    end
    sol.animate(animation or config.scrolling.motion)
    for _, slot in ipairs(slots) do
        sol.place(slot.id, slot)
    end
end

-- Follow the strip's own idea of focus, so the keyboard goes where the view
-- went.
local function settle(animation)
    scrolling.apply(animation)
    local focused = view_for(workspaces.active):focused()
    if focused then
        sol.focus(focused)
    end
end

function scrolling.adopt()
    local view = view_for(workspaces.active)
    local present = {}
    for _, window in ipairs(workspaces.visible()) do
        present[window.id] = true
        if not view:contains(window.id) then
            view:insert(window.id, options())
        end
    end
    for _, window in ipairs(sol.windows()) do
        if not present[window.id] and view:contains(window.id) then
            view:remove(window.id)
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
    view_for(workspaces.active):insert(id, options())
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
    local view = view_for(workspaces.active)
    local target = sol.window_at(x, y, id)
    if target and view:contains(target) and view:contains(id) then
        view:move_to_column_of(id, target, options())
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
    local view = view_for(workspaces.active)
    if view:contains(id) then
        view:focus_window(id, options())
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
    local view = view_for(workspaces.active)
    if view:contains(id) then
        view:widen(id, dx / math.max(sol.monitor().w, 1), options())
        scrolling.apply({ duration = 0 })
    end
end)

-- Super plus the wheel moves the view. Unmodified, the wheel still belongs to
-- whatever is under the cursor.
sol.on("scroll", function(_, dy)
    if not scrolling.active or dy == 0 then
        return
    end
    view_for(workspaces.active):focus_sideways(dy > 0 and 1 or -1, options())
    settle(config.scrolling.snap)
end)

local function bind(combo, action)
    sol.bind(combo, function()
        if not scrolling.active then
            return
        end
        action(view_for(workspaces.active))
        settle(config.scrolling.snap)
    end)
end

sol.bind("super+s", scrolling.toggle)

-- Move between columns, and within one.
bind("super+bracketleft", function(view) view:focus_sideways(-1, options()) end)
bind("super+bracketright", function(view) view:focus_sideways(1, options()) end)
bind("super+ctrl+bracketleft", function(view) view:move_column(-1, options()) end)
bind("super+ctrl+bracketright", function(view) view:move_column(1, options()) end)
bind("super+shift+bracketleft", function(view) view:focus_vertically(-1) end)
bind("super+shift+bracketright", function(view) view:focus_vertically(1) end)

-- Stack a window into this column, or push it back out into its own.
bind("super+comma", function(view) view:consume() end)
bind("super+period", function(view) view:expel(options()) end)

-- Cycle the column through the preset widths.
bind("super+r", function(view) view:cycle_width(options()) end)

return scrolling
