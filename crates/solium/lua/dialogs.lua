-- Modal dialogs, for the layouts.
--
-- A save prompt or a preferences window is not part of an arrangement. It has
-- no life of its own: it belongs to the window that opened it, it is waiting
-- for an answer, and the moment it gets one it is gone. Tiled, it takes a
-- share of the screen away from the document it is asking about, pushes that
-- document somewhere else to make room, and then hands the space back and
-- pushes it a second time when it closes. Issue #72.
--
-- So a dialog is *lifted out* of the arrangement and floated over its parent.
--
-- ## Why this is a file and not two
--
-- `tiling.lua` and `scrolling.lua` have to agree about this, and the way to
-- make two scripts agree is to give them one answer rather than two that look
-- alike. A dialog that is in tiling's tree and out of scrolling's strip is a
-- dialog that jumps into the arrangement when you press `super+s`, which is
-- the original bug wearing a different hat. What genuinely differs between the
-- two layouts is only *how a window joins and leaves* — a tree splits, a strip
-- opens a column — so that part is passed in and everything else lives here.
--
-- ## What a modal is
--
-- `window.modal` comes from the compositor and means "the client said this is
-- modal". On Wayland that is `xdg_dialog_v1.set_modal` verbatim; on X11 there
-- is no readable equivalent and the window *type* stands in for it. The
-- argument for the substitution is in `xwayland.rs`; nothing here needs to
-- know which protocol a window came from, which is the point of asking the
-- snapshot rather than the client.

local monitors = require("monitors")

-- `moved` is where the user dragged a dialog to, by id, as an offset from its
-- parent's top-left corner. See `dialogs.dropped`.
local dialogs = { moved = {} }

-- Whether a layout should leave this window out of its arrangement.
--
-- One function rather than `window.modal` written out at seven call sites,
-- because this is the predicate that is most likely to want a second clause
-- later — and a policy spelled out at seven call sites is a policy that gets
-- changed at six.
function dialogs.floats(window)
    return window.modal == true
end

-- The window a modal is waiting on, as a rect, or nil.
--
-- Nil covers all three ways there can be nothing to centre on, and they are
-- deliberately not distinguished here: the client named no parent
-- (`window.parent` is absent), it named one the compositor cannot point at
-- (`window.parent` is `false` — not mapped, not ours, or gone), or it named
-- one that is not on screen right now. A layout does the same thing in all
-- three, and the distinction is kept at the boundary for whoever needs it, not
-- thrown away — see `Parentage` in `script.rs`.
--
-- **`placed` wins over the snapshot, and this is not an optimisation.** The
-- snapshot says where every window *was* when the handler was called; a layout
-- pass is in the business of moving them, so by the time a dialog is placed
-- its parent's snapshot rect can already be a rect nothing is at. Centring on
-- it puts the dialog over where the document used to be — and since the
-- arrangement is now settled, nothing will run again to correct it. `placed`
-- is what the arrangement decided in this same pass, so it is the newer of the
-- two answers wherever it exists.
function dialogs.parent_of(window, windows, placed)
    if not window.parent then
        return nil
    end
    if placed and placed[window.parent] then
        return placed[window.parent]
    end
    for _, other in ipairs(windows or sol.windows()) do
        if other.id == window.parent then
            return other
        end
    end
    return nil
end

-- Where a modal dialog wants to be, before any screen has a say.
--
-- Centred on `over` — its parent's rect — at the size it already has. Its size
-- is kept rather than chosen: a dialog is one of the few windows that genuinely
-- knows how big it wants to be — it was laid out around a sentence of text and
-- two buttons — and a layout that resizes it is a layout that wraps "Discard
-- changes?" onto four lines.
--
-- With nothing to centre on, `over` is the fallback the caller chose (the
-- middle of a monitor), which is where a window manager has put a homeless
-- dialog for as long as there have been dialogs. **Not left where it was**: a
-- Wayland toplevel is mapped at (0, 0) until something places it, so "leave it
-- alone" means the top-left corner, under the panel, which is exactly the "lost
-- off-screen" outcome this has to avoid.
--
-- `offset` is the one case that is not centred: a dialog the user dragged
-- somewhere. It is an offset from `over` rather than a position, so the dialog
-- keeps the relationship the drag established — move the document and the
-- prompt the user pushed off it goes on being off it. See `dialogs.dropped`.
--
-- Unclamped, and that is the whole reason this is not `dialogs.rect`: which
-- screen a dialog belongs to is decided *from* this rect, so it has to exist
-- before any screen's edges are applied to it.
function dialogs.wanted(window, over, offset)
    local w = window.w
    local h = window.h
    if offset then
        return { x = over.x + offset.x, y = over.y + offset.y, w = w, h = h }
    end
    return {
        x = over.x + (over.w - w) / 2,
        y = over.y + (over.h - h) / 2,
        w = w,
        h = h,
    }
