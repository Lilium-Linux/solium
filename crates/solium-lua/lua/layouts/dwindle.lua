-- Dwindle: a binary tree of tiles, as Hyprland tiles.
--
-- A new window splits one particular tile -- the one named, else the one
-- under the pointer, else the one nearest it -- across whichever side of that
-- tile is longer, and lands on the half the pointer is in. Two windows opened
-- in a different order, or with the pointer somewhere else, give different
-- arrangements, so no function of the window list can stand in for the tree.
-- Closing a window hands its tile to its sibling.
--
-- The tree is the decisions and nothing else: which tile a window splits,
-- which way, and what each split's ratio is. Where the tiles come out is the
-- compositor's arithmetic, which knows each window's application's own least
-- and largest size: `tree:boxes` hands the tree over as boxes for
-- `sol.arrange`, every split a two-way split named by its node's number, and
-- the compositor places the windows, moves a seam whose edge is dragged, and
-- says where it left it (`tree:seam_moved`). `tree:layout` asks the same
-- arithmetic for the answer, through `options.solve` (`sol.solve`), where a
-- decision needs the tiles. No `sol` here: `tiling.lua` decides which tree a
-- window goes in and when. Copy it into ~/.config/solium/layouts/ to change
-- how tiles are cut.
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
-- be, `{ w, h }`, frame included), `solve` (the solver), `client_min` and
-- `client_max` (whether a window's application's own least and largest size
-- count, each a function of its id), and `floors` (each window's least size,
-- `{ [id] = { w, h } }`, standing for what the applications say, where there
-- is no desktop to ask). See `config.tiling` in config.lua.
--
-- Behaviour modelled on Hyprland's dwindle layout, with no Hyprland code.

local dwindle = {}

local Tree = {}
Tree.__index = Tree

-- How far under a size a tile may come out and still count as at it.
local SLACK = 1e-6

-- The larger of two numbers, where a NaN gives way to the other: Lua's
-- `math.max(0/0, 1)` is NaN.
local function most(a, b)
    if a ~= a then return b end
    if b ~= b then return a end
    return a > b and a or b
end

local function clamp(value, low, high)
    if value < low then return low end
    if value > high then return high end
    return value
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

-- What stands for the windows' applications' least sizes, from
-- `options.floors`, as the solver takes it; nil when none were given, so the
-- solver asks the desktop.
local function limits_of(given)
    if type(given) ~= "table" then
        return nil
    end
    local out = {}
    for id, floor in pairs(given) do
        if math.type(id) == "integer" and id >= 0 and type(floor) == "table" then
            out[id] = { min = { w = size(floor.w), h = size(floor.h) } }
        end
    end
    return out
end

local function always() return true end
local function never() return false end

-- What a call was handed, read once.
local function read(options)
    options = options or {}
    local minimum = type(options.minimum) == "table" and options.minimum or {}
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
        limits = limits_of(options.floors),
        client_min = type(options.client_min) == "function" and options.client_min or always,
        client_max = type(options.client_max) == "function" and options.client_max or never,
        solve = options.solve,
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

-- A copy of plain data, tables inside tables included.
local function copy(value)
    if type(value) ~= "table" then
        return value
    end
    local out = {}
    for key, inner in pairs(value) do
        out[key] = copy(inner)
    end
    return out
end

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

