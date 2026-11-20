-- Dwindle: a binary tree of tiles, as Hyprland tiles.
--
-- A new window splits one particular tile -- the one named, else the one
-- under the pointer, else the one nearest it -- across whichever side of that
-- tile is longer, and lands on the half the pointer is in. Two windows opened
-- in a different order, or with the pointer somewhere else, give different
-- arrangements, so no function of the window list can stand in for the tree.
-- Closing a window hands its tile to its sibling; dragging a seam moves that
-- one split and nothing beyond it.
--
-- Arithmetic on rectangles and nothing else: no `sol`, no windows, no
-- compositor. `tiling.lua` decides which tree a window goes in and when; this
-- decides where in it. Copy it into ~/.config/solium/layouts/ to change how
-- tiles are cut.
--
-- A tree is plain data, so `sol.keep` carries it across a reload as it is:
--
--     { root = 3, serial = 4, nodes = {
--         [1] = { id = 7 },                         -- a window's tile
--         [2] = { id = 9 },
--         [3] = { axis = "x", ratio = 0.5, 1, 2 },  -- a split and its two sides
--     } }
--
-- A split on "x" puts its sides next to each other and divides the width; on
-- "y" it stacks them and divides the height. `ratio` is the first side's
-- share. Node numbers are never reused, so a number names one node for the
-- tree's whole life.
--
-- Every call that measures takes `options`: the area (`x`, `y`, `w`, `h`),
-- `gap`, `split` (where a new split falls), `minimum` (the smallest a tile may
-- be, `{ w, h }`, frame included) and `floors` (each window's own smallest
-- size, `{ [id] = { w, h } }`). See `config.tiling` in config.lua.
--
-- Behaviour modelled on Hyprland's dwindle layout, with no Hyprland code.

local dwindle = {}

local Tree = {}
Tree.__index = Tree

-- How far under a size a tile may come out and still count as at it: a
-- split's halves have been through a division or two, and a tile laid out at
-- exactly the minimum can land a rounding error short of it.
local SLACK = 1e-6

-- The most and least of two numbers, where a NaN gives way to the other:
-- Lua's `math.max(0/0, 1)` is NaN, and a NaN in a tile's size would spread to
-- every tile after it.
local function most(a, b)
    if a ~= a then return b end
    if b ~= b then return a end
    return a > b and a or b
end

local function least(a, b)
    if a ~= a then return b end
    if b ~= b then return a end
    return a < b and a or b
end

-- `value` held to `low..high`; a NaN stays NaN, for `within` to catch.
local function clamp(value, low, high)
    if value < low then return low end
    if value > high then return high end
    return value
end

-- `value` held to `low..high`, or `otherwise` when that range is empty: a
-- seam with nowhere to go stays where it is.
local function within(value, low, high, otherwise)
    if low <= high then
        return clamp(value, low, high)
    end
    return otherwise
end

-- A size as a size: a number of nothing or more that is not infinite, and
-- anything else as nothing.
local function size(value)
    if type(value) == "number" and value >= 0 and value < math.huge then
        return value
    end
    return 0
end

local function number(value, otherwise)
    if type(value) == "number" then
        return value
    end
    return otherwise
end

-- Each window's floor, by id, as `options.floors` gives them. An entry under
-- something that is not a window's id, or that is not a table, is no floor.
local function floors_from(given)
    local out = {}
    if type(given) ~= "table" then
        return out
    end
    for id, floor in pairs(given) do
        if math.type(id) == "integer" and id >= 0 and type(floor) == "table" then
            out[id] = { w = size(floor.w), h = size(floor.h) }
        end
    end
    return out
end

-- What a call was handed, read once.
local function read(options)
    options = options or {}
    local minimum = type(options.minimum) == "table" and options.minimum or {}
    local floors = floors_from(options.floors)
    return {
        area = {
            x = number(options.x, 0),
            y = number(options.y, 0),
            w = number(options.w, 0),
            h = number(options.h, 0),
        },
        gap = number(options.gap, 12),
        split = number(options.split, 0.5),
        minimum = { w = size(minimum.w), h = size(minimum.h) },
        floors = floors,
        has_floors = next(floors) ~= nil,
    }
end

local function rect(x, y, w, h)
    return { x = x, y = y, w = w, h = h }
end

