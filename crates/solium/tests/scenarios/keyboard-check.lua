-- `keyboard.indicator` is known to `solium --check`, setting by setting: every
-- one `config.lua` documents is accepted beside the xkb names, and a misspelt
-- one is reported by its whole path, with the setting it was near.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

return {
    user = [[
        return {
            keyboard = {
                layout = "us,ru",
                indicator = {
                    show = "surface",
                    on = { layout = true, caps = false, num = true },
                    caps_on_focus = false,
                    fallback = false,
                    position = "top",
                    durration = 900,
                },
            },
        }
    ]],
    init = [[ require("keyboard_indicator") ]],
    steps = {
        {
            expect = function(world)
                local reported = {}
                for _, setting in ipairs(world.unknown) do
                    reported[#reported + 1] = setting.key .. " -> " .. tostring(setting.meant)
                end
                local all = table.concat(reported, ", ")
                assert(all == "keyboard.indicator.durration -> keyboard.indicator.duration",
                    "only the misspelt setting is reported, and with the one it was near: " .. all)
            end,
        },
    },
}
