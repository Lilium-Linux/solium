-- What `layouts/dwindle.lua` does, a case at a time.
--
-- A 1000x600 area at no gap unless a case says otherwise, so every number is
-- the one written down.

local dwindle = require("layouts.dwindle")
local expect = require("expect")
local spec, test = expect.spec()

local AREA = { x = 0, y = 0, w = 1000, h = 600 }

-- An options table for `area` (the 1000x600 when nil) with `gap` 0 and
-- `split` 0.5, and whatever `extra` adds or replaces.
local function options(extra, area)
    area = area or AREA
    local out = { x = area.x, y = area.y, w = area.w, h = area.h, gap = 0, split = 0.5 }
    for key, value in pairs(extra or {}) do
        out[key] = value
    end
    return out
end

local FREE = options()
local GAPPED = options({ gap = 12 })

-- The shipped `config.tiling.minimum` and `config.gap`.
local SHIPPED = options({ gap = 12, minimum = { w = 160, h = 96 } })

local function rect_of(tree, id, opts)
    for _, box in ipairs(tree:layout(opts or FREE)) do
        if box.id == id then
            return box
        end
    end
    error("window " .. id .. " is not in the tree", 2)
end

-- Every tile, by window id.
local function every(tree, opts)
    local out = tree:layout(opts or FREE)
    table.sort(out, function(a, b)
        return a.id < b.id
    end)
    return out
end

local function clone(tree)
    return dwindle.adopt(expect.copy(tree))
end

-- Two columns of two: 1 top-left, 2 top-right, 3 bottom-left, 4
-- bottom-right. One vertical seam at the root, and each column a horizontal
-- seam of its own, so each window's own parent is cut the other way to the
-- seam its side edges touch.
local function quad(opts)
    opts = opts or FREE
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, opts)
    tree:insert(2, 1, nil, nil, opts)
    tree:insert(3, 1, nil, nil, opts)
    tree:insert(4, 2, nil, nil, opts)
    return tree
end

-- Where `edge` of `box` is, as `drag_seam` takes it.
local function own_edge(box, edge)
    if edge == "left" then
        return box.x, box.y + box.h / 2
    elseif edge == "right" then
        return box.x + box.w, box.y + box.h / 2
    elseif edge == "top" then
        return box.x + box.w / 2, box.y
    end
    return box.x + box.w / 2, box.y + box.h
end

test("the first window takes everything", function()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    expect.near(rect_of(tree, 1).w, 1000, 1)
end)

-- The split runs across the longer side of the window being split, not of
-- the screen.
test("a wide window splits side by side and a tall one does not", function()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    tree:insert(2, 1, nil, nil, FREE)
    expect.near(rect_of(tree, 1).w, 500, 1, "cut down the middle")
    tree:insert(3, 2, nil, nil, FREE)
    local two, three = rect_of(tree, 2), rect_of(tree, 3)
    expect.near(two.w, 500, 1, "width untouched")
    expect.near(two.h, 300, 1, "split one above the other")
    expect.truthy(three.y > two.y, "the new window went below")
end)

test("the pointer decides which side the new window takes", function()
    local left = dwindle.new()
    left:insert(1, nil, nil, nil, FREE)
    left:insert(2, 1, 100, 300, FREE)
    expect.truthy(rect_of(left, 2).x < rect_of(left, 1).x, "dropped left, opens left")

    local right = dwindle.new()
    right:insert(1, nil, nil, nil, FREE)
    right:insert(2, 1, 900, 300, FREE)
    expect.truthy(rect_of(right, 2).x > rect_of(right, 1).x, "dropped right, opens right")
end)

test("with no target the window under the pointer is split", function()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    tree:insert(2, 1, 900, 300, FREE)
    tree:insert(3, nil, 100, 300, FREE)
    expect.near(rect_of(tree, 2).w, 500, 1, "the untouched half kept its width")
    local one = rect_of(tree, 1)
    expect.near(one.w, 500, 1, "width unchanged")
    expect.near(one.h, 300, 1, "the pointed-at window was divided")
end)

