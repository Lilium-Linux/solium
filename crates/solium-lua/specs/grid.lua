-- What `layouts/grid.lua` does.

local grid = require("layouts.grid")
local expect = require("expect")
local spec, test = expect.spec()

local AREA = { x = 0, y = 0, w = 1600, h = 900 }

-- A wide window and a tall one must not come out the same shape.
test("the grid keeps each window its own shape", function()
    local wide = { x = 0, y = 0, w = 1200, h = 400 }
    local tall = { x = 0, y = 0, w = 400, h = 1000 }
    local slots = grid.arrange({ wide, tall }, AREA)
    expect.near(slots[1].w / slots[1].h, wide.w / wide.h, 1e-6)
    expect.near(slots[2].w / slots[2].h, tall.w / tall.h, 1e-6)
end)

test("a small window is not made larger than it is", function()
    local small = { x = 0, y = 0, w = 200, h = 100 }
    local slot = grid.arrange({ small }, AREA)[1]
    expect.same({ slot.w, slot.h }, { 200, 100 })
    expect.near(slot.x + slot.w / 2, 800, 1e-6, "centred in its cell")
end)

-- Five windows are three columns of two rows, and every slot is in the area.
test("the grid is as near square as the count allows", function()
    local sizes = {}
    for index = 1, 5 do
        sizes[index] = { x = 0, y = 0, w = 1600, h = 900 }
    end
    local slots = grid.arrange(sizes, { x = 100, y = 50, w = 1600, h = 900, padding = 10 })
    expect.same(#slots, 5)
    expect.truthy(slots[3].x > slots[2].x and slots[4].y > slots[1].y, "three across, then the next row")
    for _, slot in ipairs(slots) do
        expect.truthy(slot.x >= 100 and slot.y >= 50 and slot.x + slot.w <= 1700 and slot.y + slot.h <= 950,
            "a slot left the area: " .. expect.show(slot))
    end
end)

test("nothing divides by zero on an empty desktop", function()
    expect.same(grid.arrange({}, AREA), {})
    local slot = grid.arrange({ { x = 0, y = 0, w = 0, h = 0 } }, AREA)[1]
    expect.truthy(slot.w == slot.w and slot.h == slot.h, "a window of no size has a slot that is a size")
end)

return spec