-- Shrunk by `amount` on every side, never to nothing.
local function inset(area, amount)
    return rect(
        area.x + amount,
        area.y + amount,
        most(area.w - amount * 2, 1),
        most(area.h - amount * 2, 1)
    )
end

local function contains(box, x, y)
    return x >= box.x and y >= box.y and x < box.x + box.w and y < box.y + box.h
end

-- The split a tile gets when nothing else decides: across its longer side,
-- so the cut follows the shape of the space being divided.
local function longer(box)
    return box.w >= box.h and "x" or "y"
end

local function across(axis)
    return axis == "x" and "y" or "x"
end

-- The side of a size or a rectangle that a split on `axis` divides.
local function along(axis, sized)
    return axis == "x" and sized.w or sized.h
end

-- A split's two sides, `gap` apart. The first takes `ratio` of what is left
-- once the gap is taken out, and neither side is ever less than a pixel.
local function cut(box, axis, ratio, gap)
    ratio = clamp(ratio, 0.05, 0.95)
    if axis == "x" then
        local first = most((box.w - gap) * ratio, 1)
        local second = most(box.w - gap - first, 1)
        return rect(box.x, box.y, first, box.h), rect(box.x + first + gap, box.y, second, box.h)
    end
    local first = most((box.h - gap) * ratio, 1)
    local second = most(box.h - gap - first, 1)
    return rect(box.x, box.y, box.w, first), rect(box.x, box.y + first + gap, box.w, second)
end

-- Whether a window added to `box` on `axis` takes the second side: the half
-- the pointer is in, and the far one when there is no pointer.
local function goes_second(at, box, axis)
    if not at then
        return true
    end
    if axis == "x" then
        return at.x >= box.x + box.w / 2
    end
    return at.y >= box.y + box.h / 2
end

-- What a window with `floor` asks of the room on `axis`: its own floor to be
-- given (`pull`), and the most it can be taken down to when a neighbour pulls
-- (`hold`), which is its floor or the minimum, whichever is larger.
local function need_of(floor, axis, settings)
    local pull = along(axis, floor)
    return { pull = pull, hold = most(pull, along(axis, settings.minimum)) }
end

-- Whether `half` is at least a window's `floor` and the minimum, both ways.
local function holds(half, floor, settings)
    local function larger(own, minimum)
        if own > minimum then return own end
        return minimum
    end
    return half.w + SLACK >= larger(floor.w, settings.minimum.w)
        and half.h + SLACK >= larger(floor.h, settings.minimum.h)
end

-- The ratio a split holding `ratio` is drawn at, when its sides ask `first`
-- and `second` of the room. Its own, unless exactly one side is under its
-- floor: then the seam moves towards the other side by the shortfall, or by
-- as much as the other side has above what it holds, whichever is less. With
-- both sides short the seam stays where the user put it, since moving it
-- would take from one window short of room to give to another.
local function shared(axis, ratio, first, second, box, settings)
    if first.pull <= 0 and second.pull <= 0 then
        return ratio
    end
    local span = most(along(axis, box) - settings.gap, 1)
    local one = span * clamp(ratio, 0.05, 0.95)
    local two = span - one
    local short_one, short_two = first.pull - one, second.pull - two
    if short_one > SLACK and short_two <= SLACK then
        one = one + least(short_one, most(two - second.hold, 0))
    elseif short_two > SLACK and short_one <= SLACK then
        one = one - least(short_two, most(one - first.hold, 0))
    else
        return ratio
    end
    return one / span
end

-- The sides of a window, each with the axis its seam is cut on and which side
-- of that seam's split the window is on when the seam is beside it. A
-- window's left edge is bounded by a split it is the second side of; its
-- right edge by one it is the first side of.
local EDGES = {
    left = { axis = "x", side = 2 },
    right = { axis = "x", side = 1 },
    top = { axis = "y", side = 2 },
    bottom = { axis = "y", side = 1 },
}

-- An empty tree.
function dwindle.new()
    return setmetatable({ nodes = {}, serial = 0 }, Tree)
end

