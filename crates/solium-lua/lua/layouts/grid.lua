-- A grid of thumbnails, as near square as the count allows: what the overview
-- shows its windows in.
--
-- Each window keeps its own shape inside its cell and is never made larger
-- than it is, since a small window blown up to fill a cell reads as a
-- different window.

local grid = {}

local function number(value, otherwise)
    if type(value) == "number" then
        return value
    end
    return otherwise
end

-- `size` scaled into `cell`, keeping its shape and never growing, centred.
local function fitted(size, cell)
    if not (size.w > 0 and size.h > 0) then
        return { x = cell.x, y = cell.y, w = cell.w, h = cell.h }
    end
    local scale = math.min(cell.w / size.w, cell.h / size.h, 1)
    local w, h = size.w * scale, size.h * scale
    return { x = cell.x + (cell.w - w) / 2, y = cell.y + (cell.h - h) / 2, w = w, h = h }
end

-- A rectangle for each of `sizes` (each with `w` and `h`), in the same order,
-- in the area `options` names (`x`, `y`, `w`, `h`), each `padding` (24 when
-- not given) inside its cell.
function grid.arrange(sizes, options)
    local count = #sizes
    if count == 0 then
        return {}
    end
    local padding = number(options.padding, 24)
    local x, y = number(options.x, 0), number(options.y, 0)
    local columns = math.max(math.ceil(math.sqrt(count)), 1)
    local rows = math.max(math.ceil(count / columns), 1)
    local cell_w, cell_h = number(options.w, 0) / columns, number(options.h, 0) / rows
    local out = {}
    for index, size in ipairs(sizes) do
        local column, row = (index - 1) % columns, (index - 1) // columns
        out[index] = fitted(size, {
            x = x + column * cell_w + padding,
            y = y + row * cell_h + padding,
            w = math.max(cell_w - padding * 2, 1),
            h = math.max(cell_h - padding * 2, 1),
        })
    end
    return out
end

return grid
