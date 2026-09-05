-- Scrolling: an endless strip of windows, with the screen as a viewport.
--
-- The arrangement niri and PaperWM use, and the one that makes small screens
-- work: windows keep their width and sit in a row that runs off both edges of
-- the display. Nothing is ever squeezed to fit; the view moves instead.
--
-- It is a script for the same reason tiling is. `sol.place` says where a window
-- lives, the compositor glides it there, and scrolling the viewport is nothing
-- more than placing every window again with a different offset — so the strip
-- slides rather than jumping, and no new compositor code was needed to make
-- that true.

local scrolling = { active = false, offset = 0, focused = 1 }

local GAP = 12
local COLUMN = 0.44 -- of the work area's width
local SETTLE = { duration = 260, easing = "outCubic" }
local SNAP = { duration = 200, easing = "outCubic" }

-- Oldest first, so the strip has a stable order and does not reshuffle when
-- focus moves.
local function ordered()
    local windows = sol.windows()
    local out = {}
    for i = #windows, 1, -1 do
        out[#out + 1] = windows[i]
    end
    return out
end

local function column_width(area)
    return math.floor(area.w * COLUMN)
end

function scrolling.apply(animation)
    if not scrolling.active then
        return
    end
    local windows = ordered()
    if #windows == 0 then
        return
    end

    local area = sol.monitor()
    local width = column_width(area)

    sol.animate(animation or SETTLE)
    for i, window in ipairs(windows) do
        sol.place(window.id, {
            x = area.x + GAP + (i - 1) * (width + GAP) - scrolling.offset,
            y = area.y + GAP,
            w = width,
            h = area.h - GAP * 2,
        })
    end
end

-- Bring a column fully into view. The viewport moves, never the strip's own
-- order — a window that slid out of sight is still in the same place.
function scrolling.focus(index)
    local windows = ordered()
    if #windows == 0 then
        return
    end
    index = math.max(1, math.min(index, #windows))
    scrolling.focused = index

    local area = sol.monitor()
    local width = column_width(area)
    local left = (index - 1) * (width + GAP)

    -- Only scroll far enough to reveal it; a column already on screen stays
    -- where it is rather than being centred for no reason.
    if left < scrolling.offset then
        scrolling.offset = left
    elseif left + width > scrolling.offset + area.w - GAP * 2 then
        scrolling.offset = left + width - (area.w - GAP * 2)
    end

    scrolling.apply(SNAP)
    sol.focus(windows[index].id)
end

function scrolling.toggle()
    scrolling.active = not scrolling.active
    if scrolling.active then
        sol.status("scrolling")
        scrolling.offset = 0
        scrolling.apply()
    else
        sol.status("")
    end
end

sol.bind("super+s", scrolling.toggle)
sol.bind("super+bracketright", function() scrolling.focus(scrolling.focused + 1) end)
sol.bind("super+bracketleft", function() scrolling.focus(scrolling.focused - 1) end)

-- A new window joins the end of the strip and is scrolled to, which is what
-- makes the layout usable without reaching for the mouse.
sol.on("open", function()
    if not scrolling.active then
        return
    end
    scrolling.focus(#ordered())
end)

return scrolling
