-- A scrolling strip: columns side by side, and a view that moves along them.
--
-- The unit is a column, not a window. A column holds a stack of windows that
-- share its width, and its width is a share of the *view*, never of the strip,
-- so opening a tenth window cannot make the first nine thinner: the strip gets
-- longer and the view moves. Where the view sits is measured from the focused
-- column's left edge rather than from the strip's start, so focus and view
-- cannot drift apart when a column before them changes width.
--
-- Arithmetic on rectangles and nothing else, like `dwindle.lua`.
-- `scrolling.lua` decides which strip a window goes in and when; this decides
-- where in it. Copy it into ~/.config/solium/layouts/ to change how a strip
-- behaves.
--
-- A strip is plain data, so `sol.keep` carries it across a reload as it is:
--
--     { columns = { { windows = { 7, 9 }, active = 2, width = 0.5 }, ... },
--       active = 1, offset = -12, widths = { 1/3, 1/2, 2/3 }, preset = 1 }
--
-- `active` is the focused column and each column's `active` its focused
-- window; `offset` is where the view's left edge is from the focused column's;
-- `widths` is what a column cycles through and `preset` where in it the strip
-- is, which is also the width the next column opens at.
--
-- Every call that measures takes `options`: the area (`x`, `y`, `w`, `h`),
-- `gap`, and `floors`, each window's own smallest size (`{ [id] = { w, h } }`).
-- A column is never laid out narrower than the widest floor in it.
--
-- Behaviour modelled on niri's scrolling layout, with no niri code.

local scroller = {}

local Strip = {}
Strip.__index = Strip

-- The widths a column cycles through when the configuration names none.
scroller.WIDTHS = { 1 / 3, 1 / 2, 2 / 3 }

-- The most and least of two numbers, where a NaN gives way to the other.
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

local function number(value, otherwise)
    if type(value) == "number" then
        return value
    end
    return otherwise
end

-- Each window's floor width, by id, as `options.floors` gives them.
local function floors_from(given)
    local out = {}
    if type(given) ~= "table" then
        return out
    end
    for id, floor in pairs(given) do
        local w = type(floor) == "table" and floor.w
        if math.type(id) == "integer" and type(w) == "number" and w >= 0 and w < math.huge then
            out[id] = w
        end
    end
    return out
end

-- What a call was handed, read once: the view is the area less the gap.
local function read(options)
    options = options or {}
    local gap = number(options.gap, 12)
    local x, y = number(options.x, 0), number(options.y, 0)
    local w, h = number(options.w, 0), number(options.h, 0)
    return {
        view = { x = x + gap, y = y + gap, w = most(w - gap * 2, 1), h = most(h - gap * 2, 1) },
        gap = gap,
        floors = floors_from(options.floors),
    }
end

