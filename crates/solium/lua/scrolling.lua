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
local dialogs = require("dialogs")

-- **`views` does not survive a reload, and that is named here rather than
-- fixed.**
--
-- #116's commit message predicted that the next piece of script state to matter
-- would be a third one, after `workspaces` and `modes`. This is it, and it is
-- the one `sol.keep` cannot take: a strip is userdata owned by
-- `crates/layout`, `sol.keep` holds plain data by construction, and nothing on
-- `restore` rebuilds one from anything. So `super+shift+r` in a scrolling
-- session comes back with `modes.current` still saying "scrolling" -- correctly;
-- that half is kept -- and every strip built fresh underneath it.
--
-- What is lost is more than the widths. `scrolling.started` runs `adopt`, so
-- every visible window is inserted again in `sol.windows()` order and
-- membership does return. The *arrangement* does not: the order columns were
-- moved into, which windows were stacked together with `super+comma`, each
-- column's width, which column is active, and where the view sits. Every window
-- is there and the strip is not the one you built.
--
-- Deliberately out of scope for #116. Keeping it needs a strip that can be
-- written out as plain data and read back -- a save/restore pair in
-- `crates/layout` and a `sol.layout.scroller` that takes what it hands back --
-- which is a change to the layout crate and the host rather than a line of this
-- file. Until then the honest summary of a reload is that it keeps which layout
-- is in charge and not how that layout had arranged itself.

-- `exiled` is the ids this layout has taken out of its strips because they are
-- modal dialogs, and it is what makes `unset_modal` reversible: only a window
-- that was taken out is ever put back. Same table, same reason, same name as
-- `tiling.lua` -- see `dialogs.settle`.
local scrolling = { active = false, views = {}, exiled = {} }

-- One strip per workspace per monitor.
--
-- Per workspace because scrolling on one must not move another. Per monitor
-- because a column's width is a share of *the view*, and the view is one
-- screen -- a shared strip on a 2560 and a 1920 beside it would have columns
-- that are the right width on neither.
-- Takes only the monitor: which workspace that screen is showing belongs to
-- `workspaces`, and threading the index through every call site is how one of
-- them ends up asking for the wrong screen's workspace.
local function view_for(monitor)
    monitor = monitor or (monitors.active() or {}).name
    local key = monitors.key(workspaces.on(monitor), monitor)
    if not scrolling.views[key] then
        -- The whole section, not a repackaging of it: `sol.layout.scroller`
        -- reads `widths` and `default_width` and ignores the rest. Handed over
        -- here, once per strip, rather than through `options()` -- which is
        -- rebuilt on every keystroke, and the widths cannot change between two
        -- of them.
        --
        -- This call took no argument until #117, which is the whole reason
        -- those two settings were documented and read by nothing: the strip
        -- used the constant list in `crates/layout/src/scroller.rs` instead.
        scrolling.views[key] = sol.layout.scroller(config.scrolling)
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
    return view_for(monitor), monitor
end

-- A modal dialog is in no strip, and one that stops being modal rejoins.
--
-- The same three lines as `tiling.lua`'s, over a different container, which is
-- exactly the shape `dialogs.settle` exists to keep honest: what differs is
-- how a window joins and leaves a strip, and nothing else.
local function settle_dialogs(windows)
    dialogs.settle(
        scrolling.exiled,
        windows,
        function(id)
            -- Every strip, not just this window's monitor's: a window dragged
            -- to the other screen and then made modal is still in the strip it
            -- left, and one window in two strips gets two slots.
            for _, view in pairs(scrolling.views) do
                view:remove(id)
            end
        end,
        function(id)
            local monitor = monitors.of(id)
            local view = view_for(monitor)
            if not view:contains(id) then
                view:insert(id, options(monitor))
            end
        end
    )
end

function scrolling.apply(animation)
    if not scrolling.active then
        return
    end
    local visible = workspaces.visible()
    settle_dialogs(visible)

    sol.animate(animation or config.scrolling.motion)
    -- One `sol.animate` for every screen: two strips moving at once is one
    -- movement. See docs/animation.md.
    local screens = monitors.each(visible)
    -- What the strips decided, by window id, for the same reason tiling
    -- collects it: a dialog is centred on its parent, and the snapshot says
    -- where that parent was before this pass moved it.
    local placed = {}
    for _, each in ipairs(screens) do
        local view = view_for(each.monitor.name)
        for _, slot in ipairs(view:layout(options(each.monitor.name))) do
            sol.place(slot.id, slot)
            placed[slot.id] = slot
        end
    end
    -- A second pass, because a dialog on one screen may belong to a window on
    -- another. Dialogs are in no strip, so this is the only thing that places
    -- them.
    --
    -- `options` itself rather than one screen's: a dialog goes onto its
    -- *parent's* monitor, and handing it this screen's area is what pinned a
    -- cross-screen dialog to the wrong edge. See `dialogs.place`.
    dialogs.place(visible, options, placed)
end

