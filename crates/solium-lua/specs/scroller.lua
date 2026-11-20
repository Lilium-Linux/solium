-- What `layouts/scroller.lua` does, a case at a time: a 1000x600 view at no
-- gap unless a case says otherwise, columns opening at a third.

local scroller = require("layouts.scroller")
local expect = require("expect")
local spec, test = expect.spec()

local FREE = { x = 0, y = 0, w = 1000, h = 600, gap = 0 }

local function options(extra)
    local out = { x = 0, y = 0, w = 1000, h = 600, gap = 0 }
    for key, value in pairs(extra or {}) do
        out[key] = value
    end
    return out
end

local function rect_of(strip, id, opts)
    for _, box in ipairs(strip:layout(opts or FREE)) do
        if box.id == id then
            return box
        end
    end
    error("window " .. id .. " is not in the strip", 2)
end

local function with(count)
    local strip = scroller.new()
    for id = 1, count do
        strip:insert(id, FREE)
    end
    return strip
end

local function on_screen(box, view_w)
    return box.x >= -1 and box.x + box.w <= (view_w or 1000) + 1
end

-- A window moves up and down its own column and no further (#150).
test("move vertically swaps within the column and stops at its ends", function()
    local strip = with(3)
    strip:focus_sideways(-2, FREE)
    strip:consume()
    strip:consume()
    expect.same(#strip.columns, 1, "the premise: one column of three")
    expect.same(strip.columns[1].windows, { 1, 2, 3 })
    expect.same(strip:focused(), 3)
    expect.truthy(strip:move_vertically(-1))
    expect.same(strip.columns[1].windows, { 1, 3, 2 })
    expect.same(strip:focused(), 3, "focus went with the window")
    expect.truthy(strip:move_vertically(-1))
    expect.same(strip.columns[1].windows, { 3, 1, 2 })
    expect.falsy(strip:move_vertically(-1), "at the top of the column")
    expect.same(strip.columns[1].windows, { 3, 1, 2 })
    expect.truthy(strip:move_vertically(1))
    expect.same(strip.columns[1].windows, { 1, 3, 2 })

    local alone = with(2)
    expect.falsy(alone:move_vertically(1), "a column of one has no below")
    expect.falsy(alone:move_vertically(-1))
    expect.falsy(scroller.new():move_vertically(1), "an empty strip")
end)

-- Nothing divides by how many windows there are.
test("a column keeps its width however many there are", function()
    expect.same(rect_of(with(2), 1).w, rect_of(with(10), 1).w)
end)

test("a new window opens beside the active column, not at the end", function()
    local strip = with(3)
    strip:focus_sideways(-2, FREE)
    expect.same(strip.active, 1)
    strip:insert(99, FREE)
    expect.same(strip.active, 2, "it went right of the active one")
    expect.same(strip:focused(), 99)
end)

test("focusing a visible column does not move the view", function()
    local strip = with(2)
    local before = rect_of(strip, 1).x
    strip:focus_sideways(-1, FREE)
    expect.near(rect_of(strip, 1).x, before, 1)
end)

test("focusing a column off the edge brings it into view", function()
    expect.truthy(on_screen(rect_of(with(4), 4)))
end)

test("the focused column is always visible", function()
    local strip = with(6)
    for _, step in ipairs({ -1, -1, -1, 1, 1, -4, 5 }) do
        strip:focus_sideways(step, FREE)
        local focused = strip:focused()
        expect.truthy(focused, "something is focused")
        expect.truthy(on_screen(rect_of(strip, focused)), "focused column " .. focused .. " off screen")
    end
end)

test("windows in a column share its width and split its height", function()
    local strip = with(1)
    strip:insert_into_column(2, FREE)
    strip:insert_into_column(3, FREE)
    local one, three = rect_of(strip, 1), rect_of(strip, 3)
    expect.same(one.w, three.w, "same column, same width")
    expect.near(one.h, 200, 1, "three share the height")
    expect.truthy(three.y > one.y)
end)

test("consuming takes the next column into this one", function()
    local strip = with(2)
    strip:focus_sideways(-1, FREE)
    strip:consume()
    expect.same(#strip.columns, 1)
    expect.same(#strip.columns[1].windows, 2)
end)

-- The column a window was taken from keeps a focus that is one of its own.
test("consuming the bottom of a stack leaves its column a focus it has", function()
    local strip = with(1)
    strip:insert(2, FREE)
    strip:insert_into_column(3, FREE)
    strip:focus_sideways(-1, FREE)
    strip:consume()
    expect.same(strip.columns[1].windows, { 1, 3 })
    expect.same(strip.columns[2].windows, { 2 })
    expect.same(strip.columns[2].active, 1)
end)

test("expelling puts a window back in its own column", function()
    local strip = with(1)
    strip:insert_into_column(2, FREE)
    expect.same(#strip.columns, 1)
    strip:expel(FREE)
    expect.same(#strip.columns, 2)
    expect.same(strip:focused(), 2)
end)

test("cycling width walks the widths", function()
    local strip = with(1)
    local first = rect_of(strip, 1).w
    strip:cycle_width(FREE)
    local second = rect_of(strip, 1).w
    expect.truthy(math.abs(first - second) > 1, "width changed")
    for _ = 1, #scroller.WIDTHS do
        strip:cycle_width(FREE)
    end
    expect.near(rect_of(strip, 1).w, second, 1, "and comes back around")
end)

-- `scrolling.widths` and `default_width` are the widths and where the cycle
-- starts (#117), with two numbers nothing else uses.
test("a strip cycles through the widths it was given", function()
    local strip = scroller.new({ widths = { 0.25, 0.8 }, default_width = 2 })
    strip:insert(1, FREE)
    expect.near(rect_of(strip, 1).w, 800, 1, "a new column opens at default_width")
    strip:cycle_width(FREE)
    expect.near(rect_of(strip, 1).w, 250, 1, "and cycling wraps to the first")
    strip:cycle_width(FREE)
    expect.near(rect_of(strip, 1).w, 800, 1, "a lap of two is two steps")
end)

-- What a reload hands a strip it carried across (#118).
test("a strip configured again keeps its columns and takes the new widths", function()
    local strip = with(3)
    strip:cycle_width(FREE)
    strip:focus_sideways(-2, FREE)
    local before = strip:layout(FREE)
    strip:configure({ widths = { 0.25, 0.8 }, default_width = 2 })
    expect.same(strip:layout(FREE), before, "the columns, widths, focus and view are unchanged")
    strip:insert(4, FREE)
    expect.near(rect_of(strip, 4).w, 800, 1, "a new column opens at the new default_width")
    strip:cycle_width(FREE)
    expect.near(rect_of(strip, 4).w, 250, 1, "and the cycle walks the new list")
end)

-- Read while the configuration is: one bad number costs neither the strip nor
-- anything else, and each is said.
test("widths that cannot be used fall back, and are said", function()
    local said = {}
    local function say(line)
        said[#said + 1] = line
    end
    local strip = scroller.new({ widths = { 0, "half", 1.5 } }, say)
    strip:insert(1, FREE)
    expect.near(rect_of(strip, 1).w, 1000 * scroller.WIDTHS[1], 1, "none usable is the shipped widths")
    expect.same(#said, 3, "each width left out is said")

    said = {}
    strip = scroller.new({ widths = { 0.4, 0.6 }, default_width = 9 }, say)
    strip:insert(1, FREE)
    expect.near(rect_of(strip, 1).w, 600, 1, "a place past the end is the last width")
    expect.same(#said, 1)

    said = {}
    strip = scroller.new({ widths = { 0.4, 0.6 }, default_width = 0 / 0 }, say)
    strip:insert(1, FREE)
    expect.near(rect_of(strip, 1).w, 400, 1, "a place that is no number is the first")
    expect.same(#said, 1)

    said = {}
    scroller.new({ widths = { 0.4, 0.6 } }, say)
    scroller.new(nil, say)
    expect.same(#said, 0, "nothing wrong, nothing said")
end)

test("removing a column keeps focus somewhere real", function()
    local strip = with(3)
    strip:remove(3)
    expect.truthy(strip:focused())
    strip:remove(2)
    strip:remove(1)
    expect.truthy(strip:is_empty())
    expect.same(strip:layout(FREE), {})
end)

test("removing one of a stack leaves the column", function()
    local strip = with(1)
    strip:insert_into_column(2, FREE)
    strip:remove(1)
    expect.same(#strip.columns, 1)
    expect.same(strip.columns[1].windows, { 2 })
end)

test("moving a column carries focus with it", function()
    local strip = with(3)
    local focused = strip:focused()
    expect.truthy(strip:move_column(-1, FREE))
    expect.same(strip:focused(), focused, "the window went with the move")
    expect.same(strip.active, 2)
end)

-- So the end of the strip, where a move goes on to the next monitor, is told
-- from a move that happened (#150).
test("move column says whether it moved", function()
    local strip = with(3)
    expect.falsy(strip:move_column(1, FREE), "the last column")
    expect.same(strip:windows(), { 1, 2, 3 })
    expect.truthy(strip:move_column(-1, FREE))
    expect.truthy(strip:move_column(-1, FREE))
    expect.same(strip:windows(), { 3, 1, 2 })
    expect.falsy(strip:move_column(-1, FREE), "the first column")
    expect.falsy(with(1):move_column(-1, FREE), "a strip of one")
end)

-- Where the view goes (#154): a column wider than the view, or exactly as
-- wide, starts where the view does; one in view moves nothing; one off
-- either edge comes in by the shorter way, a margin of up to a gap inside.

test("an oversized column is left aligned", function()
    local strip = scroller.new()
    strip:insert(1, FREE)
    for _ = 1, 6 do
        strip:cycle_width(FREE)
    end
    local narrow = { x = 0, y = 0, w = 300, h = 600, gap = 0 }
    expect.near(strip:layout(narrow)[1].x, 0, 1, "left-aligned")
end)

test("a column wider than the view starts where the view does", function()
    local opts = options({ gap = 12, floors = { [2] = { w = 1500, h = 0 } } })
    local strip = scroller.new()
    strip:insert(1, opts)
    strip:insert(2, opts)
    local two = rect_of(strip, 2, opts)
    expect.near(two.w, 1500, 1e-6, "the premise: the floor makes it wider than the view")
    expect.near(two.x, 12, 1e-6, "its start is the view's")
end)

test("a column exactly as wide as the view fills it", function()
    local opts = options({ gap = 12 })
    local strip = scroller.new({ widths = { 1 } })
    strip:insert(1, opts)
    strip:insert(2, opts)
    local two = rect_of(strip, 2, opts)
    expect.near(two.x, 12, 1e-6)
    expect.near(two.w, 976, 1e-6)
    strip:focus_sideways(-1, opts)
    expect.near(rect_of(strip, 1, opts).x, 12, 1e-6, "and the one before it, focused, does too")
end)

-- Halves of a 976 view at a 12 gap are 488 wide: a column off the right edge
-- comes in with its right edge a gap inside the view's, and one off the left
-- edge with its left edge a gap inside.
test("a column off either edge comes in a gap inside it", function()
    local opts = options({ gap = 12 })
    local strip = scroller.new({ widths = { 0.5 } })
    for id = 1, 4 do
        strip:insert(id, opts)
    end
    local four = rect_of(strip, 4, opts)
    expect.near(four.x + four.w, 1000 - 12 - 12, 1e-6, "off the right edge: in a gap inside")
    strip:focus_window(1, opts)
    local one = rect_of(strip, 1, opts)
    expect.near(one.x, 12 + 12, 1e-6, "off the left edge: in a gap inside")
end)

-- With less room either side than a gap, the margin is what there is.
test("the margin is never more than half what the view has over", function()
    local opts = options({ gap = 100 })
    local strip = scroller.new({ widths = { 0.9 } })
    strip:insert(1, opts)
    strip:insert(2, opts)
    local two = rect_of(strip, 2, opts)
    local view = { x = 100, w = 800 }
    expect.near(two.x - view.x, (view.w - two.w) / 2, 1e-6, "centred, with the room split")
end)

test("focusing a half visible column brings it fully on screen", function()
    local strip = with(4)
    strip:focus_sideways(-3, FREE)
    local off = rect_of(strip, 4)
    expect.truthy(off.x + off.w > 1000, "window 4 starts off the edge")
    strip:focus_window(4, FREE)
    expect.truthy(on_screen(rect_of(strip, 4)), "still not fully visible")
end)

test("widening changes the column and every window in it", function()
    local strip = scroller.new()
    strip:insert(1, FREE)
    strip:insert_into_column(2, FREE)
    local before = rect_of(strip, 1).w
    strip:widen(1, 0.2, FREE)
    local one, two = rect_of(strip, 1), rect_of(strip, 2)
    expect.truthy(one.w > before, "the column grew")
    expect.same(one.w, two.w, "both windows with it")
end)

test("a column can neither vanish nor outgrow the view", function()
    local strip = scroller.new()
    strip:insert(1, FREE)
    strip:widen(1, -10, FREE)
    expect.truthy(rect_of(strip, 1).w >= 99, "floor holds")
    strip:widen(1, 10, FREE)
    expect.truthy(rect_of(strip, 1).w <= 1001, "ceiling holds")
end)

test("a window dropped on another column joins it", function()
    local strip = with(3)
    expect.same(#strip.columns, 3)
    strip:move_to_column_of(3, 1, FREE)
    expect.same(#strip.columns, 2, "one column absorbed another")
    expect.same(strip.columns[1].windows, { 1, 3 })
    expect.same(strip:focused(), 3, "focus went with it")
end)

test("moving out of a stack leaves the column standing", function()
    local strip = scroller.new()
    strip:insert(1, FREE)
    strip:insert_into_column(2, FREE)
    strip:insert(3, FREE)
    strip:move_to_column_of(2, 3, FREE)
    expect.same(#strip.columns, 2)
    expect.same(strip.columns[1].windows, { 1 })
    expect.same(strip.columns[2].windows, { 3, 2 })
end)

test("a window cannot be moved onto itself", function()
    local strip = with(2)
    strip:move_to_column_of(1, 1, FREE)
    expect.same(#strip.columns, 2)
end)

-- A column is never narrower than its windows' floors (#115).

local function wide(floors)
    local out = {}
    for id, w in pairs(floors) do
        out[id] = { w = w, h = 0 }
    end
    return options({ floors = out })
end

test("a column is never narrower than its widest floor", function()
    local strip = with(2)
    strip:insert_into_column(3, FREE)
    local opts = wide({ [3] = 600 })
    expect.near(rect_of(strip, 2, opts).w, 600, 1e-6)
    expect.near(rect_of(strip, 3, opts).w, 600, 1e-6)
    expect.near(rect_of(strip, 1, opts).w, 1000 / 3, 1e-6)
end)

test("the next column starts after a widened one", function()
    local strip = with(3)
    local opts = wide({ [1] = 600 })
    local one, two = rect_of(strip, 1, opts), rect_of(strip, 2, opts)
    expect.near(two.x, one.x + 600, 1e-6, "column 2 starts inside column 1")
end)

-- Without the stop the share went on down out of sight, and the drag back had
-- to pay it all back first.
test("narrowing a column stops at its floor", function()
    local strip = with(1)
    local opts = wide({ [1] = 500 })
    strip:widen(1, -0.1, opts)
    expect.near(rect_of(strip, 1, opts).w, 500, 1e-6)
    strip:widen(1, 0.1, opts)
    expect.near(rect_of(strip, 1, opts).w, 600, 1e-6, "the column did not widen from its floor")
end)

test("widening a column its floor holds widens it at once", function()
    local strip = with(1)
    local opts = wide({ [1] = 500 })
    expect.near(rect_of(strip, 1, opts).w, 500, 1e-6, "the premise")
    strip:widen(1, 0.1, opts)
    expect.near(rect_of(strip, 1, opts).w, 600, 1e-6, "the press was swallowed by the floor")
end)

-- Kept across a reload.

test("a strip comes back from what was kept as the strip it was", function()
    local strip = scroller.new({ widths = { 0.25, 0.5, 0.75 }, default_width = 2 })
    for id = 1, 4 do
        strip:insert(id, FREE)
    end
    strip:insert_into_column(5, FREE)
    strip:remove(2)
    local back = scroller.adopt(expect.copy(strip))
    expect.truthy(back, "a strip")
    expect.same(back:layout(FREE), strip:layout(FREE))
    expect.same(back, strip)
    expect.truthy(scroller.adopt(expect.copy(scroller.new())), "an empty strip")
end)

test("what is not a strip is refused", function()
    local function broken(change)
        local kept = expect.copy(with(3))
        change(kept)
        return scroller.adopt(kept) == nil
    end
    expect.falsy(broken(function() end), "the strip itself is one")
    expect.truthy(broken(function(kept) kept.active = 40 end), "a focus past the end")
    expect.truthy(broken(function(kept) kept.widths = {} end), "no width to cycle through")
    expect.truthy(broken(function(kept)
        kept.columns[#kept.columns + 1] = { windows = {}, active = 1, width = 0.5 }
    end), "a column with no window in it")
    expect.truthy(broken(function(kept) kept.offset = 0 / 0 end), "a view that is nowhere")
    expect.truthy(scroller.adopt("strip") == nil)
end)

-- A whole number can come back as a float that is one; it is still the
-- place it was.
test("a kept place that came back as a float is still that place", function()
    local kept = expect.copy(with(3))
    kept.active, kept.preset = 2.0, 1.0
    kept.columns[1].active = 1.0
    local back = scroller.adopt(kept)
    expect.truthy(back)
    expect.same(math.type(back.active), "integer")
    expect.same(back:focused(), 2)
end)

return spec
