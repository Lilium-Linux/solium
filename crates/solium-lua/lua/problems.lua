-- What is broken in an effect, a rule or the configuration, with its file
-- and line, in the top-right corner of the primary monitor, while anything
-- is. It takes no input, and it goes when a reload leaves nothing broken
-- (`script::tests::the_shipped_overlay_lists_the_problems_on_the_primary_monitor`,
-- and on screen `dev/effects-check.sh overlay`).
--
-- The compositor only publishes the rows (`sol.problems()`); how they look
-- is this file and qml/problems.qml, so a configuration can replace both.
local problems = {}

-- Rows shown at most; the rest are counted.
problems.most = 8

function problems.rows()
    local all, rows = sol.problems(), {}
    for index, row in ipairs(all) do
        if index > problems.most then
            rows[#rows + 1] = { text = string.format("and %d more", #all - problems.most), severity = "error" }
            break
        end
        local at = row.file
        if row.line then
            at = at .. ":" .. row.line
        end
        rows[#rows + 1] = { text = at .. ": " .. row.message, severity = row.severity }
    end
    return rows
end

function problems.area()
    for _, monitor in ipairs(sol.monitors()) do
        if monitor.primary then
            local w = math.min(760, math.floor(monitor.whole.w / 2))
            return { x = monitor.whole.x + monitor.whole.w - w, y = monitor.whole.y, w = w, h = 24 * (problems.most + 1) + 24 }
        end
    end
    return nil
end

function problems.apply()
    local rows, area = problems.rows(), problems.area()
    if #rows == 0 or area == nil then
        sol.surface("problems", false)
        return
    end
    sol.surface("problems", {
        scene = "problems.qml",
        layer = "overlay",
        on = area,
        interactive = false,
        properties = { rows = rows },
    })
end

sol.on("problems", problems.apply)
sol.on("monitors", problems.apply)

return problems
