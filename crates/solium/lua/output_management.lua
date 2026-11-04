-- A program's arrangement of the monitors (#51): kanshi, wlr-randr, a
-- settings panel.
--
-- What such a program proposes reaches Lua as the action `monitors.apply`
-- from the surface "output-management", every monitor connected as one row of
-- `data.monitors`, in the form `sol.monitors` takes: `name`, `enabled`, and
-- for one that is on `x`, `y`, `mode = { w, h, refresh }`, `scale`,
-- `transform` and, when it is known, `vrr`. The compositor has already read
-- it whole: each mode is one its monitor offers, each scale one
-- `sol.monitors` takes, and something stays on.
--
-- The answer here takes it, keeping which monitor is `primary` from
-- `config.monitors`, which a program has no word for. The program is told it
-- succeeded only when an answer called `sol.monitors`, so an override that
-- refuses (answers without calling it) has the program told it failed. It
-- lasts until the configuration is read again: `config.monitors` is what a
-- reload, and the next session, start from.
--
-- `output_management = false` in `config.lua` stops offering the protocol.
-- See `the_shipped_configuration_answers_monitors_apply_with_sol_monitors`.

local actions = require("actions")
local config = require("config")

local output_management = {}

-- The rows to arrange the monitors by: the program's, each with `primary`
-- as the configuration has it.
function output_management.rows(proposed)
    local primary = {}
    for _, monitor in ipairs(config.monitors or {}) do
        if monitor.primary then
            primary[monitor.name] = true
        end
    end
    local rows = {}
    for _, row in ipairs(proposed or {}) do
        local taken = {}
        for key, value in pairs(row) do
            taken[key] = value
        end
        taken.primary = primary[row.name]
        rows[#rows + 1] = taken
    end
    return rows
end

actions.override("monitors.apply", function(data)
    sol.monitors(output_management.rows(data.monitors))
end)

return output_management