-- The tree as boxes for `sol.arrange`, and the area they fill: the work area
-- with the gap off every side. A split is a two-way split, resizable, named
-- by its node's number, its sides sharing by its ratio; a window's tile is
-- held to `minimum` and to its application's own least and largest size as
-- `client_min` and `client_max` say.
function Tree:boxes(options)
    local settings = read(options)
    local boxes = {}
    local function walk(at, parent, share)
        local node = self.nodes[at]
        if node == nil then
            return
        end
        local box = { parent = parent, id = at, share = share }
        boxes[#boxes + 1] = box
        local here = #boxes
        if node.id ~= nil then
            box.window = node.id
            box.min = { w = settings.minimum.w, h = settings.minimum.h }
            box.client_min = settings.client_min(node.id) and true or false
            box.client_max = settings.client_max(node.id) and true or false
            return
        end
        box.split = { axis = node.axis, gap = settings.gap, resizable = true }
        -- A ratio that is not a number (a `split` setting of 0/0) is an even
        -- split rather than a tile of nothing.
        local ratio = node.ratio
        if type(ratio) ~= "number" or ratio ~= ratio then
            ratio = 0.5
        end
        ratio = clamp(ratio, 0.05, 0.95)
        walk(node[1], here, ratio)
        walk(node[2], here, 1 - ratio)
    end
    if self.root then
        walk(self.root, nil, 1)
    end
    return boxes, inset(settings.area, settings.gap)
end

-- Where every window comes out, in tree order: `{ id, x, y, w, h, cramped,
-- tile }` each, the window's rectangle and the tile it is in, by the
-- compositor's own arithmetic.
function Tree:layout(options)
    local settings = read(options)
    if not self.root then
        return {}
    end
    if type(settings.solve) ~= "function" then
        error("dwindle: options.solve is the solver, sol.solve in a configuration", 2)
    end
    local boxes, area = self:boxes(options)
    return settings.solve(area, boxes, settings.limits)
end

-- Every window's tile, in tree order: `{ id, x, y, w, h, cramped }` each.
function Tree:tiles(options)
    local out = {}
    for _, slot in ipairs(self:layout(options)) do
        local tile = slot.tile or slot
        out[#out + 1] = { id = slot.id, x = tile.x, y = tile.y, w = tile.w, h = tile.h, cramped = slot.cramped }
    end
    return out
end

-- The tile a new window splits: the one named, else the one under the
-- pointer, else the one nearest it. A window is never its own target: a new
-- window can be under the pointer before it is in the tree, and splitting it
-- by itself would lose it. The nearest tile and not the whole area, since
-- splitting the area makes every new window another full-height column.
function Tree:target(id, target, at, tiles)
    if target ~= nil and target ~= id then
        local leaf = self:leaf(target)
        if leaf then
            return leaf
        end
    end
    if at then
        for _, box in ipairs(tiles) do
            if contains(box, at.x, at.y) then
                return self:leaf(box.id)
            end
        end
    end
    local point = at or (tiles[1] and { x = tiles[1].x, y = tiles[1].y }) or { x = 0, y = 0 }
    local nearest, distance
    for _, box in ipairs(tiles) do
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
local function box_of(tree, at, tiles, area)
    local node = tree.nodes[at]
    if node and node.id ~= nil then
        for _, box in ipairs(tiles) do
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
    local tiles = self:tiles(options)
    local split = self:target(id, target, at, tiles) or self.root
    local box = box_of(self, split, tiles, settings.area)
    self:split(id, split, box, longer(box), at, settings)
end

-- Whether the tile window `id` comes out with is at least the minimum both
-- ways and its application's own least size.
local function holds(tiles, id, settings)
    for _, tile in ipairs(tiles) do
        if tile.id == id then
            return not tile.cramped
                and tile.w + SLACK >= settings.minimum.w
                and tile.h + SLACK >= settings.minimum.h
        end
    end
    return true
end

-- Which way the tile `box` of node `old` can be split for window `new`, with
-- neither of the two under the minimum or its own application's least size:
-- across its longer side if that has room, else across the other, else
-- neither. Measured by splitting a copy and solving it, so "has room" means
-- what will be drawn.
function Tree:room_in(box, old, new, at, options)
    local settings = read(options)
    local node = self.nodes[old]
    local old_window = node and node.id
    local first_axis = longer(box)
    for _, axis in ipairs({ first_axis, across(first_axis) }) do
        local trial = dwindle.adopt(copy(self))
        trial:split(new, old, box, axis, at, settings)
        local tiles = trial:tiles(options)
        if holds(tiles, new, settings) and (old_window == nil or holds(tiles, old_window, settings)) then
            return axis
        end
    end
    return nil
end

-- `insert`, unless that would make a tile smaller than the minimum or put
-- either window under its own least size: then the other axis is tried, and
-- failing that nothing is done. The same tile and the same side as `insert`.
-- Whether the window is in the tree now; false leaves the tree as it was.
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
    local tiles = self:tiles(options)
    local split = self:target(id, target, at, tiles) or self.root
    local box = box_of(self, split, tiles, settings.area)
    local axis = self:room_in(box, split, id, at, options)
    if not axis then
        return false
    end
    self:split(id, split, box, axis, at, settings)
    return true
end

-- Split the largest tile that has room for window `id` at the minimum and
-- the least sizes: `"largest"` in `config.tiling.overflow`. Largest by area
-- among those with room, ties to the earlier in tree order; the new window
-- takes the far side. Whether the window is in the tree now.
function Tree:insert_largest(id, options)
    if self:contains(id) then
        return true
    end
    if not self.root then
        self.root = self:push({ id = id })
        return true
    end
    local settings = read(options)
    local tiles = self:tiles(options)
    for order, box in ipairs(tiles) do
        box.order = order
    end
    table.sort(tiles, function(a, b)
        local area_a, area_b = a.w * a.h, b.w * b.h
        if area_a ~= area_b then
            return area_a > area_b
        end
        return a.order < b.order
    end)
    for _, box in ipairs(tiles) do
        local leaf = self:leaf(box.id)
        local axis = leaf and self:room_in(box, leaf, id, nil, options)
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

-- Where the compositor left a seam (the `seam` event): the split at node
-- `node` takes its first side's share. False when `node` is not a split of
-- this tree or the share is not one, which changes nothing.
function Tree:seam_moved(node, shares)
    local split = self.nodes[node]
    local share = type(shares) == "table" and shares[1] or nil
    if type(split) ~= "table" or split.id ~= nil or type(share) ~= "number"
        or share ~= share or share <= 0 or share >= 1 then
        return false
    end
    split.ratio = share
    return true
end

return dwindle