end

-- A rect brought inside a work area.
--
-- Clamping matters even when there is a parent to centre on: a parent hanging
-- half off the right edge would otherwise take its dialog off with it, and a
-- dialog larger than the screen would be centred with both edges outside it.
-- Clamping after centring rather than instead of it keeps the common case
-- exact.
function dialogs.within(rect, area)
    local w = math.min(rect.w, area.w)
    local h = math.min(rect.h, area.h)
    return {
        x = math.max(area.x, math.min(rect.x, area.x + area.w - w)),
        y = math.max(area.y, math.min(rect.y, area.y + area.h - h)),
        w = w,
        h = h,
    }
end

-- Where a modal dialog goes: centred on its parent, kept on the screen.
--
-- **`area` must be the work area of the monitor the dialog is going onto, which
-- is its parent's and not its own.** The two are routinely different and the
-- difference is not transient. Every new toplevel is mapped at (0, 0)
-- (`new_toplevel`) and the compositor decides a window's monitor from the
-- centre of its rect (`Solium::output_of`), so a dialog whose parent is on DP-2
-- arrives belonging to whichever screen covers the origin. Clamp it into *that*
-- screen and it is pinned to that screen's edge; its centre stays there, so it
-- is reported on the same wrong monitor next pass and clamped identically for
-- ever. `dialogs.place` picks the area from the rect the dialog is centred on,
-- which is the only input that knows where the parent actually is.
function dialogs.rect(window, area, over, offset)
    return dialogs.within(dialogs.wanted(window, over or area, offset), area)
end

-- Take the modal dialogs out of an arrangement, and put back the one that
-- stopped being modal.
--
-- `held` is the layout's own table of ids it has given up, keyed by id. It is
-- the whole reason this is not simply "re-admit every window the arrangement
-- is missing": that is what `adopt` does, and doing it on every relayout would
-- take a window that has not had its `open` event yet and insert it with no
-- split target — so it would land where the arrangement put it rather than
-- where the pointer was, which is the one property tiling exists to have. Only
-- windows this function took out are ever put back.
--
-- `remove` and `admit` are the layout's own. `remove` must tolerate an id the
-- arrangement does not hold, because a window that opens already modal is
-- marked here without ever having been in one — and that window still has to
-- be marked, or `unset_modal` would have nothing to put back.
function dialogs.settle(held, windows, remove, admit)
    for _, window in ipairs(windows) do
        if dialogs.floats(window) then
            remove(window.id)
            held[window.id] = true
        elseif held[window.id] then
            held[window.id] = nil
            -- A window that stopped being modal is going back into the
            -- arrangement, which decides where it goes; a drag from when it
            -- was floating would otherwise be waiting for it if it ever went
            -- modal again.
            dialogs.forget(window.id)
            admit(window.id)
        end
    end
end

