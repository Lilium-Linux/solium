-- Desktop touch bindings. A mobile entry point can omit this module entirely.
-- The compositor supplies hit information and operations; Lua chooses actions.
local config = require("config")
local touch = config.touch or {}

sol.on("touch", function(event)
    if event.kind ~= "down" then return end
    if event.target == "script" then
        if touch.click then
            sol.touch(event.slot, { action = "capture" })
            sol.click(event.x, event.y)
        end
        return
    end
    if not event.window then return end
    if touch.focus then sol.focus(event.window) end
    if event.target == "frame" and not event.control and touch.move then
        sol.touch(event.slot, { action = "move", window = event.window })
    elseif event.target == "border" and touch.resize then
        sol.touch(event.slot, {
            action = "resize", window = event.window, edges = event.edges,
        })
    end
end)
