-- effects-check's `fail` section: one window at a known rect in the `top`
-- style, and (FAIL=1) a rule in every slot of every part of it naming
-- `fail-compile`, which lints clean and does not compile on any GPU. Its
-- problems are logged, not shown: no overlay, so the frames can be equal.
sol.pane("top")
local WINDOW = { x = 420, y = 190, w = 520, h = 360 }
sol.on("open", function(id)
    sol.animate({ duration = 0 })
    sol.place(id, WINDOW)
end)
sol.on("problems", function()
    for _, row in ipairs(sol.problems()) do
        sol.log(string.format("problem %s:%s: %s", row.file, tostring(row.line), row.message))
    end
end)
if os.getenv("FAIL") then
    local rules = {}
    for _, part in ipairs({ "pane", "client", "popup", "region:titlebar", "layer:bar" }) do
        for _, slot in ipairs({ "behind", "front", "replace" }) do
            rules[#rules + 1] = { match = "*", part = part, slot = slot, effect = "fail-compile" }
        end
    end
    sol.effects({ rules = rules })
end
