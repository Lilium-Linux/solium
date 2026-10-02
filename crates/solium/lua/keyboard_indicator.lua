-- The keyboard pill: a small capsule near where you type, saying that Caps
-- Lock is on, or which layout you just switched to.
--
-- Configuration, not a compositor feature. The compositor publishes data --
-- `sol.on("keyboard")` when the layout or a lock changes, `sol.text_input()`
-- and `sol.on("text_input")` for the focused text field and its caret, the
-- `Keyboard` singleton and every pane's `caret` for QML -- and this file is
-- the policy that turns it into a pill. `keyboard.indicator` in `config.lua`
-- is read here and nowhere else. Delete the `require` in `init.lua` and there
-- is no pill; copy this file and change it, and the pill is yours.
--
-- It draws the pill one of two ways, both of them plain QML on that data:
--
--   "pane"     inside the window, at the caret: every shipped pane style has
--              a `KeyboardPillLayer`, which reads the pane's `caret` and the
--              `values` this file hands every pane with `sol.pane_values`.
--              A window drawn with no frame -- fullscreen, or one drawing its
--              own decorations -- has no pane style around it to draw one,
--              so it gets the surface below at its caret instead.
--   "surface"  on an overlay `sol.surface` of its own, at the caret in the
--              global space (`qml/indicator/keyboard.qml`).
--
-- Either way, a window that says nothing about a caret gets the pill on its
-- screen instead, when `fallback = "surface"`.
--
-- Played, key by key with `us,ru` and Russian live, by the scenarios in
-- `crates/solium/tests/scenarios/`: `keyboard-surface.lua` (placed at the
-- caret, and on screen with none), `keyboard-pane.lua` (what the panes are
-- handed, Caps again on focus), `keyboard-pane-bare.lua` (a window with no
-- frame), `keyboard-off.lua` (`show = false`) and
-- `keyboard-indicator-false.lua` (`indicator = false`), through
-- `scenario::tests::every_scenario_with_a_client_passes`.

local config = require("config")

local indicator = {}

-- The overlay surface, and the scene it draws.
local SURFACE = "keyboard-indicator"
local SCENE = "indicator/keyboard.qml"

-- The surface's size: the capsule, with room for its shadow on every side.
-- Big enough for a short layout name; `KeyboardPill` centres itself in it.
local BOX_W, BOX_H = 112, 48
-- The capsule's height inside it, and the gap between the caret and it.
local PILL_H, GAP = 24, 6

-- Which pill was shown last, counted across reloads, so a cue a scene has
-- seen is never mistaken for a new one.
local kept = sol.keep("keyboard_indicator", { serial = 0 })

-- `keyboard.indicator`, or the defaults when a configuration from before it
-- existed leaves it out. `indicator = false` is `show = false`
-- (`keyboard-indicator-false.lua`).
local function settings()
    local keyboard = type(config.keyboard) == "table" and config.keyboard or {}
    local found = {}
    if type(keyboard.indicator) == "table" then
        found = keyboard.indicator
    elseif keyboard.indicator == false then
        found = { show = false }
    end
    local on = type(found.on) == "table" and found.on or {}
    return {
        show = found.show == nil and "pane" or found.show,
        layout = on.layout ~= false,
        caps = on.caps ~= false,
        num = on.num == true,
        caps_on_focus = found.caps_on_focus ~= false,
        fallback = found.fallback == nil and "surface" or found.fallback,
        position = found.position or "bottom",
        duration = tonumber(found.duration) or 1200,
    }
end

-- The window with the keyboard, if any.
local function focused()
    for _, window in ipairs(sol.windows()) do
        if window.focused then
            return window.id
        end
    end
    return nil
end

-- Keep `rect` on the monitor `area` is, so the scene is never cut in half.
local function inside(rect, area)
    rect.x = math.max(area.x, math.min(area.x + area.w - rect.w, rect.x))
    rect.y = math.max(area.y, math.min(area.y + area.h - rect.h, rect.y))
    return rect
end

-- The surface at the caret: the capsule just below it, centred on it, or
-- above it when the monitor has no room below.
local function at_caret(field, area)
    local margin = (BOX_H - PILL_H) / 2
    local x = field.x + field.w / 2 - BOX_W / 2
    local y = field.y + field.h + GAP - margin
    if y + BOX_H - margin > area.y + area.h then
        y = field.y - GAP - PILL_H - margin
    end
    return inside({ x = math.floor(x + 0.5), y = math.floor(y + 0.5), w = BOX_W, h = BOX_H }, area)
end

