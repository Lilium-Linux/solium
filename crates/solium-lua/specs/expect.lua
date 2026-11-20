-- What a spec asserts with, and how a spec file is written.
--
-- A spec file returns a list of `{ name, run }`; `src/specs.rs` runs every one
-- and fails with each that raised. Each assertion raises with its message and
-- what it was given, so a failure reads without the file open.

local expect = {}

local function show(value, depth)
    depth = depth or 0
    if type(value) ~= "table" then
        return tostring(value)
    end
    if depth > 3 then
        return "{...}"
    end
    local keys = {}
    for key in pairs(value) do
        keys[#keys + 1] = key
    end
    table.sort(keys, function(a, b)
        return tostring(a) < tostring(b)
    end)
    local parts = {}
    for _, key in ipairs(keys) do
        parts[#parts + 1] = tostring(key) .. " = " .. show(value[key], depth + 1)
    end
    return "{ " .. table.concat(parts, ", ") .. " }"
end
expect.show = show

local function fail(message, detail)
    error((message or "expected") .. (detail and (": " .. detail) or ""), 3)
end

function expect.truthy(value, message)
    if not value then
        fail(message, show(value))
    end
end

function expect.falsy(value, message)
    if value then
        fail(message, show(value))
    end
end

-- `actual` within `tolerance` of `wanted`.
function expect.near(actual, wanted, tolerance, message)
    if type(actual) ~= "number" or not (math.abs(actual - wanted) < tolerance) then
        fail(message, string.format("%s, not within %s of %s", tostring(actual), tolerance, wanted))
    end
end

local function same(a, b)
    if type(a) ~= "table" or type(b) ~= "table" then
        return a == b
    end
    for key, value in pairs(a) do
        if not same(value, b[key]) then
            return false
        end
    end
    for key in pairs(b) do
        if a[key] == nil then
            return false
        end
    end
    return true
end
expect.equal = same

-- `actual` and `wanted` are the same value, tables compared key by key.
function expect.same(actual, wanted, message)
    if not same(actual, wanted) then
        fail(message, show(actual) .. " is not " .. show(wanted))
    end
end

-- Two rectangles within half a pixel on every side.
function expect.close(actual, wanted, message)
    for _, side in ipairs({ "x", "y", "w", "h" }) do
        if not (math.abs(actual[side] - wanted[side]) < 0.5) then
            fail(message, show(actual) .. " is not " .. show(wanted))
        end
    end
end

-- A list of specs: `local spec, test = expect.spec()`, then `test(name, run)`
-- for each, and `return spec`.
function expect.spec()
    local spec = {}
    return spec, function(name, run)
        spec[#spec + 1] = { name = name, run = run }
    end
end

-- A deep copy of plain data.
function expect.copy(value)
    if type(value) ~= "table" then
        return value
    end
    local out = {}
    for key, inner in pairs(value) do
        out[key] = expect.copy(inner)
    end
    return out
end

return expect
