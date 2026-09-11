-- The deck: every window as a card, turned in three dimensions.
--
-- This file is the reason the rice exists. Solium's claim is that placing a
-- window's texture somewhere other than its real geometry is *one* operation,
-- and that every mode — overview, the switcher, peek, the genie — is a script
-- over it. A cover-flow switcher is the test of that claim, because it needs
-- the one thing a grid of thumbnails does not: perspective. If it had needed
-- new Rust, the transform layer would have been missing something.
--
-- It did not. `sol.present` already takes `rotate_y` and `perspective`, the
-- animation clock already interpolates toward whatever target is set, and
-- hit-testing already follows the transform — so a card can be clicked where
-- it is drawn without this file knowing how it was drawn.
--
--   super+tab          enter, and step forward
--   super+shift+tab    step back
--   return / escape    leave, keeping or dropping the selection
--   click a card       take that one

local monitors = require("monitors")

local deck = { active = false, order = {}, at = 1 }

-- How far a card is turned, and how far away the eye is. `perspective` is the
-- viewer distance in pixels: the smaller it is the more violent the recession,
-- and below about 600 a card at 50 degrees folds into a wedge. 1100 against a
-- 1600px screen is roughly a normal lens.
local ANGLE = 52
local EYE = 1100

-- The selected card, as a fraction of the screen. Deliberately under half:
-- the point of a switcher is the ones you are not on.
local CARD = 0.56

-- How much of a card's own width separates it from the next one. Sized from
-- the cards rather than from the screen, which is the whole point: a tiled
-- column and a floating window have very different aspects, and a gap measured
-- as a fraction of the screen leaves the narrow ones marooned in empty space
-- and the wide ones on top of each other.
local GAP = 1.08

-- Past this many either side a card is not drawn. Five on screen at once is
-- already more than anyone reads.
local REACH = 3

local ENTER = { duration = 340, easing = { 0.22, 1.0, 0.36, 1.0 } }
local STEPPING = { duration = 260, easing = "outCubic" }
local LEAVE = { duration = 240, easing = "outCubic" }

-- How big a card is, at a given remove from the selection.
--
-- The aspect of the real window is kept: a card is the window, scaled. A
-- switcher that made every window the same shape would be showing you
-- rectangles rather than windows.
local function size_of(window, away, area)
    local scale = 1.0 - math.min(away, REACH) * 0.11
    local height = area.h * CARD * scale
    local aspect = (window.w > 0 and window.h > 0) and (window.w / window.h) or 1.6
    return height * aspect, height
end

-- Where each card goes, for the whole deck at once.
--
-- Cumulative rather than per-card, because where the third card sits depends
-- on how wide the first two turned out. Walking outward from the middle and
-- accumulating half-widths is what keeps the deck evenly spaced whatever
-- shapes are in it.
local function arrange(order, at, area)
    local centre = area.x + area.w / 2
    local placed = {}

    local widths, heights = {}, {}
    for index, window in ipairs(order) do
        local away = math.abs(index - at)
        widths[index], heights[index] = size_of(window, away, area)
    end

    local function put(index, offset)
        local away = math.abs(index - at)
        local width, height = widths[index], heights[index]
        placed[index] = {
            x = centre + offset - width / 2,
            y = area.y + (area.h - height) / 2,
            w = width,
            h = height,
            -- Turned toward the middle: a card on the left presents its right
            -- edge, which is the edge nearer the viewer, and recedes away to
            -- the left. Getting this sign backwards is not subtle and it is
            -- not obviously wrong either — the deck still looks
            -- three-dimensional, it just reads as two fans facing outward
            -- instead of one object seen from the front.
            rotate_y = index == at and 0 or (index < at and -ANGLE or ANGLE),
            perspective = EYE,
            -- Falling away into the dark rather than stopping at an edge,
            -- which is what stops the outermost card from looking like a
            -- mistake. Beyond REACH it is parked invisible rather than left
            -- where it was, or it shows through the scrim at its real size.
            opacity = away == 0 and 1.0 or math.max(0.0, 1.0 - away * 0.26),
        }
        if away > REACH then
            placed[index].opacity = 0
        end
    end

    put(at, 0)
    local offset = 0
    for index = at + 1, #order do
        offset = offset + (widths[index - 1] + widths[index]) / 2 * GAP
        put(index, offset)
    end
    offset = 0
    for index = at - 1, 1, -1 do
        offset = offset - (widths[index + 1] + widths[index]) / 2 * GAP
        put(index, offset)
    end

    return placed