-- Follow the strip's own idea of focus, so the keyboard goes where the view
-- went.
--
-- The *active* monitor's strip: with two screens there are two focused
-- columns, and the keyboard belongs to the one you are looking at.
local function settle(animation)
    scrolling.apply(animation)
    local active = monitors.active()
    local focused = view_for(active and active.name):focused()
    if focused then
        sol.focus(focused)
    end
end

function scrolling.adopt()
    -- Also how a window that moved between monitors settles: missing from its
    -- new screen's strip, still in its old one's, and both fixed here.
    local present = {}
    for _, each in ipairs(monitors.each(workspaces.visible())) do
        local view = view_for(each.monitor.name)
        for _, window in ipairs(each.windows) do
            -- A dialog is deliberately absent from every strip, so `adopt` --
            -- whose whole job is to put back whatever is missing -- has to be
            -- told that this one is missing on purpose.
            if not dialogs.floats(window) then
                present[window.id] = each.monitor.name
                if not view:contains(window.id) then
                    view:insert(window.id, options(each.monitor.name))
                end
            end
        end
    end
    for key, view in pairs(scrolling.views) do
        for _, window in ipairs(sol.windows()) do
            local belongs = present[window.id]
            if view:contains(window.id)
                and (not belongs or monitors.key(workspaces.on(belongs), belongs) ~= key)
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
-- A monitor arrived or went away.
--
-- `apply` walks the monitors that exist, so a window on one that has gone is
-- in a view nothing iterates: it keeps a slot that is now off every
-- screen, and it comes back on that screen when the monitor does. `adopt` is
-- already the function that re-homes a window whose monitor changed -- it was
-- only ever called when the mode was switched on.
sol.on("monitors", function()
    scrolling.adopt()
end)

sol.on("layout", function()
    scrolling.apply()
end)

sol.on("open", function(id)
    -- A dialog joins no strip. `apply` places it over its parent and records
    -- it as exiled, so there is nothing to insert.
    --
    -- `apply` rather than `settle`, and that is the point of the branch:
    -- `settle` hands the keyboard to whatever the strip thinks is focused, and
    -- the strip has never heard of this dialog -- so a save prompt would open,
    -- be focused by the compositor, and have the focus taken straight back off
    -- it by the layout. A dialog that cannot be typed into is worse than one
    -- in the wrong place.
    if dialogs.floating(id) then
        scrolling.apply(config.scrolling.snap)
        return
    end
    local view, monitor = view_of(id)
    view:insert(id, options(monitor))
    settle(config.scrolling.snap)
end)

sol.on("close", function(id)
    for _, view in pairs(scrolling.views) do
        view:remove(id)
    end
    -- Ids are never reused, so a stale entry here would not put the wrong
    -- window back -- it would simply accumulate for the life of the session.
    scrolling.exiled[id] = nil
    dialogs.forget(id)
    scrolling.apply(config.scrolling.snap)
end)

-- Dropping a window on another column moves it there; dropped anywhere else
-- it slides back. Without this a drag in a scrolling layout could not move a
-- window at all, only pick it up and put it down again.
sol.on("drop", function(id, x, y)
    if not scrolling.active then
        return
    end
    -- A dialog stays where it was dropped, and joins no strip: it is in none,
    -- and the insert below is the one way it could end up in one. `dropped`
    -- records the drag so the next pass does not undo it -- as an offset from
    -- the parent, so the dialog still follows the window it belongs to.
    if dialogs.dropped(id) then
        scrolling.apply(config.scrolling.snap)
        return
    end
    -- Where it *landed*: a window dragged across the boundary belongs to the
    -- other screen's strip, so it leaves every strip and joins that one.
    local landed = monitors.of(id)
    local view = view_for(landed)
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
--
-- The second argument is where the pointer **is**, not how far it moved. It
-- was named `dx` here and the compositor's own doc for the event called it
-- "the delta" -- both wrong since the event was written, because
-- `ResizeGrab::motion` records `event.location`. #120 corrected the doc; this
-- handler is left doing the arithmetic it always did, because `view:widen`
-- takes a delta and there is no way to feed it an absolute position without
-- giving the scroller a set-the-width call, which is a change to a layout
-- this issue is not about and cannot verify. Named honestly so the next
-- reader sees the defect instead of inheriting the belief: dividing a screen
-- coordinate by the monitor's width does not give a fraction of anything, and
-- an edge drag in the scrolling layout jumps the column wide on the first
-- motion. Tracked separately.
sol.on("resize", function(id, x, _)
    if not scrolling.active or x == 0 then
        return
    end
    local view, monitor = view_of(id)
    if view:contains(id) then
        -- A share of *this window's* monitor: the same drag means a different
        -- fraction on a 2560 than on a 1920 beside it.
        view:widen(id, x / math.max(options(monitor).w, 1), options(monitor))
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
    view_for(name):focus_sideways(dy > 0 and 1 or -1, options(name))
    settle(config.scrolling.snap)
end)

local function bind(combo, action)
    sol.bind(combo, function()
        if not scrolling.active then
            return
        end
        local active = monitors.active()
        action(view_for(active and active.name), active and active.name)
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
