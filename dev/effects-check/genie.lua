-- effects-check's `genie` section: one window at a known rect, and a binding,
-- super+g, that holds it half way through the shipped genie, pulled down into
-- a strip below the monitor. Run twice, once with SOLIUM_GEOMETRY_ORACLE=genie
-- (a debug build's knob: the grid is the Rust genie's) and once without (the
-- grid is the `genie` folder's), the two frames must agree.
--
-- GENIE_SPREAD changes the spread of this run alone, so the check can be seen
-- to fail: 1.5 against the oracle's 1.4.
sol.pane("none")
local WINDOW = { x = 420, y = 190, w = 520, h = 360 }
sol.on("open", function(id)
    sol.animate({ duration = 0 })
    sol.place(id, WINDOW)
end)

sol.bind("super+g", function()
    for _, window in ipairs(sol.windows()) do
        sol.animate({ duration = 0 })
        sol.present(window.id, {
            deform = {
                effect = "genie",
                progress = 0.5,
                axis = "down",
                spread = tonumber(os.getenv("GENIE_SPREAD") or "1.4"),
                to = { x = 600, y = 1000, w = 120, h = 24 },
            },
        })
    end
end)