-- The surface on screen, with no caret to go to: on the focused window's
-- monitor, centred across it, at `position`.
local function on_screen(position, area)
    local x = area.x + (area.w - BOX_W) / 2
    local y
    if position == "top" then
        y = area.y + area.h * 0.08
    elseif position == "center" then
        y = area.y + (area.h - BOX_H) / 2
    else
        y = area.y + area.h * 0.92 - BOX_H
    end
    return inside({ x = math.floor(x + 0.5), y = math.floor(y + 0.5), w = BOX_W, h = BOX_H }, area)
end

-- Where the overlay surface was last put, so hiding it leaves it there.
local placed = nil

-- What the last cue was for, "" once one hid the pill. A hide with nothing
-- showing sends nothing, so a focus change costs the panes nothing
-- (`keyboard-pane.lua`, "nothing showing").
local showing = ""

-- Declare the overlay surface showing `cue`, at `rect` or where it already
-- is. The same scene each time, so the live one takes the new place and the
-- new cue in place, and is built again only on a monitor it newly reaches.
local function surface(rect, cue)
    placed = rect or placed
    if not placed then
        return
    end
    sol.surface(SURFACE, {
        scene = SCENE,
        layer = "overlay",
        on = placed,
        interactive = false,
        properties = { cue = cue },
    })
end

-- What the configuration hands every pane: whether pane layers draw the pill
-- at all, and the cue to draw.
local function panes(want, cue)
    sol.pane_values({ keyboard_indicator = { show = want, cue = cue } })
end

-- Show `what` ("caps", "layout", "num") where it belongs, or hide the pill
-- everywhere with "". A timed pill hands back to `after`, held, when it goes.
function indicator.show(what, after)
    local s = settings()
    if s.show ~= "pane" and s.show ~= "surface" then
        return
    end
    if what == "" and showing == "" then
        return
    end
    showing = what
    kept.serial = kept.serial + 1
    local cue = {
        what = what,
        serial = kept.serial,
        -- A lock's pill stays while the lock is on; a layout's goes.
        hold = what == "caps" or what == "num",
        duration = s.duration,
        after = after,
    }
    local field = sol.text_input()
    local caret = field and field.x and field or nil
    local area = sol.monitor(field and field.window or focused())

    -- In the pane only where a frame is drawn around it: a window drawn bare
    -- has no pane style to draw it, so the surface goes to its caret instead
    -- (`keyboard-pane-bare.lua`).
    local in_pane = s.show == "pane" and caret ~= nil and field.framed == true
    panes(s.show == "pane", in_pane and cue or { what = "", serial = kept.serial })

    local on_surface = what ~= ""
        and ((caret ~= nil and not in_pane) or (caret == nil and s.fallback == "surface"))
    if on_surface then
        surface(caret and at_caret(caret, area) or on_screen(s.position, area), cue)
    else
        -- Hidden where it is, so the scene stays built for the next one.
        surface(nil, { what = "", serial = kept.serial })
    end
end

-- Apply `keyboard.indicator`: build what it needs and drop what it does not.
function indicator.apply()
    local s = settings()
    showing = ""
    if s.show ~= "pane" and s.show ~= "surface" then
        if s.show then
            sol.log("keyboard.indicator.show is \"pane\", \"surface\" or false, not " .. tostring(s.show))
        end
        sol.surface(SURFACE, false)
        placed = nil
        panes(false, { what = "" })
        return
    end
    panes(s.show == "pane", { what = "" })
    -- Built now, hidden, so the first pill appears at once rather than after
    -- its scene has loaded. Whatever `show` and `fallback` say: with "pane",
    -- a window drawn bare still gets the surface at its caret.
    local area = sol.monitor(focused())
    surface(on_screen(s.position, area), { what = "", serial = kept.serial })
end

sol.on("keyboard", function(state, changed)
    local s = settings()
    if changed == "caps" and s.caps then
        indicator.show(state.caps and "caps" or "")
    elseif changed == "num" and s.num then
        indicator.show(state.num and "num" or "")
    elseif changed == "layout" and s.layout then
        -- With a lock on, its pill comes back once the layout's has gone,
        -- rather than leaving Caps Lock on with nothing shown
        -- (`keyboard-surface.lua`, `keyboard-pane-drawn.lua`).
        local after = (s.caps and state.caps and "caps") or (s.num and state.num and "num") or nil
        indicator.show("layout", after)
    end
end)

-- A field enabled or focused: Caps Lock's pill again if it is on, as macOS
-- does, and otherwise nothing left over from the last field.
sol.on("text_input", function()
    local s = settings()
    if s.caps and s.caps_on_focus and sol.keyboard().caps then
        indicator.show("caps")
    else
        indicator.show("")
    end
end)

-- The keyboard went to another window: whatever was showing was about the
-- last one.
sol.on("focus", function()
    indicator.show("")
end)

indicator.apply()

return indicator
