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
--
-- `leaving` is, for each window being closed, which strip it was in and the
-- windows either side of it there: what a refused close puts it back beside.
-- See the `closing` and `refused` handlers.
local scrolling = { active = false, views = {}, exiled = {}, leaving = {} }

-- Whether the strip closes up the moment a close is asked for, rather than once
-- the application has gone. Read when each event arrives, like `tiling.lua`'s.
-- See `reflow_on_close` in config.lua; anything but "when_gone" is the default.
local function reflows_at_once()
    return config.scrolling.reflow_on_close ~= "when_gone"
end

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
--
-- Only while this layout is in charge. `open` is heard whether or not it is,
-- and its handler settles, so a strip nobody was using focused every window
-- that opened: its new column. That was harmless while every window opened in
-- view, and it stopped being when tiling's `follow_overflow = false` began
-- parking a window on another workspace -- the strip took the keyboard there
-- (#134 review; `with_follow_overflow_off_the_window_opens_there_and_the_view_stays`
-- and `a_layout_not_in_charge_sends_nothing_to_another_workspace` in
-- `script.rs`, which load this file beside tiling as `init.lua` does). Every
-- other caller already asks `scrolling.active` first.
local function settle(animation)
    if not scrolling.active then
        return
    end
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
            -- A window being closed is already gone to a strip that closes up
            -- at once, and only `refused` puts it back: neither inserted nor
            -- present, so the sweep below takes it out of a strip that still
            -- has it. The same rule, for the same reason, as `tiling.adopt`.
            if window.leaving and reflows_at_once() then
                -- Nothing: see above.
            -- A dialog is deliberately absent from every strip, so `adopt` --
            -- whose whole job is to put back whatever is missing -- has to be
            -- told that this one is missing on purpose.
            elseif not dialogs.floats(window) then
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

-- A window being closed leaves the strip the moment the close is asked for, and
-- the strip closes the gap while the window fades where it stood -- the same
-- rule as `tiling.lua`'s, for the same reason (#128). Which columns move to do
-- it is exactly what `close` would have moved; `script.rs`'s
-- `closing_reflows_the_survivors_as_close_would_have` compares the two.
-- `reflow_on_close = "when_gone"` makes these two do nothing and leaves it all
-- to `close`.
--
-- `apply` and not `settle`, as `close` has always been: the keyboard leaves a
-- closing window when its animation lands, which is the compositor's to decide,
-- and a layout handing it to a neighbour now would take it off the window the
-- user is still looking at.
sol.on("closing", function(id)
    -- Only while this layout is in charge, for the reason `tiling.lua` gives:
    -- a strip nobody is using is not on screen, and one rearranged anyway
    -- comes back with its focus on the refused window -- which
    -- `scrolling.started` hands the keyboard to when the user switches here.
    if not scrolling.active or not reflows_at_once() then
        return
    end
    local kept
    for key, view in pairs(scrolling.views) do
        if view:contains(id) then
            local order = view:windows()
            for index, other in ipairs(order) do
                if other == id then
                    kept = { view = key, left = order[index - 1], right = order[index + 1] }
                end
            end
        end
        view:remove(id)
    end
    scrolling.leaving[id] = kept
    scrolling.apply(config.scrolling.snap)
end)

-- The strip a window belongs in now, its key and its monitor: its own
-- workspace's, on the monitor it is on. See `tiling.lua`'s `home_of`.
local function home_of(id)
    local monitor = monitors.of(id)
    local key = monitors.key(workspaces.at(id, monitor), monitor)
    if not scrolling.views[key] then
        scrolling.views[key] = sol.layout.scroller(config.scrolling)
    end
    return scrolling.views[key], key, monitor
end

