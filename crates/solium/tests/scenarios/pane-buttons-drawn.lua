-- The frame buttons of every shipped style that has them, told apart without
-- a hue: under the pointer each turns its own grey, close lighter than
-- maximise, and shows its glyph, dark on it -- `×` to close, `+` to
-- maximise. At rest neither shows a glyph.
--
-- Played by `scenario::tests::every_scenario_on_the_qt_thread_passes`, in
-- software. Every pane is 400 by 300 outside. A button is a 13-pixel disc,
-- and what is read is the 7 by 7 square in the middle of it, which is inside
-- the disc wherever it lands on the pixel grid: the fill and, while it shows,
-- the glyph, and nothing of the bar around it.

-- Where each style puts its two buttons' centres, by its own `Frame.qml`:
-- `Theme.margin` (12) from the end of the bar, `Theme.gap` (9) apart, each
-- 13 across and centred on the bar. `reveal`'s bar is there only while the
-- pointer is inside the window.
local styles = {
    { pane = "top", client = { 400, 268 }, close = { 381, 16 }, maximize = { 359, 16 } },
    { pane = "bottom", client = { 400, 270 }, close = { 381, 285 }, maximize = { 359, 285 } },
    { pane = "left", client = { 366, 300 }, close = { 17, 281 }, maximize = { 17, 259 } },
    { pane = "reveal", client = { 400, 300 }, close = { 381, 17 }, maximize = { 359, 17 }, inside = true },
    { pane = "reactive", client = { 392, 266 }, close = { 381, 15 }, maximize = { 359, 15 } },
    { pane = "pulse", client = { 400, 262 }, close = { 381, 18 }, maximize = { 359, 18 } },
}

-- The darkest and the lightest of the 7 by 7 square around `at`, as a grey,
-- and whether every pixel of it is one: red, green and blue within 2 of one
-- another. Premultiplied, as the layer holds them; `reveal` draws its bar at
-- 0.96, which takes a few levels off both ends and changes neither verdict.
local function square(world, at)
    local darkest, lightest, grey = 255, 0, true
    for y = at[2] - 3, at[2] + 3 do
        for x = at[1] - 3, at[1] + 3 do
            local r, g, b, a = world.pixel("bar", x, y)
            assert(a >= 200, string.format("(%d, %d) is not on the button: alpha %d", x, y, a))
            if math.abs(r - g) > 2 or math.abs(g - b) > 2 then
                grey = false
            end
            local level = math.floor((r + g + b) / 3)
            darkest = math.min(darkest, level)
            lightest = math.max(lightest, level)
        end
    end
    return darkest, lightest, grey
end

-- A button with its glyph showing: a light fill and dark ink on it, all grey.
-- Answers the fill, for telling the two buttons apart.
local function glyphed(world, name, style, which)
    local darkest, lightest, grey = square(world, style[which])
    local what = string.format("%s's %s under the pointer: darkest %d, lightest %d", name, which, darkest, lightest)
    assert(grey, what .. ", and not all grey")
    assert(lightest >= 128, what .. ": no light fill")
    assert(darkest <= 96, what .. ": no dark glyph on it")
    return lightest
end

-- A button with no glyph: one even fill, all grey.
local function bare(world, name, style, which)
    local darkest, lightest, grey = square(world, style[which])
    local what = string.format("%s's %s: darkest %d, lightest %d", name, which, darkest, lightest)
    assert(grey, what .. ", and not all grey")
    assert(lightest - darkest <= 24, what .. ": a glyph where none should be")
end

local steps = {}
local function step(it)
    steps[#steps + 1] = it
end

for _, style in ipairs(styles) do
    local name = style.pane
    local fills = {}
    step({ pane = name, client = style.client })
    step({ tell = { focused = true, inside = style.inside == true, title = "terminal" } })
    step({ wait = 400 })
    step({
        expect = function(world)
            bare(world, name, style, "close")
            bare(world, name, style, "maximize")
        end,
    })

    step({ point = style.close })
    step({ wait = 300 })
    step({
        expect = function(world)
            fills.close = glyphed(world, name, style, "close")
            bare(world, name, style, "maximize")
        end,
    })

    step({ point = style.maximize })
    step({ wait = 300 })
    step({
        expect = function(world)
            fills.maximize = glyphed(world, name, style, "maximize")
            bare(world, name, style, "close")
            assert(fills.close >= fills.maximize + 24,
                string.format("%s: close (%d) is not told apart from maximise (%d) by its shade",
                    name, fills.close, fills.maximize))
        end,
    })

    -- Off the buttons: both at rest again.
    step({ point = { 200, style.close[2] } })
    step({ wait = 300 })
    step({
        expect = function(world)
            bare(world, name, style, "close")
            bare(world, name, style, "maximize")
        end,
    })
end

return { qt = true, steps = steps }