-- Place one modal dialog, and whatever it is waiting on first.
--
-- The recursion is the case a file chooser makes ordinary: "Replace?" is modal
-- and transient for the chooser, which is itself modal and transient for the
-- document. Nothing arranges either of them, so unless the chooser is placed
-- before the prompt that is centred on it, the prompt is centred on a rect the
-- chooser is not at — and on the first pass, before anything has placed the
-- chooser, that rect is the (0, 0) every toplevel is mapped at, which puts the
-- prompt in the top-left corner.
--
-- `busy` is what makes that safe rather than recursive for ever: a window is
-- marked before its parent is looked at, so a client that names a cycle of
-- parents — which neither protocol forbids — places each of them once.
local function place_one(window, windows, area_for, placed, busy)
    if placed[window.id] or busy[window.id] then
        return
    end
    busy[window.id] = true

    if window.parent and not placed[window.parent] then
        for _, other in ipairs(windows) do
            if other.id == window.parent and dialogs.floats(other) then
                place_one(other, windows, area_for, placed, busy)
            end
        end
    end

    local over = dialogs.parent_of(window, windows, placed)
    -- Where it wants to be, and only then which screen that is. Asking in the
    -- other order is the cross-monitor bug: the dialog's *own* monitor is
    -- wherever it happens to have been mapped, and clamping into that one pins
    -- it to the edge of a screen its parent is not on. See `dialogs.rect`.
    local wanted = dialogs.wanted(window, over or monitors.named(window.monitor),
        dialogs.moved[window.id])
    local screen = monitors.covering(wanted) or monitors.covering(over)
    local area = area_for(screen and screen.name or window.monitor)

    local rect = dialogs.within(wanted, area)
    sol.place(window.id, rect)
    -- **Recorded, like any other placement.** `placed` is what this pass
    -- decided, and a dialog is a window something else may be centred on; left
    -- out of it, the prompt above falls through to the snapshot and is centred
    -- on where the chooser was rather than where it now is.
    placed[window.id] = rect
end

-- Place every modal dialog, over the window waiting on it.
--
-- Called by a layout after it has placed *every* monitor's arrangement, not
-- after each one: a dialog's parent may be on the other screen, and a parent
-- whose new slot has not been computed yet would be centred on where it used to
-- be. A modal is in no arrangement, so this is the only thing that places it at
-- all.
--
-- One pass over the whole visible list rather than one pass per monitor, and
-- that is the cross-screen fix rather than tidying: a dialog is placed onto its
-- *parent's* monitor, so which screen's turn it is has nothing to say about
-- where it goes. `area_for` is the layout's own `options`, asked for whichever
-- monitor that turns out to be; `placed` is every slot the arrangements just
-- decided, and this adds the dialogs to it.
function dialogs.place(windows, area_for, placed)
    local busy = {}
    for _, window in ipairs(windows) do
        if dialogs.floats(window) then
            place_one(window, windows, area_for, placed, busy)
        end
    end
end

-- Remember a dialog where the user dropped it.
--
-- A modal used to snap straight back over its parent's centre, which is a
-- floating window you cannot drag off the sentence it is covering — the exact
-- thing people move a dialog for. So a drag sticks.
--
-- Kept as an offset from the parent rather than as a position, because the
-- alternative decays: a dialog pinned to absolute coordinates stays behind when
-- the document it belongs to is moved, resized or sent to the other screen, and
-- the user has to move it again. An offset survives all three, and a modal that
-- stays where it was put *relative to its window* is still visibly attached to
-- it.
--
-- Forgotten when the dialog closes or stops being modal, and nowhere else: an
-- id is never reused, so nothing here can be applied to a different window.
function dialogs.dropped(id, windows, placed)
    windows = windows or sol.windows()
    local window = dialogs.by_id(id, windows)
    if not window or not dialogs.floats(window) then
        return false
    end
    local over = dialogs.parent_of(window, windows, placed)
        or monitors.named(window.monitor)
    dialogs.moved[id] = { x = window.x - over.x, y = window.y - over.y }
    return true
end

-- Forget a drag: a dialog that closed, or stopped being one.
function dialogs.forget(id)
    dialogs.moved[id] = nil
end

-- The window with this id in a snapshot list, or nil.
--
-- Here rather than in `monitors` because the callers are the `open` handlers,
-- which are handed an id and have to ask whether it is a dialog before they
-- insert it into anything.
function dialogs.by_id(id, windows)
    for _, window in ipairs(windows or sol.windows()) do
        if window.id == id then
            return window
        end
    end
    return nil
end

-- Whether the window with this id is one a layout should float.
function dialogs.floating(id, windows)
    local window = dialogs.by_id(id, windows)
    return window ~= nil and dialogs.floats(window)
end

return dialogs
