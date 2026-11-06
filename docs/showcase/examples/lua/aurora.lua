-- The aurora round the focused window (effects/aurora), its head running
-- round the edge once every four seconds. A showcase example
-- (docs/showcase.md): copy this file into ~/.config/solium/ and the aurora
-- folder into ~/.config/solium/effects/, and give user.lua's `effects` the
-- rule:
--
--     effects = { motion_hz = 0, rules = { require("aurora").rule } },
--
-- An effect's params can be animated by handing its rules over again with
-- new values: the slots keep what they captured and only the chain runs
-- again (crates/solium/effects/README.md, "Rules"). The rule is in
-- `effects.rules` from the start, so it is handed over with the rest; a
-- timer on the compositor's clock turns its `phase`, and asks the default
-- configuration's looks.lua, which hands every rule over, to do so again.
local PERIOD = 4000

local aurora = {}

aurora.rule = {
    match = { focused = true, fullscreen = false },
    part = "pane",
    slot = "front",
    effect = { "aurora", phase = 0 },
}

local elapsed, ticking = 0, false

local function tick()
    elapsed = elapsed + 16
    aurora.rule.effect.phase = (elapsed % PERIOD) / PERIOD
    local looks = package.loaded.looks
    if looks and looks.refresh then
        looks.refresh()
    end
end

-- Once the desktop is up, and again after a reload.
local function start()
    if not ticking then
        ticking = true
        sol.every(16, tick)
    end
end
sol.on("monitors", start)
sol.on("restore", start)

return aurora
