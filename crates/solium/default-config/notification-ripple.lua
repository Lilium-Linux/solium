-- Circular notification ripple, selected in the real-window tuner.
local settings = { duration_ms = 1000, strength_px = 10, width_px = 180 }
local last = {}

local function preload()
    -- This build's user.lua schema predates effects.load. Set the engine
    -- option after config.lua has finished validating the user settings.
    local effects = require("config").effects
    effects.load = effects.load or {}
    local found = false
    for _, name in ipairs(effects.load) do if name == "notification-ripple" then found = true end end
    if not found then effects.load[#effects.load + 1] = "notification-ripple" end
    local looks = package.loaded.looks
    if looks and looks.refresh then looks.refresh() else sol.effects(effects) end
end
sol.on("monitors", preload)
sol.on("restore", preload)

sol.on("notifications", function(event)
    -- Only a new visible banner: DND/history-only notifications and an
    -- application's repeated updates keep their normal notification policy.
    local n = event.notification
    if event.what ~= "added" or not n or n.state ~= "banner" or n.window == 0 then return end
    for _, name in ipairs({ "overview", "recents", "deskstack" }) do
        local view = package.loaded[name]
        if view and view.active then return end
    end
    local workspaces = package.loaded.workspaces
    for _, w in ipairs(sol.windows()) do
        if w.id == n.window then
            if not w.shown or w.leaving or w.minimized or w.fullscreen then return end
            if workspaces and not workspaces.on_active(w.id, w.monitor) then return end
            local received = tonumber(n.received)
            local previous = last[w.id]
            if received and previous then
                local elapsed = received - previous
                if elapsed >= 0 and elapsed < settings.duration_ms then return end
            end
            last[w.id] = received
            sol.animate({ duration = settings.duration_ms, easing = "linear" })
            sol.present_from(w.id, {
                x = w.x, y = w.y, w = w.w, h = w.h,
                deform = { effect = "notification-ripple", progress = 1,
                    strength = settings.strength_px, width = settings.width_px,
                    to = { x = w.x, y = w.y, w = w.w, h = w.h } },
            })
            return
        end
    end
end)

sol.on("close", function(id) last[id] = nil end)
return settings