-- A tree `sol.keep` carried across a reload, made a tree again; or nil when
-- what was kept is not one: a split leading to nothing, a node reached twice
-- or by nothing, a ratio that is not a number.
function dwindle.adopt(kept)
    if type(kept) ~= "table" or type(kept.nodes) ~= "table" then
        return nil
    end
    -- A whole number, which a float that is one may stand for.
    local function whole(value)
        return type(value) == "number" and math.tointeger(value) or nil
    end
    if kept.root ~= nil then
        kept.root = whole(kept.root)
        if kept.root == nil then
            return nil
        end
    end
    local reached = {}
    local ahead = { kept.root }
    while #ahead > 0 do
        local at = table.remove(ahead)
        local node = kept.nodes[at]
        if type(node) ~= "table" or reached[at] then
            return nil
        end
        reached[at] = true
        if node.id ~= nil then
            node.id = whole(node.id)
            if node.id == nil then
                return nil
            end
        elseif (node.axis == "x" or node.axis == "y")
            and type(node.ratio) == "number" and node.ratio == node.ratio
            and node.ratio > -math.huge and node.ratio < math.huge
            and whole(node[1]) and whole(node[2]) then
            node[1], node[2] = whole(node[1]), whole(node[2])
            ahead[#ahead + 1] = node[1]
            ahead[#ahead + 1] = node[2]
        else
            return nil
        end
    end
    local serial = 0
    for at in pairs(kept.nodes) do
        if not reached[at] or math.type(at) ~= "integer" then
            return nil
        end
        serial = math.max(serial, at)
    end
    kept.serial = math.max(number(kept.serial, 0), serial)
    return setmetatable(kept, Tree)
end

function Tree:is_empty()
    return self.root == nil
end

-- The node holding window `id`'s tile.
function Tree:leaf(id)
    for at, node in pairs(self.nodes) do
        if node.id == id then
            return at
        end
    end
    return nil
end

function Tree:contains(id)
    return self:leaf(id) ~= nil
end

-- The split a node hangs from.
function Tree:parent(child)
    for at, node in pairs(self.nodes) do
        if node.id == nil and (node[1] == child or node[2] == child) then
            return at
        end
    end
    return nil
end

function Tree:push(node)
    self.serial = self.serial + 1
    self.nodes[self.serial] = node
    return self.serial
end

-- Put `with` where `what` hung from `parent`, or at the root.
function Tree:attach(parent, what, with)
    if parent == nil then
        if self.root == what then
            self.root = with
        end
        return
    end
    local split = self.nodes[parent]
    for side = 1, 2 do
        if split[side] == what then
            split[side] = with
        end
    end
end

-- Every window in the tree, first sides before second.
function Tree:windows()
    local out = {}
    local function walk(at)
        local node = self.nodes[at]
        if node == nil then
            return
        end
        if node.id ~= nil then
            out[#out + 1] = node.id
        else
            walk(node[1])
            walk(node[2])
        end
    end
    if self.root then
        walk(self.root)
    end
    return out
end

-- Window `id`'s own floor.
local function floor_of(settings, id)
    return settings.floors[id] or { w = 0, h = 0 }
end

-- What the subtree at `at` asks of the room on `axis`. Split on `axis`, its
-- sides share that room, so it needs both and the gap -- and once anything in
-- it pulls, every window in it at what that window gives down to, since that
-- is where the pulling one's room comes from. Split the other way, each side
-- has the whole extent, so it needs the larger of the two.
function Tree:need(at, axis, settings)
    local node = self.nodes[at]
    if node == nil then
        return { pull = 0, hold = 0 }
    end
    if node.id ~= nil then
        return need_of(floor_of(settings, node.id), axis, settings)
    end
    local first = self:need(node[1], axis, settings)
    local second = self:need(node[2], axis, settings)
    if node.axis == axis then
        local hold = 0
        if first.hold > 0 or second.hold > 0 then
            hold = first.hold + settings.gap + second.hold
        end
        local pull = 0
        if first.pull > 0 or second.pull > 0 then
            pull = hold
        end
        return { pull = pull, hold = hold }
    end
    return { pull = most(first.pull, second.pull), hold = most(first.hold, second.hold) }
end

-- The ratio the split at `at` is drawn at in `box`: its own, or the one that
-- gives a side under its floor its floor. With no floors at all, its own.
function Tree:share(at, box, settings)
    local split = self.nodes[at]
    if not settings.has_floors then
        return split.ratio
    end
    return shared(
        split.axis,
        split.ratio,
        self:need(split[1], split.axis, settings),
        self:need(split[2], split.axis, settings),
        box,
        settings
    )
end

-- Where every window goes, in tree order: `{ id, x, y, w, h }` each.
function Tree:boxes(settings)
    local out = {}
    local function place(at, box)
        local node = self.nodes[at]
        if node == nil then
            return
        end
        if node.id ~= nil then
            out[#out + 1] = { id = node.id, x = box.x, y = box.y, w = box.w, h = box.h }
            return
        end
        local first, second = cut(box, node.axis, self:share(at, box, settings), settings.gap)
        place(node[1], first)
        place(node[2], second)
    end
    if self.root then
        place(self.root, inset(settings.area, settings.gap))
    end
    return out
end

-- The rectangle a node takes up, splits included.
function Tree:node_box(wanted, settings)
    local function find(at, box)
        if at == wanted then
            return box
        end
        local node = self.nodes[at]
        if node == nil or node.id ~= nil then
            return nil
        end
        local first, second = cut(box, node.axis, self:share(at, box, settings), settings.gap)
        return find(node[1], first) or find(node[2], second)
    end
    if not self.root then
        return nil
    end
    return find(self.root, inset(settings.area, settings.gap))
end

-- Where every window goes: `{ id, x, y, w, h }` each, in tree order, with
-- `cramped = true` on a tile smaller than its window's own floor -- a window
-- whose application will draw more than it was given.
function Tree:layout(options)
    local settings = read(options)
    local out = self:boxes(settings)
    for _, box in ipairs(out) do
        local floor = floor_of(settings, box.id)
        if box.w + SLACK < floor.w or box.h + SLACK < floor.h then
            box.cramped = true
        end
    end
    return out
end

-- The tile a new window splits: the one named, else the one under the
-- pointer, else the one nearest it. A window is never its own target: a new
-- window can be under the pointer before it is in the tree, and splitting it
-- by itself would lose it. The nearest tile and not the whole area, since
-- splitting the area makes every new window another full-height column.
function Tree:target(id, target, at, boxes)
    if target ~= nil and target ~= id then
        local leaf = self:leaf(target)
        if leaf then
            return leaf
        end
    end
    if at then
        for _, box in ipairs(boxes) do
            if contains(box, at.x, at.y) then
                return self:leaf(box.id)
            end
        end
    end
    local point = at or (boxes[1] and { x = boxes[1].x, y = boxes[1].y }) or { x = 0, y = 0 }
    local nearest, distance
    for _, box in ipairs(boxes) do
        local dx = box.x + box.w / 2 - point.x
        local dy = box.y + box.h / 2 - point.y
        local this = dx * dx + dy * dy
        if distance == nil or this < distance then
            nearest, distance = box, this
        end
    end
    return nearest and self:leaf(nearest.id) or nil
end

-- The rectangle node `at` has now: its window's tile, or the whole area.
local function box_of(tree, at, boxes, area)
    local node = tree.nodes[at]
    if node and node.id ~= nil then
        for _, box in ipairs(boxes) do
            if box.id == node.id then
                return box
            end
        end
    end
    return area
end

-- Hang window `id` beside node `target`, which takes up `box`, on `axis`.
-- The one place a node is made for a window that is not the first, so a
-- caller that decided against a split has made nothing.
function Tree:split(id, target, box, axis, at, settings)
    local fresh = self:push({ id = id })
    local first, second = fresh, target
    if goes_second(at, box, axis) then
        first, second = target, fresh
    end
    -- Where the target hung, found before the split that will hold it exists.
    local hung = self:parent(target)
    local split = self:push({ axis = axis, ratio = clamp(settings.split, 0.05, 0.95), first, second })
    self:attach(hung, target, split)
end

local function point(x, y)
    if x ~= nil and y ~= nil then
        return { x = x, y = y }
    end
    return nil
end

-- Add window `id` by splitting `target`'s tile, or the tile under the pointer
-- at `x`, `y`; both may be nil. Splits however small the halves come out:
-- the last resort, `"allow"` in `config.tiling.overflow`.
function Tree:insert(id, target, x, y, options)
    if self:contains(id) then
        return
    end
    if not self.root then
        self.root = self:push({ id = id })
        return
    end
    local settings = read(options)
    local at = point(x, y)
    local boxes = self:boxes(settings)
    local split = self:target(id, target, at, boxes) or self.root
    local box = box_of(self, split, boxes, settings.area)
    self:split(id, split, box, longer(box), at, settings)
end

-- Which way the tile `box` of node `old` can be split for window `new`, with
-- neither half under the minimum or its own window's floor: across its longer
-- side if that has room, else across the other, else neither. Measured with
-- `cut` itself at the ratio the layout will draw, so "has room" means what
-- will be drawn.
function Tree:room_in(box, old, new, at, settings)
    local node = self.nodes[old]
    local old_floor = (node and node.id ~= nil) and floor_of(settings, node.id) or { w = 0, h = 0 }
    local new_floor = floor_of(settings, new)
    local first_axis = longer(box)
    for _, axis in ipairs({ first_axis, across(first_axis) }) do
        local first, second = new_floor, old_floor
        if goes_second(at, box, axis) then
            first, second = old_floor, new_floor
        end
        local ratio = shared(
            axis,
            settings.split,
            need_of(first, axis, settings),
            need_of(second, axis, settings),
            box,
            settings
        )
        local one, two = cut(box, axis, ratio, settings.gap)
        if holds(one, first, settings) and holds(two, second, settings) then
            return axis
        end
    end
    return nil
end

-- `insert`, unless that would make a tile smaller than the minimum or put
-- either window under its own floor: then the other axis is tried, and failing
-- that nothing is done. The same tile and the same side as `insert`. Whether
-- the window is in the tree now; false leaves the tree as it was.
function Tree:insert_fitting(id, target, x, y, options)
    if self:contains(id) then
        return true
    end
    if not self.root then
        self.root = self:push({ id = id })
        return true
    end
    local settings = read(options)
    local at = point(x, y)
    local boxes = self:boxes(settings)
    local split = self:target(id, target, at, boxes) or self.root
    local box = box_of(self, split, boxes, settings.area)
    local axis = self:room_in(box, split, id, at, settings)
    if not axis then
        return false
    end
    self:split(id, split, box, axis, at, settings)
    return true
end

-- Split the largest tile that has room for window `id` at the minimum and
-- the floors: `"largest"` in `config.tiling.overflow`. Largest by area among
-- those with room, ties to the earlier in tree order; the new window takes
-- the far side. Whether the window is in the tree now.
function Tree:insert_largest(id, options)
    if self:contains(id) then
        return true
    end
    if not self.root then
        self.root = self:push({ id = id })
        return true
    end
    local settings = read(options)
    local boxes = self:boxes(settings)
    for order, box in ipairs(boxes) do
        box.order = order
    end
    table.sort(boxes, function(a, b)
        local area_a, area_b = a.w * a.h, b.w * b.h
        if area_a ~= area_b then
            return area_a > area_b
        end
        return a.order < b.order
    end)
    for _, box in ipairs(boxes) do
        local leaf = self:leaf(box.id)
        local axis = leaf and self:room_in(box, leaf, id, nil, settings)
        if axis then
            self:split(id, leaf, box, axis, nil, settings)
            return true
        end
    end
    return false
end

-- Trade the tiles of windows `a` and `b`, and nothing else: every split keeps
-- its axis and ratio. Whether both were in the tree.
function Tree:swap(a, b)
    local first, second = self:leaf(a), self:leaf(b)
    if not first or not second then
        return false
    end
    self.nodes[first] = { id = b }
    self.nodes[second] = { id = a }
    return true
end

-- Take window `id` out; its sibling takes its tile, and nothing else moves.
function Tree:remove(id)
    local leaf = self:leaf(id)
    if not leaf then
        return
    end
    local parent = self:parent(leaf)
    self.nodes[leaf] = nil
    if not parent then
        self.root = nil
        return
    end
    local split = self.nodes[parent]
    local sibling = split[1] == leaf and split[2] or split[1]
    self:attach(self:parent(parent), parent, sibling)
    self.nodes[parent] = nil
end

-- The split whose seam runs along `edge` of the window at `leaf`: cut on that
-- edge's axis, with this window's subtree on the side that puts the seam
-- under the edge. Up past splits cut the other way and splits this subtree is
-- on the far side of; nil when the window is against the area on that side.
function Tree:seam_beside(leaf, edge)
    local at = leaf
    while true do
        local parent = self:parent(at)
        if not parent then
            return nil
        end
        local split = self.nodes[parent]
        if split.axis == edge.axis and split[edge.side] == at then
            return parent
        end
        at = parent
    end
end

-- The least room on `axis` the subtree at `at` needs for each tile in it to
-- be `smallest` long, its own seams staying where they are.
function Tree:floor(at, axis, smallest, gap)
    local node = self.nodes[at]
    if node == nil then
        return 0
    end
    if node.id ~= nil then
        return smallest
    end
    local first = self:floor(node[1], axis, smallest, gap)
    local second = self:floor(node[2], axis, smallest, gap)
    if node.axis == axis then
        local ratio = clamp(node.ratio, 0.05, 0.95)
        return gap + most(first / ratio, second / (1 - ratio))
    end
    return most(first, second)
end

-- The ratios the seam of the split at `seam`, in `box`, may take: 0.05 to
-- 0.95, narrowed so no tile on either side goes under the minimum, and no
-- window under its own floor. A side already under is held where it is
-- rather than thrown up to it, so the first frame of a drag never jumps: the
-- range always takes in the ratio the seam is drawn at now.
function Tree:room(seam, box, settings)
    local split = self.nodes[seam]
    local current = self:share(seam, box, settings)
    local span = most(along(split.axis, box) - settings.gap, 1)
    local low, high = 0.05, 0.95
    local smallest = along(split.axis, settings.minimum)
    if smallest > 0 then
        low = most(low, self:floor(split[1], split.axis, smallest, settings.gap) / span)
        high = least(high, 1 - self:floor(split[2], split.axis, smallest, settings.gap) / span)
    end
    if settings.has_floors then
        local first = self:need(split[1], split.axis, settings)
        local second = self:need(split[2], split.axis, settings)
        if first.pull > 0 then
            low = most(low, first.pull / span)
        end
        if second.pull > 0 then
            high = least(high, 1 - second.pull / span)
        end
    end
    return most(least(low, current), 0.05), least(most(high, current), 0.95)
end

-- Put the seam beside window `id`'s `edge` ("left", "right", "top" or
-- "bottom") where that edge should come to rest: `edge_x` for a left or right
-- edge, `edge_y` for a top or bottom one, in the coordinates `layout` hands
-- back. A position and not a distance, so the same drag handled twice is the
-- same arrangement. Nothing moves when there is no seam on that side. False
-- for an edge that is none of the four.
function Tree:drag_seam(id, edge, edge_x, edge_y, options)
    local side = EDGES[edge]
    if not side then
        return false
    end
    local leaf = self:leaf(id)
    local seam = leaf and self:seam_beside(leaf, side)
    if not seam then
        return true
    end
    local settings = read(options)
    local box = self:node_box(seam, settings)
    if not box then
        return true
    end
    -- `cut` the other way round: the first side ends at `box.x + (box.w -
    -- gap) * ratio` and the second begins a gap later, so an edge of the
    -- second side is a gap further on for the same ratio.
    local gap = settings.gap
    local leading = side.side == 2 and gap or 0
    local ratio
    if side.axis == "x" then
        ratio = (edge_x - box.x - leading) / most(box.w - gap, 1)
    else
        ratio = (edge_y - box.y - leading) / most(box.h - gap, 1)
    end
    local low, high = self:room(seam, box, settings)
    local split = self.nodes[seam]
    split.ratio = within(ratio, low, high, split.ratio)
    return true
end

-- Grow window `id` on `axis` ("width" or "height") by `by`, a share of the
-- split's room, moving one seam. A key press names no side, so the seam after
-- the window is moved, or the one before it with the sign turned when there
-- is none after: positive `by` grows the window either way. From the ratio
-- the seam is drawn at, and held as a drag is held.
function Tree:resize(id, axis, by, options)
    local leaf = self:leaf(id)
    if not leaf then
        return
    end
    local settings = read(options)
    local trailing, leading = EDGES.bottom, EDGES.top
    if axis == "width" then
        trailing, leading = EDGES.right, EDGES.left
    end
    local seam, towards = self:seam_beside(leaf, trailing), by
    if not seam then
        seam, towards = self:seam_beside(leaf, leading), -by
    end
    if not seam then
        return
    end
    local box = self:node_box(seam, settings) or settings.area
    local low, high = self:room(seam, box, settings)
    local split = self.nodes[seam]
    split.ratio = within(self:share(seam, box, settings) + towards, low, high, split.ratio)
end

return dwindle
