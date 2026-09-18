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

local dialogs = {}

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

-- Where a modal dialog goes.
--
-- Centred on its parent, at the size it already has. Its size is kept rather
-- than chosen: a dialog is one of the few windows that genuinely knows how big
-- it wants to be — it was laid out around a sentence of text and two buttons —
-- and a layout that resizes it is a layout that wraps "Discard changes?" onto
-- four lines.
--
-- With no parent to centre on it goes to the middle of its own monitor, which
-- is where a window manager has put a homeless dialog for as long as there
-- have been dialogs. **Not left where it was**: a Wayland toplevel is mapped
-- at (0, 0) until something places it, so "leave it alone" means the top-left
-- corner, under the panel, which is exactly the "lost off-screen" outcome this
-- has to avoid.
--
-- `area` is the monitor's work area, and the result is clamped into it. That
-- matters even when there is a parent: a parent hanging half off the right
-- edge would otherwise take its dialog off with it, and a dialog larger than
-- the screen would be centred with both edges outside it. Clamping after
-- centring rather than instead of it keeps the common case exact.
function dialogs.rect(window, area, windows, placed)
    local w = math.min(window.w, area.w)
    local h = math.min(window.h, area.h)
    local over = dialogs.parent_of(window, windows, placed) or area
    local x = over.x + (over.w - w) / 2
    local y = over.y + (over.h - h) / 2
    return {
        x = math.max(area.x, math.min(x, area.x + area.w - w)),
        y = math.max(area.y, math.min(y, area.y + area.h - h)),
        w = w,
        h = h,
    }
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
            admit(window.id)
        end
    end
end

-- Place every modal dialog on this monitor, over the window waiting on it.
--
-- Called by a layout after it has placed *every* monitor's arrangement, not
-- after each one: a dialog's parent may be on the other screen, and a parent
-- whose new slot has not been computed yet would be centred on where it used
-- to be. A modal is in no arrangement, so this is the only thing that places
-- it at all.
--
-- `each` is one entry from `monitors.each`: the monitor and the windows on it.
-- `windows` is the whole visible list rather than that monitor's, for the same
-- cross-screen reason, and `placed` is every slot the arrangements just
-- decided.
function dialogs.place(each, windows, area, placed)
    for _, window in ipairs(each.windows) do
        if dialogs.floats(window) then
            sol.place(window.id, dialogs.rect(window, area, windows, placed))
        end
    end
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