end

-- Draw the deck as it currently stands.
local function lay_out(motion)
    local area = monitors.active()
    if not area then
        return
    end
    sol.animate(motion)
    for index, placement in pairs(arrange(deck.order, deck.at, area)) do
        sol.present(deck.order[index].id, placement)
    end

    -- Raising the selected card is the one thing `sol.present` has no say in:
    -- stacking is the compositor's, and focus is what moves it. Focusing as
    -- the selection moves is also what keeps the caption honest, since the
    -- caption reads the active window rather than being told anything.
    local chosen = deck.order[deck.at]
    if chosen then
        sol.focus(chosen.id)
    end
end

-- The scrim and the caption. Both are QML scenes on layers the windows are not
-- on: the scrim under them so it dims the wallpaper and not the cards, the
-- caption over them but out of the way, and neither interactive — a surface
-- that takes the pointer would swallow the click that picks a card.
local function chrome(on)
    if not on then
        sol.surface("deck-scrim", false)
        sol.surface("deck-caption", false)
        return
    end
    local area = monitors.active()
    sol.surface("deck-scrim", {
        scene = "deck-scrim.qml",
        layer = "bottom",
        on = "every-monitor",
    })
    sol.surface("deck-caption", {
        scene = "deck-caption.qml",
        layer = "overlay",
        on = { x = area.x, y = area.y + area.h - 132, w = area.w, h = 76 },
    })
end

function deck.enter()
    if deck.active then
        return
    end

    -- Only the screen the pointer is on. A switcher that gathered both
    -- monitors' windows onto one of them is a switcher that loses the window
    -- you were looking at.
    local here = monitors.active()
    deck.order = {}
    for _, window in ipairs(sol.windows()) do
        if not here or window.monitor == here.name then
            deck.order[#deck.order + 1] = window
        end
    end

    if #deck.order < 1 then
        sol.log("deck: nothing to switch between")
        return
    end

    -- Start on the window after the focused one, the way every switcher does:
    -- one press should land on the last thing you were in, not on the thing
    -- you are already looking at.
    deck.at = 1
    for index, window in ipairs(deck.order) do
        if window.focused then
            deck.at = index % #deck.order + 1
            break
        end
    end

    deck.active = true
    chrome(true)
    sol.grab_input(true)
    sol.status("deck")
    lay_out(ENTER)
end

function deck.step(by)
    if not deck.active then
        deck.enter()
        return
    end
    local count = #deck.order
    if count == 0 then
        return
    end
    deck.at = (deck.at - 1 + by) % count + 1
    lay_out(STEPPING)
end

-- `keep` decides whether leaving commits the selection. Escape puts the
-- keyboard back where it started; return and a click take the card.
function deck.leave(keep)
    if not deck.active then
        return
    end

    local chosen = keep and deck.order[deck.at] or nil

    deck.active = false
    chrome(false)
    sol.grab_input(false)
    sol.status("")

    -- Every window, not only the ones the deck was opened with: one that
    -- appeared while it was up has a transform too, and leaving has to clear
    -- all of them or it stays where the deck put it forever.
    sol.animate(LEAVE)
    for _, window in ipairs(sol.windows()) do
        sol.present_clear(window.id)
    end

    if chosen then
        sol.focus(chosen.id)
    end
    deck.order = {}
end

sol.bind("super+tab", function() deck.step(1) end)
sol.bind("super+shift+tab", function() deck.step(-1) end)
sol.bind("return", function() deck.leave(true) end)
sol.bind("escape", function() deck.leave(false) end)

-- Clicking a card takes it. `window_at` hit-tests against where a window is
-- *drawn*, so this works against a turned, scaled, half-transparent card
-- without the script knowing that is what it is looking at.
sol.on("click", function(x, y)
    if not deck.active then
        return
    end
    local id = sol.window_at(x, y)
    if id then
        for index, window in ipairs(deck.order) do
            if window.id == id then
                deck.at = index
                break
            end
        end
    end
    deck.leave(id ~= nil)
end)

return deck
