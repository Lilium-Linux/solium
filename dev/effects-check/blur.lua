-- effects-check's `blur` section: a still terminal on the left, titled
-- "effects-check-static", a video player on the right, titled
-- "effects-check-player", and, when BLUR names one of them (`player` or
-- `static`), a rule blurring that window with its own pixels. Matched by
-- title, so it does not depend on what the player calls itself.
sol.pane("none")
local LEFT = { x = 40, y = 60, w = 560, h = 380 }
local RIGHT = { x = 640, y = 60, w = 560, h = 380 }
local opened = 0
sol.on("open", function(id)
    opened = opened + 1
    sol.animate({ duration = 0 })
    sol.place(id, opened == 1 and LEFT or RIGHT)
end)
local blur = os.getenv("BLUR")
if blur then
    local title = blur == "static" and "effects-check-static" or "effects-check-player"
    sol.effects({ rules = { { match = { title = title }, part = "client", slot = "replace",
                              effect = { "blur", source = "self", passes = 3 } } } })
end