test("removing a window gives its space to its sibling", function()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    tree:insert(2, 1, nil, nil, FREE)
    tree:insert(3, 2, nil, nil, FREE)
    local before = rect_of(tree, 1)
    tree:remove(3)
    expect.same(#tree:windows(), 2)
    expect.near(rect_of(tree, 1).w, before.w, 1, "the far side did not move")
    expect.near(rect_of(tree, 2).h, 600, 1, "the sibling took the space")
end)

test("removing the last window empties the tree", function()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    tree:remove(1)
    expect.truthy(tree:is_empty())
    expect.same(tree:layout(FREE), {})
end)

test("resizing moves one seam and leaves the rest", function()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    tree:insert(2, 1, nil, nil, FREE)
    tree:insert(3, 2, nil, nil, FREE)
    local untouched = rect_of(tree, 1)
    tree:resize(2, "height", 0.2, FREE)
    expect.truthy(rect_of(tree, 2).h > 300, "the seam moved")
    expect.near(rect_of(tree, 1).w, untouched.w, 1, "the other branch is untouched")
end)

test("every window gets a box and none collapses", function()
    local tree = dwindle.new()
    for id = 1, 10 do
        tree:insert(id, id - 1, nil, nil, FREE)
    end
    local boxes = tree:layout(FREE)
    expect.same(#boxes, 10)
    for _, box in ipairs(boxes) do
        expect.truthy(box.w >= 1 and box.h >= 1, "window " .. box.id .. " collapsed")
    end
end)

test("a window is never added twice", function()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    tree:insert(1, nil, nil, nil, FREE)
    expect.same(tree:windows(), { 1 })
end)

-- A swap trades two tiles and nothing else (#150), and a second swap puts
-- the tree back.
test("swap trades two tiles and nothing else", function()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    tree:insert(2, 1, 900, 300, FREE)
    tree:insert(3, 2, 900, 500, FREE)
    tree:resize(1, "width", 0.1, FREE)
    local before = { rect_of(tree, 1), rect_of(tree, 2), rect_of(tree, 3) }

    expect.truthy(tree:swap(1, 3), "both windows are in the tree")
    expect.close(rect_of(tree, 1), before[3], "1 took 3's tile")
    expect.close(rect_of(tree, 3), before[1], "3 took 1's tile")
    expect.close(rect_of(tree, 2), before[2], "2 was not part of it")

    expect.truthy(tree:swap(3, 1))
    expect.same({ rect_of(tree, 1), rect_of(tree, 2), rect_of(tree, 3) }, before,
        "a second swap is the tree it started as")
    expect.falsy(tree:swap(1, 9), "9 is not in the tree")
    expect.same({ rect_of(tree, 1), rect_of(tree, 2), rect_of(tree, 3) }, before,
        "a refused swap changed the tree")
end)

-- With the pointer off every window, a window is still what is split: the
-- whole area would make every window another full-height column.
test("a pointer off the edge still divides a window, not the screen", function()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    tree:insert(2, nil, -500, -500, FREE)
    tree:insert(3, nil, -500, -500, FREE)
    local full_height = 0
    for _, box in ipairs(tree:layout(FREE)) do
        if box.h > 599 then
            full_height = full_height + 1
        end
    end
    expect.truthy(full_height < 3, "the screen was split each time")
end)

test("opening three windows at the origin keeps all three", function()
    local opts = options({ gap = 12 }, { x = 0, y = 0, w = 1600, h = 900 })
    local tree = dwindle.new()
    for id = 1, 3 do
        tree:insert(id, nil, 0, 0, opts)
    end
    expect.same(#tree:windows(), 3)
end)

-- A new window is under the pointer before it is in the tree, so a tiling
-- script can name it as its own target.
test("a window named as its own target is still added", function()
    local opts = options({ gap = 12 }, { x = 0, y = 0, w = 1600, h = 900 })
    local tree = dwindle.new()
    for id = 1, 4 do
        tree:insert(id, id, 0, 0, opts)
    end
    expect.same(#tree:windows(), 4, "windows were dropped")
    for _, box in ipairs(tree:layout(opts)) do
        expect.truthy(box.w >= 1 and box.h >= 1, "window " .. box.id .. " collapsed")
    end
end)

-- Seams.

test("a two by two can have its width dragged", function()
    local tree = quad()
    expect.near(rect_of(tree, 1).w, 500, 1, "starts halved")
    tree:drag_seam(1, "right", 700, 300, FREE)
    expect.near(rect_of(tree, 1).w, 700, 2, "the seam went where it was asked")
end)

test("dragging to the same place twice is the same layout", function()
    local tree = quad()
    tree:drag_seam(1, "right", 650, 300, FREE)
    local once = rect_of(tree, 1).w
    for _ = 1, 20 do
        tree:drag_seam(1, "right", 650, 300, FREE)
    end
    expect.same(rect_of(tree, 1).w, once)
end)

test("height drags find the horizontal seam", function()
    local tree = quad()
    tree:drag_seam(1, "bottom", 250, 400, FREE)
    expect.near(rect_of(tree, 1).h, 400, 2, "went where it was asked")
end)

test("a lone window has no seam to drag", function()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    tree:drag_seam(1, "right", 700, 300, FREE)
    expect.near(rect_of(tree, 1).w, 1000, 1, "unchanged")
end)

test("a side that is none of the four moves nothing and says so", function()
    local tree = quad()
    local before = every(tree)
    expect.falsy(tree:drag_seam(1, "rigth", 700, 300, FREE))
    expect.same(every(tree), before)
end)

-- The seam that moves is the one beside the edge that was grabbed (#120).

test("a right edge drag moves the seam on the right", function()
    local tree = quad()
    tree:drag_seam(1, "right", 700, 150, FREE)
    local one = rect_of(tree, 1)
    expect.near(one.x, 0, 1, "the left edge stood still")
    expect.near(one.w, 700, 2, "the right edge followed")
    expect.near(rect_of(tree, 3).w, 700, 2, "the root seam moved, so the whole column did")
    expect.near(one.h, 300, 1, "nothing horizontal moved")
end)

test("a left edge drag moves the seam on the left", function()
    local tree = quad()
    tree:drag_seam(2, "left", 700, 150, FREE)
    local two = rect_of(tree, 2)
    expect.near(two.x, 700, 2, "the left edge followed")
    expect.near(two.x + two.w, 1000, 2, "the right edge stood still")
end)

test("a bottom edge drag moves the seam below", function()
    local tree = quad()
    tree:drag_seam(1, "bottom", 250, 400, FREE)
    local one = rect_of(tree, 1)
    expect.near(one.y, 0, 1, "the top edge stood still")
    expect.near(one.h, 400, 2, "the bottom edge followed")
    expect.near(one.w, 500, 1, "nothing vertical moved")
    expect.near(rect_of(tree, 2).h, 300, 1, "the other column's seam did not move")
end)

test("a top edge drag moves the seam above", function()
    local tree = quad()
    tree:drag_seam(3, "top", 250, 200, FREE)
    local three = rect_of(tree, 3)
    expect.near(three.y, 200, 2, "the top edge followed")
    expect.near(three.y + three.h, 600, 2, "the bottom edge stood still")
    expect.near(rect_of(tree, 1).h, 200, 2, "the neighbour above gave up the space")
end)

-- Beyond an edge against the area is the screen, and the screen does not
-- move.
test("a window against the container has no seam on that side", function()
    for _, case in ipairs({
        { 1, "left", 200, 150 },
        { 1, "top", 250, 200 },
        { 2, "right", 800, 150 },
        { 3, "bottom", 250, 450 },
    }) do
        local tree = quad()
        local before = every(tree)
        tree:drag_seam(case[1], case[2], case[3], case[4], FREE)
        expect.same(every(tree), before,
            "dragging window " .. case[1] .. "'s " .. case[2] .. " edge moved something")
    end
end)

-- The quad with the bottom-right corner split again: window 4's right edge
-- is that inner seam, and its left edge the root's, two splits up, past one
-- cut the other way and one it is on the wrong side of.
local function nested()
    local tree = quad()
    tree:insert(5, 4, nil, nil, FREE)
    return tree
end

test("the seam beside an edge is not always the nearest ancestor", function()
    local tree = nested()
    expect.near(rect_of(tree, 4).w, 250, 1, "the corner was split again")
    tree:drag_seam(4, "left", 700, 450, FREE)
    expect.near(rect_of(tree, 1).w, 700, 2, "the root seam moved")
    local five = rect_of(tree, 5)
    expect.near(five.x + five.w, 1000, 2, "the far side of the inner seam is still against the edge")
end)

test("a nested window at the far edge still moves nothing", function()
    local tree = nested()
    local before = every(tree)
    tree:drag_seam(5, "right", 900, 450, FREE)
    expect.same(every(tree), before, "the inner seam was dragged")
end)

-- Every leaf's own parent in the quad is cut across, so a press that only
-- moved the parent could never make a window wider.
test("the keyboard can reach the centre vertical seam", function()
    local tree = quad()
    tree:resize(1, "width", 0.1, FREE)
    local one = rect_of(tree, 1)
    expect.near(one.w, 600, 2, "it got wider")
    expect.near(one.h, 300, 1, "and not taller")
end)

test("a window against the container grows from the other side", function()
    local tree = quad()
    tree:resize(2, "width", 0.1, FREE)
    local two = rect_of(tree, 2)
    expect.near(two.w, 600, 2, "it got wider")
    expect.near(two.x + two.w, 1000, 2, "from the left, because the right is the container's")
end)

-- At a gap, the two sides of a seam are a gap apart, and each grabbed edge
-- has to land exactly where it was asked: a share of the box rather than of
-- the box less the gap puts them half a gap off each way.
test("a dragged vertical edge lands where it was asked across the gap", function()
    local trailing = quad(GAPPED)
    trailing:drag_seam(1, "right", 700, 150, GAPPED)
    local one = rect_of(trailing, 1, GAPPED)
    expect.near(one.x + one.w, 700, 0.5, "the grabbed right edge is where it was asked")

    local leading = quad(GAPPED)
    leading:drag_seam(2, "left", 700, 150, GAPPED)
    local two = rect_of(leading, 2, GAPPED)
    expect.near(two.x, 700, 0.5, "the grabbed left edge is where it was asked")
    expect.near(one.x + one.w, two.x, 0.5, "the two sides of one seam disagree")
end)

test("a dragged horizontal edge lands where it was asked across the gap", function()
    local trailing = quad(GAPPED)
    trailing:drag_seam(1, "bottom", 250, 400, GAPPED)
    local one = rect_of(trailing, 1, GAPPED)
    expect.near(one.y + one.h, 400, 0.5, "the grabbed bottom edge is where it was asked")

    local leading = quad(GAPPED)
    leading:drag_seam(3, "top", 250, 400, GAPPED)
    local three = rect_of(leading, 3, GAPPED)
    expect.near(three.y, 400, 0.5, "the grabbed top edge is where it was asked")
    expect.near(one.y + one.h, three.y, 0.5, "the two sides of one seam disagree")
end)

-- A window handed its own edge does not move: the box a seam is measured in
-- and the tiles the layout draws are one arrangement. Off-centre first,
-- because at an even split a skew of the box by half a gap cancels out.
test("a window handed its own edge does not move", function()
    for _, case in ipairs({
        { 1, "right", 340, 150 },
        { 2, "left", 640, 150 },
        { 1, "bottom", 250, 200 },
        { 3, "top", 250, 420 },
    }) do
        local id, edge = case[1], case[2]
        local tree = quad(GAPPED)
        local centred = rect_of(tree, id, GAPPED)
        tree:drag_seam(id, edge, case[3], case[4], GAPPED)
        local before = rect_of(tree, id, GAPPED)
        expect.truthy(math.abs(before.w - centred.w) > 50 or math.abs(before.h - centred.h) > 50,
            "the fixture is off-centre before it proves anything")
        local x, y = own_edge(before, edge)
        tree:drag_seam(id, edge, x, y, GAPPED)
        expect.close(rect_of(tree, id, GAPPED), before,
            "window " .. id .. " handed its own " .. edge .. " edge moved")
    end
end)

test("dragging to the same place twice changes nothing at a real gap", function()
    local tree = quad(GAPPED)
    tree:drag_seam(1, "right", 700, 150, GAPPED)
    local once = rect_of(tree, 1, GAPPED)
    tree:drag_seam(1, "right", 700, 150, GAPPED)
    expect.near(rect_of(tree, 1, GAPPED).w, once.w, 0.001, "the second drag moved it")
end)

-- The minimum (#134). Each arrangement is built with `insert` and no
-- minimum, and only then asked a question with one in force.

local function floored(gap, w, h, area)
    return options({ gap = gap, minimum = { w = w, h = h } }, area)
end

test("a tile with no room side by side splits one above the other", function()
    local area = { x = 0, y = 0, w = 300, h = 200 }
    local opts = floored(0, 160, 96, area)
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, opts)

    local allowed = clone(tree)
    allowed:insert(2, nil, 150, 150, opts)
    expect.near(rect_of(allowed, 2, opts).w, 150, 0.5, "the premise: insert cuts this side by side")

    expect.truthy(tree:insert_fitting(2, nil, 150, 150, opts))
    local one, two = rect_of(tree, 1, opts), rect_of(tree, 2, opts)
    expect.truthy(math.abs(one.w - 300) < 0.5 and math.abs(two.w - 300) < 0.5,
        "split one above the other, full width each")
    expect.truthy(math.abs(one.h - 100) < 0.5 and math.abs(two.h - 100) < 0.5,
        "and half the height each")
    expect.truthy(two.y > one.y, "on the pointer's side, below")
end)

-- False is only useful if trying somewhere else starts from where the tree
-- was, and a node made and abandoned would stop `insert` taking the window.
test("a tile with no room either way is left alone", function()
    local area = { x = 0, y = 0, w = 300, h = 180 }
    local opts = floored(0, 160, 96, area)
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, opts)
    local before = every(tree, opts)
    expect.falsy(tree:insert_fitting(2, nil, 150, 90, opts))
    expect.falsy(tree:insert_largest(2, opts))
    expect.falsy(tree:contains(2), "a refused window is not in the tree")
    expect.same(every(tree, opts), before)
    tree:insert(2, nil, 150, 90, opts)
    expect.same(#tree:windows(), 2)
end)

-- 356 less a 12 gap each side is 332, and 332 less the 12 between is two of
-- exactly 160. A pixel narrower is 159.5 each, and 200 high is two of 94.
test("a split that lands exactly on the minimum has room", function()
    local exact = { x = 0, y = 0, w = 356, h = 224 }
    local opts = options({ gap = 12, minimum = { w = 160, h = 96 } }, exact)
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, opts)
    expect.truthy(tree:insert_fitting(2, nil, 300, 100, opts), "two halves of exactly the minimum were refused")
    for _, id in ipairs({ 1, 2 }) do
        expect.near(rect_of(tree, id, opts).w, 160, 1e-9, "window " .. id .. " is not at the minimum")
    end

    local short = options({ gap = 12, minimum = { w = 160, h = 96 } }, { x = 0, y = 0, w = 355, h = 224 })
    tree = dwindle.new()
    tree:insert(1, nil, nil, nil, short)
    expect.falsy(tree:insert_fitting(2, nil, 300, 100, short), "halves of 159.5 were let under 160")
end)

test("an empty tree always has room", function()
    local tiny = options({ gap = 12, minimum = { w = 160, h = 96 } }, { x = 0, y = 0, w = 100, h = 50 })
    local fitting, largest = dwindle.new(), dwindle.new()
    expect.truthy(fitting:insert_fitting(1, nil, nil, nil, tiny))
    expect.truthy(largest:insert_largest(1, tiny))
    expect.same(fitting:windows(), { 1 })
    expect.same(largest:windows(), { 1 })
end)

-- Window 1 on the left, 599x700; 2 over 3 on the right, 601x401 and 601x299.
-- Asked about a 300x360 minimum: 1 is the largest and has room neither way, 3
-- is already too short, and 2 has room side by side.
test("largest splits the largest tile that has room", function()
    local area = { x = 0, y = 0, w = 1200, h = 700 }
    local free = options(nil, area)
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, free)
    tree:insert(2, 1, nil, nil, free)
    tree:insert(3, 2, nil, nil, free)
    tree:drag_seam(1, "right", 599, 0, free)
    tree:drag_seam(2, "bottom", 0, 401, free)

    local opts = floored(0, 300, 360, area)
    local one, three = rect_of(tree, 1, opts), rect_of(tree, 3, opts)
    expect.truthy(math.abs(one.w - 599) < 0.5 and math.abs(three.h - 299) < 0.5,
        "the fixture is not the one described")
    expect.falsy(tree:insert_fitting(4, nil, 900, 600, opts))
    expect.truthy(tree:insert_largest(4, opts))
    local two, four = rect_of(tree, 2, opts), rect_of(tree, 4, opts)
    expect.truthy(math.abs(two.w - 300.5) < 0.5 and math.abs(four.w - 300.5) < 0.5,
        "window 2's tile was not the one split")
    expect.truthy(four.x > two.x, "the new window takes the far side")
    expect.close(rect_of(tree, 1, opts), one, "1 moved")
    expect.close(rect_of(tree, 3, opts), three, "3 moved")
end)

test("a seam cannot be dragged to make a tile smaller than the minimum", function()
    local opts = floored(12, 300, 150)
    for _, case in ipairs({
        { 1, "right", 100, 150, 1, 300, "w" },
        { 2, "left", 950, 150, 2, 300, "w" },
        { 1, "bottom", 250, 50, 1, 150, "h" },
        { 3, "top", 250, 580, 3, 150, "h" },
    }) do
        local tree = quad(GAPPED)
        tree:drag_seam(case[1], case[2], case[3], case[4], opts)
        expect.near(rect_of(tree, case[5], opts)[case[7]], case[6], 0.5,
            "dragging window " .. case[1] .. "'s " .. case[2] .. " edge went past the minimum")
    end
end)

-- The root seam, window 4's left edge, has to leave room for both halves of
-- the pair beyond it: 160, the gap, 160.
test("a seam leaves room for every tile beyond it", function()
    local tree = quad(GAPPED)
    tree:insert(5, 4, nil, nil, GAPPED)
    tree:drag_seam(4, "left", 900, 450, SHIPPED)
    local four, five = rect_of(tree, 4, SHIPPED), rect_of(tree, 5, SHIPPED)
    expect.truthy(math.abs(four.w - 160) < 0.5 and math.abs(five.w - 160) < 0.5,
        "the pair beyond the seam was squeezed under the minimum")
    expect.near(five.x + five.w, 988, 0.5, "the far edge stayed against the container")
end)

-- Clamping a seam already outside its range would move it on the first frame
-- of a drag, with the pointer still where it pressed.
test("a drag on a seam already under the minimum does not jump", function()
    local opts = floored(12, 600, 96)
    local function lopsided()
        local tree = quad(GAPPED)
        tree:drag_seam(1, "right", 340, 150, GAPPED)
        return tree
    end
    local before = every(lopsided(), opts)
    local one = rect_of(lopsided(), 1, opts)
    expect.near(one.x + one.w, 340, 0.5, "the premise: window 1's right edge at 340")
    for _, case in ipairs({ { 340, "handed its own edge" }, { 300, "asked to shrink further" } }) do
        local tree = lopsided()
        tree:drag_seam(1, "right", case[1], 150, opts)
        expect.same(every(tree, opts), before, case[2] .. ", the seam moved")
    end
    local tree = lopsided()
    tree:drag_seam(1, "right", 350, 150, opts)
    one = rect_of(tree, 1, opts)
    expect.near(one.x + one.w, 350, 0.5, "growing the small side was refused")
    tree:drag_seam(1, "right", 900, 150, opts)
    expect.near(rect_of(tree, 2, opts).w, 600, 0.5, "the far side went under its own minimum")
end)

test("a seam with both sides under the minimum holds still", function()
    local opts = floored(12, 600, 96)
    for _, to in ipairs({ 100, 494, 900 }) do
        local tree = quad(GAPPED)
        local before = every(tree, opts)
        tree:drag_seam(1, "right", to, 150, opts)
        expect.same(every(tree, opts), before, "dragged to " .. to .. ", the seam moved")
    end
end)

-- Every tile in these drags stays over 160x96, so the shipped minimum must
-- change none of them by a pixel.
test("the shipped minimum leaves every existing drag where it was", function()
    for _, case in ipairs({
        { 1, "right", 340, 150 },
        { 2, "left", 640, 150 },
        { 1, "bottom", 250, 200 },
        { 3, "top", 250, 420 },
        { 1, "right", 700, 150 },
        { 2, "left", 700, 150 },
    }) do
        local function run(opts)
            local tree = quad(opts)
            tree:drag_seam(case[1], case[2], case[3], case[4], opts)
            local x, y = own_edge(rect_of(tree, case[1], opts), case[2])
            tree:drag_seam(case[1], case[2], x, y, opts)
            return every(tree, opts)
        end
        local without, with = run(GAPPED), run(SHIPPED)
        for index, box in ipairs(without) do
            expect.same(box.id, with[index].id)
            expect.close(with[index], box, "window " .. case[1] .. "'s " .. case[2]
                .. " drag came out differently under the shipped minimum")
        end
    end
end)

test("the keyboard does not shrink a tile under the minimum", function()
    local opts = floored(12, 400, 96)
    local tree = quad(GAPPED)
    for _ = 1, 5 do
        tree:resize(1, "width", -0.05, opts)
    end
    expect.near(rect_of(tree, 1, opts).w, 400, 0.5, "shrunk past 400")

    opts = floored(12, 600, 96)
    tree = quad(GAPPED)
    tree:drag_seam(1, "right", 340, 150, GAPPED)
    local before = every(tree, opts)
    tree:resize(1, "width", -0.05, opts)
    expect.same(every(tree, opts), before, "a press shrank a tile already under the minimum")
    tree:resize(1, "width", 0.05, opts)
    expect.near(rect_of(tree, 2, opts).w, 600, 0.5, "growing window 1 took window 2 under the minimum")
end)

-- `split = 0/0` in a configuration: a seam whose range cannot take in its
-- own ratio. The press and the drag come back, and every tile is a size.
test("a nan split leaves every tile a size", function()
    local broken = options({ gap = 12, split = 0 / 0, minimum = { w = 600, h = 96 } })
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, broken)
    tree:insert(2, 1, nil, nil, broken)
    tree:resize(1, "width", 0.05, broken)
    tree:drag_seam(1, "right", 500, 300, broken)
    for _, box in ipairs(tree:layout(broken)) do
        expect.truthy(box.w == box.w and box.h == box.h and box.w < math.huge and box.h < math.huge,
            "window " .. box.id .. " was laid out at " .. expect.show(box))
    end
end)

-- Each window's own floor (#115), handed in with every call, on a tree built
-- with none: a client says how small it can go after its window has a tile.

local function with_floors(floors, extra, area)
    local out = options(extra, area)
    out.floors = floors
    return out
end

-- 1 on the left and 2 on the right, 500 each.
local function pair()
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, FREE)
    tree:insert(2, 1, nil, nil, FREE)
    return tree
end

test("a window under its floor takes room from its sibling", function()
    local tree = pair()
    local opts = with_floors({ [1] = { w = 700, h = 0 } })
    local one, two = rect_of(tree, 1, opts), rect_of(tree, 2, opts)
    expect.near(one.w, 700, 1e-6, "window 1 was not given its floor")
    expect.truthy(math.abs(two.x - 700) < 1e-6 and math.abs(two.w - 300) < 1e-6,
        "window 2 did not give up the difference")
    expect.falsy(one.cramped or two.cramped)
end)

-- A sibling gives down to its own floor or the minimum, whichever is larger.
-- A window laid out under its floor is cramped; one at the minimum is not,
-- and nor is a tile under the minimum with no floor of its own.
test("a sibling gives no more than it holds", function()
    local tree = pair()
    local opts = with_floors({ [1] = { w = 800, h = 0 } }, { minimum = { w = 400, h = 0 } })
    local one, two = rect_of(tree, 1, opts), rect_of(tree, 2, opts)
    expect.near(one.w, 600, 1e-6)
    expect.near(two.w, 400, 1e-6, "window 2 went under the minimum")
    expect.truthy(one.cramped, "a window laid out under its floor is cramped")
    expect.falsy(two.cramped, "a window at the minimum is not")

    local small = dwindle.new()
    small:insert(1, nil, nil, nil, FREE)
    small:insert(2, 1, 900, 300, FREE)
    small:drag_seam(1, "right", 900, 300, FREE)
    local under = with_floors({ [1] = { w = 10, h = 0 } }, { minimum = { w = 400, h = 0 } })
    local narrow = rect_of(small, 2, under)
    expect.near(narrow.w, 100, 1e-6, "the premise: window 2 is under the minimum")
    expect.falsy(narrow.cramped, "a tile under the minimum with no floor of its own is cramped")
end)

-- The ratio the split holds is left alone, so a floor that goes away hands
-- the room back.
test("a window whose floor goes away hands the room back", function()
    local tree = pair()
    expect.near(rect_of(tree, 1, with_floors({ [1] = { w = 700, h = 0 } })).w, 700, 1e-6)
    expect.near(rect_of(tree, 1, with_floors({ [2] = { w = 0, h = 100 } })).w, 500, 1e-6,
        "the seam stayed where the floor pushed it")
end)

test("two windows that cannot both fit keep the users seam", function()
    local tree = pair()
    tree:drag_seam(1, "right", 450, 300, FREE)
    local opts = with_floors({ [1] = { w = 700, h = 0 }, [2] = { w = 700, h = 0 } })
    local one, two = rect_of(tree, 1, opts), rect_of(tree, 2, opts)
    expect.near(one.w, 450, 1e-6)
    expect.near(two.w, 550, 1e-6)
    expect.truthy(one.cramped and two.cramped)
end)

-- On 1600x600 at a 100-wide minimum: 1 on the left, 2 and 3 side by side on
-- the right. Window 3 asks for 750; the right side needs 850 in all, so the
-- root seam moves 50, and window 2 gives 350 of its 400. Then cut the other
-- way, 3 under 2: a column is as narrow as its widest floor.
test("a floor deep in a subtree takes room across the root", function()
    local wide = { x = 0, y = 0, w = 1600, h = 600 }
    local free = options(nil, wide)
    local tree = dwindle.new()
    tree:insert(1, nil, nil, nil, free)
    tree:insert(2, 1, nil, nil, free)
    tree:insert(3, 2, nil, nil, free)
    expect.near(rect_of(tree, 3, free).w, 400, 1e-6, "the premise: 2 and 3 side by side")
    local opts = with_floors({ [3] = { w = 750, h = 0 } }, { minimum = { w = 100, h = 0 } }, wide)
    expect.near(rect_of(tree, 1, opts).w, 750, 1e-6, "the root seam")
    expect.near(rect_of(tree, 2, opts).w, 100, 1e-6, "window 2 held its minimum")
    expect.near(rect_of(tree, 3, opts).w, 750, 1e-6, "window 3 has its floor")

    local column = dwindle.new()
    column:insert(1, nil, nil, nil, FREE)
    column:insert(2, 1, nil, nil, FREE)
    column:insert(3, 2, nil, nil, FREE)
    expect.near(rect_of(column, 3).h, 300, 1e-6, "the premise: 3 under 2")
    opts = with_floors({ [3] = { w = 900, h = 0 } }, { minimum = { w = 100, h = 0 } })
    expect.near(rect_of(column, 1, opts).w, 100, 1e-6)
    expect.near(rect_of(column, 2, opts).w, 900, 1e-6, "the column is as wide as its widest floor")
    expect.near(rect_of(column, 3, opts).w, 900, 1e-6, "the column is as wide as its widest floor")
end)

-- The new window's floor is read before it is in the tree.
test("a split that would put a window under its floor is refused", function()
    local minimum = { minimum = { w = 400, h = 400 } }
    for _, owner in ipairs({ 1, 2 }) do
        local tree = dwindle.new()
        tree:insert(1, nil, nil, nil, options(minimum))
        local control = clone(tree)
        expect.truthy(control:insert_fitting(2, nil, 900, 300, options(minimum)),
            "the premise: without a floor there is room")
        local opts = with_floors({ [owner] = { w = 700, h = 0 } }, minimum)
        expect.falsy(tree:insert_fitting(2, nil, 900, 300, opts), "a floor on window " .. owner .. " was not read")
        expect.falsy(tree:insert_largest(2, opts), "a floor on window " .. owner .. " was not read")
        expect.same(tree:windows(), { 1 })
    end
end)

-- The seam itself stops, not only the picture of it: with the floor gone, it
-- is where the drag stopped it.
test("a seam stops at a neighbours own floor", function()
    local tree = pair()
    tree:drag_seam(1, "right", 900, 300, with_floors({ [2] = { w = 600, h = 0 } }))
    expect.near(rect_of(tree, 2, with_floors({ [2] = { w = 600, h = 0 } })).w, 600, 1e-6,
        "window 2 went under its floor")
    expect.near(rect_of(tree, 1).w, 400, 1e-6, "the seam went on past where it was drawn")

    tree = pair()
    local opts = with_floors({ [1] = { w = 600, h = 0 } })
    tree:drag_seam(1, "right", 100, 300, opts)
    expect.near(rect_of(tree, 1, opts).w, 600, 1e-6)
end)

test("a press stops at a windows own floor", function()
    local tree = pair()
    for _ = 1, 5 do
        tree:resize(1, "width", -0.05, with_floors({ [1] = { w = 600, h = 0 } }))
    end
    expect.near(rect_of(tree, 1).w, 600, 1e-6)
end)

-- The split holds 0.5 and window 1's floor draws it at 0.7; a press grows the
-- window from what is drawn.
test("a press beside a floor moves the seam from where it is drawn", function()
    local tree = pair()
    local opts = with_floors({ [1] = { w = 700, h = 0 } })
    tree:resize(1, "width", 0.05, opts)
    expect.near(rect_of(tree, 1, opts).w, 750, 1e-6)
end)

-- Kept across a reload.

test("a tree comes back from what was kept as the tree it was", function()
    local tree = quad(GAPPED)
    tree:insert(5, 4, nil, nil, GAPPED)
    tree:remove(2)
    tree:drag_seam(1, "right", 340, 150, GAPPED)
    local back = dwindle.adopt(expect.copy(tree))
    expect.truthy(back, "a tree")
    expect.same(back:windows(), tree:windows())
    expect.same(back:layout(GAPPED), tree:layout(GAPPED))
    back:insert(6, 1, nil, nil, GAPPED)
    expect.same(#back:windows(), #tree:windows() + 1, "a window added after joins the tree")
    expect.same(dwindle.adopt(expect.copy(dwindle.new())):windows(), {})
end)

test("what is not a tree is refused", function()
    local function broken(change)
        local kept = expect.copy(quad())
        change(kept)
        return dwindle.adopt(kept) == nil
    end
    expect.falsy(broken(function() end), "the tree itself is one")
    expect.truthy(broken(function(kept) kept.root = 99 end), "a root that is no node")
    expect.truthy(broken(function(kept) kept.nodes[99] = { id = 77 } end), "a window nothing leads to")
    expect.truthy(broken(function(kept)
        kept.nodes[99] = { axis = "x", ratio = 0.5, kept.root, kept.root }
        kept.root = 99
    end), "a node led to twice")
    expect.truthy(broken(function(kept)
        for _, node in pairs(kept.nodes) do
            if node.axis then
                node.ratio = 0 / 0
            end
        end
    end), "a ratio that is not a number")
    expect.truthy(broken(function(kept)
        for _, node in pairs(kept.nodes) do
            if node.axis then
                node[2] = nil
            end
        end
    end), "a split with one side")
    expect.truthy(dwindle.adopt(3) == nil)
end)

return spec