-- The widths `section` (`config.scrolling`) names, and where a new column
-- opens in them. An entry that is not a share of the view -- above nothing
-- and at most all of it -- is dropped, and said to `say`; with none left, the
-- shipped widths. `default_width` counts from 1; one that is not a whole
-- number from 1 is the first, and one past the end the last, each said.
local function widths_from(section, say)
    say = say or function() end
    section = type(section) == "table" and section or {}
    local widths = {}
    if type(section.widths) == "table" then
        for index, share in ipairs(section.widths) do
            if type(share) == "number" and share > 0 and share <= 1 then
                widths[#widths + 1] = share
            else
                say(string.format(
                    "scrolling.widths[%d] is %s, which is not a share of the view (a number "
                        .. "above 0 and at most 1); it is left out",
                    index,
                    tostring(share)
                ))
            end
        end
    end
    local fallback = #widths == 0
    if fallback then
        widths = { table.unpack(scroller.WIDTHS) }
    end
    local start = 1
    local given = section.default_width
    if given ~= nil then
        if type(given) == "number" and given >= 1 and given < math.huge then
            start = math.floor(math.min(given, 1000))
        else
            say(string.format(
                "scrolling.default_width is %s, which is not a place in scrolling.widths "
                    .. "(a whole number from 1); a new column opens at the first width",
                tostring(given)
            ))
        end
    end
    if start > #widths then
        say(string.format(
            "scrolling.default_width is %d, past the %d widths in force%s; a new column "
                .. "opens at the last one",
            start,
            #widths,
            fallback and " (the shipped ones, since scrolling.widths has none)" or ""
        ))
        start = #widths
    end
    return widths, start
end

-- An empty strip with the widths `section` names. `say` is told about each
-- value that cannot be used; see `widths_from`.
function scroller.new(section, say)
    local widths, preset = widths_from(section, say)
    return setmetatable({ columns = {}, active = 1, offset = 0, widths = widths, preset = preset }, Strip)
end

-- A strip `sol.keep` carried across a reload, made a strip again; or nil when
-- what was kept is not one: a column with no window, a focus past the end,
-- no width to cycle through, a number that is not one.
function scroller.adopt(kept)
    local function real(value)
        return type(value) == "number" and value == value and value > -math.huge and value < math.huge
    end
    -- A whole number, which a float that is one may stand for.
    local function whole(value)
        return type(value) == "number" and math.tointeger(value) or nil
    end
    if type(kept) ~= "table" or type(kept.columns) ~= "table" or type(kept.widths) ~= "table"
        or not real(kept.offset) or not whole(kept.active) or not whole(kept.preset) then
        return nil
    end
    kept.active, kept.preset = whole(kept.active), whole(kept.preset)
    if kept.active < 1 or kept.active > math.max(#kept.columns, 1) then
        return nil
    end
    if #kept.widths == 0 or kept.preset < 1 or kept.preset > #kept.widths then
        return nil
    end
    for _, width in ipairs(kept.widths) do
        if not real(width) then
            return nil
        end
    end
    for _, held in ipairs(kept.columns) do
        if type(held) ~= "table" or type(held.windows) ~= "table" or #held.windows == 0
            or not whole(held.active) or not real(held.width) then
            return nil
        end
        held.active = whole(held.active)
        if held.active < 1 or held.active > #held.windows then
            return nil
        end
        for row, id in ipairs(held.windows) do
            if not whole(id) then
                return nil
            end
            held.windows[row] = whole(id)
        end
    end
    return setmetatable(kept, Strip)
end

-- Take the widths `section` names now, and keep the columns, their widths,
-- the focus and the view: what a strip carried across a reload needs, since
-- the widths are configuration and the rest is arrangement.
function Strip:configure(section, say)
    self.widths, self.preset = widths_from(section, say)
end

function Strip:is_empty()
    return #self.columns == 0
end

-- The column window `id` is in, and its place in that column.
function Strip:find(id)
    for index, column in ipairs(self.columns) do
        for row, other in ipairs(column.windows) do
            if other == id then
                return index, row
            end
        end
    end
    return nil
end

function Strip:contains(id)
    return self:find(id) ~= nil
end

-- The focused window, or nil.
function Strip:focused()
    local column = self.columns[self.active]
    return column and column.windows[column.active]
end

-- Every window, column by column and top to bottom: the order a user reads.
function Strip:windows()
    local out = {}
    for _, column in ipairs(self.columns) do
        for _, id in ipairs(column.windows) do
            out[#out + 1] = id
        end
    end
    return out
end

-- How wide column `index` is laid out: its share of the view, or the widest
-- floor in it where that is wider. Everything that measures a column asks
-- this, so where a column is drawn, where the next starts and what the view
-- scrolls to agree.
function Strip:width_of(index, settings)
    local column = self.columns[index]
    if not column then
        return 0
    end
    local widest = 0
    for _, id in ipairs(column.windows) do
        widest = most(widest, settings.floors[id] or 0)
    end
    return most(most(settings.view.w * column.width, 1), widest)
end

-- Where column `index` starts along the strip, from the first column's left.
function Strip:start_of(index, settings)
    local x = 0
    for each = 1, math.min(index - 1, #self.columns) do
        x = x + self:width_of(each, settings) + settings.gap
    end
    return x
end

-- Where the view's left edge goes, from the left edge of a column at
-- `start` that is `width` wide, to show that column when the view's left
-- edge is now at `now`; all along the strip.
--
-- A column wider than the view, or exactly as wide, starts where the view
-- does: nothing shows all of a wider one, and its start is where its windows
-- begin. A column already in view, with a margin of up to a gap either side,
-- does not move the view at all, so focusing what is on screen moves nothing.
-- Otherwise the view moves the shorter way: until the column's left edge is a
-- margin inside the view's left, or its right edge a margin inside the view's
-- right.
local function in_view(now, view_w, start, width, gap)
    if width >= view_w then
        return 0
    end
    local margin = math.max(0, math.min(gap, (view_w - width) / 2))
    local leftmost = start + width + margin - view_w
    local rightmost = start - margin
    if now >= leftmost and now <= rightmost then
        return now - start
    end
    if math.abs(now - rightmost) <= math.abs(now - leftmost) then
        return rightmost - start
    end
    return leftmost - start
end

-- Focus column `index` and bring it into view. The offset is moved first by
-- the distance between the two columns, so it goes on describing the same
-- place on the strip from the new one.
function Strip:focus_column(index, settings)
    if #self.columns == 0 then
        return
    end
    index = math.min(index, #self.columns)
    local from, to = self:start_of(self.active, settings), self:start_of(index, settings)
    self.offset = self.offset + from - to
    self.active = index
    self.offset = in_view(to + self.offset, settings.view.w, to, self:width_of(index, settings), settings.gap)
end

local function column(id, width)
    return { windows = { id }, active = 1, width = width }
end

-- Open window `id` in a column of its own, right of the focused one.
function Strip:insert(id, options)
    if self:contains(id) then
        return
    end
    local at = #self.columns == 0 and 1 or self.active + 1
    table.insert(self.columns, at, column(id, self.widths[self.preset] or scroller.WIDTHS[1]))
    self:focus_column(at, read(options))
end

-- Add window `id` to the bottom of the focused column instead of beside it.
function Strip:insert_into_column(id, options)
    if self:contains(id) then
        return
    end
    local focused = self.columns[self.active]
    if not focused then
        self:insert(id, options)
        return
    end
    focused.windows[#focused.windows + 1] = id
    focused.active = #focused.windows
end

function Strip:remove(id)
    local index, row = self:find(id)
    if not index then
        return
    end
    local held = self.columns[index]
    table.remove(held.windows, row)
    if #held.windows == 0 then
        table.remove(self.columns, index)
        if self.active > #self.columns then
            self.active = math.max(#self.columns, 1)
        end
    else
        held.active = math.min(held.active, #held.windows)
    end
end

-- Focus window `id`, bringing its column into view.
function Strip:focus_window(id, options)
    local index, row = self:find(id)
    if not index then
        return
    end
    self.columns[index].active = row
    self:focus_column(index, read(options))
end

-- Move focus `by` columns, the view with it; it stops at either end.
function Strip:focus_sideways(by, options)
    if #self.columns == 0 then
        return
    end
    self:focus_column(clamp(self.active + by, 1, #self.columns), read(options))
end

-- Move focus `by` windows within the focused column.
function Strip:focus_vertically(by)
    local focused = self.columns[self.active]
    if focused then
        focused.active = clamp(focused.active + by, 1, math.max(#focused.windows, 1))
    end
end

-- Trade the focused window with the one above it (`by` -1) or below (1), focus
-- going with it. Whether it moved, which it does not at either end.
function Strip:move_vertically(by)
    local focused = self.columns[self.active]
    if not focused then
        return false
    end
    local to = focused.active + by
    if to < 1 or to > #focused.windows or to == focused.active then
        return false
    end
    local windows = focused.windows
    windows[focused.active], windows[to] = windows[to], windows[focused.active]
    focused.active = to
    return true
end

-- Trade the focused column with its neighbour, focus going with it. Whether
-- it moved, which it does not at either end of the strip.
function Strip:move_column(by, options)
    if #self.columns < 2 then
        return false
    end
    local to = clamp(self.active + by, 1, #self.columns)
    if to == self.active then
        return false
    end
    local columns = self.columns
    columns[self.active], columns[to] = columns[to], columns[self.active]
    self:focus_column(to, read(options))
    return true
end

-- Pull the next column's focused window into the bottom of this one.
function Strip:consume()
    local next_one = self.columns[self.active + 1]
    local focused = self.columns[self.active]
    if not next_one or not focused or #next_one.windows == 0 then
        return
    end
    local taken = table.remove(next_one.windows, math.min(next_one.active, #next_one.windows))
    if #next_one.windows == 0 then
        table.remove(self.columns, self.active + 1)
    else
        next_one.active = math.min(next_one.active, #next_one.windows)
    end
    focused.windows[#focused.windows + 1] = taken
    focused.active = #focused.windows
end

-- Push the focused window out into a column of its own, as wide as the one it
-- left, right of it.
function Strip:expel(options)
    local focused = self.columns[self.active]
    if not focused or #focused.windows < 2 then
        return
    end
    local taken = table.remove(focused.windows, math.min(focused.active, #focused.windows))
    focused.active = math.min(focused.active, #focused.windows)
    local at = self.active + 1
    table.insert(self.columns, at, column(taken, focused.width))
    self:focus_column(at, read(options))
end

-- Move window `id` into the column window `target` is in, below it: what
-- dropping a window on another means in a strip.
function Strip:move_to_column_of(id, target, options)
    if id == target then
        return
    end
    local to = self:find(target)
    local from, row = self:find(id)
    if not to or not from or from == to then
        return
    end
    local left = self.columns[from]
    table.remove(left.windows, row)
    local emptied = #left.windows == 0
    if emptied then
        table.remove(self.columns, from)
        if to > from then
            to = to - 1
        end
    else
        left.active = math.min(left.active, #left.windows)
    end
    local joined = self.columns[to]
    local below = #joined.windows + 1
    for index, other in ipairs(joined.windows) do
        if other == target then
            below = index + 1
        end
    end
    table.insert(joined.windows, below, id)
    joined.active = below
    self:focus_column(to, read(options))
end

-- Widen or narrow the column window `id` is in by `by`, a share of the view.
-- Between a tenth of the view and all of it, and never under the share its
-- widest floor takes: from the share it is drawn at, so a press always moves
-- what is on screen.
function Strip:widen(id, by, options)
    local index = self:find(id)
    if not index then
        return
    end
    local settings = read(options)
    local widest = 0
    for _, other in ipairs(self.columns[index].windows) do
        widest = most(widest, settings.floors[other] or 0)
    end
    local smallest = clamp(widest / most(settings.view.w, 1), 0.1, 1)
    local held = self.columns[index]
    held.width = clamp(most(held.width, smallest) + by, smallest, 1)
    self:focus_column(index, settings)
end

-- Give the focused column the next of the widths, round to the first again.
function Strip:cycle_width(options)
    self.preset = self.preset % #self.widths + 1
    local focused = self.columns[self.active]
    if focused then
        focused.width = self.widths[self.preset]
    end
    self:focus_column(self.active, read(options))
end

-- Move the view by `delta` without moving focus.
function Strip:scroll_by(delta)
    self.offset = self.offset + delta
end

-- Where every window goes: `{ id, x, y, w, h }` each, column by column. The
-- windows of a column share its height, a gap apart.
function Strip:layout(options)
    local settings = read(options)
    local view, gap = settings.view, settings.gap
    local left = self:start_of(self.active, settings) + self.offset
    local out = {}
    for index, held in ipairs(self.columns) do
        local x = view.x + self:start_of(index, settings) - left
        local width = self:width_of(index, settings)
        local rows = math.max(#held.windows, 1)
        local height = most((view.h - gap * (rows - 1)) / rows, 1)
        for row, id in ipairs(held.windows) do
            out[#out + 1] = { id = id, x = x, y = view.y + (row - 1) * (height + gap), w = width, h = height }
        end
    end
    return out
end

return scroller