-- The application declined, and the window is back: in the strip it left, in a
-- column after the window that came before it there -- the one on its left, or
-- above it in a column they shared -- or in front of the one after it when it
-- was the first. A column it shared comes back as a column of its own, and at
-- the width a new one opens at, because `remove` kept neither.
--
-- `apply` and never `settle`. `settle` hands the keyboard to whatever the strip
-- has focused, which after `insert` is this window -- and the usual reason for a
-- refusal is a "save your changes?" dialog that has the keyboard and must keep
-- it. For the same reason this is not `open`, whose handler settles.
--
-- Put back by the same rule as `tiling.lua`'s: taken out by `closing`, whatever
-- the setting says now, or missing from a strip that is in charge; beside its
-- old neighbours only if it still belongs in the strip it left.
sol.on("refused", function(id)
    local kept = scrolling.leaving[id]
    scrolling.leaving[id] = nil
    if not kept and not scrolling.active then
        return
    end
    local window = dialogs.by_id(id)
    if not window or dialogs.floats(window) then
        return
    end
    for _, view in pairs(scrolling.views) do
        if view:contains(id) then
            return
        end
    end
    local view, key, monitor = home_of(id)
    if kept and kept.view ~= key then
        kept = nil
    end
    local area = options(monitor)
    if kept and kept.left and view:contains(kept.left) then
        view:focus_window(kept.left, area)
        view:insert(id, area)
    elseif kept and kept.right and view:contains(kept.right) then
        -- `insert` opens a column to the right of the focused one, so a window
        -- that was first goes in after its old right-hand neighbour and then
        -- changes places with it.
        view:focus_window(kept.right, area)
        view:insert(id, area)
        view:move_column(-1, area)
    else
        view:insert(id, area)
    end
    scrolling.apply(config.scrolling.snap)
end)

-- The window is gone. Unchanged by `reflow_on_close`: this is the one event
-- every layout hears for every window that goes, including one that closed
-- itself and was never `closing`.
sol.on("close", function(id)
    for _, view in pairs(scrolling.views) do
        view:remove(id)
    end
    -- Ids are never reused, so a stale entry here would not put the wrong
    -- window back -- it would simply accumulate for the life of the session.
    scrolling.exiled[id] = nil
    scrolling.leaving[id] = nil
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
-- The second argument is a **screen x coordinate**, and has never once been the
-- delta this handler divides as though it were. It has been two different
-- coordinates and not one of them a delta: it was `event.location.x` -- the
-- pointer -- for the whole life of the event, which #120 corrected the
-- compositor's doc about without changing the value; and since #124 it is
-- where the *dragged edge* should come to rest. On a drag that moves no
-- horizontal edge at all -- a top or bottom border -- there is no such edge and
-- the pointer's own x comes through instead, exactly as before.
--
-- The arithmetic below is untouched by all three, and still wrong. `view:widen`
-- takes a delta, and there is no way to feed it an absolute position without
-- giving the scroller a set-the-width call; that is a change to this layout
-- rather than to the resize gesture, so it belongs to #122 and not to the
-- branch that happens to be passing through. Named honestly so the next reader
-- sees the defect instead of inheriting the belief: dividing a screen
-- coordinate by the monitor's width does not give a fraction of anything, and
-- an edge drag in the scrolling layout still jumps the column wide on the first
-- motion. Tracked as #122.
sol.on("resize", function(id, edge_x, _)
    -- `edge_x == 0` is doing duty as "no drag", and it does not mean that: it
    -- means the dragged edge belongs at screen x 0.
    --
    -- **That is a likelier accident since #124, not a less likely one.** While
    -- this was the pointer, hitting it needed the cursor on the exact column of
    -- pixels at x=0 -- rare, and it took a moving hand to get there. `edge_x` is
    -- a fixed edge now: the left edge of a column laid out at x=0 *is* zero, so
    -- a left-edge drag on the leftmost column of the leftmost monitor trips this
    -- deterministically, on the first frame and every frame after, and the
    -- handler does nothing for the whole gesture.
    --
    -- Left as it is all the same, because the fix is to give this handler
    -- arithmetic that matches its input and that is #122, not this branch.
    -- Written down rather than quietly tidied, so the next reader does not have
    -- to rediscover that the condition is a sentinel wearing a coordinate's
    -- clothes -- and does not inherit an assessment of how often it fires that
    -- was true of a value this no longer receives.
    if not scrolling.active or edge_x == 0 then
        return
    end
    local view, monitor = view_of(id)
    if view:contains(id) then
        -- A share of *this window's* monitor: the same drag means a different
        -- fraction on a 2560 than on a 1920 beside it.
        view:widen(id, edge_x / math.max(options(monitor).w, 1), options(monitor))
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
