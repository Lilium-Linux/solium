-- effects-check's `overlay` section: nothing but the overlay and a reload
-- binding, so the only thing that can appear in the top-right corner is it.
require("problems")
sol.pane("none")
sol.bind("super+shift+r", function() sol.reload() end)

-- The `overlay-effect` run: a rule naming an effect whose .frag reads a param
-- it does not have, which the overlay names at the .frag's own line.
if os.getenv("BROKEN_EFFECT") then
    sol.effects({ rules = { { match = "*", part = "client", slot = "behind", effect = "typo" } } })
end

-- Every row in the log, so the section can read what the overlay lists.
sol.on("problems", function()
    for _, row in ipairs(sol.problems()) do
        sol.log(string.format("problem %s:%s: %s", row.file, tostring(row.line), row.message))
    end
end)
