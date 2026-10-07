-- The preview shell is the shipped default: with nothing in `user.lua`,
-- `lua/preview/init.lua` fills in `config.shell.scene` before `shell.lua`
-- reads it, so a fresh install hosts the bar rather than a blank desktop.
-- See `script::tests::the_shipped_configuration_hosts_the_preview_shell_by_default`
-- for the same claim against the real shipped `init.lua`; this scenario is
-- the Lua-policy side of it, and is what a change to `preview.lua`'s own
-- logic -- as opposed to the shipped wiring -- would actually catch.
--
-- Played by `scenario::tests::every_scenario_with_a_client_passes`.

return {
    init = [[ require("preview.init"); require("shell") ]],
    steps = {
        {
            expect = function(world)
                assert(world.surfaces["shell"], "the preview shell was not hosted by default")
            end,
        },
    },
}
